//! Studio's screen-recorder controls (Plugins → Screenshot): the installed
//! recorders and what each can do, the sound-device and display pickers
//! (live lists, read once in the background when first shown), and the
//! options of the setting choices, which Studio's one choice control
//! ([`super::choice`]) draws, disabled options with their reason.

use std::time::{Duration, Instant};

use gpui::{AnyElement, ClickEvent, Context, FontWeight, IntoElement, ParentElement as _, SharedString, Styled as _, div, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_config::recorder::{RecordAudio, RecordCodec, RecordContainer, RecorderSettings};
use mandara_core::ToGpui as _;
use mandara_i18n::{t, tf, tr};
use mandara_services::recording::backend::{self, BackendStatus, Installed};
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;
use serde_json::Value;

use super::StudioView;
use super::choice::{self, ChoiceItem};
use crate::registry::Setting;
use crate::tokens::*;

/// Gap between a recorder's name and its line.
const CHIP_GAP_PX: f32 = super::choice::CHOICE_GAP_PX;
/// Vertical padding of a recorder's status badge.
const BADGE_PAD_Y_PX: f32 = 1.0;
/// Width of the recorders list.
const BACKENDS_W_PX: f32 = 360.0;

/// A list that couldn't be read is read again once this long has passed
/// (the sound server restarting, a display waking up).
const RETRY_AFTER: Duration = Duration::from_secs(3);

/// Sound devices `(outputs, inputs)` as `(name, description)`.
type Devices = (Vec<(String, String)>, Vec<(String, String)>);

/// One live list: what was read and whether the read failed (and when).
pub(super) struct Lookup<T> {
    value: Option<T>,
    failed_at: Option<Instant>,
    loading: bool,
}

impl<T> Default for Lookup<T> {
    fn default() -> Self {
        Lookup { value: None, failed_at: None, loading: false }
    }
}

impl<T: Clone> Lookup<T> {
    /// Whether a read should start now: nothing read yet, or the last read
    /// failed at least [`RETRY_AFTER`] ago (never while one runs).
    fn due(&self, now: Instant) -> bool {
        !self.loading && (self.value.is_none() || self.failed_at.is_some_and(|at| now.duration_since(at) >= RETRY_AFTER))
    }

    /// A read finished: its result, or what to show after a failure.
    fn done(&mut self, read: Result<T, T>, now: Instant) {
        self.loading = false;
        let (value, failed) = match read {
            Ok(v) => (v, None),
            Err(fallback) => (fallback, Some(now)),
        };
        self.value = Some(value);
        self.failed_at = failed;
    }
}

/// The lists the pickers offer, read off the UI thread once per visit to
/// the recorder settings (and again after a failed read).
#[derive(Default)]
pub struct RecorderRow {
    devices: Lookup<Devices>,
    displays: Lookup<Vec<String>>,
    /// GPU Screen Recorder's `--info` was read on this visit.
    probed: bool,
}

impl RecorderRow {
    /// A new visit: devices, displays and the codec report are read again.
    pub(super) fn new_visit(&mut self) {
        *self = RecorderRow::default();
    }
}

/// One option of a segmented choice: value, label, why it is disabled.
pub(super) struct ChipOption {
    pub value: Value,
    pub raw: String,
    pub label: String,
    pub locked: Option<&'static str>,
}

fn codec_label(c: RecordCodec) -> &'static str {
    match c {
        RecordCodec::H264 => "H.264",
        RecordCodec::Hevc => "HEVC",
        RecordCodec::Av1 => "AV1",
    }
}

fn container_label(c: RecordContainer) -> &'static str {
    match c {
        RecordContainer::Mp4 => "MP4",
        RecordContainer::Mkv => "MKV",
        RecordContainer::Webm => "WebM",
    }
}

/// What a recorder can do, in a line.
fn caps_line(s: &BackendStatus) -> String {
    let c = &s.caps;
    let codecs = c.codecs.iter().map(|c| codec_label(*c)).collect::<Vec<_>>().join(", ");
    let containers = c.containers.iter().map(|c| container_label(*c)).collect::<Vec<_>>().join(", ");
    let sound = if c.audio_both {
        t("system sound and microphone")
    } else if c.audio {
        t("one sound source")
    } else {
        t("no sound")
    };
    let pause = if c.pause { t("pause") } else { t("no pause") };
    tf("{codecs} · {formats} · up to {fps} fps · {sound} · {pause}", &[("codecs", &codecs), ("formats", &containers), ("fps", &c.max_fps), ("sound", &sound), ("pause", &pause)])
}

impl StudioView {
    /// A setting's choice on Studio's one choice control
    /// ([`super::choice::segmented`]): one line when it fits `room`, else a
    /// balanced grid; disabled options are dimmed and their reasons listed
    /// underneath.
    pub(super) fn chip_choice(&mut self, setting: &Setting, options: Vec<ChipOption>, current: &str, room: choice::Room, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let items = options
            .into_iter()
            .map(|o| {
                let setting = setting.clone();
                let value = o.value;
                ChoiceItem::new(SharedString::from(format!("c{}{}", setting.path, o.raw)), o.label, o.raw == current, cx.listener(move |this, _: &ClickEvent, _, cx| this.apply(&setting, value.clone(), cx)))
                    .locked(o.locked.map(|r| SharedString::from(tr(r))))
            })
            .collect();
        choice::segmented(room, &choice::SETTING_CHOICE, items, p)
    }

    /// The sound devices, read in the background on each visit (and again
    /// after a failed read).
    fn recorder_devices(&mut self, cx: &mut Context<Self>) -> Option<Devices> {
        if self.recorder_row.devices.due(Instant::now()) {
            self.recorder_row.devices.loading = true;
            cx.spawn(async move |this, cx| {
                let mixer = cx.background_executor().spawn(async { mandara_services::audio::mixer() }).await;
                let lists = match mixer {
                    Ok(m) => Ok((m.outputs.into_iter().map(|d| (d.name, d.description)).collect(), m.inputs.into_iter().map(|d| (d.name, d.description)).collect())),
                    Err(e) => {
                        tracing::warn!("recorder devices: {e}");
                        Err((Vec::new(), Vec::new()))
                    }
                };
                let _ = this.update(cx, |view, cx| {
                    view.recorder_row.devices.done(lists, Instant::now());
                    cx.notify();
                });
            })
            .detach();
        }
        self.recorder_row.devices.value.clone()
    }

    /// The displays, read in the background on each visit (and again
    /// after a failed read).
    fn recorder_displays(&mut self, cx: &mut Context<Self>) -> Option<Vec<String>> {
        if self.recorder_row.displays.due(Instant::now()) {
            self.recorder_row.displays.loading = true;
            cx.spawn(async move |this, cx| {
                let outputs = cx.background_executor().spawn(async { mandara_core::query_outputs() }).await;
                let names = match outputs {
                    Ok(list) if !list.is_empty() => Ok(list.into_iter().map(|o| o.name).collect()),
                    Ok(_) => Err(Vec::new()),
                    Err(e) => {
                        tracing::warn!("recorder displays: {e:#}");
                        Err(Vec::new())
                    }
                };
                let _ = this.update(cx, |view, cx| {
                    view.recorder_row.displays.done(names, Instant::now());
                    cx.notify();
                });
            })
            .detach();
        }
        self.recorder_row.displays.value.clone()
    }

    /// A sound device picker's options (Default plus the server's
    /// devices), or `None` while the devices are being read.
    pub(super) fn recorder_device_options(&mut self, setting: &Setting, input: bool, cfg: &ShellConfig, cx: &mut Context<Self>) -> Option<Vec<ChipOption>> {
        let current = crate::registry::get(cfg, setting).as_str().unwrap_or_default().to_string();
        let (outputs, inputs) = self.recorder_devices(cx)?;
        let list = if input { inputs } else { outputs };
        let mut options = vec![ChipOption { value: Value::from(""), raw: String::new(), label: t("Default").into(), locked: None }];
        // A device that is gone stays listed while it is chosen.
        if !current.is_empty() && !list.iter().any(|(n, _)| *n == current) {
            options.push(ChipOption { value: Value::from(current.clone()), raw: current.clone(), label: current.clone(), locked: None });
        }
        options.extend(list.into_iter().map(|(name, description)| ChipOption { value: Value::from(name.clone()), raw: name, label: description, locked: None }));
        Some(options)
    }

    /// A sound device picker: Default plus the server's devices.
    pub(super) fn recorder_device_control(&mut self, setting: &Setting, input: bool, room: choice::Room, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let current = crate::registry::get(cfg, setting).as_str().unwrap_or_default().to_string();
        let Some(options) = self.recorder_device_options(setting, input, cfg, cx) else {
            return div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("Reading audio devices…")).into_any_element();
        };
        let chips = self.chip_choice(setting, options, &current, room, p, cx);
        if self.recorder_row.devices.failed_at.is_none() {
            return chips;
        }
        // Only Default can be offered until the sound server answers.
        div()
            .row()
            .flex_col()
            .when(!room.under, |el| el.cross_end())
            .when(room.under, |el| el.cross_start())
            .gap(px(SPACE_6_PX))
            .child(chips)
            .child(div().max_w(px(super::LOCKED_NOTE_MAX_W_PX)).text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("The audio server isn't reachable"))))
            .into_any_element()
    }

    /// A display picker's options: Focused plus every output.
    pub(super) fn recorder_display_options(&mut self, setting: &Setting, cfg: &ShellConfig, cx: &mut Context<Self>) -> Vec<ChipOption> {
        let current = crate::registry::get(cfg, setting).as_str().unwrap_or_default().to_string();
        let displays = self.recorder_displays(cx).unwrap_or_default();
        let mut options = vec![ChipOption { value: Value::from(""), raw: String::new(), label: t("Focused display").into(), locked: None }];
        if !current.is_empty() && !displays.contains(&current) {
            options.push(ChipOption { value: Value::from(current.clone()), raw: current.clone(), label: current.clone(), locked: None });
        }
        options.extend(displays.into_iter().map(|d| ChipOption { value: Value::from(d.clone()), raw: d.clone(), label: d, locked: None }));
        options
    }

    /// A display picker: Focused plus every output.
    pub(super) fn recorder_display_control(&mut self, setting: &Setting, room: choice::Room, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let current = crate::registry::get(cfg, setting).as_str().unwrap_or_default().to_string();
        let options = self.recorder_display_options(setting, cfg, cx);
        self.chip_choice(setting, options, &current, room, p, cx)
    }

    /// The recorders on this computer: installed or not, the one in use,
    /// and what each supports.
    pub(super) fn recorder_backends_control(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let installed = Installed::cached();
        // Read GPU Screen Recorder's codec report once, then redraw with it.
        if !self.recorder_row.probed {
            self.recorder_row.probed = true;
            if let Some(path) = installed.gpu_screen_recorder.clone() {
                cx.spawn(async move |this, cx| {
                    cx.background_executor().spawn(async move { backend::gsr_info_now(&path) }).await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
                .detach();
            }
        }
        let settings = RecorderSettings::from_plugins(&cfg.plugins);
        let in_use = installed.resolve(settings.backend).ok();
        let rows = backend::statuses(&installed).into_iter().map(|s| {
            let (badge, color) = if Some(s.backend) == in_use {
                (t("In use"), p.primary)
            } else if s.installed {
                (t("Installed"), p.success_emerald)
            } else {
                (t("Not installed"), p.on_surface_variant)
            };
            div()
                .row()
                .flex_col()
                .gap(px(CHIP_GAP_PX))
                .child(
                    div()
                        .row()
                        .items_center()
                        .gap(px(SPACE_8_PX))
                        .child(div().text_size(px(TEXT_12_5_PX)).font_weight(FontWeight::MEDIUM).child(tr(s.backend.label())))
                        .child(div().px(px(SPACE_8_PX)).py(px(BADGE_PAD_Y_PX)).rounded_full().bg(color.with_alpha(ALPHA_14).hsla()).text_color(color.hsla()).text_size(px(TEXT_11_PX)).child(badge)),
                )
                .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(caps_line(&s))))
                .when_some((s.backend == backend::Backend::GpuScreenRecorder && s.installed && !s.probed).then_some(s.probe_failed), |el, failed| {
                    let note = if failed { t("Couldn't read which codecs your graphics card encodes, so all of them are offered") } else { t("Checking which codecs your graphics card encodes…") };
                    el.child(div().text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(note)))
                })
                .when(s.backend == backend::Backend::WfRecorder && s.installed && settings.audio == RecordAudio::Both, |el| el.child(div().text_size(px(TEXT_11_PX)).text_color(p.warning_amber.hsla()).child(tr(backend::audio_reason(s.backend, RecordAudio::Both).unwrap_or_default()))))
        });
        div().max_w(px(BACKENDS_W_PX)).row().flex_col().gap(px(SPACE_10_PX)).children(rows).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A list is read once per visit; a failed read is tried again after a
    /// pause, a good one isn't; a new visit reads everything again.
    #[test]
    fn lookups_retry_failures_and_refresh_per_visit() {
        let t0 = Instant::now();
        let mut l: Lookup<Vec<String>> = Lookup::default();
        assert!(l.due(t0), "nothing read yet");
        l.loading = true;
        assert!(!l.due(t0), "never two reads at once");
        l.done(Err(Vec::new()), t0);
        assert_eq!(l.value, Some(Vec::new()), "a failure still shows the fallback");
        assert!(!l.due(t0), "not at once after a failure");
        assert!(l.due(t0 + RETRY_AFTER), "a failed read is tried again");
        l.done(Ok(vec!["DP-1".into()]), t0 + RETRY_AFTER);
        assert!(!l.due(t0 + RETRY_AFTER * 10), "a good read stays for the visit");
        let mut row = RecorderRow { displays: l, probed: true, ..RecorderRow::default() };
        row.new_visit();
        assert!(row.displays.due(t0) && row.devices.due(t0) && !row.probed, "a new visit reads again");
    }
}
