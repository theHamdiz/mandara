//! Line counts for the build report: code, comment and blank lines of the
//! workspace's Rust (per crate), vendored Rust, the example plugins, the
//! locale catalogs and the WIT interfaces.
//!
//! A small lexer per language, enough to tell code from comments: line
//! comments (`//`, `///`, `//!`, `#`), nested block comments, string
//! literals (escapes, Rust raw strings, TOML multi-line strings) and Rust
//! char literals vs lifetimes, so a `"// not a comment"` string or a `'"'`
//! char doesn't derail the count. A line holding any code counts as code,
//! otherwise one with comment text counts as a comment; whitespace-only
//! lines (also inside comments and strings) count as blank.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// Code, comment and blank line totals.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Lines {
    pub code: u64,
    pub comments: u64,
    pub blank: u64,
}

impl Lines {
    pub fn total(&self) -> u64 {
        self.code + self.comments + self.blank
    }

    pub fn add(&mut self, other: Lines) {
        self.code += other.code;
        self.comments += other.comments;
        self.blank += other.blank;
    }
}

/// A string literal opener and whether `\` escapes inside it.
#[derive(Copy, Clone, Debug)]
pub struct Quote {
    pub byte: u8,
    pub escapes: bool,
}

/// The comment and string rules of one language.
#[derive(Copy, Clone, Debug)]
pub struct Syntax {
    /// Line comment markers.
    pub line: &'static [&'static str],
    /// Block comment open / close markers.
    pub block: Option<(&'static str, &'static str)>,
    /// Block comments nest (Rust).
    pub nested: bool,
    pub quotes: &'static [Quote],
    /// A tripled quote opens a multi-line string (TOML `"""`, `'''`).
    pub triple_quotes: bool,
    /// Rust raw strings: `r"…"`, `r#"…"#`, `br"…"`, `cr"…"`.
    pub raw_strings: bool,
    /// Rust char / byte literals (`'x'`, `'\n'`, `b'"'`) next to lifetimes.
    pub char_literals: bool,
}

const DOUBLE_QUOTE: Quote = Quote { byte: b'"', escapes: true };
const SINGLE_QUOTE_LITERAL: Quote = Quote { byte: b'\'', escapes: false };

pub const RUST: Syntax = Syntax { line: &["//"], block: Some(("/*", "*/")), nested: true, quotes: &[DOUBLE_QUOTE], triple_quotes: false, raw_strings: true, char_literals: true };
pub const TOML: Syntax = Syntax { line: &["#"], block: None, nested: false, quotes: &[DOUBLE_QUOTE, SINGLE_QUOTE_LITERAL], triple_quotes: true, raw_strings: false, char_literals: false };
pub const WIT: Syntax = Syntax { line: &["//"], block: Some(("/*", "*/")), nested: true, quotes: &[DOUBLE_QUOTE], triple_quotes: false, raw_strings: false, char_literals: false };

/// Length of a tripled quote.
const TRIPLE: usize = 3;

/// Lexer state carried across lines.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum State {
    Code,
    /// Inside a block comment, at this nesting depth.
    Block(u32),
    Str { quote: u8, escapes: bool, triple: bool },
    /// Inside a Rust raw string closed by `"` and this many `#`.
    Raw(usize),
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Bytes in the UTF-8 sequence starting with `lead`.
fn utf8_len(lead: u8) -> usize {
    /// Lead-byte prefixes of 2-, 3- and 4-byte sequences.
    const TWO: u8 = 0b1100_0000;
    const THREE: u8 = 0b1110_0000;
    const FOUR: u8 = 0b1111_0000;
    if lead >= FOUR {
        4
    } else if lead >= THREE {
        3
    } else if lead >= TWO {
        2
    } else {
        1
    }
}

/// If a Rust raw string opens at `i`, the number of `#` and the index just
/// past its opening `"`.
fn raw_open(b: &[u8], i: usize) -> Option<(usize, usize)> {
    if b[i] != b'r' {
        return None;
    }
    // `r` must start a token, or follow a `b` / `c` prefix that does.
    let starts = match i.checked_sub(1).map(|p| b[p]) {
        None => true,
        Some(b'b' | b'c') => i < 2 || !is_ident(b[i - 2]),
        Some(p) => !is_ident(p),
    };
    if !starts {
        return None;
    }
    let hashes = b[i + 1..].iter().take_while(|&&c| c == b'#').count();
    (b.get(i + 1 + hashes) == Some(&b'"')).then_some((hashes, i + hashes + 2))
}

/// If a Rust char literal opens at `i` (a `'`), the index just past it;
/// `None` for a lifetime or label.
fn char_literal_end(b: &[u8], i: usize) -> Option<usize> {
    let next = *b.get(i + 1)?;
    if next == b'\\' {
        // `'\n'`, `'\''`, `'\u{1F600}'`: the escape's first char is never
        // the closing quote, so the close is the next `'` after it.
        let from = i + TRIPLE;
        return b.get(from..)?.iter().position(|&c| c == b'\'').map(|p| from + p + 1);
    }
    let close = i + 1 + utf8_len(next);
    (next != b'\'' && b.get(close) == Some(&b'\'')).then_some(close + 1)
}

/// Counts the lines of `text` written in `syntax`.
pub fn count(text: &str, syntax: &Syntax) -> Lines {
    let mut lines = Lines::default();
    let mut state = State::Code;
    for line in text.lines() {
        if line.trim().is_empty() {
            lines.blank += 1;
            continue;
        }
        let b = line.as_bytes();
        let mut code = matches!(state, State::Str { .. } | State::Raw(_));
        let mut comment = matches!(state, State::Block(_));
        let mut i = 0;
        while i < b.len() {
            let rest = &b[i..];
            match state {
                State::Block(depth) => {
                    let (open, close) = syntax.block.unwrap_or_default();
                    if rest.starts_with(close.as_bytes()) {
                        state = if depth > 1 { State::Block(depth - 1) } else { State::Code };
                        i += close.len();
                    } else if syntax.nested && rest.starts_with(open.as_bytes()) {
                        state = State::Block(depth + 1);
                        i += open.len();
                    } else {
                        i += 1;
                    }
                }
                State::Str { quote, escapes, triple } => {
                    if escapes && b[i] == b'\\' {
                        i += 2;
                    } else if b[i] == quote && (!triple || rest.len() >= TRIPLE && rest[..TRIPLE].iter().all(|&c| c == quote)) {
                        state = State::Code;
                        i += if triple { TRIPLE } else { 1 };
                    } else {
                        i += 1;
                    }
                }
                State::Raw(hashes) => {
                    if b[i] == b'"' && rest.len() > hashes && rest[1..=hashes].iter().all(|&c| c == b'#') {
                        state = State::Code;
                        i += 1 + hashes;
                    } else {
                        i += 1;
                    }
                }
                State::Code => {
                    if b[i].is_ascii_whitespace() {
                        i += 1;
                    } else if syntax.line.iter().any(|m| rest.starts_with(m.as_bytes())) {
                        comment = true;
                        break;
                    } else if let Some((open, _)) = syntax.block.filter(|(open, _)| rest.starts_with(open.as_bytes())) {
                        comment = true;
                        state = State::Block(1);
                        i += open.len();
                    } else if let Some((hashes, next)) = syntax.raw_strings.then(|| raw_open(b, i)).flatten() {
                        code = true;
                        state = State::Raw(hashes);
                        i = next;
                    } else if let Some(q) = syntax.quotes.iter().find(|q| q.byte == b[i]) {
                        code = true;
                        let triple = syntax.triple_quotes && rest.len() >= TRIPLE && rest[..TRIPLE].iter().all(|&c| c == q.byte);
                        state = State::Str { quote: q.byte, escapes: q.escapes, triple };
                        i += if triple { TRIPLE } else { 1 };
                    } else if let Some(end) = (syntax.char_literals && b[i] == b'\'').then(|| char_literal_end(b, i)).flatten() {
                        code = true;
                        i = end;
                    } else {
                        code = true;
                        i += 1;
                    }
                }
            }
        }
        if code {
            lines.code += 1;
        } else if comment {
            lines.comments += 1;
        } else {
            lines.code += 1;
        }
    }
    lines
}

/// Directory names never counted (build output, VCS, editor state).
const SKIPPED_DIRS: &[&str] = &["target", "dist", "node_modules"];

/// Every file under `dir` with extension `ext`, sorted, skipping build
/// output and hidden directories.
pub fn files_with_ext(dir: &Path, ext: &str) -> Vec<PathBuf> {
    fn walk(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            let p = e.path();
            if p.is_dir() {
                if !name.starts_with('.') && !SKIPPED_DIRS.contains(&name.as_ref()) {
                    walk(&p, ext, out);
                }
            } else if p.extension().is_some_and(|x| x == ext) {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, ext, &mut out);
    out.sort();
    out
}

/// Lines of a group of files (one crate, the vendored tree, …).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Group {
    pub name: String,
    pub files: u64,
    #[serde(flatten)]
    pub lines: Lines,
}

/// Counts `files` as `syntax` into a group called `name`.
pub fn count_files(name: &str, files: &[PathBuf], syntax: &Syntax) -> Group {
    let mut lines = Lines::default();
    for f in files {
        // Non-UTF-8 bytes don't stop the count.
        let bytes = std::fs::read(f).unwrap_or_default();
        lines.add(count(&String::from_utf8_lossy(&bytes), syntax));
    }
    Group { name: name.to_string(), files: files.len() as u64, lines }
}

/// Subdirectories of `dir` that hold a `Cargo.toml`, sorted.
pub fn crate_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir).map(|it| it.flatten().map(|e| e.path()).filter(|p| p.join("Cargo.toml").is_file()).collect()).unwrap_or_default();
    out.sort();
    out
}

/// The build's line report.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    /// Workspace Rust: every crate under `crates/` plus `xtask`.
    pub rust: Group,
    /// Vendored Rust (`vendor/*`), patched into the build.
    pub vendored: Group,
    /// Example plugins (`plugins/`), built separately for WebAssembly.
    pub plugins: Group,
    /// Locale catalogs (`crates/*/locales/*.toml`).
    pub locales: Group,
    /// WIT interfaces (`crates/**/*.wit`).
    pub wit: Group,
    /// Per-crate Rust counts, in path order.
    #[serde(rename = "crate")]
    pub crates: Vec<Group>,
}

/// Counts the workspace at `root`.
pub fn report(root: &Path) -> Report {
    let mut crate_paths = crate_dirs(&root.join("crates"));
    crate_paths.push(root.join("xtask"));
    let crates: Vec<Group> = crate_paths
        .iter()
        .map(|dir| {
            let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            count_files(&name, &files_with_ext(dir, "rs"), &RUST)
        })
        .collect();
    let mut rust = Group { name: "workspace".into(), ..Group::default() };
    for c in &crates {
        rust.files += c.files;
        rust.lines.add(c.lines);
    }
    let locale_files: Vec<PathBuf> = crate_dirs(&root.join("crates")).iter().flat_map(|d| files_with_ext(&d.join("locales"), "toml")).collect();
    Report {
        rust,
        vendored: count_files("vendor", &files_with_ext(&root.join("vendor"), "rs"), &RUST),
        plugins: count_files("plugins", &files_with_ext(&root.join("plugins"), "rs"), &RUST),
        locales: count_files("locales", &locale_files, &TOML),
        wit: count_files("wit", &files_with_ext(&root.join("crates"), "wit"), &WIT),
        crates,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(code: u64, comments: u64, blank: u64) -> Lines {
        Lines { code, comments, blank }
    }

    #[test]
    fn rust_line_kinds() {
        let src = "//! crate docs\n\n/// item docs\nfn main() { // trailing\n    let x = 1;\n}\n// plain\n";
        assert_eq!(count(src, &RUST), lines(3, 3, 1));
    }

    #[test]
    fn rust_block_comments_nest() {
        let src = "/* one\n   /* two */\n   still comment\n*/\nlet a = 1; /* tail */\n/* a */ let b = 2;\n";
        assert_eq!(count(src, &RUST), lines(2, 4, 0));
    }

    #[test]
    fn blank_lines_inside_comments_are_blank() {
        assert_eq!(count("/*\n\n*/\n", &RUST), lines(0, 2, 1));
    }

    #[test]
    fn comment_markers_in_strings_are_code() {
        let src = "let s = \"// not a comment\";\nlet t = \"/* nor this\";\nlet u = 1;\n";
        assert_eq!(count(src, &RUST), lines(3, 0, 0));
    }

    #[test]
    fn escaped_quotes_stay_in_the_string() {
        assert_eq!(count("let s = \"a \\\" // b\"; // c\n// d\n", &RUST), lines(1, 1, 0));
    }

    #[test]
    fn multi_line_strings_are_code() {
        let src = "let s = \"first\n// inside the string\n/* also */\";\n// after\n";
        assert_eq!(count(src, &RUST), lines(3, 1, 0));
    }

    #[test]
    fn raw_strings() {
        let src = "let s = r#\"has \" and // inside\n// still the string\n\"#; // done\n// comment\n";
        assert_eq!(count(src, &RUST), lines(3, 1, 0));
        // `br"…"` and a plain identifier ending in `r` before a string.
        assert_eq!(count("let b = br\"/*\";\nlet r = for_r\"x\";\n// c\n", &RUST), lines(2, 1, 0));
    }

    #[test]
    fn char_literals_and_lifetimes() {
        let src = "let q = '\"'; // c\nfn f<'a>(x: &'a str) {}\nlet e = '\\''; let n = '\\n';\nlet u = 'é'; let w = '\\u{1F600}';\n// end\n";
        assert_eq!(count(src, &RUST), lines(4, 1, 0));
        // A lifetime must not swallow the following quote as a char.
        assert_eq!(count("fn g<'a>(s: &'a str) -> &'a str { \"/*\" }\n// c\n", &RUST), lines(1, 1, 0));
    }

    #[test]
    fn toml_strings_and_comments() {
        let src = "# heading\nkey = \"value # not comment\"\npath = 'C:\\dir' # trailing\nlong = \"\"\"\n# inside\n\"\"\"\n\n";
        assert_eq!(count(src, &TOML), lines(5, 1, 1));
    }

    #[test]
    fn wit_comments() {
        assert_eq!(count("/// docs\ninterface host {\n  // note\n  log: func(msg: string);\n}\n", &WIT), lines(3, 2, 0));
    }

    #[test]
    fn totals_add_up() {
        let mut a = lines(1, 2, 3);
        a.add(lines(4, 5, 6));
        assert_eq!(a, lines(5, 7, 9));
        assert_eq!(a.total(), 21);
    }
}
