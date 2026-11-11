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
            movie(edge, 0)
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
    fs::remove_file(source.join("default/sleep-1.mov")).unwrap();
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
    fs::create_dir_all(&real).unwrap();
    fs::create_dir_all(&local).unwrap();
    fs::write(real.join("stamp"), "real").unwrap();
    assert_eq!(
        resolve_with_preview(shared.path(), "272", Some(preview.path())),
        Some((real, "shared"))
    );
    fs::write(local.join("stamp"), "preview").unwrap();
    assert_eq!(
        resolve_with_preview(shared.path(), "272", Some(preview.path())),
        Some((local, "preview"))
    );
}

#[test]
fn routes_reject_unsafe_paths_and_require_write_authorization() {
    let root = tempfile::tempdir().unwrap();
    for rest in [
        "../codex/idle.mov",
        "272/../stamp",
        "272/codex/../../stamp",
        "272/unknown/idle.mov",
        "272/codex/slack-1.webp",
    ] {
        assert!(asset(root.path(), rest, None).is_none());
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
    assert_eq!(reader.respond("/art/video/272/codex/idle.mov").status, 404);
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
    assert_eq!(state["packs"]["272"]["installed"], true);
    assert_eq!(state["packs"]["272"]["source"], "shared");
    assert!(state["packs"]["272"]["bytes"].as_u64().unwrap() > 0);
    let reader = crate::data::Reader::new(root.path().to_owned());
    let response = reader.respond("/art/video/272/codex/idle.mov");
    assert_eq!(response.content_type, "video/quicktime");
    assert_eq!(response.cache_control, "no-store");
    assert_eq!(response.body, movie(272, 0));
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
    fs::write(dir.join("codex/idle.mov"), &bytes).unwrap();
    let get = |range: Option<&str>| asset(root.path(), "272/codex/idle.mov", range).unwrap();
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
