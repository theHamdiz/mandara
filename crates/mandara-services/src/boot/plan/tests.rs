//! Plans for each distribution layout: the exact files and commands.

use std::path::{Path, PathBuf};

use super::*;
use crate::boot::detect::tests::{arch_grub, cachyos_limine, debian_grub, detect_in, fedora_grub, opensuse_grub, refind};

/// A staged GRUB theme folder.
pub(crate) fn grub_stage(dir: &Path, name: &str) -> Staged {
    let d = dir.join(name);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("theme.txt"), "desktop-image: \"background.png\"\n+ boot_menu {\n  item_font = \"Unifont Regular 16\"\n}\n").unwrap();
    std::fs::write(d.join("background.png"), "p").unwrap();
    Staged { name: name.into(), dir: d, include: None }
}

pub(crate) fn plymouth_stage(dir: &Path, name: &str) -> Staged {
    let d = dir.join(name);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join(format!("{name}.plymouth")), format!("[Plymouth Theme]\nModuleName=two-step\n[two-step]\nImageDir=/usr/share/plymouth/themes/{name}\n")).unwrap();
    for f in ["entry.png", "bullet.png", "lock.png"] {
        std::fs::write(d.join(f), "p").unwrap();
    }
    Staged { name: name.into(), dir: d, include: None }
}

fn kinds(plan: &Plan) -> Vec<&'static str> {
    plan.steps.iter().map(|s| match s {
        Step::GrubPreflight { .. } => "preflight",
        Step::Backup { .. } => "backup",
        Step::InstallTheme { .. } => "install",
        Step::MakeFont { .. } => "mkfont",
        Step::GrubKeys { .. } => "keys",
        Step::GrubDropIn { .. } => "dropin",
        Step::GrubConfig { .. } => "grub-cfg",
        Step::LimineOptions { .. } => "limine",
        Step::CopyFile { .. } => "copy",
        Step::LimineEnroll { .. } => "enroll",
        Step::RefindInclude { .. } => "refind",
        Step::PlymouthDefault { .. } => "plymouth",
        Step::Initramfs { .. } => "initramfs",
    }).collect()
}

#[test]
fn arch_grub_edits_only_the_theme_keys_then_rebuilds_checked() {
    let (_d, det) = detect_in(|r| arch_grub(r, true));
    let stage = tempfile::tempdir().unwrap();
    let req = Request::MenuTheme { theme: grub_stage(stage.path(), "mandara-vimix-1080p"), gfxmode: Some("1920x1080".into()) };
    let plan = build(&det, &req, None).unwrap();
    assert_eq!(kinds(&plan), ["preflight", "install", "backup", "keys", "backup", "grub-cfg"]);
    assert_eq!(plan.steps[1], Step::InstallTheme { from: stage.path().join("mandara-vimix-1080p"), to: PathBuf::from("/boot/grub/themes/mandara-vimix-1080p"), kind: Kind::Grub });
    let Step::GrubKeys { file, keys } = &plan.steps[3] else { panic!() };
    assert_eq!(file, Path::new("/etc/default/grub"));
    assert_eq!(keys, &[("GRUB_THEME".to_string(), Some("/boot/grub/themes/mandara-vimix-1080p/theme.txt".to_string())), ("GRUB_GFXMODE".into(), Some("1920x1080,auto".into()))]);
    assert_eq!(plan.steps[5], Step::GrubConfig { mkconfig: "/usr/bin/grub-mkconfig".into(), check: Some("/usr/bin/grub-script-check".into()), cfg: "/boot/grub/grub.cfg".into() });
    assert_eq!(plan.digest(), build(&det, &req, None).unwrap().digest(), "the same request plans the same");
    assert!(plan.steps.iter().all(|s| !s.detail().is_empty() && !s.label().is_empty()));
}

#[test]
fn debian_uses_a_drop_in_and_never_edits_etc_default_grub() {
    let (_d, det) = detect_in(|r| debian_grub(r, false));
    let stage = tempfile::tempdir().unwrap();
    let plan = build(&det, &Request::MenuTheme { theme: grub_stage(stage.path(), "mandara-image"), gfxmode: None }, None).unwrap();
    assert_eq!(kinds(&plan), ["preflight", "install", "backup", "dropin", "backup", "grub-cfg"]);
    assert_eq!(plan.steps[3], Step::GrubDropIn { file: "/etc/default/grub.d/99-mandara-boot-look.cfg".into(), keys: vec![("GRUB_THEME".into(), Some("/boot/grub/themes/mandara-image/theme.txt".into()))] });
    assert!(!plan.steps.iter().any(|s| matches!(s, Step::Backup { path } if path == Path::new("/etc/default/grub"))));
    assert_eq!(plan.steps[5], Step::GrubConfig { mkconfig: "/usr/sbin/grub-mkconfig".into(), check: Some("/usr/bin/grub-script-check".into()), cfg: "/boot/grub/grub.cfg".into() });
}

#[test]
fn fedora_and_opensuse_plan_grub2_paths_and_a_graphical_terminal() {
    let stage = tempfile::tempdir().unwrap();
    let theme = grub_stage(stage.path(), "mandara-tela-2k");
    let (_d, det) = detect_in(fedora_grub);
    let plan = build(&det, &Request::MenuTheme { theme: theme.clone(), gfxmode: Some("2560x1440".into()) }, None).unwrap();
    let Step::GrubKeys { keys, .. } = &plan.steps[3] else { panic!("{:?}", plan.steps) };
    assert!(keys.contains(&("GRUB_THEME".into(), Some("/boot/grub2/themes/mandara-tela-2k/theme.txt".into()))));
    assert!(keys.contains(&("GRUB_TERMINAL_OUTPUT".into(), Some("gfxterm".into()))), "Fedora's console terminal can't show a theme");
    assert!(matches!(&plan.steps[5], Step::GrubConfig { mkconfig, cfg, .. } if mkconfig == Path::new("/usr/sbin/grub2-mkconfig") && cfg == Path::new("/boot/grub2/grub.cfg")), "the real config, never the ESP stub");

    let (_d, det) = detect_in(opensuse_grub);
    let plan = build(&det, &Request::MenuTheme { theme, gfxmode: None }, None).unwrap();
    let Step::GrubKeys { keys, .. } = &plan.steps[3] else { panic!() };
    assert_eq!(keys.len(), 1, "gfxterm already: only the theme changes: {keys:?}");
}

#[test]
fn refusals_are_clear_and_change_nothing() {
    let stage = tempfile::tempdir().unwrap();
    let theme = grub_stage(stage.path(), "mandara-x");
    let menu = Request::MenuTheme { theme: theme.clone(), gfxmode: None };
    // An older Fedora with the whole menu on the ESP.
    let (_d, det) = detect_in(|r| {
        fedora_grub(r);
        crate::boot::detect::tests::file(r, "/boot/efi/EFI/fedora/grub.cfg", &"menuentry 'F' { }\n".repeat(400));
    });
    assert!(build(&det, &menu, None).unwrap_err().contains("older layout"));
    // A Limine machine gets no GRUB theme, and Limine options need Limine.
    let (_d, det) = detect_in(|r| cachyos_limine(r, false));
    assert!(build(&det, &menu, None).is_err());
    let (_d, grub) = detect_in(|r| arch_grub(r, false));
    assert!(build(&grub, &Request::LimineStyle { style: LimineStyle { backdrop: Some("000000".into()), ..Default::default() } }, None).is_err());
    // Bad input never reaches a plan.
    assert!(build(&grub, &Request::MenuTheme { theme: Staged { name: "../evil".into(), ..theme.clone() }, gfxmode: None }, None).is_err());
    assert!(build(&grub, &Request::MenuTheme { theme: theme.clone(), gfxmode: Some("1920x1080; rm".into()) }, None).is_err());
    assert!(build(&grub, &Request::MenuTheme { theme: Staged { dir: "relative".into(), ..theme.clone() }, gfxmode: None }, None).is_err());
    // A FreeBSD answer.
    assert!(build(&crate::boot::detect::Detection::unsupported(), &menu, None).unwrap_err().contains("FreeBSD"));
    // systemd-boot has nothing to theme.
    let (_d, det) = detect_in(|r| {
        crate::boot::detect::tests::file(r, "/etc/os-release", "ID=arch\n");
        std::fs::create_dir_all(r.join("sys/firmware/efi")).unwrap();
        crate::boot::detect::tests::file(r, "/proc/self/mountinfo", "20 1 0:1 / /efi rw - vfat /dev/sda1 rw\n");
        crate::boot::detect::tests::file(r, "/efi/loader/loader.conf", "timeout 3\n");
    });
    assert!(build(&det, &menu, None).unwrap_err().contains("systemd-boot"));
}

#[test]
fn limine_plans_options_wallpaper_and_enrolment() {
    let (_d, det) = detect_in(|r| cachyos_limine(r, true));
    let style = LimineStyle { wallpaper: Some("/home/u/.cache/mandara/boot/stage/limine/wallpaper.png".into()), wallpaper_style: Some("stretched".into()), backdrop: Some("0B1026".into()), branding: Some("Mandara\u{7}".into()), branding_colour: Some("7cd5c4".into()), palette: Some(vec!["0b1026".into(); 8]), ..Default::default() };
    let plan = build(&det, &Request::LimineStyle { style }, None).unwrap();
    assert_eq!(kinds(&plan), ["backup", "backup", "copy", "limine", "enroll"]);
    let Step::LimineOptions { file, options } = &plan.steps[3] else { panic!() };
    assert_eq!(file, Path::new("/boot/limine.conf"));
    assert!(options.contains(&("wallpaper".into(), "boot():/mandara-boot-look/wallpaper.png".into())));
    assert!(options.contains(&("backdrop".into(), "0b1026".into())), "lower-cased");
    assert!(options.contains(&("interface_branding".into(), "Mandara".into())), "control characters dropped");
    assert_eq!(plan.steps[2], Step::CopyFile { from: "/home/u/.cache/mandara/boot/stage/limine/wallpaper.png".into(), to: "/boot/mandara-boot-look/wallpaper.png".into() });
    assert!(plan.notes.contains(&Note::LimineEnrolled) && plan.notes.contains(&Note::LimineEntryTool));
    // Bad colours and an empty style are refused.
    assert!(build(&det, &Request::LimineStyle { style: LimineStyle { backdrop: Some("zzzzzz".into()), ..Default::default() } }, None).is_err());
    assert!(build(&det, &Request::LimineStyle { style: LimineStyle::default() }, None).is_err());
    assert!(build(&det, &Request::LimineStyle { style: LimineStyle { palette: Some(vec!["000000".into(); 7]), ..Default::default() } }, None).is_err());
    // A shadowing config: refused.
    let (_d, det) = detect_in(|r| {
        cachyos_limine(r, false);
        crate::boot::detect::tests::file(r, "/boot/EFI/limine/limine.conf", "timeout: 1\n");
    });
    assert!(build(&det, &Request::LimineStyle { style: LimineStyle { backdrop: Some("000000".into()), ..Default::default() } }, None).unwrap_err().contains("before"));
}

#[test]
fn splash_plans_rebuild_the_initramfs_the_distribution_way() {
    let stage = tempfile::tempdir().unwrap();
    let staged = plymouth_stage(stage.path(), "mandara-splash");
    let req = Request::Splash { theme: SplashChoice::Staged(staged) };
    for (layout, argv) in [
        ((|r: &Path| cachyos_limine(r, false)) as fn(&Path), vec!["/usr/bin/limine-mkinitcpio"]),
        ((|r: &Path| arch_grub(r, false)) as fn(&Path), vec!["/usr/bin/mkinitcpio", "-P"]),
        (fedora_grub as fn(&Path), vec!["/usr/bin/dracut", "--force", "--regenerate-all"]),
        ((|r: &Path| debian_grub(r, false)) as fn(&Path), vec!["/usr/sbin/update-initramfs", "-u", "-k", "all"]),
    ] {
        let (_d, det) = detect_in(layout);
        let plan = build(&det, &req, None).unwrap();
        assert_eq!(kinds(&plan), ["install", "backup", "plymouth", "initramfs"]);
        assert_eq!(plan.steps[0], Step::InstallTheme { from: stage.path().join("mandara-splash"), to: "/usr/share/plymouth/themes/mandara-splash".into(), kind: Kind::Plymouth });
        assert_eq!(plan.steps[3], Step::Initramfs { argv: argv.iter().map(|s| s.to_string()).collect() });
    }
    // An installed theme needs no install step; a missing one is refused.
    let (_d, det) = detect_in(|r| arch_grub(r, false));
    let plan = build(&det, &Request::Splash { theme: SplashChoice::Installed { name: "spinner".into() } }, None).unwrap();
    assert_eq!(kinds(&plan), ["backup", "plymouth", "initramfs"]);
    assert!(build(&det, &Request::Splash { theme: SplashChoice::Installed { name: "nope".into() } }, None).is_err());
}

#[test]
fn splash_notes_explain_why_it_may_not_show() {
    let stage = tempfile::tempdir().unwrap();
    let req = Request::Splash { theme: SplashChoice::Staged(plymouth_stage(stage.path(), "mandara-splash")) };
    let (_d, det) = detect_in(|r| {
        arch_grub(r, false);
        crate::boot::detect::tests::file(r, "/etc/mkinitcpio.conf", "HOOKS=(base udev autodetect block filesystems)\n");
        crate::boot::detect::tests::file(r, "/proc/cmdline", "root=/dev/sda2 rw\n");
    });
    let plan = build(&det, &req, None).unwrap();
    assert_eq!(plan.notes, [Note::PlymouthNotInInitramfs, Note::NoSplashOnCmdline]);
    // No initramfs tool at all: refused.
    let (_d, det) = detect_in(|r| {
        arch_grub(r, false);
        std::fs::remove_file(r.join("usr/bin/mkinitcpio")).unwrap();
    });
    assert!(build(&det, &req, None).unwrap_err().contains("initramfs"));
}

#[test]
fn refind_plans_an_include() {
    let (_d, det) = detect_in(refind);
    let stage = tempfile::tempdir().unwrap();
    let d = stage.path().join("rEFInd-minimal");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("theme.conf"), "banner themes/rEFInd-minimal/background.png\n").unwrap();
    std::fs::write(d.join("background.png"), "p").unwrap();
    let plan = build(&det, &Request::MenuTheme { theme: Staged { name: "rEFInd-minimal".into(), dir: d, include: Some("theme.conf".into()) }, gfxmode: None }, None).unwrap();
    assert_eq!(kinds(&plan), ["install", "backup", "refind"]);
    assert_eq!(plan.steps[2], Step::RefindInclude { file: "/boot/EFI/refind/refind.conf".into(), include: "themes/rEFInd-minimal/theme.conf".into() });
}

#[test]
fn requests_round_trip_as_json() {
    let approved = Approved { request: Request::LimineStyle { style: LimineStyle { backdrop: Some("101010".into()), ..Default::default() } }, digest: "ab".into() };
    let text = serde_json::to_string(&approved).unwrap();
    assert_eq!(serde_json::from_str::<Approved>(&text).unwrap(), approved);
    assert!(serde_json::from_str::<Approved>(r#"{"request":{"kind":"run-shell","cmd":"id"},"digest":"x"}"#).is_err(), "only known requests parse");
}

#[test]
fn limine_presets_come_from_mandara_themes() {
    for t in mandara_theme::catalog::builtin() {
        let style = LimineStyle::from_palette(&t.palette(true), Some("/w.png".into()));
        let opts = style.options().unwrap_or_else(|e| panic!("{}: {e}", t.id));
        assert!(opts.iter().any(|(k, v)| k == "term_palette" && v.split(';').count() == 8), "{}", t.id);
        assert!(opts.iter().any(|(k, v)| k == "term_background" && v.starts_with("c0")));
        assert!(LimineStyle::from_palette(&t.palette(false), None).options().unwrap().iter().any(|(k, v)| k == "term_background" && v.starts_with("00")));
    }
}

#[test]
fn a_theme_gets_the_graphical_terminal_and_keeps_serial_output() {
    let stage = tempfile::tempdir().unwrap();
    let theme = grub_stage(stage.path(), "mandara-x");
    let keys = |text: &str| {
        let (_d, det) = detect_in(|r| {
            arch_grub(r, false);
            crate::boot::detect::tests::file(r, "/etc/default/grub", text);
        });
        let plan = build(&det, &Request::MenuTheme { theme: theme.clone(), gfxmode: None }, None).unwrap();
        plan.steps.iter().find_map(|s| match s { Step::GrubKeys { keys, .. } => Some(keys.clone()), _ => None }).unwrap()
    };
    let k = |pairs: &[(&str, Option<&str>)]| pairs.iter().map(|(a, b)| (a.to_string(), b.map(str::to_string))).collect::<Vec<_>>();
    let theme_key = ("GRUB_THEME", Some("/boot/grub/themes/mandara-x/theme.txt"));
    // Arch's cloud image: GRUB_TERMINAL="serial console" with an input set.
    assert_eq!(keys("GRUB_TERMINAL_INPUT=console\nGRUB_TERMINAL=\"serial console\"\n"), k(&[theme_key, ("GRUB_TERMINAL", None), ("GRUB_TERMINAL_OUTPUT", Some("gfxterm serial"))]));
    // A plain console terminal: the input keeps it.
    assert_eq!(keys("GRUB_TERMINAL=console\n"), k(&[theme_key, ("GRUB_TERMINAL", None), ("GRUB_TERMINAL_INPUT", Some("console")), ("GRUB_TERMINAL_OUTPUT", Some("gfxterm"))]));
    // Debian's cloud image already has gfxterm next to serial: untouched.
    assert_eq!(keys("GRUB_TERMINAL_OUTPUT=\"gfxterm serial\"\n"), k(&[theme_key]));
    assert_eq!(keys("GRUB_TERMINAL_OUTPUT=\"console serial\"\n"), k(&[theme_key, ("GRUB_TERMINAL_OUTPUT", Some("gfxterm serial"))]));
}
