//! Git repositories: every local repository under a few project folders at
//! a glance. The bar chip is a branch glyph with the number of repositories
//! that need attention (uncommitted changes or unpushed commits), just the
//! glyph when none do. The panel opens with a summary (repositories, dirty,
//! unpushed) and lists them by most recent activity with the branch,
//! ahead / behind, staged / modified / untracked counts and the last
//! commit; clicking a row reveals Terminal, Open folder, Copy path and
//! Fetch.
//!
//! Data comes from the `git` CLI ([`mandara_services::git_repos`]) on a
//! background thread: a scan when the panel opens and every 5 minutes while
//! the chip is on screen; nothing runs off screen. Settings
//! (`[plugins.settings.git-repos]`): `roots` (comma-separated folders;
//! missing ones are skipped) and `depth`. Commands: `git list`,
//! `git fetch <repo>`, `git refresh`.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Task, Window, div,
    prelude::FluentBuilder as _, px, size,
};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::git_repos::{self as svc, Repo};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::extras::{Flash, toast};
use super::model_view as fn_view;
use super::panel_fx::{
    ANGLE_DIAGONAL_DEG, ANGLE_DOWN_DEG, HEADER_H, PanelShape, ViewSet, alpha, empty_state, header, partner, pill_button, readable, redraw_in,
    rim,
};
use crate::fonts::{fonts, tabular_nums};
use crate::icons::icon;
use crate::kit::{icon_button, reveal, text, tokens};

pub const ID: &str = "git-repos";

// ── Layout ───────────────────────────────────────────────────────────────

const PANEL_W: f32 = 520.0;
const PAD: f32 = 14.0;
const GAP: f32 = tokens::MD;
/// Summary hero: height, padding, figures and the gradient tones.
const HERO_H: f32 = 86.0;
const HERO_PAD_X_PX: f32 = 18.0;
const HERO_VALUE_PX: f32 = 28.0;
const HERO_VALUE_LINE_PX: f32 = 32.0;
const HERO_LABEL_GAP_PX: f32 = 2.0;
const HERO_DIVIDER_W_PX: f32 = 1.0;
const HERO_DIVIDER_H_PX: f32 = 40.0;
const HERO_DIVIDER_ALPHA: f32 = 0.1;
const HERO_ANGLE_DEG: f32 = 125.0;
const HERO_DARK_L: (f32, f32) = (0.29, 0.22);
const HERO_DARK_CHROMA: ((f32, f32), (f32, f32)) = ((0.55, 0.07), (0.5, 0.055));
const HERO_LIGHT_L: (f32, f32) = (0.93, 0.955);
const HERO_LIGHT_CHROMA: ((f32, f32), (f32, f32)) = ((0.35, 0.045), (0.3, 0.035));
/// Rows.
const ROW_H: f32 = 58.0;
const ROW_GAP: f32 = 4.0;
/// The action strip under a selected row.
const ACTIONS_H: f32 = 60.0;
const ROW_PAD_X_PX: f32 = 10.0;
const ROW_INNER_GAP_PX: f32 = 12.0;
const ROW_HOVER: (f32, f32) = (0.05, 0.04);
const ROW_SELECTED: (f32, f32) = (0.09, 0.07);
const ROW_RADIUS_INSET_PX: f32 = 2.0;
const ROW_MIN_RADIUS_PX: f32 = 10.0;
const LINE_GAP_PX: f32 = 2.0;
const INLINE_GAP_PX: f32 = 6.0;
const MUTED_ALPHA: f32 = 0.85;
/// State avatar.
const AVATAR_PX: f32 = 34.0;
const AVATAR_ICON_PX: f32 = 16.0;
const AVATAR_RADIUS_FRAC: f32 = 0.32;
const AVATAR_FILL: (f32, f32) = (0.18, 0.1);
/// Branch / ahead-behind badges.
const BADGE_H_PX: f32 = 18.0;
const BADGE_PAD_X_PX: f32 = 7.0;
const BADGE_TEXT_PX: f32 = 10.5;
const BADGE_GAP_PX: f32 = 3.0;
const BADGE_ICON_PX: f32 = 10.0;
const BADGE_FILL: (f32, f32) = (0.14, 0.1);
const BRANCH_MAX_W_PX: f32 = 150.0;
/// Action strip.
const ACTIONS_PAD_PX: f32 = 8.0;
const ACTIONS_GAP_PX: f32 = 6.0;
const PATH_TEXT_PX: f32 = 10.5;
/// Rows visible before the list scrolls.
const VISIBLE_ROWS: f32 = 6.5;
const LIST_MAX_H: f32 = VISIBLE_ROWS * (ROW_H + ROW_GAP);
const LIST_FADE_PX: f32 = 22.0;
const EMPTY_H: f32 = 180.0;
const REFRESH_BTN_PX: f32 = 30.0;
/// Bar chip.
const CHIP_ICON_PX: f32 = 15.0;
const CHIP_GAP_PX: f32 = 4.0;
const CHIP_TEXT_PX: f32 = 12.0;
const SIZE_EPSILON_PX: f32 = 0.5;

// ── Timing ───────────────────────────────────────────────────────────────

/// Background rescan while the chip is on screen.
const SCAN_EVERY: Duration = Duration::from_secs(300);
/// The poller stops once no view rendered for this long past a cycle.
const IDLE_GRACE: Duration = Duration::from_secs(5);
/// While a scan or fetch runs, the "Scanning…" line is redrawn this often.
const BUSY_TICK: Duration = Duration::from_millis(500);

// ── Settings ─────────────────────────────────────────────────────────────

/// The `roots` setting as written (resolved off the UI thread, since that
/// touches the disk) and the search depth.
fn settings(cx: &App) -> (String, usize) {
    let cfg = crate::state::config(cx).read(cx).snapshot();
    let roots: String = cfg.plugins.setting(ID, "roots", String::from(svc::DEFAULT_ROOTS));
    let depth: i64 = cfg.plugins.setting(ID, "depth", svc::DEFAULT_DEPTH);
    (roots, depth.clamp(svc::DEPTH_RANGE.0, svc::DEPTH_RANGE.1) as usize)
}

/// Resolves the roots and scans (blocking): `(existing roots, repos)`.
fn scan_blocking(roots: &str, depth: usize) -> (usize, Vec<Repo>) {
    let roots = svc::roots(roots);
    (roots.len(), svc::scan(&roots, depth))
}

// ── Model ────────────────────────────────────────────────────────────────

/// Labels of one repository, formatted once per scan.
struct Row {
    name: SharedString,
    branch: SharedString,
    changes: SharedString,
    commit: SharedString,
    when: SharedString,
    path: SharedString,
}

struct Model {
    repos: Vec<Repo>,
    rows: Vec<Row>,
    /// Roots that exist (empty: nothing to scan).
    roots: usize,
    scanned_at: Option<Instant>,
    scanning: bool,
    /// The row whose actions are revealed.
    selected: Option<PathBuf>,
    fetching: HashSet<PathBuf>,
    flash: Flash,
    /// `git refresh` callers waiting for the scan to land.
    waiters: Vec<async_channel::Sender<Result<String, String>>>,
    /// This opening of the panel already scanned.
    opened: bool,
    seen: Option<Instant>,
    poller: Option<Task<()>>,
    close_watch: Rc<Cell<bool>>,
    views: ViewSet,
}

impl Model {
    fn new() -> Self {
        Self {
            repos: Vec::new(),
            rows: Vec::new(),
            roots: 0,
            scanned_at: None,
            scanning: false,
            selected: None,
            fetching: HashSet::new(),
            flash: Flash::default(),
            waiters: Vec::new(),
            opened: false,
            seen: None,
            poller: None,
            close_watch: Rc::default(),
            views: ViewSet::default(),
        }
    }

    fn absorb(&mut self, roots: usize, repos: Vec<Repo>) {
        self.roots = roots;
        self.rows = repos.iter().map(row_labels).collect();
        if self.selected.as_ref().is_some_and(|s| !repos.iter().any(|r| &r.path == s)) {
            self.selected = None;
        }
        self.repos = repos;
        self.scanned_at = Some(Instant::now());
        self.scanning = false;
    }

    /// Replaces one repository after a fetch.
    fn replace(&mut self, repo: Repo) {
        if let Some(i) = self.repos.iter().position(|r| r.path == repo.path) {
            self.rows[i] = row_labels(&repo);
            self.repos[i] = repo;
        }
    }

    fn attention(&self) -> usize {
        self.repos.iter().filter(|r| r.needs_attention()).count()
    }
}

fn summary_json(repos: &[Repo]) -> serde_json::Value {
    json!({
        "count": repos.len(),
        "dirty": repos.iter().filter(|r| r.status.dirty()).count(),
        "unpushed": repos.iter().filter(|r| r.status.ahead > 0).count(),
        "repos": repos,
    })
}

fn row_labels(r: &Repo) -> Row {
    let st = &r.status;
    let branch = st.branch.clone().unwrap_or_else(|| match &st.head {
        Some(h) => tf("detached at {commit}", &[("commit", &h.chars().take(7).collect::<String>())]),
        None => t("no commits yet").into(),
    });
    let mut parts: Vec<String> = Vec::new();
    if st.conflicts > 0 {
        parts.push(tf("{count} conflicted", &[("count", &st.conflicts)]));
    }
    if st.staged > 0 {
        parts.push(tf("{count} staged", &[("count", &st.staged)]));
    }
    if st.modified > 0 {
        parts.push(tf("{count} modified", &[("count", &st.modified)]));
    }
    if st.untracked > 0 {
        parts.push(tf("{count} untracked", &[("count", &st.untracked)]));
    }
    let changes = match &r.error {
        Some(e) => e.clone(),
        None if parts.is_empty() => t("Clean").into(),
        None => parts.join(" · "),
    };
    let commit = r.last_commit.as_ref().map(|c| c.subject.clone()).unwrap_or_default();
    let when = if r.activity > 0 { super::notifications::ago(r.activity) } else { String::new() };
    Row { name: r.name.clone().into(), branch: branch.into(), changes: changes.into(), commit: commit.into(), when: when.into(), path: r.path.display().to_string().into() }
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

/// Runs blocking work on its own thread (a scan starts several `git`
/// processes; it stays off GPUI's executor pool).
fn off_thread<T: Send + 'static>(name: &str, f: impl FnOnce() -> T + Send + 'static) -> async_channel::Receiver<T> {
    let (tx, rx) = async_channel::bounded(1);
    let _ = std::thread::Builder::new().name(name.into()).spawn(move || {
        let _ = tx.send_blocking(f());
    });
    rx
}

/// Starts a scan unless one is running.
fn scan(m: &Shared, cx: &mut App) {
    if std::mem::replace(&mut m.borrow_mut().scanning, true) {
        return;
    }
    let (roots, depth) = settings(cx);
    let m = m.clone();
    m.borrow().views.notify(cx);
    cx.spawn(async move |cx| {
        let rx = off_thread("mandara-git-scan", move || scan_blocking(&roots, depth));
        let (count, repos) = rx.recv().await.unwrap_or_default();
        let (views, waiters, json) = {
            let mut mb = m.borrow_mut();
            let waiters = std::mem::take(&mut mb.waiters);
            let json = (!waiters.is_empty()).then(|| summary_json(&repos).to_string());
            mb.absorb(count, repos);
            (mb.views.clone(), waiters, json)
        };
        if let Some(json) = json {
            for w in waiters {
                let _ = w.try_send(Ok(json.clone()));
            }
        }
        views.notify_async(cx);
    })
    .detach();
}

/// Marks a view as on screen and keeps the 5-minute rescan running while
/// any is.
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
            let (idle, due) = {
                let mb = m.borrow();
                (mb.seen.is_none_or(|t| t.elapsed() > slept + IDLE_GRACE), mb.scanned_at.is_none_or(|t| t.elapsed() + IDLE_GRACE >= SCAN_EVERY))
            };
            if idle {
                break;
            }
            cx.update(|cx| {
                if due {
                    scan(&m, cx);
                }
                m.borrow().views.notify(cx);
            });
            drop(m);
            cx.background_executor().timer(SCAN_EVERY).await;
            slept = SCAN_EVERY;
        }
        if let Some(m) = weak.upgrade() {
            m.borrow_mut().poller = None;
        }
    }));
}

// ── Actions ──────────────────────────────────────────────────────────────

/// The user's terminal (Studio → Applications), started in `dir`.
fn open_terminal(dir: &Path, cx: &App) -> Result<(), String> {
    let cmd = crate::state::config(cx).read(cx).get().apps.terminal_command().ok_or_else(|| t("No terminal found — choose one in Studio → Applications").to_string())?;
    let argv: Vec<&str> = cmd.split_whitespace().collect();
    let (program, args) = argv.split_first().ok_or_else(|| t("No terminal found — choose one in Studio → Applications").to_string())?;
    use std::os::unix::process::CommandExt as _;
    let mut c = std::process::Command::new(program);
    c.args(args).current_dir(dir).process_group(0);
    mandara_services::reaper::spawn(&mut c).map(|_| ()).map_err(|e| tf("Could not start {terminal}: {error}", &[("terminal", program), ("error", &e)]))
}

fn fetch(m: &Shared, path: PathBuf, cx: &mut App) {
    if !m.borrow_mut().fetching.insert(path.clone()) {
        return;
    }
    m.borrow().views.notify(cx);
    let m = m.clone();
    cx.spawn(async move |cx| {
        let p = path.clone();
        let rx = off_thread("mandara-git-fetch", move || (svc::fetch(&p), svc::inspect(&p)));
        let Ok((result, repo)) = rx.recv().await else { return };
        let views = {
            let mut mb = m.borrow_mut();
            mb.fetching.remove(&path);
            match result {
                Ok(()) => mb.flash.show(true, tf("Fetched {repo}", &[("repo", &repo.name)])),
                Err(e) => mb.flash.show(false, e),
            }
            mb.replace(repo);
            mb.views.clone()
        };
        views.notify_async(cx);
    })
    .detach();
}

// ── Pieces ───────────────────────────────────────────────────────────────

fn state_color(r: &Repo, palette: &MandaraPalette) -> Color {
    let c = if r.error.is_some() || r.status.conflicts > 0 {
        palette.urgent_coral
    } else if r.status.dirty() {
        palette.warning_amber
    } else if r.status.ahead > 0 {
        palette.primary
    } else {
        palette.success_emerald
    };
    readable(c, palette)
}

fn badge(glyph: Option<&'static str>, label: impl Into<SharedString>, tint: Color, palette: &MandaraPalette) -> gpui::Div {
    div()
        .flex_none()
        .h(px(BADGE_H_PX))
        .px(px(BADGE_PAD_X_PX))
        .rounded_full()
        .flex()
        .items_center()
        .gap(px(BADGE_GAP_PX))
        .bg(tint.with_alpha(alpha(BADGE_FILL, palette)).hsla())
        .font_features(tabular_nums())
        .text_size(px(BADGE_TEXT_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(tint.hsla())
        .when_some(glyph, |el, g| el.child(icon(g, BADGE_ICON_PX, tint)))
        .child(div().min_w(px(0.0)).truncate().child(label.into()))
}

fn path_id(prefix: &'static str, path: &Path) -> ElementId {
    use std::hash::{Hash as _, Hasher as _};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    ElementId::NamedInteger(SharedString::new_static(prefix), h.finish())
}

fn actions(r: &Repo, row: &Row, m: &Shared, palette: &MandaraPalette, cx: &App) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let muted = palette.on_surface_variant;
    let fetching = m.borrow().fetching.contains(&r.path);
    let mono = fonts(cx).mono;
    let button = |id: &'static str, glyph: &'static str, label: &'static str, f: Rc<dyn Fn(&mut App)>| {
        pill_button(path_id(id, &r.path), Some(glyph), t(label), accent, palette, move |_, cx| f(cx))
    };
    let (p1, p2, p3, p4) = (r.path.clone(), r.path.clone(), r.path.clone(), r.path.clone());
    let (m1, m2, m3) = (m.clone(), m.clone(), m.clone());
    let terminal = button(
        "git-term",
        "terminal",
        tk("Terminal"),
        Rc::new(move |cx| {
            if let Err(e) = open_terminal(&p1, cx) {
                m1.borrow().flash.show(false, e);
                m1.borrow().views.notify(cx);
            }
        }),
    );
    let folder = button("git-folder", "folder", tk("Open folder"), Rc::new(move |_| super::dock::stacks::open_path(&p2)));
    let copy = button(
        "git-copy",
        "copy",
        tk("Copy path"),
        Rc::new(move |cx| {
            let path = p3.display().to_string();
            match mandara_services::clipboard::serve(path.as_bytes().to_vec(), None) {
                Ok(()) => m2.borrow().flash.show(true, tf("Copied {path}", &[("path", &path)])),
                Err(e) => m2.borrow().flash.show(false, e),
            }
            m2.borrow().views.notify(cx);
        }),
    );
    let fetch_btn = if fetching {
        pill_button(path_id("git-fetching", &r.path), Some("timer"), t("Fetching…"), muted, palette, |_, _| {}).into_any_element()
    } else {
        button("git-fetch", "download", tk("Fetch"), Rc::new(move |cx| fetch(&m3, p4.clone(), cx))).into_any_element()
    };
    div()
        .h(px(ACTIONS_H))
        .px(px(ACTIONS_PAD_PX))
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(ACTIONS_GAP_PX))
        .child(div().flex().items_center().gap(px(ACTIONS_GAP_PX)).child(terminal).child(folder).child(copy).child(fetch_btn))
        .child(div().px(px(LINE_GAP_PX)).truncate().font_family(mono).text_size(px(PATH_TEXT_PX)).text_color(muted.with_alpha(MUTED_ALPHA).hsla()).child(row.path.clone()))
        .into_any_element()
}

fn repo_row(i: usize, m: &Shared, palette: &MandaraPalette, radius: f32, cx: &App) -> AnyElement {
    let mb = m.borrow();
    let (r, row) = (&mb.repos[i], &mb.rows[i]);
    let selected = mb.selected.as_ref() == Some(&r.path);
    let tint = state_color(r, palette);
    let muted = palette.on_surface_variant;
    let hover = palette.on_surface.with_alpha(alpha(ROW_HOVER, palette));
    let sel = readable(palette.primary, palette).with_alpha(alpha(ROW_SELECTED, palette));
    let st = &r.status;
    let ahead_behind = (st.ahead > 0 || st.behind > 0).then(|| {
        let mut s = String::new();
        if st.ahead > 0 {
            s.push_str(&format!("↑{}", st.ahead));
        }
        if st.behind > 0 {
            if !s.is_empty() {
                s.push(' ');
            }
            s.push_str(&format!("↓{}", st.behind));
        }
        s
    });
    let (m2, path) = (m.clone(), r.path.clone());
    let head = div()
        .id(path_id("git-row", &r.path))
        .h(px(ROW_H))
        .flex_none()
        .px(px(ROW_PAD_X_PX))
        .flex()
        .items_center()
        .gap(px(ROW_INNER_GAP_PX))
        .cursor_pointer()
        .when(!selected, |el| el.rounded(px(radius)).hover(move |s| s.bg(hover.hsla())))
        .child(
            div()
                .size(px(AVATAR_PX))
                .flex_none()
                .rounded(px(crate::style::radius(AVATAR_PX * AVATAR_RADIUS_FRAC)))
                .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, tint.with_alpha(alpha(AVATAR_FILL, palette)), partner(tint).with_alpha(alpha(AVATAR_FILL, palette) * 0.5)))
                .flex()
                .items_center()
                .justify_center()
                .child(icon("git-branch", AVATAR_ICON_PX, tint)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(LINE_GAP_PX))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(INLINE_GAP_PX))
                        .min_w(px(0.0))
                        .child(div().min_w(px(0.0)).truncate().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(row.name.clone()))
                        .child(badge(Some("git-branch"), row.branch.clone(), readable(palette.tertiary, palette), palette).max_w(px(BRANCH_MAX_W_PX)))
                        .when_some(ahead_behind, |el, ab| el.child(badge(None, ab, readable(palette.primary, palette), palette))),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(INLINE_GAP_PX))
                        .min_w(px(0.0))
                        .text_size(px(text::LABEL))
                        .text_color(muted.hsla())
                        .child(div().flex_none().text_color(tint.hsla()).font_weight(FontWeight::MEDIUM).child(row.changes.clone()))
                        .when(!row.commit.is_empty(), |el| el.child(div().flex_none().child("·")).child(div().min_w(px(0.0)).truncate().text_color(muted.with_alpha(MUTED_ALPHA).hsla()).child(row.commit.clone()))),
                ),
        )
        .child(div().flex_none().font_features(tabular_nums()).text_size(px(text::LABEL)).text_color(muted.hsla()).child(row.when.clone()))
        .on_click(move |_, _, cx| {
            let views = {
                let mut mb = m2.borrow_mut();
                mb.selected = if mb.selected.as_ref() == Some(&path) { None } else { Some(path.clone()) };
                mb.views.clone()
            };
            views.notify(cx);
        });
    if !selected {
        return head.into_any_element();
    }
    div().flex_none().rounded(px(radius)).bg(sel.hsla()).flex().flex_col().child(head).child(actions(r, row, m, palette, cx)).into_any_element()
}

fn hero(repos: usize, dirty: usize, unpushed: usize, scanned: bool, palette: &MandaraPalette, radius: f32) -> AnyElement {
    let tone = |c: Color, l: f32, (share, cap): (f32, f32)| c.with_lightness(l).with_chroma((c.chroma() * share).min(cap));
    let (bg_a, bg_b) = if palette.is_dark {
        (tone(palette.primary, HERO_DARK_L.0, HERO_DARK_CHROMA.0), tone(palette.tertiary, HERO_DARK_L.1, HERO_DARK_CHROMA.1))
    } else {
        (tone(palette.primary, HERO_LIGHT_L.0, HERO_LIGHT_CHROMA.0), tone(palette.tertiary, HERO_LIGHT_L.1, HERO_LIGHT_CHROMA.1))
    };
    let figure = |value: usize, label: &'static str, tint: Color| {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(HERO_LABEL_GAP_PX))
            .child(
                div()
                    .font_features(tabular_nums())
                    .text_size(px(HERO_VALUE_PX))
                    .line_height(px(HERO_VALUE_LINE_PX))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(tint.hsla())
                    .child(if scanned { value.to_string() } else { "–".into() }),
            )
            .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::MEDIUM).text_color(palette.on_surface_variant.hsla()).child(t(label)))
    };
    let divider = || div().w(px(HERO_DIVIDER_W_PX)).h(px(HERO_DIVIDER_H_PX)).flex_none().rounded_full().bg(palette.on_surface.with_alpha(HERO_DIVIDER_ALPHA).hsla());
    let quiet = palette.on_surface;
    div()
        .h(px(HERO_H))
        .rounded(px(radius))
        .bg(oklab_gradient(HERO_ANGLE_DEG, bg_a, bg_b))
        .shadow(rim(palette))
        .px(px(HERO_PAD_X_PX))
        .flex()
        .items_center()
        .child(figure(repos, tk("Repositories"), quiet))
        .child(divider())
        .child(figure(dirty, tk("Uncommitted"), if dirty > 0 { readable(palette.warning_amber, palette) } else { quiet }))
        .child(divider())
        .child(figure(unpushed, tk("Unpushed"), if unpushed > 0 { readable(palette.primary, palette) } else { quiet }))
        .into_any_element()
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
            // Each opening scans once.
            if !std::mem::replace(&mut m.borrow_mut().opened, true) {
                scan(m, cx);
            }
            let watch = m.borrow().close_watch.clone();
            if !watch.get() {
                let closed = m.clone();
                super::processes::on_panel_close(&self.p, &watch, cx, move |_| {
                    let mut mb = closed.borrow_mut();
                    mb.opened = false;
                    mb.selected = None;
                });
            }
        }
        let flash = m.borrow().flash.current(window, cx);
        let (scanning, scanned, count, dirty, unpushed, roots, selected, busy) = {
            let mb = m.borrow();
            (
                mb.scanning,
                mb.scanned_at,
                mb.repos.len(),
                mb.repos.iter().filter(|r| r.status.dirty()).count(),
                mb.repos.iter().filter(|r| r.status.ahead > 0).count(),
                mb.roots,
                mb.selected.is_some(),
                !mb.fetching.is_empty(),
            )
        };
        if scanning || busy {
            redraw_in(BUSY_TICK, window, cx);
        }
        let shape = PanelShape::of(cx);
        let card_r = shape.inner(PAD);
        let row_r = (card_r - ROW_RADIUS_INSET_PX).max(ROW_MIN_RADIUS_PX);

        let accent = readable(palette.primary, palette);
        let subtitle = match (scanning, scanned) {
            (true, _) => t("Scanning…").to_string(),
            (false, Some(at)) => tf("Checked {span} ago", &[("span", &super::panel_fx::duration_short(at.elapsed().as_secs() as i64))]),
            (false, None) => t("Not scanned yet").to_string(),
        };
        let rescan = {
            let m = m.clone();
            icon_button("git-rescan", "refresh", REFRESH_BTN_PX, palette.on_surface_variant, None, move |_, cx| scan(&m, cx)).into_any_element()
        };
        let head = header("git-branch", accent, t("Git repositories"), subtitle, palette, Some(rescan));

        let rows: Vec<AnyElement> = (0..count).map(|i| repo_row(i, m, palette, row_r, cx)).collect();
        let content_h = count as f32 * (ROW_H + ROW_GAP) + if selected { ACTIONS_H } else { 0.0 };
        let list_h = if count == 0 { EMPTY_H } else { content_h.min(LIST_MAX_H) };
        let list: AnyElement = if count == 0 {
            match (scanned.is_some(), roots) {
                (false, _) => empty_state("git-branch", t("Looking for repositories…"), t("Reading your project folders"), EMPTY_H, palette),
                (true, 0) => empty_state("folder", t("No project folders found"), t("Set the folders to search in Studio → Plugins → Git repositories"), EMPTY_H, palette),
                (true, _) => empty_state("git-branch", t("No repositories found"), t("Clone or init one in your project folders"), EMPTY_H, palette),
            }
            .into_any_element()
        } else {
            let fill = crate::shell::frame_color(palette);
            let clipped = content_h > LIST_MAX_H + SIZE_EPSILON_PX;
            div()
                .relative()
                .h(px(list_h))
                .child(div().id("git-list").size_full().overflow_y_scroll().flex().flex_col().gap(px(ROW_GAP)).children(rows))
                .when(clipped, |el| el.child(div().absolute().left_0().right_0().bottom_0().h(px(LIST_FADE_PX)).bg(oklab_gradient(ANGLE_DOWN_DEG, fill.with_alpha(0.0), fill))))
                .into_any_element()
        };
        let height = PAD * 2.0 + HEADER_H + GAP + HERO_H + GAP + list_h;
        if (f32::from(self.size_cell.get().height) - height).abs() > SIZE_EPSILON_PX {
            self.size_cell.set(size(px(PANEL_W), px(height)));
        }
        let p = &self.p;
        div()
            .relative()
            .size_full()
            .p(px(PAD))
            .flex()
            .flex_col()
            .gap(px(GAP))
            .child(reveal(p, 0, head))
            .child(reveal(p, 1, hero(count, dirty, unpushed, scanned.is_some(), palette, card_r)))
            .child(reveal(p, 2, list))
            .when_some(flash, |el, (ok, msg)| el.child(toast(ok, msg, PAD, palette)))
            .into_any_element()
    }
}

fn chip(m: &Shared, palette: &MandaraPalette, window: &Window, cx: &mut App) -> AnyElement {
    keep_alive(m, window, cx);
    let n = m.borrow().attention();
    let tint = if n > 0 { readable(palette.warning_amber, palette) } else { palette.on_surface };
    div()
        .flex()
        .items_center()
        .gap(px(CHIP_GAP_PX))
        .child(icon("git-branch", crate::style::bar_icon(CHIP_ICON_PX), palette.on_surface))
        .when(n > 0, |el| el.child(div().font_features(tabular_nums()).text_size(px(CHIP_TEXT_PX)).font_weight(FontWeight::SEMIBOLD).text_color(tint.hsla()).child(n.to_string())))
        .into_any_element()
}

// ── Commands ─────────────────────────────────────────────────────────────

/// Scans on its own thread; the CLI receives the JSON summary.
fn scan_now(cx: &App) -> Output {
    let (roots, depth) = settings(cx);
    let (tx, rx) = async_channel::bounded(1);
    let _ = std::thread::Builder::new().name("mandara-git-cmd".into()).spawn(move || {
        let _ = tx.send_blocking(Ok(summary_json(&scan_blocking(&roots, depth).1).to_string()));
    });
    Output::Pending(rx)
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Git repositories"), tk("Your local repositories at a glance: branch, uncommitted changes, unpushed commits"), Some(Zone::Right)).opt_in(), |window, cx, panel_cx| {
        let m = model();
        let bar_model = m.clone();
        let bar = fn_view(None, move |palette, window, cx| chip(&bar_model, palette, window, cx), window, cx);
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(PAD * 2.0 + HEADER_H + GAP + HERO_H + GAP + EMPTY_H))));
        let view = View { p: panel_cx.clone(), m, size_cell: size_cell.clone() };
        let panel = fn_view(None, move |palette, window, cx| view.render(palette, window, cx), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: size_cell }), ..PluginInstance::empty() }
    });
    r.command(
        ID,
        CommandSpec::new_output("git list", tk("Local Git repositories with branch, ahead / behind, changes and last commit (most recent first)"), |_, cx| Ok(scan_now(cx))).example("git list"),
    );
    r.command(
        ID,
        CommandSpec::new_output("git fetch", tk("Run git fetch in one repository (by folder name or path) and return its new state"), |a, cx| {
            let target = a.rest(0);
            let (roots, depth) = settings(cx);
            let (tx, rx) = async_channel::bounded(1);
            let _ = std::thread::Builder::new().name("mandara-git-cmd".into()).spawn(move || {
                let path = PathBuf::from(&target);
                let found = if path.join(".git").exists() {
                    Some(path)
                } else {
                    scan_blocking(&roots, depth).1.into_iter().find(|r| r.name.eq_ignore_ascii_case(&target)).map(|r| r.path)
                };
                let result = match found {
                    None => Err(tf("No repository named {repo} in your project folders", &[("repo", &target)])),
                    Some(p) => svc::fetch(&p).map(|()| json!(svc::inspect(&p)).to_string()),
                };
                let _ = tx.send_blocking(result);
            });
            Ok(Output::Pending(rx))
        })
        .arg(ArgSpec::required("repo", ArgKind::String, tk("folder name or path of the repository")))
        .example("git fetch mandara"),
    );
    r.command(
        ID,
        CommandSpec::new_output("git refresh", tk("Rescan the repositories now and update the bar chip"), |_, cx| {
            let Some(m) = MODEL.with(|w| w.borrow().upgrade()) else { return Ok(scan_now(cx)) };
            let (tx, rx) = async_channel::bounded(1);
            m.borrow_mut().waiters.push(tx);
            scan(&m, cx);
            Ok(Output::Pending(rx))
        })
        .example("git refresh"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_services::git_repos::{Commit, Status};

    fn repo(name: &str, status: Status) -> Repo {
        Repo { path: PathBuf::from(format!("/p/{name}")), name: name.into(), status, last_commit: Some(Commit { time: 1, author: "a".into(), subject: "fix: it".into() }), activity: 1, error: None }
    }

    #[test]
    fn labels_summarise_changes() {
        let dirty = row_labels(&repo("a", Status { branch: Some("main".into()), staged: 1, modified: 2, untracked: 3, ..Status::default() }));
        assert_eq!(dirty.changes.as_ref(), "1 staged · 2 modified · 3 untracked");
        assert_eq!((dirty.branch.as_ref(), dirty.commit.as_ref()), ("main", "fix: it"));
        let clean = row_labels(&repo("b", Status { head: Some("4f1c2a9e0d".into()), ..Status::default() }));
        assert_eq!((clean.changes.as_ref(), clean.branch.as_ref()), ("Clean", "detached at 4f1c2a9"));
    }

    #[test]
    fn attention_counts_dirty_and_unpushed() {
        let mut m = Model::new();
        m.absorb(1, vec![repo("a", Status { modified: 1, ..Status::default() }), repo("b", Status { ahead: 2, ..Status::default() }), repo("c", Status::default())]);
        assert_eq!(m.attention(), 2);
        let s = summary_json(&m.repos);
        assert_eq!((s["count"].as_u64(), s["dirty"].as_u64(), s["unpushed"].as_u64()), (Some(3), Some(1), Some(1)));
        m.selected = Some(PathBuf::from("/p/gone"));
        m.absorb(1, Vec::new());
        assert!(m.selected.is_none(), "a vanished selection is cleared");
    }
}
