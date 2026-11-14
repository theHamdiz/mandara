//! Logical Hyprlang lines keep physical locations for field-scoped editing.

use anyhow::Result;
use std::borrow::Cow;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Continuation {
    None,
    Hyprland,
    Sway,
}

pub(crate) struct LogicalLine<'a> {
    pub number: usize,
    pub offset: usize,
    pub end: usize,
    pub last: usize,
    pub continued: bool,
    pub text: Cow<'a, str>,
}

pub(crate) struct LogicalLines<'a> {
    lines: std::str::SplitInclusive<'a, char>,
    number: usize,
    offset: usize,
    continuation: Continuation,
}

impl<'a> LogicalLines<'a> {
    pub fn new(source: &'a str, continuation: Continuation) -> Self {
        Self {
            lines: source.split_inclusive('\n'),
            number: 0,
            offset: 0,
            continuation,
        }
    }
}

impl<'a> Iterator for LogicalLines<'a> {
    type Item = Result<LogicalLine<'a>>;
    fn next(&mut self) -> Option<Self::Item> {
        let raw = self.lines.next()?;
        let (number, offset) = (self.number + 1, self.offset);
        self.number += 1;
        self.offset += raw.len();
        let mut text = Cow::Borrowed(raw.trim_end_matches('\n'));
        let mut continued = false;
        let mut newline = raw.ends_with('\n');
        while self.continuation != Continuation::None
            && (self.continuation == Continuation::Hyprland || newline)
            && text.ends_with('\\')
            && !(self.continuation == Continuation::Sway && text.starts_with('#'))
        {
            let Some(next) = self.lines.next() else {
                if self.continuation == Continuation::Sway {
                    break;
                }
                return Some(Err(anyhow::anyhow!(
                    mandara_i18n::t("A continued shortcut ends before its next line.")
                )));
            };
            continued = true;
            newline = next.ends_with('\n');
            self.number += 1;
            self.offset += next.len();
            let prefix = &text[..text.len() - 1];
            let prefix_len = if self.continuation == Continuation::Hyprland {
                prefix.trim_end_matches([' ', '\t']).len()
            } else {
                prefix.len()
            };
            let next = next.trim_end_matches('\n');
            if !prefix_len
                .checked_add(next.len())
                .is_some_and(|size| size <= super::MAX_FILE_BYTES)
            {
                return Some(Err(anyhow::anyhow!(
                    mandara_i18n::t("The shortcut configuration is too large to edit.")
                )));
            }
            let buffer = text.to_mut();
            buffer.truncate(prefix_len);
            buffer.reserve(next.len());
            buffer.push_str(next);
        }
        Some(Ok(LogicalLine {
            number,
            offset,
            end: self.offset,
            last: self.number,
            continued,
            text,
        }))
    }
}

pub(crate) fn followed_by_brace(source: &str, end: usize) -> bool {
    source[end..]
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        == Some("{")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn joining_preserves_physical_locations_and_borrows_uncontinued_lines() {
        let source = "# keep\nsource = \\\nkeys.conf\nbind = SUPER, Q, exec, foot\n";
        let lines = LogicalLines::new(source, Continuation::Hyprland)
            .collect::<Result<Vec<_>>>()
            .unwrap();
        assert!(matches!(lines[0].text, Cow::Borrowed(_)));
        assert_eq!(lines[1].text, "source =keys.conf");
        assert!(lines[1].continued);
        assert_eq!((lines[1].number, lines[2].number), (2, 4));
        assert_eq!(
            &source[lines[2].offset..lines[2].end],
            "bind = SUPER, Q, exec, foot\n"
        );
        assert!(
            LogicalLines::new(concat!("source = ", "\\"), Continuation::Hyprland)
                .next()
                .unwrap()
                .is_err()
        );
    }
    #[test]
    fn brace_lookahead_skips_comments_and_stays_with_the_next_statement() {
        assert!(!followed_by_brace("bindsym --locked\n# keep\n\n{\n", 17));
        assert!(followed_by_brace("bindsym --locked\n\n{\n", 17));
        assert!(!followed_by_brace("bindsym\nbindsym Mod4+q kill\n{\n", 8));
    }
}

#[cfg(test)]
mod sway_tests {
    use super::*;
    #[test]
    fn sway_preserves_spaces_before_continuations_and_unindented_comments_do_not_continue() {
        let source = "bindsym \\\nMod4+q kill\n# comment \\\nbindsym Mod4+w kill\n";
        let lines = LogicalLines::new(source, Continuation::Sway)
            .collect::<Result<Vec<_>>>()
            .unwrap();
        assert_eq!(lines[0].text, "bindsym Mod4+q kill");
        assert_eq!((lines[0].number, lines[0].last), (1, 2));
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[2].number, 4);
    }
    #[test]
    fn sway_does_not_join_a_backslash_without_a_following_physical_line() {
        for source in [
            concat!("bindsym Mod4+q exec echo ", "\\"),
            concat!("bindsym Mod4+q exec echo ", "\\", "\n"),
        ] {
            let line = LogicalLines::new(source, Continuation::Sway)
                .next()
                .unwrap()
                .unwrap();
            assert!(!line.continued);
            assert!(line.text.ends_with('\\'));
        }
    }
}
