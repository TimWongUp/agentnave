//! Tracks human drafts and holds user input while a wakeup is being submitted.
use std::time::{Duration, Instant};

struct Held {
    chars: usize,
    inputs: Vec<(Vec<u8>, bool)>,
}

#[derive(Default)]
pub(super) struct Gate {
    last: Option<Instant>,
    draft: usize,
    sequence: Vec<u8>,
    pasting: bool,
    held: Option<Held>,
    #[cfg(test)]
    draft_wait: Option<Duration>,
}

impl Gate {
    pub fn input(&mut self, bytes: &[u8], paste: bool) -> bool {
        self.last = Some(Instant::now());
        if let Some(held) = &mut self.held {
            held.inputs.push((bytes.to_vec(), paste));
            true
        } else {
            self.track_input(bytes, paste);
            false
        }
    }

    fn track_input(&mut self, bytes: &[u8], paste: bool) {
        if paste
            && !self.pasting
            && self.sequence.is_empty()
            && !bytes.starts_with(b"\x1b[200~")
            && !b"\x1b[200~".starts_with(bytes)
        {
            self.draft = self
                .draft
                .saturating_add(bytes.iter().filter(|b| **b & 0xc0 != 0x80).count());
        } else {
            self.track(bytes);
        }
    }

    pub fn ready(&self) -> bool {
        let quiet = if self.draft == 0 {
            Duration::from_secs(3)
        } else {
            self.draft_delay()
        };
        self.held.is_none() && self.last.is_none_or(|at| at.elapsed() >= quiet)
    }

    fn draft_delay(&self) -> Duration {
        #[cfg(test)]
        if let Some(delay) = self.draft_wait {
            return delay;
        }
        Duration::from_secs(300)
    }

    #[cfg(test)]
    pub fn set_draft_wait(&mut self, delay: Duration) {
        self.draft_wait = Some(delay);
    }

    #[cfg(test)]
    pub fn is_holding(&self) -> bool {
        self.held.is_some()
    }

    pub fn hold(&mut self, chars: usize) {
        self.held = Some(Held {
            chars,
            inputs: Vec::new(),
        });
    }

    pub fn release(&mut self, commit: bool) -> (usize, Vec<u8>) {
        let Some(held) = self.held.take() else {
            return (0, Vec::new());
        };
        if commit {
            self.draft = 0;
        }
        let mut bytes = Vec::new();
        for (input, paste) in held.inputs {
            self.track_input(&input, paste);
            bytes.extend(input);
        }
        (held.chars, bytes)
    }

    fn key(&mut self, code: u32, modifiers: u32) {
        let modifiers = modifiers.saturating_sub(1) & !0xc0;
        match (code, modifiers + 1) {
            (13 | 3 | 21, 1) | (99 | 117, 5) => self.draft = 0,
            (8 | 127, 1) => self.draft = self.draft.saturating_sub(1),
            (32..=0x10ffff, 1 | 2)
                if !(57344..=63743).contains(&code)
                    && char::from_u32(code).is_some_and(|c| !c.is_control()) =>
            {
                self.draft = self.draft.saturating_add(1);
            }
            _ => {}
        }
    }

    fn escape(&mut self) {
        let seq = std::mem::take(&mut self.sequence);
        if seq == b"\x1b[200~" {
            self.pasting = true;
            return;
        }
        let Some(body) = seq.strip_prefix(b"\x1b[") else {
            return;
        };
        if let Some(body) = body.strip_suffix(b"u") {
            let Ok(body) = std::str::from_utf8(body) else {
                return;
            };
            let mut params = body.split(';');
            let code = params
                .next()
                .and_then(|p| p.split(':').next()?.parse().ok());
            let mut mods = params.next().unwrap_or("").split(':');
            let modifier = mods
                .next()
                .filter(|s| !s.is_empty())
                .unwrap_or("1")
                .parse()
                .ok();
            if mods.next() != Some("3")
                && let (Some(code), Some(modifier)) = (code, modifier)
            {
                self.key(code, modifier);
            }
        } else if let Some(body) = body.strip_suffix(b"_") {
            self.win32(body);
        }
    }

    fn win32(&mut self, body: &[u8]) {
        let Ok(body) = std::str::from_utf8(body) else {
            return;
        };
        let mut params = [0u32, 0, 0, 0, 0, 1];
        let mut fields = body.split(';');
        for (param, field) in params.iter_mut().zip(&mut fields) {
            if !field.is_empty() {
                let Ok(value) = field.parse() else { return };
                *param = value;
            }
        }
        if fields.next().is_some() {
            return;
        }
        let [_, _, code, down, control, repeat] = params;
        if down == 0 || repeat == 0 || (0xdc00..=0xdfff).contains(&code) {
            return;
        }
        let code = if (0xd800..=0xdbff).contains(&code) {
            0xfffd
        } else {
            code
        };
        let altgr = control & 3 != 0 && control & 12 != 0 && code >= 32;
        let modifiers = if altgr {
            1
        } else {
            1 + u32::from(control & 16 != 0)
                + 2 * u32::from(control & 3 != 0)
                + 4 * u32::from(control & 12 != 0 && !matches!(code, 3 | 21))
        };
        let before = self.draft;
        self.key(code, modifiers);
        let extra = (repeat - 1) as usize;
        if self.draft > before {
            self.draft = self.draft.saturating_add(extra);
        } else if self.draft < before && matches!(code, 8 | 127) {
            self.draft = self.draft.saturating_sub(extra);
        }
    }

    fn track(&mut self, bytes: &[u8]) {
        for (index, &byte) in bytes.iter().enumerate() {
            if self.pasting {
                self.paste_byte(byte);
            } else if self.sequence == b"\x1b"
                && (byte == 0x1b || (index == 0 && (byte < 0x20 || byte == 0x7f)))
            {
                self.sequence.clear();
                if byte == 0x1b {
                    self.sequence.push(byte);
                } else {
                    self.key(byte.into(), 1);
                }
            } else if byte == 0x1b || !self.sequence.is_empty() {
                self.sequence.push(byte);
                let complete = match self.sequence.as_slice() {
                    [0x1b] | [0x1b, b'[' | b'O'] => false,
                    [0x1b, b'[', b'M', ..] => self.sequence.len() >= 6,
                    [0x1b, b'[', ..] => (0x40..=0x7e).contains(&byte),
                    _ => true,
                };
                if complete || self.sequence.len() >= 128 {
                    self.escape();
                }
            } else if byte < 0x80 {
                self.key(byte.into(), 1);
            } else if byte & 0xc0 != 0x80 {
                self.draft = self.draft.saturating_add(1);
            }
        }
    }

    fn paste_byte(&mut self, byte: u8) {
        if byte != 0x1b && self.sequence.is_empty() {
            self.draft = self.draft.saturating_add(usize::from(byte & 0xc0 != 0x80));
            return;
        }
        self.sequence.push(byte);
        if self.sequence == b"\x1b[201~" {
            self.pasting = false;
            self.sequence.clear();
        } else if !b"\x1b[201~".starts_with(&self.sequence) {
            let restart = byte == 0x1b;
            self.draft = self.draft.saturating_add(
                self.sequence.iter().filter(|b| **b & 0xc0 != 0x80).count() - usize::from(restart),
            );
            self.sequence.clear();
            if restart {
                self.sequence.push(byte);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn counts_text_paste_and_edit_keys() {
        let mut g = Gate::default();
        g.input("你好".as_bytes(), false);
        assert_eq!(g.draft, 2);
        g.input(b"\x7f\x08\x7f", false);
        assert_eq!(g.draft, 0);
        g.input(b"a\rb", true);
        assert_eq!(g.draft, 3);
        g.last = Some(Instant::now() - Duration::from_secs(61));
        assert!(!g.ready());
        g.input(b"\x15", false);
        assert_eq!(g.draft, 0);
        g.input(b"\x1b[20", false);
        g.input(b"0~a\rb\x1b[201~", false);
        assert_eq!(g.draft, 3);
        g.input(b"\r", false);
        assert_eq!(g.draft, 0);
        g.input(b"x\x03", false);
        assert_eq!(g.draft, 0);
    }
    #[test]
    fn draft_waits_five_minutes_since_latest_user_input() {
        let mut g = Gate::default();
        g.input(b"draft", false);
        g.last = Some(Instant::now() - Duration::from_secs(299));
        assert!(!g.ready());
        g.last = Some(Instant::now() - Duration::from_secs(300));
        assert!(g.ready());
        g.input(b"x", false);
        assert!(!g.ready());
        g.last = Some(Instant::now() - Duration::from_secs(299));
        assert!(!g.ready());
        g.last = Some(Instant::now() - Duration::from_secs(300));
        assert!(g.ready());
        g.hold(1);
        assert!(!g.ready());
        g.input(b"new", false);
        assert_eq!(g.release(true).1, b"new");
        assert_eq!(g.draft, 3);
        g.input(b"\r", false);
        g.last = Some(Instant::now() - Duration::from_secs(2));
        assert!(!g.ready());
        g.last = Some(Instant::now() - Duration::from_secs(3));
        assert!(g.ready());
    }

    #[test]
    fn cancelled_wakeup_retains_old_draft_and_applies_held_edits() {
        let mut gate = Gate::default();
        gate.input(b"old", false);
        gate.hold(2);
        gate.input(b"\x7fxy", false);
        let (chars, held) = gate.release(false);
        assert_eq!(chars, 2);
        assert_eq!(held, b"\x7fxy");
        assert_eq!(gate.draft, 4);
        gate.hold(2);
        gate.input(b"z", false);
        assert_eq!(gate.release(true).1, b"z");
        assert_eq!(gate.draft, 1);
    }

    #[test]
    fn tracks_csi_u_and_win32_only_when_keys_type() {
        let mut g = Gate::default();
        g.input(
            b"\x1b[97;1:1u\x1b[97;1:2u\x1b[97;1:3u\x1b[A\x1b[120;3u",
            false,
        );
        assert_eq!(g.draft, 2);
        g.input(b"\x1b[127u\x1b[0;0;233;1;0;3_", false);
        assert_eq!(g.draft, 4);
        g.input(b"\x1b[99;5u", false);
        assert_eq!(g.draft, 0);
        assert!(!g.ready());
    }
}
