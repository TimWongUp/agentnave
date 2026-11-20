use ruzstd::decoding::{FrameDecoder, StreamingDecoder};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};

/// Decodes a zstd file made of several frames (DeepSeek Harness appends one per
/// batch). A frame still being written ends the stream; its complete lines are kept.
pub(super) struct Frames {
    source: Option<BufReader<File>>,
    frame: Option<StreamingDecoder<BufReader<File>, FrameDecoder>>,
}

impl Frames {
    pub(super) fn new(file: File) -> Self {
        Self {
            source: Some(BufReader::new(file)),
            frame: None,
        }
    }
}

impl Read for Frames {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if let Some(frame) = &mut self.frame {
                match frame.read(buf) {
                    Ok(0) => {
                        self.source = self.frame.take().map(StreamingDecoder::into_inner);
                    }
                    Ok(n) => return Ok(n),
                    Err(_) => {
                        self.frame = None;
                        return Ok(0);
                    }
                }
                continue;
            }
            let Some(source) = self.source.as_mut() else {
                return Ok(0);
            };
            if source.fill_buf()?.is_empty() {
                self.source = None;
                return Ok(0);
            }
            match self.source.take().map(StreamingDecoder::new) {
                Some(Ok(frame)) => self.frame = Some(frame),
                _ => return Ok(0),
            }
        }
    }
}
