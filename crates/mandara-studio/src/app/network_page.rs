//! Studio → Network & Bluetooth.
//!
//! * **Connections** — every wired / Wi-Fi device with its addresses,
//!   gateway, DNS, MAC and link speed (read-only).
//! * **VPN** — NetworkManager VPN and WireGuard profiles with a connect
//!   switch, plus importing a WireGuard `.conf` or OpenVPN `.ovpn` file.
//! * **Hotspot** — share this computer's connection; while it runs, a QR
//!   code phones can scan to join.
//! * **Saved networks** — autoconnect, priority, forget, IPv4 / DNS editor
//!   and joining a hidden network.
//! * **Bluetooth** — adapters, devices with battery, trust, rename and
//!   forget, discovery and pairing (prompts appear in the shell's dialog).
//!
//! NetworkManager is called directly (`mandara_services::nm_admin`, off the
//! UI thread); Bluetooth goes through the shell (`mandara msg bluetooth …`),
//! which owns the BlueZ actor and the pairing agent. Every action here is
//! also a documented command.

use mandara_ui::bidi::Bidi as _;
use std::time::{Duration, Instant};

use gpui::{AnyElement, ClickEvent, Context, FontWeight, Global, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_config::connectivity::wifi_qr_payload;
use mandara_core::ToGpui;
use mandara_i18n::{t, tf};
use mandara_services::nm_admin::{self, DbusNm, DeviceDetails, HotspotInfo, Ipv4Settings, NmBackend as _, SavedWifi, VpnCredentials, VpnInfo};
use mandara_theme::MandaraPalette;
use mandara_ui::icons::icon;
use mandara_ui::state::update_config;
use serde::Deserialize;

use super::StudioView;
use super::compositor_page::{badge, button, card, chip, group_title, heading, tip};
use super::form::{self, job, note, qr, shell, switch};
use crate::registry::{self, Section, Setting};
use crate::tokens::*;

/// Re-read NetworkManager / BlueZ this often while the page shows.
const REFRESH: Duration = Duration::from_secs(5);
/// A forget / remove click arms for this long; a second click confirms.
const ARM_WINDOW: Duration = Duration::from_secs(4);
/// Device / VPN / network row glyph.
const ROW_ICON_PX: f32 = 18.0;
const ROW_DISC_PX: f32 = 36.0;
/// QR code module size.
const QR_MODULE_PX: f32 = 4.0;
/// Detail cards: one per device.
const DETAIL_CARD_MIN_W_PX: f32 = 280.0;
/// Priority steps per click.
const PRIORITY_STEP: i32 = 10;
/// Seconds per day (relative "last used" dates).
const SECS_PER_DAY: u64 = 86_400;
/// Battery under this percent shows in amber.
const LOW_BATTERY: u8 = 20;

/// Bluetooth as `bluetooth status` reports it.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct BtStatus {
    available: bool,
    powered: bool,
    discovering: bool,
    devices: Vec<BtDevice>,
    adapters: Vec<BtAdapter>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct BtDevice {
    name: String,
    address: String,
    icon: String,
    connected: bool,
    paired: bool,
    trusted: bool,
    battery: Option<u8>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct BtAdapter {
    name: String,
    alias: String,
    powered: bool,
    selected: bool,
}

type Loaded<T> = Option<Result<T, String>>;

#[derive(Default)]
struct NetStudio {
    loaded_at: Option<Instant>,
    loading: bool,
    vpns: Loaded<Vec<VpnInfo>>,
    saved: Loaded<Vec<SavedWifi>>,
    devices: Loaded<Vec<DeviceDetails>>,
    hotspot: Option<HotspotInfo>,
    /// The hotspot's name / password revealed for sharing.
    share: Option<(String, Option<String>)>,
    bt: Loaded<BtStatus>,
    /// Saved network whose IPv4 editor is open, and its settings.
    editing: Option<(String, Option<Ipv4Settings>)>,
    /// Bluetooth device being renamed.
    renaming: Option<String>,
    armed: Option<(String, Instant)>,
    /// Actions in flight (keys of the rows they belong to).
    busy: Vec<String>,
    note: Option<(String, bool)>,
}
impl Global for NetStudio {}

fn st(cx: &mut gpui::App) -> &mut NetStudio {
    cx.default_global::<NetStudio>()
}

fn nm<T: Send + 'static>(f: impl FnOnce(&DbusNm) -> anyhow::Result<T> + Send + 'static) -> impl FnOnce() -> Result<T, String> + Send + 'static {
    move || DbusNm::connect().and_then(|nm| f(&nm)).map_err(|e| format!("{e:#}"))
}

fn bt_icon(icon: &str) -> &'static str {
    match icon {
        i if i.starts_with("audio") => "headphones",
        i if i.starts_with("input-keyboard") => "keyboard",
        i if i.starts_with("input") => "pointer",
        _ => "bluetooth",
    }
}

/// "today", "yesterday", "N days ago", or "never".
fn last_used(unix: u64) -> String {
    if unix == 0 {
        return t("never used").into();
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    match now.saturating_sub(unix) / SECS_PER_DAY {
        0 => t("used today").into(),
        1 => t("used yesterday").into(),
        d => tf("used {days} days ago", &[("days", &d)]),
    }
}

impl StudioView {
    /// Reloads everything (every `REFRESH` while shown, or after an action).
    fn net_refresh(&mut self, force: bool, cx: &mut Context<Self>) {
        let s = st(cx);
        if s.loading || (!force && s.loaded_at.is_some_and(|t| t.elapsed() < REFRESH)) {
            return;
        }
        s.loading = true;
        job(
            cx,
            nm(|nm| {
                let vpns = nm_admin::vpns(nm).map_err(|e| format!("{e:#}"));
                let saved = nm_admin::saved_wifi(nm).map_err(|e| format!("{e:#}"));
                let devices = nm.devices().map_err(|e| format!("{e:#}"));
                let hotspot = nm_admin::hotspot_status(nm, false).ok().flatten();
                Ok((vpns, saved, devices, hotspot))
            }),
            |_, r, cx| {
                let s = st(cx);
                s.loading = false;
                s.loaded_at = Some(Instant::now());
                match r {
                    Ok((vpns, saved, devices, hotspot)) => {
                        s.vpns = Some(vpns);
                        s.saved = Some(saved);
                        s.devices = Some(devices);
                        if !hotspot.as_ref().is_some_and(|h| h.active) {
                            s.share = None;
                        }
                        s.hotspot = hotspot;
                    }
                    Err(e) => {
                        s.vpns = Some(Err(e.clone()));
                        s.saved = Some(Err(e.clone()));
                        s.devices = Some(Err(e));
                    }
                }
            },
        );
        shell(vec!["bluetooth".into(), "status".into()], cx, |_, r, cx| {
            st(cx).bt = Some(r.and_then(|v| serde_json::from_value::<BtStatus>(v).map_err(|e| e.to_string())));
        });
        // Keep refreshing while this page shows.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REFRESH).await;
            let _ = this.update(cx, |view, cx| {
                if view.section == Section::Network && view.query.is_empty() {
                    view.net_refresh(false, cx);
                }
            });
        })
        .detach();
    }

    /// Runs an action, shows its result, then reloads.
    fn net_action(&mut self, key: String, work: impl FnOnce() -> Result<String, String> + Send + 'static, cx: &mut Context<Self>) {
        st(cx).busy.push(key.clone());
        job(cx, work, move |view, r, cx| {
            let s = st(cx);
            s.busy.retain(|k| *k != key);
            s.note = Some(match r {
                Ok(msg) => (msg, false),
                Err(e) => (e, true),
            });
            view.net_refresh(true, cx);
        });
    }

    /// A Bluetooth action through the shell.
    fn net_bt_action(&mut self, key: String, args: Vec<String>, done_msg: String, cx: &mut Context<Self>) {
        st(cx).busy.push(key.clone());
        shell(args, cx, move |view, r, cx| {
            let s = st(cx);
            s.busy.retain(|k| *k != key);
            s.note = Some(match r {
                Ok(_) => (done_msg, false),
                Err(e) => (e, true),
            });
            view.net_refresh(true, cx);
        });
    }

    /// First click arms, a second within `ARM_WINDOW` confirms.
    fn net_arm(key: &str, cx: &mut Context<Self>) -> bool {
        let s = st(cx);
        if s.armed.as_ref().is_some_and(|(k, at)| k == key && at.elapsed() < ARM_WINDOW) {
            s.armed = None;
            true
        } else {
            s.armed = Some((key.to_string(), Instant::now()));
            false
        }
    }

    fn net_armed(key: &str, cx: &mut Context<Self>) -> bool {
        st(cx).armed.as_ref().is_some_and(|(k, at)| k == key && at.elapsed() < ARM_WINDOW)
    }

    pub(super) fn network_page(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        self.net_refresh(false, cx);
        let group = |g: &str| -> Vec<&'static Setting> { registry::in_section(Section::Network).filter(|s| s.group == g).collect() };
        let note_el = st(cx).note.clone().map(|(text, error)| note(&text, error, p));
        let details = self.net_details(p, cx);
        let vpn = self.net_vpn(p, cx);
        let vpn_rows = self.rows(group(registry::system_settings::VPN), cfg, p, cx);
        let hotspot = self.net_hotspot(cfg, p, cx);
        let hotspot_rows = self.rows(group(registry::system_settings::HOTSPOT), cfg, p, cx);
        let saved = self.net_saved(p, cx);
        let bt = self.net_bluetooth(cfg, p, cx);
        let bt_rows = self.rows(group(registry::system_settings::BLUETOOTH), cfg, p, cx);
        let section = |title: String, body: Vec<AnyElement>| div().row().flex_col().gap(px(SPACE_10_PX)).child(group_title(title, p)).children(body);
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_26_PX))
            .children(note_el)
            .child(section(t("Connections").into(), vec![details]))
            // Registry rows carry the group title; the bespoke card follows.
            .child(div().row().flex_col().gap(px(SPACE_10_PX)).child(vpn_rows).child(vpn))
            .child(div().row().flex_col().gap(px(SPACE_10_PX)).child(hotspot_rows).child(hotspot))
            .child(section(t("Saved Wi-Fi networks").into(), vec![saved]))
            .child(div().row().flex_col().gap(px(SPACE_10_PX)).child(bt_rows).child(bt))
            .into_any_element()
    }

    fn net_details(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let devices = st(cx).devices.clone();
        let list = match devices {
            None => return card(p).child(div().text_color(p.on_surface_variant.hsla()).child(t("Reading network devices…"))).into_any_element(),
            Some(Err(e)) => return card(p).child(heading(t("NetworkManager isn't reachable"), e, p)).into_any_element(),
            Some(Ok(list)) if list.is_empty() => return card(p).child(heading(t("No network devices"), t("Wired and Wi-Fi adapters appear here."), p)).into_any_element(),
            Some(Ok(list)) => list,
        };
        let mono = mandara_ui::fonts::fonts(cx).mono.clone();
        let cards: Vec<_> = list.into_iter().map(|d| {
            let connected = d.connection.is_some();
            let line = |label: String, value: String| {
                div()
                    .row()
                    .justify_between()
                    .gap(px(SPACE_12_PX))
                    .text_size(px(TEXT_12_5_PX))
                    .child(div().text_color(p.on_surface_variant.hsla()).child(label))
                    .child(div().font_family(mono.clone()).text_end().min_w(px(0.0)).overflow_hidden().text_ellipsis().child(value))
            };
            let dash = || "—".to_string();
            card(p)
                .flex_1()
                .child(
                    div()
                        .row()
                        .items_center()
                        .gap(px(SPACE_10_PX))
                        .child(icon(if d.kind == "wifi" { "wifi-4" } else { "ethernet" }, ROW_ICON_PX, if connected { p.primary } else { p.on_surface_variant }))
                        .child(div().flex_1().min_w(px(0.0)).font_weight(FontWeight::SEMIBOLD).child(d.connection.clone().unwrap_or_else(|| t("Not connected").into())))
                        .child(badge(d.interface.clone(), p.on_surface_variant)),
                )
                .child(line(t("Address").into(), if d.addresses.is_empty() { dash() } else { d.addresses.join(", ") }))
                .child(line(t("Gateway").into(), d.gateway.clone().unwrap_or_else(dash)))
                .child(line(t("DNS").into(), if d.dns.is_empty() { dash() } else { d.dns.join(", ") }))
                .child(line(t("MAC").into(), if d.mac.is_empty() { dash() } else { d.mac.clone() }))
                .child(line(t("Speed").into(), d.speed_mbps.map(|s| tf("{speed} Mb/s", &[("speed", &s)])).unwrap_or_else(dash)))
        }).collect();
        let n = cards.len();
        super::grid::fit(self.page.column_w, CardGrid { min_w: DETAIL_CARD_MIN_W_PX, max_w: DIM_1000_PX, gap: SPACE_14_PX }).balanced(n).render(cards)
    }

    fn net_vpn(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let vpns = st(cx).vpns.clone();
        let busy = st(cx).busy.clone();
        let mut col = card(p).gap(px(SPACE_4_PX)).p(px(SPACE_8_PX));
        match vpns {
            None => col = col.child(div().p(px(SPACE_10_PX)).text_color(p.on_surface_variant.hsla()).child(t("Reading VPNs…"))),
            Some(Err(e)) => col = col.child(div().p(px(SPACE_10_PX)).child(heading(t("NetworkManager isn't reachable"), e, p))),
            Some(Ok(list)) if list.is_empty() => {
                col = col.child(div().p(px(SPACE_10_PX)).child(heading(t("No VPNs yet"), t("Import a WireGuard (.conf) or OpenVPN (.ovpn) file below — it becomes a NetworkManager connection."), p)))
            }
            Some(Ok(list)) => {
                for v in list {
                    let key = format!("vpn-{}", v.uuid);
                    let working = busy.contains(&key) || v.connecting;
                    let (uuid, name, on) = (v.uuid.clone(), v.name.clone(), v.active);
                    let remove_key = format!("vpn-rm-{}", v.uuid);
                    let armed = Self::net_armed(&remove_key, cx);
                    let (uuid2, name2) = (v.uuid.clone(), v.name.clone());
                    col = col.child(
                        div()
                            .px(px(SPACE_12_PX))
                            .py(px(SPACE_10_PX))
                            .rounded(px(RADIUS_14_PX))
                            .row()
                            .items_center()
                            .gap(px(SPACE_12_PX))
                            .when(on, |el| el.bg(p.primary.with_alpha(ALPHA_10).hsla()))
                            .child(disc("shield", on, p))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .row()
                                    .flex_col()
                                    .child(div().font_weight(FontWeight::MEDIUM).overflow_hidden().text_ellipsis().whitespace_nowrap().child(v.name.clone()))
                                    .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(if v.connecting {
                                        t("Connecting…").to_string()
                                    } else if on {
                                        t("Connected").to_string()
                                    } else {
                                        t("Off").to_string()
                                    })),
                            )
                            .child(badge(if v.kind == "wireguard" { "WireGuard".to_string() } else if v.kind == "openvpn" { "OpenVPN".to_string() } else { v.kind.clone() }, p.tertiary))
                            .child(
                                div()
                                    .id(SharedString::from(format!("{remove_key}-btn")))
                                    .size(px(DIM_30_PX))
                                    .rounded_full()
                                    .row()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .when(armed, |el| el.bg(p.urgent_coral.with_alpha(ALPHA_16).hsla()))
                                    .hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_8).hsla()))
                                    .child(icon("trash", ICON_14_PX, if armed { p.urgent_coral } else { p.on_surface_variant }))
                                    .tooltip(tip(if armed { t("Click again to delete this VPN") } else { t("Delete this VPN") }, p))
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        if Self::net_arm(&remove_key, cx) {
                                            let (u, n) = (uuid2.clone(), name2.clone());
                                            this.net_action(format!("vpn-{u}"), nm(move |nm| nm_admin::vpn_remove(nm, &u).map(|_| ())).map_ok(tf("Deleted {name}", &[("name", &n)])), cx);
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("{key}-switch")))
                                    .cursor_pointer()
                                    .when(working, |el| el.opacity(ALPHA_50))
                                    .child(switch(on, p))
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        if st(cx).busy.contains(&format!("vpn-{uuid}")) {
                                            return;
                                        }
                                        let (u, n, up) = (uuid.clone(), name.clone(), !on);
                                        if up {
                                            let remember = n.clone();
                                            update_config(cx, move |c| c.network.last_vpn = Some(remember));
                                        }
                                        let msg = if up { tf("Connecting {name}…", &[("name", &n)]) } else { tf("Disconnected {name}", &[("name", &n)]) };
                                        this.net_action(format!("vpn-{u}"), nm(move |nm| nm_admin::vpn_set(nm, &u, up).map(|_| ())).map_ok(msg), cx);
                                    })),
                            ),
                    );
                }
            }
        }
        // Import row.
        let file = form::input("vpn-file", t("~/Downloads/office.conf or .ovpn"), false, cx);
        let name = form::input("vpn-name", t("Name (optional)"), false, cx);
        let user = form::input("vpn-user", t("Username (OpenVPN, optional)"), false, cx);
        let pass = form::input("vpn-pass", t("Password (OpenVPN, optional)"), true, cx);
        let is_ovpn = form::value("vpn-file", cx).to_lowercase().ends_with(".ovpn");
        let importing = busy.iter().any(|k| k == "vpn-import");
        let import = button("vpn-import", "download", if importing { t("Importing…") } else { t("Import") }, true, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
            let file = form::value("vpn-file", cx);
            if file.is_empty() {
                st(cx).note = Some((t("Choose a .conf or .ovpn file first").into(), true));
                return cx.notify();
            }
            let path = mandara_config::paths::expand_tilde(&file);
            let name = form::value("vpn-name", cx);
            let creds = VpnCredentials { username: form::value("vpn-user", cx), password: form::value("vpn-pass", cx) };
            for k in ["vpn-file", "vpn-name", "vpn-user", "vpn-pass"] {
                form::clear(k, cx);
            }
            this.net_action(
                "vpn-import".into(),
                nm(move |nm| {
                    let creds = (!creds.username.is_empty() || !creds.password.is_empty()).then_some(&creds);
                    nm_admin::import_vpn(nm, &path, (!name.is_empty()).then_some(name.as_str()), creds)
                })
                .map_ok_with(|n| tf("Imported {name} — switch it on above", &[("name", &n)])),
                cx,
            );
        }));
        let fields = div()
            .row()
            .flex_col()
            .gap(px(SPACE_8_PX))
            .p(px(SPACE_10_PX))
            .child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface_variant.hsla()).child(t("Import a profile")))
            .child(super::grid::pair(self.page.column_w, DIM_220_PX, SPACE_8_PX, div().flex_1().child(file), div().flex_1().child(name)))
            .when(is_ovpn, |el| el.child(super::grid::pair(self.page.column_w, DIM_220_PX, SPACE_8_PX, div().flex_1().child(user), div().flex_1().child(pass))))
            .child(div().row().items_center().gap(px(SPACE_10_PX)).child(import).child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(if nm_admin::openvpn_plugin_installed() {
                t("WireGuard works out of the box; OpenVPN uses NetworkManager's plugin.")
            } else {
                t("WireGuard works out of the box; .ovpn files need networkmanager-openvpn.")
            })));
        col.child(fields).into_any_element()
    }

    fn net_hotspot(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let (hotspot, share, busy) = {
            let s = st(cx);
            (s.hotspot.clone(), s.share.clone(), s.busy.iter().any(|k| k == "hotspot"))
        };
        let running = hotspot.as_ref().is_some_and(|h| h.active);
        let ssid = hotspot.as_ref().filter(|h| running && !h.ssid.is_empty()).map(|h| h.ssid.clone()).unwrap_or_else(|| cfg.network.hotspot_name());
        let password = form::input("hotspot-pass", t("Password (8+ characters; empty keeps or makes one)"), true, cx);
        let band = cfg.network.hotspot_band.nm().map(String::from);
        let name = cfg.network.hotspot_name();
        let action = if running {
            button("hotspot-toggle", "close", if busy { t("Stopping…") } else { t("Stop hotspot") }, false, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                st(cx).share = None;
                this.net_action("hotspot".into(), nm(|nm| nm_admin::hotspot_stop(nm).map(|_| ())).map_ok(t("Hotspot stopped").into()), cx);
            }))
        } else {
            button("hotspot-toggle", "wifi-4", if busy { t("Starting…") } else { t("Start hotspot") }, true, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                let pass = form::value("hotspot-pass", cx);
                if !pass.is_empty() && !mandara_config::connectivity::valid_hotspot_password(&pass) {
                    st(cx).note = Some((t("The password needs 8–63 characters").into(), true));
                    return cx.notify();
                }
                form::clear("hotspot-pass", cx);
                let (name, band) = (name.clone(), band.clone());
                st(cx).busy.push("hotspot".into());
                job(
                    cx,
                    nm(move |nm| nm_admin::hotspot_start(nm, &name, (!pass.is_empty()).then_some(pass.as_str()), band.as_deref())),
                    |view, r, cx| {
                        let s = st(cx);
                        s.busy.retain(|k| k != "hotspot");
                        match r {
                            Ok(info) => {
                                s.note = Some((tf("Hotspot {name} is on — scan the code to join", &[("name", &info.ssid)]), false));
                                s.share = Some((info.ssid, info.password));
                            }
                            Err(e) => s.note = Some((e, true)),
                        }
                        view.net_refresh(true, cx);
                    },
                );
            }))
        };
        let reveal = running.then(|| {
            button("hotspot-share", "external", if share.is_some() { t("Hide code") } else { t("Show code") }, false, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                if st(cx).share.take().is_some() {
                    return cx.notify();
                }
                job(cx, nm(|nm| nm_admin::hotspot_status(nm, true)), |_, r, cx| match r {
                    Ok(Some(info)) => st(cx).share = Some((info.ssid, info.password)),
                    Ok(None) => {}
                    Err(e) => st(cx).note = Some((e, true)),
                });
            }))
        });
        let status = div()
            .row()
            .items_center()
            .gap(px(SPACE_12_PX))
            .child(disc("wifi-4", running, p))
            .child(heading(
                if running { tf("Sharing as {name}", &[("name", &ssid)]) } else { t("Hotspot is off").into() },
                if running { t("Phones and laptops can join this network; they use this computer's connection.").to_string() } else { tf("Starts as {name}. Wi-Fi is taken over while it runs.", &[("name", &ssid)]) },
                p,
            ))
            .child(div().flex_1());
        let qr_block = share.filter(|_| running).map(|(name, pass)| {
            let payload = wifi_qr_payload(&name, pass.as_deref(), false);
            div()
                .row()
                .flex_wrap()
                .items_center()
                .gap(px(SPACE_18_PX))
                .child(qr(&payload, QR_MODULE_PX))
                .child(
                    div()
                        .row()
                        .flex_col()
                        .gap(px(SPACE_6_PX))
                        .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("Network")))
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(name))
                        .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("Password")))
                        .child(div().font_family(mandara_ui::fonts::fonts(cx).mono.clone()).child(pass.unwrap_or_else(|| t("(stored by NetworkManager)").into()))),
                )
        });
        card(p)
            .child(status)
            .when(!running, |el| el.child(password))
            .child(div().row().gap(px(SPACE_10_PX)).child(action).children(reveal))
            .children(qr_block)
            .into_any_element()
    }

    fn net_saved(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let (saved, editing, busy) = {
            let s = st(cx);
            (s.saved.clone(), s.editing.clone(), s.busy.clone())
        };
        let mut col = card(p).gap(px(SPACE_4_PX)).p(px(SPACE_8_PX));
        match saved {
            None => col = col.child(div().p(px(SPACE_10_PX)).text_color(p.on_surface_variant.hsla()).child(t("Reading saved networks…"))),
            Some(Err(e)) => col = col.child(div().p(px(SPACE_10_PX)).child(heading(t("NetworkManager isn't reachable"), e, p))),
            Some(Ok(list)) if list.iter().all(|w| w.hotspot) => col = col.child(div().p(px(SPACE_10_PX)).child(heading(t("No saved networks"), t("Networks you join appear here."), p))),
            Some(Ok(list)) => {
                for w in list.into_iter().filter(|w| !w.hotspot) {
                    let key = format!("wifi-{}", w.uuid);
                    let working = busy.contains(&key);
                    let open = editing.as_ref().is_some_and(|(u, _)| *u == w.uuid);
                    let forget_key = format!("wifi-forget-{}", w.uuid);
                    let armed = Self::net_armed(&forget_key, cx);
                    let (u1, u2, u3, u4, u5) = (w.uuid.clone(), w.uuid.clone(), w.uuid.clone(), w.uuid.clone(), w.uuid.clone());
                    let (name, auto, prio) = (w.name.clone(), w.autoconnect, w.priority);
                    let stepper = |id: String, glyph: &'static str, delta: i32, uuid: String, cx: &mut Context<Self>| {
                        div()
                            .id(SharedString::from(id))
                            .size(px(DIM_24_PX))
                            .rounded_full()
                            .row()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .bg(p.surface_container_high.hsla())
                            .hover(|s| s.opacity(ALPHA_80))
                            .child(icon(glyph, ICON_11_PX, p.on_surface_variant))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                let (u, next) = (uuid.clone(), prio + delta);
                                this.net_action(format!("wifi-{u}"), nm(move |nm| nm_admin::set_priority(nm, &u, next).map(|_| ())).map_ok(tf("Priority {priority}", &[("priority", &next)])), cx);
                            }))
                    };
                    let row = div()
                        .px(px(SPACE_12_PX))
                        .py(px(SPACE_10_PX))
                        .rounded(px(RADIUS_14_PX))
                        .row()
                        .flex_wrap()
                        .items_center()
                        .gap(px(SPACE_12_PX))
                        .when(w.active, |el| el.bg(p.primary.with_alpha(ALPHA_10).hsla()))
                        .when(working, |el| el.opacity(ALPHA_60))
                        .child(disc(if w.secured { "lock" } else { "wifi-4" }, w.active, p))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(DIM_140_PX))
                                .row()
                                .flex_col()
                                .child(div().row().items_center().gap(px(SPACE_8_PX)).child(div().font_weight(FontWeight::MEDIUM).child(w.name.clone())).when(w.active, |el| el.child(badge(t("Connected"), p.success_emerald))).when(w.hidden, |el| el.child(badge(t("Hidden"), p.tertiary))))
                                .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(if w.ssid != w.name && !w.ssid.is_empty() { format!("{} · {}", w.ssid, last_used(w.last_used)) } else { last_used(w.last_used) })),
                        )
                        .child(
                            div()
                                .row()
                                .items_center()
                                .gap(px(SPACE_6_PX))
                                .id(SharedString::from(format!("{key}-prio")))
                                .tooltip(tip(t("Priority: the higher one is joined when several are in range"), p))
                                .child(stepper(format!("{key}-down"), "minus", -PRIORITY_STEP, u1, cx))
                                .child(div().w(px(DIM_30_PX)).text_center().text_size(px(TEXT_12_PX)).font_features(mandara_ui::fonts::tabular_nums()).child(prio.to_string()))
                                .child(stepper(format!("{key}-up"), "plus", PRIORITY_STEP, u2, cx)),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("{key}-auto")))
                                .row()
                                .items_center()
                                .gap(px(SPACE_6_PX))
                                .cursor_pointer()
                                .text_size(px(TEXT_12_PX))
                                .text_color(p.on_surface_variant.hsla())
                                .child(t("Auto-join"))
                                .child(switch(auto, p))
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    let u = u3.clone();
                                    let msg = if auto { t("Won't join automatically") } else { t("Joins automatically") };
                                    this.net_action(format!("wifi-{u}"), nm(move |nm| nm_admin::set_autoconnect(nm, &u, !auto).map(|_| ())).map_ok(msg.into()), cx);
                                })),
                        )
                        .child(chip(SharedString::from(format!("{key}-ip")), t("IP & DNS"), open, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                            let u = u4.clone();
                            if st(cx).editing.as_ref().is_some_and(|(e, _)| *e == u) {
                                st(cx).editing = None;
                                return cx.notify();
                            }
                            st(cx).editing = Some((u.clone(), None));
                            job(cx, nm(move |nm| nm_admin::ipv4(nm, &u)), |_, r, cx| match r {
                                Ok(v4) => {
                                    form::fill("ip-address", &v4.addresses.join(", "), cx);
                                    form::fill("ip-gateway", v4.gateway.as_deref().unwrap_or_default(), cx);
                                    form::fill("ip-dns", &v4.dns.join(", "), cx);
                                    if let Some((_, slot)) = st(cx).editing.as_mut() {
                                        *slot = Some(v4);
                                    }
                                }
                                Err(e) => st(cx).note = Some((e, true)),
                            });
                        })))
                        .child(
                            div()
                                .id(SharedString::from(forget_key.clone()))
                                .h(px(DIM_30_PX))
                                .px(px(SPACE_10_PX))
                                .rounded_full()
                                .row()
                                .items_center()
                                .gap(px(SPACE_6_PX))
                                .cursor_pointer()
                                .text_size(px(TEXT_12_PX))
                                .text_color(if armed { p.urgent_coral.hsla() } else { p.on_surface_variant.hsla() })
                                .when(armed, |el| el.bg(p.urgent_coral.with_alpha(ALPHA_16).hsla()))
                                .hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_8).hsla()))
                                .child(icon("trash", ICON_13_PX, if armed { p.urgent_coral } else { p.on_surface_variant }))
                                .child(if armed { t("Forget?") } else { t("Forget") })
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    if Self::net_arm(&forget_key, cx) {
                                        let (u, n) = (u5.clone(), name.clone());
                                        this.net_action(format!("wifi-{u}"), nm(move |nm| nm_admin::forget(nm, &u).map(|_| ())).map_ok(tf("Forgot {name}", &[("name", &n)])), cx);
                                    }
                                    cx.notify();
                                })),
                        );
                    col = col.child(row);
                    if open {
                        col = col.child(self.net_ip_editor(&w.uuid, editing.as_ref().and_then(|(_, v)| v.clone()), p, cx));
                    }
                }
            }
        }
        // Hidden network.
        let ssid = form::input("hidden-ssid", t("Network name"), false, cx);
        let pass = form::input("hidden-pass", t("Password (empty for open networks)"), true, cx);
        let join = button("hidden-join", "plus", t("Join"), false, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
            let (ssid, pass) = (form::value("hidden-ssid", cx), form::value("hidden-pass", cx));
            if ssid.is_empty() {
                st(cx).note = Some((t("The network name is empty").into(), true));
                return cx.notify();
            }
            form::clear("hidden-ssid", cx);
            form::clear("hidden-pass", cx);
            this.net_action("hidden".into(), nm(move |nm| nm_admin::connect_hidden(nm, &ssid, (!pass.is_empty()).then_some(pass.as_str()))).map_ok_with(|s| tf("Joining {name}…", &[("name", &s)])), cx);
        }));
        col.child(
            div()
                .p(px(SPACE_10_PX))
                .row()
                .flex_col()
                .gap(px(SPACE_8_PX))
                .child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface_variant.hsla()).child(t("Join a hidden network")))
                .child(super::grid::pair(self.page.column_w, DIM_220_PX, SPACE_8_PX, div().flex_1().child(ssid), div().flex_1().child(pass)))
                .child(div().row().child(join)),
        )
        .into_any_element()
    }

    fn net_ip_editor(&mut self, uuid: &str, v4: Option<Ipv4Settings>, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let Some(v4) = v4 else {
            return div().px(px(SPACE_20_PX)).py(px(SPACE_10_PX)).text_color(p.on_surface_variant.hsla()).child(t("Reading IP settings…")).into_any_element();
        };
        let manual = v4.method == "manual";
        let set_method = |id: &'static str, label: &'static str, method: &'static str, active: bool, cx: &mut Context<Self>| {
            chip(id, label, active, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                if let Some((_, Some(v))) = st(cx).editing.as_mut() {
                    v.method = method.into();
                }
                cx.notify();
            }))
        };
        let address = form::input("ip-address", t("Address, e.g. 192.168.1.20/24"), false, cx);
        let gateway = form::input("ip-gateway", t("Gateway, e.g. 192.168.1.1"), false, cx);
        let dns = form::input("ip-dns", t("DNS servers, e.g. 1.1.1.1, 9.9.9.9 (empty = automatic)"), false, cx);
        let uuid = uuid.to_string();
        let method = v4.method.clone();
        let save = button("ip-save", "check", t("Save"), true, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            let split = |s: String| s.split([',', ' ']).map(str::trim).filter(|x| !x.is_empty()).map(String::from).collect::<Vec<_>>();
            let dns = split(form::value("ip-dns", cx));
            let next = Ipv4Settings {
                method: method.clone(),
                addresses: if method == "manual" { split(form::value("ip-address", cx)) } else { Vec::new() },
                gateway: Some(form::value("ip-gateway", cx)).filter(|g| method == "manual" && !g.is_empty()),
                ignore_auto_dns: !dns.is_empty(),
                dns,
            };
            let u = uuid.clone();
            st(cx).editing = None;
            this.net_action(format!("wifi-{u}"), nm(move |nm| nm_admin::set_ipv4(nm, &u, &next).map(|_| ())).map_ok(t("Saved — reconnect to apply").into()), cx);
        }));
        div()
            .mx(px(SPACE_12_PX))
            .mb(px(SPACE_8_PX))
            .p(px(SPACE_14_PX))
            .rounded(px(RADIUS_14_PX))
            .bg(p.surface_container_high.hsla())
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .child(div().row().gap(px(SPACE_6_PX)).child(set_method("ip-auto", t("Automatic (DHCP)"), "auto", !manual, cx)).child(set_method("ip-manual", t("Manual"), "manual", manual, cx)))
            .when(manual, |el| el.child(super::grid::pair(self.page.column_w, DIM_220_PX, SPACE_8_PX, div().flex_1().child(address), div().flex_1().child(gateway))))
            .child(dns)
            .child(div().row().items_center().gap(px(SPACE_10_PX)).child(save).child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("Changes apply the next time this network connects."))))
            .into_any_element()
    }

    fn net_bluetooth(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let (bt, busy, renaming) = {
            let s = st(cx);
            (s.bt.clone(), s.busy.clone(), s.renaming.clone())
        };
        let status = match bt {
            None => return card(p).child(div().text_color(p.on_surface_variant.hsla()).child(t("Reading Bluetooth…"))).into_any_element(),
            Some(Err(e)) => return card(p).child(heading(t("Bluetooth needs the shell"), tf("Start Mandara to manage devices here ({error}).", &[("error", &e)]), p)).into_any_element(),
            Some(Ok(s)) if !s.available => return card(p).child(heading(t("No Bluetooth adapter"), t("Plug one in or check that the bluetooth service runs."), p)).into_any_element(),
            Some(Ok(s)) => s,
        };
        let powered = status.powered;
        let power = div()
            .id("bt-power")
            .row()
            .items_center()
            .gap(px(SPACE_8_PX))
            .cursor_pointer()
            .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(if powered { t("On") } else { t("Off") }))
            .child(switch(powered, p))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.net_bt_action("bt-power".into(), vec!["bluetooth".into(), "power".into(), if powered { "off".into() } else { "on".into() }], if powered { t("Bluetooth off").into() } else { t("Bluetooth on").into() }, cx);
            }));
        let discovering = status.discovering;
        let scan = powered.then(|| {
            button("bt-scan", "refresh", if discovering { t("Stop searching") } else { t("Find devices") }, !discovering, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.net_bt_action("bt-scan".into(), vec!["bluetooth".into(), "scan".into(), if discovering { "off".into() } else { "on".into() }], if discovering { t("Stopped searching").into() } else { t("Searching for devices…").into() }, cx);
            }))
        });
        let adapters = (status.adapters.len() > 1).then(|| {
            div().row().flex_wrap().gap(px(SPACE_6_PX)).children(status.adapters.iter().map(|a| {
                let name = a.name.clone();
                let label = if a.alias.is_empty() { a.name.clone() } else { format!("{} · {}", a.alias, a.name) };
                chip(SharedString::from(format!("bt-adapter-{}", a.name)), label, a.selected, p).when(!a.powered, |el| el.opacity(ALPHA_60)).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    let pick = Some(name.clone());
                    update_config(cx, move |c| c.bluetooth.adapter = pick);
                    st(cx).loaded_at = None;
                }))
            }))
        });
        let _ = cfg;
        let header = div().row().flex_wrap().items_center().gap(px(SPACE_12_PX)).child(disc("bluetooth", powered, p)).child(heading(t("Bluetooth"), if discovering { t("Searching — new devices appear below; Pair shows a code to compare.").to_string() } else { t("Paired devices, their battery and trust.").to_string() }, p)).child(div().flex_1()).children(scan).child(power);
        let mut col = card(p).child(header).children(adapters);
        let (known, nearby): (Vec<BtDevice>, Vec<BtDevice>) = status.devices.into_iter().partition(|d| d.paired);
        for (title, list) in [(t("Your devices"), known), (t("Nearby"), nearby)] {
            if list.is_empty() {
                continue;
            }
            col = col.child(div().pt(px(SPACE_6_PX)).text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface_variant.hsla()).child(title));
            for d in list {
                col = col.child(self.net_bt_row(d, &busy, renaming.as_deref(), p, cx));
            }
        }
        col.into_any_element()
    }

    fn net_bt_row(&mut self, d: BtDevice, busy: &[String], renaming: Option<&str>, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let key = format!("bt-{}", d.address);
        let working = busy.contains(&key);
        let addr = d.address.clone();
        let forget_key = format!("bt-forget-{}", d.address);
        let armed = Self::net_armed(&forget_key, cx);
        let is_renaming = renaming == Some(d.address.as_str());
        let mut actions = div().row().flex_wrap().items_center().gap(px(SPACE_6_PX));
        if d.paired {
            let (a1, a2, a3, a4, trusted, connected) = (addr.clone(), addr.clone(), addr.clone(), addr.clone(), d.trusted, d.connected);
            actions = actions
                .child(chip(SharedString::from(format!("{key}-conn")), if connected { t("Disconnect") } else { t("Connect") }, false, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let verb = if connected { "disconnect" } else { "connect" };
                    this.net_bt_action(format!("bt-{a1}"), vec!["bluetooth".into(), verb.into(), a1.clone()], if connected { t("Disconnected").into() } else { t("Connecting…").into() }, cx);
                })))
                .child(chip(SharedString::from(format!("{key}-trust")), t("Trusted"), trusted, p).tooltip(tip(t("Trusted devices connect without asking and reconnect automatically"), p)).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.net_bt_action(format!("bt-{a2}"), vec!["bluetooth".into(), "trust".into(), if trusted { "off".into() } else { "on".into() }, a2.clone()], if trusted { t("No longer trusted").into() } else { t("Trusted").into() }, cx);
                })))
                .child(chip(SharedString::from(format!("{key}-rename")), t("Rename"), is_renaming, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    let s = st(cx);
                    s.renaming = if s.renaming.as_deref() == Some(a3.as_str()) { None } else { Some(a3.clone()) };
                    cx.notify();
                })))
                .child(
                    chip(SharedString::from(forget_key.clone()), if armed { t("Forget?") } else { t("Forget") }, false, p)
                        .when(armed, |el| el.bg(p.urgent_coral.with_alpha(ALPHA_16).hsla()).text_color(p.urgent_coral.hsla()))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            if Self::net_arm(&forget_key, cx) {
                                this.net_bt_action(format!("bt-{a4}"), vec!["bluetooth".into(), "forget".into(), a4.clone()], t("Device removed").into(), cx);
                            }
                            cx.notify();
                        })),
                );
        } else {
            let a = addr.clone();
            actions = actions.child(button(SharedString::from(format!("{key}-pair")), "plus", if working { t("Pairing…") } else { t("Pair") }, true, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.net_bt_action(format!("bt-{a}"), vec!["bluetooth".into(), "pair".into(), a.clone()], t("Paired and trusted").into(), cx);
            })));
        }
        let battery = d.battery.map(|b| badge(format!("{b}%"), if b < LOW_BATTERY { p.warning_amber } else { p.success_emerald }));
        let rename_row = is_renaming.then(|| {
            let field = form::input("bt-rename", t("New name"), false, cx);
            let a = addr.clone();
            div().row().items_center().gap(px(SPACE_8_PX)).pad_start(px(ROW_DISC_PX + SPACE_12_PX)).child(div().flex_1().child(field)).child(button("bt-rename-save", "check", t("Save"), true, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                let name = form::value("bt-rename", cx);
                if name.is_empty() {
                    return;
                }
                form::clear("bt-rename", cx);
                st(cx).renaming = None;
                this.net_bt_action(format!("bt-{a}"), vec!["bluetooth".into(), "rename".into(), a.clone(), name.clone()], tf("Renamed to {name}", &[("name", &name)]), cx);
            })))
        });
        div()
            .px(px(SPACE_4_PX))
            .py(px(SPACE_8_PX))
            .row()
            .flex_col()
            .gap(px(SPACE_8_PX))
            .when(working, |el| el.opacity(ALPHA_60))
            .child(
                div()
                    .row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(SPACE_12_PX))
                    .child(disc(bt_icon(&d.icon), d.connected, p))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(DIM_140_PX))
                            .row()
                            .flex_col()
                            .child(div().row().items_center().gap(px(SPACE_8_PX)).child(div().font_weight(FontWeight::MEDIUM).child(d.name.clone())).when(d.connected, |el| el.child(badge(t("Connected"), p.success_emerald))).children(battery))
                            .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).font_family(mandara_ui::fonts::fonts(cx).mono.clone()).child(d.address.clone())),
                    )
                    .child(actions),
            )
            .children(rename_row)
            .into_any_element()
    }
}

/// A tinted round glyph (filled when active).
fn disc(glyph: &'static str, active: bool, p: &MandaraPalette) -> impl IntoElement {
    div()
        .size(px(ROW_DISC_PX))
        .flex_none()
        .rounded_full()
        .row()
        .items_center()
        .justify_center()
        .bg(if active { p.primary.with_alpha(ALPHA_16).hsla() } else { p.surface_container_high.hsla() })
        .child(icon(glyph, ROW_ICON_PX, if active { p.primary } else { p.on_surface_variant }))
}

/// `Result<(), String>` / `Result<T, String>` work → a message on success.
trait MapOk<T> {
    fn map_ok(self, msg: String) -> Box<dyn FnOnce() -> Result<String, String> + Send>;
    fn map_ok_with(self, f: impl FnOnce(T) -> String + Send + 'static) -> Box<dyn FnOnce() -> Result<String, String> + Send>;
}

impl<T: 'static, F: FnOnce() -> Result<T, String> + Send + 'static> MapOk<T> for F {
    fn map_ok(self, msg: String) -> Box<dyn FnOnce() -> Result<String, String> + Send> {
        Box::new(move || self().map(|_| msg))
    }
    fn map_ok_with(self, f: impl FnOnce(T) -> String + Send + 'static) -> Box<dyn FnOnce() -> Result<String, String> + Send> {
        Box::new(move || self().map(f))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_dates_and_icons() {
        assert_eq!(last_used(0), "never used");
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        assert_eq!(last_used(now), "used today");
        assert_eq!(last_used(now - 3 * SECS_PER_DAY), "used 3 days ago");
        assert_eq!(bt_icon("audio-headphones"), "headphones");
        let s: BtStatus = serde_json::from_value(serde_json::json!({ "available": true, "powered": true, "devices": [{ "name": "Buds", "address": "AA", "paired": true, "battery": 80 }] })).unwrap();
        assert_eq!(s.devices[0].battery, Some(80));
    }
}
