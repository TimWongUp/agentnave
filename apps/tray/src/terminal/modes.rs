//! Tracks whether the CLI subscribes to appearance-change notices (DECSET/DECRST 2031) by reading
//! the private-mode sequences in its output. A sequence may carry several modes
//! (`ESC [ ? 2004 ; 2031 h`) and may be split across reads, so the parser keeps its place.

const APPEARANCE: &[u8] = b"2031";
/// A mode list longer than this is not one the CLI sets.
const LONGEST: usize = 64;

#[derive(Default)]
enum State {
    #[default]
    Ground,
    Escape,
    Bracket,
    /// Inside `ESC [ ?`, with the digits and semicolons so far.
    Modes(Vec<u8>),
}

#[derive(Default)]
pub(super) struct Tracker {
    state: State,
    subscribed: bool,
    paste: bool,
}

impl Tracker {
    pub(super) fn appearance(&self) -> bool {
        self.subscribed
    }

    pub(super) fn paste(&self) -> bool {
        self.paste
    }

    pub(super) fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.step(byte);
        }
    }

    fn step(&mut self, byte: u8) {
        self.state = match (std::mem::take(&mut self.state), byte) {
            (_, 0x1b) => State::Escape,
            (State::Escape, b'[') => State::Bracket,
            (State::Bracket, b'?') => State::Modes(Vec::new()),
            (State::Modes(mut modes), b'0'..=b'9' | b';') if modes.len() < LONGEST => {
                modes.push(byte);
                State::Modes(modes)
            }
            (State::Modes(modes), b'h' | b'l') => {
                if modes.split(|&b| b == b';').any(|mode| mode == APPEARANCE) {
                    self.subscribed = byte == b'h';
                }
                if modes.split(|&b| b == b';').any(|mode| mode == b"2004") {
                    self.paste = byte == b'h';
                }
                State::Ground
            }
            _ => State::Ground,
        };
    }
}
