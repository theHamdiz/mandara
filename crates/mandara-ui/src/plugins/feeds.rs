//! A small subscription reader: real headlines, conditional requests and
//! local read history. Only the selected, visible feed refreshes.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{AnyElement, App, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, px, size};
use mandara_config::ShellConfig;
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::feeds::{Cache, HttpSource, Snapshot};
use mandara_theme::MandaraPalette;

use super::fn_view;
use super::integration::{self, Data};
use super::panel_fx::{EXPIRY_MARGIN, glass_card, header, pill_button, readable, redraw_in};
use crate::bidi::Bidi as _;
use crate::icons::icon;
use crate::kit::{alpha, metrics, text, tokens};

pub const ID: &str = "feeds";
/// Six readable headline cards fit before the list scrolls.
const PANEL_W: f32 = 430.0;
const PANEL_H: f32 = 520.0;
const ROW_H: f32 = 68.0;
const BAR_ICON_PX: f32 = 16.0;
/// Keep subscription selectors compact and bound pasted configuration.
pub const MAX_SUBSCRIPTIONS: usize = 16;
pub const DEFAULT_REFRESH_SECS: i64 = 300;
pub const MIN_REFRESH_SECS: i64 = 60;
pub const MAX_REFRESH_SECS: i64 = 3600;

type FeedData = Rc<RefCell<Data<String, Snapshot>>>;

#[derive(Default)]
struct Reader {
    selected: usize,
    data: FeedData,
    /// Read actions can finish before an older refresh. Keep their visible
    /// state until a later snapshot includes the persisted flags.
    read: HashMap<String, Arc<BTreeSet<String>>>,
}

thread_local! { static READER: Rc<RefCell<Reader>> = Rc::default(); }
fn reader() -> Rc<RefCell<Reader>> { READER.with(Rc::clone) }

/// Keep distinct feed addresses in configured order within the list limit.
pub fn subscriptions(cfg: &ShellConfig) -> Result<Vec<String>, String> {
    let text = cfg.plugins.setting(ID, "subscriptions", String::new());
    let mut urls = Vec::new();
    for url in text.split_whitespace() {
        if !urls.iter().any(|seen| seen == url) { urls.push(url.to_owned()); }
        if urls.len() > MAX_SUBSCRIPTIONS { return Err(t("Keep up to 16 feed subscriptions").into()); }
    }
    Ok(urls)
}

fn refresh_seconds(cfg: &ShellConfig) -> Duration {
    Duration::from_secs(cfg.plugins.setting(ID, "refresh_seconds", DEFAULT_REFRESH_SECS).clamp(MIN_REFRESH_SECS, MAX_REFRESH_SECS) as u64)
}

/// Refresh through the cache, retaining conditional-request state and read flags.
fn refresh(url: String, force: bool, cfg: &ShellConfig, cx: &mut App) {
    let data = reader().borrow().data.clone();
    let address = url.clone();
    integration::load(&data, url, force, refresh_seconds(cfg), move |previous| Cache::default().refresh(&HttpSource::default(), &address, previous.as_deref()), cx);
}

fn setup(p: &MandaraPalette) -> AnyElement {
    pill_button("feeds-settings", Some("sliders"), t("Settings"), readable(p.primary, p), p,
        |_, cx| crate::state::open_studio_at(Some("plugins/feeds/settings".into()), cx))
        .focusable().tab_index(0).into_any_element()
}

/// Mark an article read only after its validated web address opens successfully.
fn open_article(url: String, link: String, cx: &mut App) {
    let data = reader().borrow().data.clone();
    cx.spawn(async move |cx| {
        let address = url.clone();
        let article = link.clone();
        let result = mandara_services::runtime::unblock(move || {
            if !mandara_services::feeds::web_url(&article) { return Err(t("This article has no web address").to_string()); }
            mandara_services::session::open_uri(&article).map_err(|error| error.to_string())?;
            Cache::default().mark_read(&address, &article)
        }).await;
        let views = {
            let mut data = data.borrow_mut();
            if data.value.as_ref().is_some_and(|snapshot| snapshot.feed.url == url) {
                match result {
                    Ok(_) => { Arc::make_mut(reader().borrow_mut().read.entry(url).or_default()).insert(link); }
                    Err(error) => data.error = Some(error),
                }
            }
            data.views.clone()
        };
        views.notify_async(cx);
    }).detach();
}

/// Refresh the selected feed while this panel is visible and keep cached headlines.
fn panel(context: &mandara_plugins::PanelCx, p: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let cfg = crate::state::config(cx).read(cx).snapshot();
    let subscriptions = subscriptions(&cfg);
    let r = reader();
    let (selected, data) = { let reader = r.borrow(); (reader.selected, reader.data.clone()) };
    data.borrow().views.track(window);
    let urls = subscriptions.as_deref().unwrap_or_default();
    let selected = selected.min(urls.len().saturating_sub(1));
    let url = urls.get(selected).cloned();
    data.borrow_mut().select(url.clone().unwrap_or_default());
    if context.progress() > 0.0 && let Some(url) = &url {
        refresh(url.clone(), false, &cfg, cx);
        redraw_in(refresh_seconds(&cfg) + EXPIRY_MARGIN, window, cx);
    }
    let model = data.borrow();
    let snapshot = model.value.as_deref().filter(|snapshot| Some(&snapshot.feed.url) == url.as_ref());
    let local_read = {
        let mut reader = r.borrow_mut();
        reader.read.retain(|url, _| urls.contains(url));
        if let Some(snapshot) = snapshot && let Some(read) = reader.read.get_mut(&snapshot.feed.url) {
            Arc::make_mut(read).retain(|link| snapshot.feed.entries.iter().any(|entry| &entry.link == link));
        }
        url.as_ref().and_then(|url| reader.read.get(url)).cloned().unwrap_or_default()
    };
    let subtitle = snapshot.map(|snapshot| tf("{count} unread", &[("count", &snapshot.feed.entries.iter().filter(|entry| !snapshot.read.contains(&entry.link) && !local_read.contains(&entry.link)).count())])).unwrap_or_else(|| t("Your subscriptions, together").into());
    let mut column = div().size_full().p(px(tokens::LG)).row().flex_col().gap(px(tokens::MD))
        .child(header("rss", readable(p.primary, p), t("Feeds"), subtitle, p, Some(setup(p))));
    if let Some(url) = url.clone() {
        let go = url.clone();
        column = column.child(div().row().items_center().gap(px(tokens::SM))
            .child(div().flex_1().min_w_0().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).truncate()
                .child(snapshot.map(|snapshot| snapshot.feed.title.clone()).unwrap_or(url)))
            .child(pill_button("feeds-refresh", Some("refresh"), if model.busy() { t("Refreshing…") } else { t("Refresh") }, readable(p.primary, p), p, move |_, cx| {
                let cfg = crate::state::config(cx).read(cx).snapshot();
                refresh(go.clone(), true, &cfg, cx);
            }).focusable().tab_index(0)));
    }
    if urls.len() > 1 {
        column = column.child(div().row().gap(px(tokens::XS)).flex_wrap().children(urls.iter().enumerate().map(|(index, _)| {
            pill_button(SharedString::from(format!("feed-{index}")), None, (index + 1).to_string(), if index == selected { readable(p.primary, p) } else { p.on_surface_variant }, p, move |_, cx| {
                let reader = reader();
                reader.borrow_mut().selected = index;
                let views = reader.borrow().data.borrow().views.clone();
                views.notify(cx);
            }).focusable().tab_index(0)
        })));
    }
    let problem = subscriptions.as_ref().err().cloned().or_else(|| model.error.clone()).or_else(|| snapshot.and_then(|snapshot| snapshot.problem.clone()));
    if let Some(problem) = problem {
        column = column.child(glass_card(p).p(px(tokens::SM)).text_size(px(text::BODY_S)).text_color(readable(p.warning_amber, p).hsla()).child(crate::bidi::paragraph(problem)));
    }
    let content = match snapshot {
        Some(snapshot) if !snapshot.feed.entries.is_empty() => div().id("feed-headlines").flex_1().min_h_0().overflow_y_scroll().row().flex_col().gap(px(tokens::SM))
            .children(snapshot.feed.entries.iter().enumerate().map(|(index, entry)| {
                let read = snapshot.read.contains(&entry.link) || local_read.contains(&entry.link);
                let url = snapshot.feed.url.clone();
                let link = entry.link.clone();
                glass_card(p).id(SharedString::from(format!("feed-article-{index}"))).focusable().tab_index(0)
                    .min_h(px(ROW_H)).flex_none().p(px(tokens::MD)).row().items_center().gap(px(tokens::MD)).cursor_pointer()
                    .hover(|style| style.bg(p.primary.with_alpha(alpha::HOVER).hsla()))
                    .focus_visible(|style| style.bg(p.primary.with_alpha(alpha::SELECTED).hsla()))
                    .child(icon(if read { "check" } else { "rss" }, metrics::TILE_ICON, if read { p.on_surface_variant } else { readable(p.primary, p) }))
                    .child(div().flex_1().min_w_0().text_size(px(text::BODY)).font_weight(if read { FontWeight::NORMAL } else { FontWeight::MEDIUM }).line_clamp(2).child(crate::bidi::paragraph(entry.title.clone())))
                    .on_click(move |_, _, cx| open_article(url.clone(), link.clone(), cx))
            })).into_any_element(),
        _ => div().flex_1().row().flex_col().items_center().justify_center().gap(px(tokens::SM))
            .child(icon("rss", metrics::TILE_H, p.on_surface_variant))
            .child(if urls.is_empty() { t("Add your favourite feeds in Studio") } else if model.busy() { t("Fetching headlines…") } else { t("No headlines yet") }).into_any_element(),
    };
    column.child(content).into_any_element()
}

/// Register commands and lazy views without reading feeds during startup.
pub fn register(registry: &mut PluginRegistry) {
    registry.register(PluginManifest::builtin(ID, tk("Feeds"), tk("RSS, Atom and JSON subscriptions with local read history"), Some(Zone::Right)).opt_in(), |window, cx, context| {
        let bar = fn_view(None, |p, _, _| icon("rss", crate::style::bar_icon(BAR_ICON_PX), readable(p.primary, p)).into_any_element(), window, cx);
        let context = context.clone();
        let view = fn_view(None, move |p, window, cx| panel(&context, p, window, cx), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view, size: Rc::new(Cell::new(size(px(PANEL_W), px(PANEL_H)))) }), ..PluginInstance::empty() }
    });
    registry.command(ID, CommandSpec::new_output("feeds refresh", tk("Refresh a feed and return its headlines and read history"), |args, _| {
        let url = args.str(0)?.to_owned();
        Ok(crate::powerctl::pending(move || Cache::default().refresh(&HttpSource::default(), &url, None).and_then(|snapshot| serde_json::to_value(snapshot).map_err(|error| error.to_string()))))
    }).arg(ArgSpec::required("url", ArgKind::String, tk("RSS, Atom or JSON feed address"))).example("feeds refresh https://example.org/feed.xml"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscription_settings_keep_order_remove_duplicates_and_bound_the_list() {
        let mut cfg = ShellConfig::default();
        cfg.plugins.settings.entry(ID.into()).or_default().insert("subscriptions".into(), toml::Value::String("https://one.example/feed\nhttps://two.example/feed https://one.example/feed".into()));
        assert_eq!(subscriptions(&cfg).unwrap(), ["https://one.example/feed", "https://two.example/feed"]);
        cfg.plugins.settings.get_mut(ID).unwrap().insert("subscriptions".into(), toml::Value::String((0..=MAX_SUBSCRIPTIONS).map(|n| format!("https://example.org/{n}")).collect::<Vec<_>>().join(" ")));
        assert!(subscriptions(&cfg).is_err());
        cfg.plugins.settings.get_mut(ID).unwrap().insert("refresh_seconds".into(), toml::Value::Integer(0));
        assert_eq!(refresh_seconds(&cfg), Duration::from_secs(MIN_REFRESH_SECS as u64));
    }
}
