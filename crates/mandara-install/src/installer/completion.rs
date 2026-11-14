//! First steps after installation, including the calling terminal's PATH.

use mandara_i18n::t;

use super::{Installer, Level};

impl Installer<'_> {
    pub(super) fn completion(&mut self) {
        self.say(Level::Done, t("Mandara is installed."));
        self.say(
            Level::Info,
            t("Open Mandara Studio from your application menu."),
        );
        self.say(Level::Info, t("Studio design: Themes → Studio"));
        self.say(Level::Info, t("Backups and app permissions: Studio → Plugins"));
        self.say(Level::Info, t("App colours: Themes → Other apps"));
        self.say(
            Level::Info,
            t("Compositor shortcuts: Keyboard shortcuts → Compositor"),
        );
        self.say(Level::Info, t("Updates: Studio → About → Update now"));
        if !std::env::split_paths(&self.facts.path_var).any(|path| path == self.paths.bin_dir) {
            self.say(Level::Info, t("If you added Mandara to PATH, open a new terminal before running these commands."));
        }
        for line in [t("Start it:        mandara            (or log in again if autostart is on)"), t("Settings:        mandara studio"), t("Health & repair: mandara studio maintenance")] {
            self.say(Level::Info, line);
        }
    }
}
