//! Conservative tracking of user edits, independent of terminal replies.
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct Gate {
    last: Option<Instant>,
    draft: bool,
    generation: u64,
}
impl Gate {
    pub fn input(&mut self, bytes: &[u8], paste: bool) {
        self.last = Some(Instant::now());
        self.generation += 1;
        if paste && !bytes.is_empty() {
            self.draft = true;
            return;
        }
        let mut escape = false;
        for &b in bytes {
            if b == 0x1b {
                escape = true;
                continue;
            }
            if escape {
                if (0x40..=0x7e).contains(&b) && b != b'[' {
                    escape = false;
                }
                continue;
            }
            match b {
                b'\r' | b'\n' | 3 | 21 => self.draft = false,
                0x20..=0xff if b != 0x7f => self.draft = true,
                _ => {}
            }
        }
    }
    pub fn ready(&self) -> bool {
        !self.draft
            && self
                .last
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(3))
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn draft_survives_edits_until_submit_or_clear() {
        let mut g = Gate::default();
        assert!(g.ready());
        g.input("你好".as_bytes(), false);
        assert!(g.draft);
        g.input(b"\x1b[D\x7f", false);
        assert!(g.draft);
        g.input(b"\x15", false);
        assert!(!g.draft);
        g.input(b"paste\r", true);
        assert!(g.draft);
        g.input(b"\r", false);
        assert!(!g.draft);
        assert!(!g.ready());
    }
}
