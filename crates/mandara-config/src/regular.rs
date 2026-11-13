//! Inspect the opened configuration descriptor before reading its contents.

use std::{fs::File, path::Path};

use anyhow::{Result, ensure};
use rustix::fs::{Mode, OFlags};

pub(crate) fn open(path: &Path) -> Result<File> {
    // A path can become a FIFO after discovery. Never wait for its writer.
    let file = File::from(rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    ensure!(
        file.metadata()?.is_file(),
        "configuration is not a regular file"
    );
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Read as _, os::unix::fs::symlink, sync::mpsc, time::Duration};

    #[test]
    fn regular_sources_follow_symlinks_and_special_files_fail_without_waiting() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        let alias = directory.path().join("config");
        std::fs::write(&target, "source").unwrap();
        symlink(&target, &alias).unwrap();
        let mut text = String::new();
        open(&alias).unwrap().read_to_string(&mut text).unwrap();
        assert_eq!(text, "source");
        assert!(open(directory.path()).is_err());

        let fifo = directory.path().join("fifo");
        rustix::fs::mkfifoat(rustix::fs::CWD, &fifo, Mode::RUSR | Mode::WUSR).unwrap();
        let (send, receive) = mpsc::channel();
        let worker = std::thread::spawn(move || send.send(open(&fifo).is_err()).unwrap());
        assert!(receive.recv_timeout(Duration::from_secs(1)).unwrap());
        worker.join().unwrap();
    }
}
