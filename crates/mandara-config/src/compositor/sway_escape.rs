//! Sway's argument unescaping precedes include word expansion.

use anyhow::{Result, ensure};

pub(super) fn assignment(raw: &str) -> Result<String> {
    ensure!(
        raw.len() <= crate::keybinds::MAX_FILE_BYTES,
        mandara_i18n::t("The shortcut configuration is too large to edit.")
    );
    let mut bytes = Vec::with_capacity(raw.len());
    for word in super::sway_words::arguments(raw) {
        if !bytes.is_empty() {
            bytes.push(b' ');
        }
        let start = bytes.len();
        bytes.extend_from_slice(word.as_bytes());
        argument(&mut bytes, start)?;
    }
    String::from_utf8(bytes).map_err(|_| {
        anyhow::anyhow!(mandara_i18n::t(
            "Computed shortcuts must be edited in their source configuration."
        ))
    })
}

/// Decode only the appended argument, preserving earlier joined arguments.
pub(super) fn argument(bytes: &mut Vec<u8>, start: usize) -> Result<()> {
    let unsupported =
        || mandara_i18n::t("Computed shortcuts must be edited in their source configuration.");
    ensure!(!bytes[start..].contains(&0), unsupported());
    let (mut read, mut written) = (start, start);
    while read < bytes.len() {
        let byte = bytes[read];
        read += 1;
        if byte != b'\\' || read == bytes.len() {
            bytes[written] = byte;
            written += 1;
            continue;
        }
        let next = bytes[read];
        read += 1;
        let decoded = match next {
            b'0' => anyhow::bail!(unsupported()),
            b'a' => 7,
            b'b' => 8,
            b'f' => 12,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'v' => 11,
            b'\\' | b'\'' | b'"' | b'?' => next,
            b'x' => {
                // Sway 1.12 consumes at most two decimal digits in this form.
                let mut value = 0;
                for _ in 0..2 {
                    let Some(&digit) = bytes.get(read).filter(|byte| byte.is_ascii_digit()) else {
                        break;
                    };
                    value = value * 16 + digit - b'0';
                    read += 1;
                }
                if value == 0 {
                    continue;
                }
                value
            }
            _ => {
                bytes[written] = b'\\';
                written += 1;
                next
            }
        };
        bytes[written] = decoded;
        written += 1;
    }
    bytes.truncate(written);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_argument_escapes_are_decoded_once() {
        let mut bytes = br#"prefix \"a\t\x20\?\$\q\\b"#.to_vec();
        argument(&mut bytes, "prefix ".len()).unwrap();
        assert_eq!(bytes, b"prefix \"a\t ?\\$\\q\\b");
        for input in [&b"name\0suffix"[..], &br"name\tpart\0suffix"[..]] {
            assert!(
                argument(&mut input.to_vec(), 0).is_err(),
                "NUL-truncated arguments cannot authorize partial conflict discovery"
            );
        }
    }
}
