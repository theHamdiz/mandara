//! Output power through KDE's DPMS protocol.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_client::protocol::wl_output::WlOutput;
use wayland_protocols_plasma::dpms::client::{org_kde_kwin_dpms as dpms, org_kde_kwin_dpms_manager as manager};

use super::wayland::Wl;

#[derive(Default)]
pub(super) struct Power {
    pub manager: Option<manager::OrgKdeKwinDpmsManager>,
    outputs: BTreeMap<u32, (dpms::OrgKdeKwinDpms, bool)>,
}

impl Power {
    pub fn new(manager: Option<manager::OrgKdeKwinDpmsManager>) -> Self {
        Self { manager, ..Default::default() }
    }

    pub fn watch(&mut self, output: &WlOutput, id: u32, qh: &QueueHandle<Wl>) {
        if let Some(manager) = &self.manager {
            self.outputs.insert(id, (manager.get::<u32, Wl>(output, qh, id), false));
        }
    }

    pub fn remove(&mut self, id: u32) {
        if let Some((output, _)) = self.outputs.remove(&id) { output.release(); }
    }

    pub fn set(&self, on: bool) -> Result<()> {
        let supported: Vec<_> = self.outputs.values().filter(|(_, supported)| *supported).collect();
        if supported.is_empty() { bail!("{}", mandara_i18n::t("These displays do not support power control")); }
        for (output, _) in supported {
            output.set(if on { dpms::Mode::On } else { dpms::Mode::Off } as u32);
        }
        Ok(())
    }
}

impl Dispatch<manager::OrgKdeKwinDpmsManager, ()> for Wl {
    fn event(_: &mut Self, _: &manager::OrgKdeKwinDpmsManager, _: manager::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<dpms::OrgKdeKwinDpms, u32> for Wl {
    fn event(state: &mut Self, _: &dpms::OrgKdeKwinDpms, event: dpms::Event, id: &u32, _: &Connection, _: &QueueHandle<Self>) {
        if let dpms::Event::Supported { supported } = event && let Some((_, value)) = state.power.outputs.get_mut(id) {
            *value = supported != 0;
        }
    }
}
