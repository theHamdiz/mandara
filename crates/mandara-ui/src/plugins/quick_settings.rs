//! Quick settings: toggle tiles for the sidebar (Wi-Fi, Bluetooth,
//! light/dark appearance, keep-awake, night light, mute all sound,
//! presentation mode when its plugin is on, and
//! KeePassXC when it's installed), a screen brightness slider (laptop
//! backlight or DDC/CI monitors), plus the keep-awake command.

use gpui::{App, Global, IntoElement, ParentElement, Styled, div, px};
use mandara_core::ToGpui as _;
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginInstance, PluginManifest, PluginRegistry};
use mandara_services::{BluetoothCommand, NetworkCommand};
use mandara_theme::MandaraPalette;
use serde_json::json;

use super::fn_view;
use crate::hub::hub;
use crate::fonts::tabular_nums;
use crate::kit::{text, toggle_tile, tokens};
use crate::state::{config, update_config};

pub const ID: &str = "quick-settings";

/// The brightness slider's id (its drag state in [`crate::slider_model`]).
const BRIGHTNESS_SLIDER: &str = "qs-brightness";
/// Gap between the brightness label line and its slider.
const BRIGHTNESS_ROW_GAP_PX: f32 = 6.0;
const PERCENT: f32 = 100.0;

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

/// Keep toggle behavior shared while a menu supplies only its frame.
fn decorated_toggle(
    decorate: &impl Fn(gpui::Stateful<gpui::Div>, &'static str, bool) -> gpui::Stateful<gpui::Div>,
    id: &'static str,
    icon: &'static str,
    label: impl Into<gpui::SharedString>,
    detail: Option<String>,
    active: bool,
    palette: &MandaraPalette,
    on_click: impl Fn(&mut gpui::Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    decorate(toggle_tile(id, icon, label, detail, active, palette, on_click), id, active)
}

/// The toggle tiles (Wi-Fi, Bluetooth, appearance, keep awake, night
/// light, mute all, presentation, KeePassXC) and the brightness slider: the sidebar card,
/// and the main menu's home page.
pub fn tiles(palette: &MandaraPalette, cx: &mut App) -> gpui::AnyElement {
    tiles_with(palette, cx, |tile, _, _| tile)
}

/// Reuse the same controls with the containing surface's card decoration.
/// Sidebar callers retain the ordinary quick-toggle appearance.
pub(super) fn tiles_with(
    palette: &MandaraPalette,
    cx: &mut App,
    decorate: impl Fn(gpui::Stateful<gpui::Div>, &'static str, bool) -> gpui::Stateful<gpui::Div>,
) -> gpui::AnyElement {
                let hub = hub(cx).read(cx);
                let (net, bt) = (&hub.network, &hub.bluetooth);
                let (wifi_on, bt_on) = (net.wifi_enabled, bt.powered);
                let dark = config(cx).read(cx).get().theme.mode.is_dark();
                let awake = keep_awake_active(cx);
                let row1 = div()
                    .flex()
                    .gap(px(tokens::SM))
                    .child(decorated_toggle(
                        &decorate,
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
                    .child(decorated_toggle(
                        &decorate,
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
                    .child(decorated_toggle(
                        &decorate,
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
                    .child(decorated_toggle(
                        &decorate,
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
                    .child(decorated_toggle(
                        &decorate,
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
                    .child(decorated_toggle(
                        &decorate,
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
                // Presentation mode (when its plugin is on) and KeePassXC (when
                // it's installed and Mandara's dialog is on) share a row.
                let presentation = {
                    let cfg = config(cx).read(cx).snapshot();
                    cx.global::<mandara_plugins::PluginRegistry>().is_enabled(super::presentation::ID, &cfg.plugins)
                }
                .then(|| {
                    let on = super::presentation::active(cx);
                    decorated_toggle(
                        &decorate,
                        "qs-presentation",
                        "monitor",
                        mandara_i18n::t("Presenting"),
                        Some(if on { mandara_i18n::t("On").into() } else { mandara_i18n::t("Off").into() }),
                        on,
                        palette,
                        move |_, cx| super::presentation::set(!on, false, cx),
                    )
                });
                let keepass = crate::keepassxc_prompt::available(cx).then(|| {
                    let open = crate::keepassxc_prompt::unlocked(cx);
                    decorated_toggle(
                        &decorate,
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
                    )
                });
                let row4 = (presentation.is_some() || keepass.is_some()).then(|| div().flex().gap(px(tokens::SM)).children(presentation).children(keepass));
                // Screen brightness, when this machine can change it.
                let brightness = crate::osd::brightness_level(cx).map(|level| {
                    let shown = crate::slider_model::held(&BRIGHTNESS_SLIDER.into()).unwrap_or(level);
                    div()
                        .pt(px(tokens::XS))
                        .flex()
                        .flex_col()
                        .gap(px(BRIGHTNESS_ROW_GAP_PX))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(tokens::SM))
                                .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(text::BODY_S)).child(mandara_i18n::t("Brightness")))
                                .child(div().font_features(tabular_nums()).text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(format!("{}%", (shown * PERCENT).round()))),
                        )
                        .child(super::panel_fx::big_slider(BRIGHTNESS_SLIDER, level, "sun", false, palette, |v, _, cx| crate::osd::set_brightness(v, cx)))
                });
                div().flex().flex_col().gap(px(tokens::SM)).child(row1).child(row2).child(row3).children(row4).children(brightness).into_any_element()
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, mandara_i18n::tk("Quick settings"), mandara_i18n::tk("Toggles for the sidebar"),None), |window, cx, _| {
        let sidebar = fn_view(None, |palette, _, cx| tiles(palette, cx), window, cx);
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
