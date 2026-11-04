//! The installer's terminal face: the Mandara wordmark, the frame of the
//! arrow-key menu and the keys that menu understands.
//!
//! Everything here is a pure function of its arguments (no terminal writes),
//! so the drawing and the key sequences are tested without a terminal.

use crate::host::Row;

/// The wordmark, [`ART_WIDTH`] columns wide.
const ART: [&str; 6] = [
    "███╗   ███╗  █████╗  ███╗   ██╗ ██████╗   █████╗  ██████╗   █████╗ ",
    "████╗ ████║ ██╔══██╗ ████╗  ██║ ██╔══██╗ ██╔══██╗ ██╔══██╗ ██╔══██╗",
    "██╔████╔██║ ███████║ ██╔██╗ ██║ ██║  ██║ ███████║ ██████╔╝ ███████║",
    "██║╚██╔╝██║ ██╔══██║ ██║╚██╗██║ ██║  ██║ ██╔══██║ ██╔══██╗ ██╔══██║",
    "██║ ╚═╝ ██║ ██║  ██║ ██║ ╚████║ ██████╔╝ ██║  ██║ ██║  ██║ ██║  ██║",
    "╚═╝     ╚═╝ ╚═╝  ╚═╝ ╚═╝  ╚═══╝ ╚═════╝  ╚═╝  ╚═╝ ╚═╝  ╚═╝ ╚═╝  ╚═╝",
];

/// How many columns the wordmark takes.
pub(crate) const ART_WIDTH: usize = 67;

/// The title when the run has no terminal: one line, no art.
const PLAIN_TITLE: &str = "MANDARA";

/// The hint under every menu.
const HINT: &str = "\n  ↑/↓ change · enter select · esc cancel";

/// The title block: the wordmark, a rule, then `label`. On a terminal too
/// narrow for the art, [`PLAIN_TITLE`] replaces it rather than wrapping.
pub(crate) fn banner(columns: usize, label: &str) -> String {
    if columns >= ART_WIDTH {
        let mut out = ART.join("\n");
        out.push('\n');
        out.push_str(&"─".repeat(ART_WIDTH));
        out.push('\n');
        out.push_str(label);
        return out;
    }
    // Too narrow even for the rule: the plain title, nothing that wraps.
    if columns < PLAIN_TITLE.len() {
        return format!("{PLAIN_TITLE}\n{label}");
    }
    format!("{PLAIN_TITLE}\n{}\n{label}", "─".repeat(columns.min(PLAIN_TITLE.len())))
}

/// The menu as a block of lines: the prompt, each row with its value in a
/// column of its own, and the key hints. `cursor` is the highlighted row.
pub(crate) fn menu_frame(prompt: &str, rows: &[Row], cursor: usize) -> String {
    let width = rows.iter().map(|r| r.label.chars().count()).max().unwrap_or_default();
    let mut out = format!("\n  {prompt}\n\n");
    for (i, row) in rows.iter().enumerate() {
        let marker = if i == cursor { "▸" } else { " " };
        let label = format!("{:<width$}", row.label, width = width);
        let line = if row.value.is_empty() { format!("  {marker} {label}") } else { format!("  {marker} {label}  {}", row.value) };
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out.push_str(HINT);
    out
}

/// A key the menu acts on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Key {
    Up,
    Down,
    Enter,
    /// Escape, `q`, Ctrl-C or Ctrl-D: leave the screen without changing
    /// anything.
    Cancel,
    /// A key the menu ignores.
    Other,
}

/// Reads one key from the start of `buf`, returning it with the bytes it
/// used. `None` means the bytes so far are an incomplete sequence (a lone
/// escape could still become an arrow), so the caller waits for more.
pub(crate) fn decode(buf: &[u8]) -> Option<(Key, usize)> {
    let first = *buf.first()?;
    match first {
        b'\x1b' => match buf.get(1) {
            None => None,
            Some(b'[') | Some(b'O') => match buf.get(2) {
                None => None,
                // Left/right (C, D) and anything else are ignored.
                Some(b'A') => Some((Key::Up, 3)),
                Some(b'B') => Some((Key::Down, 3)),
                Some(_) => Some((Key::Other, 3)),
            },
            Some(_) => Some((Key::Cancel, 1)),
        },
        b'\r' | b'\n' => Some((Key::Enter, 1)),
        b'k' | b'K' => Some((Key::Up, 1)),
        b'j' | b'J' => Some((Key::Down, 1)),
        // q, Ctrl-C, Ctrl-D (end of input in some terminals).
        b'q' | b'Q' | 0x03 | 0x04 => Some((Key::Cancel, 1)),
        _ => Some((Key::Other, 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wordmark_is_a_rectangle() {
        for row in ART {
            assert_eq!(row.chars().count(), ART_WIDTH, "{row}");
        }
    }

    #[test]
    fn the_banner_never_wraps() {
        let full = banner(100, "Mandara Shell installer · Arch");
        assert!(full.starts_with(ART[0]) && full.ends_with("\nMandara Shell installer · Arch"), "{full}");
        let narrow = banner(40, "Mandara Shell installer · Arch");
        assert!(!narrow.contains(ART[0]), "{narrow}");
        assert_eq!(narrow.lines().count(), 3);
        assert!(narrow.lines().all(|l| l.chars().count() <= 40), "{narrow}");
        // A terminal narrower than the plain title still gets a rule no wider than it.
        assert!(banner(10, "x").lines().all(|l| l.chars().count() <= PLAIN_TITLE.len()), "{}", banner(10, "x"));
    }

    #[test]
    fn rows_and_values_line_up() {
        let rows = [Row::new("Compositor", "Niri (installed)"), Row::new("Install", "")];
        let frame = menu_frame("Setup", &rows, 0);
        assert!(frame.contains("  ▸ Compositor  Niri (installed)"), "{frame}");
        assert!(frame.contains("\n    Install\n"), "{frame}");
        assert!(frame.contains("↑/↓ change"), "{frame}");
        // The value of the first row starts two columns after the label.
        let line = frame.lines().find(|l| l.contains("Compositor")).unwrap();
        let value_column = line[..line.find("Niri").unwrap()].chars().count();
        assert_eq!(value_column, 2 + 2 + "Compositor".chars().count() + 2);
    }

    #[test]
    fn decodes_the_keys_the_menu_uses() {
        assert_eq!(decode(b"\x1b[A"), Some((Key::Up, 3)));
        assert_eq!(decode(b"\x1b[B"), Some((Key::Down, 3)));
        assert_eq!(decode(b"\x1bOA"), Some((Key::Up, 3)));
        assert_eq!(decode(b"\x1b[C"), Some((Key::Other, 3)));
        assert_eq!(decode(b"\r"), Some((Key::Enter, 1)));
        assert_eq!(decode(b"k"), Some((Key::Up, 1)));
        assert_eq!(decode(b"J"), Some((Key::Down, 1)));
        assert_eq!(decode(b"q"), Some((Key::Cancel, 1)));
        assert_eq!(decode(b"\x03"), Some((Key::Cancel, 1)));
        assert_eq!(decode(b"\x04"), Some((Key::Cancel, 1)));
        assert_eq!(decode(b"z"), Some((Key::Other, 1)));
    }

    #[test]
    fn incomplete_sequences_wait_for_more_bytes() {
        assert_eq!(decode(b""), None);
        // A lone escape may be the start of an arrow key.
        assert_eq!(decode(b"\x1b"), None);
        assert_eq!(decode(b"\x1b["), None);
        assert_eq!(decode(b"\x1b\x1b"), Some((Key::Cancel, 1)));
    }
}
