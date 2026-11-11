//! Loopback HTTP for character clips. WebKit plays an `http://127.0.0.1` movie from memory after
//! one load, while a custom scheme makes it ask for small ranges on every frame.
use super::asset;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

static BASE: OnceLock<String> = OnceLock::new();
/// Connections being served; the players need a handful, anything past this is dropped.
static OPEN: AtomicUsize = AtomicUsize::new(0);
const MAX_OPEN: usize = 32;
const MAX_REQUEST: u64 = 16 * 1024;

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
    // A connection that never finishes its request does not keep a thread forever.
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    // The request line and headers together stay within MAX_REQUEST bytes.
    let mut reader = BufReader::new(stream.try_clone()?.take(MAX_REQUEST));
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let mut range = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header.trim().is_empty() {
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
        .and_then(|rest| asset(shared, rest, range.as_deref()));
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
        "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n",
        response.status,
        response.content_type,
        response.body.len()
    );
    for (name, value) in &response.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    out.write_all(head.as_bytes())?;
    if method == "GET" {
        out.write_all(&response.body)?;
    }
    out.flush()
}
