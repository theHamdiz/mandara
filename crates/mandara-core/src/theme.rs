//! GPUI integration of the theme engine: a shared `Entity<ThemeEngine>`
//! registered as a global so every surface (bar, island, Studio) renders from
//! the same live palette.

use std::time::Instant;

use gpui::{App, AppContext as _, Entity, Global, Window};
use mandara_theme::{MandaraPalette, PaletteAdjustments, ThemeEngine, ThemeSource};
mod layers;
use layers::ThemeLayers;

/// Global handle to the shell-wide theme entity.
#[derive(Clone)]
pub struct GlobalTheme {
    layers: ThemeLayers<Entity<ThemeEngine>>,
}

impl Global for GlobalTheme {}

/// Creates the theme entity and installs it as a global.
pub fn init_theme(
    source: ThemeSource,
    dark: bool,
    adjustments: PaletteAdjustments,
    cx: &mut App,
) -> Entity<ThemeEngine> {
    let entity = cx.new(|_| ThemeEngine::new(source, dark, adjustments));
    cx.set_global(GlobalTheme {
        layers: ThemeLayers {
            shell: entity.clone(),
            presentation: None,
        },
    });
    entity
}

pub fn theme_entity(cx: &App) -> Entity<ThemeEngine> {
    cx.global::<GlobalTheme>().layers.shell.clone()
}

/// Studio may draw its own palette while desktop previews retain the shell theme.
pub fn set_presentation_theme(theme: Option<Entity<ThemeEngine>>, cx: &mut App) {
    cx.global_mut::<GlobalTheme>().layers.presentation = theme;
    cx.refresh_windows();
}

/// Samples the palette for the frame being rendered and requests another
/// animation frame while a crossfade is running. Call from `render`.
pub fn frame_palette(window: &mut Window, cx: &mut App) -> MandaraPalette {
    let now = Instant::now();
    let entity = cx.global::<GlobalTheme>().layers.frame();
    let (palette, animating) = entity.read(cx).sample(now);
    if animating {
        window.request_animation_frame();
    }
    palette
}
