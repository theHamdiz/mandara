//! Reattach file-event watches when an updater replaces a generated directory.

use rustix::fs::inotify::{self, ReadFlags, WatchFlags};
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub(super) struct Parent {
    current: Option<(PathBuf, i32)>,
}
impl Parent {
    pub fn sync(&mut self, events: impl AsFd, desired: Option<&Path>) -> rustix::io::Result<()> {
        let path = desired.and_then(|path| path.ancestors().find(|path| path.is_dir()));
        if self.current.as_ref().map(|(path, _)| path.as_path()) == path {
            return Ok(());
        }
        if let Some((_, descriptor)) = self.current.take() {
            match inotify::remove_watch(&events, descriptor) {
                Ok(()) | Err(rustix::io::Errno::INVAL) => {}
                Err(error) => return Err(error),
            }
        }
        if let Some(path) = path {
            let flags = WatchFlags::CLOSE_WRITE
                | WatchFlags::MOVED_TO
                | WatchFlags::CREATE
                | WatchFlags::DELETE
                | WatchFlags::DELETE_SELF
                | WatchFlags::MOVE_SELF;
            self.current = Some((path.into(), inotify::add_watch(&events, path, flags)?));
        }
        Ok(())
    }
    pub fn observe(
        &mut self,
        events: impl AsFd,
        descriptor: i32,
        flags: ReadFlags,
    ) -> rustix::io::Result<bool> {
        if !flags.intersects(ReadFlags::DELETE_SELF | ReadFlags::MOVE_SELF | ReadFlags::IGNORED)
            || !self
                .current
                .as_ref()
                .is_some_and(|(_, current)| *current == descriptor)
        {
            return Ok(false);
        }
        self.current = None;
        // A moved directory remains alive; release its watch before attaching to
        // the replacement so repeated updates cannot accumulate descriptors.
        match inotify::remove_watch(events, descriptor) {
            Ok(()) | Err(rustix::io::Errno::INVAL) => Ok(true),
            Err(error) => Err(error),
        }
    }
    pub fn recovering(&self, desired: &Path) -> bool {
        self.current
            .as_ref()
            .is_none_or(|(path, _)| path != desired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_removed_parent_is_watched_from_its_ancestor_until_recreated() {
        let root = tempfile::tempdir().unwrap();
        let generated = root.path().join("generated");
        std::fs::create_dir(&generated).unwrap();
        let events =
            inotify::init(inotify::CreateFlags::NONBLOCK | inotify::CreateFlags::CLOEXEC).unwrap();
        let mut parent = Parent::default();
        parent.sync(&events, Some(&generated)).unwrap();
        assert!(!parent.recovering(&generated));
        std::fs::remove_dir(&generated).unwrap();
        parent.sync(&events, Some(&generated)).unwrap();
        assert!(parent.recovering(&generated));
        std::fs::create_dir(&generated).unwrap();
        parent.sync(&events, Some(&generated)).unwrap();
        assert!(!parent.recovering(&generated));
        parent
            .observe(
                &events,
                parent.current.as_ref().unwrap().1,
                ReadFlags::IGNORED,
            )
            .unwrap();
        parent.sync(&events, Some(&generated)).unwrap();
        assert!(!parent.recovering(&generated));
        parent.sync(&events, None).unwrap();
        assert!(parent.current.is_none());
    }

    #[test]
    fn replacing_a_parent_reattaches_to_the_new_inode_and_releases_the_old_watch() {
        let root = tempfile::tempdir().unwrap();
        let generated = root.path().join("generated");
        std::fs::create_dir(&generated).unwrap();
        let events =
            inotify::init(inotify::CreateFlags::NONBLOCK | inotify::CreateFlags::CLOEXEC).unwrap();
        let mut parent = Parent::default();
        parent.sync(&events, Some(&generated)).unwrap();
        let old = parent.current.as_ref().unwrap().1;
        std::fs::rename(&generated, root.path().join("old")).unwrap();
        std::fs::create_dir(&generated).unwrap();
        let mut bytes = [std::mem::MaybeUninit::uninit(); 4096];
        let mut reader = inotify::Reader::new(&events, &mut bytes);
        let event = reader.next().unwrap();
        assert!(event.events().contains(ReadFlags::MOVE_SELF));
        assert!(parent.observe(&events, event.wd(), event.events()).unwrap());
        parent.sync(&events, Some(&generated)).unwrap();
        assert_ne!(parent.current.as_ref().unwrap().1, old);
        assert_eq!(
            inotify::remove_watch(&events, old),
            Err(rustix::io::Errno::INVAL)
        );
        std::fs::write(generated.join("colors.json"), "{}").unwrap();
        let mut reader = inotify::Reader::new(&events, &mut bytes);
        let mut changed = false;
        while let Ok(event) = reader.next() {
            changed |= event.wd() == parent.current.as_ref().unwrap().1
                && event.events().contains(ReadFlags::CLOSE_WRITE);
        }
        assert!(
            changed,
            "the replacement directory must deliver palette writes"
        );
    }
}
