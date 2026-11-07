use std::cell::Cell;

use mandara_config::PluginsConfig;

fn settings() -> PluginsConfig {
    let mut config = PluginsConfig::default();
    config.settings.insert("example".into(), toml::toml! {
        place = "Alexandria"
        count = 7
        enabled = true
        list = ["one", "two"]
    });
    config
}

#[test]
fn valid_setting_does_not_construct_its_fallback() {
    let config = settings();
    assert_eq!(config.setting_or_else("example", "place", || -> String { panic!("valid setting used its fallback") }), "Alexandria");
    assert_eq!(config.setting_or_else("example", "count", || -> i64 { panic!("valid setting used its fallback") }), 7);
    assert!(config.setting_or_else("example", "enabled", || -> bool { panic!("valid setting used its fallback") }));
    assert_eq!(config.setting_or_else("example", "list", || -> Vec<String> { panic!("valid setting used its fallback") }), ["one", "two"]);
}

#[test]
fn missing_or_invalid_settings_construct_one_fallback() {
    let config = settings();
    for (plugin, key) in [("missing", "place"), ("example", "missing"), ("example", "count")] {
        let calls = Cell::new(0);
        let value = config.setting_or_else(plugin, key, || {
            calls.set(calls.get() + 1);
            "fallback".to_string()
        });
        assert_eq!(value, "fallback");
        assert_eq!(calls.get(), 1);
    }
}

#[test]
fn borrowed_setting_keeps_the_snapshot_string() {
    let mut config = settings();
    let stored = config.settings["example"]["place"].as_str().unwrap();
    let borrowed = config.setting_str("example", "place").unwrap();
    assert!(std::ptr::eq(stored, borrowed));
    assert_eq!(config.setting_str("example", "count"), None);
    assert_eq!(config.setting_str("example", "missing"), None);
    assert_eq!(config.setting_str("missing", "place"), None);
    config.settings.get_mut("example").unwrap().insert("place".into(), "Cairo".into());
    assert_eq!(config.setting_str("example", "place"), Some("Cairo"));
}

#[test]
fn eager_setting_keeps_its_existing_conversion_contract() {
    let config = settings();
    assert_eq!(config.setting("example", "place", String::new()), "Alexandria");
    assert_eq!(config.setting("example", "count", 0_u8), 7);
    assert_eq!(config.setting("example", "place", 12_u8), 12);
    assert_eq!(config.setting("example", "list", Vec::<String>::new()), ["one", "two"]);
}
