//! Reversible theme-key edits for compositors without generated includes.

use anyhow::{Context as _, Result};
use std::path::Path;

pub trait Validator {
    /// Check the edited theme with its compositor in an isolated process.
    fn validate(&self, body: &str) -> Result<()>;
}

/// The caller records ownership and backs up the original before this write.
pub fn guarded_write(path: &Path, next: &str, validator: &dyn Validator) -> Result<()> {
    // Check a private copy first: a compositor's file watcher must never see a rejected edit.
    validator.validate(next).context("the compositor rejected the theme; previous settings were kept")?;
    crate::store::write_atomic(path,next.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Reject;
    impl Validator for Reject { fn validate(&self,_:&str)->Result<()> { anyhow::bail!("native rejection") } }

    #[test]
    fn rejected_edits_restore_the_original_target_without_replacing_its_symlink() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("real");
        let link = root.path().join("config");
        std::fs::write(&target,"personal").unwrap();
        std::os::unix::fs::symlink(&target,&link).unwrap();
        assert!(guarded_write(&link,"theme",&Reject).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(),"personal");
        assert_eq!(std::fs::read_link(&link).unwrap(),target);
        let absent = root.path().join("absent");
        assert!(guarded_write(&absent,"theme",&Reject).is_err());
        assert!(!absent.exists());
    }
}
