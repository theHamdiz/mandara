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
///
/// A wallpaper source is decoded to find its colours, which takes long
/// enough to hold up the first frame: it is resolved in the background and
/// crossfaded in, with the default palette showing until then.
pub fn init_theme(
    source: ThemeSource,
    dark: bool,
    adjustments: PaletteAdjustments,
    cx: &mut App,
) -> Entity<ThemeEngine> {
    let wallpaper = matches!(source, ThemeSource::Wallpaper { .. });
    let entity = cx.new(|_| {
        if wallpaper {
            ThemeEngine::provisional(source.clone(), dark, adjustments)
        } else {
            ThemeEngine::new(source.clone(), dark, adjustments)
        }
    });
    cx.set_global(GlobalTheme {
        layers: ThemeLayers {
            shell: entity.clone(),
            presentation: None,
        },
    });
    if wallpaper {
        let job = cx.background_executor().spawn({
            let source = source.clone();
            async move { source.resolve(dark) }
        });
        let engine = entity.clone();
        cx.spawn(async move |cx| match job.await {
            Ok(resolved) => {
                engine.update(cx, |engine, cx| {
                    // A config change while this ran has its own resolve under way.
                    if *engine.source() == source && engine.is_dark() == dark {
                        engine.apply_resolved(source, dark, resolved, Instant::now());
                        cx.notify();
                    }
                });
            }
            Err(err) => tracing::warn!("wallpaper theme failed ({err:#}); keeping Mandara Corniche"),
        })
        .detach();
    }
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
