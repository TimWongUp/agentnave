//! Minimal RFC 6455 WebSocket client over a Unix domain socket for Codex `app-server`.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex};

use base64::Engine;

const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

pub(super) struct WebSocketReader {
    stream: BufReader<UnixStream>,
    writer: Arc<Mutex<UnixStream>>,
}

pub(super) fn connect(path: &Path) -> Result<(WebSocketReader, Arc<Mutex<UnixStream>>), String> {
    let mut stream = UnixStream::connect(path).map_err(|e| format!("connect {path:?}: {e}"))?;
    let mut key_bytes = [0u8; 16];
    getrandom::fill(&mut key_bytes).map_err(|e| format!("random key: {e}"))?;
    let key = base64::engine::general_purpose::STANDARD.encode(key_bytes);
    let request = format!(
        "GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .and_then(|()| stream.flush())
        .map_err(|e| format!("handshake write: {e}"))?;

    let mut reader = BufReader::new(
        stream
            .try_clone()
            .map_err(|e| format!("clone stream: {e}"))?,
    );
    let headers = read_http_headers(&mut reader)?;
    verify_handshake(&headers, &key)?;

    let writer = Arc::new(Mutex::new(stream));
    Ok((
        WebSocketReader {
            stream: reader,
            writer: writer.clone(),
        },
        writer,
    ))
}

fn read_http_headers(reader: &mut BufReader<UnixStream>) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    let mut total = 0usize;
    loop {
        let mut line = String::new();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| format!("handshake read: {e}"))?;
        if n == 0 {
            return Err("unexpected EOF during WebSocket handshake".to_owned());
        }
        total += n;
        if total > MAX_HEADER_BYTES {
            return Err("WebSocket handshake headers too large".to_owned());
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            return Ok(lines);
        }
        lines.push(trimmed.to_owned());
    }
}

fn verify_handshake(lines: &[String], key: &str) -> Result<(), String> {
    let status = lines
        .first()
        .ok_or_else(|| "empty WebSocket handshake response".to_owned())?;
    if !status.contains(" 101 ") {
        return Err(format!("expected 101 Switching Protocols, got: {status}"));
    }
    let expected =
        base64::engine::general_purpose::STANDARD.encode(sha1(&format!("{key}{WS_GUID}")));
    let accept = lines.iter().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("sec-websocket-accept")
            .then(|| value.trim())
    });
    if accept != Some(expected.as_str()) {
        return Err("invalid Sec-WebSocket-Accept header".to_owned());
    }
    Ok(())
}

pub(super) fn send_text(writer: &Mutex<UnixStream>, text: &str) -> Result<(), String> {
    send_frame(writer, 0x1, text.as_bytes())
}

fn send_frame(writer: &Mutex<UnixStream>, opcode: u8, payload: &[u8]) -> Result<(), String> {
    let mut frame = Vec::with_capacity(14 + payload.len());
    frame.push(0x80 | opcode);
    let len = payload.len();
    if len < 126 {
        frame.push(0x80 | (len as u8));
    } else if len <= u16::MAX as usize {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        frame.push(0x80 | 127);
        frame.extend_from_slice(&(len as u64).to_be_bytes());
    }
    let mut mask = [0u8; 4];
    getrandom::fill(&mut mask).map_err(|e| format!("random mask: {e}"))?;
    frame.extend_from_slice(&mask);
    for (index, &byte) in payload.iter().enumerate() {
        frame.push(byte ^ mask[index % 4]);
    }
    let mut stream = writer.lock().unwrap_or_else(|p| p.into_inner());
    stream
        .write_all(&frame)
        .and_then(|()| stream.flush())
        .map_err(|e| format!("ws write: {e}"))
}

impl WebSocketReader {
    pub(super) fn read_text(&mut self) -> Result<Option<String>, String> {
        let mut message = Vec::new();
        let mut reading_fragments = false;
        loop {
            let mut head = [0u8; 2];
            if let Err(err) = self.stream.read_exact(&mut head) {
                if err.kind() == std::io::ErrorKind::UnexpectedEof && !reading_fragments {
                    return Ok(None);
                }
                return Err(format!("ws read header: {err}"));
            }
            let fin = head[0] & 0x80 != 0;
            let opcode = head[0] & 0x0f;
            let masked = head[1] & 0x80 != 0;
            let len = read_payload_len(&mut self.stream, head[1] & 0x7f)?;
            let mut mask = [0u8; 4];
            if masked {
                self.stream
                    .read_exact(&mut mask)
                    .map_err(|e| format!("ws read mask: {e}"))?;
            }
            let mut payload = vec![0u8; len];
            self.stream
                .read_exact(&mut payload)
                .map_err(|e| format!("ws read payload: {e}"))?;
            if masked {
                for (index, byte) in payload.iter_mut().enumerate() {
                    *byte ^= mask[index % 4];
                }
            }
            match opcode {
                0x8 => return Ok(None),
                0x9 => {
                    let _ = send_frame(&self.writer, 0xA, &payload);
                }
                0xA => {}
                0x0..=0x2 => {
                    if message.len().saturating_add(payload.len()) > MAX_FRAME_BYTES {
                        return Err("WebSocket message too large".to_owned());
                    }
                    message.extend_from_slice(&payload);
                    reading_fragments = !fin;
                    if fin {
                        let text =
                            String::from_utf8(message).map_err(|e| format!("utf8 frame: {e}"))?;
                        return Ok(Some(text));
                    }
                }
                other => return Err(format!("unexpected WebSocket opcode: {other}")),
            }
        }
    }
}

fn read_payload_len(stream: &mut BufReader<UnixStream>, tag: u8) -> Result<usize, String> {
    let len = match tag {
        0..=125 => usize::from(tag),
        126 => {
            let mut buf = [0u8; 2];
            stream
                .read_exact(&mut buf)
                .map_err(|e| format!("ws read len16: {e}"))?;
            usize::from(u16::from_be_bytes(buf))
        }
        127 => {
            let mut buf = [0u8; 8];
            stream
                .read_exact(&mut buf)
                .map_err(|e| format!("ws read len64: {e}"))?;
            usize::try_from(u64::from_be_bytes(buf))
                .map_err(|_| "frame length overflows usize".to_owned())?
        }
        _ => unreachable!(),
    };
    if len > MAX_FRAME_BYTES {
        return Err(format!("WebSocket frame too large: {len}"));
    }
    Ok(len)
}

/// SHA-1 for RFC 6455 `Sec-WebSocket-Accept` verification without an extra crate.
pub(super) fn sha1(input: &str) -> [u8; 20] {
    let bytes = input.as_bytes();
    let bit_len = (bytes.len() as u64).wrapping_mul(8);
    let mut msg = Vec::with_capacity(bytes.len() + 72);
    msg.extend_from_slice(bytes);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    for chunk in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 80];
        for (index, &word) in chunk.as_chunks::<4>().0.iter().enumerate() {
            w[index] = u32::from_be_bytes(word);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDCu32),
                _ => (b ^ c ^ d, 0xCA62C1D6u32),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (index, word) in h.iter().enumerate() {
        out[index * 4..(index + 1) * 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}
