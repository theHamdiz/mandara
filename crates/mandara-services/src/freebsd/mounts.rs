//! Mounted filesystems from `getfsstat(2)` (FreeBSD has no
//! `/proc/mounts`), with their size and use from the same `statfs`
//! records, so listing every drive costs one system call.

/// Filesystem types that hold user data on local disks.
const LOCAL_TYPES: [&str; 7] = ["ufs", "zfs", "msdosfs", "ext2fs", "fusefs.ntfs", "exfat", "fusefs.exfat"];
/// Mount points never shown as drives (boot and EFI partitions).
const SKIPPED: [&str; 2] = ["/boot", "/efi"];

/// One mounted filesystem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    /// `f_mntfromname`: `/dev/ada0p2`, `zroot/ROOT/default`.
    pub source: String,
    /// `f_mntonname`.
    pub mount: String,
    /// `f_fstypename`: `ufs`, `zfs`…
    pub fstype: String,
    /// Bytes.
    pub total: u64,
    /// Bytes in use; what unprivileged users can still write is the rest.
    pub used: u64,
}

/// The ZFS pool a dataset belongs to (`zroot/ROOT/default` → `zroot`).
fn pool(source: &str) -> &str {
    source.split('/').next().unwrap_or(source)
}

/// Local data filesystems, one per disk: devices under `/dev`, and one
/// mount per ZFS pool (its datasets share the pool's space; the shortest
/// mount point stands for it, like `/` for `zroot`).
pub fn local(all: Vec<Mount>) -> Vec<Mount> {
    let mut out: Vec<Mount> = Vec::new();
    for m in all {
        if !LOCAL_TYPES.contains(&m.fstype.as_str()) || SKIPPED.iter().any(|p| m.mount.starts_with(p)) {
            continue;
        }
        let zfs = m.fstype == "zfs";
        if !zfs && !m.source.starts_with("/dev/") {
            continue;
        }
        let same = |o: &Mount| if zfs { o.fstype == "zfs" && pool(&o.source) == pool(&m.source) } else { o.source == m.source };
        match out.iter_mut().find(|o| same(o)) {
            Some(existing) if m.mount.len() < existing.mount.len() => *existing = m,
            Some(_) => {}
            None => out.push(m),
        }
    }
    out
}

#[cfg(target_os = "freebsd")]
pub use sys::*;

#[cfg(target_os = "freebsd")]
mod sys {
    use super::*;

    fn text(raw: &[libc::c_char]) -> String {
        let bytes: Vec<u8> = raw.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Every mounted filesystem (`MNT_NOWAIT`: cached figures, never
    /// blocking on a hung network mount).
    pub fn all() -> Vec<Mount> {
        // SAFETY: a null buffer asks only for the count.
        let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
        let Ok(count) = usize::try_from(count) else { return Vec::new() };
        let mut buf: Vec<libc::statfs> = Vec::with_capacity(count);
        let bytes = libc::c_long::try_from(count * std::mem::size_of::<libc::statfs>()).unwrap_or(0);
        // SAFETY: `buf` has room for `count` records, `bytes` long.
        let got = unsafe { libc::getfsstat(buf.as_mut_ptr(), bytes, libc::MNT_NOWAIT) };
        let Ok(got) = usize::try_from(got) else { return Vec::new() };
        // SAFETY: the kernel filled `got <= count` records.
        unsafe { buf.set_len(got.min(count)) };
        buf.iter()
            .map(|s| {
                let block = s.f_bsize.max(1);
                let total = s.f_blocks.saturating_mul(block);
                let avail = u64::try_from(s.f_bavail).unwrap_or(0).saturating_mul(block);
                Mount { source: text(&s.f_mntfromname), mount: text(&s.f_mntonname), fstype: text(&s.f_fstypename), total, used: total.saturating_sub(avail) }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(source: &str, mount: &str, fstype: &str) -> Mount {
        Mount { source: source.into(), mount: mount.into(), fstype: fstype.into(), total: 1, used: 0 }
    }

    #[test]
    #[cfg(target_os = "freebsd")]
    fn reads_live_mounts() {
        let root = local(all()).into_iter().find(|m| m.mount == "/").expect("/ is a local filesystem");
        assert!(root.total > 0 && root.used <= root.total);
    }

    /// `mount -p` on the FreeBSD 14.4 test machine, plus a ZFS layout.
    #[test]
    fn keeps_local_disks_and_one_mount_per_pool() {
        let all = vec![
            m("/dev/gpt/rootfs", "/", "ufs"),
            m("devfs", "/dev", "devfs"),
            m("/dev/gpt/efiesp", "/boot/efi", "msdosfs"),
            m("zroot/usr/home", "/usr/home", "zfs"),
            m("zroot/ROOT/default", "/zroot-root", "zfs"),
            m("zroot", "/zroot", "zfs"),
            m("tank/media", "/tank/media", "zfs"),
            m("tmpfs", "/tmp", "tmpfs"),
            m("/usr/home/u/x", "/mnt/null", "nullfs"),
            m("/dev/da0s1", "/media/usb", "msdosfs"),
        ];
        let mounts: Vec<String> = local(all).into_iter().map(|m| m.mount).collect();
        assert_eq!(mounts, ["/", "/zroot", "/tank/media", "/media/usb"]);
    }
}
