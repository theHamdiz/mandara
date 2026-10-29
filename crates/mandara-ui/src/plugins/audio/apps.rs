//! Per-app sound in the sound panel (and a compact list in the sidebar
//! card): one row per app playing sound — its icon and desktop-entry name,
//! what it plays, a volume slider, a mute pill, an output picker when there
//! is more than one output — with several streams of one app folded into
//! one row whose slider sets them all.
//!
//! The list follows the sound server's stream events only while a view that
//! shows it is on screen ([`AppsTap`]); rows spring in and out
//! ([`Presence`]); a dragged slider or clicked mute shows your value until
//! the server has caught up ([`AppsTap::volume`]), so the echo of an older
//! command never makes the slider jump back mid-drag.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui::{AnyElement, App, FontWeight, IntoElement, ParentElement, SharedString, Styled, Window, div, prelude::FluentBuilder as _, px};
use mandara_core::{SpringConfig, ToGpui};
use mandara_i18n::{t, tf};
use mandara_services::audio::{AudioCommand, Device, Stream};
use mandara_theme::MandaraPalette;

use super::super::panel_fx::{PILL_H_PX, Readout, TINT_FILL, TINT_HOVER, ViewSet, alpha, big_slider, pill_button, readable};
use super::{send, volume_icon, volume_text};
use crate::bidi::Bidi as _;
use crate::fonts::tabular_nums;
use crate::hub::hub;
use crate::kit::text;

/// Volume sliders snap to hundredths (one displayed percent).
const VOLUME_STEPS: f32 = 100.0;
/// A slider value you set wins over the server's for this long after your
/// last change: long enough for the server to echo it, short enough that a
/// change made elsewhere shows soon after.
const ECHO_HOLD: Duration = Duration::from_millis(700);
/// How often the watchdog checks whether a view still shows the list.
const WATCHDOG: Duration = Duration::from_millis(700);
/// No view rendered the list for this long: stop following the server.
const IDLE: Duration = Duration::from_millis(1600);
/// Re-render a view this often while it shows the list (keeps the watchdog
/// fed and lets expired holds fall back to the server value).
pub(super) const KEEPALIVE: Duration = Duration::from_millis(500);

/// Row enter / leave spring (critically damped: rows never bounce past
/// their height), and when it counts as settled.
const PRESENCE_STIFFNESS: f32 = 320.0;
const PRESENCE_SETTLE: f32 = 0.004;
/// A leaving row below this presence is dropped.
const PRESENCE_GONE: f32 = 0.01;

/// What the list shows: grouped apps plus the outputs to pick from.
#[derive(Clone, Debug, Default)]
pub(super) struct Snapshot {
    pub groups: Vec<AppGroup>,
    pub outputs: Vec<Device>,
}

/// One app's streams, folded into a row.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct AppGroup {
    /// Identity across refreshes (app id or name, lowercase).
    pub key: String,
    /// The desktop entry's name when one matches, else the stream's.
    pub name: SharedString,
    /// What [`crate::plugins::workspaces::app_icon`] resolves.
    pub icon_key: String,
    /// The title of what it plays (empty when the app sets none).
    pub media: SharedString,
    pub indices: Vec<u32>,
    /// The loudest stream's volume (the slider sets them all to one level).
    pub volume: f32,
    /// Every stream is muted by you (see [`Stream::own_muted`]).
    pub muted: bool,
    /// Every stream is paused.
    pub corked: bool,
    /// The output of the first stream.
    pub sink: u32,
}

impl AppGroup {
    pub fn count(&self) -> usize {
        self.indices.len()
    }
}

/// The key that folds streams of one app together: its app id, else its
/// name. Never the executable alone: Electron apps, Python scripts and
/// players launched through wrappers share one.
fn group_key(s: &Stream) -> String {
    s.app_id.as_deref().filter(|k| !k.is_empty()).unwrap_or(&s.app).to_lowercase()
}

/// Folds streams into one group per app, in order of the oldest stream so
/// rows keep their places and new apps join at the end. `identify` gives
/// an app's display name and icon key.
pub(super) fn group_streams(streams: &[Stream], identify: impl Fn(&Stream) -> (String, String)) -> Vec<AppGroup> {
    let mut sorted: Vec<&Stream> = streams.iter().collect();
    sorted.sort_by_key(|s| s.index);
    let mut groups: Vec<AppGroup> = Vec::new();
    for s in sorted {
        let key = group_key(s);
        match groups.iter_mut().find(|g| g.key == key) {
            Some(g) => {
                g.indices.push(s.index);
                g.volume = g.volume.max(s.volume);
                g.muted &= s.own_muted;
                // A playing stream's title wins over a paused one's.
                if !s.media.is_empty() && (g.media.is_empty() || (g.corked && !s.corked)) {
                    g.media = s.media.clone().into();
                }
                g.corked &= s.corked;
            }
            None => {
                let (name, icon_key) = identify(s);
                groups.push(AppGroup {
                    key,
                    name: name.into(),
                    icon_key,
                    media: s.media.clone().into(),
                    indices: vec![s.index],
                    volume: s.volume,
                    muted: s.own_muted,
                    corked: s.corked,
                    sink: s.sink,
                });
            }
        }
    }
    groups
}

/// An app's name from its desktop entry and the icon key that resolves in
/// the icon theme (the dock's lookup), falling back to the stream's own.
fn identify(s: &Stream) -> (String, String) {
    let lower = s.app.to_lowercase();
    let ids: Vec<&str> = [s.app_id.as_deref(), s.binary.as_deref(), Some(lower.as_str())].into_iter().flatten().filter(|k| !k.is_empty()).collect();
    let name = ids.iter().find_map(|k| crate::apps::name_for_app_id(k)).unwrap_or_else(|| s.app.clone());
    let icon_key = s
        .icon
        .as_deref()
        .into_iter()
        .chain(ids.iter().copied())
        .chain(std::iter::once(s.app.as_str()))
        .find(|k| crate::apps::icon_for_app_id(k, false).is_some())
        // No icon: the monogram is drawn from the name.
        .map_or_else(|| name.clone(), str::to_string);
    (name, icon_key)
}

/// Reads and groups the mixer (blocking; off the UI thread).
fn read_snapshot() -> Snapshot {
    match mandara_services::audio::mixer() {
        Ok(m) => Snapshot { groups: group_streams(&m.streams, identify), outputs: m.outputs },
        Err(err) => {
            tracing::debug!("audio apps: {err}");
            Snapshot::default()
        }
    }
}

/// A slider position (`0..=1`) as a volume up to `limit`, snapped to whole
/// percents.
pub(super) fn slider_volume(frac: f32, limit: f32) -> f32 {
    ((frac.clamp(0.0, 1.0) * limit * VOLUME_STEPS).round() / VOLUME_STEPS).clamp(0.0, limit)
}

/// A volume as a slider position (volumes above `limit` pin the end).
pub(super) fn slider_frac(volume: f32, limit: f32) -> f32 {
    if limit > 0.0 { (volume / limit).clamp(0.0, 1.0) } else { 0.0 }
}

/// The value to show: yours while you changed it within [`ECHO_HOLD`],
/// else the server's.
pub(super) fn shown<T: Copy>(server: T, held: Option<&(T, Instant)>, now: Instant) -> T {
    match held {
        Some((value, at)) if now.saturating_duration_since(*at) < ECHO_HOLD => *value,
        _ => server,
    }
}

/// The next visible output after `current` (wrapping), `None` when there is
/// nothing else to switch to.
pub(super) fn next_output<'a>(outputs: &[&'a Device], current: u32) -> Option<&'a Device> {
    if outputs.len() < 2 {
        return None;
    }
    let next = match outputs.iter().position(|d| d.index == current) {
        Some(i) => outputs[(i + 1) % outputs.len()],
        None => outputs[0],
    };
    (next.index != current).then_some(next)
}

/// Follows the server's app streams while a view shows them: started by
/// [`Self::snapshot`] from a render, stopped by a watchdog once no view has
/// rendered for [`IDLE`] (the panel / sidebar closed).
#[derive(Default)]
pub(super) struct AppsTap {
    snapshot: Rc<Snapshot>,
    alive: Option<Arc<AtomicBool>>,
    last_render: Option<Instant>,
    views: ViewSet,
    held_volume: HashMap<String, (f32, Instant)>,
    held_mute: HashMap<String, (bool, Instant)>,
}

impl AppsTap {
    /// The one tap of this (UI) thread: every bar's sound panel and sidebar
    /// card share it, so at most one server watcher runs.
    pub fn shared() -> Rc<RefCell<Self>> {
        thread_local! {
            static TAP: Rc<RefCell<AppsTap>> = Rc::default();
        }
        TAP.with(Rc::clone)
    }

    /// The latest list; registers the rendering view and starts following
    /// the server when nothing does yet.
    pub fn snapshot(this: &Rc<RefCell<Self>>, window: &Window, cx: &mut App) -> Rc<Snapshot> {
        let mut s = this.borrow_mut();
        s.views.track(window);
        s.last_render = Some(Instant::now());
        if s.alive.is_none() {
            s.start(this.clone(), cx);
        }
        s.snapshot.clone()
    }

    fn start(&mut self, this: Rc<RefCell<Self>>, cx: &mut App) {
        let alive = Arc::new(AtomicBool::new(true));
        let (tx, rx) = async_channel::bounded::<()>(1);
        let _ = tx.force_send(()); // read right away
        {
            let (alive_check, alive_change, tx) = (alive.clone(), alive.clone(), tx.clone());
            mandara_services::audio::watch_mixer(move || alive_check.load(Ordering::Relaxed), move || {
                let _ = tx.force_send(());
                alive_change.load(Ordering::Relaxed)
            });
        }
        self.alive = Some(alive.clone());
        POKE.with(|p| *p.borrow_mut() = Some(tx));
        // Re-read on every poke; ends once the watcher and the tap drop
        // their senders.
        let reader = this.clone();
        cx.spawn(async move |cx| {
            while rx.recv().await.is_ok() {
                let snap = cx.background_executor().spawn(async { read_snapshot() }).await;
                let views = {
                    let mut s = reader.borrow_mut();
                    if s.alive.is_none() {
                        break;
                    }
                    let now = Instant::now();
                    s.held_volume.retain(|_, (_, at)| now.saturating_duration_since(*at) < ECHO_HOLD);
                    s.held_mute.retain(|_, (_, at)| now.saturating_duration_since(*at) < ECHO_HOLD);
                    if *s.snapshot.groups != snap.groups || *s.snapshot.outputs != snap.outputs {
                        s.snapshot = Rc::new(snap);
                        Some(s.views.clone())
                    } else {
                        None
                    }
                };
                if let Some(views) = views {
                    views.notify_async(cx);
                }
            }
        })
        .detach();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(WATCHDOG).await;
                let mut s = this.borrow_mut();
                if s.last_render.is_none_or(|t| t.elapsed() > IDLE) {
                    alive.store(false, Ordering::Relaxed);
                    s.alive = None;
                    POKE.with(|p| *p.borrow_mut() = None);
                    break;
                }
            }
        })
        .detach();
    }

    /// The volume to draw for `g`.
    pub fn volume(&self, g: &AppGroup) -> f32 {
        shown(g.volume, self.held_volume.get(&g.key), Instant::now())
    }

    /// The mute to draw for `g`.
    pub fn muted(&self, g: &AppGroup) -> bool {
        shown(g.muted, self.held_mute.get(&g.key), Instant::now())
    }

    /// Holds a dragged volume; whether it differs from what was last sent
    /// (unchanged slider positions send nothing).
    fn hold_volume(&mut self, key: &str, volume: f32) -> bool {
        let now = Instant::now();
        let fresh = self.held_volume.get(key).is_some_and(|(v, at)| *v == volume && now.saturating_duration_since(*at) < ECHO_HOLD);
        self.held_volume.insert(key.to_string(), (volume, now));
        !fresh
    }
}

/// Fades and folds rows in and out as apps start and stop (per view).
#[derive(Default)]
pub(super) struct Presence {
    rows: Vec<PresenceRow>,
}

struct PresenceRow {
    group: AppGroup,
    live: bool,
    readout: Readout,
}

impl Presence {
    /// This frame's rows with their presence (`0..=1`): live ones in list
    /// order, leaving ones where they were until they have folded away.
    pub fn frame(&mut self, groups: &[AppGroup], window: &Window) -> Vec<(AppGroup, f32)> {
        for row in &mut self.rows {
            row.live = false;
        }
        for (i, g) in groups.iter().enumerate() {
            match self.rows.iter_mut().find(|r| r.group.key == g.key) {
                Some(row) => {
                    row.group = g.clone();
                    row.live = true;
                }
                None => {
                    let at = self.rows.iter().position(|r| groups[i..].iter().skip(1).any(|n| n.key == r.group.key)).unwrap_or(self.rows.len());
                    self.rows.insert(at, PresenceRow { group: g.clone(), live: true, readout: Readout::new(SpringConfig::critical(PRESENCE_STIFFNESS), 0.0, PRESENCE_SETTLE) });
                }
            }
        }
        let mut out = Vec::with_capacity(self.rows.len());
        self.rows.retain_mut(|row| {
            let a = row.readout.show(if row.live { 1.0 } else { 0.0 }, window).clamp(0.0, 1.0);
            if !row.live && a <= PRESENCE_GONE {
                return false;
            }
            out.push((row.group.clone(), a));
            true
        });
        out
    }
}

// ── Rows ─────────────────────────────────────────────────────────────────

/// App row geometry: head (icon, name, title, percent, mute), the gap above
/// the slider, the slider (the output card's), and the icon size.
pub(super) const ROW_HEAD_H: f32 = 34.0;
pub(super) const ROW_SLIDER_GAP: f32 = 8.0;
const SLIDER_H: f32 = 40.0;
pub(super) const ROW_H: f32 = ROW_HEAD_H + ROW_SLIDER_GAP + SLIDER_H;
const APP_ICON_PX: f32 = 28.0;
const ROW_HEAD_GAP_PX: f32 = 8.0;
const NAME_TEXT_PX: f32 = text::BODY_S + 0.5;
const COUNT_GAP_PX: f32 = 6.0;
const PERCENT_TEXT_PX: f32 = text::BODY;
/// The output picker: a round icon button the height of a pill.
const OUTPUT_ICON_PX: f32 = 13.0;
/// Joins a row's title and the output it plays on.
const SUBTITLE_SEPARATOR: &str = " · ";
/// Paused apps are drawn at this opacity.
const PAUSED_OPACITY: f32 = 0.6;
/// Above 100 % the percentage turns amber (boosted).
const UNITY_VOLUME: f32 = 1.0;

/// Everything a row's controls need.
pub(super) struct RowCx<'a> {
    pub tap: &'a Rc<RefCell<AppsTap>>,
    pub limit: f32,
    pub all_muted: bool,
    /// Outputs you can send an app to (hidden ones left out).
    pub outputs: &'a [&'a Device],
    /// The default output (apps elsewhere name theirs).
    pub default_sink: Option<u32>,
    pub palette: &'a MandaraPalette,
    pub id_prefix: &'static str,
}

/// An app's name with its stream count when it has several.
fn name_line(g: &AppGroup, palette: &MandaraPalette) -> impl IntoElement {
    div()
        .row()
        .items_center()
        .gap(px(COUNT_GAP_PX))
        .min_w(px(0.0))
        .child(div().min_w(px(0.0)).text_size(px(NAME_TEXT_PX)).font_weight(FontWeight::MEDIUM).truncate().child(g.name.clone()))
        .when(g.count() > 1, |el| {
            el.child(div().flex_none().text_size(px(text::LABEL)).font_features(tabular_nums()).text_color(palette.on_surface_variant.hsla()).child(tf("{n} streams", &[("n", &g.count())])))
        })
}

/// The slider for `g`: sets every stream of the app; holds your value.
pub(super) fn app_slider(g: &AppGroup, rc: &RowCx<'_>) -> gpui::Stateful<gpui::Div> {
    let tap = rc.tap.borrow();
    let (volume, muted) = (tap.volume(g), tap.muted(g));
    drop(tap);
    let (tap, key, indices, limit) = (rc.tap.clone(), g.key.clone(), g.indices.clone(), rc.limit);
    big_slider(
        SharedString::from(format!("{}-vol-{}", rc.id_prefix, g.key)),
        slider_frac(volume, limit),
        volume_icon(volume, muted || rc.all_muted),
        muted || rc.all_muted,
        rc.palette,
        move |frac, _, cx| {
            let v = slider_volume(frac, limit);
            let (changed, views) = {
                let mut s = tap.borrow_mut();
                (s.hold_volume(&key, v), s.views.clone())
            };
            if changed {
                send(cx, AudioCommand::SetStreamVolume(indices.clone(), v));
                views.notify(cx);
            }
        },
    )
}

thread_local! {
    /// The running tap's re-read trigger (for changes no server event
    /// reports, e.g. a mute recorded while global mute is on).
    static POKE: RefCell<Option<async_channel::Sender<()>>> = const { RefCell::new(None) };
}

/// Sends `command` to the audio service and, once it is done, re-reads the
/// app list if a view shows it: a mute recorded for global mute's release
/// changes nothing on the server, so no server event would refresh it.
/// Resolves with the service's reply.
pub(super) fn request_then_refresh(command: AudioCommand, cx: &mut App) -> Result<async_channel::Receiver<Result<String, String>>, String> {
    let service = hub(cx).read(cx).audio_service.clone().ok_or("audio service unavailable")?;
    let reply = service.request(command);
    let (tx, rx) = async_channel::bounded(1);
    cx.spawn(async move |cx| {
        let result = reply.recv().await.unwrap_or_else(|_| Err("audio service stopped".into()));
        cx.update(|_| POKE.with(|p| p.borrow().as_ref().map(|p| p.force_send(()))));
        let _ = tx.send(result).await;
    })
    .detach();
    Ok(rx)
}

/// Mutes / unmutes every stream of the app (while global mute is on, the
/// choice is kept for its release).
fn toggle_mute(tap: &Rc<RefCell<AppsTap>>, key: &str, indices: Vec<u32>, mute: bool, cx: &mut App) {
    let views = {
        let mut s = tap.borrow_mut();
        s.held_mute.insert(key.to_string(), (mute, Instant::now()));
        s.views.clone()
    };
    views.notify(cx);
    if let Err(err) = request_then_refresh(AudioCommand::SetStreamMute(indices, mute), cx) {
        tracing::debug!("audio apps: {err}");
    }
}

/// The mute pill of a row.
pub(super) fn mute_pill(g: &AppGroup, rc: &RowCx<'_>) -> AnyElement {
    let muted = rc.tap.borrow().muted(g);
    let (tap, key, indices) = (rc.tap.clone(), g.key.clone(), g.indices.clone());
    let p = rc.palette;
    pill_button(
        SharedString::from(format!("{}-mute-{}", rc.id_prefix, g.key)),
        Some(if muted { "volume-mute" } else { "volume-high" }),
        if muted { t("Muted") } else { t("Mute") },
        if muted { p.urgent_coral } else { p.on_surface_variant },
        p,
        move |_, cx| toggle_mute(&tap, &key, indices.clone(), !muted, cx),
    )
    .into_any_element()
}

/// The percentage, tabular; amber when boosted past 100 %.
pub(super) fn percent(volume: f32, muted: bool, palette: &MandaraPalette) -> impl IntoElement {
    let color = if muted {
        palette.on_surface_variant
    } else if volume > UNITY_VOLUME {
        readable(palette.warning_amber, palette)
    } else {
        palette.on_surface
    };
    div().flex_none().font_features(tabular_nums()).text_size(px(PERCENT_TEXT_PX)).font_weight(FontWeight::MEDIUM).text_color(color.hsla()).child(volume_text(volume))
}

/// The output picker: a click sends the app to the next output (shown only
/// with more than one).
fn output_button(g: &AppGroup, rc: &RowCx<'_>) -> Option<AnyElement> {
    use gpui::{InteractiveElement as _, StatefulInteractiveElement as _};
    let next = next_output(rc.outputs, g.sink)?;
    let (indices, target) = (g.indices.clone(), next.name.clone());
    let p = rc.palette;
    let tint = p.on_surface_variant;
    let hover = tint.with_alpha(alpha(TINT_HOVER, p));
    Some(
        div()
            .id(SharedString::from(format!("{}-out-{}", rc.id_prefix, g.key)))
            .size(px(PILL_H_PX))
            .flex_none()
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(tint.with_alpha(alpha(TINT_FILL, p)).hsla())
            .hover(move |st| st.bg(hover.hsla()))
            .child(crate::icons::icon("headphones", OUTPUT_ICON_PX, tint))
            .on_click(move |_, _, cx| send(cx, AudioCommand::MoveStreams(indices.clone(), target.clone())))
            .into_any_element(),
    )
}

/// What a row says under the name: the title (or "Paused"), plus the
/// output when it isn't the default one.
fn subtitle(g: &AppGroup, rc: &RowCx<'_>, cx: &App) -> SharedString {
    let title: SharedString = if g.corked { t("Paused").into() } else { g.media.clone() };
    let elsewhere = rc.default_sink.is_some_and(|d| d != g.sink);
    let Some(device) = rc.outputs.iter().find(|d| d.index == g.sink).filter(|_| elsewhere) else { return title };
    let cfg = crate::state::config(cx).read(cx).get();
    let label = cfg.audio.device_label(&device.name, &device.description);
    if title.is_empty() { label.to_string().into() } else { format!("{title}{SUBTITLE_SEPARATOR}{label}").into() }
}

/// A full app row for the sound panel.
pub(super) fn panel_row(g: &AppGroup, rc: &RowCx<'_>, cx: &App) -> AnyElement {
    let p = rc.palette;
    let (volume, muted) = {
        let tap = rc.tap.borrow();
        (tap.volume(g), tap.muted(g))
    };
    let subtitle = subtitle(g, rc, cx);
    let output = output_button(g, rc);
    div()
        .h(px(ROW_H))
        .flex()
        .flex_col()
        .when(g.corked, |el| el.opacity(PAUSED_OPACITY))
        .child(
            div()
                .h(px(ROW_HEAD_H))
                .row()
                .items_center()
                .gap(px(ROW_HEAD_GAP_PX))
                .child(crate::plugins::workspaces::app_icon(&g.icon_key, APP_ICON_PX, p.on_surface, false, p))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .child(name_line(g, p))
                        .when(!subtitle.is_empty(), |el| el.child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).truncate().child(subtitle))),
                )
                .child(percent(volume, muted || rc.all_muted, p))
                .when_some(output, |el, o| el.child(o))
                .child(mute_pill(g, rc)),
        )
        .child(div().h(px(ROW_SLIDER_GAP)))
        .child(app_slider(g, rc))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(index: u32, app: &str, volume: f32) -> Stream {
        Stream { index, app: app.into(), binary: Some(app.to_lowercase()), volume, ..Default::default() }
    }

    fn ident(s: &Stream) -> (String, String) {
        (s.app.clone(), s.app.to_lowercase())
    }

    #[test]
    fn streams_of_one_app_fold_into_one_row() {
        let mut a = stream(12, "Firefox", 0.4);
        a.media = "Song".into();
        a.corked = true;
        let mut b = stream(15, "Firefox", 0.9);
        b.media = "Video".into();
        b.own_muted = true;
        let c = stream(13, "mpv", 1.2);
        let groups = group_streams(&[b.clone(), c, a], ident);
        assert_eq!(groups.iter().map(|g| g.name.as_ref()).collect::<Vec<_>>(), ["Firefox", "mpv"], "ordered by the oldest stream");
        let ff = &groups[0];
        assert_eq!(ff.indices, [12, 15]);
        assert_eq!(ff.count(), 2);
        assert!((ff.volume - 0.9).abs() < 1e-6, "the loudest stream");
        assert!(!ff.muted, "one stream still plays");
        assert!(!ff.corked, "one stream still plays");
        assert_eq!(ff.media.as_ref(), "Video", "the playing stream's title wins");
        // Both muted by you: the row is muted.
        let mut a2 = stream(12, "Firefox", 0.4);
        a2.own_muted = true;
        assert!(group_streams(&[a2, b], ident)[0].muted);
    }

    #[test]
    fn grouping_prefers_app_id_then_name_never_the_binary() {
        let mut a = stream(1, "Chromium", 0.5);
        a.app_id = Some("org.chromium.Chromium".into());
        let mut b = stream(2, "Chromium (WebRTC)", 0.5);
        b.app_id = Some("org.chromium.Chromium".into());
        let c = stream(3, "Spotify", 0.5);
        let d = stream(4, "spotify", 0.5);
        // Two Electron apps share one executable.
        let mut e = stream(5, "Signal", 0.5);
        e.binary = Some("electron".into());
        let mut f = stream(6, "Element", 0.5);
        f.binary = Some("electron".into());
        let groups = group_streams(&[a, b, c, d, e, f], ident);
        assert_eq!(groups.len(), 4);
        assert_eq!(groups[0].indices, [1, 2]);
        assert_eq!(groups[1].indices, [3, 4], "names compare case-insensitively");
        assert!(group_streams(&[], ident).is_empty());
    }

    #[test]
    fn slider_maps_to_whole_percents_up_to_the_limit() {
        assert_eq!(slider_volume(0.5, 1.0), 0.5);
        assert_eq!(slider_volume(0.333, 1.0), 0.33);
        assert_eq!(slider_volume(1.0, 1.5), 1.5);
        assert_eq!(slider_volume(2.0, 1.5), 1.5, "clamped");
        assert_eq!(slider_volume(-1.0, 1.5), 0.0);
        assert_eq!(slider_frac(0.75, 1.5), 0.5);
        assert_eq!(slider_frac(1.4, 1.0), 1.0, "a boosted app pins the end at a 100 % limit");
        assert_eq!(slider_frac(0.5, 0.0), 0.0);
        for limit in [1.0_f32, 1.5] {
            for step in 0..=100 {
                let v = slider_volume(step as f32 / 100.0, limit);
                assert!((slider_volume(slider_frac(v, limit), limit) - v).abs() < 1e-6, "round trip at {v}");
            }
        }
    }

    #[test]
    fn your_value_wins_until_the_hold_expires() {
        let now = Instant::now();
        assert_eq!(shown(0.2, None, now), 0.2);
        assert_eq!(shown(0.2, Some(&(0.8, now)), now + ECHO_HOLD / 2), 0.8, "a stale echo doesn't move the slider");
        assert_eq!(shown(0.2, Some(&(0.8, now)), now + ECHO_HOLD), 0.2);
        let mut tap = AppsTap::default();
        assert!(tap.hold_volume("ff", 0.5));
        assert!(!tap.hold_volume("ff", 0.5), "same position: nothing to send");
        assert!(tap.hold_volume("ff", 0.51));
        let g = AppGroup { key: "ff".into(), volume: 0.1, ..Default::default() };
        assert_eq!(tap.volume(&g), 0.51);
    }

    #[test]
    fn output_picker_cycles_visible_outputs() {
        let d = |index: u32| Device { index, name: format!("sink{index}"), ..Default::default() };
        let (a, b, c) = (d(1), d(2), d(3));
        let list = [&a, &b, &c];
        assert_eq!(next_output(&list, 1).map(|d| d.index), Some(2));
        assert_eq!(next_output(&list, 3).map(|d| d.index), Some(1), "wraps");
        assert_eq!(next_output(&list, 9).map(|d| d.index), Some(1), "on a hidden output: the first visible one");
        assert!(next_output(&[&a], 1).is_none(), "one output: no picker");
    }
}
