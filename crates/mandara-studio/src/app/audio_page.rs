//! Studio → Audio & media: per-app volume mixer, output / input device
//! pickers (PulseAudio protocol, pure Rust; works on PipeWire) and the
//! media panel's visualizer style with a live preview, plus the shell's
//! global "Mute all sound" switch. Devices can be
//! renamed and hidden (`[audio]`), apps sent to their own output
//! (`[[audio.routes]]`), media players ignored (`[media]`), and shell
//! sounds (`[sounds]`) previewed.
//!
//! The mixer subscribes to stream / device events only while this page is
//! open; the preview taps the shared spectrum capture only while sound is
//! playing and the page is visible.

use mandara_ui::bidi::Bidi as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui::{AnyElement, ClickEvent, StyledImage as _, Context, FontWeight, Global, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_core::ToGpui;
use mandara_i18n::{t, tf};
use mandara_services::audio::{self, Device, Mixer, Stream};
use mandara_theme::MandaraPalette;
use mandara_ui::icons::icon;
use mandara_ui::plugins::visualizer;

use super::StudioView;
use super::compositor_page::{badge, card, group_title, heading};
use crate::registry::{self, Section};
use crate::tokens::*;

#[derive(Default)]
struct AudioStudio {
    mixer: Option<Result<Mixer, String>>,
    /// The watcher + refresh loop run while this is set and true.
    alive: Option<Arc<AtomicBool>>,
    tap: Option<mandara_services::spectrum::Spectrum>,
    smoothed: Vec<f32>,
    /// Volume being dragged per stream (shown until the server confirms).
    dragging: Option<(u32, f32)>,
    /// The device (server name) whose label is being edited.
    renaming: Option<String>,
}
impl Global for AudioStudio {}

fn state(cx: &mut gpui::App) -> &mut AudioStudio {
    cx.default_global::<AudioStudio>()
}

/// Runs a mixer call off the UI thread; failures are logged.
fn run(f: impl FnOnce() -> Result<(), String> + Send + 'static) {
    std::thread::spawn(move || {
        if let Err(err) = f() {
            tracing::warn!("audio: {err}");
        }
    });
}

/// Preview spectrum when nothing plays: two Gaussian bumps (height,
/// center, width on the 0–1 frequency axis) over a floor.
const SAMPLE_BASS: (f32, f32, f32) = (0.75, 0.18, 0.16);
const SAMPLE_MIDS: (f32, f32, f32) = (0.5, 0.55, 0.2);
const SAMPLE_FLOOR: f32 = 0.12;
/// Size of an app's icon in the mixer.
const APP_ICON_PX: f32 = 30.0;
/// Volume sliders snap to hundredths.
const VOLUME_STEPS: f32 = 100.0;
/// The media panel's visualizer width and its horizontal padding, so the
/// preview is true to size.
const MEDIA_PANEL_W_PX: f32 = 384.0;
const MEDIA_PANEL_PAD_PX: f32 = 32.0;
/// Height of the visualizer preview.
const VISUALIZER_H_PX: f32 = 40.0;
/// Refresh while the live spectrum tap runs (≈ 30 fps).
const TAP_REFRESH: Duration = Duration::from_millis(33);
/// How often the mixer checks for PipeWire events otherwise.
const EVENT_POLL: Duration = Duration::from_millis(150);
/// Full mixer re-read every this many polls (≈ 6 s) as a safety net.
const SAFETY_REFRESH_TICKS: u32 = 40;

/// A sample spectrum for the preview when nothing is playing.
fn sample_levels(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let x = i as f32 / (n - 1).max(1) as f32;
            let bump = |(height, center, width): (f32, f32, f32)| height * (-((x - center) / width).powi(2)).exp();
            (bump(SAMPLE_BASS) + bump(SAMPLE_MIDS) + SAMPLE_FLOOR).clamp(0.0, 1.0)
        })
        .collect()
}

impl StudioView {
    /// Starts the event watcher and the refresh loop (once per visit).
    fn ensure_mixer(cx: &mut Context<Self>) {
        if state(cx).alive.as_ref().is_some_and(|a| a.load(Ordering::Relaxed)) {
            return;
        }
        let alive = Arc::new(AtomicBool::new(true));
        let dirty = Arc::new(AtomicBool::new(true));
        state(cx).alive = Some(alive.clone());
        {
            let (alive, dirty) = (alive.clone(), dirty.clone());
            let alive2 = alive.clone();
            audio::watch_mixer(move || alive2.load(Ordering::Relaxed), move || {
                dirty.store(true, Ordering::Relaxed);
                alive.load(Ordering::Relaxed)
            });
        }
        cx.spawn(async move |this, cx| {
            let mut tick = 0u32;
            loop {
                let fast = this.update(cx, |_, cx| state(cx).tap.as_ref().is_some_and(|t| t.is_active())).unwrap_or(false);
                cx.background_executor().timer(if fast { TAP_REFRESH } else { EVENT_POLL }).await;
                tick = tick.wrapping_add(1);
                let on_page = this.update(cx, |view, _| view.section == Section::Audio && view.query.is_empty()).unwrap_or(false);
                if !on_page {
                    alive.store(false, Ordering::Relaxed);
                    let _ = this.update(cx, |_, cx| {
                        let st = state(cx);
                        st.alive = None;
                        st.tap = None;
                        st.smoothed.clear();
                    });
                    return;
                }
                // Safety net: refresh every few seconds even without events.
                if dirty.swap(false, Ordering::Relaxed) || tick.is_multiple_of(SAFETY_REFRESH_TICKS) {
                    let result = cx.background_executor().spawn(async { audio::mixer() }).await;
                    let _ = this.update(cx, |_, cx| {
                        let st = state(cx);
                        st.mixer = Some(result);
                        st.dragging = None;
                        cx.notify();
                    });
                } else if fast {
                    let _ = this.update(cx, |_, cx| cx.notify());
                }
            }
        })
        .detach();
    }

    pub(super) fn audio_page(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        Self::ensure_mixer(cx);
        let mixer = state(cx).mixer.clone();
        let body: AnyElement = match mixer {
            None => card(p).child(div().text_color(p.on_surface_variant.hsla()).child(t("Reading audio devices…"))).into_any_element(),
            Some(Err(e)) => card(p)
                .child(heading(t("The audio server isn't reachable"), tf("PipeWire (pipewire-pulse) or PulseAudio must be running: {error}", &[("error", &e)]), p))
                .into_any_element(),
            Some(Ok(m)) => {
                let outputs = self.devices("out", &m.outputs, true, cfg, p, cx);
                let inputs = self.devices("in", &m.inputs, false, cfg, p, cx);
                let routes = self.routes(cfg, &m.outputs, p, cx);
                let apps = self.streams(&m.streams, p, cx);
                let mute_all = self.mute_all(m.all_muted, p, cx);
                div()
                    .row()
                    .flex_col()
                    .gap(px(SPACE_26_PX))
                    .child(mute_all)
                    .child(div().row().flex_col().gap(px(SPACE_10_PX)).child(group_title(t("Apps"), p)).child(apps))
                    // Output and input side by side when both fit.
                    .child(super::grid::fit(self.page.column_w, PANEL_GRID).at_most(2).render([
                        div().flex_1().row().flex_col().gap(px(SPACE_10_PX)).child(group_title(t("Output device"), p)).child(outputs),
                        div().flex_1().row().flex_col().gap(px(SPACE_10_PX)).child(group_title(t("Input device"), p)).child(inputs),
                    ]))
                    .child(div().row().flex_col().gap(px(SPACE_10_PX)).child(group_title(t("Per-app output"), p)).child(routes))
                    .into_any_element()
            }
        };
        use crate::registry::alert_settings::{PLAYERS, SOUND_EVENTS, SOUNDS, VOLUME};
        let mine = [VOLUME, PLAYERS, SOUNDS, SOUND_EVENTS];
        let preview = self.visualizer_preview(cfg, p, cx);
        let rows = self.rows(registry::in_section(Section::Audio).filter(|s| !mine.contains(&s.group)), cfg, p, cx);
        let volume = self.rows(registry::in_section(Section::Audio).filter(|s| s.group == VOLUME), cfg, p, cx);
        let players = self.rows(registry::in_section(Section::Audio).filter(|s| s.group == PLAYERS), cfg, p, cx);
        let ignore = self.ignore_list("players", mandara_i18n::tk("Never follow these players"), &cfg.media.ignore_players, |c| &mut c.media.ignore_players, p, cx);
        let sounds = self.rows(registry::in_section(Section::Audio).filter(|s| s.group == SOUNDS || s.group == SOUND_EVENTS), cfg, p, cx);
        let previews = self.sound_previews(p, cx);
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_26_PX))
            .child(body)
            .child(volume)
            .child(div().row().flex_col().gap(px(SPACE_10_PX)).child(group_title(t("Visualizer preview"), p)).child(preview))
            .child(rows)
            .child(div().row().flex_col().gap(px(SPACE_10_PX)).child(players).child(card(p).child(heading(t("Ignored players"), t("Identity or bus name, e.g. firefox or kdeconnect; the media panel and lock screen never follow them."), p)).child(ignore)))
            .child(div().row().flex_col().gap(px(SPACE_10_PX)).child(sounds).child(previews))
            .into_any_element()
    }

    fn devices(&mut self, kind: &'static str, list: &[Device], output: bool, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        if list.is_empty() {
            return card(p).child(div().text_color(p.on_surface_variant.hsla()).child(if output { t("No output devices") } else { t("No input devices") })).into_any_element();
        }
        let mut col = card(p).gap(px(SPACE_4_PX)).p(px(SPACE_8_PX));
        let renaming = state(cx).renaming.clone();
        for (n, d) in list.iter().enumerate() {
            let name = d.name.clone();
            let picked = d.index;
            let active = d.is_default;
            let hidden = cfg.audio.is_hidden(&d.name);
            let label = cfg.audio.device_label(&d.name, &d.description).to_string();
            let device_actions = self.device_actions(kind, n, d, hidden, p, cx);
            col = col.child(
                div()
                    .id(SharedString::from(format!("dev-{kind}-{n}")))
                    .px(px(SPACE_12_PX))
                    .py(px(SPACE_10_PX))
                    .rounded(px(RADIUS_14_PX))
                    .row()
                    .items_center()
                    .gap(px(SPACE_12_PX))
                    .cursor_pointer()
                    .when(active, |el| el.bg(p.primary.with_alpha(ALPHA_12).hsla()))
                    .when(!active, |el| el.hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_5).hsla())))
                    .child(
                        div()
                            .size(px(DIM_18_PX))
                            .rounded_full()
                            .row()
                            .items_center()
                            .justify_center()
                            .border_2()
                            .border_color(if active { p.primary.hsla() } else { p.outline.with_alpha(ALPHA_60).hsla() })
                            .when(active, |el| el.child(div().size(px(DIM_8_PX)).rounded_full().bg(p.primary.hsla()))),
                    )
                    .child(icon(if output { "headphones" } else { "mic" }, ICON_16_PX, if active { p.primary } else { p.on_surface_variant }))
                    .when(hidden && !active, |el| el.opacity(ALPHA_60))
                    .child(div().flex_1().min_w(px(0.0)).overflow_hidden().text_ellipsis().whitespace_nowrap().font_weight(if active { FontWeight::MEDIUM } else { FontWeight::NORMAL }).child(label))
                    .when(hidden, |el| el.child(badge(t("Hidden"), p.on_surface_variant)))
                    .when(d.muted, |el| el.child(badge(t("Muted"), p.urgent_coral)))
                    .child(device_actions)
                    .child(div().w(px(DIM_48_PX)).text_end().text_size(px(TEXT_12_PX)).font_features(mandara_ui::fonts::tabular_nums()).text_color(p.on_surface_variant.hsla()).child(format!("{:.0}%", d.volume * 100.0)))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        if active {
                            return;
                        }
                        let name = name.clone();
                        run(move || if output { audio::set_default_output(&name) } else { audio::set_default_input(&name) });
                        // Optimistic: mark it default right away.
                        if let Some(Ok(m)) = state(cx).mixer.as_mut() {
                            for d in if output { &mut m.outputs } else { &mut m.inputs } {
                                d.is_default = d.index == picked;
                            }
                        }
                        cx.notify();
                    })),
            );
            if renaming.as_deref() == Some(d.name.as_str()) {
                let key = d.name.clone();
                let field = self.list_field(
                    format!("rename-{key}"),
                    cfg.audio.device_labels.get(&key).map(String::as_str).unwrap_or_default(),
                    t("Your name for this device (empty: its own name)"),
                    std::rc::Rc::new(move |c: &mut ShellConfig, v: String| {
                        let v = v.trim().to_string();
                        if v.is_empty() {
                            c.audio.device_labels.remove(&key);
                        } else {
                            c.audio.device_labels.insert(key.clone(), v);
                        }
                    }),
                    cx,
                );
                col = col.child(div().px(px(SPACE_12_PX)).pb(px(SPACE_8_PX)).child(field));
            }
        }
        col.into_any_element()
    }

    /// Rename / hide buttons on a device row.
    fn device_actions(&mut self, kind: &'static str, n: usize, d: &Device, hidden: bool, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let small = |id: String, glyph: &'static str, tip: &'static str, p: &MandaraPalette| {
            div()
                .id(SharedString::from(id))
                .size(px(DIM_28_PX))
                .flex_none()
                .rounded_full()
                .row()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_8).hsla()))
                .child(icon(glyph, ICON_14_PX, p.on_surface_variant))
                .tooltip(super::compositor_page::tip(t(tip), p))
        };
        let (rename_name, hide_name) = (d.name.clone(), d.name.clone());
        div()
            .row()
            .flex_none()
            .items_center()
            .child(small(format!("dev-{kind}-{n}-rename"), "note", mandara_i18n::tk("Rename"), p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                this.list_fields.clear();
                let st = state(cx);
                st.renaming = if st.renaming.as_deref() == Some(rename_name.as_str()) { None } else { Some(rename_name.clone()) };
                cx.notify();
            })))
            .child(small(format!("dev-{kind}-{n}-hide"), if hidden { "plus" } else { "minus" }, if hidden { mandara_i18n::tk("Show again") } else { mandara_i18n::tk("Hide from lists and switching") }, p).on_click(move |_: &ClickEvent, _, cx| {
                cx.stop_propagation();
                let name = hide_name.clone();
                mandara_ui::update_config(cx, move |c| {
                    if c.audio.is_hidden(&name) {
                        c.audio.hidden_devices.retain(|h| *h != name);
                    } else {
                        c.audio.hidden_devices.push(name);
                    }
                });
            }))
            .into_any_element()
    }

    /// `[[audio.routes]]`: send an app's new streams to a chosen output.
    fn routes(&mut self, cfg: &ShellConfig, outputs: &[Device], p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let mut list = div().row().flex_col().gap(px(SPACE_8_PX));
        for (i, route) in cfg.audio.routes.iter().enumerate() {
            let app = self.list_field(
                format!("route-{i}-app"),
                &route.app,
                t("App, binary or role (music, phone, game…)"),
                std::rc::Rc::new(move |c: &mut ShellConfig, v: String| {
                    if let Some(r) = c.audio.routes.get_mut(i) {
                        r.app = v.trim().to_string();
                    }
                }),
                cx,
            );
            let mut picks = div().row().flex_wrap().gap(px(SPACE_6_PX));
            for (n, o) in outputs.iter().filter(|o| !cfg.audio.is_hidden(&o.name) || o.name == route.output).enumerate() {
                let name = o.name.clone();
                picks = picks.child(super::compositor_page::chip(SharedString::from(format!("route-{i}-o{n}")), cfg.audio.device_label(&o.name, &o.description).to_string(), route.output == o.name, p).on_click(move |_: &ClickEvent, _, cx| {
                    let name = name.clone();
                    mandara_ui::update_config(cx, move |c| {
                        if let Some(r) = c.audio.routes.get_mut(i) {
                            r.output = name;
                        }
                    });
                }));
            }
            let delete = div()
                .id(SharedString::from(format!("route-{i}-del")))
                .size(px(DIM_32_PX))
                .flex_none()
                .rounded_full()
                .row()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(p.urgent_coral.with_alpha(ALPHA_14).hsla()))
                .child(icon("trash", ICON_15_PX, p.on_surface_variant))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.list_fields.clear();
                    mandara_ui::update_config(cx, |c| {
                        if i < c.audio.routes.len() {
                            c.audio.routes.remove(i);
                        }
                    });
                }));
            list = list.child(
                div()
                    .p(px(SPACE_10_PX))
                    .rounded(px(RADIUS_14_PX))
                    .bg(p.surface_container_high.with_alpha(ALPHA_50).hsla())
                    .row()
                    .flex_col()
                    .gap(px(SPACE_8_PX))
                    .child(div().row().items_center().gap(px(SPACE_8_PX)).child(div().flex_1().min_w(px(0.0)).child(app)).child(delete))
                    .child(picks),
            );
        }
        let add = super::compositor_page::button("route-add", "plus", t("Add app"), false, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
            this.list_fields.clear();
            mandara_ui::update_config(cx, |c| c.audio.routes.push(mandara_config::alerts::AudioRoute::default()));
        }));
        card(p)
            .child(heading(t("Send apps to their own output"), t("New sound from a matching app goes to the chosen device, e.g. calls to a headset and music to the speakers."), p))
            .child(list)
            .child(div().row().child(add))
            .into_any_element()
    }

    /// Global mute ("Mute all sound"): state + switch. The shell owns it
    /// (`audio mute-all`), so the switch goes over IPC.
    fn mute_all(&mut self, on: bool, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let tint = if on { p.urgent_coral } else { p.on_surface_variant };
        card(p)
            .child(
                div()
                    .id("audio-mute-all")
                    .row()
                    .items_center()
                    .gap(px(SPACE_14_PX))
                    .cursor_pointer()
                    .child(div().size(px(DIM_40_PX)).flex_none().rounded(px(RADIUS_12_PX)).bg(tint.with_alpha(ALPHA_16).hsla()).row().items_center().justify_center().child(icon(if on { "volume-off" } else { "volume-high" }, ICON_16_PX, tint)))
                    .child(div().flex_1().min_w(px(0.0)).child(heading(
                        t("Mute all sound"),
                        t("Silences every output (speakers, HDMI and virtual devices such as EasyEffects) and every app at once. Switching it off restores your own mutes. Also: middle-click the bar's sound icon, or `mandara msg audio mute-all toggle`."),
                        p,
                    )))
                    .when(on, |el| el.child(badge(t("All sound muted"), p.urgent_coral)))
                    .child(super::form::switch(on, p))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        if let Some(ipc) = cx.try_global::<crate::perf::ShellIpc>().cloned() {
                            let words = ["audio", "mute-all", if on { "off" } else { "on" }].map(String::from).to_vec();
                            std::thread::spawn(move || {
                                if let Err(err) = (ipc.0)(words) {
                                    tracing::warn!("audio mute-all: {err}");
                                }
                            });
                        }
                        // Optimistic until the next mixer read.
                        if let Some(Ok(m)) = state(cx).mixer.as_mut() {
                            m.all_muted = !on;
                        }
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    /// Preview buttons for each shell sound (plays even while sounds are off).
    fn sound_previews(&mut self, p: &MandaraPalette, _cx: &mut Context<Self>) -> AnyElement {
        use mandara_services::sounds::SoundEvent;
        let label = |e: &SoundEvent| match e {
            SoundEvent::VolumeChange => mandara_i18n::tk("Volume"),
            SoundEvent::NotificationLow => mandara_i18n::tk("Low urgency"),
            SoundEvent::Notification => mandara_i18n::tk("Notification"),
            SoundEvent::NotificationCritical => mandara_i18n::tk("Critical"),
            SoundEvent::PowerPlug => mandara_i18n::tk("Charger"),
            SoundEvent::Screenshot => mandara_i18n::tk("Screenshot"),
            SoundEvent::Login => mandara_i18n::tk("Login"),
            SoundEvent::BatteryLow => mandara_i18n::tk("Low battery"),
            _ => mandara_i18n::tk("Sound"),
        };
        let mut row = div().row().flex_wrap().gap(px(SPACE_6_PX));
        for (n, e) in SoundEvent::CONFIGURABLE.iter().enumerate() {
            let key = e.key().to_string();
            row = row.child(super::compositor_page::chip(SharedString::from(format!("sound-try-{n}")), t(label(e)), false, p).child(icon("play", ICON_12_PX, p.primary)).on_click(move |_: &ClickEvent, _, cx| {
                if let Some(ipc) = cx.try_global::<crate::perf::ShellIpc>().cloned() {
                    let words = vec!["sounds".to_string(), "preview".to_string(), key.clone()];
                    std::thread::spawn(move || {
                        if let Err(err) = (ipc.0)(words) {
                            tracing::warn!("sound preview: {err}");
                        }
                    });
                }
            }));
        }
        card(p).child(heading(t("Listen"), t("Plays each sound from your theme at the volume above (even while shell sounds are off). Your own files: `[sounds.files]` in config.toml."), p)).child(row).into_any_element()
    }

    fn streams(&mut self, list: &[Stream], p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        if list.is_empty() {
            return card(p).child(heading(t("No apps are playing sound"), t("Apps appear here with their own volume while they play."), p)).into_any_element();
        }
        let dragging = state(cx).dragging;
        // "Allow above 100 %" sets the slider's top, as in the shell.
        let limit = mandara_ui::state::config(cx).read(cx).get().audio.volume_limit();
        let mut col = card(p).gap(px(SPACE_4_PX)).p(px(SPACE_8_PX));
        for s in list {
            let index = s.index;
            let volume = dragging.filter(|(i, _)| *i == index).map(|(_, v)| v).unwrap_or(s.volume);
            // The stream's own icon name, else its app id / binary / name as
            // a window app id would be resolved; a monogram otherwise.
            let candidates: Vec<String> = [s.app_id.clone(), s.binary.clone(), Some(s.app.clone()), Some(s.app.to_lowercase())].into_iter().flatten().filter(|k| !k.is_empty()).collect();
            let found = s
                .icon
                .as_deref()
                .and_then(|name| mandara_ui::icon_theme::resolve(name, None, false))
                .or_else(|| candidates.iter().find_map(|k| mandara_ui::apps::icon_for_app_id(k, false)));
            let art = match found {
                Some(found) if found.symbolic => gpui::svg().external_path(found.path.to_string_lossy().to_string()).size(px(DIM_30_PX)).flex_none().text_color(p.on_surface.hsla()).into_any_element(),
                Some(found) => div().size(px(DIM_30_PX)).flex_none().child(gpui::img(found.path).size_full().object_fit(gpui::ObjectFit::Contain)).into_any_element(),
                None => mandara_ui::plugins::workspaces::app_icon(candidates.first().map(String::as_str).unwrap_or("audio"), APP_ICON_PX, p.on_surface, false, p),
            };
            let muted = s.muted;
            let slider = mandara_ui::kit::slider(SharedString::from(format!("stream-{index}")), (volume / limit).min(1.0), p, move |f, _, cx| {
                let v = ((f * limit * VOLUME_STEPS).round() / VOLUME_STEPS).clamp(0.0, limit);
                state(cx).dragging = Some((index, v));
                run(move || audio::set_stream_volume(index, v));
            });
            col = col.child(
                div()
                    .px(px(SPACE_12_PX))
                    .py(px(SPACE_10_PX))
                    .rounded(px(RADIUS_14_PX))
                    .row()
                    .items_center()
                    .gap(px(SPACE_12_PX))
                    .when(s.corked, |el| el.opacity(ALPHA_60))
                    .child(art)
                    .child(
                        // The name column shrinks with the window; the slider takes the rest.
                        div()
                            .w(gpui::relative(MIXER_NAME_SHARE))
                            .max_w(px(MIXER_NAME_MAX_W_PX))
                            .min_w(px(0.0))
                            .flex_none()
                            .row()
                            .flex_col()
                            .child(div().font_weight(FontWeight::MEDIUM).overflow_hidden().text_ellipsis().whitespace_nowrap().child(s.app.clone()))
                            .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).overflow_hidden().text_ellipsis().whitespace_nowrap().child(if s.corked { t("Paused").to_string() } else { s.media.clone() })),
                    )
                    .child(div().flex_1().min_w(px(0.0)).child(slider))
                    .child(div().w(px(DIM_46_PX)).text_end().text_size(px(TEXT_12_PX)).font_features(mandara_ui::fonts::tabular_nums()).text_color(if volume > 1.0 { p.warning_amber.hsla() } else { p.on_surface_variant.hsla() }).child(format!("{:.0}%", volume * 100.0)))
                    .child(
                        div()
                            .id(("stream-mute", index as usize))
                            .size(px(DIM_32_PX))
                            .rounded_full()
                            .row()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .bg(if muted { p.urgent_coral.with_alpha(ALPHA_16).hsla() } else { p.surface_container_high.hsla() })
                            .hover(|st| st.opacity(ALPHA_85))
                            .child(icon(if muted { "volume-mute" } else { "volume-high" }, ICON_15_PX, if muted { p.urgent_coral } else { p.on_surface_variant }))
                            .tooltip(super::compositor_page::tip(if muted { t("Unmute") } else { t("Mute") }, p))
                            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                run(move || audio::set_stream_mute(index, !muted));
                                if let Some(Ok(m)) = state(cx).mixer.as_mut()
                                    && let Some(s) = m.streams.iter_mut().find(|s| s.index == index)
                                {
                                    s.muted = !muted;
                                }
                                cx.notify();
                            })),
                    ),
            );
        }
        col.into_any_element()
    }

    fn visualizer_preview(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let settings = visualizer::Settings::from_config(cfg);
        let st = state(cx);
        if st.tap.is_none() {
            st.tap = mandara_services::spectrum::subscribe();
        }
        let live = st.tap.as_ref().filter(|t| t.is_active()).map(|t| t.bands());
        let levels = match live {
            Some(bands) => {
                let next = visualizer::resample(&bands, settings.bars);
                visualizer::smooth(&mut st.smoothed, &next, settings.smoothing);
                st.smoothed.clone()
            }
            None => {
                st.smoothed.clear();
                sample_levels(settings.bars)
            }
        };
        let (from, to) = (p.primary, p.tertiary);
        card(p)
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_10_PX))
                    .child(div().flex_1().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(if live.is_some() { t("Live — what's playing now") } else { t("Sample levels — play something to see it live") }))
                    .when(live.is_some(), |el| el.child(badge(t("Live"), p.success_emerald))),
            )
            .child(
                // The media panel's width, so the preview is true to size.
                div().w_full().max_w(px(MEDIA_PANEL_W_PX + MEDIA_PANEL_PAD_PX)).px(px(SPACE_16_PX)).py(px(SPACE_14_PX)).rounded(px(RADIUS_16_PX)).bg(p.surface_dim.hsla()).child(visualizer::draw(&settings, &levels, None, from, to, p, VISUALIZER_H_PX)),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_levels_are_in_range() {
        let v = sample_levels(32);
        assert_eq!(v.len(), 32);
        assert!(v.iter().all(|x| (0.0..=1.0).contains(x)));
        assert!(v[5] > v[31], "a bass-heavy shape");
    }
}
