//! A desktop edit keeps its original text until a successful save.

/// A store supplies the current text and applies one accepted edit.
#[cfg(test)]
pub trait TextStore {
    type Error;

    fn current(&self) -> Option<&str>;
    fn write(&mut self, text: &str) -> Result<(), Self::Error>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum SaveError<E> {
    Changed,
    Removed,
    Store(E),
}

/// `None` is a new item; existing items retain their exact opening text.
pub struct Draft {
    original: Option<String>,
    retry: bool,
}

impl Draft {
    /// Existing items keep their exact opening text; None denotes a new item.
    pub fn new(original: Option<String>) -> Self {
        Self { original, retry: false }
    }

    /// The last acknowledged baseline, never a failed or merely queued write.
    pub fn text(&self) -> &str {
        self.original.as_deref().unwrap_or_default()
    }

    /// Checks the shared model before dispatch; the worker checks notes again.
    pub fn prepare(&mut self, text: &str, current: Option<&str>) -> Result<bool, SaveError<String>> {
        if self.text() == text && !self.retry { return Ok(false); }
        if let Some(original) = &self.original {
            match current {
                None if self.retry && text.trim().is_empty() => {}
                None => return Err(SaveError::Removed),
                Some(current) if current != original && current != text => return Err(SaveError::Changed),
                Some(current) if current == text && !self.retry => {
                    self.committed(text);
                    return Ok(false);
                }
                _ => {}
            }
        } else if text.trim().is_empty() { return Ok(false); }
        self.retry = true;
        Ok(true)
    }

    /// Supplies the worker comparison text without copying an unrelated item.
    pub fn original(&self) -> Option<&str> { self.original.as_deref() }
    /// Matching content still needs a write after partial filename/index failure.
    pub fn retrying(&self) -> bool { self.retry }

    /// Only an actual persistence receipt advances the draft's comparison text.
    pub fn committed(&mut self, text: &str) {
        self.original = Some(text.to_owned());
        self.retry = false;
    }

    /// An unchanged field never overwrites a newer external edit. A failed
    /// write keeps the opening text, so the same draft can be retried.
    #[cfg(test)]
    pub fn save<S: TextStore>(&mut self, text: &str, store: &mut S) -> Result<(), SaveError<S::Error>> {
        if self.text() == text {
            return Ok(());
        }
        if let Some(original) = &self.original {
            let current = store.current().ok_or(SaveError::Removed)?;
            if current != original && current != text {
                return Err(SaveError::Changed);
            }
            if current == text {
                self.original = Some(text.to_owned());
                return Ok(());
            }
        } else if text.trim().is_empty() {
            return Ok(());
        }
        store.write(text).map_err(SaveError::Store)?;
        self.original = Some(text.to_owned());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Store {
        text: Option<String>,
        writes: usize,
        fail: bool,
    }

    impl TextStore for Store {
        type Error = &'static str;

        fn current(&self) -> Option<&str> {
            self.text.as_deref()
        }

        fn write(&mut self, text: &str) -> Result<(), Self::Error> {
            self.writes += 1;
            if self.fail {
                return Err("disk unavailable");
            }
            self.text = Some(text.to_owned());
            Ok(())
        }
    }

    fn store(text: Option<&str>) -> Store {
        Store { text: text.map(str::to_owned), writes: 0, fail: false }
    }

    #[test]
    fn editing_and_retrying_preserve_unicode_and_multiline_text() {
        let mut store = store(Some("# Shopping\nmilk"));
        let mut draft = Draft::new(store.text.clone());
        let text = "# أفكار\nfirst line\nsecond line\n";
        store.fail = true;
        assert_eq!(draft.save(text, &mut store), Err(SaveError::Store("disk unavailable")));
        assert_eq!(draft.text(), "# Shopping\nmilk");
        assert_eq!(store.text.as_deref(), Some("# Shopping\nmilk"));
        store.fail = false;
        assert_eq!(draft.save(text, &mut store), Ok(()));
        assert_eq!(store.text.as_deref(), Some(text));
        assert_eq!(draft.text(), text);
        draft.save(text, &mut store).unwrap();
        assert_eq!(store.writes, 2, "saving an already committed draft does not write again");
    }

    #[test]
    fn external_changes_and_removal_never_lose_either_text() {
        let mut draft = Draft::new(Some("opening text".into()));
        let mut store = store(Some("edited in another view"));
        assert_eq!(draft.save("my desktop edit", &mut store), Err(SaveError::Changed));
        assert_eq!(store.text.as_deref(), Some("edited in another view"));
        draft.save("opening text", &mut store).unwrap();
        assert_eq!(store.writes, 0);
        store.text = None;
        assert_eq!(draft.save("my desktop edit", &mut store), Err(SaveError::Removed));
        assert_eq!(draft.text(), "opening text");
    }

    #[test]
    fn new_empty_drafts_do_not_create_items_and_matching_external_saves_are_idempotent() {
        let mut draft = Draft::new(None);
        let mut store = store(None);
        draft.save(" \n\t", &mut store).unwrap();
        assert_eq!(store.writes, 0);
        draft.save("first task", &mut store).unwrap();
        assert_eq!(store.writes, 1);
        store.text = Some("same update".into());
        draft.save("same update", &mut store).unwrap();
        assert_eq!(store.writes, 1);
    }

    #[test]
    fn matching_body_does_not_skip_the_retry_of_failed_metadata() {
        let mut draft = Draft::new(Some("Old title".into()));
        assert_eq!(draft.prepare("New title", Some("Old title")), Ok(true));
        assert!(draft.retrying());
        // The body reached disk but its rename or index did not.
        assert_eq!(draft.prepare("New title", Some("New title")), Ok(true));
        assert_eq!(draft.text(), "Old title");
        draft.committed("New title");
        assert_eq!(draft.prepare("New title", Some("New title")), Ok(false));
    }

    #[test]
    fn a_deleted_body_can_retry_its_index_without_becoming_a_removed_conflict() {
        let mut draft = Draft::new(Some("Delete this".into()));
        assert_eq!(draft.prepare("", Some("Delete this")), Ok(true));
        assert_eq!(draft.prepare("", None), Ok(true));
        draft.committed("");
        assert_eq!(draft.prepare("", None), Ok(false));
    }
}
