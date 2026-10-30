//! `CHANGELOG.md`, compiled in: every release's notes in plain words, for
//! `mandara changelog`, Studio → About → What's new and the GitHub release
//! notes (`cargo xtask changelog`).
//!
//! The format is a small, strict Markdown subset:
//!
//! ```text
//! ## Unreleased                  ← notes for the next release
//! ## 0.1.2 — 2026-10-30          ← a release (em dash or hyphen)
//! One short paragraph saying what the release is about.
//! ### Fixed                      ← a section: New, Improved, Fixed, Under the hood
//! - An item, wrapped over as
//!   many indented lines as it needs.
//! ```
//!
//! `cargo xtask bump` turns `Unreleased` into the new version when the
//! shell is released ([`stamp`]).

/// The changelog this build was made from.
pub const TEXT: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../CHANGELOG.md"));

/// The heading of the notes for the next release.
pub const UNRELEASED: &str = "Unreleased";

/// One release's notes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Release {
    /// `0.1.2`, or [`UNRELEASED`].
    pub version: String,
    /// `2026-10-30`, when the heading has one.
    pub date: Option<String>,
    /// The paragraph under the heading.
    pub summary: String,
    pub sections: Vec<Section>,
}

/// A group of items (`New`, `Improved`, `Fixed`, `Under the hood`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Section {
    pub title: String,
    /// One entry per bullet, its lines joined with spaces.
    pub items: Vec<String>,
}

impl Release {
    pub fn is_unreleased(&self) -> bool {
        self.version == UNRELEASED
    }

    /// No summary and no items.
    pub fn is_empty(&self) -> bool {
        self.summary.is_empty() && self.sections.iter().all(|s| s.items.is_empty())
    }

    /// The notes as Markdown without the version heading (a GitHub release
    /// body; [`parse_notes`] reads it back).
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        if !self.summary.is_empty() {
            out.push_str(&self.summary);
            out.push_str("\n\n");
        }
        for s in self.sections.iter().filter(|s| !s.items.is_empty()) {
            out.push_str(&format!("### {}\n\n", s.title));
            for item in &s.items {
                out.push_str(&format!("- {item}\n"));
            }
            out.push('\n');
        }
        out.trim_end().to_string()
    }

    /// The notes for a terminal: plain text wrapped at `width` columns.
    pub fn to_text(&self, width: usize) -> String {
        let mut out = match &self.date {
            Some(date) => format!("Mandara {} · {date}\n", self.version),
            None if self.is_unreleased() => "Mandara · not released yet\n".to_string(),
            None => format!("Mandara {}\n", self.version),
        };
        if !self.summary.is_empty() {
            out.push('\n');
            out.push_str(&wrap(&plain(&self.summary), width, "", ""));
        }
        for s in self.sections.iter().filter(|s| !s.items.is_empty()) {
            out.push_str(&format!("\n{}\n", s.title));
            for item in &s.items {
                out.push_str(&wrap(&plain(item), width, "  • ", "    "));
            }
        }
        out
    }
}

/// `## 0.1.2 — 2026-10-30` → (`0.1.2`, `2026-10-30`).
fn heading(line: &str) -> Option<(String, Option<String>)> {
    let rest = line.strip_prefix("## ")?.trim();
    let (version, date) = match rest.split_once(['—', '–']).or_else(|| rest.split_once(" - ")) {
        Some((v, d)) => (v.trim(), Some(d.trim().to_string()).filter(|d| !d.is_empty())),
        None => (rest, None),
    };
    Some((version.trim_start_matches('v').to_string(), date))
}

/// Reads one release's body (everything under its heading) into `r`.
fn read_body<'a>(r: &mut Release, lines: impl Iterator<Item = &'a str>) {
    let mut summary: Vec<&str> = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        if let Some(title) = trimmed.strip_prefix("### ") {
            r.sections.push(Section { title: title.trim().to_string(), items: Vec::new() });
        } else if let Some(item) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            if r.sections.is_empty() {
                r.sections.push(Section::default());
            }
            r.sections.last_mut().expect("pushed above").items.push(item.trim().to_string());
        } else if trimmed.is_empty() {
            continue;
        } else if let Some(item) = r.sections.last_mut().and_then(|s| s.items.last_mut()) {
            // A wrapped bullet line.
            item.push(' ');
            item.push_str(trimmed);
        } else if r.sections.is_empty() {
            summary.push(trimmed);
        }
    }
    r.summary = summary.join(" ");
}

/// Every release in a changelog, newest first as written.
pub fn parse(text: &str) -> Vec<Release> {
    let mut releases = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some((version, date)) = heading(line) else {
            continue;
        };
        let mut body = Vec::new();
        while let Some(next) = lines.peek() {
            if next.starts_with("## ") {
                break;
            }
            body.push(lines.next().expect("peeked"));
        }
        let mut r = Release { version, date, ..Default::default() };
        read_body(&mut r, body.into_iter());
        releases.push(r);
    }
    releases
}

/// A release body without its heading (a GitHub release's notes). Text
/// after a `---` rule (install instructions, links) is left out.
pub fn parse_notes(version: &str, body: &str) -> Release {
    let mut r = Release { version: version.trim_start_matches('v').to_string(), ..Default::default() };
    read_body(&mut r, body.lines().take_while(|l| l.trim() != "---"));
    r
}

/// The compiled-in releases, without an empty Unreleased.
pub fn releases() -> Vec<Release> {
    parse(TEXT).into_iter().filter(|r| !(r.is_unreleased() && r.is_empty())).collect()
}

/// `0.1.2` (and `0.1.3-dev`, `v0.1.2`) → (0, 1, 2).
fn numeric(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next().flatten().unwrap_or(0)))
}

/// One release's notes, by version (`0.1.2`, `v0.1.2`, `unreleased`).
pub fn release<'a>(releases: &'a [Release], version: &str) -> Option<&'a Release> {
    if version.eq_ignore_ascii_case(UNRELEASED) {
        return releases.iter().find(|r| r.is_unreleased());
    }
    let wanted = numeric(version)?;
    releases.iter().find(|r| numeric(&r.version) == Some(wanted))
}

/// The notes a build of `version` shipped with: that release, or for a
/// `-dev` build the newest release before it.
pub fn current<'a>(releases: &'a [Release], version: &str) -> Option<&'a Release> {
    let v = numeric(version)?;
    let dev = version.contains('-');
    releases.iter().filter(|r| numeric(&r.version).is_some_and(|n| if dev { n < v } else { n <= v })).max_by_key(|r| numeric(&r.version))
}

/// Releases newer than `after`, up to and including `upto`, newest first
/// (what's new since an update).
pub fn between<'a>(releases: &'a [Release], after: &str, upto: &str) -> Vec<&'a Release> {
    let (Some(after), Some(upto)) = (numeric(after), numeric(upto)) else {
        return Vec::new();
    };
    let mut list: Vec<&Release> = releases.iter().filter(|r| numeric(&r.version).is_some_and(|n| n > after && n <= upto)).collect();
    list.sort_by_key(|r| std::cmp::Reverse(numeric(&r.version)));
    list
}

/// Markdown emphasis, code ticks and links reduced to their text.
pub fn plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(['*', '`', '[']) {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        if let Some(r) = rest.strip_prefix("**") {
            rest = r;
        } else if let Some(r) = rest.strip_prefix('`') {
            rest = r;
        } else if rest.starts_with('[')
            && let Some(close) = rest.find("](")
            && let Some(end) = rest[close..].find(')')
        {
            out.push_str(&rest[1..close]);
            rest = &rest[close + end + 1..];
        } else {
            out.push_str(&rest[..1]);
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

/// Greedy word wrap at `width` columns, `first` before the first line and
/// `indent` before the rest; ends with a newline.
fn wrap(text: &str, width: usize, first: &str, indent: &str) -> String {
    let mut out = String::from(first);
    let mut column = first.chars().count();
    let mut line_start = true;
    for word in text.split_whitespace() {
        let len = word.chars().count();
        if !line_start && column + 1 + len > width {
            out.push('\n');
            out.push_str(indent);
            column = indent.chars().count();
            line_start = true;
        }
        if !line_start {
            out.push(' ');
            column += 1;
        }
        out.push_str(word);
        column += len;
        line_start = false;
    }
    out.push('\n');
    out
}

/// The changelog with its Unreleased notes released as `version` on
/// `date`, and a fresh empty Unreleased above them. When nobody wrote
/// notes, `fallback` items (the release's commit subjects) fill a Changes
/// section so a release is never without any.
pub fn stamp(text: &str, version: &str, date: &str, fallback: &[String]) -> String {
    let releases = parse(text);
    let notes_written = releases.iter().find(|r| r.is_unreleased()).is_some_and(|r| !r.is_empty());
    let mut fresh = format!("## {UNRELEASED}\n\n## {version} — {date}\n");
    if !notes_written && !fallback.is_empty() {
        fresh.push_str("\nThe changes in this release, taken from its commit history.\n\n### Changes\n\n");
        for item in fallback {
            fresh.push_str(&format!("- {item}\n"));
        }
    }
    let mut out = String::with_capacity(text.len() + fresh.len());
    let mut stamped = false;
    for line in text.lines() {
        if !stamped && heading(line).is_some_and(|(v, _)| v == UNRELEASED) {
            out.push_str(&fresh);
            stamped = true;
            continue;
        }
        // Before the first release heading when there was no Unreleased.
        if !stamped && heading(line).is_some() {
            out.push_str(&fresh);
            out.push('\n');
            stamped = true;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !stamped {
        out.push('\n');
        out.push_str(&fresh);
    }
    out
}

/// A commit subject as a changelog line: the Conventional Commits prefix
/// dropped and the first letter capitalized (`fix(ui): stop the leak` →
/// `Stop the leak.`). `None` for release bookkeeping.
pub fn commit_line(subject: &str) -> Option<String> {
    let subject = subject.trim();
    if subject.is_empty() || subject.starts_with("chore(release)") || subject.starts_with("Merge ") {
        return None;
    }
    let text = match subject.split_once(": ") {
        Some((kind, rest)) if !kind.contains(' ') => rest.trim(),
        _ => subject,
    };
    let mut chars = text.chars();
    let first = chars.next()?;
    let mut line: String = first.to_uppercase().chain(chars).collect();
    if !line.ends_with(['.', '!', '?']) {
        line.push('.');
    }
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Changelog\n\nIntro.\n\n## Unreleased\n\n## 0.2.0 — 2026-11-02\n\nA bigger one.\n\n### New\n\n- **Dock** magnifies,\n  smoothly.\n- A `mandara msg` command.\n\n### Fixed\n\n- See [issue 4](https://example.com/4).\n\n## 0.1.0 - 2026-10-29\n\nFirst.\n";

    #[test]
    fn parses_headings_summaries_sections_and_wrapped_items() {
        let all = parse(SAMPLE);
        assert_eq!(all.len(), 3);
        assert!(all[0].is_unreleased() && all[0].is_empty());
        let r = &all[1];
        assert_eq!((r.version.as_str(), r.date.as_deref(), r.summary.as_str()), ("0.2.0", Some("2026-11-02"), "A bigger one."));
        assert_eq!(r.sections[0].title, "New");
        assert_eq!(r.sections[0].items, vec!["**Dock** magnifies, smoothly.".to_string(), "A `mandara msg` command.".to_string()]);
        assert_eq!(r.sections[1].items.len(), 1);
        assert_eq!((all[2].version.as_str(), all[2].date.as_deref()), ("0.1.0", Some("2026-10-29")));
    }

    #[test]
    fn markdown_round_trips_through_release_notes() {
        let r = parse(SAMPLE).remove(1);
        let body = format!("{}\n\n---\n\nInstall with one command: …", r.to_markdown());
        let back = parse_notes("v0.2.0", &body);
        assert_eq!(back.summary, r.summary);
        assert_eq!(back.sections, r.sections);
        assert_eq!(back.version, "0.2.0");
    }

    #[test]
    fn plain_text_strips_markup_and_wraps() {
        assert_eq!(plain("**Dock** and `code` and [a link](https://x.y/z) *ok*"), "Dock and code and a link *ok*");
        let text = parse(SAMPLE)[1].to_text(24);
        assert!(text.starts_with("Mandara 0.2.0 · 2026-11-02\n"));
        assert!(text.contains("  • Dock magnifies,\n    smoothly.\n"), "{text}");
        // The heading line is never wrapped.
        assert!(text.lines().skip(1).all(|l| l.chars().count() <= 24), "{text}");
    }

    #[test]
    fn finds_releases_by_version_current_and_range() {
        let all = parse(SAMPLE);
        assert_eq!(release(&all, "v0.2.0").unwrap().version, "0.2.0");
        assert!(release(&all, "unreleased").unwrap().is_unreleased());
        assert!(release(&all, "0.3.0").is_none());
        assert_eq!(current(&all, "0.2.0").unwrap().version, "0.2.0");
        assert_eq!(current(&all, "0.2.1-dev").unwrap().version, "0.2.0");
        assert_eq!(current(&all, "0.2.0-dev").unwrap().version, "0.1.0");
        let range: Vec<&str> = between(&all, "0.0.9", "0.2.0").iter().map(|r| r.version.as_str()).collect();
        assert_eq!(range, ["0.2.0", "0.1.0"]);
        assert!(between(&all, "0.2.0", "0.2.0").is_empty());
    }

    #[test]
    fn stamping_releases_the_written_notes() {
        let text = "# Changelog\n\n## Unreleased\n\n### Fixed\n\n- A thing.\n\n## 0.1.0 — 2026-10-29\n\nFirst.\n";
        let out = stamp(text, "0.1.1", "2026-11-01", &["Ignored.".into()]);
        let all = parse(&out);
        assert!(all[0].is_unreleased() && all[0].is_empty());
        assert_eq!((all[1].version.as_str(), all[1].date.as_deref()), ("0.1.1", Some("2026-11-01")));
        assert_eq!(all[1].sections[0].items, vec!["A thing.".to_string()]);
        assert!(!out.contains("Ignored"));
        assert_eq!(all[2].summary, "First.");
    }

    #[test]
    fn stamping_without_notes_falls_back_to_commits() {
        let out = stamp("# Changelog\n\n## 0.1.0 — 2026-10-29\n\nFirst.\n", "0.1.1", "2026-11-01", &["Stop the leak.".into()]);
        let all = parse(&out);
        assert!(all[0].is_unreleased());
        assert!(!all[1].summary.is_empty());
        assert_eq!(all[1].sections[0].title, "Changes");
        assert_eq!(all[1].sections[0].items, vec!["Stop the leak.".to_string()]);
        assert_eq!(all[2].version, "0.1.0");
    }

    #[test]
    fn commit_subjects_become_sentences() {
        assert_eq!(commit_line("fix(ui): stop the picker leak").as_deref(), Some("Stop the picker leak."));
        assert_eq!(commit_line("feat: dock magnification!").as_deref(), Some("Dock magnification!"));
        assert_eq!(commit_line("Plain subject").as_deref(), Some("Plain subject."));
        assert_eq!(commit_line("chore(release): mandara v0.1.2"), None);
    }

    #[test]
    fn the_shipped_changelog_is_well_formed() {
        let all = parse(TEXT);
        assert!(all.first().is_some_and(Release::is_unreleased), "CHANGELOG.md must start with an Unreleased section");
        for r in all.iter().filter(|r| !r.is_unreleased()) {
            assert!(numeric(&r.version).is_some(), "{} isn't a version", r.version);
            assert!(r.date.as_deref().is_some_and(|d| d.len() == 10), "{} has no YYYY-MM-DD date", r.version);
            assert!(!r.summary.is_empty(), "{} has no summary", r.version);
            assert!(r.sections.iter().all(|s| !s.title.is_empty() && !s.items.is_empty()), "{} has an empty or untitled section", r.version);
        }
        assert!(release(&all, "0.1.0").is_some());
    }
}
