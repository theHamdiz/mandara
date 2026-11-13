//! Update a CSS custom property without replacing its containing rule.

use anyhow::{Result, ensure};

const PROPERTY: &str = "--pywalfox-font-size";

pub(super) fn resize(source: &str, size: i64) -> Result<String> {
    let bytes = source.as_bytes();
    let mut result = String::with_capacity(source.len());
    let (mut cursor, mut copied, mut boundary, mut changed) = (0, 0, false, false);
    while cursor < bytes.len() {
        if bytes[cursor..].starts_with(b"/*") {
            cursor = source[cursor + 2..]
                .find("*/")
                .map_or(bytes.len(), |end| cursor + 2 + end + 2);
            continue;
        }
        if matches!(bytes[cursor], b'\'' | b'"') {
            let quote = bytes[cursor];
            cursor += 1;
            while cursor < bytes.len() {
                if bytes[cursor] == b'\\' {
                    cursor = (cursor + 2).min(bytes.len());
                } else if bytes[cursor] == quote {
                    cursor += 1;
                    break;
                } else {
                    cursor += 1;
                }
            }
            boundary = false;
            continue;
        }
        if boundary && bytes[cursor..].starts_with(PROPERTY.as_bytes()) {
            let colon = cursor + PROPERTY.len();
            let gap = bytes[colon..]
                .iter()
                .take_while(|byte| byte.is_ascii_whitespace())
                .count();
            if bytes.get(colon + gap) == Some(&b':') {
                let start = colon + gap + 1;
                let end = bytes[start..]
                    .iter()
                    .position(|byte| matches!(byte, b';' | b'}'))
                    .map_or(bytes.len(), |end| start + end);
                let value = &source[start..end];
                let leading = value.len() - value.trim_start().len();
                let trailing = value.len() - value.trim_end().len();
                result.push_str(&source[copied..start + leading]);
                use std::fmt::Write as _;
                write!(result, "{size}px")?;
                if value.contains("!important") {
                    result.push_str(" !important");
                }
                result.push_str(&value[value.len() - trailing..]);
                copied = end;
                cursor = end;
                changed = true;
                boundary = false;
                continue;
            }
        }
        if !bytes[cursor].is_ascii_whitespace() {
            boundary = matches!(bytes[cursor], b'{' | b';');
        }
        cursor += 1;
    }
    ensure!(changed, "browser CSS has no font size setting");
    result.push_str(&source[copied..]);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn minified_and_multiline_rules_keep_selectors_braces_and_other_properties() {
        for (source, expected) in [
            (
                ":root{--pywalfox-font-size:12px;color:red}",
                ":root{--pywalfox-font-size:18px;color:red}",
            ),
            (
                ":root {\n  --pywalfox-font-size: 12px !important;\n  color: red;\n}\n",
                ":root {\n  --pywalfox-font-size: 18px !important;\n  color: red;\n}\n",
            ),
            (
                "/* --pywalfox-font-size:4px; */ a{content:'--pywalfox-font-size:2px;';--pywalfox-font-size:12px}",
                "/* --pywalfox-font-size:4px; */ a{content:'--pywalfox-font-size:2px;';--pywalfox-font-size:18px}",
            ),
        ] {
            assert_eq!(resize(source, 18).unwrap(), expected);
        }
        assert!(resize("/* --pywalfox-font-size:12px; */", 18).is_err());
    }
}
