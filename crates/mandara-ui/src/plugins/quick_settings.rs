//! Quick settings: toggle tiles for the sidebar (Wi-Fi, Bluetooth,
//! light/dark appearance, keep-awake, night light, mute all sound, and
//! KeePassXC when it's installed) plus the keep-awake command.

use gpui::{App, Global, IntoElement, ParentElement, Styled, div, px};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginInstance, PluginManifest, PluginRegistry};
use mandara_services::{BluetoothCommand, NetworkCommand};
use serde_json::json;

use super::fn_view;
use crate::hub::hub;
use crate::kit::{toggle_tile, tokens};
use crate::state::{config, update_config};

pub const ID: &str = "quick-settings";

/// Holds the logind idle/sleep inhibitor while keep-awake is on.
#[derive(Default)]
pub struct KeepAwake(Option<mandara_services::session::Inhibitor>);
impl Global for KeepAwake {}

pub fn keep_awake_active(cx: &App) -> bool {
    cx.try_global::<KeepAwake>().is_some_and(|k| k.0.is_some())
}

pub fn set_keep_awake(on: bool, cx: &mut App) -> Result<bool, String> {
    if !cx.has_global::<KeepAwake>() {
        cx.set_global(KeepAwake::default());
    }
    if on && !keep_awake_active(cx) {
        let inhibitor = mandara_services::session::inhibit_idle("Keep awake")?;
        cx.global_mut::<KeepAwake>().0 = Some(inhibitor);
    } else if !on {
        // Dropping the inhibitor's fd releases the lock.
        cx.global_mut::<KeepAwake>().0 = None;
    }
    cx.refresh_windows();
    Ok(on)
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, mandara_i18n::tk("Quick settings"), mandara_i18n::tk("Toggles for the sidebar"),None), |window, cx, _| {
        let sidebar = fn_view(
            None,
            |palette, _, cx| {
                let hub = hub(cx).read(cx);
                let (net, bt) = (&hub.network, &hub.bluetooth);
                let (wifi_on, bt_on) = (net.wifi_enabled, bt.powered);
                let dark = config(cx).read(cx).get().theme.mode.is_dark();
                let awake = keep_awake_active(cx);
                let row1 = div()
                    .flex()
                    .gap(px(tokens::SM))
                    .child(toggle_tile(
                        "qs-wifi",
                        super::network::status_icon(net),
                        mandara_i18n::t("Wi-Fi"),
                        Some(if net.wifi_enabled && !net.primary.is_empty() { net.primary.clone() } else if net.wifi_enabled { mandara_i18n::t("On").into() } else { mandara_i18n::t("Off").into() }),
                        net.wifi_enabled,
                        palette,
                        move |_, cx| {
                            if let Some(s) = super::hub_network(cx) {
                                s.send(NetworkCommand::SetWifi(!wifi_on));
                            }
                        },
                    ))
                    .child(toggle_tile(
                        "qs-bt",
                        if bt.powered { "bluetooth" } else { "bluetooth-off" },
                        mandara_i18n::t("Bluetooth"),
                        Some(if !bt.powered {
                            mandara_i18n::t("Off").into()
                        } else if bt.connected_count() > 0 {
                            mandara_i18n::tf("{count} connected", &[("count", &bt.connected_count())])
                        } else {
                            mandara_i18n::t("On").into()
                        }),
                        bt.powered,
                        palette,
                        move |_, cx| {
                            if let Some(s) = super::hub_bluetooth(cx) {
                                s.send(BluetoothCommand::SetPower(!bt_on));
                            }
                        },
                    ));
                let row2 = div()
                    .flex()
                    .gap(px(tokens::SM))
                    .child(toggle_tile(
                        "qs-theme",
                        if dark { "moon" } else { "sun" },
                        if dark { mandara_i18n::t("Dark") } else { mandara_i18n::t("Light") },
                        Some(mandara_i18n::t("Appearance").into()),
                        dark,
                        palette,
                        |_, cx| {
                            let mode = config(cx).read(cx).get().theme.mode.toggled();
                            update_config(cx, |c| c.theme.mode = mode);
                        },
                    ))
                    .child(toggle_tile(
                        "qs-awake",
                        "coffee",
                        mandara_i18n::t("Keep awake"),
                        Some(if awake { mandara_i18n::t("On").into() } else { mandara_i18n::t("Off").into() }),
                        awake,
                        palette,
                        move |_, cx| {
                            let _ = set_keep_awake(!awake, cx);
                        },
                    ));
                let night = crate::night_light::active(cx);
                let all_muted = crate::hub::hub(cx).read(cx).audio.all_muted;
                let row3 = div()
                    .flex()
                    .gap(px(tokens::SM))
                    .child(toggle_tile(
                        "qs-night-light",
                        "sunset",
                        mandara_i18n::t("Night light"),
                        Some(if night { mandara_i18n::t("Warm").into() } else { mandara_i18n::t("Off").into() }),
                        night,
                        palette,
                        |_, cx| {
                            crate::night_light::toggle(cx);
                            cx.refresh_windows();
                        },
                    ))
                    // Global mute (every output and app; `audio mute-all`).
                    .child(toggle_tile(
                        "qs-mute-all",
                        if all_muted { "volume-off" } else { "volume-high" },
                        mandara_i18n::t("Mute all"),
                        Some(if all_muted { mandara_i18n::t("All sound muted").into() } else { mandara_i18n::t("Off").into() }),
                        all_muted,
                        palette,
                        |_, cx| {
                            if let Some(s) = crate::hub::hub(cx).read(cx).audio_service.clone() {
                                s.send(mandara_services::AudioCommand::ToggleMuteAll);
                            }
                        },
                    ));
                // KeePassXC: only when it's installed and Mandara's dialog is on.
                let keepass = crate::keepassxc_prompt::available(cx).then(|| {
                    let open = crate::keepassxc_prompt::unlocked(cx);
                    div().flex().gap(px(tokens::SM)).child(toggle_tile(
                        "qs-keepassxc",
                        if open { "lock-open" } else { "key" },
                        mandara_i18n::t("KeePassXC"),
                        Some(if open { mandara_i18n::t("Unlocked").into() } else { mandara_i18n::t("Unlock").into() }),
                        open,
                        palette,
                        |_, cx| {
                            if let Err(e) = crate::keepassxc_prompt::open(None, cx) {
                                tracing::warn!("keepassxc: {e}");
                            }
                        },
                    ))
                });
                div().flex().flex_col().gap(px(tokens::SM)).child(row1).child(row2).child(row3).children(keepass).into_any_element()
            },
            window,
            cx,
        );
        PluginInstance { sidebar: Some(sidebar), ..PluginInstance::empty() }
    });
    r.command(
        ID,
        CommandSpec::new("keep-awake", mandara_i18n::tk("Block idle and sleep (logind inhibitor)"), |a, cx| {
            let on = match a.str(0)? {
                "toggle" => !keep_awake_active(cx),
                other => matches!(other, "on" | "true" | "1"),
            };
            Ok(json!({ "keep_awake": set_keep_awake(on, cx)? }))
        })
        .arg(ArgSpec::required("state", ArgKind::Enum, mandara_i18n::tk("on / off / toggle")).choices(&["on", "off", "toggle"])),
    );
}
