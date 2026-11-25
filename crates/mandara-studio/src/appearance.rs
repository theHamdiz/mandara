//! Studio's palette is independent; desktop previews keep the desktop engine.

use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Entity, Global};
use mandara_config::ShellConfig;
use mandara_theme::{Color, SchemeVariant, ThemeEngine, ThemeSource};

use crate::latest::Latest;

#[derive(Clone, PartialEq)]
struct PaletteKey {
    color: [u8; 4],
    variant: SchemeVariant,
    dark: bool,
}

impl PaletteKey {
    fn of(config: &ShellConfig) -> Option<Self> {
        let studio = config.studio.as_deref()?;
        Some(Self {
            color: studio.theme.color.to_rgba8(),
            variant: studio.theme.variant,
            dark: studio.theme.mode.is_dark(),
        })
    }

    fn source(&self) -> ThemeSource {
        let [r, g, b, a] = self.color;
        ThemeSource::Seed {
            color: Color::from_rgba8(r, g, b, a),
            variant: self.variant,
        }
    }
}

#[derive(Default)]
struct Presentation {
    requests: Latest<PaletteKey>,
    engine: Option<Entity<ThemeEngine>>,
    duration: Duration,
    error: Option<String>,
}
impl Global for Presentation {}

/// Keep transitions bounded even when a config was edited by hand.
const MAX_TRANSITION_MS: u64 = 5_000;

fn duration(config: &ShellConfig) -> Duration {
    let Some(studio) = config.studio.as_deref() else {
        return Duration::ZERO;
    };
    if mandara_ui::style::studio_motion(config)
        .reduced(mandara_core::spring::system_reduced_motion())
    {
        Duration::ZERO
    } else {
        Duration::from_millis(studio.theme.transition_ms.min(MAX_TRANSITION_MS))
    }
}

pub(crate) fn error(cx: &App) -> Option<&str> {
    cx.try_global::<Presentation>()
        .and_then(|state| state.error.as_deref())
}

pub(crate) fn retry(cx: &mut App) {
    cx.global_mut::<Presentation>().requests.refresh();
    drive(cx);
}

pub(crate) fn init(cx: &mut App) {
    cx.set_global(Presentation::default());
    sync(cx);
    let config = mandara_ui::state::config(cx);
    cx.observe(&config, |_, cx| sync(cx)).detach();
}

fn sync(cx: &mut App) {
    let config = mandara_ui::state::config(cx).read(cx).snapshot();
    let key = PaletteKey::of(&config);
    cx.global_mut::<Presentation>().requests.set(key.clone());
    if key.is_none() {
        let previous = cx.global_mut::<Presentation>().engine.take();
        cx.global_mut::<Presentation>().error = None;
        if previous.is_some() {
            mandara_core::set_presentation_theme(None, cx);
        }
        return;
    }
    if let Some(engine) = cx.global::<Presentation>().engine.clone()
        && let Some(studio) = config.studio.as_deref()
    {
        let duration = duration(&config);
        if *engine.read(cx).adjustments() != studio.theme.adjustments
            || cx.global::<Presentation>().duration != duration
        {
            engine.update(cx, |engine, cx| {
                engine.set_duration(duration);
                engine.set_adjustments(studio.theme.adjustments, false, Instant::now());
                cx.notify();
            });
            cx.global_mut::<Presentation>().duration = duration;
            cx.refresh_windows();
        }
    }
    drive(cx);
}

fn drive(cx: &mut App) {
    let Some(request) = cx.global_mut::<Presentation>().requests.begin() else {
        return;
    };
    let source = request.key.source();
    let dark = request.key.dark;
    let job = cx.background_executor().spawn({
        let source = source.clone();
        async move { source.resolve(dark) }
    });
    cx.spawn(async move |cx| {
        let result = job.await;
        cx.update(|cx| {
            if cx.global_mut::<Presentation>().requests.finish(request) {
                match result {
                    Ok(resolved) => {
                        let config = mandara_ui::state::config(cx).read(cx).snapshot();
                        if let Some(studio) = config.studio.as_deref() {
                            let duration = duration(&config);
                            let engine = if let Some(engine) =
                                cx.global::<Presentation>().engine.clone()
                            {
                                engine.update(cx, |engine, cx| {
                                    engine.set_duration(duration);
                                    engine.set_adjustments(
                                        studio.theme.adjustments,
                                        false,
                                        Instant::now(),
                                    );
                                    engine.apply_resolved(source, dark, resolved, Instant::now());
                                    cx.notify();
                                });
                                engine
                            } else {
                                cx.new(|_| {
                                    let mut engine = ThemeEngine::from_resolved(
                                        source,
                                        dark,
                                        studio.theme.adjustments,
                                        resolved,
                                    );
                                    engine.set_duration(duration);
                                    engine
                                })
                            };
                            let state = cx.global_mut::<Presentation>();
                            state.engine = Some(engine.clone());
                            state.duration = duration;
                            state.error = None;
                            mandara_core::set_presentation_theme(Some(engine), cx);
                        }
                    }
                    Err(error) => {
                        cx.global_mut::<Presentation>().error = Some(format!("{error:#}"));
                        cx.refresh_windows();
                    }
                }
            }
            drive(cx);
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::studio::StudioLook;

    #[test]
    fn palette_identity_ignores_desktop_appearance_but_tracks_studio_changes() {
        let mut config = ShellConfig {
            studio: Some(Box::new(StudioLook::Yaru.appearance())),
            ..Default::default()
        };
        let original = PaletteKey::of(&config).unwrap();
        for look in mandara_config::looks::LOOKS {
            config = look.apply(&config, Default::default());
            assert!(PaletteKey::of(&config).as_ref() == Some(&original));
        }
        config.studio.as_mut().unwrap().theme.mode = mandara_config::ThemeMode::Dark;
        assert!(PaletteKey::of(&config).as_ref() != Some(&original));
        config.studio = None;
        assert!(PaletteKey::of(&config).is_none());
    }

    #[test]
    fn studio_transitions_respect_accessibility_and_duration_bounds() {
        let mut config = ShellConfig {
            studio: Some(Box::new(StudioLook::Arcade.appearance())),
            ..Default::default()
        };
        config.studio.as_mut().unwrap().theme.transition_ms = u64::MAX;
        assert!(duration(&config) <= Duration::from_millis(MAX_TRANSITION_MS));
        config.motion.reduce_motion = true;
        assert_eq!(duration(&config), Duration::ZERO);
    }
}
