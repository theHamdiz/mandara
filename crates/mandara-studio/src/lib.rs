//! Mandara Studio — the live settings and design app (`org.mandara.studio`),
//! an `xdg_toplevel` running as its own process (`mandara studio`). Every
//! change is mirrored to the running shell over IPC, so it previews
//! instantly on the real shell, and Studio's own CPU / memory never count
//! against the shell's.

pub mod app;
pub mod maintenance;
pub mod perf;
pub mod registry;
pub mod text_field;
pub mod tokens;
pub mod ui_state;

use gpui::{App, AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowHandle, WindowKind, WindowOptions, point, px, size};

pub use app::StudioView;
pub use perf::{ShellIpc, ShellLink, ShellQuery};

#[derive(Default)]
struct StudioWindow(Option<WindowHandle<StudioView>>);
impl gpui::Global for StudioWindow {}

/// A plugin modal to open: `plugins/<id>` (details) or
/// `plugins/<id>/settings`, as passed by `mandara studio plugins --plugin
/// <id> [--settings]`.
fn plugin_target(target: &str) -> Option<(String, app::plugin_modal::ModalView)> {
    let mut parts = target.split('/');
    let (_, id) = (parts.next().filter(|first| first.eq_ignore_ascii_case("plugins"))?, parts.next().filter(|id| !id.is_empty() && app::plugins_local::PluginsTab::parse(id).is_none())?);
    let view = if parts.next() == Some("settings") { app::plugin_modal::ModalView::Settings } else { app::plugin_modal::ModalView::Detail };
    Some((id.to_string(), view))
}

/// A Plugins tab to show: `plugins/<discover|installed|updates|marketplace|local>` with an
/// optional Local-tab section to scroll to (`plugins/local/contract`), as
/// passed by `mandara studio plugins --tab local [--scroll contract]`;
/// `marketplace` alone is a shortcut for `plugins/marketplace`.
fn tab_target(target: &str) -> Option<(app::plugins_local::PluginsTab, Option<app::plugins_local::LocalSection>)> {
    use app::plugins_local::{LocalSection, PluginsTab};
    let mut parts = target.split('/');
    let first = parts.next()?;
    if let Some(tab) = PluginsTab::parse(first).filter(|t| *t == PluginsTab::Marketplace) {
        return Some((tab, None));
    }
    if !first.eq_ignore_ascii_case("plugins") {
        return None;
    }
    let tab = PluginsTab::parse(parts.next()?)?;
    Some((tab, parts.next().and_then(LocalSection::parse)))
}

/// An Applications tab to show: `applications/<installed|defaults|startup|launching>`,
/// as passed by `mandara studio applications --tab startup`.
fn apps_tab_target(target: &str) -> Option<app::apps_page::AppsTab> {
    let (section, tab) = target.split_once('/')?;
    (resolve_section(section) == Some(registry::Section::Applications)).then_some(())?;
    app::apps_page::AppsTab::parse(tab.split('/').next()?)
}

/// Names of pages merged into another one, still accepted on the command
/// line (`mandara studio appearance` opens Themes).
const FORMER_SECTIONS: &[(&str, registry::Section)] = &[("appearance", registry::Section::Themes), ("looks", registry::Section::Themes), ("cursor", registry::Section::Themes), ("cursors", registry::Section::Themes)];

/// Whether a command-line target opens Themes at its Cursor section
/// (`themes/cursor`, or the moved `cursor` / `input/cursor`).
fn cursor_target(target: &str) -> bool {
    let t = target.to_lowercase();
    matches!(t.as_str(), "cursor" | "cursors" | "themes/cursor" | "input/cursor" | "appearance/cursor")
}

/// The section a command-line name opens: the English title (any case, or
/// with dashes for spaces: `login-screen`), its translation, a former page
/// name ([`FORMER_SECTIONS`]) or a title prefix (`sidebar`);
/// `plugins/<id>[/settings]` opens Plugins.
pub fn resolve_section(target: &str) -> Option<registry::Section> {
    let name = target.split('/').next().unwrap_or_default().to_lowercase();
    if name.is_empty() {
        return None;
    }
    if let Some((_, section)) = FORMER_SECTIONS.iter().find(|(former, _)| *former == name) {
        return Some(*section);
    }
    let all = registry::Section::ALL;
    all.into_iter()
        .find(|s| s.title().to_lowercase() == name || s.title().to_lowercase().replace(' ', "-") == name || s.label().to_lowercase() == name)
        .or_else(|| all.into_iter().find(|s| s.title().to_lowercase().starts_with(&name)))
}

/// Opens Studio on a section (by English title, case-insensitive, or its
/// translation) — or on a plugin's modal (`plugins/<id>[/settings]`) — or
/// focuses it.
pub fn open_at(section: Option<&str>, cx: &mut App) {
    let plugin = section.and_then(plugin_target);
    let tab = section.and_then(tab_target);
    let apps_tab = section.and_then(apps_tab_target);
    // `power/<part>` scrolls the Power & idle page to that part.
    let power_part = section.and_then(|t| t.strip_prefix("power/")).map(str::to_string);
    let cursor = section.is_some_and(cursor_target);
    let section = if tab.is_some() { Some(registry::Section::Plugins) } else { section.and_then(resolve_section) };
    let show = move |view: &mut StudioView, window: &mut gpui::Window, cx: &mut gpui::Context<StudioView>| {
        if let Some(s) = section {
            view.show(s, cx);
        }
        if let Some((tab, scroll)) = tab {
            view.show_plugins_tab(tab, scroll, cx);
        }
        if let Some(tab) = apps_tab {
            view.show_apps_tab(tab, cx);
        }
        if let Some((id, which)) = &plugin {
            view.open_plugin(id, *which, window, cx);
        }
        if let Some(part) = &power_part {
            view.show_power_part(part, cx);
        }
        if cursor {
            view.show_cursor_section(cx);
        }
    };
    if let Some(handle) = cx.try_global::<StudioWindow>().and_then(|w| w.0)
        && handle
            .update(cx, |view, window, cx| {
                show(view, window, cx);
                window.activate_window();
            })
            .is_ok()
    {
        return;
    }
    if let Some(handle) = open_window(cx) {
        let _ = handle.update(cx, |view, window, cx| show(view, window, cx));
    }
}

/// Opens Studio, or focuses it when already open.
pub fn open(cx: &mut App) {
    open_at(None, cx);
}

/// Default window size.
pub(crate) const DEFAULT_W_PX: f32 = 1120.0;
const DEFAULT_H_PX: f32 = 780.0;
/// Smallest window (the icon rail and stacked rows still fit).
const MIN_W_PX: f32 = 460.0;
const MIN_H_PX: f32 = 420.0;

/// 1120×780, or `MANDARA_STUDIO_SIZE=WxH` (screenshots, layout testing).
fn initial_size() -> gpui::Size<gpui::Pixels> {
    std::env::var("MANDARA_STUDIO_SIZE")
        .ok()
        .and_then(|v| v.split_once('x').and_then(|(w, h)| Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?))))
        .map(|(w, h)| size(px(w.max(MIN_W_PX)), px(h.max(MIN_H_PX))))
        .unwrap_or(size(px(DEFAULT_W_PX), px(DEFAULT_H_PX)))
}

fn open_window(cx: &mut App) -> Option<WindowHandle<StudioView>> {
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions { title: Some(mandara_i18n::t("Mandara Studio").into()), appears_transparent: true, traffic_light_position: None }),
        window_bounds: Some(WindowBounds::Windowed(Bounds { origin: point(px(0.0), px(0.0)), size: initial_size() })),
        focus: true,
        show: true,
        kind: WindowKind::Normal,
        app_id: Some(mandara_core::STUDIO_APP_ID.to_string()),
        window_min_size: Some(size(px(MIN_W_PX), px(MIN_H_PX))),
        // Full frame rate while unfocused too: the live previews (login
        // screen, springs) are often watched beside another window. Studio
        // only redraws while something animates, so idle cost is unchanged.
        inactive_frame_interval: None,
        ..Default::default()
    };
    match cx.open_window(options, |_, cx| cx.new(StudioView::new)) {
        Ok(handle) => {
            cx.set_global(StudioWindow(Some(handle)));
            Some(handle)
        }
        Err(err) => {
            tracing::error!("opening Mandara Studio: {err:#}");
            None
        }
    }
}

/// Opens the Studio window and quits the process once it closes.
pub fn run(section: Option<&str>, cx: &mut App) {
    cx.set_global(StudioWindow::default());
    mandara_ui::state::init_language(cx);
    // Studio's own cards, corners and springs follow the Look & feel
    // settings live, like the shell's.
    mandara_ui::style::init(cx);
    // Links Studio opens (plugin pages, docs) use the browser picked in
    // Applications, like the shell's.
    mandara_ui::plugins::system_admin::init_openers(cx);
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() {
            // A display test's revert timer lives in this process: closing
            // Studio mid-countdown must not make an untested mode stick
            // (the monitor might not show it), so revert now.
            if mandara_services::outputs::revert_remaining().is_some() {
                if let Err(err) = mandara_services::outputs::revert_now() {
                    tracing::warn!("reverting the display test on close: {err}");
                }
            }
            cx.quit();
        }
    })
    .detach();
    open_at(section, cx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use registry::Section;

    /// Every Studio entry of the launcher opens the section it names, and
    /// every section is reachable from the launcher.
    #[test]
    fn launcher_studio_entries_resolve() {
        let mut reached = Vec::new();
        for (slug, title, icon, _) in mandara_ui::overlays::STUDIO_SECTIONS {
            let section = resolve_section(slug).unwrap_or_else(|| panic!("launcher Studio entry `{slug}` opens no section"));
            assert_eq!(section.title(), *title, "launcher entry `{slug}` is titled {title:?} but opens {:?}", section.title());
            assert_eq!(section.icon(), *icon, "launcher entry `{slug}` and the sidebar use different icons");
            reached.push(section);
        }
        for s in Section::ALL {
            assert!(reached.contains(&s), "Studio section {:?} has no launcher entry", s.title());
        }
        // The launcher lists sections in sidebar order.
        assert!(reached.iter().copied().eq(registry::nav_order()), "launcher Studio entries aren't in sidebar order");
    }

    #[test]
    fn plugin_deep_links_parse() {
        assert_eq!(resolve_section("plugins/weather/settings"), Some(Section::Plugins));
        assert_eq!(plugin_target("plugins/weather/settings").map(|(id, v)| (id, v == app::plugin_modal::ModalView::Settings)), Some(("weather".into(), true)));
        assert_eq!(plugin_target("plugins/weather").map(|(id, v)| (id, v == app::plugin_modal::ModalView::Detail)), Some(("weather".into(), true)));
        assert_eq!(resolve_section("Login-Screen"), Some(Section::LoginScreen));
        assert_eq!(resolve_section(""), None);
        assert_eq!(resolve_section("appearance"), Some(Section::Themes), "the merged Appearance page still opens");
    }

    #[test]
    fn cursor_deep_links_open_themes() {
        for target in ["themes/cursor", "cursor", "Cursors", "input/cursor"] {
            assert!(cursor_target(target), "{target}");
        }
        assert_eq!(resolve_section("cursor"), Some(registry::Section::Themes));
        assert_eq!(resolve_section("themes/cursor"), Some(registry::Section::Themes));
        assert!(!cursor_target("themes") && !cursor_target("input"));
    }

    #[test]
    fn power_deep_links_open_the_power_page_only() {
        assert_eq!(resolve_section("power/hooks"), Some(Section::Power));
        assert!(plugin_target("power/hooks").is_none(), "power parts aren't plugin ids");
        assert!(app::power_page::PARTS.contains(&"hooks"));
    }

    #[test]
    fn plugin_tab_deep_links_parse() {
        use app::plugins_local::{LocalSection, PluginsTab};
        assert_eq!(tab_target("plugins/local"), Some((PluginsTab::Local, None)));
        assert_eq!(tab_target("plugins/local/contract"), Some((PluginsTab::Local, Some(LocalSection::Contract))));
        assert_eq!(tab_target("plugins/marketplace"), Some((PluginsTab::Marketplace, None)));
        assert_eq!(tab_target("marketplace"), Some((PluginsTab::Marketplace, None)));
        assert_eq!(tab_target("plugins/discover"), Some((PluginsTab::Discover, None)));
        assert_eq!(tab_target("plugins/updates"), Some((PluginsTab::Updates, None)));
        assert_eq!(tab_target("plugins/installed"), Some((PluginsTab::Installed, None)));
        assert!(plugin_target("plugins/updates").is_none() && plugin_target("plugins/discover").is_none(), "tab names aren't plugin ids");
        use app::apps_page::AppsTab;
        assert_eq!(apps_tab_target("applications/startup"), Some(AppsTab::Startup));
        assert_eq!(apps_tab_target("Applications/installed"), Some(AppsTab::Installed));
        assert_eq!(apps_tab_target("plugins/startup"), None);
        assert_eq!(tab_target("applications/installed"), None);
        assert_eq!(tab_target("plugins/weather"), None);
        assert_eq!(tab_target("themes"), None);
        assert!(plugin_target("plugins/local").is_none(), "tab names aren't plugin ids");
    }
}
