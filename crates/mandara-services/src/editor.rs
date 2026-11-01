//! Opening a file at a line in the user's editor: `$VISUAL`, else
//! `$EDITOR` (terminal editors run in the preferred terminal), else the
//! desktop's default app for the file through the portal.

use std::path::Path;

/// Editors that run in a terminal, with how they take a line.
const TERMINAL_EDITORS: &[(&str, LineArg)] = &[
    ("vi", LineArg::Plus),
    ("vim", LineArg::Plus),
    ("nvim", LineArg::Plus),
    ("nano", LineArg::Plus),
    ("micro", LineArg::Plus),
    ("emacs", LineArg::Plus),
    ("kak", LineArg::Plus),
    ("joe", LineArg::Plus),
    ("ne", LineArg::Plus),
    ("hx", LineArg::Colon),
    ("helix", LineArg::Colon),
];

/// Graphical editors, with how they take a line.
const GUI_EDITORS: &[(&str, LineArg)] = &[
    ("code", LineArg::Goto),
    ("codium", LineArg::Goto),
    ("code-oss", LineArg::Goto),
    ("vscodium", LineArg::Goto),
    ("zed", LineArg::Colon),
    ("zeditor", LineArg::Colon),
    ("subl", LineArg::Colon),
    ("kate", LineArg::DashL),
    ("kwrite", LineArg::DashL),
    ("gedit", LineArg::Plus),
    ("gnome-text-editor", LineArg::Plus),
    ("mousepad", LineArg::Plus),
    ("pluma", LineArg::Plus),
    ("xed", LineArg::Plus),
    ("gvim", LineArg::Plus),
];

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum LineArg {
    /// `+12 file`
    Plus,
    /// `file:12`
    Colon,
    /// `--goto file:12`
    Goto,
    /// `-l 12 file`
    DashL,
}

fn with_line(how: LineArg, file: &str, line: usize) -> Vec<String> {
    match how {
        LineArg::Plus => vec![format!("+{line}"), file.to_string()],
        LineArg::Colon => vec![format!("{file}:{line}")],
        LineArg::Goto => vec!["--goto".into(), format!("{file}:{line}")],
        LineArg::DashL => vec!["-l".into(), line.to_string(), file.to_string()],
    }
}

/// The command line that opens `file` at `line` with `editor` (`$VISUAL` /
/// `$EDITOR`, possibly with flags), wrapped in `terminal` when it is a
/// terminal editor. `None` when there is no editor, or a terminal editor
/// and no terminal.
pub fn editor_argv(editor: Option<&str>, terminal: Option<&str>, file: &Path, line: usize) -> Option<Vec<String>> {
    let words: Vec<String> = editor?.split_whitespace().map(str::to_string).collect();
    let program = words.first()?;
    let base = Path::new(program).file_name().and_then(|n| n.to_str()).unwrap_or(program);
    let file = file.display().to_string();
    if let Some((_, how)) = GUI_EDITORS.iter().find(|(n, _)| *n == base) {
        return Some(words.iter().cloned().chain(with_line(*how, &file, line)).collect());
    }
    let how = TERMINAL_EDITORS.iter().find(|(n, _)| *n == base).map(|(_, h)| *h).unwrap_or(LineArg::Plus);
    let command: Vec<String> = words.iter().cloned().chain(with_line(how, &file, line)).collect();
    let script = command.iter().map(|w| crate::updates::sh_quote(w)).collect::<Vec<_>>().join(" ");
    Some(crate::updates::terminal_argv(terminal?, &script))
}

/// Opens `file` at `line`: `$VISUAL` / `$EDITOR` (in `terminal` for a
/// terminal editor), else the default app for the file.
pub fn open_at(file: &Path, line: usize, terminal: Option<&str>) -> Result<(), String> {
    let editor = std::env::var("VISUAL").ok().filter(|v| !v.trim().is_empty()).or_else(|| std::env::var("EDITOR").ok().filter(|v| !v.trim().is_empty()));
    if let Some(argv) = editor_argv(editor.as_deref(), terminal, file, line) {
        use std::os::unix::process::CommandExt as _;
        return crate::reaper::spawn(std::process::Command::new(&argv[0]).args(&argv[1..]).process_group(0)).map(|_| ()).map_err(|e| e.to_string());
    }
    crate::session::open_uri(&format!("file://{}", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editors_open_at_the_line() {
        let f = Path::new("/home/u/.config/mango/config.conf");
        assert_eq!(editor_argv(Some("code --wait"), None, f, 12).unwrap(), ["code", "--wait", "--goto", "/home/u/.config/mango/config.conf:12"]);
        assert_eq!(editor_argv(Some("/usr/bin/kate"), None, f, 3).unwrap(), ["/usr/bin/kate", "-l", "3", "/home/u/.config/mango/config.conf"]);
        let vim = editor_argv(Some("nvim"), Some("foot"), f, 7).unwrap();
        assert_eq!(&vim[..3], ["foot", "sh", "-c"]);
        assert!(vim[3].contains("nvim") && vim[3].contains("+7"), "{vim:?}");
        assert!(editor_argv(Some("nvim"), None, f, 7).is_none(), "a terminal editor needs a terminal");
        assert!(editor_argv(None, Some("foot"), f, 7).is_none());
        let hx = editor_argv(Some("hx"), Some("kitty"), f, 2).unwrap();
        assert!(hx[3].contains("config.conf:2"), "{hx:?}");
    }
}
