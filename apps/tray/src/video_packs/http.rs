//! Loopback HTTP for character clips. WebKit plays an `http://127.0.0.1` movie from memory after
//! one load, while a custom scheme makes it ask for small ranges on every frame.
use super::stream_asset;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

static BASE: OnceLock<String> = OnceLock::new();
/// Connections being served; the players need a handful, anything past this is dropped.
static OPEN: AtomicUsize = AtomicUsize::new(0);
const MAX_OPEN: usize = 32;
const MAX_REQUEST: u64 = 16 * 1024;
/// A client that dribbles its request head may not keep one of the OPEN slots for long.
const HEAD_TIMEOUT: Duration = Duration::from_secs(5);
/// A player that stops reading its clip frees the slot once the socket buffer stays full.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// `http://127.0.0.1:<port>/<token>`, once the listener is up.
pub fn base() -> Option<&'static str> {
    BASE.get().map(String::as_str)
}

pub fn start(shared: PathBuf) {
    let listener = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(listener) => listener,
        Err(error) => return eprintln!("无法启动视频本机端口：{error}"),
    };
    let Ok(port) = listener.local_addr().map(|addr| addr.port()) else {
        return;
    };
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        return;
    }
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let _ = BASE.set(format!("http://127.0.0.1:{port}/{token}"));
    let shared = Arc::new(shared);
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            if OPEN.fetch_add(1, Ordering::Relaxed) >= MAX_OPEN {
                OPEN.fetch_sub(1, Ordering::Relaxed);
                continue;
            }
            let shared = shared.clone();
            let prefix = format!("/{token}/");
            thread::spawn(move || {
                let _ = serve(&shared, &prefix, stream);
                OPEN.fetch_sub(1, Ordering::Relaxed);
            });
        }
    });
}

/// One request per connection: GET or HEAD of `/<token>/<tier>/<character>/<clip>.mov`.
fn serve(shared: &Path, prefix: &str, stream: TcpStream) -> std::io::Result<()> {
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    stream.set_read_timeout(Some(HEAD_TIMEOUT))?;
    // The request line and headers together stay within MAX_REQUEST bytes.
    let mut reader = stream.try_clone()?.take(MAX_REQUEST);
    let head = read_head(&stream, &mut reader, Instant::now())?;
    let mut lines = head.split("\r\n");
    let mut parts = lines.next().unwrap_or_default().split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let mut range = None;
    for header in lines {
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.trim().eq_ignore_ascii_case("range")
        {
            range = Some(value.trim().to_owned());
        }
    }
    let path = target.split('?').next().unwrap_or("");
    let found = matches!(method, "GET" | "HEAD")
        .then(|| path.strip_prefix(prefix))
        .flatten()
        .filter(|rest| rest.ends_with(".mov"))
        .and_then(|rest| stream_asset(shared, rest, range.as_deref()));
    let mut out = stream;
    let Some(response) = found else {
        return out.write_all(
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
    };
    let reason = match response.status {
        206 => "Partial Content",
        416 => "Range Not Satisfiable",
        _ => "OK",
    };
    let mut head = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: {}\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n",
        response.status, response.content_type, response.content_length, response.cache_control
    );
    for (name, value) in &response.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    out.write_all(head.as_bytes())?;
    if method == "GET"
        && let Some((file, length)) = response.file
    {
        std::io::copy(&mut file.take(length), &mut out)?;
    }
    out.flush()
}

/// The request head, completed before `HEAD_TIMEOUT`; a slow or trickling client is cut off
/// instead of holding its thread and slot.
fn read_head(
    stream: &TcpStream,
    reader: &mut impl Read,
    started: Instant,
) -> std::io::Result<String> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 2048];
    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
        let remaining = HEAD_TIMEOUT
            .checked_sub(started.elapsed())
            .filter(|remaining| !remaining.is_zero())
            .ok_or(std::io::ErrorKind::TimedOut)?;
        stream.set_read_timeout(Some(remaining))?;
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
