//! Screenshot utility: camera button in the bar (right-click = instant
//! region capture); panel with region / window / screen capture, a timer,
//! and copy / save toggles.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{App, FontWeight, IntoElement, ParentElement, Styled, div, px, size};
use mandara_core::ToGpui;
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};

use super::fn_view;
use crate::icons::icon;
use crate::kit::{reveal, text, toggle_tile, tokens};
use crate::state::{config, shell, update_config};

pub const ID: &str = "screenshot";

/// Capture-timer choices the timer tile cycles through, in seconds.
const DELAY_STEPS_S: [u64; 3] = [3, 5, 10];
/// Longest `--delay` a command may ask for, in seconds.
const MAX_DELAY_S: u64 = 60;
/// Lead-in before a screen recording starts from the panel (lets the panel
/// close first).
const RECORD_LEAD_IN: std::time::Duration = std::time::Duration::from_millis(250);
/// Bar chip glyph, recording dot and gap.
const BAR_ICON_PX: f32 = 15.0;
const REC_DOT_PX: f32 = 9.0;
const REC_GAP_PX: f32 = 6.0;
const PANEL_W_PX: f32 = 400.0;
const PANEL_H_PX: f32 = 272.0;

/// The next timer step after `delay` (off after the longest).
fn next_delay(delay: u64) -> u64 {
    DELAY_STEPS_S.iter().copied().find(|d| *d > delay).unwrap_or(0)
}

/// The screenshot folder as shown in the panel (`~` for home).
fn dir_label(cx: &App) -> gpui::SharedString {
    let setting = mandara_config::workflow::ScreenshotSettings::from_plugins(&config(cx).read(cx).get().plugins).directory;
    let dir = mandara_services::screenshot::output_dir(&setting);
    let home = dirs::home_dir().unwrap_or_default();
    match dir.strip_prefix(&home) {
        Ok(rest) => format!("~/{}", rest.display()).into(),
        Err(_) => dir.display().to_string().into(),
    }
}

#[derive(Copy, Clone)]
struct Options {
    copy: bool,
    save: bool,
    delay: u64,
}

fn options(cx: &App) -> Options {
    let cfg = config(cx).read(cx).get();
    Options {
        copy: cfg.plugins.setting(ID, "copy", true),
        save: cfg.plugins.setting(ID, "save", true),
        delay: cfg.plugins.setting(ID, "delay", 0u64),
    }
}

fn set_option(cx: &mut App, key: &'static str, value: toml::Value) {
    update_config(cx, |c| {
        let table = c.plugins.settings.entry(ID.to_string()).or_default();
        // Copy and save can't both be off (a capture would do nothing).
        if value == toml::Value::Boolean(false) {
            let other = if key == "copy" { "save" } else if key == "save" { "copy" } else { "" };
            if !other.is_empty() {
                table.insert(other.to_string(), toml::Value::Boolean(true));
            }
        }
        table.insert(key.to_string(), value);
    });
}

/// [`start_delayed`] with the panel timer.
pub(crate) fn start(mode: &str, cx: &mut App) -> Result<Option<async_channel::Receiver<Result<String, String>>>, String> {
    start_delayed(mode, None, cx)
}

/// `--delay [N]` / `--delay=N` in a command's words: `None` without the
/// flag, the countdown setting when no number follows.
fn parse_delay(words: &[String], countdown: u64) -> Result<Option<u64>, String> {
    let mut delay = None;
    let mut it = words.iter().peekable();
    while let Some(w) = it.next() {
        let value = if w == "--delay" {
            match it.peek() {
                Some(n) if n.parse::<f64>().is_ok() => it.next().cloned(),
                _ => None,
            }
        } else if let Some(v) = w.strip_prefix("--delay=") {
            Some(v.to_string())
        } else {
            return Err(format!("unexpected argument {w:?} (expected --delay [seconds])"));
        };
        let secs = match value {
            Some(v) => v.parse::<f64>().map_err(|_| format!("--delay: {v:?} is not a number of seconds"))?.round().max(0.0) as u64,
            None => countdown,
        };
        delay = Some(secs.min(MAX_DELAY_S));
    }
    Ok(delay)
}

/// Starts a capture: `region` opens the snipping overlay; `window` /
/// `screen` capture immediately (natively, no external tools). `delay`
/// overrides the panel timer; a delayed capture shows a countdown (Esc
/// cancels) and nothing of it is in the frame.
pub(crate) fn start_delayed(mode: &str, delay: Option<u64>, cx: &mut App) -> Result<Option<async_channel::Receiver<Result<String, String>>>, String> {
    shell(cx).update(cx, |s, cx| s.set_panel(None, cx));
    if !matches!(mode, "region" | "pick-window" | "window" | "screen") {
        return Err(format!("unknown mode {mode:?}"));
    }
    // The timer waits before capturing (the panel has already closed).
    let delay = delay.unwrap_or(options(cx).delay);
    if delay > 0 {
        let (tx, rx) = async_channel::bounded(1);
        let edit_after = mandara_config::workflow::ScreenshotSettings::from_plugins(&config(cx).read(cx).get().plugins).edit_after;
        let snip = |mode, preselect, cx: &mut App| crate::snip::begin(mode, false, preselect, delay, None, Some(tx.clone()), cx);
        match mode {
            "region" => snip(crate::snip::SnipMode::Region, crate::snip::Preselect::None, cx),
            "pick-window" => snip(crate::snip::SnipMode::Window, crate::snip::Preselect::None, cx),
            "window" | "screen" if edit_after => snip(crate::snip::SnipMode::Region, if mode == "window" { crate::snip::Preselect::FocusedWindow } else { crate::snip::Preselect::Full }, cx),
            _ => {
                // A direct capture once the countdown is over (Esc answers
                // "capture cancelled" instead).
                let mode = mode.to_string();
                let cancel = tx.clone();
                let job: crate::snip::Job = Box::new(move |cx| {
                    if let Some(pending) = capture(&mode, cx) {
                        cx.spawn(async move |_| {
                            let result = pending.recv().await.unwrap_or_else(|_| Err("capture was cancelled".into()));
                            let _ = tx.send(result).await;
                        })
                        .detach();
                    }
                });
                crate::snip::begin(crate::snip::SnipMode::Screen, false, crate::snip::Preselect::None, delay, Some(job), Some(cancel), cx);
            }
        }
        return Ok(Some(rx));
    }
    Ok(capture(mode, cx))
}

/// Performs a capture now. Immediate captures report through a
/// notification too, so bar clicks get feedback (commands get the result).
fn capture(mode: &str, cx: &mut App) -> Option<async_channel::Receiver<Result<String, String>>> {
    match mode {
        "region" => {
            crate::snip::open(crate::snip::SnipMode::Region, cx);
            None
        }
        "pick-window" => {
            crate::snip::open(crate::snip::SnipMode::Window, cx);
            None
        }
        // "Edit after capture": open the editor with the window / screen selected.
        "window" | "screen" if mandara_config::workflow::ScreenshotSettings::from_plugins(&config(cx).read(cx).get().plugins).edit_after => {
            crate::snip::open_editor(if mode == "window" { crate::snip::Preselect::FocusedWindow } else { crate::snip::Preselect::Full }, cx);
            None
        }
        _ => {
            let rx = crate::snip::capture_now(mode == "window", cx);
            // Fan the result out to a notification and to the caller.
            let (tx, out) = async_channel::bounded(1);
            cx.spawn(async move |cx| {
                let result = rx.recv().await.unwrap_or_else(|_| Err("capture thread ended".into()));
                cx.update(|cx| crate::snip::report(result.clone(), cx));
                let _ = tx.send(result).await;
            })
            .detach();
            Some(out)
        }
    }
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, mandara_i18n::tk("Screenshot"), mandara_i18n::tk("Region, window and screen capture"),Some(Zone::Right)), |window, cx, panel_cx| {
        let bar = fn_view(
            None,
            |palette, _, cx| {
                let rec = crate::recorder::recorder(cx).map(|r| (r.read(cx).clock(), r.read(cx).busy));
                match rec {
                    // Recording: red dot + elapsed time (click to stop).
                    Some((Some(clock), _)) => div()
                        .flex()
                        .items_center()
                        .gap(px(REC_GAP_PX))
                        .child(div().size(px(REC_DOT_PX)).rounded_full().bg(palette.urgent_coral.hsla()))
                        .child(div().font_features(crate::fonts::tabular_nums()).text_color(palette.urgent_coral.hsla()).child(clock))
                        .into_any_element(),
                    Some((None, true)) => div().flex().items_center().child(icon("timer", crate::style::bar_icon(BAR_ICON_PX), palette.urgent_coral)).into_any_element(),
                    _ => div().flex().items_center().child(icon("camera", crate::style::bar_icon(BAR_ICON_PX), palette.on_surface)).into_any_element(),
                }
            },
            window,
            cx,
        );
        let p = panel_cx.clone();
        let panel = fn_view(
            None,
            move |palette, _, cx| {
                let o = options(cx);
                let modes = div()
                    .flex()
                    .gap(px(tokens::SM))
                    .child(toggle_tile("shot-region", "crop", mandara_i18n::t("Region"), Some(mandara_i18n::t("Select area").into()), true, palette, |_, cx| {
                        let _ = start("region", cx);
                    }))
                    .child(toggle_tile("shot-window", "window", mandara_i18n::t("Window"), Some(mandara_i18n::t("Pick a window").into()), false, palette, |_, cx| {
                        let _ = start("pick-window", cx);
                    }))
                    .child(toggle_tile("shot-screen", "monitor", mandara_i18n::t("Screen"), Some(mandara_i18n::t("Whole output").into()), false, palette, |_, cx| {
                        let _ = start("screen", cx);
                    }));
                let recording = crate::recorder::is_recording(cx);
                let record = div()
                    .flex()
                    .gap(px(tokens::SM))
                    .child(toggle_tile(
                        "rec-screen",
                        if recording { "close" } else { "circle" },
                        if recording { mandara_i18n::t("Stop recording") } else { mandara_i18n::t("Record screen") },
                        Some(if recording { crate::recorder::recorder(cx).and_then(|r| r.read(cx).clock()).unwrap_or_default() } else { "AV1 · WebM".into() }),
                        recording,
                        palette,
                        move |_, cx| {
                            shell(cx).update(cx, |s, cx| s.set_panel(None, cx));
                            if recording {
                                let _ = crate::recorder::stop(cx);
                            } else {
                                let _ = crate::recorder::start(String::new(), None, RECORD_LEAD_IN, cx);
                            }
                        },
                    ))
                    .child(toggle_tile("rec-region", "crop", mandara_i18n::t("Record region"), Some(mandara_i18n::t("Select an area").into()), false, palette, |_, cx| {
                        shell(cx).update(cx, |s, cx| s.set_panel(None, cx));
                        crate::snip::open_for_recording(cx);
                    }));
                let opts = div()
                    .flex()
                    .gap(px(tokens::SM))
                    .child(toggle_tile("shot-copy", "clipboard", mandara_i18n::t("Auto-copy"), Some(if o.copy { mandara_i18n::t("On").into() } else { mandara_i18n::t("Off").into() }), o.copy, palette, move |_, cx| set_option(cx, "copy", toml::Value::Boolean(!o.copy))))
                    .child(toggle_tile("shot-save", "disk", mandara_i18n::t("Save to disk"), Some(if o.save { dir_label(cx).to_string() } else { mandara_i18n::t("Clipboard only").into() }), o.save, palette, move |_, cx| set_option(cx, "save", toml::Value::Boolean(!o.save))))
                    .child(toggle_tile(
                        "shot-delay",
                        "timer",
                        if o.delay == 0 { mandara_i18n::t("No timer").to_string() } else { mandara_i18n::tf("{seconds}s timer", &[("seconds", &o.delay)]) },
                        None,
                        o.delay > 0,
                        palette,
                        move |_, cx| set_option(cx, "delay", toml::Value::Integer(i64::try_from(next_delay(o.delay)).unwrap_or(0))),
                    ));
                div()
                    .size_full()
                    .p(px(tokens::LG))
                    .flex()
                    .flex_col()
                    .gap(px(tokens::MD))
                    .child(reveal(&p, 0, div().flex().justify_between().child(div().font_weight(FontWeight::MEDIUM).child(mandara_i18n::t("Screenshot"))).child(
                        div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(dir_label(cx)),
                    )))
                    .child(reveal(&p, 1, modes))
                    .child(reveal(&p, 2, record))
                    .child(reveal(&p, 3, opts))
                    .into_any_element()
            },
            window,
            cx,
        );
        PluginInstance {
            bar: Some(bar),
            panel: Some(PanelSpec { view: panel, size: Rc::new(Cell::new(size(px(PANEL_W_PX), px(PANEL_H_PX)))) }),
            // While recording, clicking the red dot stops; otherwise open the panel.
            on_click: Some(Rc::new(|_, cx| {
                if crate::recorder::is_recording(cx) {
                    let _ = crate::recorder::stop(cx);
                } else {
                    shell(cx).update(cx, |s, cx| s.toggle_panel(ID, cx));
                }
            })),
            on_secondary_click: Some(Rc::new(|_, cx| {
                let _ = start("region", cx);
            })),
            ..PluginInstance::empty()
        }
    });
    r.command(
        ID,
        CommandSpec::new_output("ocr", mandara_i18n::tk("Read the text in an image file (PP-OCRv5; auto, latin or arabic)"), |a, _| {
            let path = std::path::PathBuf::from(a.get(0).ok_or("expected an image path")?);
            let script = mandara_services::ocr::Script::parse(a.get(1).unwrap_or("auto"));
            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let result = mandara_plugins::guard("ocr command", || -> Result<String, String> {
                    let img = image::open(&path).map_err(|e| format!("{}: {e}", path.display()))?.to_rgba8();
                    let t = std::time::Instant::now();
                    let text = mandara_services::ocr::recognize(&img, script)?;
                    tracing::info!("ocr of {}×{} took {:?}", img.width(), img.height(), t.elapsed());
                    Ok(text)
                })
                .unwrap_or_else(|| Err("internal error (logged)".into()));
                let _ = tx.send_blocking(result);
            });
            Ok(Output::Pending(rx))
        })
        .arg(ArgSpec::required("path", ArgKind::String, mandara_i18n::tk("PNG / JPEG / WebP file")))
        .arg(ArgSpec::optional("script", ArgKind::Enum, mandara_i18n::tk("which recognizer(s)")).choices(&["auto", "latin", "arabic"]))
        .example("ocr ~/Pictures/Screenshots/shot.png arabic"),
    );
    r.command(
        ID,
        CommandSpec::new_output("record", mandara_i18n::tk("Screen recording: start (screen), region (pick an area), stop, status"), |a, cx| {
            match a.get(0).unwrap_or("status") {
                "start" | "screen" => {
                    crate::recorder::start(String::new(), None, std::time::Duration::ZERO, cx)?;
                    Ok(Output::Value(serde_json::json!({ "recording": "starting" })))
                }
                "region" => {
                    crate::snip::open_for_recording(cx);
                    Ok(Output::Value(serde_json::json!("select an area to record")))
                }
                "stop" => Ok(Output::Pending(crate::recorder::stop(cx)?)),
                "status" => {
                    let r = crate::recorder::recorder(cx);
                    let (clock, busy, path, frames) = r
                        .map(|r| {
                            let r = r.read(cx);
                            (r.clock(), r.busy, r.active.as_ref().map(|a| a.path.display().to_string()), r.active.as_ref().map(|a| a.frames()))
                        })
                        .unwrap_or((None, false, None, None));
                    Ok(Output::Value(serde_json::json!({ "recording": clock.is_some(), "elapsed": clock, "busy": busy, "path": path, "frames": frames })))
                }
                other => Err(format!("unknown action {other:?}")),
            }
        })
        .arg(ArgSpec::optional("action", ArgKind::Enum, mandara_i18n::tk("what to do (default: status)")).choices(&["start", "screen", "region", "stop", "status"]))
        .example("record start")
        .example("record stop"),
    );
    r.command(
        ID,
        CommandSpec::new_output("screenshot", mandara_i18n::tk("Capture: region/pick-window open the snipping tool; window/screen capture immediately; --delay [N] counts down first (Esc cancels)"), |a, cx| {
            let countdown = mandara_config::workflow::ScreenshotSettings::from_plugins(&config(cx).read(cx).get().plugins).countdown_secs();
            let delay = parse_delay(a.0.get(1..).unwrap_or_default(), countdown)?;
            Ok(match start_delayed(a.str(0)?, delay, cx)? {
                Some(rx) => Output::Pending(rx),
                None => Output::Value(serde_json::json!("snipping tool opened")),
            })
        })
        .arg(ArgSpec::required("mode", ArgKind::Enum, mandara_i18n::tk("what to capture")).choices(&["region", "pick-window", "window", "screen"]))
        .arg(ArgSpec::optional("delay", ArgKind::String, mandara_i18n::tk("--delay [seconds]: count down first (default: the Countdown setting)")))
        .example("screenshot screen")
        .example("screenshot region --delay 3"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn delay_flag_forms() {
        assert_eq!(parse_delay(&words(""), 3), Ok(None));
        assert_eq!(parse_delay(&words("--delay"), 3), Ok(Some(3)), "no number: the countdown setting");
        assert_eq!(parse_delay(&words("--delay 5"), 3), Ok(Some(5)));
        assert_eq!(parse_delay(&words("--delay=2"), 3), Ok(Some(2)));
        assert_eq!(parse_delay(&words("--delay 0"), 3), Ok(Some(0)), "0 captures at once");
        assert_eq!(parse_delay(&words("--delay 999"), 3), Ok(Some(MAX_DELAY_S)));
        assert!(parse_delay(&words("--delay=soon"), 3).is_err());
        assert!(parse_delay(&words("now"), 3).is_err());
    }
}
