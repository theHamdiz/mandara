//! Reddit: communities, posts and comment threads in a panel, through
//! Reddit's official API with your own installed-app client id.
//!
//! Registration is cheap: nothing connects until the panel shows a list.
//! One worker serves the panel and the commands, one request at a time;
//! nothing refreshes while the panel is hidden (an optional slow refresh
//! runs only while it is open), and closing it releases pictures, then
//! posts and comments.

mod grid;
mod images;
mod model;
mod panel;
mod reader;
mod sidebar;
mod starter;
mod text;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, IntoElement, size, px};
use mandara_config::ShellConfig;
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::reddit::{Query, Sort, Span, Subreddit, Want};

use super::model_view;
use super::panel_fx::readable;
use super::settings_cache::{ConfigSettings, SettingsCache};
use crate::icons::icon;

pub const ID: &str = "reddit";
/// Communities kept in your list.
pub const MAX_SUBSCRIPTIONS: usize = 50;
/// A calm, safe-for-work start; edit it in Studio or from Discover.
pub const DEFAULT_SUBSCRIPTIONS: &str = "popular technology linux science space";
/// The optional refresh of an open list, in minutes (0 is off).
pub const MAX_REFRESH_MINUTES: i64 = 60;
pub const REFRESH_STEP_MINUTES: i64 = 5;
const PANEL_W: f32 = 460.0;
/// The grid widens the panel so three tiles fit side by side.
const GRID_PANEL_W: f32 = 620.0;
const PANEL_H: f32 = 640.0;
const BAR_ICON_PX: f32 = 16.0;

/// How NSFW posts (and, without "hide", spoilers) appear.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate { Hide, Blur, Show }

impl Gate {
    fn parse(text: &str, default: Self) -> Self {
        match text { "hide" => Self::Hide, "blur" => Self::Blur, "show" => Self::Show, _ => default }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout { Cards, Compact, Grid }

impl Layout {
    /// The value saved in the configuration.
    pub fn key(self) -> &'static str { match self { Self::Cards => "cards", Self::Compact => "compact", Self::Grid => "grid" } }

    /// The next layout of the panel's switch.
    pub fn next(self) -> Self { match self { Self::Cards => Self::Compact, Self::Compact => Self::Grid, Self::Grid => Self::Cards } }
}

/// Resolved once per configuration and language.
pub struct Settings {
    pub client_id: Box<str>,
    pub username: Option<Box<str>>,
    pub subscriptions: Vec<Subreddit>,
    /// The sidebar card's community; empty follows your first community.
    pub card: Option<Subreddit>,
    pub sort: Sort,
    pub span: Span,
    pub nsfw: Gate,
    pub spoilers: Gate,
    pub images: bool,
    pub layout: Layout,
    pub refresh: Option<Duration>,
}

/// Read the plugin's settings with their defaults.
pub fn settings(cfg: &ShellConfig) -> Settings {
    let text = |key: &str| cfg.plugins.setting_str(ID, key).unwrap_or("").trim();
    let list = cfg.plugins.setting_str(ID, "subreddits").unwrap_or(DEFAULT_SUBSCRIPTIONS);
    let username = text("username");
    let minutes = cfg.plugins.setting(ID, "refresh_minutes", 0_i64).clamp(0, MAX_REFRESH_MINUTES);
    Settings {
        client_id: text("client_id").into(),
        username: (!username.is_empty()).then(|| username.trim_start_matches("u/").trim_start_matches("/u/").into()),
        subscriptions: parse_subscriptions(list),
        card: Subreddit::parse(text("card_subreddit")),
        sort: Sort::parse(text("sort")).unwrap_or_default(),
        span: Span::parse(text("span")).unwrap_or_default(),
        nsfw: Gate::parse(text("nsfw"), Gate::Hide),
        spoilers: match Gate::parse(text("spoilers"), Gate::Blur) { Gate::Hide => Gate::Blur, gate => gate },
        images: cfg.plugins.setting(ID, "images", true),
        layout: match text("layout") { "compact" => Layout::Compact, "grid" => Layout::Grid, _ => Layout::Cards },
        refresh: (minutes > 0).then(|| Duration::from_secs(minutes as u64 * SECS_PER_MINUTE)),
    }
}

const SECS_PER_MINUTE: u64 = 60;

/// Distinct valid names in order, within the list limit.
pub fn parse_subscriptions(text: &str) -> Vec<Subreddit> {
    let mut list: Vec<Subreddit> = Vec::new();
    for word in text.split([' ', ',', '\n', '\t']).filter(|word| !word.is_empty()) {
        if let Some(sub) = Subreddit::parse(word) && !model::subscribed(&list, sub.as_str()) && list.len() < MAX_SUBSCRIPTIONS { list.push(sub); }
    }
    list
}

impl ConfigSettings for Settings {
    type Scope = ();

    fn resolve(config: &ShellConfig, _: mandara_i18n::Lang, (): ()) -> Self { settings(config) }
}

thread_local! {
    static SETTINGS: RefCell<SettingsCache<Settings>> = const { RefCell::new(SettingsCache::new()) };
}

fn cached(config: &Arc<ShellConfig>) -> Rc<Settings> {
    SETTINGS.with(|cache| cache.borrow_mut().get(config, mandara_i18n::language(), ()))
}

pub(crate) fn current(cx: &App) -> Rc<Settings> { cached(&crate::state::config(cx).read(cx).snapshot()) }

/// Switched off: drop everything it holds, including the token.
pub(super) fn stop() {
    SETTINGS.with(|cache| cache.borrow_mut().clear());
    model::stop();
}

/// Write the subscription list back to the configuration.
fn save_subscriptions(list: &[Subreddit], cx: &mut App) {
    let text = list.iter().map(Subreddit::as_str).collect::<Vec<_>>().join(" ");
    crate::state::update_config(cx, |cfg| { cfg.plugins.settings.entry(ID.into()).or_default().insert("subreddits".into(), toml::Value::String(text)); });
}

/// Add a community to your list (at the end).
pub fn subscribe(name: &str, cx: &mut App) -> Result<(), String> {
    let sub = Subreddit::parse(name).ok_or_else(|| t("Use a community name like r/linux").to_string())?;
    let mut list = current(cx).subscriptions.clone();
    if model::subscribed(&list, sub.as_str()) { return Ok(()); }
    if list.len() >= MAX_SUBSCRIPTIONS { return Err(tf("Keep up to {count} communities", &[("count", &MAX_SUBSCRIPTIONS)])); }
    list.push(sub);
    save_subscriptions(&list, cx);
    Ok(())
}

pub fn unsubscribe(name: &str, cx: &mut App) {
    let mut list = current(cx).subscriptions.clone();
    list.retain(|sub| !sub.same(name));
    save_subscriptions(&list, cx);
}

/// Move a community up (negative) or down in your list.
pub fn move_subscription(name: &str, delta: isize, cx: &mut App) {
    let mut list = current(cx).subscriptions.clone();
    let Some(at) = list.iter().position(|sub| sub.same(name)) else { return };
    let to = (at as isize + delta).clamp(0, list.len() as isize - 1) as usize;
    if to == at { return; }
    let sub = list.remove(at);
    list.insert(to, sub);
    save_subscriptions(&list, cx);
}

/// The panel's width for a layout.
pub(super) fn panel_width(layout: Layout) -> f32 { if layout == Layout::Grid { GRID_PANEL_W } else { PANEL_W } }

/// The panel's switch: the next layout, saved like the Studio setting.
pub(super) fn cycle_layout(cx: &mut App) {
    let next = current(cx).layout.next();
    crate::state::update_config(cx, |cfg| { cfg.plugins.settings.entry(ID.into()).or_default().insert("layout".into(), toml::Value::String(next.key().into())); });
}

/// The preview sizes the panel draws, in physical pixels.
fn want(scale: f32) -> Want {
    Want { thumb: (panel::THUMB_PX * scale).round() as u32, media: (panel::MEDIA_W * scale).round() as u32 }
}

/// Show a community (and the panel). `target` is a name, `r/name`, a
/// community address or a post address.
pub fn open(target: &str, cx: &mut App) -> Result<(), String> {
    let settings = current(cx);
    let shared = model::model();
    let want = shared.borrow().want.unwrap_or(want(1.0));
    if let Some(post) = mandara_services::reddit::post_in_url(target) {
        shared.borrow_mut().open_post(post, want);
    } else {
        let sub = Subreddit::parse(target).ok_or_else(|| t("Use a community name like r/linux, or a Reddit post address").to_string())?;
        let mut m = shared.borrow_mut();
        m.page = model::Page::Posts;
        m.searching = false;
        let query = Query { subreddit: sub, sort: m.feed.query.as_ref().map_or(settings.sort, |q| q.sort), span: m.feed.query.as_ref().map_or(settings.span, |q| q.span), search: None };
        m.show(query, want);
    }
    reveal(&shared, cx)
}

/// Show the panel on Discover, where you find and follow communities.
pub(super) fn discover(cx: &mut App) -> Result<(), String> {
    let shared = model::model();
    { let mut m = shared.borrow_mut(); m.page = model::Page::Discover; m.searching = false; }
    reveal(&shared, cx)
}

/// Show the panel: from its bar button, or (without one: an opt-in plugin
/// starts without one) as a floating card.
fn reveal(shared: &model::Shared, cx: &mut App) -> Result<(), String> {
    let in_bar = { let cfg = crate::state::config(cx).read(cx).snapshot(); [&cfg.bar.left, &cfg.bar.center, &cfg.bar.right].iter().any(|zone| zone.iter().any(|id| id == ID)) };
    if in_bar || crate::floating::is_floating(ID, cx) {
        crate::state::shell(cx).update(cx, |s, cx| s.set_panel(Some(ID.into()), cx));
    } else {
        crate::floating::float(ID, None, None, cx)?;
    }
    let views = shared.borrow().views.clone();
    model::pump(shared, cx);
    views.notify(cx);
    Ok(())
}

/// Follows the shell once per process (sidebar cards are built every time
/// the sidebar opens): what the reader holds goes once the panel closes;
/// nothing is created until the reader is first used.
fn follow_shell(cx: &mut App) {
    thread_local! { static FOLLOWING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
    if FOLLOWING.with(|f| f.replace(true)) { return; }
    cx.observe(&crate::state::shell(cx), |shell, cx| {
        let open = shell.read(cx).panel.as_deref() == Some(ID) || crate::floating::is_floating(ID, cx);
        if let Some(shared) = if open { Some(model::model()) } else { model::existing() } {
            model::Model::follow_panel(&shared, open, cx);
        }
    }).detach();
}

/// The sidebar closed: the card's posts go, and with the panel closed too
/// the session forgets the replies it kept for them.
fn release_card(cx: &mut App) {
    let Some(shared) = model::existing() else { return };
    let open = crate::state::shell(cx).read(cx).panel.as_deref() == Some(ID) || crate::floating::is_floating(ID, cx);
    model::Model::release_card(&shared, open, cx);
}

pub fn register(registry: &mut PluginRegistry) {
    registry.register(PluginManifest::builtin(ID, tk("Reddit"), tk("Browse Reddit communities, posts and comment threads"), Some(Zone::Right)).opt_in(), |window, cx, context| {
        follow_shell(cx);
        let sidebar = model_view(None, sidebar::render, window, cx);
        if context.is_card() {
            return PluginInstance { sidebar: Some(sidebar), ..PluginInstance::empty() };
        }
        let bar = model_view(None, |p, _, _| icon("messages", crate::style::bar_icon(BAR_ICON_PX), readable(p.primary, p)).into_any_element(), window, cx);
        let search = panel::search_field(cx);
        let add = panel::add_field(cx);
        let context = context.clone();
        let panel_size = Rc::new(Cell::new(size(px(PANEL_W), px(PANEL_H))));
        let fields = panel::Fields { search, add, size: panel_size.clone() };
        let view = model_view(None, move |p, window, cx| panel::render(&context, &fields, p, window, cx), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view, size: panel_size }), sidebar: Some(sidebar), ..PluginInstance::empty() }
    });
    // The card's posts live in the shared model: they go when the sidebar closes.
    registry.sidebar_card(ID, mandara_plugins::CardRelease::shared(release_card));
    registry.command(ID, CommandSpec::new("reddit open", tk("Open a community or a post in the Reddit panel"), |args, cx| {
        open(&args.rest(0), cx)?;
        Ok(serde_json::json!({ "opened": args.rest(0) }))
    }).arg(ArgSpec::required("target", ArgKind::String, tk("r/name, a community address or a post address"))).example("reddit open r/linux"));
    registry.command(ID, CommandSpec::new_output("reddit posts", tk("A page of a community's posts as JSON"), |args, cx| {
        let sub = Subreddit::parse(args.str(0)?).ok_or_else(|| t("Use a community name like r/linux").to_string())?;
        let settings = current(cx);
        let sort = match args.get(1) { Some(sort) => Sort::parse(sort).ok_or_else(|| t("Sort by hot, new, top or rising").to_string())?, None => settings.sort };
        let span = match args.get(2) { Some(span) => Span::parse(span).ok_or_else(|| t("Use hour, day, week, month, year or all").to_string())?, None => settings.span };
        let query = Query { subreddit: sub, sort, span, search: None };
        let credentials = credentials(&settings);
        Ok(crate::powerctl::pending(move || {
            let mut session = model::session();
            session.configure(&credentials.0, credentials.1.as_deref());
            let page = session.listing(&query, None, 0, want(1.0), Instant::now()).map_err(|error| error.to_string())?;
            Ok(super::json(&*page))
        }))
    }).arg(ArgSpec::required("subreddit", ArgKind::String, tk("community name"))).arg(ArgSpec::optional("sort", ArgKind::String, tk("hot, new, top or rising")).choices(&["hot", "new", "top", "rising"]))
        .arg(ArgSpec::optional("range", ArgKind::String, tk("time range of top posts")).choices(&["hour", "day", "week", "month", "year", "all"])).example("reddit posts linux top week"));
    registry.command(ID, CommandSpec::new_output("reddit thread", tk("A post and its comments as JSON"), |args, cx| {
        let target = args.str(0)?;
        let id: Box<str> = mandara_services::reddit::post_in_url(target).or_else(|| mandara_services::reddit::post_id(target)).ok_or_else(|| t("Use a post id or a Reddit post address").to_string())?.into();
        let credentials = credentials(&current(cx));
        Ok(crate::powerctl::pending(move || {
            let mut session = model::session();
            session.configure(&credentials.0, credentials.1.as_deref());
            let thread = session.thread(&id, None, want(1.0), Instant::now()).map_err(|error| error.to_string())?;
            Ok(super::json(&thread))
        }))
    }).arg(ArgSpec::required("post", ArgKind::String, tk("post id or address"))).example("reddit thread https://www.reddit.com/r/linux/comments/abc123/"));
    registry.command(ID, CommandSpec::new("reddit subscriptions", tk("Your communities, in order"), |_, cx| {
        Ok(serde_json::json!(current(cx).subscriptions))
    }));
    registry.command(ID, CommandSpec::new("reddit subscribe", tk("Add a community to your list"), |args, cx| {
        subscribe(args.str(0)?, cx)?;
        Ok(serde_json::json!(current(cx).subscriptions))
    }).arg(ArgSpec::required("subreddit", ArgKind::String, tk("community name"))).example("reddit subscribe r/rust"));
    registry.command(ID, CommandSpec::new("reddit unsubscribe", tk("Remove a community from your list"), |args, cx| {
        unsubscribe(args.str(0)?, cx);
        Ok(serde_json::json!(current(cx).subscriptions))
    }).arg(ArgSpec::required("subreddit", ArgKind::String, tk("community name"))).example("reddit unsubscribe r/rust"));
    registry.command(ID, CommandSpec::new("reddit status", tk("Connection, rate limit and what the reader holds"), |_, _| {
        // Never wait for the worker here: a request in flight holds the session.
        let session = std::sync::LazyLock::get(&model::SESSION).map(|session| match session.try_lock() {
            Ok(session) => super::json(session.status(Instant::now())),
            Err(_) => serde_json::json!({ "busy": true }),
        });
        Ok(serde_json::json!({ "session": session, "reader": model::existing().map(|shared| shared.borrow().summary()) }))
    }));
}

/// The client id and user name a request is made with.
fn credentials(settings: &Settings) -> (Box<str>, Option<Box<str>>) {
    (settings.client_id.clone(), settings.username.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(pairs: &[(&str, toml::Value)]) -> ShellConfig {
        let mut config = ShellConfig::default();
        let table = config.plugins.settings.entry(ID.into()).or_default();
        for (key, value) in pairs { table.insert((*key).into(), value.clone()); }
        config
    }

    #[test]
    fn defaults_are_safe_and_every_setting_round_trips() {
        let defaults = settings(&ShellConfig::default());
        assert_eq!(defaults.nsfw, Gate::Hide, "NSFW is off by default");
        assert_eq!(defaults.spoilers, Gate::Blur, "spoilers are covered by default");
        assert!(defaults.images && defaults.refresh.is_none() && defaults.client_id.is_empty());
        assert_eq!(defaults.layout, Layout::Cards);
        assert_eq!(defaults.subscriptions.iter().map(Subreddit::as_str).collect::<Vec<_>>(), ["popular", "technology", "linux", "science", "space"]);
        let custom = settings(&config(&[
            ("client_id", " fixture_id ".into()), ("username", "u/someone".into()), ("subreddits", "r/rust, linux\nRUST no!pe x".into()),
            ("sort", "top".into()), ("span", "week".into()), ("nsfw", "blur".into()), ("spoilers", "show".into()), ("images", false.into()),
            ("layout", "compact".into()), ("refresh_minutes", 10.into()),
        ]));
        assert_eq!(&*custom.client_id, "fixture_id");
        assert_eq!(custom.username.as_deref(), Some("someone"));
        assert_eq!(custom.subscriptions.iter().map(Subreddit::as_str).collect::<Vec<_>>(), ["rust", "linux"], "names are deduplicated without case");
        assert_eq!((custom.sort, custom.span, custom.nsfw, custom.spoilers, custom.images, custom.layout), (Sort::Top, Span::Week, Gate::Blur, Gate::Show, false, Layout::Compact));
        assert_eq!(custom.refresh, Some(Duration::from_secs(600)));
        let grid = settings(&config(&[("layout", "grid".into()), ("card_subreddit", "r/space".into())]));
        assert_eq!(grid.layout, Layout::Grid);
        assert_eq!(grid.card.as_ref().map(Subreddit::as_str), Some("space"));
        assert_eq!(sidebar::community(&defaults).map(|sub| sub.to_string()), Some("r/popular".into()), "the card follows your first community");
        assert_eq!([Layout::Cards.next(), Layout::Compact.next(), Layout::Grid.next()], [Layout::Compact, Layout::Grid, Layout::Cards]);
        assert!([Layout::Cards, Layout::Compact, Layout::Grid].iter().all(|layout| settings(&config(&[("layout", layout.key().into())])).layout == *layout), "the switch saves what Studio reads");
        assert!(panel_width(Layout::Grid) > panel_width(Layout::Cards));
        let odd = settings(&config(&[("spoilers", "hide".into()), ("nsfw", "nonsense".into()), ("refresh_minutes", 9999.into()), ("subreddits", "".into())]));
        assert_eq!(odd.spoilers, Gate::Blur, "spoilers can be covered, never removed");
        assert_eq!(odd.nsfw, Gate::Hide);
        assert_eq!(odd.refresh, Some(Duration::from_secs(MAX_REFRESH_MINUTES as u64 * 60)));
        assert!(odd.subscriptions.is_empty(), "an emptied list stays empty");
    }

    /// NSFW posts are left out by default, blurred or shown on request;
    /// spoilers are covered until revealed or allowed.
    #[test]
    fn nsfw_and_spoiler_gates_follow_the_settings_and_the_reveal() {
        use mandara_services::reddit::Post;
        let nsfw = Post { id: "a".into(), title: "t".into(), nsfw: true, ..Post::default() };
        let spoiler = Post { id: "b".into(), title: "t".into(), spoiler: true, ..Post::default() };
        let plain = Post { id: "c".into(), title: "t".into(), ..Post::default() };
        let defaults = settings(&ShellConfig::default());
        assert!(panel::hidden(&nsfw, &defaults) && !panel::hidden(&spoiler, &defaults) && !panel::hidden(&plain, &defaults));
        assert!(panel::covered(&spoiler, &defaults, false) && !panel::covered(&spoiler, &defaults, true));
        assert!(!panel::covered(&plain, &defaults, false));
        let blur = settings(&config(&[("nsfw", "blur".into())]));
        assert!(!panel::hidden(&nsfw, &blur) && panel::covered(&nsfw, &blur, false) && !panel::covered(&nsfw, &blur, true));
        let show = settings(&config(&[("nsfw", "show".into()), ("spoilers", "show".into())]));
        assert!(!panel::hidden(&nsfw, &show) && !panel::covered(&nsfw, &show, false) && !panel::covered(&spoiler, &show, false));
    }

    #[test]
    fn subscription_lists_are_bounded() {
        let many = (0..80).map(|n| format!("community_{n}")).collect::<Vec<_>>().join(" ");
        assert_eq!(parse_subscriptions(&many).len(), MAX_SUBSCRIPTIONS);
    }
}
