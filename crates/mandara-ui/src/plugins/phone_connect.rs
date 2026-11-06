//! A paired-device companion using the desktop's KDE Connect daemon.
//! Clipboard text leaves this computer only after an explicit Send click.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gpui::{AnyElement, App, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, px, size};
use mandara_config::ShellConfig;
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::phone_connect::{Action, Backend as _, Device, KdeConnect};
use mandara_theme::MandaraPalette;

use super::{model_view as fn_view, integration};
use super::panel_fx::{EXPIRY_MARGIN, glass_card, header, pill_button, readable, redraw_in};
use crate::bidi::Bidi as _;
use crate::icons::icon;
use crate::kit::{text, tokens};

pub const ID: &str = "phone-connect";
/// Room for paired device controls and the pairing prompt above the list.
const PANEL_W: f32 = 440.0;
const PANEL_H: f32 = 500.0;
const BAR_ICON_PX: f32 = 16.0;
const DEVICE_ICON_PX: f32 = 32.0;
/// The ping action uses the bundled quick-action glyph.
const PING_ICON: &str = "bolt";
pub const DEFAULT_REFRESH_SECS: i64 = 15;
pub const MIN_REFRESH_SECS: i64 = 5;
pub const MAX_REFRESH_SECS: i64 = 300;

type PhoneData = Rc<RefCell<integration::Data<(), Vec<Device>>>>;
type PhoneAction = Rc<RefCell<integration::Action<()>>>;
thread_local! {
    static DATA: RefCell<Option<PhoneData>> = const { RefCell::new(None) };
    static ACTIONS: PhoneAction = Rc::default();
}
fn data() -> PhoneData { DATA.with(|slot| slot.borrow_mut().get_or_insert_with(Rc::default).clone()) }

/// Discard a disabled device snapshot without constructing a new cache.
pub(super) fn stop() {
    if let Some(data) = DATA.with(|slot| slot.borrow_mut().take()) { data.borrow_mut().clear(); }
}

fn refresh_interval(cfg: &ShellConfig) -> Duration {
    Duration::from_secs(cfg.plugins.setting(ID, "refresh_seconds", DEFAULT_REFRESH_SECS).clamp(MIN_REFRESH_SECS, MAX_REFRESH_SECS) as u64)
}

/// Read native daemon state on demand; the panel owns the refresh cadence.
fn refresh(force: bool, cfg: &ShellConfig, cx: &mut App) {
    if !super::lifecycle::is_on(ID, cx) { return; }
    integration::load(&data(), (), force, refresh_interval(cfg), |_| KdeConnect.devices(), cx);
}

/// Coalesce explicit device actions and refresh device state on completion.
fn act(id: String, action: Action, cx: &mut App) -> async_channel::Receiver<Result<String, String>> {
    let actions = ACTIONS.with(Rc::clone);
    let views = data().borrow().views.clone();
    integration::act(&actions, (), views, move || KdeConnect.act(&id, action).map(|()| serde_json::json!({"ok":true})), |cx| {
        let cfg = crate::state::config(cx).read(cx).snapshot();
        if super::lifecycle::is_on(ID, cx) {
            data().borrow_mut().invalidate();
            refresh(true, &cfg, cx);
        }
    }, cx)
}

/// Map command names to explicit actions; unknown or destructive actions are refused.
fn command_action(value: &str) -> Option<Action> {
    Some(match value { "ring" => Action::Ring, "ping" => Action::Ping, "pair" => Action::Pair, "accept" => Action::AcceptPair, "reject" => Action::RejectPair, "unpair" => Action::Unpair, _ => return None })
}

fn action_button(device: &Device, action: Action, label: &'static str, glyph: &'static str, p: &MandaraPalette) -> AnyElement {
    let id = device.id.clone();
    pill_button(SharedString::from(format!("phone-{}-{glyph}", device.id)), Some(glyph), label, readable(p.primary, p), p,
        move |_, cx| { drop(act(id.clone(), action.clone(), cx)); }).focusable().tab_index(0).into_any_element()
}

/// Gate content controls on pairing, reachability and the advertised plugins.
fn device_card(device: &Device, pending: bool, confirm: &Rc<RefCell<Option<String>>>, p: &MandaraPalette) -> AnyElement {
    let state = if device.connected() { t("Connected") } else if device.paired { t("Paired · offline") } else { t("Available to pair") };
    let mut card = glass_card(p).p(px(tokens::MD)).row().flex_col().gap(px(tokens::SM))
        .child(div().row().items_center().gap(px(tokens::MD))
            .child(icon(if device.kind == "desktop" { "monitor" } else { "phone" }, DEVICE_ICON_PX, readable(p.primary, p)))
            .child(div().flex_1().min_w_0().row().flex_col().gap(px(tokens::XS))
                .child(div().font_weight(FontWeight::SEMIBOLD).truncate().child(device.name.clone()))
                .child(div().text_size(px(text::BODY_S)).text_color(p.on_surface_variant.hsla()).child(state)))
            .children(device.battery.as_ref().map(|battery| div().text_size(px(text::BODY_S)).font_features(crate::fonts::tabular_nums())
                .child(if battery.charging { tf("{percent}% · charging", &[("percent", &battery.percent)]) } else { format!("{}%", battery.percent) }))));
    if pending { return card.child(div().text_size(px(text::LABEL)).child(t("Working…"))).into_any_element(); }
    let mut actions = div().row().flex_wrap().gap(px(tokens::SM));
    if device.incoming_pair && !device.paired {
        card = card.child(div().text_size(px(text::BODY_S)).child(t("This device is asking to pair. Check the name on your phone before accepting.")));
        actions = actions.child(action_button(device, Action::AcceptPair, t("Accept"), "check", p)).child(action_button(device, Action::RejectPair, t("Reject"), "close", p));
    } else if device.reachable && !device.paired {
        card = card.child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(device.id.clone()));
        actions = actions.child(action_button(device, Action::Pair, t("Pair"), "plus", p));
    }
    if device.supports("kdeconnect_findmyphone") { actions = actions.child(action_button(device, Action::Ring, t("Ring"), "bell", p)); }
    if device.supports("kdeconnect_ping") { actions = actions.child(action_button(device, Action::Ping, t("Ping"), PING_ICON, p)); }
    if device.supports("kdeconnect_share") {
        let id = device.id.clone();
        actions = actions.child(pill_button(SharedString::from(format!("phone-clipboard-{id}")), Some("clipboard"), t("Send clipboard text"), readable(p.primary, p), p, move |_, cx| {
            let text = cx.read_from_clipboard().and_then(|clipboard| clipboard.text()).unwrap_or_default();
            drop(act(id.clone(), Action::ShareText(text), cx));
        }).focusable().tab_index(0));
    }
    if device.paired {
        let id = device.id.clone();
        if confirm.borrow().as_ref() == Some(&id) {
            card = card.child(div().text_size(px(text::BODY_S)).child(t("Forget this paired device?")));
            actions = actions.child(action_button(device, Action::Unpair, t("Forget device"), "close", p));
            let confirm = confirm.clone();
            actions = actions.child(pill_button(SharedString::from(format!("phone-cancel-{id}")), None, t("Cancel"), p.on_surface_variant, p, move |window, cx| {
                *confirm.borrow_mut() = None; cx.notify(window.current_view());
            }).focusable().tab_index(0));
        } else {
            let confirm = confirm.clone();
            actions = actions.child(pill_button(SharedString::from(format!("phone-forget-{id}")), None, t("Forget…"), p.on_surface_variant, p, move |window, cx| {
                *confirm.borrow_mut() = Some(id.clone()); cx.notify(window.current_view());
            }).focusable().tab_index(0));
        }
    }
    card.child(actions).into_any_element()
}

/// Load daemon state on demand and refresh only while the panel is visible.
fn panel(context: &PanelCx, confirm: &Rc<RefCell<Option<String>>>, p: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let cfg = crate::state::config(cx).read(cx).snapshot();
    let data = data();
    data.borrow().views.track(window);
    if context.progress() > 0.0 {
        refresh(false, &cfg, cx);
        redraw_in(refresh_interval(&cfg) + EXPIRY_MARGIN, window, cx);
    }
    let model = data.borrow();
    let actions = ACTIONS.with(Rc::clone);
    let actions = actions.borrow();
    let mut column = div().size_full().p(px(tokens::LG)).row().flex_col().gap(px(tokens::MD))
        .child(header("phone", readable(p.primary, p), t("Phone Connect"), t("Your devices, within reach"), p, Some(pill_button("phone-settings", Some("sliders"), t("Settings"), readable(p.primary, p), p,
            |_, cx| crate::state::open_studio_at(Some("plugins/phone-connect/settings".into()), cx)).focusable().tab_index(0).into_any_element())));
    if let Some(error) = model.error.as_deref().or_else(|| actions.error_for(&())) {
        column = column.child(glass_card(p).p(px(tokens::SM)).text_size(px(text::BODY_S)).text_color(readable(p.warning_amber, p).hsla()).child(crate::bidi::paragraph(error.to_owned())));
    }
    let available = cfg.plugins.setting(ID, "show_unpaired", true);
    let devices: Vec<_> = model.value.as_deref().into_iter().flatten().filter(|device| device.paired || available).collect();
    let content = if devices.is_empty() {
        div().flex_1().row().flex_col().items_center().justify_center().gap(px(tokens::MD))
            .child(icon("phone", DEVICE_ICON_PX, p.on_surface_variant))
            .child(if model.busy() { t("Looking for your devices…") } else { t("No devices yet") })
            .child(div().text_size(px(text::BODY_S)).text_color(p.on_surface_variant.hsla()).child(crate::bidi::paragraph(t("Install KDE Connect on your computer and phone, then pair them here.")))).into_any_element()
    } else {
        div().id("phone-devices").flex_1().min_h_0().overflow_y_scroll().row().flex_col().gap(px(tokens::SM))
            .children(devices.iter().map(|device| device_card(device, actions.busy(), confirm, p))).into_any_element()
    };
    column.child(content).child(pill_button("phone-refresh", Some("refresh"), t("Refresh"), readable(p.primary, p), p, |_, cx| {
        let cfg = crate::state::config(cx).read(cx).snapshot(); refresh(true, &cfg, cx);
    }).focusable().tab_index(0)).into_any_element()
}

/// Register lazy phone views and commands without starting the daemon at startup.
pub fn register(registry: &mut PluginRegistry) {
    registry.register(PluginManifest::builtin(ID, tk("Phone Connect"), tk("Paired devices, phone battery, ringing and text sharing"), Some(Zone::Right)).opt_in(), |window, cx, context| {
        let bar = fn_view(None, |p, _, _| icon("phone", crate::style::bar_icon(BAR_ICON_PX), readable(p.primary, p)).into_any_element(), window, cx);
        let context = context.clone();
        let confirm = Rc::default();
        let view = fn_view(None, move |p, window, cx| panel(&context, &confirm, p, window, cx), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view, size: Rc::new(Cell::new(size(px(PANEL_W), px(PANEL_H)))) }), ..PluginInstance::empty() }
    });
    registry.command(ID, CommandSpec::new_output("phone devices", tk("Paired and discovered devices reported by KDE Connect"), |_, _| Ok(crate::powerctl::pending(|| KdeConnect.devices().and_then(|devices| serde_json::to_value(devices).map_err(|error| error.to_string()))))));
    registry.command(ID, CommandSpec::new_output("phone action", tk("Ring, ping or change pairing for a device"), |args, cx| {
        let action = command_action(args.str(1)?).ok_or_else(|| t("Unknown phone action").to_string())?;
        Ok(Output::Pending(act(args.str(0)?.into(), action, cx)))
    }).arg(ArgSpec::required("device", ArgKind::String, tk("Device id from phone devices"))).arg(ArgSpec::required("action", ArgKind::String, tk("Action")).choices(&["ring", "ping", "pair", "accept", "reject", "unpair"])).example("phone action phone_01 ring"));
    registry.command(ID, CommandSpec::new_output("phone send", tk("Send text to a paired, connected device"), |args, cx| Ok(Output::Pending(act(args.str(0)?.into(), Action::ShareText(args.rest(1)), cx))))
        .arg(ArgSpec::required("device", ArgKind::String, tk("Device id from phone devices"))).arg(ArgSpec::required("text", ArgKind::String, tk("Text to send"))).example("phone send phone_01 'Hello from Mandara'"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabling_releases_a_snapshot_even_if_a_request_still_holds_the_data_slot() {
        stop();
        assert!(DATA.with(|slot| slot.borrow().is_none()));
        let held = data();
        let snapshot = std::sync::Arc::new(Vec::<Device>::with_capacity(64));
        let weak = std::sync::Arc::downgrade(&snapshot);
        held.borrow_mut().value = Some(snapshot);
        stop();
        assert!(weak.upgrade().is_none());
        assert!(held.borrow().value.is_none());
        assert!(DATA.with(|slot| slot.borrow().is_none()));
    }

    #[test]
    fn destructive_and_unknown_commands_are_never_implicit() {
        assert_eq!(command_action("ring"), Some(Action::Ring));
        assert_eq!(command_action("unpair"), Some(Action::Unpair));
        assert_eq!(command_action("unknown"), None);
        assert_eq!(refresh_interval(&ShellConfig::default()), Duration::from_secs(DEFAULT_REFRESH_SECS as u64));
    }

    #[test]
    fn ping_uses_a_bundled_icon() {
        assert!(crate::icons::svg_source(PING_ICON).is_some());
    }
}
