//! Mandara Login — an animated, hyper-modern greetd greeter.
//!
//! * [`run`] opens the login screen: a full-screen GPUI window (inside a
//!   kiosk compositor such as `cage`) or, for `--demo`, an overlay layer
//!   surface on the running session.
//! * Backgrounds are AV1 clips decoded in pure Rust ([`media`]), a Ken
//!   Burns slideshow, or a generated aurora ([`background`]).
//! * [`greetd`] speaks greetd's IPC and drives multi-step PAM logins.
//! * [`install`] registers it as the system's display manager;
//!   [`install_probe`] reports its state to `mandara install`.
//!
//! The demo never contacts greetd, never checks the password against the
//! system, never starts a session and never calls power actions.

pub mod assets;
pub mod auth;
pub mod background;
pub mod cli;
pub mod config;
pub mod greetd;
pub mod i18n;
pub mod install;
pub mod install_probe;
pub mod keyring_pam;
pub mod manager;
pub mod media;
pub mod pacing;
pub mod power;
pub mod sessions;
pub mod users;
pub mod view;

use anyhow::{Context as _, Result, bail};
use gpui::layer_shell::KeyboardInteractivity;
use gpui::{App, AppContext as _, Bounds, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, point, px, size};
use mandara_core::{LayerSurfaceSpec, SurfaceRole};

use crate::config::GreeterConfig;

/// Wayland `app_id` of the greeter window.
pub const GREETER_APP_ID: &str = "org.mandara.greeter";

#[derive(Clone, Debug, Default)]
pub struct GreeterOptions {
    /// Overlay on the current session; nothing is authenticated or started.
    pub demo: bool,
    /// Force an overlay layer surface (default: demo only) instead of a
    /// full-screen toplevel window.
    pub layer_shell: Option<bool>,
    /// Override the configured language (`ar`, `en`, …).
    pub language: Option<String>,
    /// Override the configured theme id.
    pub theme: Option<String>,
    /// Override the backdrop (`auto`, `video`, `stills`, `aurora`).
    pub background: Option<String>,
    /// Demo only: play a scripted wrong-then-right login (for screenshots).
    pub autoplay: bool,
}

/// Opens the login screen and runs until a session starts (real mode) or
/// the demo is closed.
pub fn run(opts: GreeterOptions) -> Result<()> {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        bail!("the greeter needs a Wayland compositor (WAYLAND_DISPLAY is not set); greetd should start it inside one, e.g. `cage -s -- mandara greeter`");
    }
    if !opts.demo && std::env::var_os("GREETD_SOCK").is_none() {
        bail!("GREETD_SOCK is not set: `mandara greeter` must be started by greetd. Try `mandara greeter --demo` to preview it on this session.");
    }
    let mut config = GreeterConfig::load(opts.demo);
    if let Some(lang) = &opts.language {
        config.language = lang.clone();
    }
    if let Some(theme) = &opts.theme {
        config.theme.preset = theme.clone();
    }
    if let Some(bg) = &opts.background {
        config.background.mode = config::BackgroundMode::parse(bg).with_context(|| format!("unknown background {bg:?} (auto, video, stills, aurora, solid)"))?;
    }
    let autoplay = opts.autoplay && opts.demo;
    mandara_i18n::set_language(config.lang());
    let layer = opts.layer_shell.unwrap_or(opts.demo);
    let outputs = if layer { mandara_core::query_outputs().unwrap_or_default() } else { Vec::new() };
    let demo = opts.demo;
    tracing::info!("mandara greeter: {} ({} mode, theme {}, language {})", if demo { "demo" } else { "greetd" }, if layer { "overlay" } else { "fullscreen" }, config.theme.preset, config.lang().code());

    gpui_platform::application().with_assets(assets::GreeterAssets).run(move |cx: &mut App| {
        cx.set_global(mandara_ui::Fonts::resolve(&mandara_config::TypographyConfig::default(), cx));
        let options = if layer {
            let display = cx.displays().first().map(|d| d.id());
            let (w, h) = outputs
                .first()
                .map(|g| (g.width as f32, g.height as f32))
                .or_else(|| cx.displays().first().map(|d| (f32::from(d.bounds().size.width), f32::from(d.bounds().size.height))))
                .unwrap_or((1920.0, 1080.0));
            // The scripted self-test never takes the keyboard (or, see the
            // view, the pointer): someone may be using the machine.
            let keyboard = if autoplay { KeyboardInteractivity::None } else { KeyboardInteractivity::Exclusive };
            let mut spec = LayerSurfaceSpec::overlay(SurfaceRole::Lock, size(px(w.max(320.0)), px(h.max(240.0))), display).with_keyboard(keyboard);
            spec.role = SurfaceRole::Lock;
            let mut o = spec.window_options();
            o.focus = true;
            o.window_background = WindowBackgroundAppearance::Opaque;
            o
        } else {
            let bounds = Bounds { origin: point(px(0.0), px(0.0)), size: size(px(1920.0), px(1080.0)) };
            WindowOptions {
                titlebar: None,
                window_bounds: Some(WindowBounds::Fullscreen(bounds)),
                focus: true,
                show: true,
                kind: WindowKind::Normal,
                is_movable: false,
                is_resizable: true,
                is_minimizable: false,
                window_background: WindowBackgroundAppearance::Opaque,
                app_id: Some(GREETER_APP_ID.to_string()),
                inactive_frame_interval: None,
                ..Default::default()
            }
        };
        let config = config.clone();
        if let Err(e) = cx.open_window(options, move |window, cx| cx.new(|cx| view::GreeterView::new_live(demo, autoplay, config, window, cx))) {
            tracing::error!("greeter window: {e:#}");
            cx.quit();
        }
    });
    Ok(())
}
