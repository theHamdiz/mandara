//! Safe reads of FreeBSD `sysctl(3)` values by name or MIB: fixed-size
//! integers, strings and variable-length binary tables (process lists).
//! Nothing here parses text; [`super`] decodes what it returns.

use std::ffi::CStr;
use std::mem::{MaybeUninit, size_of};

/// Attempts at a variable-length read: the table can grow between the size
/// query and the read (`ENOMEM`), so it is re-sized and read again.
const GROW_ATTEMPTS: usize = 4;
/// Slack added to a variable-length table's reported size, out of 8.
const GROW_EIGHTHS: usize = 1;
/// Longest MIB this module resolves (`dev.cpu.15.temperature` is 4).
const MAX_MIB: usize = 8;

/// One fixed-size value by name (`hw.physmem`, `kern.cp_time`…). `None`
/// when the node doesn't exist or its size isn't `T`'s.
pub fn value<T: Copy>(name: &CStr) -> Option<T> {
    let mut out = MaybeUninit::<T>::uninit();
    let mut len = size_of::<T>();
    // SAFETY: `out` is writable for `len` bytes; the kernel writes at most
    // `len` bytes and reports how many it wrote.
    let rc = unsafe { libc::sysctlbyname(name.as_ptr(), out.as_mut_ptr().cast(), &mut len, std::ptr::null(), 0) };
    // SAFETY: the kernel filled all `size_of::<T>()` bytes (checked), and
    // every `T` used here is plain old data.
    (rc == 0 && len == size_of::<T>()).then(|| unsafe { out.assume_init() })
}

/// An integer of either width: some nodes are `int`, some `long`, some
/// `u_int`, depending on the release (`vm.stats.vm.*` are `u_int`).
pub fn number(name: &CStr) -> Option<u64> {
    let mut buf = [0u8; size_of::<u64>()];
    let mut len = buf.len();
    // SAFETY: `buf` is writable for `len` bytes.
    let rc = unsafe { libc::sysctlbyname(name.as_ptr(), buf.as_mut_ptr().cast(), &mut len, std::ptr::null(), 0) };
    if rc != 0 {
        return None;
    }
    match len {
        4 => Some(u64::from(u32::from_ne_bytes(buf[..4].try_into().ok()?))),
        8 => Some(u64::from_ne_bytes(buf)),
        _ => None,
    }
}

/// A signed `int` node (`hw.acpi.battery.time` is -1 when unknown).
pub fn int(name: &CStr) -> Option<i32> {
    value::<libc::c_int>(name)
}

/// A string node (`kern.hostname`, `hw.model`), without its NUL.
pub fn string(name: &CStr) -> Option<String> {
    let bytes = bytes_by_name(name)?;
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    Some(String::from_utf8_lossy(&bytes[..end]).trim().to_string())
}

/// A variable-length node by name, as raw bytes (`kern.cp_times`).
pub fn bytes_by_name(name: &CStr) -> Option<Vec<u8>> {
    let mut mib = [0 as libc::c_int; MAX_MIB];
    let mut n = mib.len();
    // SAFETY: `mib` holds `n` ints; the kernel writes at most that many.
    if unsafe { libc::sysctlnametomib(name.as_ptr(), mib.as_mut_ptr(), &mut n) } != 0 {
        return None;
    }
    bytes(&mib[..n])
}

/// A variable-length node by MIB (`kern.proc.proc`, `kern.proc.args.<pid>`).
/// Reads into one buffer sized from the kernel's estimate, retrying while
/// the table grows.
pub fn bytes(mib: &[libc::c_int]) -> Option<Vec<u8>> {
    let mut buf: Vec<u8> = Vec::new();
    for _ in 0..GROW_ATTEMPTS {
        let mut len = 0usize;
        // SAFETY: a size query (null output buffer).
        if unsafe { libc::sysctl(mib.as_ptr(), mib.len() as libc::c_uint, std::ptr::null_mut(), &mut len, std::ptr::null(), 0) } != 0 {
            return None;
        }
        let want = len + len * GROW_EIGHTHS / 8;
        buf.clear();
        buf.reserve_exact(want);
        let mut got = want;
        // SAFETY: `buf` has `want` writable bytes; the kernel writes at most
        // `got` bytes and sets `got` to the count written.
        let rc = unsafe { libc::sysctl(mib.as_ptr(), mib.len() as libc::c_uint, buf.as_mut_ptr().cast(), &mut got, std::ptr::null(), 0) };
        if rc == 0 {
            // SAFETY: the kernel initialised the first `got <= want` bytes.
            unsafe { buf.set_len(got.min(want)) };
            return Some(buf);
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::ENOMEM) {
            return None;
        }
    }
    None
}
