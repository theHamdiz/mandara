//! Previews the polkit authentication dialog without a running shell or a
//! real polkit request: `cargo run -p mandara-ui --example polkit_dialog [seconds]`.
//! Any password but "wrong" "authenticates"; Esc cancels. Closes itself
//! after the given number of seconds (default 8).

use std::time::Duration;

use gpui::App;

fn main() {
    let seconds: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(8);
    let cfg = mandara_config::ConfigStore::open_default().map(|s| s.snapshot()).unwrap_or_default();
    mandara_i18n::set_language(cfg.general.locale.lang());
    gpui_platform::application().with_assets(mandara_ui::MandaraAssets).run(move |cx: &mut App| {
        cx.set_global(mandara_ui::Fonts::resolve(&cfg.typography, cx));
        mandara_core::init_theme(cfg.theme.source.clone(), cfg.theme.mode.is_dark(), cfg.theme.adjustments, cx);
        mandara_ui::polkit::open_demo(cx);
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_secs(seconds)).await;
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}
