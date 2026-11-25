//! Background update checks (`updates.self_check`): Mandara itself and,
//! with `updates.plugins`, marketplace plugins. The checks reuse their
//! cached answer for `updates.check_interval_hours`, so this ticks often
//! but asks GitHub rarely; each new version is announced once with a
//! regular notification (quiet modes apply). Installing happens in Studio
//! → About or with `mandara update apply`.

use std::time::Duration;

use gpui::App;
use mandara_i18n::{t, tf};
use mandara_services::self_update::{self as su, plugins};

use crate::state::config;

/// First check after startup (out of the way of session start).
const STARTUP_DELAY: Duration = Duration::from_secs(120);
/// How often the cache is consulted.
const TICK: Duration = Duration::from_secs(30 * 60);

fn notify(summary: String, body: String, cx: &mut App) {
    crate::plugins::notifications::send(cx, mandara_services::NotificationCommand::Post { app: t("Mandara updates").into(), icon: "download".into(), summary, body });
}

pub fn init(cx: &mut App) {
    cx.spawn(async move |cx| {
        cx.background_executor().timer(STARTUP_DELAY).await;
        loop {
            let updates = cx.update(|cx| config(cx).read(cx).snapshot().updates.clone());
            if updates.self_check && !mandara_services::offline::is_offline() {
                let interval = updates.interval();
                let with_plugins = updates.plugins;
                let task = cx.background_executor().spawn(async move {
                    let shell = su::check(false, interval).inspect_err(|e| tracing::debug!("update check: {e}")).ok();
                    let plugins = if with_plugins { plugins::check_all(false, interval).inspect_err(|e| tracing::debug!("plugin update check: {e}")).unwrap_or_default() } else { Vec::new() };
                    let shell = shell.filter(|i| i.available && su::first_shell_announcement(&i.latest));
                    let plugins: Vec<_> = plugins.into_iter().filter(|u| u.available && plugins::first_announcement(u)).collect();
                    (shell, plugins)
                });
                let (shell, plugin_updates) = task.await;
                cx.update(|cx| {
                    if let Some(info) = shell {
                        let size = match (info.download_size(false), info.full_size()) {
                            (Some(d), Some(f)) if info.patch.is_some() => tf("Smart update: {patch} instead of {full}", &[("patch", &su::human_size(d)), ("full", &su::human_size(f))]),
                            (_, Some(f)) => tf("Full update: {size} (no patch from your version)", &[("size", &su::human_size(f))]),
                            _ => String::new(),
                        };
                        let body = if size.is_empty() { t("Install it from Studio → About.").to_string() } else { format!("{size}. {}", t("Install it from Studio → About.")) };
                        notify(tf("Mandara {version} is available", &[("version", &info.latest)]), body, cx);
                    }
                    for u in plugin_updates {
                        notify(tf("{plugin} {version} is available", &[("plugin", &u.id), ("version", &u.latest_version())]), t("Update it from Studio → Plugins.").to_string(), cx);
                    }
                });
            }
            cx.background_executor().timer(TICK).await;
        }
    })
    .detach();
}
