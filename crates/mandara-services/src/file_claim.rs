//! Move one directory entry without overwriting a concurrent replacement.

use std::{ffi::OsStr, fs::File, io};

pub(crate) fn rename(from: &File, name: &OsStr, to: &File, destination: &OsStr) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        rustix::fs::renameat_with(
            from,
            name,
            to,
            destination,
            rustix::fs::RenameFlags::NOREPLACE,
        )?;
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (from, name, to, destination);
        Err(io::ErrorKind::Unsupported.into())
    }
}
