//! Each native dialect keeps its own check and live-application semantics.

mod private;
mod river;
#[cfg(test)]
mod tests;

use anyhow::{Context as _, Result, ensure};
use mandara_config::{
    compositor::Target,
    doctor::{self, CheckEnv, theme::LiveCheck},
    keybinds::{self, Binding, PreparedEdit},
};
use mandara_i18n::t;
use std::{path::Path, sync::Arc};

use super::{Session, transaction::Backend};

pub(super) struct Native<'a> {
    session: &'a Session,
    edit: &'a PreparedEdit,
    driver: Arc<dyn mandara_compositor::CompositorBackend>,
    river: river::Edit,
}

impl<'a> Native<'a> {
    pub fn new(session: &'a Session, edit: &'a PreparedEdit) -> Result<Self> {
        let driver = mandara_compositor::connect()
            .filter(|driver| driver.target() == session.target)
            .context(t(
                "The compositor connection is unavailable. No files were changed.",
            ))?;
        Ok(Self {
            session,
            edit,
            driver,
            river: Default::default(),
        })
    }

    fn private(&self) -> bool {
        matches!(
            self.session.target,
            Target::Hyprland
                | Target::River
                | Target::Labwc
                | Target::Aqueous
                | Target::Cosmic
                | Target::KWin
        )
    }

    fn check_body(&self, path: &Path, body: &str) -> Result<()> {
        let dialect = if path == self.edit.id.file || path == self.edit.id.target {
            &self.edit.id.file
        } else {
            path
        };
        let rows = keybinds::parse(self.session.target, dialect, body)?;
        match self.session.target {
            Target::Hyprland if path == self.edit.id.file => private::hyprland(
                self.session,
                &self.edit.id.file,
                rows.get(self.edit.id.index)
                    .context(t("The shortcut is no longer available."))?,
            ),
            Target::River => private::river(
                rows.get(self.edit.id.index)
                    .context(t("The shortcut is no longer available."))?,
            ),
            Target::Labwc => private::labwc(body),
            _ => Ok(()),
        }
    }

    fn reload(&self, before: &Binding, after: &Binding) -> Result<()> {
        match self.session.target {
            Target::River => self.river.apply(before, after),
            Target::Labwc => private::labwc_reload(),
            // Both compositors watch their own settings; their drivers need no
            // restart or second write to apply a changed native setting.
            Target::Aqueous | Target::Cosmic => Ok(()),
            _ => self.driver.reload_config(),
        }
    }
}

impl Backend for Native<'_> {
    fn validate(&self, path: &Path, body: &str) -> Result<()> {
        self.check_body(path, body)
    }
    fn check(&self) -> LiveCheck {
        if self.private() {
            (|| {
                let body = keybinds::read_source(&self.edit.id.file)?;
                self.check_body(&self.edit.id.file, &body)
            })()
            .into()
        } else {
            doctor::check_with(
                self.session.target,
                &self.session.root,
                &CheckEnv::default(),
            )
            .into()
        }
    }
    fn apply(&self) -> Result<()> {
        ensure!(
            self.session.target != Target::KWin,
            "{}",
            t("KWin shortcuts must be saved through its shortcut service.")
        );
        self.reload(&self.edit.old_binding, self.edit.binding())
    }
    fn restore(&self) -> Result<()> {
        if self.session.target == Target::River {
            self.river
                .restore(&self.edit.old_binding, self.edit.binding())
        } else {
            self.reload(self.edit.binding(), &self.edit.old_binding)
        }
    }
    fn version(&self) -> Option<(u32, u32)> {
        self.driver.version()
    }
}
