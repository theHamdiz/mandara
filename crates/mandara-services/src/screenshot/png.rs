//! Streaming PNG output with a bounded compressed clipboard payload.

use mandara_i18n::t;
use std::io::{self, Write};

/// Same order of magnitude as clipboard image capture; oversized output is an error.
pub const MAX_BYTES: usize = 32 << 20;
/// Reserve the retained byte storage before temporary text and painting allocations.
const INITIAL_BYTES: usize = 64 << 10;

/// A streaming PNG destination that refuses writes beyond the clipboard limit.
pub struct Output {
    bytes: Vec<u8>,
    limit: usize,
}

impl Default for Output {
    fn default() -> Self { Self::new() }
}

impl Output {
    /// Reserve persistent output storage before allocating temporary image pixels.
    pub fn new() -> Self {
        Self {
            bytes: Vec::with_capacity(INITIAL_BYTES),
            limit: MAX_BYTES,
        }
    }
    /// Hand off the encoded allocation without copying its bytes.
    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let length = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .filter(|length| *length <= self.limit)
            .ok_or_else(|| io::Error::other(t("The image is larger than the clipboard limit")))?;
        self.bytes
            .try_reserve(length - self.bytes.len())
            .map_err(io::Error::other)?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_writer_refuses_overflow_without_appending_partial_data() {
        let mut output = Output {
            bytes: vec![1, 2],
            limit: 4,
        };
        assert_eq!(output.write(&[3, 4]).unwrap(), 2);
        assert!(output.write(&[5]).is_err());
        assert_eq!(output.finish(), [1, 2, 3, 4]);
    }
}
