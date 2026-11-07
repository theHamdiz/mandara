//! CI status: GitHub Actions runs of a few repositories. The bar chip is a
//! workflow glyph with a rotating ring of dots while any run is queued or
//! in progress and a success-emerald or urgent-coral mark for the latest
//! finished run (a muted glyph until repositories are set). The panel shows
//! one card per repository with its latest runs (status, workflow, branch,
//! commit title, duration and age); a click opens the run on GitHub. A
//! desktop notification says when a run you saw in progress finishes.
//!
//! Data comes from `GET /repos/{owner}/{repo}/actions/runs`
//! ([`mandara_services::ci`]), polled off the UI thread every 30 s while a
//! run is going and every 5 minutes otherwise, only while the chip or the
//! panel is on screen; unchanged lists cost a `304`. Rate limits pause
//! polling until GitHub's reset and say so in the cards. Settings
//! (`[plugins.settings.ci]`): `repos` (comma-separated `owner/name`),
//! `use_gh_token` (use `gh auth token` when the gh CLI is signed in; else
//! `GITHUB_TOKEN`; else anonymous), `notify`. Commands: `ci status`,
//! `ci refresh`.

mod replies;
mod ring;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Task, Window, div,
    prelude::FluentBuilder as _, px, size,
};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{CommandSpec, Output, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::ci::{self as svc, AuthSource, Fetch, Phase, Run};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::extras::caption;
use super::model_view as fn_view;
use super::panel_fx::{ANGLE_DOWN_DEG, HEADER_H, PanelShape, ViewSet, alpha, empty_state, glass_card, glow, header, readable, redraw_in};
use crate::fonts::tabular_nums;
use crate::icons::icon;
use crate::kit::{format_duration, icon_button, reveal, text, tokens};

pub const ID: &str = "ci";

// ── Layout ───────────────────────────────────────────────────────────────

const PANEL_W: f32 = 500.0;
const PAD: f32 = 14.0;
const GAP: f32 = tokens::MD;
/// Repository cards.
const CARD_PAD_PX: f32 = 10.0;
const CARD_HEAD_H: f32 = 26.0;
const CARD_GAP: f32 = 10.0;
const CARD_HEAD_GAP_PX: f32 = 4.0;
const NOTE_H: f32 = 30.0;
const NOTE_ICON_PX: f32 = 13.0;
/// Run rows.
const RUN_H: f32 = 46.0;
const RUN_GAP: f32 = 2.0;
const RUN_PAD_X_PX: f32 = 6.0;
const RUN_INNER_GAP_PX: f32 = 10.0;
const RUN_HOVER: (f32, f32) = (0.06, 0.05);
const RUN_RADIUS_PX: f32 = 10.0;
const LINE_GAP_PX: f32 = 1.0;
const META_W_PX: f32 = 64.0;
/// The status disc of a run.
const DISC_PX: f32 = 26.0;
const DISC_ICON_PX: f32 = 13.0;
const DISC_FILL: (f32, f32) = (0.18, 0.12);
/// Overall pill in a card's head.
const PILL_H_PX: f32 = 20.0;
const PILL_PAD_X_PX: f32 = 8.0;
const PILL_GAP_PX: f32 = 5.0;
const PILL_TEXT_PX: f32 = 10.5;
const PILL_DOT_PX: f32 = 6.0;
const PILL_FILL: (f32, f32) = (0.14, 0.1);
const DOT_GLOW: (f32, f32) = (0.5, 6.0);
/// Cards visible before the list scrolls.
const CARDS_MAX_H: f32 = 560.0;
const LIST_FADE_PX: f32 = 22.0;
const EMPTY_H: f32 = 170.0;
const FOOTER_H: f32 = 16.0;
const REFRESH_BTN_PX: f32 = 30.0;
/// Bar chip: the glyph box, the glyph, the dots of the ring and the mark.
const CHIP_BOX_PX: f32 = 20.0;
const CHIP_ICON_PX: f32 = 13.0;
const CHIP_IDLE_ICON_PX: f32 = 15.0;
const RING_TRACK_ALPHA: f32 = 0.16;
const MARK_PX: f32 = 6.0;
const MARK_OFFSET_PX: f32 = -1.0;
const CHIP_MUTED_ALPHA: f32 = 0.5;
const SIZE_EPSILON_PX: f32 = 0.5;

// ── Timing ───────────────────────────────────────────────────────────────

/// Poll cadence while any run is queued or in progress.
const FAST_EVERY: Duration = Duration::from_secs(30);
/// Poll cadence otherwise.
const SLOW_EVERY: Duration = Duration::from_secs(300);
/// An opened panel refreshes data older than this.
const PANEL_FRESH: Duration = FAST_EVERY;
/// The poller stops once no view rendered for this long past a cycle.
const IDLE_GRACE: Duration = Duration::from_secs(5);
/// Running durations in the open panel tick this often.
const PANEL_TICK: Duration = Duration::from_secs(1);
const SECS_PER_MIN: u64 = 60;

// ── Settings ─────────────────────────────────────────────────────────────

/// The settings as written (`repos` is parsed only when it changed).
struct Settings {
    raw: String,
    use_gh: bool,
}

fn settings(cx: &App) -> Settings {
    let cfg = crate::state::config(cx).read(cx).snapshot();
    Settings { raw: cfg.plugins.setting(ID, "repos", String::new()), use_gh: cfg.plugins.setting(ID, "use_gh_token", true) }
}

// ── Model ────────────────────────────────────────────────────────────────

/// One run's labels, formatted once per poll.
struct RunRow {
    title: SharedString,
    meta: SharedString,
    status: SharedString,
    when: SharedString,
    duration: SharedString,
}

#[derive(Default)]
struct RepoState {
    runs: Vec<Run>,
    rows: Vec<RunRow>,
    etag: Option<String>,
    error: Option<String>,
}

struct Model {
    /// The `repos` setting as last read (raw, to notice edits cheaply).
    raw: String,
    repos: Vec<String>,
    states: HashMap<String, RepoState>,
    auth: Option<AuthSource>,
    polled_at: Option<Instant>,
    polling: bool,
    /// GitHub's rate limit resets then.
    limited_until: Option<Instant>,
    /// Runs seen queued or in progress (notified when they finish).
    watching: HashSet<u64>,
    waiters: Vec<async_channel::Sender<Result<String, String>>>,
    /// The chip ring's phase origin.
    epoch: Instant,
    seen: Option<Instant>,
    poller: Option<Task<()>>,
    close_watch: Rc<Cell<bool>>,
    opened: bool,
    views: ViewSet,
}

impl Model {
    fn new() -> Self {
        Self {
            raw: String::new(),
            repos: Vec::new(),
            states: HashMap::new(),
            auth: None,
            polled_at: None,
            polling: false,
            limited_until: None,
            watching: HashSet::new(),
            waiters: Vec::new(),
            epoch: Instant::now(),
            seen: None,
            poller: None,
            close_watch: Rc::default(),
            opened: false,
            views: ViewSet::default(),
        }
    }

    fn running(&self) -> usize {
        self.states.values().flat_map(|s| &s.runs).filter(|r| r.phase == Phase::Running).count()
    }

    fn cadence(&self) -> Duration {
        if self.running() > 0 { FAST_EVERY } else { SLOW_EVERY }
    }

    fn limited(&self) -> Option<Duration> {
        self.limited_until.and_then(|u| u.checked_duration_since(Instant::now()))
    }

    /// The most recently finished run of any repository.
    fn latest_finished(&self) -> Option<&Run> {
        self.states.values().flat_map(|s| &s.runs).filter(|r| r.finished()).max_by_key(|r| r.updated_at)
    }

    /// Folds one poll's answers in; returns the watched runs that finished
    /// (`(repo, run)`).
    fn absorb(&mut self, auth: AuthSource, results: Vec<(String, Fetch)>) -> Vec<(String, Run)> {
        self.auth = Some(auth);
        let mut finished = Vec::new();
        for (repo, fetch) in results {
            let st = self.states.entry(repo.clone()).or_default();
            match fetch {
                Fetch::NotModified => st.error = None,
                Fetch::Runs { runs, etag } => {
                    finished.extend(runs.iter().filter(|r| r.finished() && self.watching.contains(&r.id)).map(|r| (repo.clone(), r.clone())));
                    st.rows = runs.iter().map(run_row).collect();
                    st.runs = runs;
                    st.etag = etag;
                    st.error = None;
                }
                Fetch::RateLimited(secs) => {
                    self.limited_until = Some(Instant::now() + Duration::from_secs(secs));
                    st.error = None;
                }
                Fetch::Failed(e) => st.error = Some(e),
            }
        }
        let repos = &self.repos;
        self.states.retain(|k, _| repos.contains(k));
        self.watching = self.states.values().flat_map(|s| &s.runs).filter(|r| r.phase == Phase::Running).map(|r| r.id).collect();
        self.polled_at = Some(Instant::now());
        self.polling = false;
        finished
    }

    fn status_json(&self) -> serde_json::Value {
        let repos: Vec<serde_json::Value> = self
            .repos
            .iter()
            .map(|r| {
                let st = self.states.get(r);
                json!({ "repo": r, "runs": st.map(|s| s.runs.as_slice()).unwrap_or_default(), "error": st.and_then(|s| s.error.clone()) })
            })
            .collect();
        json!({ "auth": self.auth, "running": self.running(), "rate_limited_for_s": self.limited().map(|d| d.as_secs()), "repos": repos })
    }
}

fn status_label(r: &Run) -> &'static str {
    match r.phase {
        Phase::Running if r.status == "in_progress" => t("Running"),
        Phase::Running => t("Queued"),
        Phase::Success => t("Passed"),
        Phase::Failure => t("Failed"),
        Phase::Cancelled if r.conclusion.as_deref() == Some("skipped") => t("Skipped"),
        Phase::Cancelled => t("Cancelled"),
    }
}

fn run_row(r: &Run) -> RunRow {
    let meta = if r.branch.is_empty() { format!("{} · #{}", r.workflow, r.run_number) } else { format!("{} · {} · #{}", r.workflow, r.branch, r.run_number) };
    RunRow {
        title: r.title.clone().into(),
        meta: meta.into(),
        status: status_label(r).into(),
        when: super::notifications::ago(r.created_at).into(),
        duration: format_duration(r.duration(0).max(0) as u64).into(),
    }
}

fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

type Shared = Rc<RefCell<Model>>;

thread_local! {
    static MODEL: RefCell<Weak<RefCell<Model>>> = const { RefCell::new(Weak::new()) };
}

fn model() -> Shared {
    if let Some(m) = MODEL.with(|w| w.borrow().upgrade()) {
        return m;
    }
    let m = Rc::new(RefCell::new(Model::new()));
    MODEL.with(|w| *w.borrow_mut() = Rc::downgrade(&m));
    m
}

/// Notifies about runs that finished since they were seen running.
fn announce(finished: &[(String, Run)], cx: &mut App) {
    for (repo, run) in finished {
        let summary = match run.phase {
            Phase::Success => tf("{workflow} passed in {repo}", &[("workflow", &run.workflow), ("repo", repo)]),
            Phase::Failure => tf("{workflow} failed in {repo}", &[("workflow", &run.workflow), ("repo", repo)]),
            _ => tf("{workflow} was cancelled in {repo}", &[("workflow", &run.workflow), ("repo", repo)]),
        };
        let body = if run.branch.is_empty() { run.title.clone() } else { format!("{} · {}", run.branch, run.title) };
        super::notifications::send(cx, mandara_services::NotificationCommand::Post { app: t("CI status").into(), icon: "workflow".into(), summary, body });
    }
}

/// Polls every repository off the UI thread when due (or `force`d).
fn poll(m: &Shared, force: bool, cx: &mut App) {
    let cfg = crate::state::config(cx).read(cx).snapshot();
    let raw = cfg.plugins.settings.get(ID).and_then(|settings| settings.get("repos")).and_then(toml::Value::as_str).unwrap_or_default();
    let (repos, etags): (Vec<String>, Vec<Option<String>>) = {
        let mut mb = m.borrow_mut();
        let changed = mb.raw != raw || mb.polled_at.is_none();
        let due = mb.polled_at.is_none_or(|t| t.elapsed() >= mb.cadence());
        if mb.polling || !(force || changed || due) || (!force && !changed && mb.limited().is_some()) {
            return;
        }
        if changed {
            mb.repos = svc::parse_repos(raw);
            mb.raw.clear();
            mb.raw.push_str(raw);
        }
        let repos = mb.repos.clone();
        if repos.is_empty() {
            mb.states.clear();
            mb.watching.clear();
            mb.polled_at = Some(Instant::now());
            let waiters = std::mem::take(&mut mb.waiters);
            replies::send(&waiters, || mb.status_json().to_string());
            return;
        }
        mb.polling = true;
        let etags = repos.iter().map(|r| mb.states.get(r).and_then(|st| st.etag.clone())).collect();
        (repos, etags)
    };
    let m = m.clone();
    m.borrow().views.notify(cx);
    let use_gh = cfg.plugins.setting(ID, "use_gh_token", true);
    let notify = cfg.plugins.setting(ID, "notify", true);
    cx.spawn(async move |cx| {
        let (tx, rx) = async_channel::bounded(1);
        let _ = std::thread::Builder::new().name("mandara-ci".into()).spawn(move || {
            let auth = svc::auth(use_gh);
            let results: Vec<(String, Fetch)> = repos.iter().zip(etags).map(|(r, etag)| (r.clone(), svc::fetch(r, &auth, etag.as_deref()))).collect();
            let _ = tx.send_blocking((auth.source, results));
        });
        let Ok((source, results)) = rx.recv().await else {
            m.borrow_mut().polling = false;
            return;
        };
        cx.update(|cx| {
            let (finished, views) = {
                let mut mb = m.borrow_mut();
                let finished = mb.absorb(source, results);
                let waiters = std::mem::take(&mut mb.waiters);
                replies::send(&waiters, || mb.status_json().to_string());
                (finished, mb.views.clone())
            };
            if notify {
                announce(&finished, cx);
            }
            views.notify(cx);
        });
    })
    .detach();
}

/// Marks a view as on screen and keeps polling while any is: every 30 s
/// while a run is going, every 5 minutes otherwise.
fn keep_alive(m: &Shared, window: &Window, cx: &mut App) {
    let mut mb = m.borrow_mut();
    mb.views.track(window);
    mb.seen = Some(Instant::now());
    if mb.poller.is_some() {
        return;
    }
    let weak = Rc::downgrade(m);
    mb.poller = Some(cx.spawn(async move |cx| {
        let mut slept = Duration::ZERO;
        loop {
            let Some(m) = weak.upgrade() else { return };
            // On screen = rendered since this poller went to sleep (each
            // wake notifies the views, so a visible chip always has).
            if m.borrow().seen.is_none_or(|t| t.elapsed() > slept + IDLE_GRACE) {
                break;
            }
            cx.update(|cx| {
                poll(&m, false, cx);
                m.borrow().views.notify(cx);
            });
            // Sleep to the next due poll (or the rate limit's reset).
            let wait = {
                let mb = m.borrow();
                let every = mb.cadence();
                let next = mb.polled_at.map_or(every, |t| every.saturating_sub(t.elapsed()));
                next.max(mb.limited().unwrap_or_default()).max(IDLE_GRACE)
            };
            drop(m);
            cx.background_executor().timer(wait).await;
            slept = wait;
        }
        if let Some(m) = weak.upgrade() {
            m.borrow_mut().poller = None;
        }
    }));
}

// ── Pieces ───────────────────────────────────────────────────────────────

fn phase_color(p: Phase, palette: &MandaraPalette) -> Color {
    readable(
        match p {
            Phase::Running => palette.primary,
            Phase::Success => palette.success_emerald,
            Phase::Failure => palette.urgent_coral,
            Phase::Cancelled => palette.on_surface_variant,
        },
        palette,
    )
}

fn phase_icon(p: Phase) -> &'static str {
    match p {
        Phase::Running => "timer",
        Phase::Success => "check",
        Phase::Failure => "close",
        Phase::Cancelled => "minus",
    }
}

fn status_pill(label: SharedString, p: Phase, palette: &MandaraPalette) -> AnyElement {
    let tint = phase_color(p, palette);
    div()
        .flex_none()
        .h(px(PILL_H_PX))
        .px(px(PILL_PAD_X_PX))
        .rounded_full()
        .flex()
        .items_center()
        .gap(px(PILL_GAP_PX))
        .bg(tint.with_alpha(alpha(PILL_FILL, palette)).hsla())
        .text_size(px(PILL_TEXT_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(tint.hsla())
        .child(div().size(px(PILL_DOT_PX)).rounded_full().bg(tint.hsla()).shadow(vec![glow(tint.with_alpha(DOT_GLOW.0), DOT_GLOW.1, 0.0, 0.0)]))
        .child(label)
        .into_any_element()
}

fn run_element(repo: &str, i: usize, run: &Run, row: &RunRow, palette: &MandaraPalette, now: i64) -> AnyElement {
    let tint = phase_color(run.phase, palette);
    let muted = palette.on_surface_variant;
    let hover = palette.on_surface.with_alpha(alpha(RUN_HOVER, palette));
    // Running durations tick in the open panel; finished ones are fixed.
    let duration: SharedString = if run.phase == Phase::Running { format_duration(run.duration(now) as u64).into() } else { row.duration.clone() };
    let url = run.html_url.clone();
    let id = {
        use std::hash::{Hash as _, Hasher as _};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (repo, i, run.id).hash(&mut h);
        ElementId::NamedInteger(SharedString::new_static("ci-run"), h.finish())
    };
    div()
        .id(id)
        .h(px(RUN_H))
        .flex_none()
        .px(px(RUN_PAD_X_PX))
        .rounded(px(crate::style::radius(RUN_RADIUS_PX)))
        .flex()
        .items_center()
        .gap(px(RUN_INNER_GAP_PX))
        .cursor_pointer()
        .hover(move |s| s.bg(hover.hsla()))
        .child(
            div()
                .size(px(DISC_PX))
                .flex_none()
                .rounded_full()
                .bg(tint.with_alpha(alpha(DISC_FILL, palette)).hsla())
                .flex()
                .items_center()
                .justify_center()
                .child(icon(phase_icon(run.phase), DISC_ICON_PX, tint)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(LINE_GAP_PX))
                .child(div().min_w(px(0.0)).truncate().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).child(row.title.clone()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(PILL_GAP_PX))
                        .min_w(px(0.0))
                        .text_size(px(text::LABEL))
                        .text_color(muted.hsla())
                        .child(div().flex_none().text_color(tint.hsla()).font_weight(FontWeight::SEMIBOLD).child(row.status.clone()))
                        .child(div().flex_none().child("·"))
                        .child(div().min_w(px(0.0)).truncate().child(row.meta.clone())),
                ),
        )
        .child(
            div()
                .w(px(META_W_PX))
                .flex_none()
                .flex()
                .flex_col()
                .items_end()
                .gap(px(LINE_GAP_PX))
                .font_features(tabular_nums())
                .child(div().text_size(px(text::BODY_S)).child(duration))
                .child(div().text_size(px(text::LABEL)).text_color(muted.hsla()).child(row.when.clone())),
        )
        .on_click(move |_, _, _| {
            let url = url.clone();
            std::thread::spawn(move || {
                if let Err(err) = mandara_services::session::open_uri(&url) {
                    tracing::warn!("ci: opening {url}: {err}");
                }
            });
        })
        .into_any_element()
}

/// A quiet line inside a card (an error or the rate limit).
fn note(glyph: &'static str, msg: String, tint: Color, palette: &MandaraPalette) -> AnyElement {
    div()
        .h(px(NOTE_H))
        .px(px(RUN_PAD_X_PX))
        .flex()
        .items_center()
        .gap(px(PILL_GAP_PX))
        .text_size(px(text::LABEL))
        .text_color(palette.on_surface_variant.hsla())
        .child(icon(glyph, NOTE_ICON_PX, tint))
        .child(div().min_w(px(0.0)).truncate().child(msg))
        .into_any_element()
}

/// One repository's card and its height.
fn card(repo: &str, mb: &Model, palette: &MandaraPalette, radius: f32, now: i64) -> (AnyElement, f32) {
    let st = mb.states.get(repo);
    let (owner, name) = repo.split_once('/').unwrap_or(("", repo));
    let latest = st.and_then(|s| s.runs.first().zip(s.rows.first()));
    let mut h = CARD_PAD_PX * 2.0 + CARD_HEAD_H;
    let head = div()
        .h(px(CARD_HEAD_H))
        .px(px(RUN_PAD_X_PX))
        .flex()
        .items_center()
        .gap(px(CARD_HEAD_GAP_PX))
        .child(div().flex_none().text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(format!("{owner}/")))
        .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(name.to_string()))
        .when_some(latest, |el, (run, row)| el.child(status_pill(row.status.clone(), run.phase, palette)));
    let mut body = div().flex().flex_col().gap(px(RUN_GAP));
    let warn = readable(palette.warning_amber, palette);
    if let Some(wait) = mb.limited() {
        let minutes = wait.as_secs().div_ceil(SECS_PER_MIN).max(1);
        body = body.child(note("timer", tf("GitHub's rate limit was reached; checking again in {minutes} min", &[("minutes", &minutes)]), warn, palette));
        h += NOTE_H + RUN_GAP;
    }
    match st {
        Some(s) => {
            if let Some(e) = &s.error {
                body = body.child(note("alert", e.clone(), readable(palette.urgent_coral, palette), palette));
                h += NOTE_H + RUN_GAP;
            }
            if s.runs.is_empty() && s.error.is_none() && mb.polled_at.is_some() {
                body = body.child(note("workflow", t("No workflow runs yet").into(), palette.on_surface_variant, palette));
                h += NOTE_H + RUN_GAP;
            }
            for (i, (run, row)) in s.runs.iter().zip(&s.rows).enumerate() {
                body = body.child(run_element(repo, i, run, row, palette, now));
                h += RUN_H + RUN_GAP;
            }
        }
        None => {
            body = body.child(note("timer", t("Checking…").into(), palette.on_surface_variant, palette));
            h += NOTE_H + RUN_GAP;
        }
    }
    let el = glass_card(palette).rounded(px(radius)).p(px(CARD_PAD_PX)).flex().flex_col().child(head).child(body).into_any_element();
    (el, h)
}

// ── Panel ────────────────────────────────────────────────────────────────

struct View {
    p: PanelCx,
    m: Shared,
    size_cell: Rc<Cell<gpui::Size<gpui::Pixels>>>,
}

impl View {
    fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        let m = &self.m;
        keep_alive(m, window, cx);
        if self.p.progress() > 0.0 {
            let fresh = m.borrow().polled_at.is_some_and(|t| t.elapsed() < PANEL_FRESH);
            if !std::mem::replace(&mut m.borrow_mut().opened, true) && !fresh {
                poll(m, true, cx);
            } else {
                poll(m, false, cx);
            }
            let watch = m.borrow().close_watch.clone();
            if !watch.get() {
                let closed = m.clone();
                super::processes::on_panel_close(&self.p, &watch, cx, move |_| closed.borrow_mut().opened = false);
            }
            if m.borrow().running() > 0 {
                redraw_in(PANEL_TICK, window, cx);
            }
        }
        let shape = PanelShape::of(cx);
        let card_r = shape.inner(PAD);
        let now = now_unix();
        let mb = m.borrow();
        let running = mb.running();
        let accent = readable(palette.primary, palette);
        let checked = mb.polled_at.map(|t| tf("checked {span} ago", &[("span", &super::panel_fx::duration_short(t.elapsed().as_secs() as i64))]));
        let subtitle = match (mb.repos.is_empty(), mb.polling, running, checked) {
            (true, _, _, _) => t("No repositories yet").to_string(),
            (false, true, _, _) | (false, false, _, None) => t("Checking…").to_string(),
            (false, false, 0, Some(c)) => tf("Nothing running · {checked}", &[("checked", &c)]),
            (false, false, n, Some(c)) => tf("{count} running · {checked}", &[("count", &n), ("checked", &c)]),
        };
        let refresh = {
            let m = m.clone();
            icon_button("ci-refresh", "refresh", REFRESH_BTN_PX, palette.on_surface_variant, None, move |_, cx| poll(&m, true, cx)).into_any_element()
        };
        let head = header("workflow", accent, t("CI status"), subtitle, palette, Some(refresh));

        let (content, content_h): (AnyElement, f32) = if mb.repos.is_empty() {
            (empty_state("workflow", t("No repositories to watch"), t("Add owner/name repositories in Studio → Plugins → CI status"), EMPTY_H, palette).into_any_element(), EMPTY_H)
        } else {
            let mut total = 0.0;
            let mut cards = Vec::with_capacity(mb.repos.len());
            for repo in &mb.repos {
                let (el, h) = card(repo, &mb, palette, card_r, now);
                total += h + CARD_GAP;
                cards.push(el);
            }
            let total = total - CARD_GAP;
            let shown = total.min(CARDS_MAX_H);
            let fill = crate::shell::frame_color(palette);
            let el = div()
                .relative()
                .h(px(shown))
                .child(div().id("ci-cards").size_full().overflow_y_scroll().flex().flex_col().gap(px(CARD_GAP)).children(cards))
                .when(total > CARDS_MAX_H + SIZE_EPSILON_PX, |el| el.child(div().absolute().left_0().right_0().bottom_0().h(px(LIST_FADE_PX)).bg(oklab_gradient(ANGLE_DOWN_DEG, fill.with_alpha(0.0), fill))))
                .into_any_element();
            (el, shown)
        };
        let auth_line = match mb.auth {
            Some(AuthSource::GhCli) => Some(t("Signed in through the gh CLI")),
            Some(AuthSource::Env) => Some(t("Using the GITHUB_TOKEN variable")),
            Some(AuthSource::Anonymous) => Some(t("Anonymous: public repositories, 60 requests an hour")),
            None => None,
        };
        drop(mb);
        let footer_h = if auth_line.is_some() { GAP + FOOTER_H } else { 0.0 };
        let height = PAD * 2.0 + HEADER_H + GAP + content_h + footer_h;
        if (f32::from(self.size_cell.get().height) - height).abs() > SIZE_EPSILON_PX {
            self.size_cell.set(size(px(PANEL_W), px(height)));
        }
        let p = &self.p;
        div()
            .size_full()
            .p(px(PAD))
            .flex()
            .flex_col()
            .gap(px(GAP))
            .child(reveal(p, 0, head))
            .child(reveal(p, 1, content))
            .when_some(auth_line, |el, line| el.child(reveal(p, 2, div().h(px(FOOTER_H)).flex().justify_center().child(caption(line, palette)))))
            .into_any_element()
    }
}

/// The bar chip: the workflow glyph, a rotating ring while runs go, and a
/// mark for the latest finished run.
fn chip(m: &Shared, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    keep_alive(m, window, cx);
    // Notices edits of the `repos` setting (config changes re-render).
    poll(m, false, cx);
    let mb = m.borrow();
    if mb.repos.is_empty() {
        return icon("workflow", crate::style::bar_icon(CHIP_IDLE_ICON_PX), palette.on_surface.with_alpha(CHIP_MUTED_ALPHA)).into_any_element();
    }
    let running = mb.running() > 0;
    let mark = mb.latest_finished().map(|r| r.phase).filter(|p| matches!(p, Phase::Success | Phase::Failure)).map(|p| phase_color(p, palette));
    let side = crate::style::bar_icon(CHIP_BOX_PX);
    let mut boxed = div().relative().size(px(side)).flex_none().flex().items_center().justify_center().child(icon("workflow", crate::style::bar_icon(CHIP_ICON_PX), palette.on_surface));
    if running {
        let accent = readable(palette.primary, palette);
        let track = palette.on_surface.with_alpha(RING_TRACK_ALPHA);
        let reduced = cx.reduce_motion() || crate::state::config(cx).read(cx).get().motion.reduced(mandara_core::spring::system_reduced_motion());
        boxed = boxed.child(ring::draw(side, mb.epoch.elapsed(), reduced, accent, track));
        if !reduced {
            redraw_in(ring::STEP, window, cx);
        }
    }
    if let Some(c) = mark {
        boxed = boxed.child(div().absolute().right(px(MARK_OFFSET_PX)).bottom(px(MARK_OFFSET_PX)).size(px(MARK_PX)).rounded_full().bg(c.hsla()).border_1().border_color(crate::shell::frame_color(palette).hsla()));
    }
    boxed.into_any_element()
}

// ── Commands ─────────────────────────────────────────────────────────────

/// Fetches every repository on its own thread (no cached model).
fn fetch_now(cx: &App) -> Output {
    let s = settings(cx);
    let (tx, rx) = async_channel::bounded(1);
    let _ = std::thread::Builder::new().name("mandara-ci-cmd".into()).spawn(move || {
        let auth = svc::auth(s.use_gh);
        let repos: Vec<serde_json::Value> = svc::parse_repos(&s.raw)
            .iter()
            .map(|r| match svc::fetch(r, &auth, None) {
                Fetch::Runs { runs, .. } => json!({ "repo": r, "runs": runs, "error": null }),
                Fetch::RateLimited(secs) => json!({ "repo": r, "runs": [], "error": format!("rate limited for {secs} s") }),
                Fetch::Failed(e) => json!({ "repo": r, "runs": [], "error": e }),
                Fetch::NotModified => json!({ "repo": r, "runs": [], "error": null }),
            })
            .collect();
        let _ = tx.send_blocking(Ok(json!({ "auth": auth.source, "repos": repos }).to_string()));
    });
    Output::Pending(rx)
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("CI status"), tk("GitHub Actions runs of your repositories, with a note when a run finishes"), Some(Zone::Right)).opt_in(), |window, cx, panel_cx| {
        let m = model();
        let bar_model = m.clone();
        let bar = fn_view(None, move |palette, window, cx| chip(&bar_model, palette, window, cx), window, cx);
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(PAD * 2.0 + HEADER_H + GAP + EMPTY_H))));
        let view = View { p: panel_cx.clone(), m, size_cell: size_cell.clone() };
        let panel = fn_view(None, move |palette, window, cx| view.render(palette, window, cx), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: size_cell }), ..PluginInstance::empty() }
    });
    r.command(
        ID,
        CommandSpec::new_output("ci status", tk("Latest GitHub Actions runs of the watched repositories"), |_, cx| {
            if let Some(m) = MODEL.with(|w| w.borrow().upgrade()) {
                let mb = m.borrow();
                if mb.polled_at.is_some_and(|t| t.elapsed() < mb.cadence()) && mb.raw == settings(cx).raw {
                    return Ok(Output::Value(mb.status_json()));
                }
            }
            Ok(fetch_now(cx))
        })
        .example("ci status"),
    );
    r.command(
        ID,
        CommandSpec::new_output("ci refresh", tk("Check the watched repositories now"), |_, cx| {
            let Some(m) = MODEL.with(|w| w.borrow().upgrade()) else { return Ok(fetch_now(cx)) };
            let (tx, rx) = async_channel::bounded(1);
            m.borrow_mut().waiters.push(tx);
            poll(&m, true, cx);
            Ok(Output::Pending(rx))
        })
        .example("ci refresh"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: u64, phase: Phase, updated_at: i64) -> Run {
        Run {
            id,
            workflow: "CI".into(),
            title: "feat: x".into(),
            branch: "main".into(),
            event: "push".into(),
            run_number: id,
            status: if phase == Phase::Running { "in_progress".into() } else { "completed".into() },
            conclusion: match phase {
                Phase::Success => Some("success".into()),
                Phase::Failure => Some("failure".into()),
                Phase::Cancelled => Some("cancelled".into()),
                Phase::Running => None,
            },
            phase,
            html_url: format!("https://github.com/o/r/actions/runs/{id}"),
            created_at: updated_at - 60,
            started_at: updated_at - 60,
            updated_at,
        }
    }

    #[test]
    fn finished_watched_runs_are_reported_once() {
        let mut m = Model::new();
        m.repos = vec!["o/r".into()];
        let none = m.absorb(AuthSource::Anonymous, vec![("o/r".into(), Fetch::Runs { runs: vec![run(2, Phase::Running, 100), run(1, Phase::Success, 50)], etag: Some("\"a\"".into()) })]);
        assert!(none.is_empty(), "nothing was watched yet");
        assert_eq!((m.running(), m.cadence()), (1, FAST_EVERY));
        let done = m.absorb(AuthSource::Anonymous, vec![("o/r".into(), Fetch::Runs { runs: vec![run(2, Phase::Failure, 200), run(1, Phase::Success, 50)], etag: None })]);
        assert_eq!(done.iter().map(|(_, r)| r.id).collect::<Vec<_>>(), [2]);
        assert_eq!(m.latest_finished().map(|r| r.id), Some(2));
        assert_eq!((m.running(), m.cadence()), (0, SLOW_EVERY));
        let again = m.absorb(AuthSource::Anonymous, vec![("o/r".into(), Fetch::NotModified)]);
        assert!(again.is_empty(), "a finished run is announced once");
        assert_eq!(m.states["o/r"].runs.len(), 2, "a 304 keeps the runs");
    }

    #[test]
    fn errors_and_rate_limits_land_in_the_model() {
        let mut m = Model::new();
        m.repos = vec!["o/a".into(), "o/b".into()];
        m.absorb(AuthSource::GhCli, vec![("o/a".into(), Fetch::Failed("Not found".into())), ("o/b".into(), Fetch::RateLimited(120)), ("o/gone".into(), Fetch::NotModified)]);
        assert_eq!(m.states["o/a"].error.as_deref(), Some("Not found"));
        assert!(m.limited().is_some_and(|d| d.as_secs() > 100));
        assert!(!m.states.contains_key("o/gone"), "repositories no longer watched are dropped");
        let j = m.status_json();
        assert_eq!(j["auth"], "gh-cli");
        assert_eq!(j["repos"][0]["error"], "Not found");
    }
}
