//! Mandara Login's state for `mandara install`, asked in-process: whether
//! it is the login screen, whether its installed copy is out of date, and
//! what `greeter use mandara` / `greeter keyring-unlock` would change.

use std::path::{Path, PathBuf};

use mandara_install::LoginProbe;

use crate::install::Kiosk;
use crate::keyring_pam::{Action, KeyringPlan};
use crate::manager::{self, MANDARA, SwitchPlan, SysRoot};

/// greetd's unit name without `.service`, as `display-manager.service`
/// points to it.
const GREETD: &str = "greetd";

/// [`LoginProbe`] on the real system.
pub struct InstallerProbe;

impl InstallerProbe {
    fn plan(exe: &Path) -> Result<SwitchPlan, String> {
        SwitchPlan::build(&SysRoot::system(), MANDARA, Kiosk::Cage, PathBuf::from(exe))
    }
}

impl LoginProbe for InstallerProbe {
    fn active(&self) -> bool {
        let d = manager::detect(&SysRoot::system());
        d.display_manager.as_deref() == Some(GREETD) && d.active == Some(MANDARA)
    }

    fn outdated(&self, exe: &Path) -> bool {
        Self::plan(exe).is_ok_and(|p| p.already_active && p.install_mandara)
    }

    fn switch_plan(&self, exe: &Path, after_packages: bool) -> String {
        match Self::plan(exe) {
            Ok(mut plan) => {
                if after_packages {
                    // What the installer installs first no longer blocks it.
                    let root = SysRoot::system();
                    let resolved: Vec<String> = manager::missing_for(&root, plan.target, plan.kiosk, plan.detection.family).iter().map(manager::Missing::describe).collect();
                    plan.errors.retain(|e| !resolved.contains(e));
                }
                plan.describe()
            }
            Err(e) => format!("error: {e}"),
        }
    }

    fn keyring_plan(&self) -> Option<String> {
        let plan = KeyringPlan::build(&SysRoot::system(), Action::Add);
        plan.changes().then(|| plan.describe())
    }
}
