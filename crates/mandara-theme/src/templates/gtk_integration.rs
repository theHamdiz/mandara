//! GTK imports and their generated stylesheet share the same sandbox visibility.

use std::io;
use std::path::{Component, Path, PathBuf};

use super::{Ledger, Placement};

/// These GTK 4 clients have their own Flatpak configuration namespace.
const GTK_FLATPAKS: [&str; 2] = ["io.github.screwys.Rufin", "io.github.celluloid_player.Celluloid"];

/// Find existing app homes without creating configuration for unused clients.
/// `home` is explicit so exports and tests never consult another user's state.
pub fn gtk_flatpak_directories(home: &Path, version: &str) -> io::Result<Vec<PathBuf>> {
    if version != "gtk-4.0" { return Ok(Vec::new()); }
    let mut directories = Vec::with_capacity(GTK_FLATPAKS.len());
    for app in GTK_FLATPAKS {
        let root = home.join(".var/app").join(app);
        match std::fs::metadata(&root) {
            Ok(metadata) if metadata.is_dir() => directories.push(root.join("config").join(version)),
            Ok(_) => return Err(io::Error::new(io::ErrorKind::NotADirectory, format!("{} is not an app directory", root.display()))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {},
            Err(error) => return Err(error),
        }
    }
    Ok(directories)
}

/// Install an owned stylesheet beside `gtk.css`, preserving personal CSS and Undo.
pub fn install_gtk_stylesheet(
    ledger: &mut Ledger,
    owner: &str,
    directory: &Path,
    filename: &str,
    body: &str,
) -> io::Result<[PathBuf; 2]> {
    let mut components = Path::new(filename).components();
    if !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
        || filename == "gtk.css"
    {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid GTK stylesheet name"));
    }
    let path = directory.join(filename);
    let import = directory.join("gtk.css");
    let escaped = path.to_str().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "GTK stylesheet path is not UTF-8"))?
        .replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\a ").replace('\r', "\\d ").replace('\u{c}', "\\c ");
    let line = format!("@import url(\"{escaped}\");");
    ledger.put_file(owner, &path, body)?;
    ledger.put_block(owner, &import, &[&line], Placement::AfterImports)?;
    Ok([path, import])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatpak_styles_follow_only_existing_gtk4_app_homes_and_undo_personal_css() {
        let root = tempfile::tempdir().unwrap();
        assert!(gtk_flatpak_directories(root.path(), "gtk-4.0").unwrap().is_empty());
        let app = root.path().join(".var/app").join(GTK_FLATPAKS[0]);
        std::fs::create_dir_all(&app).unwrap();
        let directories = gtk_flatpak_directories(root.path(), "gtk-4.0").unwrap();
        assert_eq!(directories, [app.join("config/gtk-4.0")]);
        assert!(gtk_flatpak_directories(root.path(), "gtk-3.0").unwrap().is_empty());
        let directory = &directories[0];
        std::fs::create_dir_all(directory).unwrap();
        let original = "/* personal */\nlabel { padding: 2px; }\n";
        std::fs::write(directory.join("gtk.css"), original).unwrap();
        let mut ledger = Ledger::default();
        install_gtk_stylesheet(&mut ledger, "gtk", directory, "mandara-colors.css", "/* Mandara Shell */\n").unwrap();
        assert!(std::fs::read_to_string(directory.join("gtk.css")).unwrap().contains(directory.join("mandara-colors.css").to_str().unwrap()));
        assert!(!root.path().join(".var/app").join(GTK_FLATPAKS[1]).exists());
        assert!(ledger.take_back_checked("gtk").errors.is_empty());
        assert_eq!(std::fs::read_to_string(directory.join("gtk.css")).unwrap(), original);
        assert!(!directory.join("mandara-colors.css").exists());
        std::fs::write(root.path().join(".var/app").join(GTK_FLATPAKS[1]), "not a directory").unwrap();
        assert_eq!(gtk_flatpak_directories(root.path(), "gtk-4.0").unwrap_err().kind(), io::ErrorKind::NotADirectory);
    }

    #[test]
    fn names_cannot_escape_the_gtk_directory_or_replace_the_import() {
        let mut ledger = Ledger::default();
        for filename in ["", "../colors.css", "nested/colors.css", "/colors.css", "gtk.css", "."] {
            assert!(install_gtk_stylesheet(&mut ledger, "gtk", Path::new("/unused"), filename, "").is_err());
        }
        assert!(ledger.is_empty());
    }

    #[test]
    fn paths_are_lossless_and_css_line_breaks_cannot_escape_the_import() {
        use std::os::unix::ffi::OsStringExt as _;
        let root = tempfile::tempdir().unwrap();
        let invalid = root.path().join(std::ffi::OsString::from_vec(vec![b'g', 0xff]));
        let mut ledger = Ledger::default();
        assert!(install_gtk_stylesheet(&mut ledger, "gtk", &invalid, "colors.css", "colors").is_err());
        assert!(!invalid.exists());
        assert!(ledger.is_empty());
        let directory = root.path().join("quoted\"\\\n\r\u{c}");
        install_gtk_stylesheet(&mut ledger, "gtk", &directory, "colors.css", "colors").unwrap();
        let import = std::fs::read_to_string(directory.join("gtk.css")).unwrap();
        assert_eq!(import.lines().count(), 1);
        for escaped in ["\\\"", "\\\\", "\\a ", "\\d ", "\\c "] { assert!(import.contains(escaped)); }
    }

    #[test]
    fn legacy_import_migrates_without_losing_personal_css_or_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("gtk-4.0");
        std::fs::create_dir(&directory).unwrap();
        let personal = root.path().join("personal.css");
        let original = "/* my theme */\n@import 'my-colors.css';\nbutton { padding: 3px; }\n";
        std::fs::write(&personal, original).unwrap();
        let import = directory.join("gtk.css");
        std::os::unix::fs::symlink(&personal, &import).unwrap();
        let mut ledger = Ledger::default();
        let legacy = "@import url(\"file:///unavailable/cache/gtk.css\");";
        ledger.put_block("gtk", &import, &[legacy], Placement::AfterImports).unwrap();
        let local = directory.join("mandara-colors.css");
        let personal_colors = "/* my existing file */\n";
        std::fs::write(&local, personal_colors).unwrap();
        install_gtk_stylesheet(&mut ledger, "gtk", &directory, "mandara-colors.css", "/* Mandara Shell */\n@define-color accent_color red;\n").unwrap();
        let migrated = std::fs::read_to_string(&personal).unwrap();
        assert!(!migrated.contains(legacy));
        assert!(migrated.contains(local.to_str().unwrap()));
        assert!(migrated.contains("button { padding: 3px; }"));
        assert_eq!(std::fs::read_link(&import).unwrap(), personal);
        assert!(local.is_file(), "the config-visible stylesheet does not depend on cache access");
        assert!(ledger.take_back_checked("gtk").errors.is_empty());
        assert_eq!(std::fs::read_to_string(&personal).unwrap(), original);
        assert_eq!(std::fs::read_to_string(&local).unwrap(), personal_colors);
        assert!(ledger.is_empty());
    }
}
