//! The screen/window capture entry point remains usable by crate consumers.

#[test]
fn capture_keeps_its_public_boolean_signature() {
    let _: fn(bool, &mut gpui::App) -> async_channel::Receiver<Result<String, String>> = mandara_ui::snip::capture_now;
}

#[test]
fn panel_capture_is_a_typed_host_command_for_any_plugin() {
    let mut registry = mandara_plugins::PluginRegistry::default();
    mandara_ui::plugins::register_builtin(&mut registry);
    for id in ["clock", "media", "third-party-panel"] {
        let words = vec!["panel".into(), "capture".into(), id.into()];
        assert_eq!(registry.commands.resolve(&words).unwrap().spec.path, "panel capture");
    }
    assert!(registry.commands.resolve(&["panel".into(), "capture".into()]).is_err());
}
