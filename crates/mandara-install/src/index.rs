//! Package databases (sync databases, package lists, repository trees):
//! whether they are usable before installing anything, and how each
//! package manager brings them up to date.
//!
//! A system whose databases were never synced can't install anything:
//! pacman reports every package as "target not found" (issue #45). The
//! check reads files only, so it works the same in every locale and in a
//! dry run, and tests point it at a fixture root.
//!
//! | Manager | Checked before installing | Brought up to date with |
//! |---|---|---|
//! | pacman | `<DBPath>/sync/<repo>.db` of every repo in `pacman.conf` | `pacman -Syu` (a full upgrade, asked first) |
//! | apt-get | a `*_Packages` list in `/var/lib/apt/lists` | `apt-get update` (always run before installing) |
//! | emerge | the Gentoo repository tree | `emerge --sync` |
//! | dnf, zypper, apk, eopkg | nothing: they fetch metadata themselves | `dnf makecache --refresh`, `zypper refresh`, `apk update`, `eopkg update-repo`, only after an install failed |
//! | xbps | nothing: `xbps-install -S` syncs on every install | — |

use std::path::{Path, PathBuf};

use crate::pkg::PackageManager;

/// pacman's database folder when `pacman.conf` doesn't set `DBPath`.
const PACMAN_DB_PATH: &str = "/var/lib/pacman/";
const PACMAN_CONF: &str = "/etc/pacman.conf";
/// The `pacman.conf` section that holds options, not a repository.
const PACMAN_OPTIONS: &str = "options";
/// Nested `Include =` files followed at most (pacman.conf includes are
/// shallow; this only stops a loop).
const MAX_INCLUDE_DEPTH: usize = 8;
const APT_LISTS: &str = "/var/lib/apt/lists";
/// Marks a downloaded apt package index (`…_binary-amd64_Packages`, also
/// when stored compressed as `…_Packages.lz4`).
const APT_PACKAGES_MARK: &str = "_Packages";
/// Where the Gentoo repository lives (the default, then the old default);
/// `profiles/repo_name` is in every synced tree.
const GENTOO_TREES: &[&str] = &["/var/db/repos/gentoo", "/usr/portage"];
const GENTOO_TREE_MARK: &str = "profiles/repo_name";

/// The package databases that are missing or unusable (empty), by name
/// (`core`, `extra`, `package lists`…). Empty when they look fine or the
/// manager keeps its metadata up to date itself.
pub fn missing(pm: PackageManager, root: &Path) -> Vec<String> {
    match pm {
        PackageManager::Pacman => {
            let (db_path, repos) = pacman_repos(root);
            let sync = sys(root, &db_path).join("sync");
            repos.into_iter().filter(|repo| std::fs::metadata(sync.join(format!("{repo}.db"))).map(|m| m.len() == 0).unwrap_or(true)).collect()
        }
        PackageManager::AptGet => {
            let lists = std::fs::read_dir(sys(root, APT_LISTS)).into_iter().flatten().flatten().any(|e| e.file_name().to_string_lossy().contains(APT_PACKAGES_MARK));
            if lists { Vec::new() } else { vec!["package lists".to_string()] }
        }
        PackageManager::Emerge => {
            let synced = GENTOO_TREES.iter().any(|t| sys(root, t).join(GENTOO_TREE_MARK).is_file());
            if synced { Vec::new() } else { vec!["Gentoo repository".to_string()] }
        }
        PackageManager::Dnf | PackageManager::Zypper | PackageManager::Xbps | PackageManager::Apk | PackageManager::Eopkg => Vec::new(),
    }
}

/// argv bringing the package databases up to date (run it as root), or
/// `None` when the install command already does (xbps).
///
/// pacman gets a full `-Syu`, never a bare `-Sy`: syncing the database and
/// then installing without upgrading is a partial upgrade (new packages
/// built against libraries newer than the installed ones), which Arch
/// doesn't support. `unattended` adds `--noconfirm`; otherwise pacman
/// shows the upgrade and asks itself.
pub fn sync_argv(pm: PackageManager, unattended: bool) -> Option<Vec<String>> {
    let argv: &[&str] = match pm {
        PackageManager::Pacman if unattended => &["pacman", "-Syu", "--noconfirm"],
        PackageManager::Pacman => &["pacman", "-Syu"],
        PackageManager::AptGet => &["apt-get", "update"],
        PackageManager::Dnf => &["dnf", "makecache", "--refresh"],
        PackageManager::Zypper => &["zypper", "--non-interactive", "refresh"],
        PackageManager::Apk => &["apk", "update"],
        PackageManager::Emerge => &["emerge", "--sync"],
        PackageManager::Eopkg => &["eopkg", "update-repo"],
        PackageManager::Xbps => return None,
    };
    Some(argv.iter().map(|a| a.to_string()).collect())
}

/// Whether [`sync_argv`] also upgrades every installed package (asked
/// first, since it is more than the installer's own business).
pub fn sync_upgrades_system(pm: PackageManager) -> bool {
    pm == PackageManager::Pacman
}

/// Whether the databases are brought up to date before every install
/// (cheap and harmless: apt's lists only).
pub fn sync_before_install(pm: PackageManager) -> bool {
    pm == PackageManager::AptGet
}

/// argv asking, read-only and without root, whether the repositories carry
/// `package`, and whether the answer is "yes" from the output. `None` when
/// the manager has no quick, reliable query (the package tables are
/// trusted then). Run it with `LC_ALL=C`.
pub fn available_query(pm: PackageManager, package: &str) -> Option<(Vec<String>, fn(&crate::host::Output) -> bool)> {
    fn by_exit(o: &crate::host::Output) -> bool {
        o.success
    }
    fn by_listing(o: &crate::host::Output) -> bool {
        o.success && !o.stdout.trim().is_empty()
    }
    let (argv, judge): (&[&str], fn(&crate::host::Output) -> bool) = match pm {
        PackageManager::Pacman => (&["pacman", "-Si"], by_exit),
        PackageManager::AptGet => (&["apt-cache", "show"], by_exit),
        PackageManager::Xbps => (&["xbps-query", "-R"], by_exit),
        PackageManager::Apk => (&["apk", "search", "-x"], by_listing),
        PackageManager::Dnf | PackageManager::Zypper | PackageManager::Emerge | PackageManager::Eopkg => return None,
    };
    Some((argv.iter().map(|a| a.to_string()).chain([package.to_string()]).collect(), judge))
}

/// `abs` inside `root`.
fn sys(root: &Path, abs: &str) -> PathBuf {
    root.join(abs.trim_start_matches('/'))
}

/// pacman's database folder and its repositories, in `pacman.conf` order,
/// following `Include =` files (globs in the file name included).
fn pacman_repos(root: &Path) -> (String, Vec<String>) {
    let mut db_path = PACMAN_DB_PATH.to_string();
    let mut repos = Vec::new();
    let mut section = String::new();
    read_pacman_conf(root, &sys(root, PACMAN_CONF), 0, &mut section, &mut db_path, &mut repos);
    (db_path, repos)
}

fn read_pacman_conf(root: &Path, file: &Path, depth: usize, section: &mut String, db_path: &mut String, repos: &mut Vec<String>) {
    let Ok(text) = std::fs::read_to_string(file) else { return };
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            *section = name.trim().to_string();
            if *section != PACMAN_OPTIONS && !repos.contains(section) {
                repos.push(section.clone());
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim(), value.trim());
        if key == "DBPath" && section == PACMAN_OPTIONS {
            *db_path = value.to_string();
        } else if key == "Include" && depth < MAX_INCLUDE_DEPTH {
            for included in expand(&sys(root, value)) {
                read_pacman_conf(root, &included, depth + 1, section, db_path, repos);
            }
        }
    }
}

/// A path whose file name may hold `*` wildcards (`/etc/pacman.d/*.conf`),
/// as the files it matches, sorted.
fn expand(path: &Path) -> Vec<PathBuf> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !name.contains('*') {
        return vec![path.to_path_buf()];
    }
    let Some(dir) = path.parent() else { return Vec::new() };
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().filter(|e| wildcard(&name, &e.file_name().to_string_lossy())).map(|e| e.path()).collect();
    out.sort();
    out
}

/// `*`-only glob matching.
fn wildcard(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = name.strip_prefix(first) else { return false };
    let parts: Vec<&str> = parts.collect();
    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            return rest.ends_with(part);
        }
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    /// CachyOS's layout: its own repos first, mirrorlists included, the
    /// databases never synced (issue #45).
    const CACHYOS_CONF: &str = "[options]\nArchitecture = auto\n# DBPath = /nope/\n\n[cachyos]\nInclude = /etc/pacman.d/cachyos-mirrorlist\n\n[core]\nInclude = /etc/pacman.d/mirrorlist\n[extra]\nInclude = /etc/pacman.d/mirrorlist\n#[testing]\n[multilib]\nInclude = /etc/pacman.d/mirrorlist\n";

    #[test]
    fn pacman_reports_every_unsynced_repo() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "etc/pacman.conf", CACHYOS_CONF);
        write(root, "etc/pacman.d/mirrorlist", "Server = https://geo.mirror.pkgbuild.com/$repo/os/$arch\n");
        assert_eq!(missing(PackageManager::Pacman, root), ["cachyos", "core", "extra", "multilib"]);
        // Synced, except an empty (interrupted) download.
        for repo in ["cachyos", "core", "extra"] {
            write(root, &format!("var/lib/pacman/sync/{repo}.db"), "db");
        }
        write(root, "var/lib/pacman/sync/multilib.db", "");
        assert_eq!(missing(PackageManager::Pacman, root), ["multilib"]);
        write(root, "var/lib/pacman/sync/multilib.db", "db");
        assert!(missing(PackageManager::Pacman, root).is_empty());
    }

    #[test]
    fn pacman_follows_db_path_and_included_repo_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "etc/pacman.conf", "[options]\nDBPath = /srv/pacman/\nInclude = /etc/pacman.d/repos/*.conf\n");
        write(root, "etc/pacman.d/repos/10-core.conf", "[core]\nServer = x\n");
        write(root, "etc/pacman.d/repos/20-extra.conf", "[extra]\nServer = x\n");
        write(root, "etc/pacman.d/repos/README", "[not-a-repo]\n");
        assert_eq!(missing(PackageManager::Pacman, root), ["core", "extra"]);
        write(root, "srv/pacman/sync/core.db", "db");
        write(root, "srv/pacman/sync/extra.db", "db");
        assert!(missing(PackageManager::Pacman, root).is_empty());
        // No pacman.conf at all: nothing to check.
        assert!(missing(PackageManager::Pacman, &root.join("elsewhere")).is_empty());
    }

    #[test]
    fn apt_needs_downloaded_package_lists() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // What a fresh container or an emptied cache leaves behind.
        write(root, "var/lib/apt/lists/lock", "");
        std::fs::create_dir_all(root.join("var/lib/apt/lists/partial")).unwrap();
        assert_eq!(missing(PackageManager::AptGet, root), ["package lists"]);
        write(root, "var/lib/apt/lists/deb.debian.org_debian_dists_trixie_main_binary-amd64_Packages.lz4", "x");
        assert!(missing(PackageManager::AptGet, root).is_empty());
    }

    #[test]
    fn gentoo_needs_its_repository_tree() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(missing(PackageManager::Emerge, dir.path()), ["Gentoo repository"]);
        write(dir.path(), "var/db/repos/gentoo/profiles/repo_name", "gentoo\n");
        assert!(missing(PackageManager::Emerge, dir.path()).is_empty());
    }

    #[test]
    fn self_refreshing_managers_are_never_flagged() {
        let dir = tempfile::tempdir().unwrap();
        for pm in [PackageManager::Dnf, PackageManager::Zypper, PackageManager::Xbps, PackageManager::Apk, PackageManager::Eopkg] {
            assert!(missing(pm, dir.path()).is_empty(), "{pm:?}");
        }
    }

    #[test]
    fn sync_commands_per_manager() {
        let argv = |pm, unattended| sync_argv(pm, unattended).map(|a| a.join(" "));
        assert_eq!(argv(PackageManager::Pacman, false).as_deref(), Some("pacman -Syu"));
        assert_eq!(argv(PackageManager::Pacman, true).as_deref(), Some("pacman -Syu --noconfirm"));
        assert_eq!(argv(PackageManager::AptGet, true).as_deref(), Some("apt-get update"));
        assert_eq!(argv(PackageManager::Dnf, true).as_deref(), Some("dnf makecache --refresh"));
        assert_eq!(argv(PackageManager::Zypper, true).as_deref(), Some("zypper --non-interactive refresh"));
        assert_eq!(argv(PackageManager::Apk, true).as_deref(), Some("apk update"));
        assert_eq!(argv(PackageManager::Emerge, true).as_deref(), Some("emerge --sync"));
        assert_eq!(argv(PackageManager::Eopkg, true).as_deref(), Some("eopkg update-repo"));
        assert_eq!(argv(PackageManager::Xbps, true), None);
        for pm in PackageManager::ALL {
            // Never a bare `-Sy`: no partial upgrades on Arch.
            if let Some(a) = sync_argv(pm, true) {
                assert!(!a.iter().any(|w| w == "-Sy"), "{pm:?}: {a:?}");
            }
            assert_eq!(sync_upgrades_system(pm), pm == PackageManager::Pacman);
        }
    }

    #[test]
    fn availability_queries() {
        let q = |pm| available_query(pm, "niri").map(|(a, _)| a.join(" "));
        assert_eq!(q(PackageManager::Pacman).as_deref(), Some("pacman -Si niri"));
        assert_eq!(q(PackageManager::AptGet).as_deref(), Some("apt-cache show niri"));
        assert_eq!(q(PackageManager::Xbps).as_deref(), Some("xbps-query -R niri"));
        assert_eq!(q(PackageManager::Apk).as_deref(), Some("apk search -x niri"));
        assert_eq!(q(PackageManager::Dnf), None);
        // apk lists nothing (and still succeeds) for a package it lacks.
        let (_, judge) = available_query(PackageManager::Apk, "x").unwrap();
        assert!(!judge(&crate::host::Output { success: true, ..Default::default() }));
        assert!(judge(&crate::host::Output { success: true, stdout: "niri-25.11-r2\n".into(), ..Default::default() }));
    }

    #[test]
    fn wildcards() {
        assert!(wildcard("*.conf", "core.conf"));
        assert!(wildcard("10-*.conf", "10-core.conf"));
        assert!(!wildcard("*.conf", "README"));
        assert!(wildcard("a*b*c", "aXXbYYc"));
        assert!(!wildcard("a*b*c", "aXXcYYb"));
    }
}
