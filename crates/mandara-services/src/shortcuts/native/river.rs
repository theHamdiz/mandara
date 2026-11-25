//! Track exactly which live mapping this transaction owns before rolling back.

use anyhow::{Result, ensure};
use mandara_config::{doctor, keybinds::Binding};
use mandara_i18n::t;
use std::{cell::Cell, process::Command, sync::atomic::AtomicBool};

pub(super) fn words(row: &Binding, unmap: bool) -> Result<Vec<String>> {
    ensure!(
        row.read_only.is_none(),
        "{}",
        t("Computed shortcuts must be edited in their source configuration.")
    );
    let mut words = vec![if unmap { "unmap" } else { "map" }.into()];
    for field in &row.fields {
        if unmap && field.label == "Action" {
            break;
        }
        if unmap && (field.label == "Layout" || field.label == "Flags" && field.value != "-release")
        {
            continue;
        }
        words.push(field.value.clone());
    }
    Ok(words)
}

#[derive(Default)]
pub(super) struct Edit(Cell<bool>);

impl Edit {
    pub fn apply(&self, before: &Binding, after: &Binding) -> Result<()> {
        self.apply_with(before, after, &run)
    }

    pub(super) fn apply_with(
        &self,
        before: &Binding,
        after: &Binding,
        run: &dyn Fn(&[String]) -> Result<()>,
    ) -> Result<()> {
        self.replace(before, after, run)?;
        self.0.set(true);
        Ok(())
    }

    pub fn restore(&self, before: &Binding, after: &Binding) -> Result<()> {
        self.restore_with(before, after, &run)
    }

    pub(super) fn restore_with(
        &self,
        before: &Binding,
        after: &Binding,
        run: &dyn Fn(&[String]) -> Result<()>,
    ) -> Result<()> {
        if self.0.get() {
            self.replace(after, before, run)?;
            self.0.set(false);
        }
        Ok(())
    }

    fn replace(
        &self,
        before: &Binding,
        after: &Binding,
        run: &dyn Fn(&[String]) -> Result<()>,
    ) -> Result<()> {
        let unmap = words(before, true)?;
        let map = words(after, false)?;
        let original = words(before, false)?;
        run(&unmap)?;
        if let Err(error) = run(&map) {
            // A rejected map installs nothing. Restore only the mapping removed
            // above, without unmapping the destination this edit never owned.
            return match run(&original) {
                Ok(()) => Err(error),
                Err(restore) => Err(error.context(format!(
                    "restoring the previous mapping failed: {restore:#}"
                ))),
            };
        }
        Ok(())
    }
}

fn run(words: &[String]) -> Result<()> {
    let mut command = Command::new("riverctl");
    command.args(words);
    crate::app_themes::process::native(command, &AtomicBool::new(false), doctor::CHECK_TIMEOUT)
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::{compositor::Target, keybinds};
    use std::cell::RefCell;

    #[test]
    fn layout_pinned_mapping_uses_only_supported_unmap_flags() {
        let rows = keybinds::parse(
            Target::River,
            std::path::Path::new("init"),
            "riverctl map -release -repeat -layout 2 resize Super Q spawn 'touch /never-run'\n",
        )
        .unwrap();
        assert_eq!(
            words(&rows[0], false).unwrap(),
            [
                "map",
                "-release",
                "-repeat",
                "-layout",
                "2",
                "resize",
                "Super",
                "Q",
                "spawn",
                "touch /never-run"
            ]
        );
        assert_eq!(
            words(&rows[0], true).unwrap(),
            ["unmap", "-release", "resize", "Super", "Q"]
        );
    }

    #[test]
    fn rejected_mapping_restores_only_its_original_and_unapplied_undo_does_nothing() {
        let rows = keybinds::parse(
            Target::River,
            std::path::Path::new("init"),
            "riverctl map normal Super Q close\nriverctl map normal Super W close\n",
        )
        .unwrap();
        let edit = Edit::default();
        // No riverctl exists in this test: a premature rollback must not invoke it.
        edit.restore(&rows[0], &rows[1]).unwrap();
        let calls = RefCell::new(Vec::new());
        let error = edit
            .replace(&rows[0], &rows[1], &|words| {
                calls.borrow_mut().push(words.to_vec());
                ensure!(calls.borrow().len() != 2, "rejected destination");
                Ok(())
            })
            .unwrap_err();
        assert!(error.to_string().contains("rejected destination"));
        let calls = calls.into_inner();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0], words(&rows[0], true).unwrap());
        assert_eq!(calls[2], words(&rows[0], false).unwrap());
        assert!(!edit.0.get());
    }
}
