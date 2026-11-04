//! Installer-owned PATH entries in shell profiles.

use std::path::{Path, PathBuf};

/// Marks the line [`add_to_path`] writes after it, so a second install adds
/// nothing and an install to another folder replaces the old line.
pub(super) const PATH_MARK: &str = "# Added by the Mandara installer";

/// `text` as one POSIX shell word: single-quoted, each `'` written `'\''`,
/// so `$`, backticks, `"`, `\` and spaces stay literal.
pub(super) fn sh_word(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// `text` as one fish argument: single-quoted, with `\` and `'` escaped.
pub(super) fn fish_word(text: &str) -> String {
    format!("'{}'", text.replace('\\', r"\\").replace('\'', r"\'"))
}

/// The POSIX line that puts `dir` first on `PATH` unless it's already there.
pub(super) fn posix_path_line(dir: &str) -> String {
    let word = sh_word(dir);
    format!("case \":$PATH:\" in *:{word}:*) ;; *) export PATH={word}:\"$PATH\" ;; esac")
}

/// The line to suggest when Mandara doesn't edit the profiles itself.
pub(super) fn path_hint(dir: &Path) -> String {
    format!("export PATH={}:\"$PATH\"", sh_word(&dir.to_string_lossy()))
}

/// Whether the line following a marker is an installer PATH command.
fn owned_line(line: &str) -> bool {
    let line = line.trim_end();
    (line.starts_with("case \":$PATH:\" in *:") && line.ends_with(":\"$PATH\" ;; esac"))
        || (line.starts_with("export PATH=") && (line.ends_with(":$PATH\"") || line.ends_with(":\"$PATH\"")))
        || line.starts_with("fish_add_path --path ")
}

/// Replaces marked PATH commands while preserving unrelated user lines.
fn replace_marked(text: &str, line: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len() + line.len());
    let mut lines = text.split_inclusive('\n').peekable();
    let mut found = false;
    while let Some(l) = lines.next() {
        out.push_str(l);
        if l.trim_end() == PATH_MARK {
            found = true;
            if !l.ends_with('\n') {
                out.push('\n');
            }
            if lines.peek().is_some_and(|next| owned_line(next)) {
                lines.next();
            }
            out.push_str(line);
            out.push('\n');
        }
    }
    found.then_some(out)
}

/// Puts `bin_dir` on `PATH` in `~/.profile`, `~/.bashrc`, an existing
/// `~/.zshrc` and, when fish is set up, a fish `conf.d` snippet. Returns the
/// files it changed: a file that already puts this folder on `PATH` is left
/// alone, and an older installer line for another folder is replaced.
pub(super) fn add_to_path(home: &Path, bin_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    update(home, bin_dir, false)
}

/// Refreshes marked entries without adding profiles for a directory
/// already on PATH.
pub(super) fn reconcile(home: &Path, bin_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    update(home, bin_dir, true)
}

fn update(home: &Path, bin_dir: &Path, marked_only: bool) -> std::io::Result<Vec<PathBuf>> {
    use std::io::{ErrorKind, Write as _};
    let dir = bin_dir.to_string_lossy();
    let posix = posix_path_line(&dir);
    let mut targets = vec![(home.join(".profile"), posix.clone()), (home.join(".bashrc"), posix.clone())];
    if home.join(".zshrc").is_file() {
        targets.push((home.join(".zshrc"), posix));
    }
    let fish = home.join(".config/fish");
    if fish.is_dir() {
        targets.push((fish.join("conf.d/mandara-path.fish"), format!("fish_add_path --path {}", fish_word(&dir))));
    }
    let mut changed = Vec::new();
    for (file, line) in targets {
        let current = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        let updated = match replace_marked(&current, &line) {
            Some(updated) => updated,
            None if marked_only || current.lines().any(|l| l == line) => continue,
            None => format!("{current}{}{PATH_MARK}\n{line}\n", if current.is_empty() || current.ends_with('\n') { "" } else { "\n" }),
        };
        if current == updated {
            continue;
        }
        replace_file(&file, |staged| staged.write_all(updated.as_bytes()))?;
        changed.push(file);
    }
    Ok(changed)
}

/// Stages beside the target and preserves a profile's symlink and mode.
fn replace_file(path: &Path, write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind};
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(m) => Some(m),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let target = if metadata.as_ref().is_some_and(|m| m.is_symlink()) { std::fs::canonicalize(path)? } else { path.to_path_buf() };
    let parent = target.parent().ok_or_else(|| Error::other("profile has no parent directory"))?;
    std::fs::create_dir_all(parent)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    if let Ok(m) = std::fs::metadata(&target) {
        staged.as_file().set_permissions(m.permissions())?;
    }
    write(staged.as_file_mut())?;
    staged.as_file().sync_all()?;
    staged.persist(&target).map_err(|e| e.error)?;
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Write as _};
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    #[test]
    fn retained_markers_preserve_user_lines_and_a_missing_newline() {
        for text in [format!("{PATH_MARK}\nexport EDITOR=vi\n"), PATH_MARK.to_string()] {
            let updated = replace_marked(&text, "new path").unwrap();
            assert!(updated.starts_with(&format!("{PATH_MARK}\nnew path\n")));
            assert_eq!(updated.contains("export EDITOR=vi"), text.contains("export EDITOR=vi"));
        }
    }

    #[test]
    fn replacement_keeps_symlinks_and_permissions() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("actual-profile");
        std::fs::write(&target, format!("{PATH_MARK}\nexport PATH=\"/old:$PATH\"\nexport EDITOR=vi\n")).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).unwrap();
        let link = home.path().join(".profile");
        symlink("actual-profile", &link).unwrap();
        add_to_path(home.path(), Path::new("/new/bin")).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o640);
        let text = std::fs::read_to_string(&target).unwrap();
        assert!(text.contains("/new/bin") && text.contains("export EDITOR=vi") && !text.contains("/old"));
    }

    #[test]
    fn failed_staging_preserves_the_original_and_removes_the_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join(".profile");
        std::fs::write(&profile, "original\n").unwrap();
        assert!(replace_file(&profile, |file| {
            file.write_all(b"partial")?;
            Err(io::Error::other("write failed"))
        }).is_err());
        assert_eq!(std::fs::read_to_string(profile).unwrap(), "original\n");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn reconciling_updates_only_existing_marked_profiles() {
        let home = tempfile::tempdir().unwrap();
        add_to_path(home.path(), Path::new("/old/bin")).unwrap();
        std::fs::write(home.path().join(".bashrc"), "export EDITOR=vi\n").unwrap();
        let changed = reconcile(home.path(), Path::new("/new/bin")).unwrap();
        assert_eq!(changed, [home.path().join(".profile")]);
        assert_eq!(std::fs::read_to_string(home.path().join(".bashrc")).unwrap(), "export EDITOR=vi\n");
        assert!(reconcile(home.path(), Path::new("/new/bin")).unwrap().is_empty());
    }
}
