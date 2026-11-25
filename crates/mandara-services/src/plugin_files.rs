//! Read-only, descriptor-relative access to a folder the person approved.

use std::fs::File;
use std::io::{Read as _, Seek as _};
use std::path::{Component, Path, PathBuf};

use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, openat, statat};

/// A page stays small enough to return names and opaque handles in one broker reply.
pub const MAX_PAGE: usize = 60;
/// Large directories are reported as truncated rather than retained without a bound.
pub const MAX_SCAN: usize = 20_000;
/// One read leaves ample space for base64 and metadata in the broker reply.
pub const MAX_READ_BYTES: usize = 64 * 1024;
/// Descriptor traversal is bounded even for a deeply nested approved tree.
const MAX_DEPTH: usize = 32;
const OPEN: OFlags = OFlags::RDONLY.union(OFlags::NOFOLLOW).union(OFlags::CLOEXEC).union(OFlags::NONBLOCK);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { File, Directory }

#[derive(Debug)]
pub struct Entry {
    pub name: String,
    /// Kept by the host behind a handle, never included in plugin JSON.
    pub relative: PathBuf,
    pub kind: Kind,
    pub size: u64,
}

#[derive(Debug)]
pub struct Page {
    pub total: usize,
    pub offset: usize,
    pub truncated: bool,
    pub entries: Vec<Entry>,
}

#[derive(Debug)]
pub struct Read {
    pub bytes: Vec<u8>,
    pub size: u64,
    pub offset: u64,
    pub next_offset: u64,
    pub eof: bool,
}

/// The broker uses the same scope for listing and reading; neither accepts an absolute child path.
pub trait ScopedFiles {
    fn entries(&self, relative: &Path, offset: usize, limit: usize) -> Result<Page, String>;
    fn read(&self, relative: &Path, offset: u64, limit: usize) -> Result<Read, String>;
}

pub struct Folder(File);

impl Folder {
    /// Called on a blocking worker with a root taken from the live grants.
    pub fn open(root: &Path) -> Result<Self, String> {
        if !root.is_absolute() { return Err("the granted folder must be absolute".into()); }
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        rustix::fs::open(root, OPEN | OFlags::DIRECTORY, Mode::empty()).map(|fd| Self(File::from(fd))).map_err(|e| e.to_string())
    }

    fn child(&self, relative: &Path, directory: bool) -> Result<File, String> {
        let mut current = self.0.try_clone().map_err(|e| e.to_string())?;
        let mut components = relative.components().peekable();
        let mut depth = 0;
        while let Some(component) = components.next() {
            let Component::Normal(name) = component else { return Err("the file is outside the granted folder".into()) };
            depth += 1;
            if depth > MAX_DEPTH { return Err("the file is nested too deeply".into()); }
            let flags = if directory || components.peek().is_some() { OPEN | OFlags::DIRECTORY } else { OPEN };
            current = File::from(openat(&current, name, flags, Mode::empty()).map_err(|e| e.to_string())?);
        }
        Ok(current)
    }
}

impl ScopedFiles for Folder {
    fn entries(&self, relative: &Path, offset: usize, limit: usize) -> Result<Page, String> {
        if limit == 0 || limit > MAX_PAGE || offset > MAX_SCAN { return Err("the directory page is out of range".into()); }
        let directory = self.child(relative, true)?;
        let entries = Dir::read_from(&directory).map_err(|e| e.to_string())?;
        let mut found = Vec::new();
        let mut truncated = false;
        for (at, entry) in entries.enumerate() {
            if at >= MAX_SCAN { truncated = true; break; }
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            if name.to_bytes() == b"." || name.to_bytes() == b".." { continue; }
            let Ok(metadata) = statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW) else { continue };
            let kind = match FileType::from_raw_mode(metadata.st_mode) {
                FileType::RegularFile => Kind::File,
                FileType::Directory => Kind::Directory,
                _ => continue,
            };
            let name = std::ffi::OsStr::from_bytes(name.to_bytes());
            found.push(Entry { name: name.to_string_lossy().into_owned(), relative: relative.join(name), kind, size: u64::try_from(metadata.st_size).unwrap_or(0) });
        }
        found.sort_by_cached_key(|entry| (entry.name.to_lowercase(), entry.relative.clone()));
        let total = found.len();
        Ok(Page { total, offset, truncated, entries: found.into_iter().skip(offset).take(limit).collect() })
    }

    fn read(&self, relative: &Path, offset: u64, limit: usize) -> Result<Read, String> {
        if limit == 0 || limit > MAX_READ_BYTES { return Err("the file read is out of range".into()); }
        let mut file = self.child(relative, false)?;
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file() { return Err("only regular files can be read".into()); }
        let size = metadata.len();
        if offset > size { return Err("the offset is beyond the file".into()); }
        file.seek(std::io::SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.take(limit as u64).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        let next_offset = offset.checked_add(bytes.len() as u64).ok_or("the file offset overflowed")?;
        Ok(Read { eof: next_offset >= size || bytes.len() < limit, bytes, size, offset, next_offset })
    }
}

use std::os::unix::ffi::OsStrExt as _;

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn approved_files_are_paged_and_binary_reads_are_bounded() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("nested")).unwrap();
        std::fs::write(root.path().join("nested/data.bin"), [0, 255, 128, 10]).unwrap();
        std::fs::write(root.path().join("z.txt"), b"last").unwrap();
        let folder = Folder::open(root.path()).unwrap();
        let page = folder.entries(Path::new(""), 0, 1).unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.offset, 0);
        assert!(!page.truncated);
        assert_eq!(page.entries[0].name, "nested");
        assert_eq!(page.entries[0].kind, Kind::Directory);
        assert_eq!(folder.entries(Path::new(""), 1, 1).unwrap().entries[0].name, "z.txt");
        let file = &folder.entries(Path::new("nested"), 0, 1).unwrap().entries[0].relative;
        let first = folder.read(file, 0, 2).unwrap();
        assert_eq!(first.bytes, [0, 255]);
        assert_eq!((first.size, first.next_offset, first.eof), (4, 2, false));
        let last = folder.read(file, first.next_offset, MAX_READ_BYTES).unwrap();
        assert_eq!(last.bytes, [128, 10]);
        assert!(last.eof);
        assert!(folder.read(file, u64::MAX, 1).is_err());
        assert!(folder.read(file, 0, MAX_READ_BYTES + 1).is_err());
        assert!(folder.entries(Path::new(""), 0, MAX_PAGE + 1).is_err());
    }

    #[test]
    fn traversal_links_and_non_regular_files_never_leave_the_scope() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), b"private").unwrap();
        symlink(outside.path(), root.path().join("escape")).unwrap();
        symlink(outside.path().join("secret"), root.path().join("file-link")).unwrap();
        rustix::fs::mkfifoat(rustix::fs::CWD, root.path().join("pipe"), Mode::RUSR | Mode::WUSR).unwrap();
        let folder = Folder::open(root.path()).unwrap();
        assert!(folder.entries(Path::new(""), 0, MAX_PAGE).unwrap().entries.is_empty());
        for path in [Path::new("../secret"), Path::new("/etc/passwd"), Path::new("escape/secret"), Path::new("file-link"), Path::new("pipe"), Path::new("")] {
            assert!(folder.read(path, 0, 1).is_err(), "{}", path.display());
        }
        assert!(folder.entries(Path::new("escape"), 0, 1).is_err());
    }

    #[test]
    fn replacing_a_listed_file_with_a_link_does_not_change_what_may_be_read() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("file"), b"approved").unwrap();
        std::fs::write(outside.path().join("secret"), b"private").unwrap();
        let folder = Folder::open(root.path()).unwrap();
        let listed = folder.entries(Path::new(""), 0, 1).unwrap().entries.remove(0);
        std::fs::remove_file(root.path().join("file")).unwrap();
        symlink(outside.path().join("secret"), root.path().join("file")).unwrap();
        assert!(folder.read(&listed.relative, 0, 1).is_err());
    }

    #[test]
    fn an_open_scope_survives_its_directory_being_replaced() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("approved");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("file"), b"approved").unwrap();
        let folder = Folder::open(&root).unwrap();
        std::fs::rename(&root, parent.path().join("old")).unwrap();
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("file"), b"replacement").unwrap();
        assert_eq!(folder.read(Path::new("file"), 0, MAX_READ_BYTES).unwrap().bytes, b"approved");
    }
}
