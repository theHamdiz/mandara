use super::*;

#[test]
fn existing_configs_follow_the_shell_until_a_studio_look_is_selected() {
    let config: ShellConfig = toml::from_str("").unwrap();
    assert!(config.studio.is_none());
    assert!(std::ptr::eq(config.studio_typography(), &config.typography));
    assert!(!toml::to_string(&config).unwrap().contains("[studio"));
}

#[test]
fn every_studio_look_survives_shell_look_changes_and_serialization() {
    for preset in StudioLook::ALL {
        let mut config = ShellConfig {
            studio: Some(Box::new(preset.appearance())),
            ..Default::default()
        };
        let selected = config.studio.clone();
        for look in crate::looks::LOOKS {
            config = look.apply(&config, crate::looks::LookOptions::default());
            assert_eq!(config.studio, selected, "{} changed Studio", look.id);
        }
        let restored: ShellConfig = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
        assert_eq!(restored.studio, selected);
        assert_eq!(
            restored.studio_typography().ui_font,
            preset.definition().ui_font
        );
    }
}

#[test]
fn preset_measurements_fit_the_frame_bounds() {
    for preset in StudioLook::ALL {
        let appearance = preset.appearance();
        assert_eq!(appearance.frame, appearance.frame.bounded());
        assert_eq!(appearance.look, preset);
        assert_eq!(appearance.theme.mode.is_dark(), preset.definition().dark);
        assert!(
            appearance
                .theme
                .source()
                .resolve(appearance.theme.mode.is_dark())
                .is_ok()
        );
    }
}

#[test]
fn invalid_frame_measurements_cannot_break_layout() {
    let frame = StudioFrame {
        inset: f32::NAN,
        radius: f32::INFINITY,
        sidebar_width: -1.0,
        title_height: f32::MAX,
        ..Default::default()
    }
    .bounded();
    assert_eq!(frame.inset, StudioFrame::default().inset);
    assert_eq!(frame.radius, StudioFrame::default().radius);
    assert_eq!(frame.sidebar_width, ranges::SIDEBAR_WIDTH.min);
    assert_eq!(frame.title_height, ranges::TITLE_HEIGHT.max);
}

#[test]
fn studio_appearance_is_absent_without_a_heap_allocation_by_default() {
    assert_eq!(
        std::mem::size_of::<Option<Box<StudioAppearance>>>(),
        std::mem::size_of::<usize>()
    );
    assert!(ShellConfig::default().studio.is_none());
}
