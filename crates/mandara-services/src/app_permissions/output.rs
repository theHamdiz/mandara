//! Remove Flatpak's terminal escaping before parsing its metadata key file.

use anyhow::{Result, ensure};

pub(super) fn decode(text: String) -> Result<String> {
    let invalid = || mandara_i18n::t("The Flatpak permission data is invalid or too large.");
    ensure!(text.len() as u64 <= super::native::MAX_BYTES, invalid());
    let mut bytes = text.into_bytes();
    let (mut read, mut written) = (0, 0);
    while read < bytes.len() {
        if bytes[read] != b'\\' {
            bytes[written] = bytes[read];
            read += 1;
            written += 1;
            continue;
        }
        read += 1;
        if bytes.get(read) == Some(&b'\\') {
            bytes[written] = b'\\';
            read += 1;
            written += 1;
            continue;
        }
        let digits = match bytes.get(read) {
            Some(b'x') => 2,
            Some(b'u') => 4,
            Some(b'U') => 8,
            _ => anyhow::bail!(invalid()),
        };
        read += 1;
        let end = read + digits;
        let number = bytes
            .get(read..end)
            .and_then(|hex| {
                hex.iter().try_fold(0_u32, |value, byte| {
                    char::from(*byte)
                        .to_digit(16)
                        .map(|digit| value * 16 + digit)
                })
            })
            .and_then(char::from_u32)
            .filter(|ch| *ch != '\0')
            .ok_or_else(|| anyhow::anyhow!(invalid()))?;
        // An encoded scalar never needs more bytes than its escape occupied.
        let mut encoded = [0; 4];
        let encoded = number.encode_utf8(&mut encoded).as_bytes();
        bytes[written..written + encoded.len()].copy_from_slice(encoded);
        written += encoded.len();
        read = end;
    }
    bytes.truncate(written);
    Ok(String::from_utf8(bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_and_key_file_escaping_are_separate_layers() {
        let printed = r"[Context]
filesystems=/tmp/A\\;B;/tmp/C\\\\:D:ro;/tmp/E\\\\\\\\F:ro;
";
        let metadata = decode(printed.into()).unwrap();
        let permissions = crate::app_install::flatpak::parse_context(&metadata).unwrap();
        assert_eq!(
            permissions.filesystems,
            [r"/tmp/A;B", r"/tmp/C\:D:ro", r"/tmp/E\\F:ro"]
        );
        assert_eq!(
            decode(r"a\x09b\u200Ec\U000E0001d\\n".into()).unwrap(),
            "a\tb\u{200e}c\u{e0001}d\\n"
        );
        assert_eq!(decode("مرحبا\n".into()).unwrap(), "مرحبا\n");
        for invalid in [
            r"\",
            r"\q",
            r"\xG0",
            r"\u123",
            r"\U00110000",
            r"\uD800",
            r"\x00",
        ] {
            assert!(decode(invalid.into()).is_err(), "{invalid}");
        }
    }
}
