//! Coordinate native read-modify-write operations across atomic replacements.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read as _};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

thread_local! {
    // Nested ownership journals can live beside the file being edited.
    static HELD: RefCell<BTreeMap<PathBuf, Weak<Directory>>> = const { RefCell::new(BTreeMap::new()) };
}

struct Directory {
    path: PathBuf,
    _file: File,
}
impl Drop for Directory {
    fn drop(&mut self) {
        // Closing the last descriptor releases the operating-system lock.
        HELD.with(|held| {
            held.borrow_mut().remove(&self.path);
        });
    }
}

fn directory(path: &Path) -> io::Result<Rc<Directory>> {
    if let Some(existing) = HELD.with(|held| held.borrow().get(path).and_then(Weak::upgrade)) {
        return Ok(existing);
    }
    let file = File::open(path)?;
    file.lock()?;
    let directory = Rc::new(Directory {
        path: path.into(),
        _file: file,
    });
    HELD.with(|held| {
        held.borrow_mut()
            .insert(path.into(), Rc::downgrade(&directory));
    });
    Ok(directory)
}

/// An identity captured from the actual file descriptor, including its contents' revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStamp {
    device: u64,
    inode: u64,
    length: u64,
    seconds: i64,
    nanos: i64,
}
impl FileStamp {
    pub fn of(file: &File) -> io::Result<Self> {
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(io::Error::other("configuration is not a regular file"));
        }
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            seconds: metadata.mtime(),
            nanos: metadata.mtime_nsec(),
        })
    }
    pub fn at(path: &Path) -> io::Result<Option<Self>> {
        match fs::metadata(path) {
            Ok(metadata) if !metadata.is_file() => {
                return Err(io::Error::other("configuration is not a regular file"));
            }
            Ok(_) => {}
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    && fs::symlink_metadata(path)
                        .is_err_and(|error| error.kind() == io::ErrorKind::NotFound) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        }
        match File::open(path) {
            Ok(file) => Self::of(&file).map(Some),
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    && fs::symlink_metadata(path)
                        .is_err_and(|error| error.kind() == io::ErrorKind::NotFound) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
}

/// Hold this on the blocking worker through reading, merging and publishing.
/// Directory locks survive file replacement and are reentrant on this thread.
/// Editors outside Mandara need not cooperate; `check` also detects their edits.
pub struct WriteLock {
    source: PathBuf,
    target: PathBuf,
    _directories: [Option<Rc<Directory>>; 2],
}
impl WriteLock {
    pub fn new(path: &Path) -> io::Result<Self> {
        let slot = super::destination::slot(path)?;
        let target = super::destination::identity(path)?;
        let source_parent = slot
            .parent()
            .ok_or_else(|| io::Error::other("configuration has no parent"))?;
        let target_parent = target
            .parent()
            .ok_or_else(|| io::Error::other("configuration has no parent"))?;
        fs::create_dir_all(source_parent)?;
        fs::create_dir_all(target_parent)?;
        let source_parent = source_parent.canonicalize()?;
        let target_parent = target_parent.canonicalize()?;
        let directories = if source_parent == target_parent {
            [Some(directory(&source_parent)?), None]
        } else {
            let (first, second) = if source_parent < target_parent {
                (&source_parent, &target_parent)
            } else {
                (&target_parent, &source_parent)
            };
            [Some(directory(first)?), Some(directory(second)?)]
        };
        let guard = Self {
            source: path.into(),
            target,
            _directories: directories,
        };
        guard.check_target()?;
        Ok(guard)
    }
    pub fn path(&self) -> &Path {
        &self.source
    }
    pub fn target(&self) -> &Path {
        &self.target
    }
    pub fn check_target(&self) -> io::Result<()> {
        if super::destination::identity(&self.source)? != self.target {
            return Err(conflict());
        }
        Ok(())
    }
    /// Check the expected bytes in chunks rather than allocating another config.
    pub fn check(&self, expected: &[u8]) -> io::Result<Option<FileStamp>> {
        self.check_with(expected, |path| File::open(path))
    }
    fn check_with(&self, expected: &[u8], open: impl FnOnce(&Path) -> io::Result<File>) -> io::Result<Option<FileStamp>> {
        self.check_target()?;
        let initial = FileStamp::at(&self.target)?;
        let mut file = match open(&self.target) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound && expected.is_empty() => {
                return match FileStamp::at(&self.target)? {
                    None => Ok(None),
                    Some(_) => Err(conflict()),
                };
            }
            Err(error) => return Err(error),
        };
        let stamp = FileStamp::of(&file)?;
        if Some(stamp) != initial {
            return Err(conflict());
        }
        // Small fixed comparison storage keeps large user configs off the heap.
        const CHUNK_BYTES: usize = 4096;
        let mut buffer = [0; CHUNK_BYTES];
        let mut offset = 0;
        loop {
            let count = match file.read(&mut buffer) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            if count == 0 {
                break;
            }
            if expected.get(offset..offset + count) != Some(&buffer[..count]) {
                return Err(conflict());
            }
            offset += count;
        }
        if offset != expected.len() || FileStamp::at(&self.target)? != Some(stamp) {
            return Err(conflict());
        }
        Ok(Some(stamp))
    }
    pub fn check_stamp(&self, expected: Option<FileStamp>) -> io::Result<()> {
        self.check_target()?;
        if FileStamp::at(&self.target)? != expected {
            return Err(conflict());
        }
        Ok(())
    }
}
fn conflict() -> io::Error {
    io::Error::other("configuration changed while it was being edited; try again")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_writers_share_a_directory_lock_and_release_it_for_other_threads() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config");
        let outer = WriteLock::new(&path).unwrap();
        let inner = WriteLock::new(&root.path().join("journal")).unwrap();
        assert!(Rc::ptr_eq(
            outer._directories[0].as_ref().unwrap(),
            inner._directories[0].as_ref().unwrap()
        ));
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let lock = WriteLock::new(&path).unwrap();
            tx.send(()).unwrap();
            drop(lock);
            HELD.with(|held| assert!(held.borrow().is_empty()));
        });
        assert!(rx.try_recv().is_err());
        drop(inner);
        drop(outer);
        rx.recv().unwrap();
        thread.join().unwrap();
        HELD.with(|held| assert!(held.borrow().is_empty()));
    }
    #[test]
    fn unchanged_text_on_a_replaced_inode_is_still_a_conflict() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config");
        fs::write(&path, "personal").unwrap();
        let lock = WriteLock::new(&path).unwrap();
        let stamp = lock.check(b"personal").unwrap();
        let replacement = root.path().join("replacement");
        fs::write(&replacement, "personal").unwrap();
        fs::rename(replacement, &path).unwrap();
        assert!(lock.check_stamp(stamp).is_err());
        fs::write(&path, "new preference").unwrap();
        assert!(lock.check(b"personal").is_err());
    }
    #[test]
    fn a_file_appearing_after_a_missing_open_is_never_accepted_as_empty() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config");
        let lock = WriteLock::new(&path).unwrap();
        assert!(lock.check(b"").unwrap().is_none());
        let result = lock.check_with(b"", |target| {
            fs::write(target, "personal edit")?;
            Err(io::ErrorKind::NotFound.into())
        });
        assert!(result.is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "personal edit");
    }

}
