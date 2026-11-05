//! Default devices in the sound panel and sidebar, backed by the shared mixer tap.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px,
};
use mandara_config::alerts::AudioConfig;
use mandara_core::ToGpui;
use mandara_i18n::{t, tf};
use mandara_services::audio::{self, Device};
use mandara_theme::MandaraPalette;

use super::apps::Snapshot;
use crate::bidi::Bidi as _;
use crate::kit::{text, tokens};

/// Device rows fit a comfortable pointer target in the compact sidebar.
const ROW_H: f32 = 32.0;
/// Keep long device lists inside the panel; the rest remain scrollable.
const VISIBLE_ROWS: usize = 4;
/// One wrapped service error, with room for a retry below the device name.
const ERROR_H: f32 = 40.0;
/// Selection and hover use the same low tonal fill as other audio controls.
const SELECTED_ALPHA: f32 = 0.14;
const HOVER_ALPHA: f32 = 0.07;
/// Keep the selected fill's text contrast; underlining marks keyboard focus.
const FOCUS_ALPHA: f32 = SELECTED_ALPHA;
/// Picker arrows stay smaller than the device label.
const ICON_PX: f32 = 12.0;
/// Rotate the bundled horizontal chevron for the two disclosure states.
const DISCLOSURE_ICON: &str = "chevron-right";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Output,
    Input,
}

impl Kind {
    /// Keep microphone devices separate from playback outputs.
    fn list(self, snapshot: &Snapshot) -> &[Device] {
        match self {
            Self::Output => &snapshot.outputs,
            Self::Input => &snapshot.inputs,
        }
    }

    /// Describe the control independently of a hardware-provided device name.
    fn label(self) -> &'static str {
        match self {
            Self::Output => t("Output device"),
            Self::Input => t("Input device"),
        }
    }
}

#[derive(Default)]
pub(super) struct State {
    open: Option<Kind>,
    busy: bool,
    error: Option<String>,
    generation: u64,
}

impl State {
    /// Hidden panels release their expanded list and any stale failure text.
    pub fn close(&mut self) {
        self.open = None;
        self.error = None;
        self.generation = self.generation.wrapping_add(1);
    }

    /// Opening or closing a list invalidates replies from its previous use.
    fn toggle(&mut self, kind: Kind) {
        let next = (self.open != Some(kind)).then_some(kind);
        self.close();
        self.open = next;
    }

    /// Release the write guard even when its view has since been replaced.
    fn finish(&mut self, kind: Kind, generation: u64, result: Result<(), String>) {
        self.busy = false;
        if self.generation == generation && self.open == Some(kind) {
            match result {
                Ok(()) => self.close(),
                Err(error) => self.error = Some(error),
            }
        }
    }
}

pub(super) struct Controls {
    pub name: AnyElement,
    pub list: Option<AnyElement>,
    pub height: f32,
}

/// Respect Studio's hidden-device choices without reordering server identities.
fn visible<'a>(list: &'a [Device], cfg: &'a AudioConfig) -> impl Iterator<Item = &'a Device> {
    list.iter().filter(|device| !cfg.is_hidden(&device.name))
}

/// Recheck after a hotplug event; never send a name that has disappeared.
fn apply(
    list: &[Device],
    name: &str,
    set: impl FnOnce(&str) -> Result<(), String>,
) -> Result<(), String> {
    let device = list
        .iter()
        .find(|device| device.name == name)
        .ok_or_else(|| tf("No device matching \"{query}\"", &[("query", &name)]).to_string())?;
    if device.is_default {
        Ok(())
    } else {
        set(&device.name)
    }
}

/// Device enumeration and server writes stay off the UI thread. Only one
/// selection is in flight; closing a view does not reopen it on a late failure.
fn select(state: &Rc<RefCell<State>>, kind: Kind, name: String, cx: &mut App) {
    if state.borrow().busy {
        return;
    }
    let generation = {
        let mut state = state.borrow_mut();
        state.busy = true;
        state.error = None;
        state.generation
    };
    let state = state.clone();
    cx.spawn(async move |cx| {
        let result = cx
            .background_executor()
            .spawn(async move {
                let mixer = audio::mixer()?;
                match kind {
                    Kind::Output => apply(&mixer.outputs, &name, audio::set_default_output),
                    Kind::Input => apply(&mixer.inputs, &name, audio::set_default_input),
                }
            })
            .await;
        cx.update(|cx| {
            state.borrow_mut().finish(kind, generation, result);
            cx.refresh_windows();
        });
    })
    .detach();
}

/// The current device opens a bounded list in place. Keeping the controls
/// inside the parent surface preserves its input mask and dismissal behavior.
pub(super) fn controls(
    id: &'static str,
    kind: Kind,
    current: SharedString,
    snapshot: &Snapshot,
    state: &Rc<RefCell<State>>,
    p: &MandaraPalette,
    cx: &App,
) -> Controls {
    let config = crate::state::config(cx).read(cx).get();
    let mut devices = visible(kind.list(snapshot), &config.audio);
    let first = devices.next();
    let count = usize::from(first.is_some()) + devices.count();
    let can_switch = count > 1 || first.is_some_and(|device| !device.is_default);
    let open = state.borrow().open == Some(kind);
    let busy = state.borrow().busy;
    let error = open.then(|| state.borrow().error.clone()).flatten();
    let name = if can_switch || open {
        let click = state.clone();
        let keys = state.clone();
        div()
            .id(id)
            .role(gpui::Role::Button)
            .aria_label(kind.label())
            .aria_expanded(open)
            .focusable()
            .tab_index(0)
            .min_w_0()
            .row()
            .items_center()
            .gap(px(tokens::XS))
            .cursor_pointer()
            .focus(|style| style.bg(p.primary.with_alpha(FOCUS_ALPHA).hsla()).underline())
            .text_color(p.on_surface.hsla())
            .child(div().flex_1().min_w_0().truncate().child(current))
            .child(div().flex_none().child(gpui::svg()
                .path(format!("icons/{DISCLOSURE_ICON}.svg"))
                .size(px(ICON_PX))
                .text_color(p.primary.hsla())
                .with_transformation(gpui::Transformation::rotate(gpui::radians(
                    if open { -std::f32::consts::FRAC_PI_2 } else { std::f32::consts::FRAC_PI_2 },
                )))))
            .on_click(move |_, window, _| {
                click.borrow_mut().toggle(kind);
                window.refresh();
            })
            .on_key_down(move |event, window, cx| {
                if event.keystroke.key != "escape" { return; }
                keys.borrow_mut().close();
                cx.stop_propagation();
                window.refresh();
            })
            .into_any_element()
    } else {
        div().truncate().child(current).into_any_element()
    };
    if !open {
        return Controls {
            name,
            list: None,
            height: 0.0,
        };
    }
    let list_h = count.clamp(1, VISIBLE_ROWS) as f32 * ROW_H;
    let mut rows = div()
        .id(SharedString::from(format!("{id}-list")))
        .h(px(list_h))
        .flex_none()
        .flex()
        .flex_col()
        .overflow_y_scroll();
    for device in visible(kind.list(snapshot), &config.audio) {
        let label = config
            .audio
            .device_label(&device.name, &device.description)
            .to_string();
        let click_name = device.name.clone();
        let (click, keys) = (state.clone(), state.clone());
        let active = device.is_default;
        let tint = if active { p.primary } else { p.on_surface };
        let hover = p.on_surface.with_alpha(HOVER_ALPHA).hsla();
        rows = rows.child(
            div()
                .id(SharedString::from(format!("{id}-{}", device.name)))
                .role(gpui::Role::Button)
                .aria_label(label.clone())
                .aria_selected(active)
                .focusable()
                .tab_index(0)
                .h(px(ROW_H))
                .flex_none()
                .px(px(tokens::XS))
                .rounded(px(tokens::SM))
                .row()
                .items_center()
                .gap(px(tokens::SM))
                .cursor_pointer()
                .when(active, |row| {
                    row.bg(p.primary.with_alpha(SELECTED_ALPHA).hsla())
                })
                .hover(move |style| style.bg(hover))
                .focus(|style| style.bg(p.primary.with_alpha(FOCUS_ALPHA).hsla()).underline())
                .when(busy, |row| row.opacity(super::UNAVAILABLE_OPACITY))
                .text_color(p.on_surface.hsla())
                .text_size(px(text::BODY_S))
                .font_weight(if active {
                    FontWeight::MEDIUM
                } else {
                    FontWeight::NORMAL
                })
                .child(crate::icons::icon(
                    if active {
                        "check"
                    } else if kind == Kind::Output {
                        "headphones"
                    } else {
                        "mic"
                    },
                    ICON_PX,
                    tint,
                ))
                .child(div().flex_1().min_w_0().truncate().child(label))
                .on_click(move |_, _, cx| select(&click, kind, click_name.clone(), cx))
                .on_key_down(move |event, window, cx| {
                    if event.keystroke.key == "escape" {
                        cx.stop_propagation();
                        keys.borrow_mut().close();
                        window.refresh();
                    }
                }),
        );
    }
    if count == 0 {
        rows = rows.child(
            div()
                .h(px(ROW_H))
                .flex_none()
                .text_size(px(text::BODY_S))
                .child(match kind {
                    Kind::Output => t("No output devices"),
                    Kind::Input => t("No input devices"),
                }),
        );
    }
    let height = list_h + tokens::SM + if error.is_some() { ERROR_H } else { 0.0 };
    let list = div()
        .pt(px(tokens::SM))
        .flex_none()
        .flex()
        .flex_col()
        .child(rows)
        .when_some(error, |list, error| {
            list.child(
                div()
                    .h(px(ERROR_H))
                    .text_size(px(text::LABEL))
                    .text_color(super::super::panel_fx::readable(p.warning_amber, p).hsla())
                    .child(crate::bidi::paragraph(error)),
            )
        })
        .into_any_element();
    Controls {
        name,
        list: Some(list),
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn device(name: &str, description: &str, is_default: bool) -> Device {
        Device {
            name: name.into(),
            description: description.into(),
            is_default,
            ..Default::default()
        }
    }

    #[test]
    fn choices_keep_labels_defaults_hidden_policy_and_server_identity_separate() {
        let devices = [
            device("speakers", "Built-in", true),
            device("headset", "USB", false),
            device("hidden", "Virtual", false),
        ];
        let mut cfg = AudioConfig::default();
        cfg.hidden_devices.push("hidden".into());
        cfg.device_labels
            .insert("headset".into(), "Desk headphones".into());
        let visible: Vec<_> = visible(&devices, &cfg).collect();
        assert_eq!(
            visible.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            ["speakers", "headset"]
        );
        assert!(visible[0].is_default);
        assert_eq!(
            cfg.device_label(&visible[1].name, &visible[1].description),
            "Desk headphones"
        );
        assert_eq!(visible[1].name, "headset");
    }

    #[test]
    fn a_disappeared_device_never_reaches_the_writer_and_default_is_a_noop() {
        let devices = [
            device("speakers", "Built-in", true),
            device("headset", "USB", false),
        ];
        let writes = Cell::new(0);
        assert!(
            apply(&devices, "missing", |_| {
                writes.set(writes.get() + 1);
                Ok(())
            })
            .is_err()
        );
        apply(&devices, "speakers", |_| {
            writes.set(writes.get() + 1);
            Ok(())
        })
        .unwrap();
        assert_eq!(writes.get(), 0);
        apply(&devices, "headset", |name| {
            assert_eq!(name, "headset");
            writes.set(writes.get() + 1);
            Ok(())
        })
        .unwrap();
        assert_eq!(writes.get(), 1);
        assert_eq!(
            apply(&devices, "headset", |_| Err("server disconnected".into())),
            Err("server disconnected".into())
        );
    }

    #[test]
    fn input_and_output_lists_remain_distinct() {
        let snapshot = Snapshot {
            outputs: vec![device("speaker", "Speaker", true)],
            inputs: vec![device("microphone", "Mic", true)],
            ..Default::default()
        };
        assert_eq!(Kind::Output.list(&snapshot)[0].name, "speaker");
        assert_eq!(Kind::Input.list(&snapshot)[0].name, "microphone");
        let mut state = State {
            open: Some(Kind::Input),
            busy: true,
            error: Some("old failure".into()),
            ..Default::default()
        };
        state.close();
        assert!(state.open.is_none());
        assert!(state.error.is_none());
        assert!(state.busy);
    }

    #[test]
    fn a_late_reply_cannot_change_a_reopened_picker() {
        for result in [Ok(()), Err("old failure".into())] {
            let mut state = State::default();
            state.toggle(Kind::Output);
            state.busy = true;
            let generation = state.generation;
            state.close();
            state.toggle(Kind::Output);
            state.finish(Kind::Output, generation, result);
            assert_eq!(state.open, Some(Kind::Output));
            assert!(state.error.is_none());
            assert!(!state.busy);
        }
    }

    #[test]
    fn only_the_current_selection_reports_its_result() {
        let mut state = State::default();
        state.toggle(Kind::Input);
        state.busy = true;
        state.finish(Kind::Input, state.generation, Err("server disconnected".into()));
        assert_eq!(state.error.as_deref(), Some("server disconnected"));
        assert_eq!(state.open, Some(Kind::Input));
        assert!(!state.busy);
        state.busy = true;
        state.finish(Kind::Input, state.generation, Ok(()));
        assert!(state.open.is_none());
        assert!(state.error.is_none());
        assert!(!state.busy);
    }

    #[test]
    fn focused_and_selected_device_labels_keep_aa_contrast() {
        for theme in mandara_theme::catalog::builtin() {
            for dark in [false, true] {
                let palette = theme.palette(dark);
                for surface in [palette.surface_base, palette.surface_container, palette.surface_container_low] {
                    for alpha in [SELECTED_ALPHA, FOCUS_ALPHA] {
                        let background = palette.primary.with_alpha(alpha).over(surface);
                        assert!(palette.on_surface.contrast_ratio(background) >= 4.5, "{} {dark} {alpha}", theme.id);
                    }
                }
            }
        }
    }

    #[test]
    fn choices_only_reference_bundled_icons() {
        for name in [DISCLOSURE_ICON, "check", "headphones", "mic"] {
            assert!(crate::icons::svg_source(name).is_some(), "missing picker icon {name}");
        }
    }
}
