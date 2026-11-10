//! Right-clicks in the gaps of a split bar (`bar.right_click_gaps`).
//!
//! A split bar (glass pills, the island, the micro pill, the notch, and
//! the bar styles built from them) gives the compositor only its pieces as
//! input region, so a right-click between them lands on the desktop. With
//! the setting on, the desktop asks here first: a click in the strip the
//! bar takes along its edge ([`super::layout::band_hit`]) opens the bar's
//! main menu there instead of the desktop's menu. Anywhere else on the
//! desktop the desktop's menu opens as before, and the main menu's
//! Apps & tools page gains a "Desktop menu" tile that opens it where the
//! gap was clicked ([`open_desktop_menu`]).
//!
//! The check runs once per right-click, never per frame: the setting, then
//! the output's shell surface (looked up in place) and the bar metrics it
//! already resolved for its last frame.

use gpui::{App, Global};

use crate::state::{config, shell};

/// The last gap click that opened the main menu: where the "Desktop menu"
/// tile opens the desktop's menu.
#[derive(Default)]
struct LastGap(Option<(String, (f32, f32))>);
impl Global for LastGap {}

/// Whether a right-click on `output` at `at` (output px; the output is
/// `size` px big when the caller knows it) lands in a split bar's gaps
/// while they open the main menu ([`mandara_config::ShellConfig::bar_gaps_open_menu`]),
/// and where along the bar.
pub fn hit(output: &str, at: (f32, f32), size: Option<(f32, f32)>, cx: &App) -> Option<f32> {
    let cfg = config(cx).read(cx).get();
    if !cfg.bar_gaps_open_menu() || !cfg.bar_on_output(output) || super::autohide::hidden_by_command() {
        return None;
    }
    let size = size.unwrap_or_else(|| crate::plugins::dock::output_size(output, cx));
    crate::floating::shell_view(output, cx)?.read(cx).gap_hit(size, at)
}

/// Opens the main menu for a right-click on `output` at `at` when it lands
/// in a split bar's gaps ([`hit`]); `Err` hands the output back, the click
/// being the desktop's.
pub fn route(output: String, at: (f32, f32), size: Option<(f32, f32)>, cx: &mut App) -> Result<(), String> {
    let Some(along) = hit(&output, at, size, cx) else { return Err(output) };
    crate::shell::toggle_main_menu(&output, along, cx);
    cx.default_global::<LastGap>().0 = Some((output, at));
    Ok(())
}

/// The "Desktop menu" tile: closes the main menu and opens the desktop's
/// menu where the gap was right-clicked, or, when the main menu was opened
/// some other way, in the middle of the output it shows on.
pub fn open_desktop_menu(cx: &mut App) {
    let menu = crate::plugins::main_menu::ID;
    let gap = cx.try_global::<LastGap>().and_then(|g| g.0.clone()).filter(|(output, _)| shell(cx).read(cx).panel_on(output) == Some(menu));
    let (output, at) = match gap {
        Some(gap) => gap,
        None => {
            let Some(output) = crate::backdrop::outputs(cx).into_iter().find(|o| shell(cx).read(cx).panel_on(o) == Some(menu)) else { return };
            let (w, h) = crate::plugins::dock::output_size(&output, cx);
            (output, (w / 2.0, h / 2.0))
        }
    };
    shell(cx).update(cx, |s, cx| s.set_panel(None, cx));
    crate::desktop_menu::open(&output, at, cx);
}
