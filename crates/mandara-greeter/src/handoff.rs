//! Keep the login VT in graphics mode between Mandara Login and the desktop.
//!
//! greetd's session worker puts the VT back into text mode and connects the
//! session's stdout/stderr to it before the compositor takes DRM master. That
//! is the flash of a TTY (cursor, cleared console, compositor banners) users
//! see after signing in. [`run`] is the first argv of every Wayland session
//! Mandara starts: it switches the VT to graphics mode, blanks it, sends
//! stdio away from the console, then `exec`s the real session command.

use std::io::Write as _;
use std::os::fd::AsRawFd as _;
use std::os::unix::process::CommandExt as _;
use std::process::Command;

/// Linux `KDSETMODE` / `KDGETMODE` (from `<linux/kd.h>`).
const KDGETMODE: libc::c_ulong = 0x4B3B;
const KDSETMODE: libc::c_ulong = 0x4B3A;
/// Graphics mode: the VT does not draw a text console.
const KD_GRAPHICS: libc::c_int = 0x01;

/// Runs as `mandara greeter handoff -- <session…>`: prepares the VT, then
/// replaces this process with the session.
pub fn run(argv: &[String]) -> anyhow::Result<()> {
    let Some((program, args)) = argv.split_first().filter(|(p, _)| !p.is_empty()) else {
        anyhow::bail!("handoff needs a session command after --");
    };
    prepare_vt();
    silence_stdio();
    let err = Command::new(program).args(args).exec();
    Err(anyhow::anyhow!("exec {program}: {err}"))
}

/// Puts `/dev/tty` into graphics mode and blanks it when it is a Linux VT.
/// Best effort: a pty or a missing device is left alone.
pub fn prepare_vt() {
    let Ok(mut tty) = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty") else {
        return;
    };
    let fd = tty.as_raw_fd();
    let mut mode: libc::c_int = 0;
    // SAFETY: KDGETMODE writes one int through the valid pointer; failure
    // means this is not a Linux VT (pty, pipe, …).
    if unsafe { libc::ioctl(fd, KDGETMODE, &mut mode) } != 0 {
        return;
    }
    // SAFETY: KDSETMODE takes the mode by value.
    let _ = unsafe { libc::ioctl(fd, KDSETMODE, KD_GRAPHICS) };
    let _ = tty.write_all(b"\x1b[H\x1b[2J\x1b[3J\x1b[?25l");
    let _ = tty.flush();
}

/// Point stdout/stderr at `/dev/null` so session banners never hit the VT.
fn silence_stdio() {
    let Ok(null) = std::fs::OpenOptions::new().read(true).write(true).open("/dev/null") else {
        return;
    };
    // SAFETY: dup2 onto 1 and 2; null stays open for the lifetime of exec.
    use std::os::fd::IntoRawFd as _;
    let fd = null.into_raw_fd();
    unsafe {
        let _ = libc::dup2(fd, 1);
        let _ = libc::dup2(fd, 2);
        if fd > 2 {
            let _ = libc::close(fd);
        }
    }
}

/// Prefixes a Wayland session argv with this binary's handoff entry so the
/// VT stays dark until the compositor owns the display.
pub fn wrap_wayland_command(session: &[String]) -> Vec<String> {
    let exe = std::env::current_exe().ok().and_then(|p| p.into_os_string().into_string().ok()).unwrap_or_else(|| "mandara".into());
    let mut out = Vec::with_capacity(session.len() + 4);
    out.push(exe);
    out.push("greeter".into());
    out.push("handoff".into());
    out.push("--".into());
    out.extend(session.iter().cloned());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_puts_handoff_before_the_session() {
        let wrapped = wrap_wayland_command(&["niri-session".into()]);
        assert!(wrapped.len() >= 5, "{wrapped:?}");
        assert_eq!(wrapped[wrapped.len() - 4], "greeter");
        assert_eq!(wrapped[wrapped.len() - 3], "handoff");
        assert_eq!(wrapped[wrapped.len() - 2], "--");
        assert_eq!(wrapped[wrapped.len() - 1], "niri-session");
    }

    #[test]
    fn wrap_keeps_session_arguments() {
        let wrapped = wrap_wayland_command(&["Hyprland".into(), "--config".into(), "/tmp/x.conf".into()]);
        assert_eq!(&wrapped[wrapped.len() - 3..], ["Hyprland", "--config", "/tmp/x.conf"]);
    }
}
