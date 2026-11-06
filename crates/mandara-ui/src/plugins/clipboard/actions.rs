//! Row events carry one small target and the panel's existing state handle.

use gpui::App;
use mandara_services::clipboard::{ClipEntry, ClipboardCommand};

use super::rows::{RowAction, RowActions};
use super::{Shared, edit_image, send, shell, update};

#[derive(Copy, Clone)]
struct Target {
    id: u64,
    pinned: bool,
    click_pastes: bool,
}

impl Target {
    /// Button commands preserve the row's drawn pin state and alternate paste action.
    fn command(self, action: RowAction) -> Option<ClipboardCommand> {
        match action {
            RowAction::Restore(alternate) => Some(ClipboardCommand::Restore {
                id: self.id,
                paste: self.click_pastes != alternate,
            }),
            RowAction::Pin => Some(ClipboardCommand::Pin {
                id: self.id,
                pinned: !self.pinned,
            }),
            RowAction::Delete => Some(ClipboardCommand::Delete(self.id)),
            RowAction::Edit | RowAction::Pick => None,
        }
    }

    fn picker(self, current: Option<u64>) -> Option<u64> {
        if current == Some(self.id) {
            None
        } else {
            Some(self.id)
        }
    }
}

/// Cloning handlers shares the UI state; it does not allocate callback objects.
#[derive(Clone)]
pub(super) struct Actions {
    target: Target,
    ui: Shared,
}

impl Actions {
    pub fn new(ui: &Shared, entry: &ClipEntry, click_pastes: bool) -> Self {
        Self {
            target: Target {
                id: entry.id,
                pinned: entry.pinned,
                click_pastes,
            },
            ui: ui.clone(),
        }
    }
}

impl RowActions for Actions {
    fn run(&self, action: RowAction, cx: &mut App) {
        match action {
            RowAction::Edit => {
                shell(cx).update(cx, |shell, cx| shell.set_panel(None, cx));
                if let Err(error) = edit_image(self.target.id, cx) {
                    tracing::warn!("clipboard edit: {error}");
                }
            }
            RowAction::Pick => update(&self.ui, cx, |ui| ui.picker = self.target.picker(ui.picker)),
            action => {
                if matches!(action, RowAction::Restore(_)) {
                    shell(cx).update(cx, |shell, cx| shell.set_panel(None, cx));
                }
                if let Some(command) = self.target.command(action) {
                    send(cx, command);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mouse and keyboard restoration share the same target and inverse-action rules.
    #[test]
    fn row_commands_preserve_ids_pin_state_and_inverse_paste() {
        for pinned in [false, true] {
            for click_pastes in [false, true] {
                let target = Target {
                    id: 42,
                    pinned,
                    click_pastes,
                };
                for alternate in [false, true] {
                    assert!(
                        matches!(target.command(RowAction::Restore(alternate)), Some(ClipboardCommand::Restore { id: 42, paste }) if paste == (click_pastes != alternate))
                    );
                }
                assert!(
                    matches!(target.command(RowAction::Pin), Some(ClipboardCommand::Pin { id: 42, pinned: next }) if next != pinned)
                );
                assert!(matches!(
                    target.command(RowAction::Delete),
                    Some(ClipboardCommand::Delete(42))
                ));
                assert!(target.command(RowAction::Edit).is_none());
                assert!(target.command(RowAction::Pick).is_none());
                assert_eq!(target.picker(None), Some(42));
                assert_eq!(target.picker(Some(5)), Some(42));
                assert_eq!(target.picker(Some(42)), None);
            }
        }
    }
}
