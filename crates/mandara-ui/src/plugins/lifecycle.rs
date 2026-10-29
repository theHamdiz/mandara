//! Plugin lifecycle in the running shell: switching plugins on and off,
//! removing and restoring built-ins (rules in
//! [`mandara_plugins::lifecycle`]), refusing commands of off plugins, and
//! stopping the background work an off plugin owns.
//!
//! What stops where:
//! * bar items, panels and sidebar cards: the shell instantiates only
//!   enabled plugins, so their views, timers and the shared pollers they
//!   hold (containers, updates, services, drives, power, net speed…) end
//!   with the last instance;
//! * clipboard history and the CPU / memory sampler: [`crate::hub`];
//! * weather requests: `hub::ensure_weather`;
//! * privacy monitor: `alerts::configure_privacy`;
//! * tray host, soundscapes playback and the Pomodoro timer: [`sync`] here;
//! * screen-time tracking and the dock check the registry themselves.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::App;
use mandara_config::PluginsConfig;
use mandara_i18n::{t, tf};
use mandara_plugins::{CommandGate, PluginOrigin, PluginRegistry};

use crate::state::{config, update_config};

/// What to do with a plugin.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Enable,
    Disable,
    /// Built-ins only: off and out of every layout, until restored.
    Remove,
    Restore,
}

/// Whether plugin `id` is on under the live config.
pub fn is_on(id: &str, cx: &App) -> bool {
    let Some(registry) = cx.try_global::<PluginRegistry>() else { return true };
    registry.is_enabled(id, &config(cx).read(cx).get().plugins)
}

/// Why commands of plugin `owner` can't run now (it is off or removed), or
/// `None` when they can (on, or not a plugin: `core`, `lock`, …).
pub fn off_reason(owner: &str, cx: &App) -> Option<String> {
    let registry = cx.try_global::<PluginRegistry>()?;
    let manifest = &registry.get(owner)?.manifest;
    let plugins = &config(cx).read(cx).get().plugins;
    if registry.is_enabled(owner, plugins) {
        return None;
    }
    let name = manifest.display_name();
    Some(if plugins.is_removed(owner) {
        tf("{plugin} was removed. Restore it in Studio → Plugins or with `mandara msg plugins restore {id}`", &[("plugin", &name), ("id", &owner)])
    } else {
        tf("{plugin} is off. Turn it on in Studio → Plugins or with `mandara msg plugins enable {id}`", &[("plugin", &name), ("id", &owner)])
    })
}

/// Applies `action` to plugin `id` and writes the config (the shell
/// follows live). Unknown ids may still be switched on or off (a
/// third-party plugin that isn't loaded yet); removing and restoring are
/// for built-in plugins.
pub fn apply(id: &str, action: Action, cx: &mut App) -> Result<(), String> {
    let manifest = cx.try_global::<PluginRegistry>().and_then(|r| r.get(id)).map(|f| f.manifest.clone());
    let (name, zone, builtin) = match &manifest {
        Some(m) => (m.display_name(), m.zone, m.origin == PluginOrigin::Builtin),
        None => (id.to_string(), None, false),
    };
    if matches!(action, Action::Remove | Action::Restore) && !builtin {
        return Err(if manifest.is_some() {
            t("Only Mandara's own plugins are removed this way; uninstall third-party plugins from their page in Studio → Plugins").to_string()
        } else {
            tf("No Mandara plugin `{id}`; see `mandara msg plugins list`", &[("id", &id)])
        });
    }
    let mut result = Ok(());
    update_config(cx, |c| {
        result = match action {
            Action::Enable => mandara_plugins::lifecycle::set_enabled(c, id, &name, true, zone),
            Action::Disable => mandara_plugins::lifecycle::set_enabled(c, id, &name, false, zone),
            Action::Remove => mandara_plugins::lifecycle::remove(c, id, &name),
            Action::Restore => {
                mandara_plugins::lifecycle::restore(c, id, zone);
                Ok(())
            }
        };
    });
    result
}

/// Installs the command gate and starts following plugin switches.
pub fn init(cx: &mut App) {
    cx.set_global(CommandGate(Rc::new(off_reason)));
    sync(cx);
    cx.observe(&config(cx), |_, cx| sync(cx)).detach();
}

thread_local! {
    /// The plugin switches [`sync`] last acted on.
    static LAST: RefCell<Option<PluginsConfig>> = const { RefCell::new(None) };
}

/// Stops what an off plugin owns outside its views. Runs on every config
/// change; returns at once unless the plugin switches moved.
fn sync(cx: &mut App) {
    let plugins = config(cx).read(cx).get().plugins.clone();
    let unchanged = LAST.with(|l| l.borrow().as_ref().is_some_and(|p| p.enabled == plugins.enabled && p.disabled == plugins.disabled && p.removed == plugins.removed));
    if unchanged {
        return;
    }
    LAST.with(|l| *l.borrow_mut() = Some(plugins));
    if !is_on(super::tray::ID, cx) {
        super::tray::stop(cx);
    }
    if !is_on(super::soundscapes::ID, cx) {
        super::soundscapes::stop();
    }
    if !is_on(crate::productivity::POMODORO_ID, cx)
        && let Some(p) = cx.try_global::<crate::productivity::GlobalProductivity>().map(|g| g.0.clone())
    {
        p.update(cx, |p, cx| {
            if p.pomodoro.running() {
                p.pause();
                cx.notify();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use mandara_config::ShellConfig;
    use mandara_plugins::{PluginOrigin, PluginRegistry};

    fn registry() -> PluginRegistry {
        let mut r = PluginRegistry::default();
        super::super::register_builtin(&mut r);
        r
    }

    /// The built-ins that ship switched off.
    const OPT_IN: &[&str] = &["dock", "mini-stats", "prayer-times", "screen-time", "wallpaper-generator"];

    #[test]
    fn opt_in_builtins_are_off_by_default_and_prayer_times_among_them() {
        let r = registry();
        let mut off: Vec<&str> = r.manifests().filter(|m| m.origin == PluginOrigin::Builtin && !m.enabled_by_default).map(|m| m.id.as_str()).collect();
        off.sort_unstable();
        assert_eq!(off, OPT_IN);
        let c = ShellConfig::default();
        for id in OPT_IN {
            assert!(!r.is_enabled(id, &c.plugins), "{id} must be off in a fresh config");
            assert!(c.plugin_places(id).is_empty(), "{id} must not be in the default layout");
        }
    }

    #[test]
    fn every_essential_plugin_is_a_registered_builtin() {
        let r = registry();
        for (id, why) in mandara_plugins::lifecycle::ESSENTIAL {
            let m = &r.get(id).unwrap_or_else(|| panic!("{id} is not registered")).manifest;
            assert_eq!(m.origin, PluginOrigin::Builtin, "{id}");
            assert!(!why.is_empty());
            let mut c = ShellConfig::default();
            c.plugins.disabled.push(id.to_string());
            assert!(r.is_enabled(id, &c.plugins), "{id} stays on");
        }
    }

    #[test]
    fn commands_of_off_plugins_say_so_and_core_commands_run() {
        // The gate's verdict depends only on the registry and the config.
        let r = registry();
        let mut c = ShellConfig::default();
        let prayer = r.commands.all().iter().find(|s| s.path == "prayer next").expect("prayer next");
        assert!(!prayer.works_when_off && !r.is_enabled(&prayer.owner, &c.plugins), "prayer times off: `prayer next` is refused");
        let dock = r.commands.all().iter().find(|s| s.path == "dock").expect("dock");
        assert!(dock.works_when_off, "`dock on` must work while the dock is off");
        c.plugins.enabled.push("prayer-times".into());
        assert!(r.is_enabled(&prayer.owner, &c.plugins));
        // `core` and other non-plugin owners are never gated.
        assert!(r.commands.all().iter().filter(|s| s.owner == "core").all(|s| r.get(&s.owner).is_none()));
    }
}
