//! Borrow effective styles from the desktop or Studio without cloning config.

use mandara_config::desktop::StyleConfig;
use mandara_config::{MotionConfig, ShellConfig, TypographyConfig};

use super::LookKey;

pub(super) trait StyleSource {
    type Key: PartialEq;
    fn independent_studio(config: &ShellConfig) -> bool;
    fn style(config: &ShellConfig) -> &StyleConfig;
    fn typography(config: &ShellConfig) -> &TypographyConfig;
    fn motion(config: &ShellConfig) -> MotionConfig;
    fn cards(config: &ShellConfig) -> (f32, f32);
    fn icon_scale(config: &ShellConfig) -> f32;
    fn key(config: &ShellConfig) -> Self::Key;
    fn icons_changed(before: &Self::Key, after: &Self::Key) -> bool;
}

pub(super) struct Desktop;
pub(super) struct Studio;

impl StyleSource for Desktop {
    type Key = LookKey;
    fn independent_studio(_: &ShellConfig) -> bool {
        false
    }
    fn style(c: &ShellConfig) -> &StyleConfig {
        &c.style
    }
    fn typography(c: &ShellConfig) -> &TypographyConfig {
        &c.typography
    }
    fn motion(c: &ShellConfig) -> MotionConfig {
        c.motion
    }
    fn cards(c: &ShellConfig) -> (f32, f32) {
        (c.sidebar.look.card_radius, c.sidebar.look.card_padding)
    }
    fn icon_scale(c: &ShellConfig) -> f32 {
        c.bar.icon_scale()
    }
    fn key(c: &ShellConfig) -> Self::Key {
        LookKey::of(c)
    }
    fn icons_changed(before: &Self::Key, after: &Self::Key) -> bool {
        before.appearance.icon_theme != after.appearance.icon_theme
    }
}

#[derive(PartialEq)]
pub(super) struct StudioKey {
    independent: bool,
    style: StyleConfig,
    motion: MotionConfig,
    weight: u16,
    cards: (f32, f32),
    icon_scale: f32,
    icon_theme: Option<String>,
    time_locale: String,
}

impl StyleSource for Studio {
    type Key = StudioKey;
    fn independent_studio(c: &ShellConfig) -> bool {
        c.studio.is_some()
    }
    fn style(c: &ShellConfig) -> &StyleConfig {
        c.studio.as_deref().map_or(&c.style, |s| &s.style)
    }
    fn typography(c: &ShellConfig) -> &TypographyConfig {
        c.studio_typography()
    }
    fn motion(c: &ShellConfig) -> MotionConfig {
        let mut motion = c.studio.as_deref().map_or(c.motion, |s| s.motion);
        // Accessibility remains shared even when the visual recipe is independent.
        motion.reduce_motion |= c.motion.reduce_motion;
        motion.follow_system |= c.motion.follow_system;
        motion
    }
    fn cards(c: &ShellConfig) -> (f32, f32) {
        c.studio.as_deref().map_or_else(
            || Desktop::cards(c),
            |s| {
                let frame = s.frame.bounded();
                (frame.card_radius, frame.card_padding)
            },
        )
    }
    fn icon_scale(c: &ShellConfig) -> f32 {
        // Studio's icons use their control's size, independent of bar icon scaling.
        if c.studio.is_some() {
            1.0
        } else {
            Desktop::icon_scale(c)
        }
    }
    fn key(c: &ShellConfig) -> Self::Key {
        StudioKey {
            independent: Self::independent_studio(c),
            style: Self::style(c).clone(),
            motion: Self::motion(c),
            weight: Self::typography(c).weight,
            cards: Self::cards(c),
            icon_scale: Self::icon_scale(c),
            icon_theme: c.appearance.icon_theme.clone(),
            time_locale: c.datetime.time_locale.clone(),
        }
    }
    fn icons_changed(before: &Self::Key, after: &Self::Key) -> bool {
        before.icon_theme != after.icon_theme
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::studio::StudioLook;

    #[test]
    fn studio_components_and_motion_stay_separate_from_desktop_looks() {
        let config = ShellConfig {
            studio: Some(Box::new(StudioLook::Comic.appearance())),
            ..Default::default()
        };
        let key = Studio::key(&config);
        for look in mandara_config::looks::LOOKS {
            let changed = look.apply(&config, Default::default());
            assert!(Studio::key(&changed) == key, "{} changed Studio", look.id);
            assert_eq!(
                Studio::style(&changed),
                &config.studio.as_ref().unwrap().style
            );
        }
    }

    #[test]
    fn unselected_studio_appearance_and_accessibility_follow_existing_settings() {
        let mut config = ShellConfig::default();
        assert!(std::ptr::eq(
            Studio::style(&config),
            Desktop::style(&config)
        ));
        assert_eq!(Studio::cards(&config), Desktop::cards(&config));
        config.studio = Some(Box::new(StudioLook::Arcade.appearance()));
        config.motion.reduce_motion = true;
        config.motion.follow_system = true;
        assert!(Studio::motion(&config).reduce_motion);
        assert!(Studio::motion(&config).follow_system);
    }
}
