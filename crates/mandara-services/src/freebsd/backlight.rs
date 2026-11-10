//! Display backlights on FreeBSD: backlight(9) devices in
//! `/dev/backlight/`, read and set with their ioctls (what `backlight(8)`
//! does, without a process per change). Levels are percentages. The
//! devices belong to the `video` group, which seatd users are in.

/// `BACKLIGHTMAXLEVELS` (`sys/backlight.h`).
const MAX_LEVELS: usize = 100;
/// `BACKLIGHTMAXNAMELENGTH`.
const NAME_LEN: usize = 64;

/// `struct backlight_props`.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct Props {
    pub brightness: u32,
    pub nlevels: u32,
    pub levels: [u32; MAX_LEVELS],
}

/// `struct backlight_info`.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct Info {
    pub name: [u8; NAME_LEN],
    /// `BACKLIGHT_TYPE_PANEL` (0) or `BACKLIGHT_TYPE_KEYBOARD` (1).
    pub kind: u32,
}

/// `_IOWR(group, num, size)` on FreeBSD (`sys/ioccom.h`).
const fn iowr(group: u8, num: u8, size: usize) -> libc::c_ulong {
    const IOC_INOUT: u64 = 0x4000_0000 | 0x8000_0000;
    const IOCPARM_MASK: u64 = 0x1fff;
    (IOC_INOUT | ((size as u64 & IOCPARM_MASK) << 16) | ((group as u64) << 8) | num as u64) as libc::c_ulong
}

pub const BACKLIGHTGETSTATUS: libc::c_ulong = iowr(b'G', 0, std::mem::size_of::<Props>());
pub const BACKLIGHTUPDATESTATUS: libc::c_ulong = iowr(b'G', 1, std::mem::size_of::<Props>());
pub const BACKLIGHTGETINFO: libc::c_ulong = iowr(b'G', 2, std::mem::size_of::<Info>());
/// A panel (not a keyboard) backlight.
pub const TYPE_PANEL: u32 = 0;

#[cfg(target_os = "freebsd")]
pub use sys::*;

#[cfg(target_os = "freebsd")]
mod sys {
    use super::*;
    use std::os::fd::AsRawFd as _;
    use std::path::{Path, PathBuf};

    pub const ROOT: &str = "/dev/backlight";

    /// One device: its file name in [`ROOT`], kind and current percent.
    pub struct Device {
        pub name: String,
        pub kind: u32,
        pub percent: u32,
    }

    fn path(name: &str) -> Option<PathBuf> {
        (!name.is_empty() && !name.contains('/') && name != "..").then(|| Path::new(ROOT).join(name))
    }

    fn ioctl<T>(file: &std::fs::File, request: libc::c_ulong, arg: &mut T) -> std::io::Result<()> {
        // SAFETY: `arg` is the request's own struct, readable and writable.
        let rc = unsafe { libc::ioctl(file.as_raw_fd(), request, std::ptr::from_mut(arg)) };
        if rc == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
    }

    fn zeroed_props() -> Props {
        Props { brightness: 0, nlevels: 0, levels: [0; MAX_LEVELS] }
    }

    /// Reads one device.
    pub fn read(name: &str) -> Option<Device> {
        let file = std::fs::File::open(path(name)?).ok()?;
        let mut props = zeroed_props();
        ioctl(&file, BACKLIGHTGETSTATUS, &mut props).ok()?;
        let mut info = Info { name: [0; NAME_LEN], kind: TYPE_PANEL };
        let kind = ioctl(&file, BACKLIGHTGETINFO, &mut info).map(|_| info.kind).unwrap_or(TYPE_PANEL);
        Some(Device { name: name.to_string(), kind, percent: props.brightness.min(100) })
    }

    /// Every backlight device (panels and keyboards).
    pub fn devices() -> Vec<Device> {
        let Ok(dir) = std::fs::read_dir(ROOT) else { return Vec::new() };
        let mut out: Vec<Device> = dir.flatten().filter_map(|e| read(&e.file_name().to_string_lossy())).collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Sets a device to `percent` (0–100).
    pub fn set(name: &str, percent: u32) -> std::io::Result<()> {
        let path = path(name).ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let file = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
        let mut props = zeroed_props();
        props.brightness = percent.min(100);
        ioctl(&file, BACKLIGHTUPDATESTATUS, &mut props)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The request numbers `sys/backlight.h` defines on amd64.
    #[test]
    fn ioctl_numbers_match_the_header() {
        assert_eq!(std::mem::size_of::<Props>(), 408);
        assert_eq!(std::mem::size_of::<Info>(), 68);
        assert_eq!(BACKLIGHTGETSTATUS, 0xc198_4700);
        assert_eq!(BACKLIGHTUPDATESTATUS, 0xc198_4701);
        assert_eq!(BACKLIGHTGETINFO, 0xc044_4702);
    }
}
