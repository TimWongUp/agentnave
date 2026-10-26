//! Replacement office characters per model family, kept in the shared data directory.
//! A set must carry every frame the painted characters have, as WebP at the same size.
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use crate::data::Response;
use serde_json::{Map, Value};

static WRITE: Mutex<()> = Mutex::new(());
pub const FAMILIES: [&str; 9] = [
    "claude", "gpt", "gemini", "grok", "deepseek", "glm", "qwen", "kimi", "minimax",
];
const STATES: [&str; 10] = [
    "idle", "blink", "working", "speaking", "review", "done", "sleep", "sad", "walk-1", "walk-2",
];
/// Frame sizes and how many of STATES each has; heads have no sad or walking frames.
const SIZES: [(&str, u32, usize); 3] = [("s", 266, 10), ("m", 320, 10), ("h", 96, 7)];
const FRAME_LIMIT: usize = 1024 * 1024;

/// Every frame in upload order: `(size, state, edge)`.
fn frames() -> impl Iterator<Item = (&'static str, &'static str, u32)> {
    SIZES
        .iter()
        .flat_map(|&(size, edge, count)| STATES[..count].iter().map(move |&s| (size, s, edge)))
}

fn webp_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 30 || &b[0..4] != b"RIFF" || &b[8..12] != b"WEBP" {
        return None;
    }
    let u16le = |at: usize| u32::from(b[at]) | u32::from(b[at + 1]) << 8;
    let u24le = |at: usize| u16le(at) | u32::from(b[at + 2]) << 16;
    match &b[12..16] {
        b"VP8X" => Some((1 + u24le(24), 1 + u24le(27))),
        b"VP8 " if b[23..26] == [0x9d, 0x01, 0x2a] => {
            Some((u16le(26) & 0x3fff, u16le(28) & 0x3fff))
        }
        b"VP8L" if b[20] == 0x2f => {
            let bits = u32::from_le_bytes([b[21], b[22], b[23], b[24]]);
            Some((1 + (bits & 0x3fff), 1 + (bits >> 14 & 0x3fff)))
        }
        _ => None,
    }
}

/// `GET /app/characters`: the families with a replacement, each with a version for cache busting.
pub fn list(root: &Path) -> Value {
    let mut out = Map::new();
    for family in FAMILIES {
        if let Ok(stamp) = fs::read_to_string(root.join("characters").join(family).join("stamp")) {
            out.insert(family.into(), stamp.trim().into());
        }
    }
    Value::Object(out)
}

/// `GET /character/<family>/<size>/<state>.webp`; the page adds `?v=<stamp>`, so it caches long.
pub fn frame(root: &Path, path: &str) -> Option<Response> {
    let [family, size, file] = path.split('/').collect::<Vec<_>>()[..] else {
        return None;
    };
    let state = file.strip_suffix(".webp")?;
    if !FAMILIES.contains(&family) || !frames().any(|(z, s, _)| z == size && s == state) {
        return None;
    }
    let bytes = fs::read(root.join("characters").join(family).join(size).join(file)).ok()?;
    let mut response = Response::ok("image/webp", bytes);
    response.cache_control = "public, max-age=31536000";
    Some(response)
}

/// `POST /app/characters/<family>`: the frames in `frames()` order, each a big-endian u32 length
/// and its bytes. An empty body restores the painted character.
pub fn request(
    root: &Path,
    family: &str,
    method: &str,
    action: Option<&str>,
    body: &[u8],
) -> Response {
    if method != "POST" {
        return Response::error(405, "此操作只接受 POST");
    }
    if action != Some("1") {
        return Response::error(403, "缺少操作请求头");
    }
    if !FAMILIES.contains(&family) {
        return Response::error(404, "没有这个模型家族");
    }
    let mut parts = Vec::new();
    let mut rest = body;
    for (size, state, edge) in frames() {
        if body.is_empty() {
            break;
        }
        let Some((head, tail)) = rest.split_first_chunk::<4>() else {
            return Response::error(400, "小人图片不完整");
        };
        let len = u32::from_be_bytes(*head) as usize;
        if len > FRAME_LIMIT || len > tail.len() {
            return Response::error(400, &format!("{size}/{state}.webp 超过 1 MB 或不完整"));
        }
        let (bytes, tail) = tail.split_at(len);
        if webp_size(bytes) != Some((edge, edge)) {
            return Response::error(
                400,
                &format!("{size}/{state}.webp 必须是 {edge}×{edge} 的 WebP"),
            );
        }
        parts.push((size, state, bytes));
        rest = tail;
    }
    if !rest.is_empty() {
        return Response::error(400, "小人图片多出了内容");
    }
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let dir = root.join("characters");
    let save = || -> std::io::Result<()> {
        fs::create_dir_all(&dir)?;
        // Build the new set beside the old one, then swap, so a figure never mixes two sets.
        let staged = tempfile::tempdir_in(&dir)?;
        for (size, state, bytes) in &parts {
            fs::create_dir_all(staged.path().join(size))?;
            let mut file =
                fs::File::create(staged.path().join(size).join(format!("{state}.webp")))?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        fs::write(staged.path().join("stamp"), stamp.to_string())?;
        let old = tempfile::tempdir_in(&dir)?;
        let target = dir.join(family);
        if target.exists() {
            fs::rename(&target, old.path().join("set"))?;
        }
        if !parts.is_empty() {
            fs::rename(staged.keep(), &target)?;
        }
        Ok(())
    };
    match save() {
        Ok(()) => Response::json(&list(root)),
        Err(error) => Response::error(500, &format!("无法保存小人：{error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn webp(edge: u32) -> Vec<u8> {
        let mut b = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
        b.extend_from_slice(&(edge - 1).to_le_bytes()[..3]);
        b.extend_from_slice(&(edge - 1).to_le_bytes()[..3]);
        b
    }

    fn set(edge_for: impl Fn(u32) -> u32) -> Vec<u8> {
        let mut body = Vec::new();
        for (_, _, edge) in frames() {
            let bytes = webp(edge_for(edge));
            body.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            body.extend_from_slice(&bytes);
        }
        body
    }

    #[test]
    fn a_full_matching_set_replaces_and_resets_one_family() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(frames().count(), 27);
        assert_eq!(request(root, "qwen", "POST", None, &set(|e| e)).status, 403);
        assert_eq!(
            request(root, "other", "POST", Some("1"), &set(|e| e)).status,
            404
        );
        let wrong = request(root, "qwen", "POST", Some("1"), &set(|e| e + 1));
        assert_eq!(wrong.status, 400);
        let mut short = set(|e| e);
        short.truncate(short.len() - 10);
        assert_eq!(request(root, "qwen", "POST", Some("1"), &short).status, 400);
        assert_eq!(list(root), json!({}));

        assert_eq!(
            request(root, "qwen", "POST", Some("1"), &set(|e| e)).status,
            200
        );
        assert!(list(root)["qwen"].is_string());
        let served = frame(root, "qwen/h/sleep.webp").unwrap();
        assert_eq!(served.body, webp(96));
        assert!(frame(root, "qwen/h/sad.webp").is_none());
        assert!(frame(root, "qwen/../stamp").is_none());

        assert_eq!(request(root, "qwen", "POST", Some("1"), &[]).status, 200);
        assert_eq!(list(root), json!({}));
        assert!(frame(root, "qwen/s/idle.webp").is_none());
    }
}
