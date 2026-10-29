//! Bluetooth plugin (BlueZ): status in the bar; a panel with the adapter
//! power switch and discovery, connected devices as cards (battery when the
//! device reports it), then paired and nearby devices with connect /
//! disconnect actions.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{AnyElement, App, SharedString, FontWeight, InteractiveElement as _, IntoElement, ParentElement, StatefulInteractiveElement as _, Styled, div, prelude::FluentBuilder as _, px, size};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::BluetoothCommand;
use mandara_services::bluetooth::{BluetoothDevice, BluetoothState};
use mandara_theme::MandaraPalette;

use super::panel_fx::{
    self, ANGLE_DIAGONAL_DEG, HEADER_H, PanelShape, SECTION_H_PX, WASH_SOFT, WASH_TRACK, empty_state, glass_card, header, partner, percent_text, pill_button, readable,
    section, wash,
};
use super::{fn_view, json};
use crate::fonts::tabular_nums;
use crate::hub::hub;
use crate::icons::{icon, ring};
use crate::kit::{icon_button, reveal, text};

pub const ID: &str = "bluetooth";

const PANEL_W: f32 = 372.0;
const PAD: f32 = 14.0;
const CARD_H: f32 = 64.0;
const ROW_H: f32 = 46.0;
const GAP: f32 = 12.0;
/// Gap between a section label and its list.
const SECTION_GAP_PX: f32 = 4.0;
const SECTION: f32 = SECTION_H_PX + SECTION_GAP_PX;
const MAX_LIST: usize = 6;
/// Nearby devices always get at least this many rows.
const MIN_NEARBY: usize = 2;
const EMPTY_H: f32 = 150.0;
/// Room under an empty state for its action button.
const EMPTY_ACTION_H: f32 = 40.0;
/// Panel height before the first layout pass.
const INITIAL_PANEL_H: f32 = 260.0;
/// Gap between connected-device cards.
const CARD_GAP_PX: f32 = 8.0;
const CONTROLS_GAP_PX: f32 = 6.0;
const SCAN_BUTTON_PX: f32 = 30.0;
/// Fill behind the scan button while discovering.
const SCANNING_FILL_ALPHA: f32 = 0.14;

// Connected-device card.
const CARD_PAD_X_PX: f32 = 12.0;
const CARD_GAP_X_PX: f32 = 12.0;
const CARD_GLYPH_PX: f32 = 40.0;
const CARD_ICON_PX: f32 = 19.0;
const CARD_LINE_GAP_PX: f32 = 2.0;
/// Battery ring: diameter, stroke, track alpha and gap to the value.
const BATTERY_RING_PX: f32 = 16.0;
const BATTERY_RING_STROKE_PX: f32 = 2.5;
const BATTERY_TRACK_ALPHA: f32 = 0.12;
const BATTERY_GAP_PX: f32 = 5.0;
/// Battery percent at or below which the ring turns coral / amber.
/// Coral at or below Power → peripheral low battery (`power.peripheral_low_percent`);
/// amber at or below `battery_warn_percent`.
pub const DEFAULT_BATTERY_WARN_PERCENT: i64 = 30;
pub const BATTERY_WARN_RANGE: (i64, i64) = (5, 90);

/// `(critical, warn)` battery levels of the device rings.
fn battery_levels(cx: &App) -> (u8, u8) {
    let cfg = crate::state::config(cx).read(cx).snapshot();
    let critical = cfg.power.peripheral_low_percent;
    let warn = cfg.plugins.setting(ID, "battery_warn_percent", DEFAULT_BATTERY_WARN_PERCENT).clamp(BATTERY_WARN_RANGE.0, BATTERY_WARN_RANGE.1) as u8;
    (critical, warn.max(critical))
}
/// Battery levels are reported in percent.
const PERCENT: f32 = 100.0;

// Device rows.
const ROW_PAD_X_PX: f32 = 10.0;
const ROW_RADIUS_PX: f32 = 12.0;
const ROW_GAP_PX: f32 = 12.0;
const ROW_GLYPH_PX: f32 = 30.0;
const ROW_ICON_PX: f32 = 15.0;

// Bar item.
const BAR_ICON_PX: f32 = 15.0;
const BAR_GAP_PX: f32 = 4.0;
const BAR_OFF_ALPHA: f32 = 0.6;

/// Device-kind keywords (BlueZ `Icon` names) → glyph, first match wins.
const DEVICE_ICONS: &[(&[&str], &str)] = &[
    (&["audio", "headset", "headphone"], "headphones"),
    (&["keyboard"], "keyboard"),
    (&["mouse", "input"], "pointer"),
    (&["phone"], "monitor"),
];

fn request(cx: &mut App, command: BluetoothCommand) -> Result<Output, String> {
    let service = hub(cx).read(cx).bluetooth_service.clone().ok_or("bluetooth service unavailable")?;
    Ok(Output::Pending(service.request(command)))
}

fn send(cx: &mut App, command: BluetoothCommand) {
    if let Some(s) = hub(cx).read(cx).bluetooth_service.clone() {
        s.send(command);
    }
}

/// ASCII case-insensitive substring test (no allocation per row / frame).
fn contains_ci(hay: &str, needle: &str) -> bool {
    hay.as_bytes().windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

fn device_icon(kind: &str) -> &'static str {
    DEVICE_ICONS.iter().find(|(keys, _)| keys.iter().any(|k| contains_ci(kind, k))).map_or("bluetooth", |(_, icon)| icon)
}

fn subtitle(s: &BluetoothState) -> SharedString {
    if !s.available {
        t("No adapter found").into()
    } else if !s.powered {
        t("Off").into()
    } else if s.discovering {
        t("Searching for devices…").into()
    } else {
        match s.connected_count() {
            0 => t("On · nothing connected").into(),
            n => tf("{n} connected", &[("n", &n)]).into(),
        }
    }
}

fn connected_card(i: usize, d: &BluetoothDevice, (critical, warn): (u8, u8), palette: &MandaraPalette, radius: f32) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let address = d.address.clone();
    glass_card(palette)
        .rounded(px(radius))
        .h(px(CARD_H))
        .px(px(CARD_PAD_X_PX))
        .flex()
        .items_center()
        .gap(px(CARD_GAP_X_PX))
        .child(
            div()
                .size(px(CARD_GLYPH_PX))
                .flex_none()
                .rounded_full()
                .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, palette.primary, partner(palette.primary).with_lightness(palette.primary.lightness())))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(device_icon(&d.icon), CARD_ICON_PX, palette.on_primary)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(CARD_LINE_GAP_PX))
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).truncate().child(d.name.clone()))
                .child(div().text_size(px(text::LABEL)).text_color(accent.hsla()).child(t("Connected"))),
        )
        .when_some(d.battery, |el, b| {
            let level = f32::from(b) / PERCENT;
            let color = if b <= critical {
                palette.urgent_coral
            } else if b <= warn {
                palette.warning_amber
            } else {
                palette.success_emerald
            };
            el.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(BATTERY_GAP_PX))
                    .child(ring(level, BATTERY_RING_PX, BATTERY_RING_STROKE_PX, readable(color, palette), palette.on_surface.with_alpha(BATTERY_TRACK_ALPHA)))
                    .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).font_features(tabular_nums()).child(percent_text(level))),
            )
        })
        .child(pill_button(("bt-disconnect", i), None, t("Disconnect"), palette.on_surface_variant, palette, move |_, cx| send(cx, BluetoothCommand::Disconnect(address.clone()))))
        .into_any_element()
}

fn device_row(key: &'static str, i: usize, d: &BluetoothDevice, palette: &MandaraPalette) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let address = d.address.clone();
    let hover = wash(WASH_SOFT, palette);
    div()
        .id((key, i))
        .h(px(ROW_H))
        .px(px(ROW_PAD_X_PX))
        .rounded(px(crate::style::radius(ROW_RADIUS_PX)))
        .flex()
        .items_center()
        .gap(px(ROW_GAP_PX))
        .cursor_pointer()
        .hover(move |s| s.bg(hover.hsla()))
        .child(
            div()
                .size(px(ROW_GLYPH_PX))
                .flex_none()
                .rounded_full()
                .bg(palette.on_surface.with_alpha(if palette.is_dark { WASH_TRACK.0 } else { WASH_SOFT.0 }).hsla())
                .flex()
                .items_center()
                .justify_center()
                .child(icon(device_icon(&d.icon), ROW_ICON_PX, palette.on_surface_variant)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).truncate().child(d.name.clone()))
                .child(div().text_size(px(text::LABEL)).font_features(tabular_nums()).text_color(palette.on_surface_variant.hsla()).child(match (d.paired, d.battery) {
                    (true, Some(b)) => SharedString::from(tf("Paired · {level}%", &[("level", &b)])),
                    (true, None) => t("Paired").into(),
                    (false, _) => t("Available").into(),
                })),
        )
        .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(accent.hsla()).child(t("Connect")))
        .on_click(move |_, _, cx| send(cx, BluetoothCommand::Connect(address.clone())))
        .into_any_element()
}

fn panel_view(p: PanelCx, size_cell: Rc<Cell<gpui::Size<gpui::Pixels>>>) -> impl Fn(&MandaraPalette, &mut gpui::Window, &mut App) -> AnyElement {
    move |palette, _, cx| {
        let shape = PanelShape::of(cx);
        let levels = battery_levels(cx);
        // Borrow this frame's hub state instead of cloning the device list.
        let hub = hub(cx);
        let s = &hub.read(cx).bluetooth;
        let r = shape.inner(PAD);
        let accent = readable(palette.primary, palette);
        let connected: Vec<&BluetoothDevice> = s.devices.iter().filter(|d| d.connected).collect();
        let paired: Vec<&BluetoothDevice> = s.devices.iter().filter(|d| !d.connected && d.paired).take(MAX_LIST).collect();
        let nearby: Vec<&BluetoothDevice> = s.devices.iter().filter(|d| !d.connected && !d.paired).take(MAX_LIST.saturating_sub(paired.len()).max(MIN_NEARBY)).collect();

        let controls = div()
            .flex()
            .items_center()
            .gap(px(CONTROLS_GAP_PX))
            .when(s.powered, |el| {
                let fill = s.discovering.then(|| accent.with_alpha(SCANNING_FILL_ALPHA));
                el.child(icon_button("bt-scan", "refresh", SCAN_BUTTON_PX, if s.discovering { accent } else { palette.on_surface_variant }, fill, {
                    let on = s.discovering;
                    move |_, cx| send(cx, BluetoothCommand::SetDiscovery(!on))
                }))
            })
            .when(s.available, |el| {
                el.child(div().id("bt-power").cursor_pointer().child(panel_fx::switch(s.powered, palette)).on_click({
                    let on = s.powered;
                    move |_, _, cx| send(cx, BluetoothCommand::SetPower(!on))
                }))
            });
        let mut h = PAD * 2.0 + HEADER_H;
        let mut col = div()
            .size_full()
            .p(px(PAD))
            .flex()
            .flex_col()
            .child(reveal(&p, 0, header(if s.powered { "bluetooth" } else { "bluetooth-off" }, accent, t("Bluetooth"), subtitle(s), palette, Some(controls.into_any_element()))));
        if !s.available || !s.powered {
            h += GAP + EMPTY_H;
            let (title, hint) = if s.available {
                (t("Bluetooth is off"), t("Turn it on to connect your devices"))
            } else {
                (t("No Bluetooth adapter"), t("Plug in an adapter or enable it in firmware"))
            };
            let mut block = div().flex().flex_col().items_center().child(empty_state("bluetooth-off", title, hint, EMPTY_H - EMPTY_ACTION_H, palette));
            if s.available {
                block = block.child(pill_button("bt-on", Some("bluetooth"), t("Turn on"), accent, palette, |_, cx| send(cx, BluetoothCommand::SetPower(true))));
            }
            col = col.child(div().h(px(GAP))).child(reveal(&p, 1, block));
        } else if s.devices.is_empty() {
            h += GAP + EMPTY_H;
            let mut block = div().flex().flex_col().items_center().child(empty_state("bluetooth", t("No devices yet"), t("Put a device in pairing mode, then search"), EMPTY_H - EMPTY_ACTION_H, palette));
            if !s.discovering {
                block = block.child(pill_button("bt-search", Some("search"), t("Search"), accent, palette, |_, cx| send(cx, BluetoothCommand::SetDiscovery(true))));
            }
            col = col.child(div().h(px(GAP))).child(reveal(&p, 1, block));
        } else {
            let mut idx = 1;
            if !connected.is_empty() {
                h += GAP + connected.len() as f32 * CARD_H + (connected.len() as f32 - 1.0) * CARD_GAP_PX;
                col = col.child(div().h(px(GAP))).child(reveal(
                    &p,
                    idx,
                    div().flex().flex_col().gap(px(CARD_GAP_PX)).children(connected.iter().enumerate().map(|(i, d)| connected_card(i, d, levels, palette, r))),
                ));
                idx += 1;
            }
            for (label, key, list) in [(t("PAIRED"), "bt-paired", &paired), (t("NEARBY"), "bt-nearby", &nearby)] {
                if list.is_empty() {
                    continue;
                }
                h += GAP + SECTION + list.len() as f32 * ROW_H;
                col = col.child(div().h(px(GAP))).child(reveal(
                    &p,
                    idx,
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(SECTION_GAP_PX))
                        .child(section(label, palette, None))
                        .child(div().flex().flex_col().children(list.iter().enumerate().map(|(i, d)| device_row(key, i, d, palette)))),
                ));
                idx += 1;
            }
        }
        size_cell.set(size(px(PANEL_W), px(h)));
        col.into_any_element()
    }
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Bluetooth"), tk("Bluetooth devices"), Some(Zone::Right)), |window, cx, panel_cx| {
        let bar = fn_view(
            None,
            |palette, _, cx| {
                let s = &hub(cx).read(cx).bluetooth;
                if !s.available {
                    return div().into_any_element();
                }
                let connected = s.connected_count();
                let tint = if connected > 0 {
                    palette.primary
                } else if s.powered {
                    palette.on_surface
                } else {
                    palette.on_surface_variant.with_alpha(BAR_OFF_ALPHA)
                };
                div()
                    .flex()
                    .items_center()
                    .gap(px(BAR_GAP_PX))
                    .child(icon(if s.powered { "bluetooth" } else { "bluetooth-off" }, crate::style::bar_icon(BAR_ICON_PX), tint))
                    .when(connected > 0, |el| {
                        el.child(div().text_size(px(text::LABEL)).font_features(tabular_nums()).text_color(palette.on_surface_variant.hsla()).child(connected.to_string()))
                    })
                    .into_any_element()
            },
            window,
            cx,
        );
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(INITIAL_PANEL_H))));
        let panel = fn_view(None, panel_view(panel_cx.clone(), size_cell.clone()), window, cx);
        PluginInstance {
            bar: Some(bar),
            panel: Some(PanelSpec { view: panel, size: size_cell }),
            on_secondary_click: Some(Rc::new(|_, cx| {
                let on = hub(cx).read(cx).bluetooth.powered;
                send(cx, BluetoothCommand::SetPower(!on));
            })),
            ..PluginInstance::empty()
        }
    });

    r.command(ID, CommandSpec::new("bluetooth status", tk("Adapter state and devices"), |_, cx| Ok(json(&hub(cx).read(cx).bluetooth))));
    r.command(
        ID,
        CommandSpec::new_output("bluetooth power", tk("Turn the adapter on or off"), |a, cx| request(cx, BluetoothCommand::SetPower(a.bool(0)?)))
            .arg(ArgSpec::required("state", ArgKind::Bool, tk("on / off"))),
    );
    r.command(
        ID,
        CommandSpec::new_output("bluetooth scan", tk("Start or stop discovery"), |a, cx| request(cx, BluetoothCommand::SetDiscovery(a.bool(0)?)))
            .arg(ArgSpec::required("state", ArgKind::Bool, tk("on / off"))),
    );
    r.command(
        ID,
        CommandSpec::new_output("bluetooth connect", tk("Connect a device by name or address"), |a, cx| request(cx, BluetoothCommand::Connect(a.rest(0))))
            .arg(ArgSpec::required("device", ArgKind::String, tk("device name or MAC address")))
            .example("bluetooth connect WH-1000XM5"),
    );
    r.command(
        ID,
        CommandSpec::new_output("bluetooth disconnect", tk("Disconnect a device by name or address"), |a, cx| request(cx, BluetoothCommand::Disconnect(a.rest(0))))
            .arg(ArgSpec::required("device", ArgKind::String, tk("device name or MAC address"))),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_icons() {
        assert_eq!(device_icon("audio-headset"), "headphones");
        assert_eq!(device_icon("input-keyboard"), "keyboard");
        assert_eq!(device_icon("input-mouse"), "pointer");
        assert_eq!(device_icon(""), "bluetooth");
        assert_eq!(device_icon("Audio-Card"), "headphones");
        assert!(contains_ci("input-Gaming", "gaming") && !contains_ci("ab", "abc"));
    }
}
