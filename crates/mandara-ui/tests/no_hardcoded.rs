//! Guard against new hard-coded, user-perceivable values.
//!
//! Every value a user could notice or want to change is a setting (see
//! `docs/configuration.md`); what stays fixed is a *named* constant. This
//! test scans the shell, Studio, greeter and service sources (tests,
//! comments, strings and `const` / `static` items stripped) for bare
//! literals in the three places tunable values hide:
//!
//! - `px(<number>)` — sizes and spacing (`px(0.0)` and `px(1.0)`, zero and
//!   hairlines, are structural and exempt);
//! - `Duration::from_millis(<number>)` / `Duration::from_secs(<number>)` —
//!   delays, timeouts and refresh intervals.
//!
//! Each file's remaining sites are listed with a reason in
//! `tests/literal_allowlist.tsv`, and the counts must match exactly: a new
//! literal fails (make it a setting, or a named `const` with a doc comment
//! saying why it is fixed), and a removed one asks for the count to come
//! down, so the list never goes stale. `MANDARA_LITERALS_DUMP=1` rewrites the
//! counts, keeping the reasons.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Source roots scanned, relative to `crates/`.
const ROOTS: &[&str] = &["mandara-ui/src", "mandara-studio/src", "mandara-greeter/src", "mandara-services/src"];
/// The allowlist, relative to this crate.
const ALLOWLIST: &str = "tests/literal_allowlist.tsv";
/// Calls whose single literal argument is a tunable value.
const CALLS: &[(&str, Kind)] = &[("px(", Kind::Px), ("Duration::from_millis(", Kind::Millis), ("Duration::from_secs(", Kind::Secs)];
/// `px()` values that are structure, not style: no size and a hairline.
const STRUCTURAL_PX: &[f64] = &[0.0, 1.0];
/// Reason written for files new to the allowlist (edit it by hand).
const UNREVIEWED: &str = "UNREVIEWED: make these settings or named consts";

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Px,
    Millis,
    Secs,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Px => "px",
            Kind::Millis => "millis",
            Kind::Secs => "secs",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        CALLS.iter().map(|(_, k)| *k).find(|k| k.name() == s)
    }
}

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("crates/").to_path_buf()
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if p.is_dir() {
            if name != "tests" {
                rust_files(&p, out);
            }
        } else if name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_tests.rs") {
            out.push(p);
        }
    }
}

/// `text` with comments, string / char literals blanked to spaces (lines
/// kept), so only code is searched.
fn code_only(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let blank = |c: u8| if c == b'\n' { b'\n' } else { b' ' };
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                out.push(b' ');
                i += 1;
            }
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            let mut depth = 0;
            while i < b.len() {
                if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                    depth += 1;
                    out.extend(b"  ");
                    i += 2;
                } else if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                    depth -= 1;
                    out.extend(b"  ");
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    out.push(blank(b[i]));
                    i += 1;
                }
            }
        } else if c == b'r' && (b.get(i + 1) == Some(&b'"') || (b.get(i + 1) == Some(&b'#') && matches!(b.get(i + 2), Some(b'"' | b'#')))) && (i == 0 || !is_ident(b[i - 1])) {
            // Raw string r"…" / r#"…"#.
            let mut j = i + 1;
            let mut hashes = 0;
            while b.get(j) == Some(&b'#') {
                hashes += 1;
                j += 1;
            }
            j += 1;
            let close: Vec<u8> = std::iter::once(b'"').chain(std::iter::repeat_n(b'#', hashes)).collect();
            while j < b.len() && !b[j..].starts_with(&close) {
                j += 1;
            }
            let end = (j + close.len()).min(b.len());
            out.extend(b[i..end].iter().map(|&c| blank(c)));
            i = end;
        } else if c == b'"' {
            out.push(b' ');
            i += 1;
            while i < b.len() && b[i] != b'"' {
                let step = if b[i] == b'\\' { 2 } else { 1 };
                for k in 0..step {
                    if let Some(&c) = b.get(i + k) {
                        out.push(blank(c));
                    }
                }
                i += step;
            }
            out.push(b' ');
            i += 1;
        } else if c == b'\'' {
            // A char literal ('a', '\n', '\u{..}'); otherwise a lifetime.
            let end = if b.get(i + 1) == Some(&b'\\') {
                b[i + 2..].iter().position(|&c| c == b'\'').map(|p| i + 2 + p + 1)
            } else {
                let ch = text[i + 1..].chars().next().map_or(1, char::len_utf8);
                (b.get(i + 1 + ch) == Some(&b'\'')).then_some(i + 2 + ch)
            };
            match end {
                Some(end) => {
                    out.extend(std::iter::repeat_n(b' ', end - i));
                    i = end;
                }
                None => {
                    out.push(c);
                    i += 1;
                }
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_default()
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Byte range of the item starting at `start`: up to its `;` (or its
/// closing brace) at nesting depth 0.
fn item_end(code: &[u8], start: usize) -> usize {
    let mut depth = 0i32;
    let mut i = start;
    while i < code.len() {
        match code[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' => depth -= 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            b';' if depth == 0 => return i + 1,
            _ => {}
        }
        i += 1;
    }
    code.len()
}

/// Blanks `#[cfg(test)]` items and `const` / `static` items: what is left
/// is the code where a bare literal would be a hidden setting.
fn scannable(text: &str) -> String {
    let mut code = code_only(text).into_bytes();
    let blank = |code: &mut Vec<u8>, from: usize, to: usize| {
        for c in &mut code[from..to] {
            if *c != b'\n' {
                *c = b' ';
            }
        }
    };
    let s = String::from_utf8_lossy(&code).to_string();
    for (at, _) in s.match_indices("#[cfg(test)]") {
        let end = item_end(&code, at + "#[cfg(test)]".len());
        blank(&mut code, at, end);
    }
    let s = String::from_utf8_lossy(&code).to_string();
    for kw in ["const ", "static "] {
        for (at, _) in s.match_indices(kw) {
            if at > 0 && is_ident(code[at - 1]) {
                continue;
            }
            // `const fn`, `const {` blocks and `const` generics are code.
            let rest = s[at + kw.len()..].trim_start();
            let Some(first) = rest.chars().next() else { continue };
            if rest.starts_with("fn ") || !first.is_ascii_uppercase() || !rest.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).next().is_some_and(|n| n.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')) {
                continue;
            }
            let end = item_end(&code, at);
            blank(&mut code, at, end);
        }
    }
    String::from_utf8(code).unwrap_or_default()
}

/// Bare literal sites per kind in `text`, with their line numbers.
fn sites(text: &str) -> Vec<(Kind, usize, String)> {
    let code = scannable(text);
    let mut out = Vec::new();
    for (call, kind) in CALLS {
        for (at, _) in code.match_indices(call) {
            if *kind == Kind::Px && at > 0 && (is_ident(code.as_bytes()[at - 1]) || code.as_bytes()[at - 1] == b'.') {
                continue; // `foo_px(`, `.px(` (padding) are other calls.
            }
            let arg_start = at + call.len();
            let Some(close) = code[arg_start..].find(')') else { continue };
            let arg = code[arg_start..arg_start + close].trim();
            let number = arg.trim_end_matches("f32").trim_end_matches("f64").trim_end_matches("u64").replace('_', "");
            let Ok(value) = number.parse::<f64>() else { continue };
            if (*kind == Kind::Px && STRUCTURAL_PX.contains(&value)) || value == 0.0 {
                continue;
            }
            let line = code[..at].matches('\n').count() + 1;
            out.push((*kind, line, format!("{call}{arg})")));
        }
    }
    out
}

type Counts = BTreeMap<(String, Kind), usize>;

fn scan() -> (Counts, BTreeMap<(String, Kind), Vec<String>>) {
    let root = crates_dir();
    let mut files = Vec::new();
    for r in ROOTS {
        rust_files(&root.join(r), &mut files);
    }
    files.sort();
    let (mut counts, mut detail) = (Counts::new(), BTreeMap::new());
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        let rel = f.strip_prefix(&root).unwrap_or(&f).to_string_lossy().replace('\\', "/");
        for (kind, line, snippet) in sites(&text) {
            *counts.entry((rel.clone(), kind)).or_default() += 1;
            detail.entry((rel.clone(), kind)).or_insert_with(Vec::new).push(format!("{rel}:{line} {snippet}"));
        }
    }
    (counts, detail)
}

/// `(file, kind) -> (count, reason)` from the allowlist.
fn allowlist() -> BTreeMap<(String, Kind), (usize, String)> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(ALLOWLIST);
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let cols: Vec<&str> = l.splitn(4, '\t').collect();
            assert_eq!(cols.len(), 4, "{ALLOWLIST}: `{l}` needs file, kind, count and reason (tab-separated)");
            let kind = Kind::parse(cols[1]).unwrap_or_else(|| panic!("{ALLOWLIST}: unknown kind `{}`", cols[1]));
            let count = cols[2].parse().unwrap_or_else(|_| panic!("{ALLOWLIST}: bad count `{}`", cols[2]));
            assert!(!cols[3].trim().is_empty(), "{ALLOWLIST}: `{}` has no reason", cols[0]);
            ((cols[0].to_string(), kind), (count, cols[3].to_string()))
        })
        .collect()
}

fn write_allowlist(counts: &Counts, old: &BTreeMap<(String, Kind), (usize, String)>) {
    let mut out = String::from(
        "# Bare tunable literals that remain, per file (see tests/no_hardcoded.rs).\n\
         # file\tkind\tcount\treason — counts must match the sources exactly.\n",
    );
    for ((file, kind), n) in counts {
        let reason = old.get(&(file.clone(), *kind)).map_or(UNREVIEWED, |(_, r)| r.as_str());
        out.push_str(&format!("{file}\t{}\t{n}\t{reason}\n", kind.name()));
    }
    std::fs::write(Path::new(env!("CARGO_MANIFEST_DIR")).join(ALLOWLIST), out).expect("write the allowlist");
}

#[test]
fn no_new_hardcoded_literals() {
    let (counts, detail) = scan();
    assert!(counts.values().sum::<usize>() > 0, "the scan found nothing; did the source layout change?");
    let allowed = allowlist();
    if std::env::var_os("MANDARA_LITERALS_DUMP").is_some() {
        write_allowlist(&counts, &allowed);
        return;
    }
    let mut report = Vec::new();
    for (key, n) in &counts {
        let allowed_n = allowed.get(key).map_or(0, |(c, _)| *c);
        if *n > allowed_n {
            report.push(format!("{} has {n} bare `{}` literal(s), {allowed_n} allowed — make the new one a setting or a named const:\n  {}", key.0, key.1.name(), detail[key].join("\n  ")));
        }
    }
    for (key, (n, _)) in &allowed {
        let found = counts.get(key).copied().unwrap_or(0);
        if found < *n {
            report.push(format!("{} now has {found} bare `{}` literal(s): lower its count in {ALLOWLIST} from {n}", key.0, key.1.name()));
        }
    }
    let unreviewed: Vec<&String> = allowed.iter().filter(|(_, (_, r))| r.starts_with("UNREVIEWED")).map(|((f, _), _)| f).collect();
    assert!(unreviewed.is_empty(), "allowlist entries without a reason: {unreviewed:?}");
    assert!(report.is_empty(), "{}", report.join("\n"));
}

#[test]
fn scanner_skips_consts_tests_comments_and_strings() {
    let src = r#"
/// Doc: px(12.0) and Duration::from_millis(300).
const GAP: f32 = 4.0;
const FADE: Duration = Duration::from_millis(250);
pub(crate) static LIST: &[f32] = &[px(3.0)];
fn f() {
    let a = px(0.0);
    let b = px(1.0);
    let s = "px(9.0)";
    let c = 'x';
    let d = px(7.5); // px(8.0)
    let e = Duration::from_secs(2);
    let g = foo_px(5.0);
    let h = div().px(6.0);
}
#[cfg(test)]
mod tests {
    fn t() { let _ = px(42.0); }
}
"#;
    let found: Vec<String> = sites(src).into_iter().map(|(_, _, s)| s).collect();
    assert_eq!(found, vec!["px(7.5)".to_string(), "Duration::from_secs(2)".to_string()]);
}
