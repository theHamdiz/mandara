//! Pinned and floating plugin panels, for every plugin (third-party ones
//! included): the host does it all, plugins need nothing new.
//!
//! **Pin.** A docked panel shows a small tab on its far edge while hovered
//! (pin, pop out). Pinned, it stays open through outside clicks, focus
//! changes and overlays; its chip, Esc, `panel close` or unpinning end it.
//! When another panel needs the bar's slot, the pinned one pops out into a
//! floating card instead of closing ([`dock`]).
//!
//! **Float.** A floating card lives on its output's `mandara-floating`
//! surface: one transparent, full-output layer surface on the Top layer,
//! created when the first card floats there and closed when the last one
//! goes. All cards are drawn on it and dragged inside it (the surface
//! itself never moves or resizes, which jitters on compositors); its input
//! region is the union of the cards, updated every frame they move, so
//! everything else clicks through ([`view`]). Each card is its own
//! [`mandara_plugins::PluginInstance`], built from the plugin's factory on
//! that surface, exactly like the bar builds one per output; plugins keep
//! shared state in globals, so both instances show the same data.
//!
//! A card morphs out of the docked panel (or its chip) on the panel spring
//! and back into the bar when docked; it never closes on outside clicks.
//! Positions are remembered per plugin per output (`[[panels.floating]]`)
//! and, with `panels.remember_floating`, cards floating at shutdown come
//! back there.
//!
//! **Keyboard.** The floating surface never takes the keyboard
//! (`KeyboardInteractivity::None`), so clicking a card never steals it from
//! your window. Layer surfaces can't change that after creation, so while
//! a text field inside a card is focused (by a click), a one-pixel bridge
//! surface with exclusive keyboard exists and forwards each key to the
//! card ([`bridge`]); it goes away on Esc, a click outside the cards, a
//! click elsewhere in the card, the card closing, or another window taking
//! focus.
//!
//! Commands: `panel pin|unpin|float|dock <id>`, `panel floating`, and
//! `panel open|close|toggle` (see `plugins::core_commands`).

pub mod bridge;
pub mod dock;
pub mod geom;
pub mod view;

use gpui::layer_shell::{Anchor as LayerAnchor, KeyboardInteractivity, Layer};
use gpui::{AnyWindowHandle, App, AppContext as _, Bounds, Entity, Global, Keystroke, Pixels, WeakEntity, WindowHandle};
use mandara_core::SurfaceRole;
use mandara_i18n::{t, tf};
use mandara_plugins::PluginRegistry;
use serde_json::{Value, json};

use self::dock::Effect;
use self::geom::Anchor;
use self::view::{FloatingView, Placement};
use crate::shell::ShellView;
use crate::state::{config, shell, update_config};

/// Layer-shell exclusive zone `-1`: placed against the whole output, so
/// the floating surface shares the shell surface's coordinates.
const IGNORE_ZONES: f32 = -1.0;

/// Every output's shell surface, for the docked panels' rects.
#[derive(Default)]
struct Shells(Vec<(String, WeakEntity<ShellView>)>);
impl Global for Shells {}

#[derive(Default)]
struct Floating {
    /// `(plugin, output)` of every card floating now (not those leaving).
    live: Vec<(String, String)>,
    /// The floating surface of each output that has one.
    windows: Vec<(String, WindowHandle<FloatingView>)>,
    /// The keyboard bridge and the output it types into.
    bridge: Option<(String, AnyWindowHandle)>,
}
impl Global for Floating {}

/// Called by each output's shell surface when it is created.
pub(crate) fn register_shell(monitor: &str, view: WeakEntity<ShellView>, cx: &mut App) {
    let shells = cx.default_global::<Shells>();
    shells.0.retain(|(m, v)| m != monitor && v.upgrade().is_some());
    shells.0.push((monitor.to_string(), view));
}

fn shell_views(cx: &App) -> Vec<(String, Entity<ShellView>)> {
    cx.try_global::<Shells>().map(|s| s.0.iter().filter_map(|(m, v)| Some((m.clone(), v.upgrade()?))).collect()).unwrap_or_default()
}

fn shell_view(monitor: &str, cx: &App) -> Option<Entity<ShellView>> {
    shell_views(cx).into_iter().find(|(m, _)| m == monitor).map(|(_, v)| v)
}

/// The output `id` floats on, if it does.
pub fn floating_on(id: &str, cx: &App) -> Option<String> {
    cx.try_global::<Floating>()?.live.iter().find(|(p, _)| p == id).map(|(_, o)| o.clone())
}

/// Whether `id`'s panel is a floating card right now.
pub fn is_floating(id: &str, cx: &App) -> bool {
    floating_on(id, cx).is_some()
}

/// The plugins floating right now.
pub fn live_ids(cx: &App) -> Vec<String> {
    cx.try_global::<Floating>().map(|f| f.live.iter().map(|(p, _)| p.clone()).collect()).unwrap_or_default()
}

fn window(output: &str, cx: &App) -> Option<WindowHandle<FloatingView>> {
    cx.try_global::<Floating>()?.windows.iter().find(|(o, _)| o == output).map(|(_, h)| *h)
}

/// Forgets surfaces that went away with their output (unplugged) and the
/// cards that floated on them. Never called while a floating surface is
/// being updated (its handle would read as gone).
fn prune(cx: &mut App) {
    let Some(f) = cx.try_global::<Floating>() else { return };
    let gone: Vec<String> = f.windows.iter().filter(|(_, h)| h.read(cx).is_err()).map(|(o, _)| o.clone()).collect();
    if gone.is_empty() {
        return;
    }
    let f = cx.default_global::<Floating>();
    f.windows.retain(|(o, _)| !gone.contains(o));
    f.live.retain(|(_, o)| !gone.contains(o));
    let bridge = if f.bridge.as_ref().is_some_and(|(o, _)| gone.contains(o)) { f.bridge.take() } else { None };
    if let Some((_, h)) = bridge {
        let _ = h.update(cx, |_, window, _| window.remove_window());
    }
}

fn set_live(id: &str, output: Option<&str>, cx: &mut App) {
    let f = cx.default_global::<Floating>();
    f.live.retain(|(p, _)| p != id);
    if let Some(o) = output {
        f.live.push((id.to_string(), o.to_string()));
    }
    // The bar's chips tint for floating panels.
    for (_, v) in shell_views(cx) {
        v.update(cx, |_, cx| cx.notify());
    }
}

/// Carries out what the bar's panel slot asked for ([`dock::Effect`]).
pub fn apply(effect: Effect, cx: &mut App) {
    match effect {
        Effect::None => {}
        Effect::Float(id) => {
            if let Err(err) = float(&id, None, None, cx) {
                tracing::warn!("floating the pinned panel `{id}`: {err}");
            }
        }
        Effect::Raise(id) => raise(&id, cx),
        Effect::CloseFloating(id) => {
            close(&id, cx);
        }
    }
}

/// The floating surface of `output`, opened if needed.
fn ensure_window(output: &str, cx: &mut App) -> Result<WindowHandle<FloatingView>, String> {
    if let Some(h) = window(output, cx) {
        return Ok(h);
    }
    let geo = crate::outputs::geometries(cx).into_iter().find(|g| g.name == output).ok_or_else(|| tf("No output named {output}", &[("output", &output)]))?;
    let size = (geo.width as f32, geo.height as f32);
    let options = crate::plugins::dock::layer_options(
        SurfaceRole::Floating,
        Layer::Top,
        LayerAnchor::TOP | LayerAnchor::BOTTOM | LayerAnchor::LEFT | LayerAnchor::RIGHT,
        size,
        Some(IGNORE_ZONES),
        // Never the keyboard: see the module docs and [`bridge`].
        KeyboardInteractivity::None,
        crate::outputs::display_for(output, cx),
    );
    let name = output.to_string();
    let handle = cx.open_window(options, move |_, cx| cx.new(|cx| FloatingView::new(name, size, cx))).map_err(|e| format!("{e:#}"))?;
    cx.default_global::<Floating>().windows.push((output.to_string(), handle));
    Ok(handle)
}

/// The floating surface of `output` has no cards left: close it.
pub(crate) fn window_emptied(output: &str, cx: &mut App) {
    let Some(handle) = window(output, cx) else { return };
    if !handle.read(cx).map(|v| v.is_empty()).unwrap_or(true) {
        return;
    }
    if cx.try_global::<Floating>().and_then(|f| f.bridge.as_ref()).is_some_and(|(o, _)| o == output) {
        set_keyboard(output, false, cx);
    }
    cx.default_global::<Floating>().windows.retain(|(o, _)| o != output);
    let _ = handle.update(cx, |_, window, _| window.remove_window());
}

/// Floats `id`'s panel: on `output` (else where its panel is open, else the
/// focused output), at `at` (else where it last floated there, else lifted
/// off its docked panel or chip). A card already floating moves to `at`,
/// or comes to the front.
pub fn float(id: &str, output: Option<&str>, at: Option<(f32, f32)>, cx: &mut App) -> Result<Value, String> {
    if id == crate::plugins::main_menu::ID {
        return Err(t("The main menu opens from the bar only").to_string());
    }
    prune(cx);
    let cfg = config(cx).read(cx).snapshot();
    let (build, generation) = {
        let registry = cx.global::<PluginRegistry>();
        let factory = registry.get(id).ok_or_else(|| tf("No plugin named {plugin}", &[("plugin", &id)]))?;
        if !registry.is_enabled(id, &cfg.plugins) {
            return Err(tf("{plugin} is switched off (Studio → Plugins)", &[("plugin", &id)]));
        }
        (factory.build.clone(), factory.generation)
    };
    if let Some(on) = floating_on(id, cx) {
        let moved = window(&on, cx).and_then(|handle| {
            handle
                .update(cx, |v, _, cx| match at {
                    Some(at) => v.move_card(id, at, cx),
                    None => {
                        v.raise(id, cx);
                        None
                    }
                })
                .ok()
                .flatten()
        });
        if let Some(at) = moved {
            persist(id, &on, at, true, cx);
        }
        return Ok(describe(id, cx));
    }
    let outputs = crate::outputs::geometries(cx);
    let known = |o: &String| outputs.iter().any(|g| g.name == *o);
    let output = output
        .map(str::to_string)
        .filter(known)
        .or_else(|| shell_views(cx).into_iter().find(|(_, v)| v.read(cx).shown_rect(id).is_some()).map(|(m, _)| m))
        .or_else(|| crate::outputs::focused_monitor(cx).filter(known))
        .or_else(|| outputs.first().map(|g| g.name.clone()))
        .ok_or_else(|| t("No output to float the panel on").to_string())?;
    // What it morphs out of: its open docked panel, else its chip.
    let anchor = shell_view(&output, cx).and_then(|v| {
        let v = v.read(cx);
        v.shown_rect(id).map(|b| (Anchor::Panel(rect_of(b)), b, true)).or_else(|| v.chip_rect(id).map(|b| (Anchor::Chip(rect_of(b)), b, false)))
    });
    let remembered = mandara_config::floating::position(&cfg.panels.floating, id, &output);
    let place = Placement { anchor: anchor.map(|a| a.0), from: anchor.map(|a| (a.1, a.2)), at: at.or(remembered) };
    let handle = ensure_window(&output, cx)?;
    // Live before the bar lets go, so its panel vanishes instead of
    // folding back into the chip while the card lifts off it.
    set_live(id, Some(&output), cx);
    let built = handle.update(cx, |v, window, cx| v.add_card(id, generation, &build, place, window, cx)).map_err(|e| format!("{e:#}")).and_then(|r| r);
    let origin = match built {
        Ok(origin) => origin,
        Err(err) => {
            set_live(id, None, cx);
            window_emptied(&output, cx);
            return Err(err);
        }
    };
    let shell = shell(cx);
    shell.update(cx, |s, cx| {
        if s.panel.as_deref() == Some(id) {
            s.set_panel(None, cx);
        }
    });
    persist(id, &output, origin, true, cx);
    Ok(describe(id, cx))
}

/// Sends `id`'s card back into the bar: it springs into the place its
/// panel takes there and the bar's panel opens (pinned with `pinned`).
pub fn dock(id: &str, pinned: bool, cx: &mut App) -> Result<(), String> {
    prune(cx);
    let output = floating_on(id, cx).ok_or_else(|| tf("{plugin} is not floating", &[("plugin", &id)]))?;
    set_live(id, None, cx);
    let Some(handle) = window(&output, cx) else { return Ok(()) };
    let cfg = config(cx).read(cx).snapshot();
    let want = handle.read(cx).ok().and_then(|v| v.wanted_size(id));
    let target = want.and_then(|s| shell_view(&output, cx).and_then(|v| v.read(cx).dock_target(&cfg, id, s)));
    let _ = handle.update(cx, |v, window, cx| v.begin_dock(id, target, pinned, window, cx));
    forget(id, &output, cx);
    Ok(())
}

/// A docking card landed: the bar's panel takes over (already expanded
/// where the card landed when `snap`).
pub(crate) fn docked(id: &str, output: &str, pinned: bool, snap: bool, cx: &mut App) {
    let shell = shell(cx);
    // The panel opens on the bar the card docked into.
    crate::state::on_bar(output, cx, |cx| {
        shell.update(cx, |s, cx| {
            if pinned {
                s.pin_panel(id, cx);
            } else {
                s.set_panel(Some(id.to_string()), cx);
            }
            if snap {
                s.snap_open = Some((id.to_string(), output.to_string()));
            }
        })
    });
}

/// Closes `id`'s floating card (its place stays remembered). Returns
/// whether it was floating.
pub fn close(id: &str, cx: &mut App) -> bool {
    prune(cx);
    let Some(output) = floating_on(id, cx) else { return false };
    set_live(id, None, cx);
    if let Some(handle) = window(&output, cx) {
        let _ = handle.update(cx, |v, window, cx| v.begin_close(id, window, cx));
    }
    forget(id, &output, cx);
    true
}

/// Brings `id`'s card to the front with a little pop.
pub fn raise(id: &str, cx: &mut App) {
    if let Some(handle) = floating_on(id, cx).and_then(|o| window(&o, cx)) {
        let _ = handle.update(cx, |v, _, cx| v.raise(id, cx));
    }
}

/// `{plugin, output, x, y, width, height}` of `id`'s card, or `{plugin}`.
fn describe(id: &str, cx: &App) -> Value {
    floating_on(id, cx)
        .and_then(|o| window(&o, cx))
        .and_then(|h| h.read(cx).ok().and_then(|v| v.card_json(id)))
        .unwrap_or_else(|| json!({ "plugin": id, "floating": false }))
}

/// `panel floating`: every card floating now, and every remembered place.
pub fn list(cx: &App) -> Value {
    let cards: Vec<Value> = live_ids(cx).iter().map(|id| describe(id, cx)).collect();
    let cfg = config(cx).read(cx).snapshot();
    let remembered: Vec<Value> = cfg.panels.floating.iter().map(|e| json!({ "plugin": e.plugin, "output": e.output, "x": e.x, "y": e.y, "open": e.open })).collect();
    json!({ "floating": cards, "remembered": remembered, "remember_floating": cfg.panels.remember_floating })
}

/// Remembers where `id`'s card sits on `output`.
pub(crate) fn persist(id: &str, output: &str, at: (f32, f32), open: bool, cx: &mut App) {
    update_config(cx, |c| {
        mandara_config::floating::remember(&mut c.panels.floating, id, output, at, open);
    });
}

/// `id` no longer floats on `output` (its place stays remembered).
fn forget(id: &str, output: &str, cx: &mut App) {
    update_config(cx, |c| {
        mandara_config::floating::close(&mut c.panels.floating, id, output);
    });
}

// ── Keyboard bridge ──────────────────────────────────────────────────────

/// Opens (or closes) the keyboard bridge into `output`'s floating surface.
pub(crate) fn set_keyboard(output: &str, wants: bool, cx: &mut App) {
    let current = cx.try_global::<Floating>().and_then(|f| f.bridge.clone());
    match (current, wants) {
        (Some((o, _)), true) if o == output => {}
        (current, wants) => {
            if let Some((o, h)) = current {
                cx.default_global::<Floating>().bridge = None;
                let _ = h.update(cx, |_, window, _| window.remove_window());
                if o != output {
                    release(&o, cx);
                }
            }
            if wants && let Some(h) = bridge::open(output, cx) {
                cx.default_global::<Floating>().bridge = Some((output.to_string(), h));
            }
        }
    }
}

/// Stops typing into `output`'s cards (focus back on the card).
fn release(output: &str, cx: &mut App) {
    if let Some(handle) = window(output, cx) {
        let _ = handle.update(cx, |v, window, cx| v.release_keyboard(window, cx));
    }
}

/// The bridge lost the keyboard (another window or surface took it).
pub(crate) fn keyboard_lost(output: &str, cx: &mut App) {
    release(output, cx);
    set_keyboard(output, false, cx);
}

/// A key typed while the bridge has the keyboard: dispatched to the
/// focused element of `output`'s floating surface. Esc stops typing.
pub(crate) fn forward_key(output: &str, keystroke: Keystroke, cx: &mut App) {
    let Some(handle) = window(output, cx) else {
        set_keyboard(output, false, cx);
        return;
    };
    let escape = keystroke.key == "escape";
    // Dispatched without holding the surface's view: the focused element's
    // handlers (a plugin's text field) run as they would for a real key.
    let _ = AnyWindowHandle::from(handle).update(cx, |_, window, cx| {
        window.dispatch_keystroke(keystroke, cx);
    });
    let _ = handle.update(cx, |v, window, cx| {
        if escape {
            v.release_keyboard(window, cx);
        } else {
            v.sync_keyboard(window, cx);
        }
    });
}

// ── Startup ──────────────────────────────────────────────────────────────

/// Brings back the cards that floated when the shell last stopped (with
/// `panels.remember_floating`), and stops typing into a card when another
/// window takes focus. Call once the shell surfaces are open.
pub fn init(cx: &mut App) {
    let hub = crate::hub::hub(cx);
    let mut last = hub.read(cx).focused.as_ref().map(|c| c.id);
    cx.observe(&hub, move |hub, cx| {
        let Some(focus) = hub.read(cx).focused.as_ref().map(|c| c.id) else { return };
        if Some(focus) == last {
            return;
        }
        last = Some(focus);
        if let Some((output, _)) = cx.try_global::<Floating>().and_then(|f| f.bridge.clone()) {
            keyboard_lost(&output, cx);
        }
    })
    .detach();
    cx.defer(restore);
}

fn restore(cx: &mut App) {
    let cfg = config(cx).read(cx).snapshot();
    if !cfg.panels.remember_floating {
        return;
    }
    let names: Vec<String> = crate::outputs::geometries(cx).into_iter().map(|g| g.name).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let back: Vec<(String, String)> = mandara_config::floating::to_restore(&cfg.panels.floating, &names).into_iter().map(|e| (e.plugin.clone(), e.output.clone())).collect();
    for (plugin, output) in back {
        if let Err(err) = float(&plugin, Some(&output), None, cx) {
            tracing::info!("not restoring the floating `{plugin}` panel: {err}");
        }
    }
}

fn rect_of(b: Bounds<Pixels>) -> crate::dismiss::Rect {
    crate::dismiss::Rect::new(f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height))
}

/// `r` as GPUI bounds.
fn bounds_of(r: crate::dismiss::Rect) -> Bounds<Pixels> {
    Bounds { origin: gpui::point(gpui::px(r.x), gpui::px(r.y)), size: gpui::size(gpui::px(r.w), gpui::px(r.h)) }
}
