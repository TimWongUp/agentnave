//! Minimal VT screen view for Claude Code Ink cursor positioning and erases.

const SCREEN_ROWS: usize = 50;
const SCREEN_COLS: usize = 200;

enum AnsiMatch {
    Complete(usize),
    Incomplete,
    Invalid,
}

fn match_ansi(buf: &[u8]) -> AnsiMatch {
    if buf.len() < 2 {
        return AnsiMatch::Incomplete;
    }
    match buf[1] {
        b'[' => {
            let mut i = 2;
            while i < buf.len() && (0x30..=0x3f).contains(&buf[i]) {
                i += 1;
            }
            while i < buf.len() && (0x20..=0x2f).contains(&buf[i]) {
                i += 1;
            }
            if i >= buf.len() {
                AnsiMatch::Incomplete
            } else if (0x40..=0x7e).contains(&buf[i]) {
                AnsiMatch::Complete(i + 1)
            } else {
                AnsiMatch::Invalid
            }
        }
        b']' => match_string_escape(buf, true),
        b'P' => match_string_escape(buf, false),
        _ => {
            let mut i = 1;
            while i < buf.len() && (0x20..=0x2f).contains(&buf[i]) {
                i += 1;
            }
            if i >= buf.len() {
                AnsiMatch::Incomplete
            } else if (0x30..=0x7e).contains(&buf[i]) {
                AnsiMatch::Complete(i + 1)
            } else {
                AnsiMatch::Invalid
            }
        }
    }
}

fn match_string_escape(buf: &[u8], allow_bel: bool) -> AnsiMatch {
    let mut i = 2;
    while i < buf.len() {
        if allow_bel && buf[i] == 0x07 {
            return AnsiMatch::Complete(i + 1);
        }
        if buf[i] == 0x1b {
            if i + 1 >= buf.len() {
                return AnsiMatch::Incomplete;
            }
            if buf[i + 1] == b'\\' {
                return AnsiMatch::Complete(i + 2);
            }
            return AnsiMatch::Invalid;
        }
        i += 1;
    }
    AnsiMatch::Incomplete
}

/// Minimal VT screen view for Ink cursor positioning and erases.
pub(in super::super) struct ClaudeScreen {
    lines: Vec<Vec<char>>,
    row: usize,
    col: usize,
    pending: Vec<u8>,
}

impl ClaudeScreen {
    pub(in super::super) fn new() -> Self {
        Self {
            lines: vec![vec![' '; SCREEN_COLS]; SCREEN_ROWS],
            row: 0,
            col: 0,
            pending: Vec::new(),
        }
    }

    pub(in super::super) fn feed(&mut self, data: &[u8]) {
        self.pending.extend_from_slice(data);
        let mut pos = 0;
        while pos < self.pending.len() {
            if self.pending[pos] == 0x1b {
                match match_ansi(&self.pending[pos..]) {
                    AnsiMatch::Complete(len) => {
                        let code =
                            String::from_utf8_lossy(&self.pending[pos..pos + len]).into_owned();
                        self.escape(&code);
                        pos += len;
                    }
                    AnsiMatch::Incomplete if self.pending.len() - pos < 256 => break,
                    AnsiMatch::Incomplete | AnsiMatch::Invalid => pos += 1,
                }
                continue;
            }
            let next_esc = self.pending[pos..]
                .iter()
                .position(|&b| b == 0x1b)
                .map_or(self.pending.len(), |off| pos + off);
            if !self.feed_utf8_segment(pos, next_esc, &mut pos) {
                break;
            }
        }
        if pos > 0 {
            self.pending.drain(..pos);
        }
    }

    fn feed_utf8_segment(&mut self, start: usize, end: usize, pos: &mut usize) -> bool {
        match std::str::from_utf8(&self.pending[start..end]) {
            Ok(valid) => {
                let chars: Vec<char> = valid.chars().collect();
                for ch in chars {
                    self.character(ch);
                }
                *pos = end;
                true
            }
            Err(err) => {
                let valid_len = err.valid_up_to();
                if valid_len > 0 {
                    let chars: Vec<char> =
                        std::str::from_utf8(&self.pending[start..start + valid_len])
                            .expect("valid UTF-8 prefix")
                            .chars()
                            .collect();
                    for ch in chars {
                        self.character(ch);
                    }
                    *pos = start + valid_len;
                }
                match err.error_len() {
                    None if end == self.pending.len() => false,
                    None => {
                        self.character('\u{FFFD}');
                        *pos = end;
                        true
                    }
                    Some(bad_len) => {
                        self.character('\u{FFFD}');
                        *pos += bad_len;
                        true
                    }
                }
            }
        }
    }

    fn character(&mut self, ch: char) {
        match ch {
            '\r' => self.col = 0,
            '\n' => self.row += 1,
            '\x08' => self.col = self.col.saturating_sub(1),
            c if c >= ' ' && c != '\x7f' => {
                if self.col >= SCREEN_COLS {
                    self.col = 0;
                    self.row += 1;
                }
                self.scroll();
                self.lines[self.row][self.col] = c;
                self.col += 1;
            }
            _ => {}
        }
        self.scroll();
    }

    fn scroll(&mut self) {
        if self.row >= SCREEN_ROWS {
            self.lines.remove(0);
            self.lines.push(vec![' '; SCREEN_COLS]);
            self.row = SCREEN_ROWS - 1;
        }
    }

    fn escape(&mut self, code: &str) {
        let Some(rest) = code.strip_prefix("\x1b[") else {
            return;
        };
        let Some(command) = rest.chars().last() else {
            return;
        };
        let params = &rest[..rest.len() - command.len_utf8()];
        if !params.is_empty() && !params.bytes().all(|b| b.is_ascii_digit() || b == b';') {
            return;
        }
        let values: Vec<usize> = params
            .split(';')
            .map(|v| {
                if v.is_empty() {
                    0
                } else {
                    v.parse().unwrap_or(0)
                }
            })
            .collect();
        let n = values[0].max(1);
        match command {
            'H' | 'f' => {
                let col_val = values.get(1).copied().unwrap_or(1).max(1);
                self.row = (n - 1).min(SCREEN_ROWS - 1);
                self.col = (col_val - 1).min(SCREEN_COLS - 1);
            }
            'A' => self.row = self.row.saturating_sub(n),
            'B' => self.row = (self.row + n).min(SCREEN_ROWS - 1),
            'C' => self.col = (self.col + n).min(SCREEN_COLS - 1),
            'D' => self.col = self.col.saturating_sub(n),
            'G' => self.col = (n - 1).min(SCREEN_COLS - 1),
            'J' => {
                let p = values[0];
                let start = if matches!(p, 1..=3) { 0 } else { self.row };
                let end = if p == 1 { self.row + 1 } else { SCREEN_ROWS };
                for row in start..end {
                    self.lines[row].fill(' ');
                }
            }
            'K' => {
                let p = values[0];
                let start = if matches!(p, 1 | 2) {
                    0
                } else {
                    self.col.min(SCREEN_COLS)
                };
                let end = if p == 1 {
                    (self.col + 1).min(SCREEN_COLS)
                } else {
                    SCREEN_COLS
                };
                self.lines[self.row][start..end].fill(' ');
            }
            _ => {}
        }
    }

    pub(in super::super) fn text(&self) -> String {
        self.lines
            .iter()
            .map(|row| row.iter().collect::<String>().trim_end().to_owned())
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_owned()
    }
}
