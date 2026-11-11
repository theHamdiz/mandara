# Boot look

**Boot look** restyles what you see before your desktop: the **boot menu**
(GRUB, Limine or rEFInd) and the **boot splash** (Plymouth). Pick a popular
open-source theme, dress Limine in one of Mandara's themes, or use **your
own SVG or picture** as the splash, a GRUB theme and a Limine wallpaper.

It lives in **Studio → Themes → Boot** (`mandara studio themes/boot`) and in
the `mandara msg boot …` commands. It is an opt-in plugin: until you turn it
on (on that tab, in Studio → Plugins, or with `mandara msg plugins enable
boot-look`), Mandara reads and changes nothing about how your machine boots.

Changing boot files carelessly can leave a machine that doesn't start, so
Boot look is built around one rule: **nothing changes that you didn't see
first, that isn't backed up, that the tool's own checker didn't accept, or
that one click can't undo.**

## Supported bootloaders and what each change touches

Boot look first finds out which bootloader this machine actually started
with (read-only): the Boot Loader Interface's `LoaderInfo` EFI variable
(set by Limine, systemd-boot and GRUB's `bli` module), then the firmware's
current boot entry and the file it started (`efibootmgr`), then — on BIOS
machines — the one bootloader configured here. When it can't tell (two
bootloaders configured and the firmware doesn't say), it changes no boot
menu at all.

| Bootloader | What Boot look changes | Files |
|---|---|---|
| **GRUB** | A theme folder, `GRUB_THEME`, the menu resolution (`GRUB_GFXMODE`, when the theme is made for one) and, only if GRUB draws on the text console, the graphical terminal (`GRUB_TERMINAL_OUTPUT` gets `gfxterm`, a serial console stays alongside, and a `GRUB_TERMINAL` that sets both is split so your input stays as it was) so the theme can show. Then the menu is rebuilt. | `/boot/grub/themes/<theme>/` (`/boot/grub2/…` on Fedora and openSUSE); `/etc/default/grub` — or, where `grub-mkconfig` reads drop-ins (Debian, Ubuntu, Mint), only Mandara's own `/etc/default/grub.d/99-mandara-boot-look.cfg`; `grub.cfg` rebuilt |
| **Limine** | Look options only: wallpaper and its style, backdrop, the branding line's colour (and text, if you set one), the help line's colour, the terminal's sixteen colours, foreground and background. Your boot entries are never touched. | A marked block before the first entry in Limine's config (`$ESP_PATH/limine.conf` on CachyOS); the wallpaper at `<ESP>/mandara-boot-look/wallpaper.png` |
| **rEFInd** | A theme folder and one `include themes/<theme>/<file>.conf` line (other theme includes are retired). | `<ESP>/EFI/refind/themes/<theme>/`, `refind.conf` |
| **systemd-boot** | Nothing: it has no themes or colours. Boot look says so. | — |
| **Plymouth** (splash) | Installs the theme, makes it the default with `plymouth-set-default-theme`, and rebuilds the initramfs the way your distribution does. | `/usr/share/plymouth/themes/<theme>/`, `/etc/plymouth/plymouthd.conf`, the initramfs |

Distribution specifics are handled explicitly:

| Layout | GRUB | Initramfs (splash) |
|---|---|---|
| Arch, EndeavourOS, Manjaro | `grub-mkconfig`, `/boot/grub`, edits `/etc/default/grub` | `mkinitcpio -P` |
| CachyOS with Limine | — (Limine: `limine-entry-tool` keeps managing the entries) | `limine-mkinitcpio`, which also re-hashes the Limine entries; plain `mkinitcpio` would leave entries whose checksums no longer match |
| Debian, Ubuntu, Mint, Pop!_OS | `grub-mkconfig` (what `update-grub` runs), `/boot/grub`, a drop-in in `/etc/default/grub.d` | `update-initramfs -u -k all` |
| Fedora, RHEL, Alma, Rocky | `grub2-mkconfig`, `/boot/grub2` (BLS entries; the theme still comes from `GRUB_THEME`). The small `grub.cfg` on the ESP that loads `/boot/grub2/grub.cfg` is never touched; the older layout with the whole menu on the ESP is refused | `dracut --force --regenerate-all` |
| openSUSE | `grub2-mkconfig`, `/boot/grub2`, `/etc/default/grub` | `dracut --force --regenerate-all` |

GRUB themes are always installed under `/boot/grub*/themes`, where GRUB can
read them even when the root file system is encrypted. A theme that ships
only a TrueType or OpenType font gets a GRUB font made from that font with
`grub-mkfont`, at the sizes the theme asks for — never from other fonts.

If your Limine config's hash is enrolled in the Limine binary
(`ENABLE_ENROLL_LIMINE_CONFIG=yes` in `/etc/default/limine`), Boot look
enrols the changed config again with `limine-enroll-config`, and refuses
when that tool is missing. A config that Limine would find first (for
example one in the EFI app's own folder) makes Boot look refuse rather than
edit a file Limine doesn't read.

## The safety model

1. **The plan comes first.** Every apply opens a plan listing each file it
   will back up and change and each command it will run, with notes such as
   "GRUB's menu is hidden on this system". `mandara msg boot apply … --dry-run`
   prints the same plan.
2. **Your approval, every time.** Applying asks for an administrator
   password through polkit. Only Mandara's root-owned admin helper runs as
   root (never your own copy of Mandara), and it accepts just two fixed
   actions: apply an approved request, and restore a backup. Programs are
   taken only from root-owned system folders and run with a cleared
   environment — never through a shell.
3. **Detected again with admin rights.** The root side looks at the system
   itself (some files, such as the ESP, only root can read), rebuilds the
   plan and compares it with the one you approved. If anything differs,
   nothing changes and the new plan comes back for you to review.
4. **Checked before and after.** GRUB: before anything changes, the menu is
   rebuilt into a scratch file and checked with `grub-script-check`; a
   system whose menu can't be rebuilt today gets nothing. After the change,
   the new menu is built into a scratch file, checked, and only then moved
   over `grub.cfg`. Plymouth: the theme is read back from
   `plymouth-set-default-theme`. Limine: the edit is verified to leave
   every entry exactly as it was. Themes are checked on your side and again
   by the root side on its own copy: a theme may only name files inside
   itself, and Plymouth themes must point at their own folder.
5. **Backups.** Every file is copied to
   `/var/lib/mandara-boot/backups/<id>/` before it changes. The record of
   what changed is readable by you; the copies only by root.
6. **Rollback.** If any step fails, everything already changed is put back
   (and the initramfs is rebuilt again from the restored settings), and the
   message says so.
7. **Restore.** *Restore before …* on the Boot tab (or `mandara msg boot
   restore`) undoes the last apply: files you haven't touched since come
   back byte for byte; files you edited since keep your edits and only
   Mandara's lines are reverted; the theme folder Mandara installed is
   removed; the menu or initramfs is rebuilt the same checked way.
8. **What it never does.** It never edits the kernel command line (if the
   splash needs `splash` there, it tells you), never replaces GRUB, shim,
   the Limine binary or any signed file, never touches boot entries, and
   only ever replaces theme folders it installed itself.

**Secure Boot** is unaffected: themes are pictures and text, not code, and
no signed file is replaced.

### If something still goes wrong

The menu can still be reached: GRUB and Limine both fall back to a working
menu if a theme file is missing. From a live USB, every backup is listed in
`/var/lib/mandara-boot/backups/<id>/record.json`: each `path` with the name
of its copy (`file-0`, …) in the same folder. Copy them back, then rebuild
the menu (`grub-mkconfig -o /boot/grub/grub.cfg`) or the initramfs.

## Themes from the catalog

Boot look offers popular themes with a licence that allows redistribution,
each pinned to an archive from its author's GitHub repository and checked
against its SHA-256. Nothing is bundled in Mandara: a theme downloads when
you get it, is unpacked with pure-Rust readers that refuse unsafe paths,
keep no links, devices or scripts, and cap sizes and file counts. Install
scripts that come with a theme are **never run**; for themes assembled by
their script (grub2-themes), Boot look copies the same files the script
would, read from it.

| Theme | For | Licence | Author |
|---|---|---|---|
| Vimix, Tela, Stylish, WhiteSur (1080p, 2K, 4K) | GRUB | GPL-3.0 | Vince Liuice ([grub2-themes](https://github.com/vinceliuice/grub2-themes)) |
| Minegrub | GRUB | MIT | [Lxtharia](https://github.com/Lxtharia/minegrub-theme) |
| Distro themes (Arch Linux, Debian, Fedora, Ubuntu, Linux Mint, openSUSE, NixOS, EndeavourOS, Manjaro) | GRUB | GPL-3.0 | [Adison Cavani](https://github.com/AdisonCavani/distro-grub-themes) |
| Fallout | GRUB | MIT | [shvchk](https://github.com/shvchk/fallout-grub-theme) |
| Catppuccin (Mocha, Macchiato, Frappé, Latte) | GRUB | MIT | [Catppuccin](https://github.com/catppuccin/grub) |
| Poly dark | GRUB | MIT | [shvchk](https://github.com/shvchk/poly-dark) |
| Dracula | GRUB | MIT | [Dracula Theme](https://github.com/dracula/grub) |
| rEFInd minimal | rEFInd | MIT | [Evan Purkhiser](https://github.com/evanpurkhiser/rEFInd-minimal) |
| Catppuccin for rEFInd | rEFInd | MIT | [Catppuccin](https://github.com/catppuccin/refind) |
| Plymouth collection: Rings, Angular, Circle flow, Colourful loop, Cubes, Hexagon dots, Lone, Sphere, Spinner, Connect, Liquid, Abstract ring, Cuts | Plymouth | GPL-3.0 | [Aditya Shakya](https://github.com/adi1090x/plymouth-themes) |
| Catppuccin splash | Plymouth | MIT | [Catppuccin](https://github.com/catppuccin/plymouth) |
| Dracula splash | Plymouth | MIT | [Dracula Theme](https://github.com/dracula/plymouth) |

Splash themes already installed on your system (for example from your
distribution's packages) are listed too and can be selected the same way.
Limine has no theme folders: its looks are made from Mandara's own themes
(Corniche, Montaza Twilight, Obsidian, Catppuccin, Rosé Pine and the rest).

Other themes can be **imported** from an archive, a folder or an `https://`
link. A link can't be checked against a pinned checksum, so it needs your
explicit confirmation (`--yes` on the command line); everything else about
an import — the unpacking rules, the theme checks, the plan, the backups —
is the same.

## Use my image

Choose an SVG, PNG, JPEG or WebP picture and Boot look draws it, sharp at
your screen's own resolution, as:

- a **Plymouth splash** (a theme built on Plymouth's own `two-step` plugin,
  so encrypted disks still get a proper password prompt): your picture on
  your background colour, or filling the screen, with a ring or dots
  spinner (or none), at a small, medium or large size, centred or in the
  upper third;
- a **GRUB theme**: your picture above a centred menu with a highlight in
  your accent colour, at the menu resolution you pick;
- a **Limine wallpaper**, used with any of the Limine looks.

The preview on the Boot tab shows the splash as it will look. Pictures are
capped in file size (24 MB) and pixel size (8192 px a side). An SVG may not
run scripts, embed other documents, declare entities or point at anything
outside itself, and the renderer is told to load nothing from disk.

## Commands

```bash
mandara msg boot status                     # the bootloader, firmware, splash, where their files are, and the backups
mandara msg boot list [menu|splash]         # the catalog (licence, pinned URL and SHA-256, downloaded) and installed splashes
mandara msg boot install vimix/2k           # download, verify and prepare a theme (nothing on the system changes)
mandara msg boot import ~/themes/my-grub.tar.gz
mandara msg boot import https://example.org/theme.tar.gz --yes   # unverified: needs --yes
mandara msg boot import ~/Pictures/logo.svg --background 0b1026 --accent 7cd5c4 --spinner ring --scale 0.28 --position center
mandara msg boot apply menu vimix/2k --dry-run       # the plan only
mandara msg boot apply menu vimix/2k                 # asks for approval, backs up, applies
mandara msg boot apply menu mandara-image            # your image as the GRUB theme
mandara msg boot apply splash adi1090x/rings
mandara msg boot apply splash mandara-splash         # your image as the splash
mandara msg boot apply limine montaza-twilight --wallpaper image --branding My_Laptop
mandara msg boot restore --dry-run                   # what the last apply changed
mandara msg boot restore                             # undo it (or: restore <id> from `boot status`)
```

While the plugin is off, these commands answer that it is off and change
nothing.

## FreeBSD

FreeBSD's own loader is out of scope: on FreeBSD the Boot tab and the
commands say so and change nothing.
