use super::*;

#[test]
fn existing_configs_follow_the_shell_until_a_studio_look_is_selected() {
    let config: ShellConfig = toml::from_str("").unwrap();
    assert!(config.studio.is_none());
    assert!(std::ptr::eq(config.studio_typography(), &config.typography));
    assert!(!toml::to_string(&config).unwrap().contains("[studio"));
}

#[test]
fn command_names_match_persisted_ids_and_reject_unknown_looks() {
    for look in StudioLook::ALL {
        assert_eq!(StudioLook::parse(look.slug()), Some(look));
        assert_eq!(StudioLook::parse(&look.slug().to_uppercase()), Some(look));
        assert_eq!(serde_json::to_value(look).unwrap(), look.slug());
    }
    assert!(StudioLook::parse("uninstalled-look").is_none());
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
fn bold_studio_looks_keep_the_reading_surface_clear() {
    let comic = StudioLook::Comic.appearance();
    assert_eq!(
        comic.style.ink.texture,
        crate::desktop::SurfaceTexture::None
    );
    assert!(comic.style.ink.outline <= comic.frame.border);
    assert!(comic.style.ink.shadow < comic.frame.nav_radius);
    for look in [StudioLook::Arcade, StudioLook::Telemetry] {
        let frame = look.appearance().frame;
        assert!(frame.border < comic.frame.border);
        assert!(frame.title_height < comic.frame.title_height);
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
