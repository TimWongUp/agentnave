//! A second process must wait for the user's answer, not the single-instance plugin's exit 0.
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::time::{Duration, Instant};

#[cfg(windows)]
pub(super) const REQUEST: &str = "--agentnave-uninstall-request";
const HANDSHAKE: Duration = Duration::from_secs(120);

pub(super) fn token() -> Result<String, String> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

pub(super) struct Waiter {
    listener: TcpListener,
    token: String,
}

impl Waiter {
    pub(super) fn new() -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let token = token().map_err(io::Error::other)?;
        Ok(Self { listener, token })
    }

    pub(super) fn argument(&self) -> String {
        format!(
            "{}:{}",
            self.listener.local_addr().unwrap().port(),
            self.token
        )
    }

    pub(super) fn accept(&self) -> io::Result<(TcpStream, u32)> {
        let started = Instant::now();
        loop {
            if started.elapsed() >= HANDSHAKE {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "确认页未响应"));
            }
            match self.listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                    let mut auth = [0; 64];
                    if stream.read_exact(&mut auth).is_err() || auth != self.token.as_bytes() {
                        continue;
                    }
                    let mut pid = [0; 4];
                    stream.read_exact(&mut pid)?;
                    // User interaction has no time limit; EOF/crash remains a failure.
                    stream.set_read_timeout(None)?;
                    return Ok((stream, u32::from_be_bytes(pid)));
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(error) => return Err(error),
            }
        }
    }
}

pub(super) struct Reply(TcpStream);

impl Reply {
    pub(super) fn connect(argument: &str) -> io::Result<Self> {
        let (port, token) = argument
            .split_once(':')
            .ok_or(io::ErrorKind::InvalidInput)?;
        let port: u16 = port.parse().map_err(|_| io::ErrorKind::InvalidInput)?;
        if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
        let mut stream = TcpStream::connect_timeout(&address.into(), Duration::from_secs(2))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        stream.write_all(token.as_bytes())?;
        stream.write_all(&std::process::id().to_be_bytes())?;
        Ok(Self(stream))
    }

    pub(super) fn finish(mut self, confirmed: bool) {
        let _ = self.0.write_all(&[u8::from(confirmed)]);
    }
}

pub(super) fn outcome(stream: &mut TcpStream) -> io::Result<bool> {
    let mut result = [0];
    stream.read_exact(&mut result)?;
    match result[0] {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(io::ErrorKind::InvalidData.into()),
    }
}
