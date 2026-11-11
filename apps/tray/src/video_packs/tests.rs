use super::*;
use std::sync::{Arc, atomic::AtomicBool};

fn atom(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut bytes = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
    bytes.extend(kind);
    bytes.extend(payload);
    bytes
}

fn movie(edge: u32, version: u8) -> Vec<u8> {
    let offset = if version == 0 { 76 } else { 88 };
    let mut header = vec![0; offset];
    header[0] = version;
    header.extend((edge << 16).to_be_bytes());
    header.extend((edge << 16).to_be_bytes());
    atom(b"moov", &atom(b"trak", &atom(b"tkhd", &header)))
}

fn ebml_el(id: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut out = id.to_vec();
    let len = payload.len();
    assert!(len < 0x3FFF);
    if len < 0x7F {
        out.push(0x80 | len as u8);
    } else {
        out.extend(((0x4000 | len) as u16).to_be_bytes());
    }
    out.extend(payload);
    out
}

fn ebml_u16(id: &[u8], value: u16) -> Vec<u8> {
    ebml_el(id, &value.to_be_bytes())
}

fn webm_header(doctype: &[u8]) -> Vec<u8> {
    let doc = ebml_el(&[0x42, 0x82], doctype);
    ebml_el(&[0x1A, 0x45, 0xDF, 0xA3], &doc)
}

fn webm_with_doctype(doctype: &[u8], tracks: &[u8]) -> Vec<u8> {
    let header = webm_header(doctype);
    let segment = ebml_el(
        &[0x18, 0x53, 0x80, 0x67],
        &ebml_el(&[0x16, 0x54, 0xAE, 0x6B], tracks),
    );
    [header, segment].concat()
}

fn webm_clip(edge: u32) -> Vec<u8> {
    let mut video = ebml_u16(&[0xB0], edge as u16);
    video.extend(ebml_u16(&[0xBA], edge as u16));
    let mut entry = ebml_el(&[0x83], &[1]);
    entry.extend(ebml_el(&[0xE0], &video));
    webm_with_doctype(b"webm", &ebml_el(&[0xAE], &entry))
}

fn clip_bytes(edge: u32) -> Vec<u8> {
    if CLIP_EXT == "mov" {
        movie(edge, 0)
    } else {
        webm_clip(edge)
    }
}

fn poster() -> Vec<u8> {
    let mut bytes = vec![0; 30];
    bytes[..4].copy_from_slice(b"RIFF");
    bytes[4..8].copy_from_slice(&22u32.to_le_bytes());
    bytes[8..16].copy_from_slice(b"WEBPVP8X");
    bytes[24..27].copy_from_slice(&[31, 2, 0]);
    bytes[27..30].copy_from_slice(&[31, 2, 0]);
    bytes
}

fn pack(root: &Path, tier: &str) {
    let edge = tier.parse().unwrap();
    for file in files() {
        let output = root.join(file);
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        let bytes = if output.extension().unwrap() == "webp" {
            poster()
        } else {
            clip_bytes(edge)
        };
        fs::write(output, bytes).unwrap();
    }
}

#[test]
fn quicktime_dimensions_handle_versions_atom_sizes_and_truncation() {
    for version in [0, 1] {
        let bytes = movie(272, version);
        assert_eq!(install::mov_size(&bytes), Some((272, 272)));
        for length in 0..bytes.len() {
            assert_eq!(install::mov_size(&bytes[..length]), None);
        }
        let mut extended = 1u32.to_be_bytes().to_vec();
        extended.extend(b"moov");
        extended.extend(((bytes.len() + 8) as u64).to_be_bytes());
        extended.extend(&bytes[8..]);
        assert_eq!(install::mov_size(&extended), Some((272, 272)));
        let mut remainder = bytes.clone();
        remainder[..4].fill(0);
        assert_eq!(install::mov_size(&remainder), Some((272, 272)));
        let mut invalid = bytes.clone();
        invalid[..4].copy_from_slice(&4u32.to_be_bytes());
        assert_eq!(install::mov_size(&invalid), None);
    }
    assert_eq!(install::mov_size(&movie(0, 0)), None);
}

#[test]
fn webm_dimensions_handle_vint_sizes_doctype_and_truncation() {
    let bytes = webm_clip(272);
    assert_eq!(webm::webm_size(&bytes), Some((272, 272)));
    for length in 0..bytes.len() {
        assert_eq!(webm::webm_size(&bytes[..length]), None);
    }
    let header = webm_header(b"webm");
    let seg_payload = &bytes[header.len() + 5..];
    let mut wide = header.clone();
    wide.extend([0x18, 0x53, 0x80, 0x67, 0x01]);
    wide.extend(&(seg_payload.len() as u64).to_be_bytes()[1..]);
    wide.extend(seg_payload);
    assert_eq!(webm::webm_size(&wide), Some((272, 272)));
    let mut unknown = bytes.clone();
    unknown[header.len() + 4] = 0xFF;
    assert_eq!(webm::webm_size(&unknown), Some((272, 272)));
    for length in 0..unknown.len() {
        assert_eq!(webm::webm_size(&unknown[..length]), None);
    }
    let mut streamed = header.clone();
    streamed.extend([
        0x18, 0x53, 0x80, 0x67, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    ]);
    streamed.extend(seg_payload);
    let tracks_end = streamed.len();
    streamed.extend([
        0x1F, 0x43, 0xB6, 0x75, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xE7, 0x81, 0x00,
    ]);
    assert_eq!(webm::webm_size(&streamed), Some((272, 272)));
    for length in (0..tracks_end).chain(tracks_end + 1..tracks_end + 12) {
        assert_eq!(webm::webm_size(&streamed[..length]), None);
    }
    let mut truncated_cluster = unknown.clone();
    truncated_cluster.extend([0x1F, 0x43, 0xB6, 0x75, 0x84, 0, 0]);
    assert_eq!(webm::webm_size(&truncated_cluster), None);
    let mut cluster_before_tracks = header.clone();
    cluster_before_tracks.extend([0x18, 0x53, 0x80, 0x67, 0xFF, 0x1F, 0x43, 0xB6, 0x75, 0xFF]);
    cluster_before_tracks.extend(seg_payload);
    assert_eq!(webm::webm_size(&cluster_before_tracks), None);
    let mut bad_tracks = bytes.clone();
    bad_tracks[header.len() + 9] = 0xFF;
    assert_eq!(webm::webm_size(&bad_tracks), None);
    let mut multi = ebml_el(&[0xAE], &ebml_el(&[0x83], &[2]));
    multi.extend(&bytes[header.len() + 10..]);
    assert_eq!(
        webm::webm_size(&webm_with_doctype(b"webm", &multi)),
        Some((272, 272))
    );
    assert_eq!(
        webm::webm_size(&webm_with_doctype(b"matroska", &multi)),
        None
    );
    assert_eq!(webm::webm_size(&webm_clip(0)), None);
}

#[test]
fn import_validates_all_files_before_replacing_pack() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("chosen/wrapper");
    let base = dir.path().join("video-packs");
    pack(&source, "272");
    let staged = install::stage(&base, "272", source.parent().unwrap()).unwrap();
    install::swap(&base, "272", staged).unwrap();
    let target = base.join("272");
    let before = fs::read(target.join("stamp")).unwrap();
    assert_eq!(files().count(), 221);
    assert_eq!(
        crate::characters::webp_size(&fs::read(target.join("codex/idle.webp")).unwrap()),
        Some((544, 544))
    );
    assert!(
        install::stage(&base, "544", &source)
            .unwrap_err()
            .contains("272×272")
    );
    let missing = source.join(format!("default/sleep-1.{CLIP_EXT}"));
    fs::remove_file(missing).unwrap();
    assert!(install::stage(&base, "272", &source).is_err());
    assert_eq!(fs::read(target.join("stamp")).unwrap(), before);
    pack(&source, "544");
    let staged = install::stage(&base, "544", &source).unwrap();
    install::swap(&base, "544", staged).unwrap();
    assert!(base.join("544/stamp").is_file());
}

#[test]
fn preview_resolution_never_mixes_pack_files() {
    let shared = tempfile::tempdir().unwrap();
    let preview = tempfile::tempdir().unwrap();
    let real = shared.path().join("video-packs/272");
    let local = preview.path().join("video-packs/272");
    fs::create_dir_all(real.join("claude")).unwrap();
    fs::create_dir_all(local.join("claude")).unwrap();
    fs::write(real.join("stamp"), "real").unwrap();
    let other_ext = if CLIP_EXT == "mov" { "webm" } else { "mov" };
    let wrong = real.join(format!("claude/idle.{other_ext}"));
    fs::write(wrong, b"wrong").unwrap();
    assert_eq!(
        resolve_with_preview(shared.path(), "272", Some(preview.path())),
        None
    );
    let sentinel = format!("claude/idle.{CLIP_EXT}");
    fs::write(real.join(&sentinel), b"ok").unwrap();
    assert_eq!(
        resolve_with_preview(shared.path(), "272", Some(preview.path())),
        Some((real, "shared"))
    );
    fs::write(local.join("stamp"), "preview").unwrap();
    fs::write(local.join(&sentinel), b"ok").unwrap();
    assert_eq!(
        resolve_with_preview(shared.path(), "272", Some(preview.path())),
        Some((local, "preview"))
    );
}

#[test]
fn routes_reject_unsafe_paths_and_require_write_authorization() {
    let root = tempfile::tempdir().unwrap();
    for rest in [
        format!("../codex/idle.{CLIP_EXT}"),
        "272/../stamp".to_owned(),
        "272/codex/../../stamp".to_owned(),
        format!("272/unknown/idle.{CLIP_EXT}"),
        "272/codex/slack-1.webp".to_owned(),
    ] {
        assert!(asset(root.path(), &rest, None).is_none());
    }
    for operation in ["import", "download", "cancel", "delete"] {
        let path = format!("/app/video-packs/272/{operation}");
        assert_eq!(
            request(root.path(), None, "GET", &path, Some("1"), b"").status,
            405
        );
        assert_eq!(
            request(root.path(), None, "POST", &path, None, b"").status,
            403
        );
    }
    let reader = crate::data::Reader::new(root.path().to_owned());
    assert_eq!(reader.respond("/app/video-packs").status, 200);
    assert_eq!(
        reader
            .respond(&format!("/art/video/272/codex/idle.{CLIP_EXT}"))
            .status,
        404
    );
}

#[test]
fn archives_use_the_folder_validator_and_reject_links() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    pack(&source, "272");
    let archive = root.path().join("pack.tar.gz");
    let encoder = flate2::write::GzEncoder::new(
        fs::File::create(&archive).unwrap(),
        flate2::Compression::fast(),
    );
    let mut builder = tar::Builder::new(encoder);
    builder.append_dir_all("video", &source).unwrap();
    builder.into_inner().unwrap().finish().unwrap();
    let extracted = root.path().join("extracted");
    fs::create_dir_all(&extracted).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    install::extract(&archive, &extracted, &cancel).unwrap();
    assert!(install::stage(&root.path().join("installed"), "272", &extracted).is_ok());
    let encoder = flate2::write::GzEncoder::new(
        fs::File::create(&archive).unwrap(),
        flate2::Compression::fast(),
    );
    let mut builder = tar::Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    header.set_mode(0o644);
    builder.append_link(&mut header, "link", "outside").unwrap();
    builder.into_inner().unwrap().finish().unwrap();
    assert!(install::extract(&archive, &extracted, &cancel).is_err());
}

#[test]
fn import_request_serves_installed_assets_and_delete_removes_the_pack() {
    // Preview resolution is tested with injected temporary roots; do not touch app data here.
    if crate::PREVIEW {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    pack(&source, "272");
    let body = json!({"path": source}).to_string();
    let response = request(
        root.path(),
        None,
        "POST",
        "/app/video-packs/272/import",
        Some("1"),
        body.as_bytes(),
    );
    assert_eq!(
        response.status,
        200,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    let state: Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(state["ext"], CLIP_EXT);
    assert_eq!(state["packs"]["272"]["installed"], true);
    assert_eq!(state["packs"]["272"]["source"], "shared");
    assert!(state["packs"]["272"]["bytes"].as_u64().unwrap() > 0);
    let reader = crate::data::Reader::new(root.path().to_owned());
    let clip_url = format!("/art/video/272/codex/idle.{CLIP_EXT}");
    let response = reader.respond(&clip_url);
    assert_eq!(response.content_type, CLIP_MIME);
    assert_eq!(response.cache_control, "no-store");
    assert_eq!(response.body, clip_bytes(272));
    let poster = reader.respond("/art/video/272/codex/idle.webp");
    assert_eq!(poster.cache_control, "public, max-age=31536000");
    assert_eq!(
        request(
            root.path(),
            None,
            "POST",
            "/app/video-packs/272/delete",
            Some("1"),
            b""
        )
        .status,
        200
    );
    assert!(!root.path().join("video-packs/272").exists());
    assert_eq!(status(root.path())["packs"]["272"]["installed"], false);
}

#[test]
fn wrong_poster_dimensions_and_oversized_files_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    pack(&source, "272");
    let mut bytes = poster();
    bytes[24] = 0;
    fs::write(source.join("claude/idle.webp"), bytes).unwrap();
    let base = root.path().join("installed");
    assert!(install::stage(&base, "272", &source).is_err());
    fs::File::create(source.join("claude/idle.webp"))
        .unwrap()
        .set_len(2 * 1024 * 1024 + 1)
        .unwrap();
    assert!(
        install::stage(&base, "272", &source)
            .unwrap_err()
            .contains("大小限制")
    );
    assert!(!base.join("272").exists());
}

#[test]
fn clips_are_served_in_ranges_and_never_cached() {
    if crate::PREVIEW {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("video-packs/272");
    pack(&dir, "272");
    fs::write(dir.join("stamp"), "1").unwrap();
    let bytes: Vec<u8> = (0..100).collect();
    let clip_rel = format!("codex/idle.{CLIP_EXT}");
    let route = format!("272/{clip_rel}");
    fs::write(dir.join(&clip_rel), &bytes).unwrap();
    let get = |range: Option<&str>| asset(root.path(), &route, range).unwrap();
    let header = |r: &Response, name: &str| {
        r.headers
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.clone())
    };

    let whole = get(None);
    assert_eq!((whole.status, whole.body.len()), (200, 100));
    assert_eq!(whole.cache_control, "no-store");
    assert_eq!(header(&whole, "Accept-Ranges").as_deref(), Some("bytes"));
    assert_eq!(header(&whole, "Content-Range"), None);

    for (range, from, to) in [
        ("bytes=10-19", 10, 19),
        ("bytes=90-", 90, 99),
        ("bytes=-5", 95, 99),
        ("bytes=95-500", 95, 99),
        ("bytes=0-0", 0, 0),
        ("bytes=-500", 0, 99),
    ] {
        let part = get(Some(range));
        assert_eq!(part.status, 206, "{range}");
        assert_eq!(part.body, bytes[from..=to], "{range}");
        assert_eq!(
            header(&part, "Content-Range"),
            Some(format!("bytes {from}-{to}/100")),
            "{range}"
        );
        assert_eq!(part.cache_control, "no-store");
    }

    for range in ["bytes=100-", "bytes=500-600", "bytes=-0"] {
        let refused = get(Some(range));
        assert_eq!(refused.status, 416, "{range}");
        assert!(refused.body.is_empty());
        assert_eq!(
            header(&refused, "Content-Range").as_deref(),
            Some("bytes */100")
        );
    }

    // Ranges this server does not understand are answered with the whole clip.
    for range in [
        "bytes=0-1,5-6",
        "items=0-1",
        "bytes=9-3",
        "bytes=a-b",
        "bytes=",
    ] {
        let whole = get(Some(range));
        assert_eq!((whole.status, whole.body.len()), (200, 100), "{range}");
    }
}
