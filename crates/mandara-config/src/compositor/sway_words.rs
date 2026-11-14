//! The non-executing subset of Sway's word expansion, bounded before copying.

use std::{
    ffi::OsString,
    os::unix::ffi::{OsStrExt as _, OsStringExt as _},
};

use anyhow::{Result, ensure};

use super::{IncludeEnvironment, include_env::MAX_PATH_BYTES};

fn unsupported() -> &'static str {
    mandara_i18n::t("Computed shortcuts must be edited in their source configuration.")
}

#[derive(Default)]
struct Words {
    words: Vec<OsString>,
    word: Vec<u8>,
    started: bool,
    retained: usize,
    limit: usize,
}

impl Words {
    fn push(&mut self, byte: u8, literal: bool) -> Result<()> {
        let escape = literal && matches!(byte, b'*' | b'?' | b'[' | b']' | b'\\');
        ensure!(
            self.word.len() + 1 + usize::from(escape) <= MAX_PATH_BYTES,
            mandara_i18n::t("An include path is too long.")
        );
        if escape {
            self.word.push(b'\\');
        }
        self.word.push(byte);
        self.started = true;
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        if !self.started {
            return Ok(());
        }
        ensure!(
            self.words.len() < self.limit,
            mandara_i18n::t("The shortcut configuration includes too many files.")
        );
        self.retained += self.word.len() + std::mem::size_of::<OsString>();
        ensure!(
            self.retained <= crate::keybinds::MAX_TOTAL_BYTES,
            mandara_i18n::t("The shortcut configuration is too large to edit.")
        );
        self.words
            .push(OsString::from_vec(std::mem::take(&mut self.word)));
        self.started = false;
        Ok(())
    }

    fn value(&mut self, bytes: &[u8], quoted: bool) -> Result<()> {
        for &byte in bytes {
            if !quoted && matches!(byte, b' ' | b'\t' | b'\n') {
                self.finish()?;
            } else {
                // Quotes and backslashes introduced by expansion are data.
                self.push(byte, quoted || byte == b'\\')?;
            }
        }
        Ok(())
    }
}

fn arguments(mut line: &str) -> impl Iterator<Item = &str> {
    std::iter::from_fn(move || {
        line = line.trim_start_matches(|ch: char| ch.is_ascii_whitespace());
        if line.is_empty() {
            return None;
        }
        let mut quote = None;
        let mut escaped = false;
        for (at, byte) in line.bytes().enumerate() {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if let Some(q) = quote {
                if byte == q {
                    quote = None;
                }
            } else if matches!(byte, b'\'' | b'"') {
                quote = Some(byte);
            } else if byte.is_ascii_whitespace() {
                let argument = &line[..at];
                line = &line[at + 1..];
                return Some(argument);
            }
        }
        let argument = line;
        line = "";
        Some(argument)
    })
}

pub(super) fn expand(
    raw: &str,
    environment: &IncludeEnvironment,
    limit: usize,
) -> Result<Vec<OsString>> {
    ensure!(
        raw.len() <= crate::keybinds::MAX_FILE_BYTES,
        mandara_i18n::t("The shortcut configuration is too large to edit.")
    );
    ensure!(
        environment
            .value("IFS")
            .is_none_or(|ifs| ifs.as_bytes() == b" \t\n"),
        unsupported()
    );
    // Sway removes unescaped quotes from arguments that begin with a quote
    // before calling wordexp. Remaining quotes still govern shell expansion.
    let mut line = Vec::with_capacity(raw.len());
    for arg in arguments(raw) {
        if !line.is_empty() {
            line.push(b' ');
        }
        if arg.starts_with(['"', '\'']) {
            let mut escaped = false;
            let mut quote = None;
            for byte in arg.bytes() {
                match byte {
                    b'"' | b'\'' if !escaped && quote.is_none_or(|active| active == byte) => {
                        quote = if quote.is_some() { None } else { Some(byte) };
                    }
                    b'\\' => {
                        escaped = !escaped;
                        line.push(byte);
                        continue;
                    }
                    _ => line.push(byte),
                }
                escaped = false;
            }
        } else {
            line.extend_from_slice(arg.as_bytes());
        }
    }
    let mut words = Words {
        limit: limit.min(crate::keybinds::MAX_BINDINGS),
        ..Default::default()
    };
    let mut quote = None;
    let mut at = 0;
    while at < line.len() {
        let byte = line[at];
        at += 1;
        match (quote, byte) {
            (Some(q), b) if q == b => quote = None,
            (Some(b'\''), b) => words.push(b, true)?,
            (None, b'\'' | b'"') => {
                quote = Some(byte);
                words.started = true;
            }
            (_, b'\\') => {
                let next = line
                    .get(at)
                    .copied()
                    .ok_or_else(|| anyhow::anyhow!(unsupported()))?;
                if quote == Some(b'"') && !matches!(next, b'"' | b'\\' | b'$' | b'`' | b'\n') {
                    words.push(b'\\', true)?;
                } else {
                    at += 1;
                    if next != b'\n' {
                        words.push(next, true)?;
                    }
                }
            }
            (_, b'`') => anyhow::bail!(unsupported()),
            (_, b'$') => {
                let braced = line.get(at) == Some(&b'{');
                if braced {
                    at += 1;
                }
                let start = at;
                while line
                    .get(at)
                    .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
                {
                    at += 1;
                }
                ensure!(at > start && !line[start].is_ascii_digit(), unsupported());
                let name = std::str::from_utf8(&line[start..at])?;
                if braced {
                    ensure!(line.get(at) == Some(&b'}'), unsupported());
                    at += 1;
                }
                if let Some(value) = environment.value(name) {
                    words.value(value.as_bytes(), quote.is_some())?;
                }
            }
            (None, b'~') if !words.started => {
                ensure!(
                    line.get(at)
                        .is_none_or(|b| matches!(b, b'/' | b' ' | b'\t' | b'\n')),
                    unsupported()
                );
                let home = environment
                    .value("HOME")
                    .ok_or_else(|| anyhow::anyhow!(unsupported()))?;
                for &b in home.as_bytes() {
                    words.push(b, true)?;
                }
            }
            (None, b' ' | b'\t' | b'\n') => words.finish()?,
            (_, b) => words.push(b, quote.is_some())?,
        }
    }
    ensure!(quote.is_none(), unsupported());
    words.finish()?;
    Ok(words.words)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn variables_split_before_globbing_without_becoming_shell_syntax() {
        let environment = IncludeEnvironment::captured([
            ("FILES".into(), "one.conf two.conf".into()),
            ("QUOTES".into(), "'one two' \\three".into()),
            ("HOME".into(), "/home/a [b]".into()),
        ]);
        let words = |text| expand(text, &environment, 10).unwrap();
        assert_eq!(words("$FILES"), ["one.conf", "two.conf"]);
        assert_eq!(words("pre${FILES}/post"), ["preone.conf", "two.conf/post"]);
        assert_eq!(words("$QUOTES"), ["'one", "two'", "\\\\three"]);
        assert_eq!(words("x\"$FILES\""), ["xone.conf two.conf"]);
        assert_eq!(words("x'$FILES'"), ["x$FILES"]);
        assert_eq!(words("my\\ dir/x.conf"), ["my dir/x.conf"]);
        assert_eq!(
            words("\"'keys with spaces.conf'\""),
            ["keys with spaces.conf"]
        );
        assert_eq!(
            words("'\"keys with spaces.conf\"'"),
            ["keys with spaces.conf"]
        );
        assert_eq!(words("~/keys"), ["/home/a \\[b\\]/keys"]);
        assert!(words("$UNSET").is_empty());
        assert_eq!(words("$UNSET~/keys"), ["/home/a \\[b\\]/keys"]);
        assert_eq!(words("x\"$UNSET\""), ["x"]);
        assert_eq!(words("a\u{a0}b"), ["a\u{a0}b"]);
        for unsafe_path in ["$(touch marker)", "`touch marker`", "${FILES:-other}"] {
            assert!(expand(unsafe_path, &environment, 10).is_err());
        }
        let bytes = OsString::from_vec(vec![b'a', 0xff, b'b']);
        let environment = IncludeEnvironment::captured([("FILE".into(), bytes.clone())]);
        assert_eq!(expand("$FILE", &environment, 1).unwrap(), [bytes]);
        assert!(expand("$FILE $FILE", &environment, 1).is_err());
        assert!(
            expand(
                &"a ".repeat(crate::keybinds::MAX_BINDINGS + 1),
                &environment,
                usize::MAX
            )
            .is_err()
        );
        let environment = IncludeEnvironment::captured([(
            "FILE".into(),
            OsStr::new(&"x".repeat(MAX_PATH_BYTES + 1)).into(),
        )]);
        assert!(expand("$FILE", &environment, 1).is_err());
    }
}
