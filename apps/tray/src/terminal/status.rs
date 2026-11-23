//! Streaming OSC 7501 reports, consumed once from the live PTY stream.

const PREFIX: &[u8] = b"\x1b]7501;";
pub(super) const REPLY: &[u8] =
    b"\x1b]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth\x1b\\";
const MAX: usize = 4087;

#[derive(Default)]
pub(super) struct Tracker {
    sequence: Vec<u8>,
    pub state: Option<String>,
    pub kind: Option<String>,
    pub seq: u64,
    pub last_working_seq: u64,
    pub last_done_seq: u64,
    pub last_blocked_seq: u64,
}

impl Tracker {
    pub fn feed(&mut self, bytes: &[u8]) -> usize {
        let mut queries = 0;
        for &byte in bytes {
            if self.sequence.is_empty() {
                if byte == 0x1b {
                    self.sequence.push(byte);
                }
                continue;
            }
            self.sequence.push(byte);
            if self.sequence.len() <= PREFIX.len() {
                if !PREFIX.starts_with(&self.sequence) {
                    self.sequence.clear();
                    if byte == 0x1b {
                        self.sequence.push(byte);
                    }
                }
                continue;
            }
            let ended = byte == 7 || self.sequence.ends_with(b"\x1b\\");
            if ended {
                let end = self.sequence.len() - if byte == 7 { 1 } else { 2 };
                let body = String::from_utf8_lossy(&self.sequence[PREFIX.len()..end]);
                if body.trim() == "?" {
                    queries += 1;
                } else {
                    let pairs: Vec<_> = body.split(':').filter_map(|p| p.split_once('=')).collect();
                    // Child records do not describe the CLI's own input state.
                    if !pairs
                        .iter()
                        .any(|(k, v)| k.trim() == "id" && !v.trim().is_empty())
                        && let Some(state) = pairs
                            .iter()
                            .find_map(|(k, v)| (k.trim() == "state").then_some(v.trim()))
                    {
                        if ["idle", "working", "done", "blocked", "error"].contains(&state) {
                            self.state = Some(state.to_owned());
                            self.kind = pairs.iter().find_map(|(k, v)| {
                                (k.trim() == "kind").then(|| v.trim().to_owned())
                            });
                            self.seq += 1;
                            match state {
                                "working" => self.last_working_seq = self.seq,
                                "done" => self.last_done_seq = self.seq,
                                "blocked" => self.last_blocked_seq = self.seq,
                                _ => {}
                            }
                        } else if state == "clear" {
                            self.state = None;
                            self.kind = None;
                            self.seq += 1;
                            self.last_working_seq = 0;
                            self.last_done_seq = 0;
                            self.last_blocked_seq = 0;
                        }
                    }
                }
                self.sequence.clear();
            } else if self.sequence.len() > PREFIX.len() + MAX + 1 {
                self.sequence.clear();
            }
        }
        queries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_reports_and_queries() {
        let mut t = Tracker::default();
        assert_eq!(t.feed(b"\x1b]750"), 0);
        assert_eq!(
            t.feed(b"1;state=blocked:kind=question\x07\x1b]7501;?\x1b"),
            0
        );
        assert_eq!(t.feed(b"\\"), 1);
        assert_eq!(t.state.as_deref(), Some("blocked"));
        assert_eq!(t.kind.as_deref(), Some("question"));
        assert_eq!(t.feed(b"\x1b]7501;state=working\x1b\\"), 0);
        assert_eq!(t.state.as_deref(), Some("working"));
    }
}
