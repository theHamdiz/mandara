//! The screen/window capture entry point remains usable by crate consumers.

#[test]
fn capture_keeps_its_public_boolean_signature() {
    let _: fn(bool, &mut gpui::App) -> async_channel::Receiver<Result<String, String>> = mandara_ui::snip::capture_now;
}
