//! Mozilla native messages: native-endian length followed by UTF-8 JSON.

use anyhow::{Result, ensure};
use serde_json::Value;

/// Mozilla limits host-to-browser messages to one MiB. Use the same bound in both directions.
pub(super) const MAX_MESSAGE: usize = 1024 * 1024;

#[derive(Default)]
pub(super) struct Decoder {
    prefix: [u8; 4],
    prefix_read: usize,
    remaining: usize,
    body: Vec<u8>,
}

impl Decoder {
    pub fn push(
        &mut self,
        mut input: &[u8],
        mut message: impl FnMut(Value) -> Result<()>,
    ) -> Result<()> {
        while !input.is_empty() {
            if self.prefix_read != self.prefix.len() {
                let count = input.len().min(self.prefix.len() - self.prefix_read);
                self.prefix[self.prefix_read..self.prefix_read + count]
                    .copy_from_slice(&input[..count]);
                input = &input[count..];
                self.prefix_read += count;
                if self.prefix_read != self.prefix.len() {
                    continue;
                }
                self.remaining = u32::from_ne_bytes(self.prefix) as usize;
                ensure!(
                    self.remaining > 0 && self.remaining <= MAX_MESSAGE,
                    "native message has an invalid length"
                );
                self.body.clear();
                self.body.reserve(self.remaining);
            }
            let count = input.len().min(self.remaining);
            self.body.extend_from_slice(&input[..count]);
            input = &input[count..];
            self.remaining -= count;
            if self.remaining == 0 {
                message(serde_json::from_slice(&self.body)?)?;
                self.prefix_read = 0;
            }
        }
        Ok(())
    }
}

pub(super) fn write(output: &mut impl std::io::Write, value: &Value) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(bytes.len() <= MAX_MESSAGE, "native message is too large");
    output.write_all(&(bytes.len() as u32).to_ne_bytes())?;
    output.write_all(&bytes)?;
    output.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fragmented_and_adjacent_messages_keep_their_framing() {
        let values = [json!({"action":"action:colors"}), json!({"text":"العربية"})];
        let mut bytes = Vec::new();
        for value in &values {
            write(&mut bytes, value).unwrap();
        }
        let mut decoder = Decoder::default();
        let mut received = Vec::new();
        for byte in bytes {
            decoder
                .push(&[byte], |value| {
                    received.push(value);
                    Ok(())
                })
                .unwrap();
        }
        assert_eq!(received, values);
        assert!(
            Decoder::default()
                .push(&((MAX_MESSAGE + 1) as u32).to_ne_bytes(), |_| Ok(()))
                .is_err()
        );
        assert!(
            Decoder::default()
                .push(&0_u32.to_ne_bytes(), |_| Ok(()))
                .is_err()
        );
    }
}
