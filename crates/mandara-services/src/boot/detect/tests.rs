//! Whole-distribution fixtures (the files each layout really has) and the
//! detection they lead to.

use std::path::Path;

use super::*;

pub(crate) const ARCH_GRUB: &str = "GRUB_DEFAULT=0\nGRUB_TIMEOUT=5\nGRUB_DISTRIBUTOR=\"Arch\"\nGRUB_CMDLINE_LINUX_DEFAULT=\"loglevel=3 quiet\"\nGRUB_TERMINAL_INPUT=console\nGRUB_GFXMODE=auto\nGRUB_GFXPAYLOAD_LINUX=keep\n#GRUB_THEME=\"/path/to/gfxtheme\"\n";
pub(crate) const DEBIAN_GRUB: &str = "GRUB_DEFAULT=0\nGRUB_TIMEOUT=5\nGRUB_DISTRIBUTOR=`lsb_release -i -s 2> /dev/null || echo Debian`\nGRUB_CMDLINE_LINUX_DEFAULT=\"quiet\"\nGRUB_CMDLINE_LINUX=\"\"\n#GRUB_TERMINAL=console\n#GRUB_GFXMODE=640x480\n";
pub(crate) const FEDORA_GRUB: &str = "GRUB_TIMEOUT=5\nGRUB_DISTRIBUTOR=\"$(sed 's, release .*$,,g' /etc/system-release)\"\nGRUB_DEFAULT=saved\nGRUB_DISABLE_SUBMENU=true\nGRUB_TERMINAL_OUTPUT=\"console\"\nGRUB_CMDLINE_LINUX=\"rhgb quiet\"\nGRUB_DISABLE_RECOVERY=\"true\"\nGRUB_ENABLE_BLSCFG=true\n";
pub(crate) const LIMINE_CONF: &str = "timeout: 5\ndefault_entry: 2\n\n/+CachyOS\ncomment: machine-id=0123\n//linux-cachyos\n    protocol: linux\n    kernel_path: boot():/0123/linux-cachyos/vmlinuz-linux-cachyos#abc\n    module_path: boot():/0123/linux-cachyos/initramfs-linux-cachyos#def\n";
const GRUB_CFG: &str = "### BEGIN /etc/grub.d/10_linux ###\nmenuentry 'Linux' { linux /vmlinuz }\n";

pub(crate) fn file(root: &Path, abs: &str, body: &str) {
    let p = root.join(abs.trim_start_matches('/'));
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

pub(crate) fn exe(root: &Path, abs: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt as _;
    file(root, abs, body);
    std::fs::set_permissions(root.join(abs.trim_start_matches('/')), std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn dir(root: &Path, abs: &str) {
    std::fs::create_dir_all(root.join(abs.trim_start_matches('/'))).unwrap();
}

/// mountinfo lines for `(mount point, fs type)`.
fn mounts(root: &Path, list: &[(&str, &str)]) {
    let body: String = list.iter().enumerate().map(|(i, (at, fs))| format!("{} 1 0:{i} / {} rw,relatime shared:1 - {fs} /dev/sda{i} rw\n", 20 + i, at.replace(' ', "\\040"))).collect();
    file(root, "/proc/self/mountinfo", &body);
}

/// An EFI variable with a UTF-16 string payload.
fn efivar(root: &Path, name: &str, value: &str) {
    let mut bytes = vec![7, 0, 0, 0];
    for u in value.encode_utf16().chain([0]) {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    let p = root.join(format!("sys/firmware/efi/efivars/{name}"));
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

fn plymouth(root: &Path, current: &str, initramfs: &str) {
    exe(root, "/usr/bin/plymouth-set-default-theme", "#!/bin/sh\n");
    for t in ["spinner", "bgrt", current] {
        file(root, &format!("/usr/share/plymouth/themes/{t}/{t}.plymouth"), "[Plymouth Theme]\nModuleName=two-step\n");
    }
    file(root, "/usr/share/plymouth/plymouthd.defaults", "[Daemon]\nTheme=spinner\n");
    file(root, "/etc/plymouth/plymouthd.conf", &format!("[Daemon]\nTheme={current}\n"));
    for m in ["two-step.so", "script.so", "text.so"] {
        file(root, &format!("/usr/lib/plymouth/{m}"), "");
    }
    match initramfs {
        "mkinitcpio" => {
            exe(root, "/usr/bin/mkinitcpio", "");
            file(root, "/etc/mkinitcpio.conf", "HOOKS=(base systemd autodetect microcode kms modconf block keyboard sd-vconsole plymouth sd-encrypt filesystems)\n");
        }
        "dracut" => {
            exe(root, "/usr/bin/dracut", "");
            dir(root, "/usr/lib/dracut/modules.d/50plymouth");
        }
        "update-initramfs" => {
            exe(root, "/usr/sbin/update-initramfs", "");
            file(root, "/usr/share/initramfs-tools/hooks/plymouth", "");
        }
        _ => {}
    }
    file(root, "/proc/cmdline", "BOOT_IMAGE=/vmlinuz root=UUID=x rw quiet splash\n");
}

/// Arch with GRUB (BIOS or UEFI with the ESP at /boot/efi).
pub(crate) fn arch_grub(root: &Path, uefi: bool) {
    file(root, "/etc/os-release", "NAME=\"Arch Linux\"\nID=arch\nPRETTY_NAME=\"Arch Linux\"\n");
    file(root, "/etc/default/grub", ARCH_GRUB);
    exe(root, "/usr/bin/grub-mkconfig", "#!/bin/sh\n. \"${sysconfdir}/default/grub\"\n");
    exe(root, "/usr/bin/grub-script-check", "");
    exe(root, "/usr/bin/grub-mkfont", "");
    file(root, "/boot/grub/grub.cfg", GRUB_CFG);
    plymouth(root, "bgrt", "mkinitcpio");
    if uefi {
        dir(root, "/sys/firmware/efi");
        mounts(root, &[("/", "ext4"), ("/boot/efi", "vfat")]);
        file(root, "/fixture/efibootmgr.txt", "BootCurrent: 0001\nTimeout: 1 seconds\nBootOrder: 0001,0000\nBoot0000* UEFI OS\tHD(1,GPT,x)/File(\\EFI\\BOOT\\BOOTX64.EFI)\nBoot0001* GRUB\tHD(1,GPT,x)/File(\\EFI\\GRUB\\grubx64.efi)\n");
        file(root, "/boot/efi/EFI/GRUB/grubx64.efi", "");
    } else {
        mounts(root, &[("/", "ext4")]);
    }
}

/// Debian / Ubuntu: `grub-mkconfig` reads `/etc/default/grub.d`.
pub(crate) fn debian_grub(root: &Path, uefi: bool) {
    file(root, "/etc/os-release", "PRETTY_NAME=\"Debian GNU/Linux 13 (trixie)\"\nID=debian\n");
    file(root, "/etc/default/grub", DEBIAN_GRUB);
    file(root, "/etc/default/grub.d/init-select.cfg", "# init-select\n");
    exe(root, "/usr/sbin/grub-mkconfig", "#!/bin/sh\nfor x in ${sysconfdir}/default/grub.d/*.cfg ; do\n  if [ -e \"${x}\" ]; then . \"${x}\"; fi\ndone\n");
    exe(root, "/usr/bin/grub-script-check", "");
    exe(root, "/usr/sbin/update-grub", "#!/bin/sh\nexec grub-mkconfig -o /boot/grub/grub.cfg \"$@\"\n");
    file(root, "/boot/grub/grub.cfg", GRUB_CFG);
    plymouth(root, "spinner", "update-initramfs");
    if uefi {
        dir(root, "/sys/firmware/efi");
        mounts(root, &[("/", "ext4"), ("/boot/efi", "vfat")]);
        file(root, "/boot/efi/EFI/debian/grub.cfg", "search.fs_uuid 1234 root\nset prefix=($root)'/boot/grub'\nconfigfile $prefix/grub.cfg\n");
        file(root, "/fixture/efibootmgr.txt", "BootCurrent: 0002\nBoot0002* debian\tHD(1,GPT,x)/File(\\EFI\\debian\\shimx64.efi)\n");
    } else {
        mounts(root, &[("/", "ext4")]);
    }
}

/// Fedora: grub2 naming, a stub on the ESP, BLS entries, a console terminal.
pub(crate) fn fedora_grub(root: &Path) {
    file(root, "/etc/os-release", "NAME=\"Fedora Linux\"\nID=fedora\nPRETTY_NAME=\"Fedora Linux 43\"\n");
    file(root, "/etc/default/grub", FEDORA_GRUB);
    exe(root, "/usr/sbin/grub2-mkconfig", "#!/bin/sh\n");
    exe(root, "/usr/bin/grub2-script-check", "");
    file(root, "/boot/grub2/grub.cfg", "insmod blscfg\nblscfg\n");
    dir(root, "/sys/firmware/efi");
    mounts(root, &[("/", "btrfs"), ("/boot", "ext4"), ("/boot/efi", "vfat")]);
    file(root, "/boot/efi/EFI/fedora/grub.cfg", "search --no-floppy --root-dev-only --fs-uuid --set=dev 1234\nset prefix=($dev)/grub2\nexport $prefix\nconfigfile $prefix/grub.cfg\n");
    file(root, "/fixture/efibootmgr.txt", "BootCurrent: 0000\nBoot0000* Fedora\tHD(1,GPT,x)/File(\\EFI\\fedora\\shimx64.efi)\n");
    plymouth(root, "bgrt", "dracut");
}

/// openSUSE Tumbleweed: grub2 naming, no grub.d.
pub(crate) fn opensuse_grub(root: &Path) {
    file(root, "/etc/os-release", "NAME=\"openSUSE Tumbleweed\"\nID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n");
    file(root, "/etc/default/grub", "GRUB_DISTRIBUTOR=\nGRUB_DEFAULT=saved\nGRUB_TIMEOUT=8\nGRUB_TERMINAL=\"gfxterm\"\nGRUB_GFXMODE=\"auto\"\nGRUB_THEME=/boot/grub2/themes/openSUSE/theme.txt\n");
    exe(root, "/usr/sbin/grub2-mkconfig", "#!/bin/sh\n");
    exe(root, "/usr/bin/grub2-script-check", "");
    file(root, "/boot/grub2/grub.cfg", GRUB_CFG);
    mounts(root, &[("/", "btrfs")]);
    plymouth(root, "bgrt", "dracut");
}

/// CachyOS with Limine managed by limine-entry-tool (the ESP at /boot).
pub(crate) fn cachyos_limine(root: &Path, enroll: bool) {
    file(root, "/etc/os-release", "NAME=\"CachyOS Linux\"\nID=cachyos\nID_LIKE=arch\n");
    file(root, "/etc/default/limine", &format!("ESP_PATH=\"/boot\"\nKERNEL_CMDLINE[default]+=\"quiet splash rw\"\n{}", if enroll { "ENABLE_ENROLL_LIMINE_CONFIG=yes\n" } else { "" }));
    file(root, "/etc/limine-entry-tool.conf", "ENABLE_VERIFICATION=yes\n# a random config may not switch enrolment on\nENABLE_ENROLL_LIMINE_CONFIG=yes\n");
    for t in ["limine", "limine-entry-tool", "limine-mkinitcpio", "limine-enroll-config"] {
        exe(root, &format!("/usr/bin/{t}"), "");
    }
    dir(root, "/sys/firmware/efi");
    mounts(root, &[("/", "btrfs"), ("/boot", "vfat")]);
    efivar(root, &format!("LoaderInfo-{BLI_GUID}"), "Limine 12.9.6");
    file(root, "/boot/limine.conf", LIMINE_CONF);
    file(root, "/boot/EFI/limine/limine_x64.efi", "");
    file(root, "/fixture/efibootmgr.txt", "BootCurrent: 0000\nBoot0000* Limine\tHD(1,GPT,x)/File(\\EFI\\limine\\limine_x64.efi)\n");
    plymouth(root, "wolf", "mkinitcpio");
}

pub(crate) fn refind(root: &Path) {
    file(root, "/etc/os-release", "ID=arch\n");
    dir(root, "/sys/firmware/efi");
    mounts(root, &[("/", "ext4"), ("/boot", "vfat")]);
    file(root, "/boot/EFI/refind/refind.conf", "timeout 20\nuse_nvram false\n");
    file(root, "/fixture/efibootmgr.txt", "BootCurrent: 0003\nBoot0003* rEFInd Boot Manager\tHD(1,GPT,x)/File(\\EFI\\refind\\refind_x64.efi)\n");
}

pub(crate) fn detect_in(f: impl Fn(&Path)) -> (tempfile::TempDir, Detection) {
    let dir = tempfile::tempdir().unwrap();
    f(dir.path());
    let det = detect(&Sys::under(dir.path()));
    (dir, det)
}

#[test]
fn arch_grub_bios_and_uefi() {
    let (_d, det) = detect_in(|r| arch_grub(r, false));
    assert_eq!((det.firmware, det.in_use), (Firmware::Bios, Some(Loader::Grub)));
    assert_eq!(det.evidence, "the only bootloader configured here");
    let g = det.grub.as_ref().unwrap();
    assert_eq!(g.dir, Path::new("/boot/grub"));
    assert_eq!(g.mkconfig, Path::new("/usr/bin/grub-mkconfig"));
    assert!(g.dropin_dir.is_none(), "Arch's grub-mkconfig doesn't read grub.d");
    assert_eq!(g.current.gfxmode.as_deref(), Some("auto"));
    assert!(g.readable && !g.current.menu_hidden);
    let p = det.plymouth.as_ref().unwrap();
    assert_eq!(p.initramfs, Some(Initramfs::Mkinitcpio("/usr/bin/mkinitcpio".into())));
    assert_eq!((p.in_initramfs, p.splash_on_cmdline, p.current.as_deref()), (Some(true), true, Some("bgrt")));

    let (_d, det) = detect_in(|r| arch_grub(r, true));
    assert_eq!((det.firmware, det.in_use), (Firmware::Uefi, Some(Loader::Grub)));
    assert!(det.evidence.starts_with("BootCurrent: GRUB"), "{}", det.evidence);
}

#[test]
fn debian_reads_grub_d_and_its_stub_is_fine() {
    let (_d, det) = detect_in(|r| debian_grub(r, true));
    let g = det.grub.as_ref().unwrap();
    assert_eq!(g.dropin_dir.as_deref(), Some(Path::new("/etc/default/grub.d")));
    assert!(g.efi_full_config.is_none(), "the ESP grub.cfg is a stub");
    assert_eq!(det.in_use, Some(Loader::Grub), "shim means GRUB");
    assert_eq!(det.plymouth.unwrap().initramfs, Some(Initramfs::UpdateInitramfs("/usr/sbin/update-initramfs".into())));
}

#[test]
fn fedora_and_opensuse_use_grub2_folders() {
    let (_d, det) = detect_in(fedora_grub);
    let g = det.grub.as_ref().unwrap();
    assert_eq!((g.dir.as_path(), g.mkconfig.as_path()), (Path::new("/boot/grub2"), Path::new("/usr/sbin/grub2-mkconfig")));
    assert_eq!(g.script_check.as_deref(), Some(Path::new("/usr/bin/grub2-script-check")));
    assert_eq!(g.current.terminal_output.as_deref(), Some("console"));
    assert!(g.efi_full_config.is_none());
    assert_eq!(det.plymouth.unwrap().initramfs, Some(Initramfs::Dracut("/usr/bin/dracut".into())));

    let (_d, det) = detect_in(opensuse_grub);
    let g = det.grub.as_ref().unwrap();
    assert_eq!(g.dir, Path::new("/boot/grub2"));
    assert_eq!(g.current.theme.as_deref(), Some("/boot/grub2/themes/openSUSE/theme.txt"));

    // The pre-2021 Fedora UEFI layout: the full menu on the ESP.
    let (_d, det) = detect_in(|r| {
        fedora_grub(r);
        file(r, "/boot/efi/EFI/fedora/grub.cfg", &"menuentry 'Fedora' { linux /vmlinuz }\n".repeat(200));
    });
    assert_eq!(det.grub.unwrap().efi_full_config.as_deref(), Some(Path::new("/boot/efi/EFI/fedora/grub.cfg")));
}

#[test]
fn cachyos_limine_with_its_entry_tool() {
    let (_d, det) = detect_in(|r| cachyos_limine(r, false));
    assert_eq!(det.in_use, Some(Loader::Limine));
    assert_eq!(det.evidence, "LoaderInfo: Limine 12.9.6");
    let l = det.limine.as_ref().unwrap();
    assert_eq!((l.esp.as_path(), l.config.as_path()), (Path::new("/boot"), Path::new("/boot/limine.conf")));
    assert!(l.entry_tool && l.verification && l.config_seen && l.shadowed_by.is_none());
    assert!(!l.enroll, "only /etc/default/limine may switch enrolment on");
    let p = det.plymouth.as_ref().unwrap();
    assert_eq!(p.initramfs, Some(Initramfs::LimineMkinitcpio("/usr/bin/limine-mkinitcpio".into())), "never plain mkinitcpio: the Limine entries carry hashes");
    assert_eq!(p.initramfs.as_ref().unwrap().argv(), ["/usr/bin/limine-mkinitcpio"]);
    assert!(p.installed.contains(&"wolf".to_string()));

    let (_d, det) = detect_in(|r| cachyos_limine(r, true));
    assert!(det.limine.unwrap().enroll);

    // A config in the EFI app's own folder shadows /boot/limine.conf.
    let (_d, det) = detect_in(|r| {
        cachyos_limine(r, false);
        file(r, "/boot/EFI/limine/limine.conf", "timeout: 1\n");
    });
    assert_eq!(det.limine.unwrap().shadowed_by.as_deref(), Some(Path::new("/boot/EFI/limine/limine.conf")));
}

#[test]
fn refind_systemd_boot_and_ambiguous_machines() {
    let (_d, det) = detect_in(refind);
    assert_eq!(det.in_use, Some(Loader::Refind));
    assert_eq!(det.refind.unwrap().conf, Path::new("/boot/EFI/refind/refind.conf"));

    let (_d, det) = detect_in(|r| {
        file(r, "/etc/os-release", "ID=arch\n");
        dir(r, "/sys/firmware/efi");
        mounts(r, &[("/", "ext4"), ("/efi", "vfat")]);
        file(r, "/efi/loader/loader.conf", "timeout 3\n");
        efivar(r, &format!("LoaderInfo-{BLI_GUID}"), "systemd-boot 257.1");
    });
    assert_eq!((det.in_use, det.systemd_boot), (Some(Loader::SystemdBoot), true));
    assert_eq!(det.menu_target(), Some(Loader::SystemdBoot));

    // GRUB and Limine both configured on a BIOS machine: unknown.
    let (_d, det) = detect_in(|r| {
        arch_grub(r, false);
        exe(r, "/usr/bin/limine", "");
        mounts(r, &[("/", "ext4"), ("/boot", "vfat")]);
        file(r, "/boot/limine.conf", "timeout: 3\n");
    });
    assert_eq!(det.in_use, None);
    assert!(det.evidence.contains("GRUB, Limine"), "{}", det.evidence);
    assert_eq!(det.menu_target(), None);
}

#[test]
fn parsers() {
    assert_eq!(boot_current("BootCurrent: 0001\nBoot0001* Linux Boot Manager\tHD(1,GPT)/File(\\EFI\\systemd\\systemd-bootx64.efi)\n"), Some(("Linux Boot Manager".into(), Some("\\EFI\\systemd\\systemd-bootx64.efi".into()))));
    assert_eq!(boot_current("BootCurrent: 0001\nBoot0001* Windows Boot Manager\n"), Some(("Windows Boot Manager".into(), None)));
    assert_eq!(boot_current("Timeout: 1\n"), None);
    assert_eq!(Loader::classify("\\EFI\\opensuse\\shim.efi"), Some(Loader::Grub));
    assert_eq!(Loader::classify("Windows Boot Manager"), None);
    assert_eq!(unescape_mount("/mnt/my\\040disk"), "/mnt/my disk");
    assert_eq!(ini_get("[Daemon]\nTheme=bgrt\n[Other]\nTheme=x\n", "daemon", "theme").as_deref(), Some("bgrt"));
    let c = limine_candidates(Path::new("/boot"), Some("\\EFI\\limine\\limine_x64.efi"));
    assert_eq!(c.first().map(PathBuf::as_path), Some(Path::new("/boot/EFI/limine/limine.conf")));
    assert_eq!(c.last().map(PathBuf::as_path), Some(Path::new("/boot/limine.conf")));
}

#[test]
fn native_screen_prefers_a_connected_built_in_panel() {
    let (_d, det) = detect_in(|r| {
        arch_grub(r, false);
        file(r, "/sys/class/drm/card1-DP-1/status", "connected\n");
        file(r, "/sys/class/drm/card1-DP-1/modes", "3840x2160\n1920x1080\n");
        file(r, "/sys/class/drm/card1-HDMI-A-1/status", "disconnected\n");
        file(r, "/sys/class/drm/card1-HDMI-A-1/modes", "");
    });
    assert_eq!(det.screen, Some((3840, 2160)));
    let (_d, det) = detect_in(|r| {
        arch_grub(r, false);
        file(r, "/sys/class/drm/card0-DP-1/status", "connected\n");
        file(r, "/sys/class/drm/card0-DP-1/modes", "2560x1440\n");
        file(r, "/sys/class/drm/card0-eDP-1/status", "connected\n");
        file(r, "/sys/class/drm/card0-eDP-1/modes", "2880x1800i\n");
    });
    assert_eq!(det.screen, Some((2880, 1800)));
}
