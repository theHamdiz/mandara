//! The keyboard bridge: lets you type into a floating card without the
//! floating surface ever taking the keyboard.
//!
//! The `mandara-floating` surface is created with no keyboard
//! interactivity, so clicking a card leaves your window's keyboard focus
//! alone, and layer surfaces can't change that after creation. While a
//! text field (or another keyboard-driven element) inside a card is
//! focused by a click, this one-pixel, fully click-through surface exists
//! on the Top layer with *exclusive* keyboard interactivity: the compositor
//! sends it every key, and it forwards each to the floating surface's
//! focused element ([`super::forward_key`]). It is closed as soon as
//! typing ends (Esc, a click outside the cards or elsewhere in the card,
//! the card closing) or something else takes the keyboard.

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer};
use gpui::{AnyWindowHandle, App, AppContext as _, Context, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, Render, Styled, Task, Window, div};
use mandara_core::SurfaceRole;

use crate::dismiss::DEACTIVATE_SETTLE;

/// Side of the bridge surface (never 0: a zero size trips GPUI's viewport).
const BRIDGE_PX: f32 = 1.0;
/// Placed against the whole output, like the floating surface.
const IGNORE_ZONES: f32 = -1.0;

pub struct BridgeView {
    output: String,
    focus: FocusHandle,
    /// Pending confirmation that the keyboard really left.
    settle: Option<Task<()>>,
    /// The empty input region was set.
    masked: bool,
}

impl BridgeView {
    fn new(output: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        // Losing the keyboard for good (another exclusive surface, a lock)
        // ends typing; a brief leave / enter pair does not.
        cx.observe_window_activation(window, |this: &mut Self, window, cx| {
            if window.is_window_active() {
                this.settle = None;
                return;
            }
            let output = this.output.clone();
            this.settle = Some(cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(DEACTIVATE_SETTLE).await;
                let _ = this.update_in(cx, |this, window, cx| {
                    this.settle = None;
                    if !window.is_window_active() {
                        cx.defer(move |cx| super::keyboard_lost(&output, cx));
                    }
                });
            }));
        })
        .detach();
        Self { output, focus, settle: None, masked: false }
    }
}

impl Render for BridgeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.masked {
            // Never takes a click: only the keyboard.
            window.set_input_region(Some(&[]));
            self.masked = true;
        }
        div().size_full().track_focus(&self.focus).on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
            cx.stop_propagation();
            let (output, keystroke) = (this.output.clone(), e.keystroke.clone());
            cx.defer(move |cx| super::forward_key(&output, keystroke, cx));
        }))
    }
}

/// Opens the bridge typing into `output`'s floating surface.
pub(super) fn open(output: &str, cx: &mut App) -> Option<AnyWindowHandle> {
    let options = crate::plugins::dock::layer_options(
        SurfaceRole::Floating,
        Layer::Top,
        Anchor::TOP | Anchor::LEFT,
        (BRIDGE_PX, BRIDGE_PX),
        Some(IGNORE_ZONES),
        KeyboardInteractivity::Exclusive,
        crate::outputs::display_for(output, cx),
    );
    let name = output.to_string();
    match cx.open_window(options, move |window, cx| cx.new(|cx| BridgeView::new(name, window, cx))) {
        Ok(h) => Some(h.into()),
        Err(err) => {
            tracing::warn!("floating panels: no keyboard bridge: {err:#}");
            None
        }
    }
}
