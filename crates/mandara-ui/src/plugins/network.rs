//! Network plugin (NetworkManager): connectivity icon in the bar; a panel
//! with the current connection as a hero (signal, band, live throughput),
//! the Wi-Fi radio switch, rescan, and nearby networks with signal bars,
//! lock badges and connect / disconnect actions.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::SharedString;
use gpui::{AnyElement, App, FontWeight, InteractiveElement as _, IntoElement, ParentElement, StatefulInteractiveElement as _, Styled, div, prelude::FluentBuilder as _, px, size};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::network::{AccessPoint, Connectivity, NetworkCommand, NetworkState, strength_tier};
use mandara_theme::MandaraPalette;
use serde_json::json;

use super::panel_fx::{
    self, ANGLE_DIAGONAL_DEG, Cached, HEADER_H, PanelShape, SECTION_H_PX, WASH_SOFT, empty_state, header, partner, pill_button, readable, redraw_in, section, signal_bars,
    wash,
};
use super::{fn_view, json};
use crate::fonts::tabular_nums;
use crate::hub::hub;
use crate::icons::icon;
use crate::kit::{icon_button, reveal, text};
use crate::widgets::system::format_bytes;

pub const ID: &str = "network";

const PANEL_W: f32 = 372.0;
/// Panel height before the first layout pass.
const INITIAL_PANEL_H: f32 = 360.0;
const PAD: f32 = 14.0;
const HERO_H: f32 = 96.0;
const ROW_H: f32 = 48.0;
/// Other Wi-Fi networks listed (`networks_shown`).
pub const DEFAULT_NETWORKS_SHOWN: i64 = 7;
pub const NETWORKS_SHOWN_RANGE: (i64, i64) = (1, 20);
const GAP: f32 = 12.0;
/// Height of the "no networks" empty state.
const EMPTY_LIST_H: f32 = 96.0;
/// Gap between the networks section label and its list.
const LIST_LABEL_GAP_PX: f32 = 4.0;
/// Gap between the header controls.
const CONTROLS_GAP_PX: f32 = 6.0;
/// Rescan button side.
const SCAN_BUTTON_PX: f32 = 30.0;
/// Bar status glyph and its alpha while disconnected.
const BAR_ICON_PX: f32 = 15.0;
const BAR_OFFLINE_ALPHA: f32 = 0.6;

/// Lower bounds of the 6 GHz and 5 GHz Wi-Fi bands.
const BAND_6GHZ_FROM_MHZ: u32 = 5900;
const BAND_5GHZ_FROM_MHZ: u32 = 4900;

/// Throughput sampling cadence while the panel is visible.
const RATE_EVERY: Duration = Duration::from_secs(1);
/// A sample younger than this is not retaken (absorbs timer jitter).
const RATE_MIN_GAP: Duration = Duration::from_millis(900);
/// Shortest interval used to divide byte deltas (guards tiny `dt`).
const RATE_MIN_DT_S: f64 = 0.1;

// Hero card.
/// Hero background: OKLCH lightness of the primary / tertiary stops and
/// `(share of chroma kept, chroma cap)` per stop, on dark and light themes.
const HERO_DARK_L: (f32, f32) = (0.29, 0.22);
const HERO_DARK_CHROMA: ((f32, f32), (f32, f32)) = ((0.55, 0.07), (0.5, 0.055));
const HERO_LIGHT_L: (f32, f32) = (0.93, 0.955);
const HERO_LIGHT_CHROMA: ((f32, f32), (f32, f32)) = ((0.35, 0.045), (0.3, 0.035));
/// Hero gradient angle.
const HERO_ANGLE_DEG: f32 = 125.0;
const HERO_PAD_X_PX: f32 = 14.0;
const HERO_GAP_PX: f32 = 14.0;
const HERO_GLYPH_PX: f32 = 52.0;
const HERO_ICON_PX: f32 = 24.0;
/// Disconnected glyph wash `(start, end)` alphas.
const HERO_IDLE_WASH: (f32, f32) = (0.1, 0.06);
const HERO_TITLE_PX: f32 = 16.0;
const HERO_LINE_GAP_PX: f32 = 2.0;
const RATES_TOP_PX: f32 = 4.0;
const RATES_GAP_PX: f32 = 12.0;
const RATE_ARROW_GAP_PX: f32 = 4.0;

// Access point rows.
const ROW_PAD_X_PX: f32 = 10.0;
const ROW_RADIUS_PX: f32 = 12.0;
const ROW_GAP_PX: f32 = 12.0;
const ROW_BARS_W_PX: f32 = 18.0;
const ROW_DETAIL_GAP_PX: f32 = 4.0;
const LOCK_ICON_PX: f32 = 10.0;

pub fn status_icon(state: &NetworkState) -> &'static str {
    match state.connectivity {
        Connectivity::Ethernet => "ethernet",
        Connectivity::Wifi => {
            const TIERS: [&str; 5] = ["wifi-0", "wifi-1", "wifi-2", "wifi-3", "wifi-4"];
            TIERS[usize::from(strength_tier(state.strength)).min(TIERS.len() - 1)]
        }
        _ if !state.wifi_enabled => "wifi-off",
        _ => "wifi-0",
    }
}

fn request(cx: &mut App, command: NetworkCommand) -> Result<Output, String> {
    let service = hub(cx).read(cx).network_service.clone().ok_or("network service unavailable")?;
    Ok(Output::Pending(service.request(command)))
}

fn send(cx: &mut App, command: NetworkCommand) {
    if let Some(s) = hub(cx).read(cx).network_service.clone() {
        s.send(command);
    }
}

/// A small pill switch (shared with other plugins).
pub fn switch(on: bool, palette: &MandaraPalette) -> impl IntoElement {
    panel_fx::switch(on, palette)
}

fn band(ap: &AccessPoint) -> &'static str {
    if ap.frequency_mhz > BAND_6GHZ_FROM_MHZ {
        "6 GHz"
    } else if ap.frequency_mhz > BAND_5GHZ_FROM_MHZ {
        "5 GHz"
    } else {
        "2.4 GHz"
    }
}

// ── Live throughput (from sysfs counters of the default-route interface) ─

#[derive(Default)]
struct Rates {
    iface: Option<String>,
    last: Option<(u64, u64, Instant)>,
    down: f64,
    up: f64,
    /// A sysfs read is running on the background executor.
    in_flight: bool,
    down_text: Cached<u64>,
    up_text: Cached<u64>,
}

fn default_iface() -> Option<String> {
    std::fs::read_to_string("/proc/net/route")
        .ok()?
        .lines()
        .skip(1)
        .find(|l| l.split_whitespace().nth(1) == Some("00000000"))
        .and_then(|l| l.split_whitespace().next())
        .map(str::to_string)
}

fn counters(iface: &str) -> Option<(u64, u64)> {
    let read = |f: &str| std::fs::read_to_string(format!("/sys/class/net/{iface}/statistics/{f}")).ok()?.trim().parse::<u64>().ok();
    Some((read("rx_bytes")?, read("tx_bytes")?))
}

/// One blocking read of the default-route interface and its counters.
fn read_counters() -> (Option<String>, Option<(u64, u64)>) {
    let iface = default_iface();
    let counters = iface.as_deref().and_then(counters);
    (iface, counters)
}

impl Rates {
    fn due(&self) -> bool {
        !self.in_flight && self.last.is_none_or(|(_, _, t)| t.elapsed() >= RATE_MIN_GAP)
    }

    /// Folds a read taken at `now` into the rates.
    fn apply(&mut self, (iface, counters): (Option<String>, Option<(u64, u64)>), now: Instant) {
        if iface != self.iface {
            self.iface = iface;
            self.last = None;
        }
        let Some((rx, tx)) = counters else { return };
        if let Some((prx, ptx, at)) = self.last {
            let dt = now.duration_since(at).as_secs_f64().max(RATE_MIN_DT_S);
            self.down = rx.saturating_sub(prx) as f64 / dt;
            self.up = tx.saturating_sub(ptx) as f64 / dt;
        }
        self.last = Some((rx, tx, now));
    }
}

/// Starts a throughput read off the UI thread when one is due; re-renders
/// the calling view when it lands (sysfs reads never block a frame).
fn sample_rates(rates: &Rc<RefCell<Rates>>, window: &gpui::Window, cx: &mut App) {
    if !rates.borrow().due() {
        return;
    }
    rates.borrow_mut().in_flight = true;
    let view = window.current_view();
    let task = cx.background_executor().spawn(async { read_counters() });
    let rates = rates.clone();
    cx.spawn(async move |cx| {
        let read = task.await;
        {
            let mut r = rates.borrow_mut();
            r.in_flight = false;
            r.apply(read, Instant::now());
        }
        cx.update(|cx| cx.notify(view));
    })
    .detach();
}

fn rate(bytes_per_sec: &u64) -> String {
    tf("{size}/s", &[("size", &format_bytes(*bytes_per_sec))])
}

// ── Views ────────────────────────────────────────────────────────────────

fn subtitle(state: &NetworkState) -> &'static str {
    match state.connectivity {
        Connectivity::Wifi => t("Connected · Wi-Fi"),
        Connectivity::Ethernet => t("Connected · Wired"),
        Connectivity::Connecting => t("Connecting…"),
        Connectivity::Disconnected if !state.wifi_enabled => t("Wi-Fi is off"),
        Connectivity::Disconnected => t("Not connected"),
    }
}

fn hero(state: &NetworkState, rates: &mut Rates, palette: &MandaraPalette, radius: f32) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let connected = matches!(state.connectivity, Connectivity::Wifi | Connectivity::Ethernet);
    let tone = |c: mandara_theme::Color, l: f32, (share, cap): (f32, f32)| c.with_lightness(l).with_chroma((c.chroma() * share).min(cap));
    let (bg_a, bg_b) = if palette.is_dark {
        (tone(palette.primary, HERO_DARK_L.0, HERO_DARK_CHROMA.0), tone(palette.tertiary, HERO_DARK_L.1, HERO_DARK_CHROMA.1))
    } else {
        (tone(palette.primary, HERO_LIGHT_L.0, HERO_LIGHT_CHROMA.0), tone(palette.tertiary, HERO_LIGHT_L.1, HERO_LIGHT_CHROMA.1))
    };
    let active_ap = state.access_points.iter().find(|a| a.active);
    let (title, detail): (SharedString, SharedString) = match state.connectivity {
        Connectivity::Wifi => (
            state.primary.clone().into(),
            match active_ap {
                Some(ap) => format!("{} · {} · {}%", if ap.secured { t("Secured") } else { t("Open") }, band(ap), state.strength).into(),
                None => format!("Wi-Fi · {}%", state.strength).into(),
            },
        ),
        Connectivity::Ethernet => (state.primary.clone().into(), t("Wired connection").into()),
        Connectivity::Connecting => (t("Connecting…").into(), state.primary.clone().into()),
        Connectivity::Disconnected if !state.wifi_enabled => (t("Wi-Fi is off").into(), t("Turn it on to see nearby networks").into()),
        Connectivity::Disconnected => (t("Not connected").into(), t("Pick a network below").into()),
    };
    // VPNs and the hotspot ride along on the detail line.
    let detail: SharedString = match (state.vpns.is_empty(), state.hotspot) {
        (true, false) => detail,
        (vpn_off, hotspot) => {
            let mut parts = vec![detail.to_string()];
            if !vpn_off {
                parts.push(tf("VPN {name}", &[("name", &state.vpns.join(", "))]));
            }
            if hotspot {
                parts.push(t("Hotspot on").into());
            }
            parts.join(" · ").into()
        }
    };
    let glyph = div()
        .size(px(HERO_GLYPH_PX))
        .flex_none()
        .rounded_full()
        .bg(if connected {
            oklab_gradient(ANGLE_DIAGONAL_DEG, palette.primary, partner(palette.primary).with_lightness(palette.primary.lightness()))
        } else {
            oklab_gradient(ANGLE_DIAGONAL_DEG, palette.on_surface.with_alpha(HERO_IDLE_WASH.0), palette.on_surface.with_alpha(HERO_IDLE_WASH.1))
        })
        .flex()
        .items_center()
        .justify_center()
        .child(icon(status_icon(state), HERO_ICON_PX, if connected { palette.on_primary } else { palette.on_surface_variant }));
    let trailing: Option<AnyElement> = match state.connectivity {
        Connectivity::Wifi => Some(pill_button("net-disconnect", None, t("Disconnect"), accent, palette, |_, cx| send(cx, NetworkCommand::Disconnect)).into_any_element()),
        Connectivity::Disconnected if !state.wifi_enabled && state.wifi_hardware => {
            Some(pill_button("net-on", Some("wifi-4"), t("Turn on"), accent, palette, |_, cx| send(cx, NetworkCommand::SetWifi(true))).into_any_element())
        }
        _ => None,
    };
    let show_rates = connected && rates.last.is_some();
    let (down, up) = if show_rates {
        // Formatted only when the whole-byte rate changes.
        let (d, u) = (rates.down as u64, rates.up as u64);
        (rates.down_text.get(d, rate), rates.up_text.get(u, rate))
    } else {
        Default::default()
    };
    div()
        .h(px(HERO_H))
        .rounded(px(radius))
        .bg(oklab_gradient(HERO_ANGLE_DEG, bg_a, bg_b))
        .shadow(panel_fx::rim(palette))
        .px(px(HERO_PAD_X_PX))
        .flex()
        .items_center()
        .gap(px(HERO_GAP_PX))
        .child(glyph)
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(HERO_LINE_GAP_PX))
                .child(div().text_size(px(HERO_TITLE_PX)).font_weight(FontWeight::SEMIBOLD).truncate().child(title))
                .child(div().text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).truncate().child(detail))
                .when(show_rates, |el| {
                    el.child(
                        div()
                            .mt(px(RATES_TOP_PX))
                            .flex()
                            .gap(px(RATES_GAP_PX))
                            .font_features(tabular_nums())
                            .text_size(px(text::LABEL))
                            .text_color(palette.on_surface_variant.hsla())
                            .child(div().flex().items_center().gap(px(RATE_ARROW_GAP_PX)).child(div().text_color(accent.hsla()).child("↓")).child(down))
                            .child(div().flex().items_center().gap(px(RATE_ARROW_GAP_PX)).child(div().text_color(readable(palette.tertiary, palette).hsla()).child("↑")).child(up)),
                    )
                }),
        )
        .when_some(trailing, |el, t| el.child(t))
        .into_any_element()
}

fn ap_row(i: usize, ap: &AccessPoint, palette: &MandaraPalette) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let ssid = ap.ssid.clone();
    let hover = wash(WASH_SOFT, palette);
    div()
        .id(("ap", i))
        .h(px(ROW_H))
        .px(px(ROW_PAD_X_PX))
        .rounded(px(crate::style::radius(ROW_RADIUS_PX)))
        .flex()
        .items_center()
        .gap(px(ROW_GAP_PX))
        .cursor_pointer()
        .hover(move |s| s.bg(hover.hsla()))
        .child(div().w(px(ROW_BARS_W_PX)).flex().justify_center().child(signal_bars(ap.strength, accent, palette)))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).truncate().child(ap.ssid.clone()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(ROW_DETAIL_GAP_PX))
                        .text_size(px(text::LABEL))
                        .text_color(palette.on_surface_variant.hsla())
                        .when(ap.secured, |el| el.child(icon("lock", LOCK_ICON_PX, palette.on_surface_variant)))
                        .child(format!("{} · {}", if ap.secured { t("Secured") } else { t("Open") }, band(ap))),
                ),
        )
        .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(accent.hsla()).child(t("Connect")))
        .on_click(move |_, _, cx| send(cx, NetworkCommand::Connect { ssid: ssid.clone(), password: None }))
        .into_any_element()
}

fn panel_view(p: PanelCx, size_cell: Rc<Cell<gpui::Size<gpui::Pixels>>>) -> impl Fn(&MandaraPalette, &mut gpui::Window, &mut App) -> AnyElement {
    let rates = Rc::new(RefCell::new(Rates::default()));
    move |palette, window, cx| {
        let shape = PanelShape::of(cx);
        if p.progress() > 0.0 {
            sample_rates(&rates, window, cx);
            redraw_in(RATE_EVERY, window, cx);
        }
        // Borrow this frame's hub state instead of cloning its lists.
        let hub = hub(cx);
        let state = &hub.read(cx).network;
        let limit: i64 = crate::state::config(cx).read(cx).snapshot().plugins.setting(ID, "networks_shown", DEFAULT_NETWORKS_SHOWN);
        let limit = limit.clamp(NETWORKS_SHOWN_RANGE.0, NETWORKS_SHOWN_RANGE.1) as usize;
        let others: Vec<&AccessPoint> = state.access_points.iter().filter(|a| !a.active).take(limit).collect();
        let list_h = if !state.wifi_enabled { 0.0 } else if others.is_empty() { EMPTY_LIST_H } else { others.len() as f32 * ROW_H };
        let list_block = if state.wifi_enabled { GAP + SECTION_H_PX + LIST_LABEL_GAP_PX + list_h } else { 0.0 };
        size_cell.set(size(px(PANEL_W), px(PAD * 2.0 + HEADER_H + GAP + HERO_H + list_block)));
        let accent = readable(palette.primary, palette);
        let controls = div()
            .flex()
            .items_center()
            .gap(px(CONTROLS_GAP_PX))
            .when(state.wifi_enabled, |el| el.child(icon_button("net-scan", "refresh", SCAN_BUTTON_PX, palette.on_surface_variant, None, |_, cx| send(cx, NetworkCommand::Scan))))
            .when(state.wifi_hardware || state.wifi_enabled, |el| {
                el.child(div().id("net-wifi-switch").cursor_pointer().child(panel_fx::switch(state.wifi_enabled, palette)).on_click({
                    let on = state.wifi_enabled;
                    move |_, _, cx| send(cx, NetworkCommand::SetWifi(!on))
                }))
            });
        let mut col = div()
            .size_full()
            .p(px(PAD))
            .flex()
            .flex_col()
            .child(reveal(&p, 0, header(status_icon(state), accent, t("Network"), subtitle(state), palette, Some(controls.into_any_element()))))
            .child(div().h(px(GAP)))
            .child(reveal(&p, 1, hero(state, &mut rates.borrow_mut(), palette, shape.inner(PAD))));
        if state.wifi_enabled {
            let count = state.access_points.iter().filter(|a| !a.active).count();
            let list = if others.is_empty() {
                empty_state("wifi-0", t("No networks found"), t("Rescan or move closer to an access point"), list_h, palette).into_any_element()
            } else {
                div().flex().flex_col().children(others.iter().enumerate().map(|(i, ap)| ap_row(i, ap, palette))).into_any_element()
            };
            col = col.child(div().h(px(GAP))).child(reveal(
                &p,
                2,
                div()
                    .flex()
                    .flex_col()
                    .gap(px(LIST_LABEL_GAP_PX))
                    .child(section(
                        t("NETWORKS"),
                        palette,
                        (count > 0).then(|| div().text_size(px(text::LABEL)).font_features(tabular_nums()).text_color(palette.on_surface_variant.hsla()).child(count.to_string()).into_any_element()),
                    ))
                    .child(list),
            ));
        }
        col.into_any_element()
    }
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Network"), tk("Wi-Fi and wired connectivity"), Some(Zone::Right)), |window, cx, panel_cx| {
        let bar = fn_view(
            None,
            |palette, _, cx| {
                let state = &hub(cx).read(cx).network;
                let color = if state.connectivity == Connectivity::Disconnected { palette.on_surface_variant.with_alpha(BAR_OFFLINE_ALPHA) } else { palette.on_surface };
                div().flex().items_center().child(icon(status_icon(state), crate::style::bar_icon(BAR_ICON_PX), color)).into_any_element()
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
                let on = hub(cx).read(cx).network.wifi_enabled;
                send(cx, NetworkCommand::SetWifi(!on));
            })),
            ..PluginInstance::empty()
        }
    });

    r.command(ID, CommandSpec::new("network status", tk("Connectivity, Wi-Fi radio and signal"), |_, cx| {
        let s = &hub(cx).read(cx).network;
        Ok(json!({ "connectivity": s.connectivity, "primary": s.primary, "wifi_enabled": s.wifi_enabled, "strength": s.strength, "interface": default_iface() }))
    }));
    r.command(ID, CommandSpec::new("network wifi list", tk("Nearby Wi-Fi networks"), |_, cx| Ok(json(&hub(cx).read(cx).network.access_points))));
    r.command(
        ID,
        CommandSpec::new_output("network wifi power", tk("Turn the Wi-Fi radio on or off"), |a, cx| request(cx, NetworkCommand::SetWifi(a.bool(0)?)))
            .arg(ArgSpec::required("state", ArgKind::Bool, tk("on / off")))
            .example("network wifi power off"),
    );
    r.command(ID, CommandSpec::new_output("network wifi scan", tk("Rescan for networks"), |_, cx| request(cx, NetworkCommand::Scan)));
    r.command(ID, CommandSpec::new_output("network wifi disconnect", tk("Disconnect Wi-Fi"), |_, cx| request(cx, NetworkCommand::Disconnect)));
    r.command(
        ID,
        CommandSpec::new_output("network wifi connect", tk("Connect to a network (saved profile, or with a password)"), |a, cx| {
            let ssid = a.str(0)?.to_string();
            let password = a.get(1).map(str::to_string);
            request(cx, NetworkCommand::Connect { ssid, password })
        })
        .arg(ArgSpec::required("ssid", ArgKind::String, tk("network name (quote if it has spaces)")))
        .arg(ArgSpec::optional("password", ArgKind::String, tk("WPA passphrase for new networks")))
        .example("network wifi connect Home-5G"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_and_rates() {
        let ap = |f| AccessPoint { ssid: "x".into(), strength: 50, secured: true, active: false, frequency_mhz: f };
        assert_eq!(band(&ap(2412)), "2.4 GHz");
        assert_eq!(band(&ap(5180)), "5 GHz");
        assert_eq!(band(&ap(5955)), "6 GHz");
        let mut r = Rates::default();
        let t0 = Instant::now();
        r.apply((Some("wlan0".into()), Some((1000, 500))), t0);
        r.apply((Some("wlan0".into()), Some((3000, 1500))), t0 + Duration::from_secs(2));
        assert_eq!((r.down, r.up), (1000.0, 500.0));
        assert!(!r.due(), "a fresh sample is not retaken");
        assert_eq!(r.down_text.get(r.down as u64, rate).as_ref(), rate(&1000).as_str());
    }
}
