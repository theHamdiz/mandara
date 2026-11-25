//! Draw the real Power gauge with labelled sample data in the private sandbox.
//! `cargo xtask sandbox run` can launch this example as its configured binary.

use gpui::{
    App, AppContext as _, Bounds, Context, FontWeight, IntoElement, ParentElement as _, Render,
    Styled as _, TitlebarOptions, Window, WindowBounds, WindowOptions, div, px, size,
};
use mandara_core::ToGpui as _;
use mandara_theme::MandaraPalette;
use std::{path::Path, time::Duration};

/// Match the actual gauge's percentage typography for an honest comparison.
const FIGURE_PX: f32 = 46.0;
/// Room around the real, unscaled gauge and the explicit preview label.
const WIDTH_PX: f32 = 360.0;
const HEIGHT_PX: f32 = 320.0;
const GAP_PX: f32 = 20.0;
/// Long enough to capture a frame; the example always closes itself.
const LIFETIME: Duration = Duration::from_secs(12);
/// The reported charge level, expressed as a fraction for the production painter.
const CHARGE: f32 = 0.93;

/// Refuse an unmarked runtime or a marker belonging to another checkout.
fn verify_runtime(runtime: &Path, expected: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(runtime.starts_with("/tmp"), "use cargo xtask sandbox");
    let marker = std::fs::read_to_string(runtime.join(".mandara-sandbox"))?;
    anyhow::ensure!(
        Path::new(marker.trim()).canonicalize()? == expected.canonicalize()?,
        "use this checkout's cargo xtask sandbox"
    );
    Ok(())
}

struct Preview {
    palette: MandaraPalette,
}

impl Render for Preview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let p = &self.palette;
        let inner = div()
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .text_size(px(FIGURE_PX))
                    .font_weight(FontWeight::LIGHT)
                    .child("93%"),
            )
            .child(
                div()
                    .text_color(p.on_surface_variant.hsla())
                    .child(mandara_i18n::t("On battery")),
            )
            .into_any_element();
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(GAP_PX))
            .bg(p.surface_base.hsla())
            .text_color(p.on_surface.hsla())
            .child(mandara_i18n::t("Preview"))
            .child(mandara_ui::plugins::power::charge_gauge(
                CHARGE,
                (p.primary, p.secondary),
                p,
                inner,
            ))
    }
}

/// Open the production gauge painter only inside the marked private sandbox, then exit automatically.
fn main() -> anyhow::Result<()> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .ok_or_else(|| anyhow::anyhow!("use cargo xtask sandbox"))?;
    verify_runtime(
        Path::new(&runtime),
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/sandbox"),
    )?;
    let dark = !std::env::args().any(|arg| arg == "--light");
    gpui_platform::application()
        .with_assets(mandara_ui::MandaraAssets)
        .run(move |cx: &mut App| {
            cx.set_global(mandara_ui::Fonts::resolve(
                &mandara_config::TypographyConfig::default(),
                cx,
            ));
            let bounds = Bounds::centered(None, size(px(WIDTH_PX), px(HEIGHT_PX)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    app_id: Some("org.mandara.power-preview".into()),
                    titlebar: Some(TitlebarOptions {
                        title: Some(mandara_i18n::t("Preview").into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|_| Preview {
                        palette: mandara_theme::engine::default_palette(dark),
                    })
                },
            )
            .expect("preview window");
            cx.spawn(async move |cx| {
                cx.background_executor().timer(LIFETIME).await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preview_requires_this_checkouts_private_marker() {
        let runtime = tempfile::tempdir_in("/tmp").unwrap();
        let root = tempfile::tempdir().unwrap();
        assert!(verify_runtime(runtime.path(), root.path()).is_err());
        std::fs::write(
            runtime.path().join(".mandara-sandbox"),
            root.path().to_string_lossy().as_bytes(),
        )
        .unwrap();
        assert!(verify_runtime(runtime.path(), root.path()).is_ok());
        let other = tempfile::tempdir().unwrap();
        assert!(verify_runtime(runtime.path(), other.path()).is_err());
        assert!(verify_runtime(Path::new("/run/user/1000"), root.path()).is_err());
    }
}
