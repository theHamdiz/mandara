//! GPUI integration of the theme engine: a shared `Entity<ThemeEngine>`
//! registered as a global so every surface (bar, island, Studio) renders from
//! the same live palette.

use std::time::Instant;

use gpui::{App, AppContext as _, Entity, Global, Window};
use mandara_theme::{MandaraPalette, PaletteAdjustments, ThemeEngine, ThemeSource};

/// Global handle to the shell-wide theme entity.
#[derive(Clone)]
pub struct GlobalTheme(pub Entity<ThemeEngine>);

impl Global for GlobalTheme {}

/// Creates the theme entity and installs it as a global.
pub fn init_theme(source: ThemeSource, dark: bool, adjustments: PaletteAdjustments, cx: &mut App) -> Entity<ThemeEngine> {
    let entity = cx.new(|_| ThemeEngine::new(source, dark, adjustments));
    cx.set_global(GlobalTheme(entity.clone()));
    entity
}

pub fn theme_entity(cx: &App) -> Entity<ThemeEngine> {
    cx.global::<GlobalTheme>().0.clone()
}

/// Samples the palette for the frame being rendered and requests another
/// animation frame while a crossfade is running. Call from `render`.
pub fn frame_palette(window: &mut Window, cx: &mut App) -> MandaraPalette {
    let now = Instant::now();
    let entity = theme_entity(cx);
    let (palette, animating) = entity.read(cx).sample(now);
    if animating {
        window.request_animation_frame();
    }
    palette
}
