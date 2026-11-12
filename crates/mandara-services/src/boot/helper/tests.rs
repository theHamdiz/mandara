//! Apply and restore against whole-distribution fixtures, with the tools
//! faked: what changes, what is backed up, what runs, and that a failure
//! anywhere puts everything back.

use std::cell::RefCell;
use std::path::Path;

use super::*;
use crate::boot::detect::tests::{ARCH_GRUB, DEBIAN_GRUB, LIMINE_CONF, arch_grub, cachyos_limine, debian_grub, file, refind};
use crate::boot::plan::tests::{grub_stage, plymouth_stage};
use crate::boot::plan::{LimineStyle, SplashChoice, Staged, build};

/// Plays the system tools on a fixture root.
struct Fake<'a> {
    sys: &'a Sys,
    calls: RefCell<Vec<Vec<String>>>,
    /// Fail the n-th call (0-based) whose program name ends with this.
    fail: Option<(&'static str, usize)>,
}

impl<'a> Fake<'a> {
    fn new(sys: &'a Sys) -> Self {
        Self { sys, calls: RefCell::new(Vec::new()), fail: None }
    }

    fn names(&self) -> Vec<String> {
        self.calls.borrow().iter().map(|a| Path::new(&a[0]).file_name().unwrap().to_string_lossy().into_owned()).collect()
    }
}

impl Runner for Fake<'_> {
    fn run(&self, argv: &[String]) -> Result<String, String> {
        let name = Path::new(&argv[0]).file_name().unwrap().to_string_lossy().into_owned();
        let nth = self.calls.borrow().iter().filter(|a| a[0].ends_with(&name)).count();
        self.calls.borrow_mut().push(argv.to_vec());
        if let Some((prog, n)) = self.fail
            && name.ends_with(prog)
            && n == nth
        {
            return Err(format!("{name} failed"));
        }
        let sys = self.sys;
        match name.as_str() {
            "grub-mkconfig" | "grub2-mkconfig" => {
                let out = &argv[2];
                let theme = detect::detect(sys).grub.and_then(|g| g.current.theme).unwrap_or_default();
                std::fs::write(sys.path(out), format!("set theme={theme}\nmenuentry 'Linux' {{ linux /vmlinuz }}\n")).unwrap();
                Ok(String::new())
            }
            "plymouth-set-default-theme" => {
                let conf = sys.path("/etc/plymouth/plymouthd.conf");
                match argv.get(1) {
                    Some(t) => {
                        std::fs::write(conf, format!("[Daemon]\nTheme={t}\n")).unwrap();
                        Ok(String::new())
                    }
                    None => Ok(format!("{}\n", detect::ini_get(&std::fs::read_to_string(conf).unwrap_or_default(), "Daemon", "Theme").unwrap_or_default())),
                }
            }
            "grub-mkfont" => {
                std::fs::write(sys.path(&argv[4]), "pf2").unwrap();
                Ok(String::new())
            }
            _ => Ok(String::new()),
        }
    }
}

fn approve(sys: &Sys, req: plan::Request) -> Approved {
    let det = detect::detect(sys);
    let digest = build(&det, &req, None).unwrap().digest();
    Approved { request: req, digest }
}

fn read(sys: &Sys, p: &str) -> String {
    std::fs::read_to_string(sys.path(p)).unwrap_or_default()
}

#[test]
fn grub_on_arch_applies_backs_up_and_restores_byte_for_byte() {
    let root = tempfile::tempdir().unwrap();
    arch_grub(root.path(), true);
    let sys = Sys::under(root.path());
    let stage = tempfile::tempdir().unwrap();
    let theme = grub_stage(stage.path(), "mandara-vimix-1080p");
    std::os::unix::fs::symlink("/etc/shadow", theme.dir.join("shadow.png")).unwrap();
    let approved = approve(&sys, plan::Request::MenuTheme { theme, gfxmode: Some("1920x1080".into()) });
    let before_cfg = read(&sys, "/boot/grub/grub.cfg");
    let fake = Fake::new(&sys);
    let out = apply(&sys, &detect::detect(&sys), &approved, &fake).unwrap();
    assert!(out.ok);
    let installed = sys.path("/boot/grub/themes/mandara-vimix-1080p");
    assert!(installed.join("theme.txt").is_file());
    assert!(std::fs::symlink_metadata(installed.join("shadow.png")).is_err(), "links in a staged theme never get installed");
    let defaults = read(&sys, "/etc/default/grub");
    assert!(defaults.contains("GRUB_THEME=\"/boot/grub/themes/mandara-vimix-1080p/theme.txt\""));
    assert!(defaults.contains("GRUB_GFXMODE=\"1920x1080,auto\""));
    assert!(defaults.contains("GRUB_CMDLINE_LINUX_DEFAULT=\"loglevel=3 quiet\""));
    assert!(read(&sys, "/boot/grub/grub.cfg").contains("set theme=/boot/grub/themes/mandara-vimix-1080p/theme.txt"), "the menu was rebuilt with the theme");
    assert!(!sys.path("/boot/grub/grub.cfg.mandara-new").exists(), "no scratch file is left");
    // Preflight build + check, then the real build + check.
    assert_eq!(fake.names(), ["grub-mkconfig", "grub-script-check", "grub-mkconfig", "grub-script-check"]);
    let record = latest(&sys).unwrap();
    assert_eq!(record.id, out.backup.clone().unwrap());
    assert_eq!(record.dirs[0].path, Path::new("/boot/grub/themes/mandara-vimix-1080p"));
    assert!(Installed::load(&sys).dirs.contains(&record.dirs[0].path));
    // The record is readable by everyone; the copies by root only.
    use std::os::unix::fs::PermissionsExt as _;
    let bdir = sys.path(BACKUP_DIR).join(&record.id);
    assert_eq!(std::fs::metadata(bdir.join("record.json")).unwrap().permissions().mode() & 0o777, 0o644);
    assert_eq!(std::fs::metadata(bdir.join("file-0")).unwrap().permissions().mode() & 0o777, 0o600);

    let fake = Fake::new(&sys);
    let out = restore(&sys, "latest", &fake).unwrap();
    assert!(out.ok, "{}", out.message);
    assert_eq!(read(&sys, "/etc/default/grub"), ARCH_GRUB, "byte for byte");
    assert!(!installed.exists(), "the theme Mandara installed is removed");
    assert!(!read(&sys, "/boot/grub/grub.cfg").contains("mandara"), "the menu is rebuilt without it");
    assert_ne!(read(&sys, "/boot/grub/grub.cfg"), "", "{before_cfg}");
    assert_eq!(fake.names(), ["grub-mkconfig", "grub-script-check"]);
    assert!(latest(&sys).is_none(), "nothing left to restore");
    assert!(restore(&sys, "latest", &fake).is_err());
}

#[test]
fn grub_on_debian_writes_only_its_drop_in() {
    let root = tempfile::tempdir().unwrap();
    debian_grub(root.path(), true);
    let sys = Sys::under(root.path());
    let stage = tempfile::tempdir().unwrap();
    let approved = approve(&sys, plan::Request::MenuTheme { theme: grub_stage(stage.path(), "mandara-image"), gfxmode: None });
    apply(&sys, &detect::detect(&sys), &approved, &Fake::new(&sys)).unwrap();
    assert_eq!(read(&sys, "/etc/default/grub"), DEBIAN_GRUB, "untouched");
    let dropin = read(&sys, "/etc/default/grub.d/99-mandara-boot-look.cfg");
    assert!(dropin.contains("GRUB_THEME=\"/boot/grub/themes/mandara-image/theme.txt\""));
    assert_eq!(read(&sys, "/boot/efi/EFI/debian/grub.cfg").lines().last(), Some("configfile $prefix/grub.cfg"), "the ESP stub is never touched");
    restore(&sys, "latest", &Fake::new(&sys)).unwrap();
    assert!(!sys.path("/etc/default/grub.d/99-mandara-boot-look.cfg").exists());
    assert!(sys.path("/etc/default/grub.d/init-select.cfg").exists());
}

#[test]
fn a_failed_check_puts_everything_back() {
    let root = tempfile::tempdir().unwrap();
    arch_grub(root.path(), false);
    let sys = Sys::under(root.path());
    let stage = tempfile::tempdir().unwrap();
    let approved = approve(&sys, plan::Request::MenuTheme { theme: grub_stage(stage.path(), "mandara-x"), gfxmode: None });
    let cfg = read(&sys, "/boot/grub/grub.cfg");
    // The check of the real build (the second one) fails.
    let fake = Fake { fail: Some(("grub-script-check", 1)), ..Fake::new(&sys) };
    let (code, out) = apply(&sys, &detect::detect(&sys), &approved, &fake).unwrap_err();
    assert_eq!(code, EXIT_FAILED);
    assert!(out.message.contains("put back"), "{}", out.message);
    assert_eq!(read(&sys, "/etc/default/grub"), ARCH_GRUB);
    assert_eq!(read(&sys, "/boot/grub/grub.cfg"), cfg);
    assert!(!sys.path("/boot/grub/themes/mandara-x").exists());
    assert!(!sys.path("/boot/grub/grub.cfg.mandara-new").exists());
    assert!(records(&sys).iter().all(|r| r.state == State::RolledBack));
    assert!(Installed::load(&sys).dirs.is_empty());
    // A system whose menu can't be rebuilt at all: the preflight stops it
    // before anything changes.
    let fake = Fake { fail: Some(("grub-mkconfig", 0)), ..Fake::new(&sys) };
    assert!(apply(&sys, &detect::detect(&sys), &approved, &fake).is_err());
    assert_eq!(fake.names(), ["grub-mkconfig"]);
    assert_eq!(read(&sys, "/etc/default/grub"), ARCH_GRUB);
}

#[test]
fn a_different_plan_or_a_foreign_folder_changes_nothing() {
    let root = tempfile::tempdir().unwrap();
    arch_grub(root.path(), false);
    let sys = Sys::under(root.path());
    let stage = tempfile::tempdir().unwrap();
    let mut approved = approve(&sys, plan::Request::MenuTheme { theme: grub_stage(stage.path(), "mandara-x"), gfxmode: None });
    approved.digest = "0".repeat(64);
    let fake = Fake::new(&sys);
    let (code, out) = apply(&sys, &detect::detect(&sys), &approved, &fake).unwrap_err();
    assert_eq!(code, EXIT_PLAN_CHANGED);
    assert!(out.plan.is_some(), "the new plan comes back to review");
    assert!(fake.calls.borrow().is_empty() && records(&sys).is_empty());
    // A theme folder of the same name that Mandara didn't install.
    std::fs::create_dir_all(sys.path("/boot/grub/themes/mandara-x")).unwrap();
    let approved = approve(&sys, plan::Request::MenuTheme { theme: grub_stage(stage.path(), "mandara-x"), gfxmode: None });
    let (_, out) = apply(&sys, &detect::detect(&sys), &approved, &Fake::new(&sys)).unwrap_err();
    assert!(out.message.contains("wasn't installed by Mandara"), "{}", out.message);
    assert_eq!(read(&sys, "/etc/default/grub"), ARCH_GRUB);
}

#[test]
fn a_staged_theme_is_checked_again_on_the_root_side() {
    let root = tempfile::tempdir().unwrap();
    arch_grub(root.path(), false);
    let sys = Sys::under(root.path());
    let stage = tempfile::tempdir().unwrap();
    let theme = grub_stage(stage.path(), "mandara-x");
    let approved = approve(&sys, plan::Request::MenuTheme { theme: theme.clone(), gfxmode: None });
    // Swapped after you approved it: it now points outside itself.
    std::fs::write(theme.dir.join("theme.txt"), "desktop-image: \"../../../etc/shadow\"\n").unwrap();
    assert!(apply(&sys, &detect::detect(&sys), &approved, &Fake::new(&sys)).is_err());
    assert_eq!(read(&sys, "/etc/default/grub"), ARCH_GRUB);
}

#[test]
fn limine_look_applies_around_the_entries_and_restores_your_later_edits() {
    let root = tempfile::tempdir().unwrap();
    cachyos_limine(root.path(), true);
    let sys = Sys::under(root.path());
    let stage = tempfile::tempdir().unwrap();
    let wall = stage.path().join("wallpaper.png");
    image::RgbaImage::new(4, 4).save(&wall).unwrap();
    let style = LimineStyle { wallpaper: Some(wall), backdrop: Some("0b1026".into()), branding_colour: Some("7cd5c4".into()), ..Default::default() };
    let approved = approve(&sys, plan::Request::LimineStyle { style });
    let fake = Fake::new(&sys);
    apply(&sys, &detect::detect(&sys), &approved, &fake).unwrap();
    let conf = read(&sys, "/boot/limine.conf");
    assert!(conf.contains("wallpaper: boot():/mandara-boot-look/wallpaper.png") && conf.contains("backdrop: 0b1026"));
    assert!(conf.ends_with(LIMINE_CONF.split_once("\n\n").unwrap().1), "the entries are untouched");
    assert!(sys.path("/boot/mandara-boot-look/wallpaper.png").is_file());
    assert_eq!(fake.names(), ["limine-enroll-config"], "the config's hash is enrolled again");
    // You edit the config afterwards; Restore keeps your edit.
    let edited = conf.replacen("timeout: 5", "timeout: 9", 1);
    std::fs::write(sys.path("/boot/limine.conf"), &edited).unwrap();
    let fake = Fake::new(&sys);
    let out = restore(&sys, "latest", &fake).unwrap();
    assert!(out.message.contains("only Mandara's lines"), "{}", out.message);
    assert_eq!(read(&sys, "/boot/limine.conf"), LIMINE_CONF.replacen("timeout: 5", "timeout: 9", 1));
    assert!(!sys.path("/boot/mandara-boot-look/wallpaper.png").exists());
    assert_eq!(fake.names(), ["limine-enroll-config"]);
}

#[test]
fn splash_applies_with_the_right_initramfs_tool_and_rolls_back_a_failed_rebuild() {
    let root = tempfile::tempdir().unwrap();
    cachyos_limine(root.path(), false);
    let sys = Sys::under(root.path());
    let stage = tempfile::tempdir().unwrap();
    let approved = approve(&sys, plan::Request::Splash { theme: SplashChoice::Staged(plymouth_stage(stage.path(), "mandara-splash")) });
    let fake = Fake::new(&sys);
    apply(&sys, &detect::detect(&sys), &approved, &fake).unwrap();
    assert!(sys.path("/usr/share/plymouth/themes/mandara-splash/mandara-splash.plymouth").is_file());
    assert_eq!(detect::ini_get(&read(&sys, "/etc/plymouth/plymouthd.conf"), "Daemon", "Theme").as_deref(), Some("mandara-splash"));
    assert_eq!(fake.names(), ["plymouth-set-default-theme", "plymouth-set-default-theme", "plymouth-set-default-theme", "limine-mkinitcpio"]);
    let fake = Fake::new(&sys);
    restore(&sys, "latest", &fake).unwrap();
    assert_eq!(detect::ini_get(&read(&sys, "/etc/plymouth/plymouthd.conf"), "Daemon", "Theme").as_deref(), Some("wolf"));
    assert!(!sys.path("/usr/share/plymouth/themes/mandara-splash").exists());
    assert!(fake.names().contains(&"limine-mkinitcpio".to_string()), "the initramfs is rebuilt with the old splash");

    // The rebuild fails: the old theme comes back and the initramfs is
    // rebuilt again from it.
    let approved = approve(&sys, plan::Request::Splash { theme: SplashChoice::Staged(plymouth_stage(stage.path(), "mandara-splash")) });
    let fake = Fake { fail: Some(("limine-mkinitcpio", 0)), ..Fake::new(&sys) };
    assert!(apply(&sys, &detect::detect(&sys), &approved, &fake).is_err());
    assert_eq!(detect::ini_get(&read(&sys, "/etc/plymouth/plymouthd.conf"), "Daemon", "Theme").as_deref(), Some("wolf"));
    assert_eq!(fake.names().iter().filter(|n| *n == "limine-mkinitcpio").count(), 2);
    assert!(!sys.path("/usr/share/plymouth/themes/mandara-splash").exists());
}

#[test]
fn refind_include_applies_and_restores() {
    let root = tempfile::tempdir().unwrap();
    refind(root.path());
    let sys = Sys::under(root.path());
    let stage = tempfile::tempdir().unwrap();
    let d = stage.path().join("rEFInd-minimal");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("theme.conf"), "banner themes/rEFInd-minimal/background.png\n").unwrap();
    std::fs::write(d.join("background.png"), "p").unwrap();
    let approved = approve(&sys, plan::Request::MenuTheme { theme: Staged { name: "rEFInd-minimal".into(), dir: d, include: Some("theme.conf".into()) }, gfxmode: None });
    let before = read(&sys, "/boot/EFI/refind/refind.conf");
    apply(&sys, &detect::detect(&sys), &approved, &Fake::new(&sys)).unwrap();
    assert!(read(&sys, "/boot/EFI/refind/refind.conf").contains("include themes/rEFInd-minimal/theme.conf"));
    assert!(sys.path("/boot/EFI/refind/themes/rEFInd-minimal/background.png").is_file());
    restore(&sys, "latest", &Fake::new(&sys)).unwrap();
    assert_eq!(read(&sys, "/boot/EFI/refind/refind.conf"), before);
}

#[test]
fn applying_again_keeps_the_older_version_for_restore() {
    let root = tempfile::tempdir().unwrap();
    arch_grub(root.path(), false);
    let sys = Sys::under(root.path());
    let stage = tempfile::tempdir().unwrap();
    let theme = grub_stage(stage.path(), "mandara-image");
    apply(&sys, &detect::detect(&sys), &approve(&sys, plan::Request::MenuTheme { theme: theme.clone(), gfxmode: None }), &Fake::new(&sys)).unwrap();
    std::fs::write(theme.dir.join("background.png"), "second").unwrap();
    // Same second: the second backup gets its own id.
    apply(&sys, &detect::detect(&sys), &approve(&sys, plan::Request::MenuTheme { theme, gfxmode: None }), &Fake::new(&sys)).unwrap();
    assert_eq!(read(&sys, "/boot/grub/themes/mandara-image/background.png"), "second");
    assert_eq!(records(&sys).len(), 2);
    restore(&sys, "latest", &Fake::new(&sys)).unwrap();
    assert_eq!(read(&sys, "/boot/grub/themes/mandara-image/background.png"), "p", "the first version is back");
    restore(&sys, "latest", &Fake::new(&sys)).unwrap();
    assert!(!sys.path("/boot/grub/themes/mandara-image").exists());
    assert_eq!(read(&sys, "/etc/default/grub"), ARCH_GRUB);
}

#[test]
fn only_fixed_actions_parse() {
    assert_eq!(Action::parse(&["apply".into(), "/home/u/r.json".into()]), Ok(Action::Apply("/home/u/r.json".into())));
    assert_eq!(Action::parse(&["restore".into(), "latest".into()]), Ok(Action::Restore("latest".into())));
    assert_eq!(Action::parse(&["restore".into(), "20261111-101010-menu".into()]), Ok(Action::Restore("20261111-101010-menu".into())));
    for bad in [vec!["apply", "relative.json"], vec!["restore", "../x"], vec!["run", "id"], vec!["apply"], vec!["apply", "/a", "/b"], vec![]] {
        assert!(Action::parse(&bad.iter().map(|s| s.to_string()).collect::<Vec<_>>()).is_err(), "{bad:?}");
    }
    assert_eq!(stamp(0), "19700101-000000");
    assert_eq!(stamp(1_794_355_200), "20261111-000000");
    assert_eq!(stamp(951_782_400 + 3_661), "20000229-010101");
    let _ = file;
}
