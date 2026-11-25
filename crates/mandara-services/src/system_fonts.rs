//! Loads the system's fonts into a `fontdb` database. FreeBSD keeps
//! fontconfig's files under `/usr/local/etc/fonts`, where fontdb doesn't
//! look, so there the font folders FreeBSD's packages install into are
//! read directly (the same folders fontdb falls back to, without its
//! warning).

/// Font folders on FreeBSD: packages, then the user's own.
#[cfg(target_os = "freebsd")]
const FREEBSD_DIRS: [&str; 1] = ["/usr/local/share/fonts"];

pub fn load(db: &mut fontdb::Database) {
    #[cfg(target_os = "freebsd")]
    {
        for dir in FREEBSD_DIRS {
            db.load_fonts_dir(dir);
        }
        if let Some(home) = dirs::home_dir() {
            db.load_fonts_dir(home.join(".fonts"));
            db.load_fonts_dir(home.join(".local/share/fonts"));
        }
    }
    #[cfg(not(target_os = "freebsd"))]
    db.load_system_fonts();
}
