//! Decode the key-file list layer without consuming native filesystem escapes.

use super::Permissions;

/// Keep installed-app metadata and imported bundles within a finite grant table.
const MAX_VALUES: usize = 4096;
/// Context metadata shares the bundle scanner's bounded input size.
const MAX_BYTES: usize = super::BUNDLE_SCAN_BYTES as usize;

/// Parses a metadata key file's `[Context]` group.
pub fn parse_context(text: &str) -> Option<Permissions> {
    parse(text, false)
}

/// Include revoked names so their controls remain available for a later edit.
pub(crate) fn context_domain(text: &str) -> Option<Permissions> {
    parse(text, true)
}

fn parse(text: &str, include_revoked: bool) -> Option<Permissions> {
    if text.len() > MAX_BYTES {
        return None;
    }
    let mut inside = false;
    let mut seen = false;
    let mut permissions = Permissions::default();
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == "[Context]";
            seen |= inside;
            continue;
        }
        let Some((key, value)) = line.split_once('=').filter(|_| inside) else {
            continue;
        };
        let field = match key.trim() {
            "shared" => &mut permissions.shared,
            "sockets" => &mut permissions.sockets,
            "devices" => &mut permissions.devices,
            "filesystems" => &mut permissions.filesystems,
            _ => continue,
        };
        *field = list(value, include_revoked)?;
    }
    seen.then_some(permissions)
}

fn list(raw: &str, include_revoked: bool) -> Option<Vec<String>> {
    let mut fields = Vec::new();
    let mut values = 0;
    let mut start = 0;
    let mut escaped = false;
    let mut append = |raw: &str| {
        let raw = raw.trim();
        if raw.is_empty() {
            return Some(());
        }
        values += 1;
        if values > MAX_VALUES {
            return None;
        }
        let mut decoded = decode(raw)?;
        if decoded.starts_with('!') {
            if !include_revoked {
                return Some(());
            }
            decoded.remove(0);
        }
        fields.push(decoded);
        Some(())
    };
    for (index, ch) in raw.char_indices() {
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == ';' {
            append(&raw[start..index])?;
            start = index + 1;
        }
    }
    append(&raw[start..])?;
    Some(fields)
}

fn decode(raw: &str) -> Option<String> {
    let mut value = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(ch) = chars.next() {
        value.push(if ch == '\\' {
            match chars.next()? {
                's' => ' ',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '\\' => '\\',
                ';' => ';',
                _ => return None,
            }
        } else {
            ch
        });
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filesystem_lists_decode_separators_once_and_keep_native_flag_escapes() {
        let text = "[Context]\nfilesystems=/tmp/A\\;B;~/C\\\\:D:ro;~/E\\\\\\\\F;!xdg-music;\nsockets=wayland;\n";
        let permissions = parse_context(text).unwrap();
        assert_eq!(
            permissions.filesystems,
            ["/tmp/A;B", "~/C\\:D:ro", "~/E\\\\F"]
        );
        assert_eq!(permissions.sockets, ["wayland"]);
        assert_eq!(
            context_domain(text).unwrap().filesystems.last().unwrap(),
            "xdg-music"
        );
        assert_eq!(
            list(r"\sone;two\tthree;", true).unwrap(),
            [" one", "two\tthree"]
        );
        assert!(parse_context("[Context]\nfilesystems=/tmp/A\\q;\n").is_none());
        assert!(parse_context("[Context]\nfilesystems=/tmp/A\\\n").is_none());
        assert!(
            parse_context(&format!(
                "[Context]\nsockets={}\n",
                "x;".repeat(MAX_VALUES + 1)
            ))
            .is_none()
        );
    }
}
