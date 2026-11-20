use ruzstd::decoding::{FrameDecoder, StreamingDecoder};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};

const MAX_DECOMPRESSED: u64 = 512 * 1024 * 1024;

/// Decodes a zstd file made of several frames (DeepSeek Harness appends one per
/// batch). A frame still being written ends the stream; its complete lines are kept.
pub(super) struct Frames {
    source: Option<BufReader<File>>,
    frame: Option<StreamingDecoder<BufReader<File>, FrameDecoder>>,
    emitted: u64,
    limit: u64,
}

impl Frames {
    pub(super) fn new(file: File) -> Self {
        Self::with_limit(file, MAX_DECOMPRESSED)
    }

    pub(super) fn with_limit(file: File, limit: u64) -> Self {
        Self {
            source: Some(BufReader::new(file)),
            frame: None,
            emitted: 0,
            limit,
        }
    }
}

impl Read for Frames {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if let Some(frame) = &mut self.frame {
                if self.emitted >= self.limit {
                    let mut probe = [0u8; 1];
                    match frame.read(&mut probe) {
                        Ok(0) => {
                            self.source = self.frame.take().map(StreamingDecoder::into_inner);
                            continue;
                        }
                        Ok(_) => {
                            self.frame = None;
                            return Err(io::Error::other("压缩日志解压大小超过上限"));
                        }
                        Err(_) => {
                            self.frame = None;
                            return Ok(0);
                        }
                    }
                }
                let allowed = buf.len().min((self.limit - self.emitted) as usize);
                match frame.read(&mut buf[..allowed]) {
                    Ok(0) => {
                        self.source = self.frame.take().map(StreamingDecoder::into_inner);
                    }
                    Ok(n) => {
                        self.emitted += n as u64;
                        return Ok(n);
                    }
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
