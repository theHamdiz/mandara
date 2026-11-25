use super::super::catalog::{Asset, CATALOG, Layout, SoundTheme};
use super::*;
use crate::app_install::archive::tests::tar_gz;

static NO_CANCEL: AtomicBool = AtomicBool::new(false);

/// Bytes that pass the Ogg check.
const OGG: &[u8] = b"OggS\0\x02fake-vorbis";

const fn theme(theme: &'static str, folder: &'static str, layout: Layout, aliases: &'static [(&'static str, &'static str)]) -> SoundTheme {
    SoundTheme {
        id: "test",
        name: "Test",
        author: "Test",
        license: "CC0-1.0",
        homepage: "https://example.com",
        description: "Test",
        origin: "Test",
        theme,
        swatch: 0,
        asset: Asset { url: "https://example.com/t.tar.gz", sha256: "", size: 0, folder },
        layout,
        aliases,
    }
}

static READY: SoundTheme = theme("Fancy", "pkg/Fancy", Layout::Ready, &[]);

fn write(dir: &Path, name: &str, bytes: Vec<u8>) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

/// A release with the theme at `pkg/Fancy`, an installer script, a link
/// inside the theme, one to a sound elsewhere in the archive and one out.
fn ready_archive(dir: &Path) -> PathBuf {
    write(
        dir,
        "fancy.tar.gz",
        tar_gz(
            &[
                ("pkg/Fancy/index.theme", b"[Sound Theme]\nName=Fancy\nDirectories=stereo\n", 0o644),
                ("pkg/Fancy/stereo/bell.oga", OGG, 0o755),
                ("pkg/Fancy/stereo/bell.oga.license", b"SPDX-License-Identifier: CC0-1.0\n", 0o644),
                ("pkg/Fancy/stereo/notes.txt", b"not a sound", 0o644),
                ("pkg/shared/chime.ogg", OGG, 0o644),
                ("pkg/COPYING", b"licence text", 0o644),
                ("pkg/install.sh", b"#!/bin/sh\nrm -rf ~\n", 0o755),
            ],
            &[("pkg/Fancy/stereo/message.oga", "bell.oga"), ("pkg/Fancy/stereo/dialog-warning.ogg", "../../shared/chime.ogg"), ("pkg/Fancy/stereo/escape.oga", "../../../../etc/passwd")],
        ),
    )
}

fn names(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    out.sort();
    out
}

#[test]
fn installs_only_the_theme_and_records_it() {
    let root = tempfile::tempdir().unwrap();
    let env = Env::under(root.path());
    let seen = std::cell::RefCell::new(Vec::new());
    let entry = install_archive(&ready_archive(root.path()), &READY, "abc", &env, &|p| seen.borrow_mut().push(p.phase), &NO_CANCEL).unwrap();
    let dir = env.sounds.join("Fancy");
    assert_eq!(entry.dir, dir);
    assert_eq!(entry.source, Source::Catalog { id: "test".into() });
    assert!(is_sound_theme(&dir));
    assert_eq!(names(&env.sounds), vec!["Fancy"], "no scratch folder left");
    assert_eq!(names(&dir), vec!["COPYING", "index.theme", "stereo"], "credits from above the theme come along; the script doesn't");
    assert_eq!(names(&dir.join("stereo")), vec!["bell.oga", "bell.oga.license", "dialog-warning.ogg", "message.oga"]);
    assert!(dir.join("stereo/message.oga").is_symlink(), "in-theme links stay links");
    assert!(!dir.join("stereo/dialog-warning.ogg").is_symlink(), "a sound elsewhere in the archive is copied in");
    use std::os::unix::fs::PermissionsExt as _;
    assert_eq!(std::fs::metadata(dir.join("stereo/bell.oga")).unwrap().permissions().mode() & 0o777, 0o644, "no exec bits");
    assert_eq!(installed(&env), vec![entry]);
    assert_eq!(seen.borrow().last(), Some(&Phase::Done));
    // Mandara's own copy is replaced on reinstall.
    install_archive(&ready_archive(root.path()), &READY, "def", &env, &|_| {}, &NO_CANCEL).unwrap();
    assert_eq!(installed(&env).len(), 1);
    assert_eq!(installed(&env)[0].sha256, "def");
}

#[test]
fn releases_without_a_ready_folder_are_assembled() {
    let root = tempfile::tempdir().unwrap();
    let env = Env::under(root.path());
    // freedesktop: sounds only, an index written, `make install` aliases.
    static INDEXED: SoundTheme = theme("Indexed", "src", Layout::Index("[Sound Theme]\nName=Indexed\nDirectories=stereo\n"), &[("power-plug", "device-added"), ("screen-capture", "missing")]);
    let a = write(root.path(), "a.tar.gz", tar_gz(&[("src/index.theme.in", b"_Name=Default\n", 0o644), ("src/stereo/device-added.oga", OGG, 0o644)], &[]));
    install_archive(&a, &INDEXED, "x", &env, &|_| {}, &NO_CANCEL).unwrap();
    let dir = env.sounds.join("Indexed");
    assert!(std::fs::read_to_string(dir.join("index.theme")).unwrap().contains("Name=Indexed"));
    assert_eq!(std::fs::read_link(dir.join("stereo/power-plug.oga")).unwrap(), Path::new("device-added.oga"));
    assert!(!dir.join("stereo/screen-capture.oga").exists(), "an alias of a missing sound is skipped");
    assert!(!dir.join("index.theme.in").exists());
    // COSMIC: a template with a placeholder.
    static TEMPLATE: SoundTheme = theme("Tmpl", "x/src", Layout::Template { file: "index.theme.in", replace: &[("@ThemeName@", "Tmpl")] }, &[]);
    let b = write(root.path(), "b.tar.gz", tar_gz(&[("x/src/index.theme.in", b"[Sound Theme]\nName=@ThemeName@\nDirectories=stereo/alert\n", 0o644), ("x/src/stereo/alert/bell.oga", OGG, 0o644)], &[]));
    install_archive(&b, &TEMPLATE, "x", &env, &|_| {}, &NO_CANCEL).unwrap();
    assert!(std::fs::read_to_string(env.sounds.join("Tmpl/index.theme")).unwrap().contains("Name=Tmpl"));
    assert!(env.sounds.join("Tmpl/stereo/alert/bell.oga").is_file());
    // Materia: files renamed into stereo/.
    static RENAMED: SoundTheme = theme("Renamed", "m", Layout::Renamed(&[("raw/alert_01.ogg", "dialog-error"), ("raw/alert_01.ogg", "dialog-warning")]), &[]);
    let c = write(root.path(), "c.tar.gz", tar_gz(&[("m/index.theme", b"[Sound Theme]\nName=Renamed\n", 0o644), ("m/raw/alert_01.ogg", OGG, 0o644), ("m/raw/unused.ogg", OGG, 0o644)], &[]));
    install_archive(&c, &RENAMED, "x", &env, &|_| {}, &NO_CANCEL).unwrap();
    assert_eq!(names(&env.sounds.join("Renamed/stereo")), vec!["dialog-error.ogg", "dialog-warning.ogg"]);
    assert!(!env.sounds.join("Renamed/raw").exists());
}

/// A `.tar.gz` whose one entry is named `name` (raw headers, so names
/// `tar::Builder` refuses can be written).
fn raw_tar_gz(name: &str, bytes: &[u8]) -> Vec<u8> {
    let mut h = tar::Header::new_old();
    h.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
    h.set_size(bytes.len() as u64);
    h.set_mode(0o644);
    h.set_cksum();
    let mut raw = h.as_bytes().to_vec();
    raw.extend_from_slice(bytes);
    raw.resize(raw.len().div_ceil(512) * 512 + 1024, 0);
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&raw).unwrap();
    gz.finish().unwrap()
}

#[test]
fn path_traversal_and_junk_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let env = Env::under(&root.path().join("home"));
    for evil in ["../escape.oga", "pkg/../../escape.oga", "/tmp/escape.oga"] {
        let path = write(root.path(), "evil.tar.gz", raw_tar_gz(evil, OGG));
        assert!(install_archive(&path, &READY, "abc", &env, &|_| {}, &NO_CANCEL).is_err(), "{evil}");
        assert!(import(&path, &env, &|_| {}, &NO_CANCEL).is_err(), "{evil}");
        assert!(!root.path().join("escape.oga").exists() && !root.path().join("home/escape.oga").exists(), "{evil}");
    }
    // Files that aren't sounds don't make a theme.
    let fake = write(root.path(), "fake.tar.gz", tar_gz(&[("pkg/Fancy/index.theme", b"[Sound Theme]\n", 0o644), ("pkg/Fancy/stereo/bell.oga", b"MZ not audio", 0o644)], &[]));
    assert!(install_archive(&fake, &READY, "abc", &env, &|_| {}, &NO_CANCEL).is_err());
    // Not an archive at all.
    let text = write(root.path(), "notes.txt", b"hello".to_vec());
    assert!(import(&text, &env, &|_| {}, &NO_CANCEL).is_err());
    static BAD_NAME: SoundTheme = theme("../Evil", "pkg/Fancy", Layout::Ready, &[]);
    assert!(install_archive(&ready_archive(root.path()), &BAD_NAME, "abc", &env, &|_| {}, &NO_CANCEL).is_err());
    assert_eq!(std::fs::read_dir(&env.sounds).map(|r| r.count()).unwrap_or(0), 0, "no scratch folders survive the failures");
    assert!(installed(&env).is_empty());
}

#[test]
fn unpacking_stops_past_its_cap() {
    let root = tempfile::tempdir().unwrap();
    let big = vec![0u8; 3 << 20];
    let path = write(root.path(), "bomb.tar.gz", tar_gz(&[("a/one.oga", &big, 0o644), ("a/two.oga", &big, 0o644)], &[]));
    let err = unpack_capped(&path, &root.path().join("x"), 2 << 20, &|_| {}, &NO_CANCEL).unwrap_err();
    assert_eq!(err, t("The archive unpacks to more than a sound theme should hold"));
    let cancel = AtomicBool::new(true);
    assert!(unpack_capped(&path, &root.path().join("y"), MAX_UNPACKED, &|_| {}, &cancel).is_err());
}

#[test]
fn the_file_budget_holds() {
    let mut b = Budget::default();
    for _ in 0..MAX_FILES {
        b.take(1).unwrap();
    }
    assert!(b.take(0).is_err());
    let mut b = Budget::default();
    assert!(b.take(MAX_THEME_BYTES + 1).is_err());
}

#[test]
fn remove_deletes_only_what_mandara_installed() {
    let root = tempfile::tempdir().unwrap();
    let env = Env::under(root.path());
    install_archive(&ready_archive(root.path()), &READY, "abc", &env, &|_| {}, &NO_CANCEL).unwrap();
    // A theme of your own.
    std::fs::create_dir_all(env.sounds.join("Mine/stereo")).unwrap();
    std::fs::write(env.sounds.join("Mine/index.theme"), "[Sound Theme]\n").unwrap();
    assert!(remove("Mine", &env).is_err());
    assert!(remove("../Mine", &env).is_err());
    // A tampered manifest pointing elsewhere is refused.
    let mut m = Manifest::load(&env.manifest);
    m.themes.push(Installed { theme: "Mine".into(), source: Source::Catalog { id: "x".into() }, sha256: String::new(), dir: root.path().to_path_buf(), installed_at: 0 });
    m.save(&env.manifest).unwrap();
    assert!(remove("Mine", &env).is_err());
    assert!(env.sounds.join("Mine/index.theme").exists());
    // Never replaced either.
    static MINE: SoundTheme = theme("Mine", "pkg/Fancy", Layout::Ready, &[]);
    let mut m = Manifest::load(&env.manifest);
    m.themes.retain(|i| i.theme != "Mine");
    m.save(&env.manifest).unwrap();
    assert!(install_archive(&ready_archive(root.path()), &MINE, "abc", &env, &|_| {}, &NO_CANCEL).is_err());
    assert_eq!(std::fs::read_to_string(env.sounds.join("Mine/index.theme")).unwrap(), "[Sound Theme]\n");
    // Mandara's own goes, record and folder.
    remove("Fancy", &env).unwrap();
    assert!(!env.sounds.join("Fancy").exists());
    assert!(installed(&env).is_empty());
}

#[test]
fn imports_folders_and_archives() {
    let root = tempfile::tempdir().unwrap();
    let env = Env::under(&root.path().join("home"));
    // A folder with an index, one level down.
    let picked = root.path().join("Downloads/My Sounds");
    std::fs::create_dir_all(picked.join("Calm/stereo")).unwrap();
    std::fs::write(picked.join("Calm/index.theme"), "[Sound Theme]\nName=Calm\n").unwrap();
    std::fs::write(picked.join("Calm/stereo/bell.oga"), OGG).unwrap();
    let e = import(&picked, &env, &|_| {}, &NO_CANCEL).unwrap();
    assert_eq!(e.theme, "Calm");
    assert_eq!(e.source, Source::File { path: picked.clone() });
    // A bare folder of sounds gets an index named after it.
    let bare = root.path().join("Bare Bells");
    std::fs::create_dir_all(bare.join("stereo")).unwrap();
    std::fs::write(bare.join("stereo/bell.wav"), b"RIFF\0\0\0\0WAVEfmt ").unwrap();
    let e = import(&bare, &env, &|_| {}, &NO_CANCEL).unwrap();
    assert_eq!(e.theme, "Bare-Bells");
    assert!(std::fs::read_to_string(env.sounds.join("Bare-Bells/index.theme")).unwrap().contains("Name=Bare-Bells"));
    // A zip.
    let zip_path = root.path().join("Zipped.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    zip.start_file("Zipped/index.theme", opts).unwrap();
    zip.write_all(b"[Sound Theme]\nName=Zipped\n").unwrap();
    zip.start_file("Zipped/stereo/message.ogg", opts).unwrap();
    zip.write_all(OGG).unwrap();
    zip.finish().unwrap();
    let e = import(&zip_path, &env, &|_| {}, &NO_CANCEL).unwrap();
    assert_eq!(e.theme, "Zipped");
    assert!(!e.sha256.is_empty());
    // Something already in the sounds folder isn't imported onto itself.
    assert!(import(&env.sounds.join("Calm"), &env, &|_| {}, &NO_CANCEL).is_err());
    // Nothing there.
    let empty = root.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    assert!(import(&empty, &env, &|_| {}, &NO_CANCEL).is_err());
    assert_eq!(installed(&env).len(), 3);
}

#[test]
fn links_must_be_https() {
    let root = tempfile::tempdir().unwrap();
    for url in ["http://example.com/t.tar.gz", "file:///etc/passwd", "ftp://x/y"] {
        assert_eq!(download_url(url, root.path(), &|_| {}, &NO_CANCEL).unwrap_err(), t("Only https:// links can be installed"), "{url}");
    }
}

#[test]
fn names_are_sanitized() {
    assert!(valid_name("ocean") && valid_name("COSMIC") && valid_name("My Theme 2"));
    assert!(!valid_name("") && !valid_name(".hidden") && !valid_name("a/b") && !valid_name(&"x".repeat(MAX_NAME + 1)));
    assert_eq!(sanitize_name(" My/Theme "), "My-Theme");
    assert_eq!(sanitize_name("..."), "imported-sounds");
    assert_eq!(sanitize_name(&"é".repeat(100)).chars().count(), MAX_NAME);
    assert!(valid_name(&sanitize_name("../../etc")));
}

#[test]
fn scratch_folders_are_cleaned() {
    let root = tempfile::tempdir().unwrap();
    let env = Env::under(root.path());
    std::fs::create_dir_all(env.sounds.join(format!("{SCRATCH_PREFIX}1-2/x"))).unwrap();
    std::fs::create_dir_all(env.sounds.join("Keep/stereo")).unwrap();
    clean_scratch(&env);
    assert_eq!(names(&env.sounds), vec!["Keep"]);
}

/// With `MANDARA_SOUND_ARCHIVES` naming a folder of the real release files
/// (any names; matched by SHA-256), every catalog theme installs, resolves
/// its signature sounds from the installed copy and removes cleanly.
#[test]
fn every_catalog_theme_installs_from_its_release() {
    let Some(dir) = std::env::var_os("MANDARA_SOUND_ARCHIVES") else { return };
    let files: Vec<(String, PathBuf)> = std::fs::read_dir(dir).unwrap().flatten().map(|e| (crate::cursors::install::sha256_file(&e.path()).unwrap(), e.path())).collect();
    let root = tempfile::tempdir().unwrap();
    let env = Env::under(root.path());
    let roots = vec![env.sounds.clone()];
    for t in CATALOG {
        let (_, file) = files.iter().find(|(sha, _)| sha == t.asset.sha256).unwrap_or_else(|| panic!("{}: release file missing", t.id));
        assert_eq!(std::fs::metadata(file).unwrap().len(), t.asset.size, "{}", t.id);
        let e = install_archive(file, t, t.asset.sha256, &env, &|_| {}, &NO_CANCEL).unwrap_or_else(|err| panic!("{}: {err}", t.id));
        let bytes: u64 = walk_bytes(&e.dir);
        let found: Vec<&str> = crate::sounds::SoundEvent::ALL.iter().filter(|ev| crate::sounds::lookup_in(&roots, t.theme, &ev.names()).is_some()).map(|ev| ev.key()).collect();
        eprintln!("{}: {} bytes, {} files; plays {:?}", t.id, bytes, walk_count(&e.dir), found);
        // Every sound it plays decodes.
        for ev in crate::sounds::SoundEvent::ALL {
            if let Some(p) = crate::sounds::lookup_in(&roots, t.theme, &ev.names()) {
                crate::sounds::decode(&p).unwrap_or_else(|err| panic!("{}: {err}", t.id));
            }
        }
        assert!(!found.is_empty(), "{}", t.id);
        remove(t.theme, &env).unwrap();
    }
    assert!(installed(&env).is_empty());
    assert_eq!(std::fs::read_dir(&env.sounds).unwrap().count(), 0);
}

fn walk_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| match e.file_type() {
        Ok(t) if t.is_dir() => walk_bytes(&e.path()),
        Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
        _ => 0,
    }).sum()
}

fn walk_count(dir: &Path) -> usize {
    std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| if e.file_type().is_ok_and(|t| t.is_dir()) { walk_count(&e.path()) } else { 1 }).sum()
}
