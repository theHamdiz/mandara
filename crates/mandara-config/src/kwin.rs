//! KWin owns its settings; Mandara installs only its own XDG autostart entry.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

/// A separate desktop id keeps Studio's application-menu entry unchanged.
const AUTOSTART_ID: &str = "org.mandara.shell.desktop";
/// Only files carrying this marker belong to this integration.
const MARKER: &str = "X-Mandara-KWin=true";

fn exec_value(executable: &Path) -> Result<String> {
    let text = executable.to_str().ok_or_else(|| anyhow::anyhow!(mandara_i18n::t("This executable path cannot be used in a desktop entry")))?;
    if text.contains('=') || text.chars().any(char::is_control) {
        bail!("{}", mandara_i18n::t("This executable path cannot be used in a desktop entry"));
    }
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str(r"\\\\"),
            '"' | '`' | '$' => { out.push_str(r"\\"); out.push(ch); }
            '%' => out.push_str("%%"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    Ok(out)
}

pub fn autostart_file() -> PathBuf {
    crate::apps::autostart_dirs().0.join(AUTOSTART_ID)
}

pub fn autostart_installed() -> bool {
    std::fs::read_to_string(autostart_file()).is_ok_and(|t| t.lines().any(|l| l == MARKER) && !t.lines().any(|l| l == "Hidden=true"))
}

pub fn sync_autostart(enabled: bool, executable: &Path) -> Result<Option<PathBuf>> {
    sync_autostart_at(&autostart_file(), enabled, executable)
}

fn sync_autostart_at(path: &Path, enabled: bool, executable: &Path) -> Result<Option<PathBuf>> {
    let current = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    if current.is_empty() && !enabled { return Ok(None); }
    if !current.is_empty() && !current.lines().any(|l| l == MARKER) {
        bail!("{}", mandara_i18n::tf("{path} is a custom autostart entry; Mandara left it unchanged", &[("path", &path.display())]));
    }
    let text = format!("[Desktop Entry]\nType=Application\nName=Mandara Shell\nExec={}\nOnlyShowIn=KDE;\nTerminal=false\nHidden={}\n{MARKER}\n", exec_value(executable)?, !enabled);
    if current != text { crate::store::write_atomic(path, text.as_bytes())?; }
    Ok(Some(path.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_is_opt_in_and_preserves_custom_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(AUTOSTART_ID);
        assert_eq!(sync_autostart_at(&path, false, Path::new("/bin/mandara")).unwrap(), None);
        assert!(!path.exists());
        sync_autostart_at(&path, true, Path::new("/bin/mandara")).unwrap();
        let enabled = std::fs::read_to_string(&path).unwrap();
        assert!(enabled.contains("OnlyShowIn=KDE;\n") && enabled.contains("Hidden=false\n"));
        sync_autostart_at(&path, false, Path::new("/bin/mandara")).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("Hidden=true\n"));
        std::fs::write(&path, "[Desktop Entry]\nExec=my-command\n").unwrap();
        assert!(sync_autostart_at(&path, true, Path::new("/bin/mandara")).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "[Desktop Entry]\nExec=my-command\n");
    }

    #[test]
    fn executable_paths_follow_desktop_entry_quoting_rules() {
        assert_eq!(exec_value(Path::new("/home/a b/100%/mandara")).unwrap(), "\"/home/a b/100%%/mandara\"");
        assert_eq!(exec_value(Path::new("/tmp/a\\b$`\"/mandara")).unwrap(), "\"/tmp/a\\\\\\\\b\\\\$\\\\`\\\\\"/mandara\"");
        assert!(exec_value(Path::new("/tmp/a=b/mandara")).is_err());
        assert!(exec_value(Path::new("/tmp/a\nb/mandara")).is_err());
    }
}
