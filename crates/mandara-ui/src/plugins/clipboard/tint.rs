//! Light syntax tinting for clipboard previews: one pass per line marks
//! comments, strings, numbers, keywords and keys (CSS properties, JSON /
//! YAML / TOML keys, markup tags). It is deliberately small, a hint of
//! colour from the theme rather than a full highlighter, and runs once per
//! entry when its row is first shown.

use std::ops::Range;

/// What a run of a line is.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Tok {
    Keyword,
    Str,
    Number,
    Comment,
    /// A key: CSS property, JSON / YAML / TOML key, markup tag.
    Key,
}

/// Keywords across the languages the classifier names (one shared list:
/// a word that is a keyword somewhere is almost never an identifier in a
/// short snippet elsewhere).
const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "case", "catch", "class", "const", "continue", "def", "default", "defer", "do", "elif", "else", "end", "enum", "except", "export", "extends", "false", "False", "finally",
    "fn", "for", "from", "func", "function", "go", "if", "impl", "import", "in", "interface", "is", "lambda", "let", "local", "loop", "match", "mod", "mut", "new", "nil", "None", "not", "null", "package",
    "pass", "pub", "raise", "return", "self", "Self", "static", "struct", "super", "switch", "then", "this", "throw", "trait", "true", "True", "try", "type", "use", "val", "var", "void", "where", "while",
    "with", "yield", "fi", "done", "esac", "echo", "sudo", "SELECT", "FROM", "WHERE", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "CREATE", "TABLE", "AND", "OR", "JOIN", "ON", "AS", "ORDER",
    "BY", "GROUP", "LIMIT", "int", "char", "float", "double", "bool", "string", "str", "u8", "u16", "u32", "u64", "usize", "i8", "i16", "i32", "i64", "isize", "f32", "f64", "important",
];

/// How a language writes comments and keys.
struct Syntax {
    line_comments: &'static [&'static str],
    block_comments: bool,
    /// `key:` / `key =` at the start of a line is a key.
    leading_keys: bool,
    /// `"key":` strings are keys.
    json_keys: bool,
    markup: bool,
    backtick_strings: bool,
}

fn syntax(lang: &str) -> Syntax {
    let base = Syntax { line_comments: &["//"], block_comments: true, leading_keys: false, json_keys: false, markup: false, backtick_strings: false };
    match lang {
        "python" | "ruby" | "shell" | "fish" | "perl" | "powershell" | "dockerfile" | "nix" => Syntax { line_comments: &["#"], block_comments: false, ..base },
        "yaml" | "toml" => Syntax { line_comments: &["#"], block_comments: false, leading_keys: true, ..base },
        "sql" | "lua" => Syntax { line_comments: &["--"], block_comments: lang == "sql", ..base },
        "css" => Syntax { line_comments: &[], leading_keys: true, ..base },
        "json" => Syntax { line_comments: &[], block_comments: false, json_keys: true, ..base },
        "html" | "xml" => Syntax { line_comments: &[], block_comments: false, markup: true, ..base },
        "javascript" | "typescript" => Syntax { backtick_strings: true, ..base },
        "diff" => Syntax { line_comments: &[], block_comments: false, ..base },
        // Unknown: comments only where a line starts with them.
        "" => Syntax { line_comments: &["//", "#"], ..base },
        _ => base,
    }
}

/// Tints one line of `lang`. `in_block` carries an open `/* … */` comment
/// from the previous line.
pub fn line(text: &str, lang: &str, in_block: &mut bool) -> Vec<(Range<usize>, Tok)> {
    let sx = syntax(lang);
    let b = text.as_bytes();
    let mut out: Vec<(Range<usize>, Tok)> = Vec::new();
    let mut i = 0;
    let lead = text.len() - text.trim_start().len();
    if sx.leading_keys {
        let rest = &text[lead..];
        if let Some(end) = rest.find([':', '=']).filter(|e| *e > 0) {
            let key = rest[..end].trim_end();
            if !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || "-_.\"'".contains(c)) {
                out.push((lead..lead + key.len(), Tok::Key));
                i = lead + key.len();
            }
        }
    }
    while i < b.len() {
        if *in_block {
            let end = text[i..].find("*/").map_or(b.len(), |e| i + e + 2);
            out.push((i..end, Tok::Comment));
            *in_block = end == b.len() && !text[i..].contains("*/");
            i = end;
            continue;
        }
        let rest = &text[i..];
        if sx.block_comments && rest.starts_with("/*") {
            *in_block = true;
            continue;
        }
        let only_leading = lang.is_empty();
        if sx.line_comments.iter().any(|c| rest.starts_with(c)) && (!only_leading || i == lead) && !(lang == "shell" && i > 0 && b[i - 1] == b'$') {
            out.push((i..b.len(), Tok::Comment));
            break;
        }
        let c = b[i];
        if c == b'"' || c == b'\'' || (sx.backtick_strings && c == b'`') {
            let end = string_end(b, i);
            let is_key = sx.json_keys && text[end..].trim_start().starts_with(':');
            out.push((i..end, if is_key { Tok::Key } else { Tok::Str }));
            i = end;
            continue;
        }
        if sx.markup && c == b'<' {
            let start = i + 1 + usize::from(b.get(i + 1) == Some(&b'/'));
            let end = start + text[start..].bytes().take_while(|c| c.is_ascii_alphanumeric() || *c == b'-' || *c == b':').count();
            if end > start {
                out.push((start..end, Tok::Key));
            }
            i = end.max(i + 1);
            continue;
        }
        if c.is_ascii_digit() || (c == b'#' && lang == "css") {
            let start = i;
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.' || b[i] == b'%') {
                i += 1;
            }
            if start == 0 || !is_ident(b[start - 1]) {
                out.push((start..i, Tok::Number));
            }
            continue;
        }
        if is_ident(c) {
            let start = i;
            while i < b.len() && is_ident(b[i]) {
                i += 1;
            }
            let word = &text[start..i];
            if !sx.markup && KEYWORDS.contains(&word) {
                out.push((start..i, Tok::Keyword));
            }
            continue;
        }
        if c == b'@' && lang == "css" {
            let start = i;
            i += 1;
            while i < b.len() && is_ident(b[i]) {
                i += 1;
            }
            out.push((start..i, Tok::Keyword));
            continue;
        }
        // Step over one whole character.
        i += text[i..].chars().next().map_or(1, char::len_utf8);
    }
    out
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || !c.is_ascii()
}

/// End (exclusive) of the string opened at `start`, honouring `\` escapes;
/// an unclosed string runs to the end of the line.
fn string_end(b: &[u8], start: usize) -> usize {
    let quote = b[start];
    let mut i = start + 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            c if c == quote => return i + 1,
            _ => i += 1,
        }
    }
    b.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs<'a>(text: &'a str, lang: &str) -> Vec<(&'a str, Tok)> {
        let mut block = false;
        line(text, lang, &mut block).into_iter().map(|(r, t)| (&text[r], t)).collect()
    }

    #[test]
    fn tints_code() {
        assert_eq!(runs("let x = \"hi\"; // greet", "rust"), [("let", Tok::Keyword), ("\"hi\"", Tok::Str), ("// greet", Tok::Comment)]);
        assert_eq!(runs("def f(n=10): # ten", "python"), [("def", Tok::Keyword), ("10", Tok::Number), ("# ten", Tok::Comment)]);
        assert_eq!(runs("  color: #ff8800; /* accent */", "css"), [("color", Tok::Key), ("#ff8800", Tok::Number), ("/* accent */", Tok::Comment)]);
        assert_eq!(runs("{\"name\": \"x\", \"n\": 2}", "json"), [("\"name\"", Tok::Key), ("\"x\"", Tok::Str), ("\"n\"", Tok::Key), ("2", Tok::Number)]);
        assert_eq!(runs("<div class=\"a\"></div>", "html"), [("div", Tok::Key), ("\"a\"", Tok::Str), ("div", Tok::Key)]);
        assert_eq!(runs("echo ${HOME} # x", "shell"), [("echo", Tok::Keyword), ("# x", Tok::Comment)]);
        // Identifiers with digits aren't numbers; escapes stay in strings.
        assert_eq!(runs("v2 = 'it\\'s'", "python"), [("'it\\'s'", Tok::Str)]);
        // Non-ASCII text is stepped over by whole characters.
        assert_eq!(runs("s = \"مندرة\" + ü1", "python"), [("\"مندرة\"", Tok::Str)]);
    }

    #[test]
    fn block_comments_span_lines() {
        let mut block = false;
        let first = line("a /* start", "css", &mut block);
        assert!(block);
        assert_eq!(first.last().map(|(_, t)| *t), Some(Tok::Comment));
        let second = line("still */ b: 1", "css", &mut block);
        assert!(!block);
        assert_eq!(second[0], (0..8, Tok::Comment));
    }
}
