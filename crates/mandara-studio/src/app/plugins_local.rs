//! Studio → Plugins: the store's places ([`PluginsTab`]) and the **Develop**
//! tab (slug `local`), the plugin author's handbook built from live data:
//!
//! 1. *Where plugins live*: every search folder resolved to its real path,
//!    whether it exists, how many plugins it holds, its origin semantics and
//!    the precedence rules, with open / create / copy actions.
//! 2. *Plugin format*: the folder layout and an annotated `plugin.toml`
//!    ([`authoring::EXAMPLE_MANIFEST`], parsed by a unit test).
//! 3. *Architecture*: build pipeline → sandboxed host → native views, hot
//!    reload and settings flows, with the real budgets from the runtime.
//! 4. *The contract*: exports, host imports, node kinds and tones parsed
//!    from the embedded `plugin.wit`, plus the full file.
//! 5. *Quick start*: copyable commands and the publishing summary.
//! 6. *Validate*: every folder entry with its discovery verdict, the
//!    running shell's load state (with the actual error) and a test load.
//!
//! Live: a folder watcher hot-reloads Studio's plugin registry, and the
//! page rescans whenever the registry's generation moves.

use mandara_ui::bidi::Bidi as _;
use mandara_ui::style::ClipRounded as _;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Bounds, ClickEvent, ClipboardItem, Context, Div, FontWeight, InteractiveElement, IntoElement, ParentElement, Pixels,
    SharedString, Stateful, StatefulInteractiveElement, Styled, canvas, div, point, prelude::FluentBuilder as _, px,
};
use mandara_config::ShellConfig;
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tr};
use mandara_plugins::authoring::{self, LAYOUT};
use mandara_plugins::wasm::{self, Discovered, FolderReport, Inspection, Source};
use mandara_plugins::{PluginOrigin, PluginRegistry};
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::icons::icon;

use super::StudioView;

// ── Design tokens ────────────────────────────────────────────────────────

const PAGE_GAP_PX: f32 = 18.0;
const CARD_RADIUS_PX: f32 = 22.0;
const CARD_PAD_PX: f32 = 20.0;
const CARD_GAP_PX: f32 = 16.0;
const INNER_RADIUS_PX: f32 = 16.0;
const INNER_PAD_PX: f32 = 14.0;
const TILE_PX: f32 = 38.0;
const TILE_RADIUS_PX: f32 = 12.0;
const TILE_ICON_PX: f32 = 18.0;
const TITLE_TEXT_PX: f32 = 16.0;
const BODY_TEXT_PX: f32 = 12.5;
const SMALL_TEXT_PX: f32 = 11.5;
const LABEL_TEXT_PX: f32 = 11.0;
const LINE_HEIGHT_PX: f32 = 18.0;
const CHIP_H_PX: f32 = 22.0;
const CHIP_PAD_X_PX: f32 = 8.0;
const BUTTON_H_PX: f32 = 30.0;
const BUTTON_PAD_X_PX: f32 = 12.0;
const BUTTON_ICON_PX: f32 = 13.0;
const SMALL_ICON_PX: f32 = 14.0;
const TINT_ALPHA: f32 = 0.14;
const SOFT_ALPHA: f32 = 0.08;
const HOVER_OPACITY: f32 = 0.85;
/// Code blocks.
const CODE_RADIUS_PX: f32 = 14.0;
const CODE_PAD_PX: f32 = 14.0;
const CODE_TEXT_PX: f32 = 12.0;
const CODE_LINE_PX: f32 = 19.0;
const GUTTER_W_PX: f32 = 30.0;
/// Hero gradient stops (OKLab, primary → tertiary).
const HERO_ANGLE_DEG: f32 = 135.0;
const HERO_FROM_ALPHA: f32 = 0.22;
const HERO_TO_ALPHA: f32 = 0.12;
const HERO_RADIUS_PX: f32 = 24.0;
const HERO_PAD_PX: f32 = 22.0;
const HERO_TITLE_PX: f32 = 20.0;
/// Smallest architecture diagram node (they share the row).
const NODE_MIN_W_PX: f32 = 96.0;
/// Node width when the diagram stacks vertically (narrow windows).
const NODE_STACKED_W_PX: f32 = 240.0;
const NODE_ICON_PX: f32 = 30.0;
/// Room kept above a section scrolled into view.
const SCROLL_MARGIN_PX: f32 = 12.0;
/// Precedence number badges, and the compact row of a missing system folder.
const BADGE_PX: f32 = 28.0;
const COMPACT_PAD_Y_PX: f32 = 10.0;
/// How long a copy button says "Copied".
const COPIED_FEEDBACK: Duration = Duration::from_millis(1600);
/// Frames to wait for section anchors before scrolling to a deep link.
const ANCHOR_SETTLE: Duration = Duration::from_millis(32);
/// After a folder change, the shell reloads after its own debounce: ask it
/// again once it has.
const SHELL_SETTLE: Duration = Duration::from_millis(1200);

// ── Tabs and sections ────────────────────────────────────────────────────

/// The Plugins page's places (store navigation).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum PluginsTab {
    /// The store front: featured hero and curated shelves.
    #[default]
    Discover,
    Installed,
    Updates,
    /// The community marketplace (GitHub).
    Marketplace,
    /// The plugin author's handbook.
    Local,
}

impl PluginsTab {
    pub const ALL: [PluginsTab; 5] = [PluginsTab::Discover, PluginsTab::Installed, PluginsTab::Updates, PluginsTab::Marketplace, PluginsTab::Local];

    /// Deep-link name (`mandara studio plugins --tab local`).
    pub fn slug(self) -> &'static str {
        match self {
            PluginsTab::Discover => "discover",
            PluginsTab::Installed => "installed",
            PluginsTab::Updates => "updates",
            PluginsTab::Marketplace => "marketplace",
            PluginsTab::Local => "local",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|t| t.slug() == s).or(match s.as_str() {
            "browse" | "featured" => Some(PluginsTab::Discover),
            "library" => Some(PluginsTab::Installed),
            "update" => Some(PluginsTab::Updates),
            "market" | "store" | "community" => Some(PluginsTab::Marketplace),
            "dev" | "develop" | "development" => Some(PluginsTab::Local),
            _ => None,
        })
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            PluginsTab::Discover => t("Discover"),
            PluginsTab::Installed => t("Installed"),
            PluginsTab::Updates => t("Updates"),
            PluginsTab::Marketplace => t("Community"),
            PluginsTab::Local => t("Develop"),
        }
    }

    pub(super) fn icon(self) -> &'static str {
        match self {
            PluginsTab::Discover => "zap",
            PluginsTab::Installed => "grid",
            PluginsTab::Updates => "refresh",
            PluginsTab::Marketplace => "globe",
            PluginsTab::Local => "terminal",
        }
    }
}

/// The Local tab's sections (deep link: `--scroll <slug>`).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum LocalSection {
    Folders,
    Format,
    Architecture,
    Contract,
    QuickStart,
    Validate,
}

impl LocalSection {
    pub const ALL: [LocalSection; 6] = [LocalSection::Folders, LocalSection::Format, LocalSection::Architecture, LocalSection::Contract, LocalSection::QuickStart, LocalSection::Validate];

    pub fn slug(self) -> &'static str {
        match self {
            LocalSection::Folders => "folders",
            LocalSection::Format => "format",
            LocalSection::Architecture => "architecture",
            LocalSection::Contract => "contract",
            LocalSection::QuickStart => "quick-start",
            LocalSection::Validate => "validate",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase().replace('_', "-");
        Self::ALL.into_iter().find(|x| x.slug() == s).or(match s.as_str() {
            "where" | "paths" | "directories" => Some(LocalSection::Folders),
            "manifest" | "plugin.toml" => Some(LocalSection::Format),
            "wit" | "api" => Some(LocalSection::Contract),
            "quickstart" | "start" => Some(LocalSection::QuickStart),
            "status" | "debug" => Some(LocalSection::Validate),
            _ => None,
        })
    }

    fn title(self) -> &'static str {
        match self {
            LocalSection::Folders => t("Where plugins live"),
            LocalSection::Format => t("Plugin format"),
            LocalSection::Architecture => t("Architecture"),
            LocalSection::Contract => t("The contract (WIT)"),
            LocalSection::QuickStart => t("Quick start"),
            LocalSection::Validate => t("Validate"),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            LocalSection::Folders => "folder",
            LocalSection::Format => "package",
            LocalSection::Architecture => "server",
            LocalSection::Contract => "key",
            LocalSection::QuickStart => "zap",
            LocalSection::Validate => "list-check",
        }
    }
}

// ── State ────────────────────────────────────────────────────────────────

/// The running shell's view of a third-party plugin.
#[derive(Clone, Debug, PartialEq)]
struct Runtime {
    /// `running`, `failed`, `disabled`, `not-loaded` (or `unknown` from an
    /// older shell).
    state: String,
    error: Option<String>,
}

/// A test load in Studio's own process.
#[derive(Clone, Debug)]
enum TestState {
    Running,
    Done(Result<TestReport, String>),
}

#[derive(Clone, Debug)]
struct TestReport {
    elapsed: Duration,
    bar: bool,
    panel: bool,
    sidebar: bool,
}

/// Local tab state, owned by [`StudioView`].
#[derive(Default)]
pub struct Local {
    reports: Vec<FolderReport>,
    /// (registry generation, folder-event generation) of `reports`.
    scanned: Option<(u64, u64)>,
    fs_generation: u64,
    watching: bool,
    /// From `plugins list` over IPC: `Err` when the shell isn't reachable.
    runtime: Option<Result<HashMap<String, Runtime>, String>>,
    tests: HashMap<PathBuf, TestState>,
    /// Key of the copy button showing "Copied".
    copied: Option<String>,
    copied_task: Option<gpui::Task<()>>,
    wit_open: bool,
    message: Option<(bool, String)>,
    /// Painted bounds of each section (for scrolling to it).
    anchors: Rc<RefCell<HashMap<LocalSection, Bounds<Pixels>>>>,
    /// A section to scroll to once laid out, and frames waited so far.
    pending_scroll: Option<(LocalSection, u8)>,
}

/// Frames to let pass before trusting recorded anchors.
const ANCHOR_FRAMES: u8 = 2;

impl StudioView {
    /// Switches the Plugins page to `tab`, optionally scrolling the Local
    /// tab to a section (deep links).
    pub fn show_plugins_tab(&mut self, tab: PluginsTab, scroll: Option<LocalSection>, cx: &mut Context<Self>) {
        self.section = crate::registry::Section::Plugins;
        self.query.clear();
        self.search.update(cx, |f, _| f.value.clear());
        self.plugins_tab = tab;
        match scroll {
            Some(section) if tab == PluginsTab::Local => self.local.pending_scroll = Some((section, 0)),
            _ => self.content_scroll.set_offset(point(px(0.0), px(0.0))),
        }
        cx.notify();
    }

    /// Keeps the store following the plugin folders (a folder dropped into
    /// plugins-dev shows up under Installed too, not only after the Develop
    /// tab was opened once); the number of local plugins, once scanned.
    pub(super) fn local_follow(&mut self, cx: &mut Context<Self>) -> Option<usize> {
        self.local_watch(cx);
        self.local.scanned.map(|_| self.local.reports.iter().map(FolderReport::plugin_count).sum::<usize>())
    }

    /// Rescans the folders when the registry or the folders changed.
    fn local_refresh(&mut self, force: bool, cx: &mut Context<Self>) {
        let key = (cx.global::<PluginRegistry>().generation, self.local.fs_generation);
        if !force && self.local.scanned == Some(key) {
            return;
        }
        let first = self.local.scanned.is_none();
        self.local.scanned = Some(key);
        self.local.reports = wasm::inspect(&wasm::plugin_dirs());
        self.local_query_shell(Duration::ZERO, cx);
        if !first {
            self.local_query_shell(SHELL_SETTLE, cx);
        }
    }

    /// Asks the running shell for its plugins' runtime state.
    fn local_query_shell(&mut self, delay: Duration, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let Ok(task) = this.update(cx, |_, cx| crate::perf::query(&["plugins", "list"], cx)) else { return };
            let result = match task {
                Some(task) => task.await,
                None => Err(String::new()),
            };
            let parsed = result.map(|v| {
                v.as_array()
                    .into_iter()
                    .flatten()
                    .filter(|e| e.pointer("/origin/kind").and_then(|k| k.as_str()) == Some("third-party"))
                    .filter_map(|e| {
                        let id = e.get("id")?.as_str()?.to_string();
                        let rt = e.get("runtime");
                        let state = rt.and_then(|r| r.get("state")).and_then(|s| s.as_str()).unwrap_or("unknown").to_string();
                        let error = rt.and_then(|r| r.get("error")).and_then(|s| s.as_str()).map(str::to_string);
                        Some((id, Runtime { state, error }))
                    })
                    .collect::<HashMap<_, _>>()
            });
            let _ = this.update(cx, |this, cx| {
                this.local.runtime = Some(parsed);
                cx.notify();
            });
        })
        .detach();
    }

    /// Watches the plugin folders (and their parent, to see them created):
    /// hot-reloads Studio's registry and rescans this page.
    fn local_watch(&mut self, cx: &mut Context<Self>) {
        use notify::Watcher as _;
        if self.local.watching {
            return;
        }
        self.local.watching = true;
        let dirs: Vec<PathBuf> = wasm::plugin_dirs().into_iter().map(|(d, _)| d).collect();
        let parent = mandara_config::paths::data_dir();
        let (tx, rx) = async_channel::unbounded::<()>();
        let filter = dirs.clone();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(e) = res
                && !matches!(e.kind, notify::EventKind::Access(_))
                && e.paths.iter().any(|p| filter.iter().any(|d| p.starts_with(d) || d.starts_with(p)))
            {
                let _ = tx.send_blocking(());
            }
        });
        let mut watcher = match watcher {
            Ok(w) => w,
            Err(err) => {
                tracing::warn!("Studio: watching plugin folders: {err}");
                return;
            }
        };
        let mut watched: HashSet<PathBuf> = HashSet::new();
        let mut arm = move |w: &mut notify::RecommendedWatcher| {
            for d in &dirs {
                if d.is_dir() && watched.insert(d.clone()) {
                    let _ = w.watch(d, notify::RecursiveMode::Recursive);
                }
            }
        };
        let _ = watcher.watch(&parent, notify::RecursiveMode::NonRecursive);
        arm(&mut watcher);
        cx.spawn(async move |this, cx| {
            let mut watcher = watcher;
            while rx.recv().await.is_ok() {
                cx.background_executor().timer(mandara_ui::plugins::external::RELOAD_DEBOUNCE).await;
                while rx.try_recv().is_ok() {}
                arm(&mut watcher);
                let alive = this.update(cx, |this, cx| {
                    mandara_ui::plugins::external::sync(cx);
                    this.local.fs_generation += 1;
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn local_copy(&mut self, key: String, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.local.copied = Some(key);
        self.local.copied_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FEEDBACK).await;
            let _ = this.update(cx, |this, cx| {
                this.local.copied = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Loads a plugin in Studio's own sandbox: compile, instantiate, `init`,
    /// then one `bar` / `panel` / `sidebar` call each.
    fn local_test(&mut self, plugin: Discovered, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) {
        let key = plugin.dir.clone();
        if matches!(self.local.tests.get(&key), Some(TestState::Running)) {
            return;
        }
        let mut settings: serde_json::Map<String, serde_json::Value> = plugin.settings.iter().map(|s| (s.key.clone(), s.default_json())).collect();
        if let Some(table) = cfg.plugins.settings.get(&plugin.id).and_then(|t| serde_json::to_value(t).ok()).and_then(|v| v.as_object().cloned()) {
            settings.extend(table);
        }
        let palette = mandara_ui::plugins::external::palette_for(p);
        self.local.tests.insert(key.clone(), TestState::Running);
        let task = cx.background_executor().spawn(async move {
            let allow = plugin.permissions.iter().any(|x| x == wasm::PERMISSION_COMMANDS);
            mandara_plugins::guard(&format!("plugin `{}` test load", plugin.id), move || {
                let start = Instant::now();
                let mut w = wasm::WasmPlugin::load(&plugin.id, &plugin.dir, &plugin.entry, settings, allow).map_err(|e| format!("{e:#}"))?;
                let (bar, panel, sidebar) = (w.bar(&palette).is_some(), w.panel(&palette).is_some(), w.sidebar(&palette).is_some());
                if let Some(reason) = w.disabled.clone() {
                    return Err(reason);
                }
                Ok(TestReport { elapsed: start.elapsed(), bar, panel, sidebar })
            })
            .unwrap_or_else(|| Err("the loader crashed".into()))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.local.tests.insert(key, TestState::Done(result));
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Scrolls the page to a pending deep-linked section once its anchor
    /// has been painted.
    fn local_scroll(&mut self, cx: &mut Context<Self>) {
        let Some((section, frames)) = self.local.pending_scroll else { return };
        let anchor = self.local.anchors.borrow().get(&section).copied();
        let viewport = self.content_scroll.bounds();
        match anchor {
            Some(b) if frames >= ANCHOR_FRAMES && viewport.size.height > px(0.0) => {
                let offset = self.content_scroll.offset();
                let max = self.content_scroll.max_offset();
                let y = (offset.y - (b.top() - viewport.top()) + px(SCROLL_MARGIN_PX)).min(px(0.0)).max(-max.y);
                self.content_scroll.set_offset(point(offset.x, y));
                self.local.pending_scroll = None;
                cx.notify();
            }
            _ => {
                self.local.pending_scroll = Some((section, frames.saturating_add(1)));
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(ANCHOR_SETTLE).await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
                .detach();
            }
        }
    }

    /// The Local tab.
    pub(super) fn plugins_local(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        self.local_watch(cx);
        self.local_refresh(false, cx);
        self.local_scroll(cx);
        let mono: SharedString = mandara_ui::fonts::fonts(cx).mono.clone();
        let hero = self.local_hero(p, cx);
        let folders = self.local_folders(p, &mono, cx);
        let format = self.local_format(p, &mono, cx);
        let architecture = self.local_architecture(p, &mono);
        let contract = self.local_contract(p, &mono, cx);
        let quick = self.local_quick_start(p, &mono, cx);
        let validate = self.local_validate(cfg, p, &mono, cx);
        let anchors = self.local.anchors.clone();
        let anchored = move |section: LocalSection, el: Div| {
            let anchors = anchors.clone();
            el.relative().child(
                canvas(move |bounds, _, _| {
                    anchors.borrow_mut().insert(section, bounds);
                }, |_, _, _, _| {})
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
        };
        let message = self.local.message.clone().map(|(ok, m)| {
            let c = if ok { p.success_emerald } else { p.urgent_coral };
            div().px(px(INNER_PAD_PX)).py(px(10.0)).rounded(px(12.0)).text_size(px(BODY_TEXT_PX)).bg(c.with_alpha(TINT_ALPHA - 0.02).hsla()).text_color(c.hsla()).child(m)
        });
        div()
            .row()
            .flex_col()
            .gap(px(PAGE_GAP_PX))
            .child(hero)
            .children(message)
            .child(anchored(LocalSection::Folders, folders))
            .child(anchored(LocalSection::Format, format))
            .child(anchored(LocalSection::Architecture, architecture))
            .child(anchored(LocalSection::Contract, contract))
            .child(anchored(LocalSection::QuickStart, quick))
            .child(anchored(LocalSection::Validate, validate))
            .into_any_element()
    }

    fn local_hero(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> Div {
        let chips = LocalSection::ALL.into_iter().enumerate().map(|(i, s)| {
            div()
                .id(SharedString::from(format!("local-jump-{}", s.slug())))
                .h(px(BUTTON_H_PX))
                .px(px(BUTTON_PAD_X_PX))
                .rounded_full()
                .row()
                .items_center()
                .gap(px(7.0))
                .cursor_pointer()
                .bg(p.surface_base.with_alpha(0.55).hsla())
                .shadow(mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow))
                .text_size(px(BODY_TEXT_PX))
                .font_weight(FontWeight::MEDIUM)
                .hover(|st| st.bg(p.surface_base.with_alpha(0.85).hsla()))
                .child(div().text_color(p.primary.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(format!("{}", i + 1)))
                .child(s.title())
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.local.pending_scroll = Some((s, ANCHOR_FRAMES));
                    cx.notify();
                }))
        });
        div()
            .rounded(px(HERO_RADIUS_PX))
            .p(px(HERO_PAD_PX))
            .row()
            .flex_col()
            .gap(px(12.0))
            .bg(mandara_core::oklab_gradient(HERO_ANGLE_DEG, p.primary.with_alpha(HERO_FROM_ALPHA), p.tertiary.with_alpha(HERO_TO_ALPHA)))
            .shadow(mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow))
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(12.0))
                    .child(tile("terminal", p.primary, p))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .row()
                            .flex_col()
                            .gap(px(2.0))
                            .child(div().text_size(px(HERO_TITLE_PX)).font_weight(FontWeight::SEMIBOLD).child(t("Build your own plugins")))
                            .child(div().text_size(px(BODY_TEXT_PX)).line_height(px(LINE_HEIGHT_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Mandara plugins are Rust crates compiled to sandboxed WebAssembly components. Everything below is read from this system and this build of Mandara.")))),
                    ),
            )
            .child(div().row().flex_wrap().gap(px(8.0)).children(chips))
    }

    // ── 1. Folders ───────────────────────────────────────────────────────

    fn local_folders(&mut self, p: &MandaraPalette, mono: &SharedString, cx: &mut Context<Self>) -> Div {
        let mut rows = Vec::new();
        for (i, report) in self.local.reports.iter().enumerate() {
            let (origin, about, color) = source_info(report.source, p);
            let path = display_dir(&report.dir);
            let count = report.plugin_count();
            let shadowed = report.entries.iter().filter(|(_, i)| matches!(i, Inspection::Shadowed { .. })).count();
            let open_dir = report.dir.clone();
            let create_dir = report.dir.clone();
            let copy_key = format!("folder-{i}");
            let copied = self.local.copied.as_deref() == Some(copy_key.as_str());
            let semantics: Vec<(&'static str, Color)> = match report.source {
                Source::Development => vec![(t("Enabled automatically"), p.success_emerald), (t("Hot reload"), p.primary)],
                Source::Installed => vec![(t("Opt-in"), p.secondary), (t("Hot reload"), p.primary)],
                Source::System => vec![(t("Opt-in"), p.secondary), (t("Read-only"), p.on_surface_variant), (t("Hot reload"), p.primary)],
            };
            let state = if report.exists { chip_with_icon("check", t("Exists"), p.success_emerald) } else { chip_with_icon("alert", t("Missing"), p.warning_amber) };
            let mut actions = div().row().flex_wrap().gap(px(8.0));
            if report.exists {
                actions = actions.child(button(format!("folder-open-{i}"), "external", t("Open folder"), false, p).on_click(move |_, _, _| {
                    let _ = mandara_services::session::open_uri(&format!("file://{}", open_dir.display()));
                }));
            } else if report.source != Source::System {
                actions = actions.child(button(format!("folder-create-{i}"), "plus", t("Create folder"), true, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.local.message = Some(match std::fs::create_dir_all(&create_dir) {
                        Ok(()) => (true, tf("Created {path}", &[("path", &create_dir.display())])),
                        Err(e) => (false, tf("Couldn't create {path}: {error}", &[("path", &create_dir.display()), ("error", &e)])),
                    });
                    this.local.fs_generation += 1;
                    this.local_refresh(true, cx);
                    cx.notify();
                })));
            }
            let path_text = path.clone();
            actions = actions.child(
                button(copy_key.clone(), if copied { "check" } else { "copy" }, if copied { t("Copied") } else { t("Copy path") }, false, p)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.local_copy(copy_key.clone(), path_text.clone(), cx))),
            );
            let badge = div()
                .flex_none()
                .size(px(BADGE_PX))
                .rounded_full()
                .row()
                .items_center()
                .justify_center()
                .bg(color.with_alpha(TINT_ALPHA).hsla())
                .text_color(color.hsla())
                .text_size(px(BODY_TEXT_PX))
                .font_weight(FontWeight::SEMIBOLD)
                .font_features(mandara_ui::fonts::tabular_nums())
                .child(format!("{}", i + 1));
            // A system folder no package created: one compact line.
            if report.source == Source::System && !report.exists {
                rows.push(
                    inner(p)
                        .py(px(COMPACT_PAD_Y_PX))
                        .row()
                        .flex_wrap()
                        .items_center()
                        .gap(px(10.0))
                        .child(badge)
                        .child(div().text_size(px(BODY_TEXT_PX + 0.5)).font_weight(FontWeight::SEMIBOLD).child(origin))
                        .child(div().flex_1().min_w(px(220.0)).font_family(mono.clone()).text_size(px(CODE_TEXT_PX)).text_color(p.on_surface_variant.hsla()).child(path))
                        .child(state)
                        .child(actions),
                );
                continue;
            }
            rows.push(
                inner(p)
                    .row()
                    .gap(px(14.0))
                    .child(badge)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .row()
                            .flex_col()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .row()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(div().text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).child(origin))
                                    .child(state)
                                    .child(chip(if count == 1 { t("1 plugin").to_string() } else { tf("{n} plugins", &[("n", &count)]) }, if count > 0 { p.primary } else { p.on_surface_variant }).font_features(mandara_ui::fonts::tabular_nums()))
                                    .when(shadowed > 0, |el| el.child(chip_with_icon("minus", tf("{n} shadowed", &[("n", &shadowed)]), p.on_surface_variant).font_features(mandara_ui::fonts::tabular_nums())))
                                    .children(semantics.into_iter().map(|(s, c)| chip(s, c))),
                            )
                            .child(div().font_family(mono.clone()).text_size(px(CODE_TEXT_PX + 0.5)).text_color(p.on_surface.hsla()).child(path))
                            .child(div().text_size(px(BODY_TEXT_PX)).line_height(px(LINE_HEIGHT_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(about)))
                            .child(actions),
                    ),
            );
        }
        let rule = |icon_name: &'static str, text: &'static str| {
            div().min_w(px(0.0)).row().items_start().gap(px(10.0)).child(div().pt(px(1.0)).child(icon(icon_name, SMALL_ICON_PX, p.primary))).child(div().flex_1().min_w(px(0.0)).text_size(px(BODY_TEXT_PX)).line_height(px(LINE_HEIGHT_PX)).text_color(p.on_surface_variant.hsla()).child(text))
        };
        card(p)
            .child(section_header(LocalSection::Folders, t("Mandara looks for plugins in these folders, in this order. Each plugin is a sub-folder."), p))
            .children(rows)
            .child(
                div()
                    .row()
                    .flex_col()
                    .gap(px(8.0))
                    .child(rule("list-check", t("First match wins: when the same id is in several folders, the one higher in this list is used and the others are skipped.")))
                    .child(rule("shield", t("Built-in ids can't be overridden: a plugin whose id matches a built-in plugin is skipped.")))
                    .child(rule("box", t("System folders come from $XDG_DATA_DIRS: distribution packages create them, and they're opt-in and read-only.")))
                    .child(rule("refresh", t("Changes are picked up live: add, rebuild or delete a plugin and the shell and this page update within a second."))),
            )
    }

    // ── 2. Format ────────────────────────────────────────────────────────

    fn local_format(&mut self, p: &MandaraPalette, mono: &SharedString, cx: &mut Context<Self>) -> Div {
        let tree_rows = LAYOUT.iter().map(|e| {
            let glyph = if e.path.ends_with('/') { "folder" } else { "note" };
            div()
                .row()
                .items_start()
                .gap(px(10.0))
                .pad_start(px(e.depth as f32 * 22.0))
                .child(div().pt(px(1.0)).child(icon(glyph, SMALL_ICON_PX, if e.required { p.primary } else { p.on_surface_variant })))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .row()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .row()
                                .flex_wrap()
                                .items_center()
                                .gap(px(8.0))
                                .child(div().font_family(mono.clone()).text_size(px(CODE_TEXT_PX + 0.5)).font_weight(FontWeight::MEDIUM).child(if e.depth > 0 { format!("{}  ·  {}", e.path, t("es.toml, fr.toml, …")) } else { e.path.to_string() }))
                                .child(if e.required { chip(t("Required"), p.primary) } else { chip(t("Optional"), p.on_surface_variant) }),
                        )
                        .child(div().text_size(px(SMALL_TEXT_PX + 0.5)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(tr(e.note)))),
                )
        });
        let tree = inner(p)
            .row()
            .flex_col()
            .gap(px(12.0))
            .child(div().row().items_center().gap(px(8.0)).child(icon("folder", SMALL_ICON_PX, p.primary)).child(div().font_family(mono.clone()).text_size(px(CODE_TEXT_PX + 0.5)).font_weight(FontWeight::SEMIBOLD).child("my-plugin/")))
            .children(tree_rows);
        let kinds = [
            ("bool", t("Switch")),
            ("number", t("Slider with min and max, otherwise a number field")),
            ("text", t("Text field")),
            ("choice", t("One of choices: plain strings or { value, label } tables")),
        ];
        let kinds = inner(p)
            .row()
            .flex_col()
            .gap(px(8.0))
            .child(div().text_size(px(BODY_TEXT_PX)).font_weight(FontWeight::SEMIBOLD).child(t("Setting types")))
            .children(kinds.into_iter().map(|(k, d)| {
                div().row().items_center().gap(px(10.0)).child(div().w(px(64.0)).flex_none().font_family(mono.clone()).text_size(px(CODE_TEXT_PX)).text_color(p.primary.hsla()).child(k)).child(div().flex_1().min_w(px(0.0)).text_size(px(SMALL_TEXT_PX + 0.5)).text_color(p.on_surface_variant.hsla()).child(d))
            }))
            .child(div().text_size(px(SMALL_TEXT_PX)).line_height(px(16.0)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Invalid declarations are skipped with a warning; see Validate below."))));
        let left = div().row().flex_col().gap(px(12.0)).when(!self.stack_rows, |el| el.w(px(320.0)).flex_none()).child(tree).child(kinds);
        let example = self.code_block("manifest", "plugin.toml", authoring::EXAMPLE_MANIFEST, CodeLang::Toml, true, p, mono, cx);
        card(p)
            .child(section_header(LocalSection::Format, t("A plugin is a folder with a manifest and a WebAssembly component. This example contains every field and every setting type."), p))
            .child(div().row().when(self.stack_rows, |el| el.flex_col()).gap(px(CARD_GAP_PX)).child(left).child(div().flex_1().min_w(px(0.0)).child(example)))
    }

    // ── 3. Architecture ──────────────────────────────────────────────────

    fn local_architecture(&mut self, p: &MandaraPalette, mono: &SharedString) -> Div {
        let rtl = mandara_i18n::language().is_rtl();
        let stacked = self.stack_rows;
        let arrow = || div().flex_none().row().items_center().justify_center().px(px(2.0)).text_size(px(TITLE_TEXT_PX)).text_color(p.primary.with_alpha(0.7).hsla()).child(if stacked { "↓" } else if rtl { "←" } else { "→" });
        let node = |glyph: &'static str, title: &'static str, detail: String, color: Color| {
            div()
                .flex_1()
                .min_w(px(NODE_MIN_W_PX))
                .when(stacked, |el| el.flex_none().w(px(NODE_STACKED_W_PX)))
                .p(px(12.0))
                .rounded(px(INNER_RADIUS_PX))
                .row()
                .flex_col()
                .items_center()
                .gap(px(6.0))
                .bg(color.with_alpha(SOFT_ALPHA + 0.02).hsla())
                .shadow(mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow))
                .child(div().size(px(NODE_ICON_PX)).rounded(px(10.0)).row().items_center().justify_center().bg(color.with_alpha(TINT_ALPHA + 0.04).hsla()).child(icon(glyph, SMALL_ICON_PX + 1.0, color)))
                .child(div().text_size(px(BODY_TEXT_PX)).font_weight(FontWeight::SEMIBOLD).text_center().child(title))
                .child(div().font_family(mono.clone()).text_size(px(LABEL_TEXT_PX)).text_center().text_color(p.on_surface_variant.hsla()).child(detail))
        };
        let pipeline = div()
            .row()
            .when(stacked, |el| el.flex_col().items_center())
            .when(!stacked, |el| el.items_stretch())
            .gap(px(6.0))
            .child(node("note", t("Rust crate"), "src/lib.rs + wit-bindgen".into(), p.secondary))
            .child(arrow())
            .child(node("terminal", t("cargo build"), format!("--target {}", authoring::WASM_TARGET), p.secondary))
            .child(arrow())
            .child(node("package", t("WebAssembly component"), "plugin.wasm".into(), p.tertiary))
            .child(arrow())
            .child(node("shield", t("Mandara host"), "wasmtime".into(), p.primary))
            .child(arrow())
            .child(node("grid", t("Native views"), t("themed by Mandara").into(), p.success_emerald));
        let call_ms = wasm::CALL_BUDGET.as_millis();
        let init_ms = wasm::INIT_BUDGET.as_millis();
        let cache = mandara_config::paths::cache_dir().join("plugins");
        let fact = |glyph: &'static str, text: String, color: Color| {
            div().min_w(px(0.0)).row().items_start().gap(px(10.0)).child(div().pt(px(1.0)).child(icon(glyph, SMALL_ICON_PX, color))).child(div().flex_1().min_w(px(0.0)).text_size(px(BODY_TEXT_PX)).line_height(px(LINE_HEIGHT_PX)).font_features(mandara_ui::fonts::tabular_nums()).child(text))
        };
        let sandbox = inner(p)
            .row()
            .flex_col()
            .gap(px(10.0))
            .child(div().row().items_center().gap(px(8.0)).child(icon("shield", SMALL_ICON_PX, p.primary)).child(div().text_size(px(BODY_TEXT_PX + 0.5)).font_weight(FontWeight::SEMIBOLD).child(t("The sandbox"))))
            .child({
                let facts = [
                    fact("close", t("No filesystem access").into(), p.urgent_coral),
                    fact("close", t("No network access").into(), p.urgent_coral),
                    fact("close", t("No environment variables or arguments").into(), p.urgent_coral),
                    fact("close", t("No subprocesses").into(), p.urgent_coral),
                    fact("timer", tf("{call} ms per call, {init} ms for init", &[("call", &call_ms), ("init", &init_ms)]), p.warning_amber),
                    fact("alert", t("A trap or a timeout stops only that plugin, until it is reloaded").into(), p.warning_amber),
                    fact("check", tf("Only with the commands permission: up to {n} Mandara commands per call", &[("n", &wasm::MAX_QUEUED_COMMANDS)]), p.success_emerald),
                    fact("bolt", tf("Compiled once to native code, cached in {path}", &[("path", &display_dir(&cache))]), p.primary),
                ];
                super::grid::fit(self.page.column_w - 2.0 * (CARD_PAD_PX + INNER_PAD_PX), crate::tokens::LOCAL_CARD_GRID).balanced(facts.len()).render(facts)
            });
        let flow = |title: &'static str, glyph: &'static str, steps: Vec<String>| {
            let n = steps.len();
            inner(p)
                .flex_1()
                .min_w(px(260.0))
                .row()
                .flex_col()
                .gap(px(10.0))
                .child(div().row().items_center().gap(px(8.0)).child(icon(glyph, SMALL_ICON_PX, p.primary)).child(div().text_size(px(BODY_TEXT_PX + 0.5)).font_weight(FontWeight::SEMIBOLD).child(title)))
                .children(steps.into_iter().enumerate().map(move |(i, s)| {
                    div()
                        .row()
                        .items_start()
                        .gap(px(10.0))
                        .child(
                            div()
                                .flex_none()
                                .row()
                                .flex_col()
                                .items_center()
                                .child(div().size(px(20.0)).rounded_full().row().items_center().justify_center().bg(p.primary.with_alpha(TINT_ALPHA).hsla()).text_color(p.primary.hsla()).text_size(px(LABEL_TEXT_PX)).font_weight(FontWeight::SEMIBOLD).font_features(mandara_ui::fonts::tabular_nums()).child(format!("{}", i + 1)))
                                .when(i + 1 < n, |el| el.child(div().w(px(2.0)).h(px(12.0)).mt(px(2.0)).rounded_full().bg(p.primary.with_alpha(0.25).hsla()))),
                        )
                        .child(div().flex_1().min_w(px(0.0)).pt(px(1.0)).text_size(px(BODY_TEXT_PX)).line_height(px(LINE_HEIGHT_PX)).font_features(mandara_ui::fonts::tabular_nums()).child(s))
                }))
        };
        let debounce = mandara_ui::plugins::external::RELOAD_DEBOUNCE.as_millis();
        let hot = flow(
            t("Hot reload"),
            "refresh",
            vec![
                t("You copy a new plugin.wasm, plugin.toml or translation into the folder").into(),
                tf("The shell's folder watcher waits {ms} ms for writes to settle", &[("ms", &debounce)]),
                t("Changed plugins get a fresh instance; removed ones are unloaded").into(),
                t("The bar item, panel and sidebar card re-render: no restart").into(),
            ],
        );
        let settings = flow(
            t("Where settings come from"),
            "sliders",
            vec![
                t("You declare [[settings]] in plugin.toml").into(),
                t("Studio shows them in your plugin's settings modal").into(),
                t("Values are saved in [plugins.settings.<id>] in config.toml").into(),
                t("host::setting(key) returns the value as JSON, or the declared default").into(),
            ],
        );
        card(p)
            .child(section_header(LocalSection::Architecture, t("How a plugin goes from source code to the bar, and what it may do there."), p))
            .child(inner(p).child(pipeline))
            .child(sandbox)
            .child(div().row().flex_wrap().gap(px(CARD_GAP_PX)).child(hot).child(settings))
    }

    // ── 4. Contract ──────────────────────────────────────────────────────

    fn local_contract(&mut self, p: &MandaraPalette, mono: &SharedString, cx: &mut Context<Self>) -> Div {
        let c = authoring::contract();
        let stack = self.stack_rows;
        let table = |title: &'static str, subtitle: &'static str, rows: Vec<(String, String, String, Vec<Div>)>| {
            inner(p)
                .row()
                .flex_col()
                .gap(px(2.0))
                .child(div().row().flex_wrap().items_baseline().gap(px(8.0)).pb(px(8.0)).child(div().text_size(px(BODY_TEXT_PX + 0.5)).font_weight(FontWeight::SEMIBOLD).child(title)).child(div().text_size(px(SMALL_TEXT_PX)).text_color(p.on_surface_variant.hsla()).child(subtitle)))
                .children(rows.into_iter().enumerate().map(|(i, (name, sig, desc, badges))| {
                    div()
                        .py(px(8.0))
                        .row()
                        .when(stack, |el| el.flex_col().gap(px(4.0)))
                        .when(!stack, |el| el.gap(px(14.0)))
                        .when(i > 0, |el| el.border_t_1().border_color(p.on_surface.with_alpha(0.06).hsla()))
                        .child(
                            div()
                                .when(!stack, |el| el.w(px(300.0)).flex_none())
                                .row()
                                .flex_col()
                                .gap(px(2.0))
                                .child(div().font_family(mono.clone()).text_size(px(CODE_TEXT_PX + 0.5)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(name))
                                .child(div().font_family(mono.clone()).text_size(px(LABEL_TEXT_PX)).text_color(p.on_surface_variant.hsla()).child(sig)),
                        )
                        .child(div().flex_1().min_w(px(0.0)).row().flex_col().gap(px(6.0)).child(div().text_size(px(BODY_TEXT_PX)).line_height(px(LINE_HEIGHT_PX)).child(mandara_ui::bidi::paragraph(desc))).when(!badges.is_empty(), |el| el.child(div().row().flex_wrap().gap(px(6.0)).children(badges))))
                }))
        };
        let exports = c.exports.iter().map(|f| (f.name.clone(), f.signature.clone(), export_doc(&f.name).map(str::to_string).unwrap_or_else(|| f.doc.clone()), Vec::new())).collect();
        let imports = c
            .imports
            .iter()
            .map(|f| {
                let badges = if f.name == "command" {
                    vec![chip_with_icon("key", t("Needs the commands permission"), p.warning_amber), chip(tf("At most {n} per call", &[("n", &wasm::MAX_QUEUED_COMMANDS)]), p.secondary).font_features(mandara_ui::fonts::tabular_nums())]
                } else {
                    Vec::new()
                };
                (f.name.clone(), f.signature.clone(), import_doc(&f.name).map(str::to_string).unwrap_or_else(|| f.doc.clone()), badges)
            })
            .collect();
        let kinds = c.kinds.iter().map(|k| {
            div()
                .row()
                .items_start()
                .gap(px(10.0))
                .child(div().w(px(70.0)).flex_none().font_family(mono.clone()).text_size(px(CODE_TEXT_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(k.name.clone()))
                .child(div().flex_1().min_w(px(0.0)).text_size(px(SMALL_TEXT_PX + 0.5)).line_height(px(16.0)).text_color(p.on_surface_variant.hsla()).child(kind_doc(&k.name).map(str::to_string).unwrap_or_else(|| k.doc.clone())))
        });
        let tones = c.tones.iter().map(|k| {
            let color = tone(&k.name).map(|t| mandara_ui::plugins::external::tone_color(t, p)).unwrap_or(p.on_surface);
            div().h(px(CHIP_H_PX + 4.0)).px(px(10.0)).rounded_full().row().items_center().gap(px(6.0)).bg(color.with_alpha(TINT_ALPHA).hsla()).child(div().size(px(8.0)).rounded_full().bg(color.hsla())).child(div().font_family(mono.clone()).text_size(px(CODE_TEXT_PX)).text_color(color.hsla()).child(k.name.clone()))
        });
        let fields = c.node_fields.iter().map(|f| {
            div().h(px(CHIP_H_PX)).px(px(CHIP_PAD_X_PX)).rounded(px(8.0)).row().items_center().gap(px(4.0)).bg(p.on_surface.with_alpha(0.05).hsla()).font_family(mono.clone()).text_size(px(LABEL_TEXT_PX + 0.5)).child(div().child(f.name.clone())).child(div().text_color(p.on_surface_variant.hsla()).child(f.signature.clone()))
        });
        let views = inner(p)
            .row()
            .flex_col()
            .gap(px(12.0))
            .child(div().row().flex_wrap().items_baseline().gap(px(8.0)).child(div().text_size(px(BODY_TEXT_PX + 0.5)).font_weight(FontWeight::SEMIBOLD).child(t("Views"))).child(div().text_size(px(SMALL_TEXT_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("A view is a list of nodes; nodes[root] is the top one and children are indices. Mandara draws them natively.")))))
            .child({
                let kinds: Vec<_> = kinds.collect();
                super::grid::fit(self.page.column_w - 2.0 * (CARD_PAD_PX + INNER_PAD_PX), crate::tokens::LOCAL_CARD_GRID).balanced(kinds.len()).render(kinds)
            })
            .child(div().text_size(px(SMALL_TEXT_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface_variant.hsla()).child(t("Tones map to the live theme")))
            .child(div().row().flex_wrap().gap(px(6.0)).children(tones))
            .child(div().text_size(px(SMALL_TEXT_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface_variant.hsla()).child(t("Node fields")))
            .child(div().row().flex_wrap().gap(px(6.0)).children(fields));
        let open = self.local.wit_open;
        let lines = authoring::WIT.lines().count();
        let toggle = div()
            .id("local-wit-toggle")
            .h(px(BUTTON_H_PX + 4.0))
            .px(px(BUTTON_PAD_X_PX))
            .rounded(px(12.0))
            .row()
            .items_center()
            .gap(px(10.0))
            .cursor_pointer()
            .bg(p.surface_container_high.hsla())
            .hover(|s| s.opacity(HOVER_OPACITY))
            .child(icon(if open { "arrow-down" } else { mandara_ui::bidi::dir_icon("chevron-right") }, SMALL_ICON_PX, p.on_surface_variant))
            .child(div().text_size(px(BODY_TEXT_PX)).font_weight(FontWeight::SEMIBOLD).child(t("Full WIT")))
            .child(div().font_family(mono.clone()).text_size(px(LABEL_TEXT_PX + 0.5)).text_color(p.on_surface_variant.hsla()).child(format!("{} · {}", c.package, authoring::WIT_REPO_PATH)))
            .child(div().flex_1())
            .child(div().text_size(px(SMALL_TEXT_PX)).text_color(p.on_surface_variant.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(tf("{n} lines", &[("n", &lines)])))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.local.wit_open = !this.local.wit_open;
                cx.notify();
            }));
        let full = open.then(|| self.code_block("wit", "plugin.wit", authoring::WIT, CodeLang::Wit, true, p, mono, cx));
        card(p)
            .child(section_header(LocalSection::Contract, t("The interface between your plugin and Mandara, read from the plugin.wit this build was compiled with."), p))
            .child(table(t("Exports"), t("Your plugin provides these"), exports))
            .child(table(t("Host imports"), t("Mandara provides these"), imports))
            .child(views)
            .child(toggle)
            .children(full)
    }

    // ── 5. Quick start ───────────────────────────────────────────────────

    fn local_quick_start(&mut self, p: &MandaraPalette, mono: &SharedString, cx: &mut Context<Self>) -> Div {
        let dev = wasm::plugin_dirs().into_iter().find(|(_, s)| *s == Source::Development).map(|(d, _)| d).unwrap_or_default();
        let steps: Vec<AnyElement> = authoring::quick_start(&dev)
            .into_iter()
            .enumerate()
            .map(|(i, step)| {
                let block = self.code_block(&format!("step-{i}"), "", &step.commands, CodeLang::Shell, false, p, mono, cx);
                div()
                    .row()
                    .gap(px(12.0))
                    .child(div().flex_none().size(px(26.0)).rounded_full().row().items_center().justify_center().bg(p.primary.hsla()).text_color(p.on_primary.hsla()).text_size(px(BODY_TEXT_PX)).font_weight(FontWeight::SEMIBOLD).font_features(mandara_ui::fonts::tabular_nums()).child(format!("{}", i + 1)))
                    .child(div().flex_1().min_w(px(0.0)).row().flex_col().gap(px(8.0)).child(div().pt(px(3.0)).text_size(px(BODY_TEXT_PX + 0.5)).font_weight(FontWeight::SEMIBOLD).child(mandara_ui::bidi::paragraph(tr(step.title)))).child(block))
                    .into_any_element()
            })
            .collect();
        let local_guide = authoring::GUIDE_LOCAL_PATHS.iter().map(PathBuf::from).find(|p| p.is_file());
        let guide_uri = local_guide.as_ref().map(|p| format!("file://{}", p.display())).unwrap_or_else(|| authoring::GUIDE_URL.to_string());
        let example_uri = format!("{}/tree/main/{}", authoring::REPOSITORY, authoring::EXAMPLE_DIR);
        let open_dev = dev.clone();
        let links = div()
            .row()
            .flex_wrap()
            .gap(px(8.0))
            .child(button("local-guide".into(), "note", t("Open PLUGINS.md"), true, p).on_click(move |_, _, _| {
                let _ = mandara_services::session::open_uri(&guide_uri);
            }))
            .child(button("local-example".into(), "external", t("Example plugin on GitHub"), false, p).on_click(move |_, _, _| {
                let _ = mandara_services::session::open_uri(&example_uri);
            }))
            .child(button("local-open-dev".into(), "folder", t("Open the development folder"), false, p).on_click(move |_, _, _| {
                let _ = std::fs::create_dir_all(&open_dev);
                let _ = mandara_services::session::open_uri(&format!("file://{}", open_dev.display()));
            }));
        let topic = mandara_services::marketplace::TOPIC;
        let asset = format!("<id>{}", mandara_services::marketplace::ASSET_SUFFIX);
        let limit_mb = mandara_services::marketplace::MAX_BYTES >> 20;
        let publish_step = |n: usize, title: &'static str, code: Option<String>, note: String| {
            div()
                .flex_1()
                .min_w(px(200.0))
                .p(px(12.0))
                .rounded(px(12.0))
                .bg(p.surface_base.with_alpha(0.5).hsla())
                .row()
                .flex_col()
                .gap(px(6.0))
                .child(div().row().items_center().gap(px(8.0)).child(div().text_color(p.primary.hsla()).font_weight(FontWeight::SEMIBOLD).font_features(mandara_ui::fonts::tabular_nums()).child(format!("{n}"))).child(div().text_size(px(BODY_TEXT_PX)).font_weight(FontWeight::SEMIBOLD).child(title)))
                .when_some(code, |el, c| el.child(div().row().child(div().px(px(8.0)).py(px(3.0)).rounded(px(8.0)).bg(p.primary.with_alpha(TINT_ALPHA).hsla()).text_color(p.primary.hsla()).font_family(mono.clone()).text_size(px(CODE_TEXT_PX)).child(c))))
                .child(div().text_size(px(SMALL_TEXT_PX + 0.5)).line_height(px(16.0)).text_color(p.on_surface_variant.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(note))
        };
        let publish = inner(p)
            .row()
            .flex_col()
            .gap(px(10.0))
            .child(div().row().items_center().gap(px(8.0)).child(icon("download", SMALL_ICON_PX, p.primary)).child(div().text_size(px(BODY_TEXT_PX + 0.5)).font_weight(FontWeight::SEMIBOLD).child(t("Publish to the marketplace"))).child(div().text_size(px(SMALL_TEXT_PX)).text_color(p.on_surface_variant.hsla()).child(t("Free, on GitHub: no account, no review queue"))))
            .child(
                div()
                    .row()
                    .flex_wrap()
                    .gap(px(10.0))
                    .child(publish_step(1, t("Tag the repository"), Some(topic.to_string()), t("A public repository with this topic (About → Topics).").into()))
                    .child(publish_step(2, t("Attach a release asset"), Some(asset), t("A .tar.gz of your plugin folder, named after its id.").into()))
                    .child(publish_step(3, t("Users install it"), None, tf("It appears in Marketplace, ranked by stars. Archives up to {mb} MB; paths, links and the id are checked on install.", &[("mb", &limit_mb)]))),
            );
        card(p)
            .child(section_header(LocalSection::QuickStart, t("From nothing to a plugin in your bar. Run these in a terminal; the last step copies it straight into your development folder."), p))
            .children(steps)
            .child(links)
            .child(publish)
    }

    // ── 6. Validate ──────────────────────────────────────────────────────

    fn local_validate(&mut self, cfg: &ShellConfig, p: &MandaraPalette, mono: &SharedString, cx: &mut Context<Self>) -> Div {
        let registry = cx.global::<PluginRegistry>();
        let builtin = |id: &str| registry.get(id).is_some_and(|f| f.manifest.origin == PluginOrigin::Builtin);
        let enabled: HashSet<String> = registry.manifests().filter(|m| registry.is_enabled(&m.id, &cfg.plugins)).map(|m| m.id.clone()).collect();
        let shell = match &self.local.runtime {
            None => None,
            Some(Ok(map)) => Some(map.clone()),
            Some(Err(_)) => None,
        };
        let shell_down = matches!(self.local.runtime, Some(Err(_)));
        struct Row {
            dir: PathBuf,
            title: String,
            subtitle: String,
            status: (&'static str, &'static str, Color),
            detail: Option<String>,
            detail_is_error: bool,
            problems: Vec<String>,
            plugin: Option<Discovered>,
            icon_path: Option<PathBuf>,
        }
        let mut rows: Vec<Row> = Vec::new();
        for report in &self.local.reports {
            for (dir, inspection) in &report.entries {
                let folder = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let mut row = Row { dir: dir.clone(), title: folder.clone(), subtitle: display_dir(dir), status: ("check", t("Ready"), p.primary), detail: None, detail_is_error: false, problems: Vec::new(), plugin: None, icon_path: None };
                match inspection {
                    Inspection::NoManifest => {
                        row.status = ("note", t("Not a plugin"), p.on_surface_variant);
                        row.detail = Some(t("No plugin.toml in this folder").into());
                    }
                    Inspection::Invalid { error } => {
                        row.status = ("alert", t("Invalid manifest"), p.urgent_coral);
                        row.detail = Some(error.clone());
                        row.detail_is_error = true;
                    }
                    Inspection::Shadowed { id, by } => {
                        row.title = id.clone();
                        row.status = ("minus", t("Shadowed"), p.on_surface_variant);
                        row.detail = Some(tf("Skipped: {path} has the same id and comes first", &[("path", &display_dir(by))]));
                    }
                    Inspection::Found { plugin, entry_missing, problems } => {
                        row.title = plugin.name.clone();
                        row.icon_path = Some(plugin.dir.join("icon.png")).filter(|f| f.is_file());
                        row.problems = problems.clone();
                        row.subtitle = format!("{} · v{} · {}", plugin.id, plugin.version, display_dir(dir));
                        let on = enabled.contains(&plugin.id);
                        if builtin(&plugin.id) {
                            row.status = ("alert", t("Skipped"), p.urgent_coral);
                            row.detail = Some(tf("“{id}” is a built-in plugin id, and built-ins can't be overridden: rename it", &[("id", &plugin.id)]));
                        } else if *entry_missing {
                            row.status = ("alert", t("Component missing"), p.urgent_coral);
                            row.detail = Some(tf("{file} not found: build the plugin and copy the .wasm here", &[("file", &plugin.entry.strip_prefix(&plugin.dir).unwrap_or(&plugin.entry).display())]));
                            row.plugin = None;
                        } else {
                            row.plugin = Some(plugin.clone());
                            match shell.as_ref().and_then(|m| m.get(&plugin.id)) {
                                Some(rt) if rt.state == "running" => row.status = ("check", t("Running"), p.success_emerald),
                                Some(rt) if rt.state == "failed" => {
                                    row.status = ("alert", t("Failed to load"), p.urgent_coral);
                                    row.detail = rt.error.clone();
                                    row.detail_is_error = true;
                                }
                                Some(rt) if rt.state == "disabled" => {
                                    row.status = ("alert", t("Stopped after an error"), p.warning_amber);
                                    row.detail = rt.error.clone();
                                    row.detail_is_error = true;
                                }
                                _ if !on => {
                                    row.status = ("circle", t("Off"), p.on_surface_variant);
                                    row.detail = Some(t("Valid. Turn it on in Installed to load it.").into());
                                }
                                _ if shell_down => row.detail = Some(t("Valid. The shell isn't running, so its load state is unknown.").into()),
                                _ => row.detail = Some(t("Enabled. It loads when its bar item, panel or sidebar card is first shown.").into()),
                            }
                        }
                    }
                }
                rows.push(row);
            }
        }
        let attention = rows.iter().filter(|r| r.status.2 == p.urgent_coral || r.status.2 == p.warning_amber || !r.problems.is_empty()).count();
        let total = rows.len();
        let summary = div()
            .row()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .child(chip(if total == 1 { t("1 plugin folder checked").to_string() } else { tf("{n} plugin folders checked", &[("n", &total)]) }, p.primary).font_features(mandara_ui::fonts::tabular_nums()))
            .child(if attention > 0 { chip_with_icon("alert", tf("{n} need attention", &[("n", &attention)]), p.urgent_coral) } else { chip_with_icon("check", t("All good"), p.success_emerald) })
            .child(match &self.local.runtime {
                None => chip(t("Asking the shell…"), p.on_surface_variant),
                Some(Ok(_)) => chip_with_icon("server", t("Live state from the running shell"), p.secondary),
                Some(Err(_)) => chip_with_icon("alert", t("Shell not running: file checks only"), p.warning_amber),
            })
            .child(div().flex_1())
            .child(button("local-rescan".into(), "refresh", t("Check again"), false, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.local.tests.clear();
                this.local_refresh(true, cx);
                cx.notify();
            })));
        let body: Vec<AnyElement> = if rows.is_empty() {
            vec![inner(p)
                .row()
                .flex_col()
                .items_center()
                .gap(px(8.0))
                .py(px(22.0))
                .child(icon("package", 26.0, p.on_surface_variant.with_alpha(0.6)))
                .child(div().font_weight(FontWeight::MEDIUM).child(t("No plugins in your local folders yet")))
                .child(div().text_size(px(BODY_TEXT_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Follow Quick start above: your plugin shows up here, and in Installed, as soon as you copy it in."))))
                .into_any_element()]
        } else {
            rows.into_iter()
                .enumerate()
                .map(|(i, r)| {
                    let (glyph, label, color) = r.status;
                    let test = r.plugin.as_ref().and_then(|_| self.local.tests.get(&r.dir).cloned());
                    let open_dir = r.dir.clone();
                    let mut actions = div().row().flex_wrap().gap(px(8.0)).child(button(format!("local-row-open-{i}"), "folder", t("Open folder"), false, p).on_click(move |_, _, _| {
                        let _ = mandara_services::session::open_uri(&format!("file://{}", open_dir.display()));
                    }));
                    if let Some(plugin) = r.plugin.clone() {
                        let running = matches!(test, Some(TestState::Running));
                        let cfg = cfg.clone();
                        let palette = p.clone();
                        actions = actions.child(
                            button(format!("local-row-test-{i}"), if running { "timer" } else { "zap" }, if running { t("Testing…") } else { t("Test load") }, false, p)
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.local_test(plugin.clone(), &cfg, &palette, cx))),
                        );
                    }
                    let tile_el = match &r.icon_path {
                        Some(path) => gpui::img(gpui::ImageSource::from(path.clone())).size(px(TILE_PX)).rounded(px(TILE_RADIUS_PX)).into_any_element(),
                        None => tile("box", color, p).into_any_element(),
                    };
                    let test_el = test.and_then(|t| match t {
                        TestState::Running => None,
                        TestState::Done(Ok(rep)) => {
                            let mark = |ok: bool| if ok { "✓" } else { "—" };
                            Some(
                                div()
                                    .px(px(12.0))
                                    .py(px(8.0))
                                    .rounded(px(10.0))
                                    .bg(p.success_emerald.with_alpha(TINT_ALPHA - 0.04).hsla())
                                    .text_color(p.success_emerald.hsla())
                                    .text_size(px(BODY_TEXT_PX))
                                    .font_features(mandara_ui::fonts::tabular_nums())
                                    .child(mandara_ui::bidi::paragraph(tf("Loaded and initialized in {ms} ms · bar {bar} · panel {panel} · sidebar {sidebar}", &[("ms", &rep.elapsed.as_millis()), ("bar", &mark(rep.bar)), ("panel", &mark(rep.panel)), ("sidebar", &mark(rep.sidebar))]))),
                            )
                        }
                        TestState::Done(Err(e)) => Some(error_box(&e, p, mono)),
                    });
                    inner(p)
                        .row()
                        .gap(px(12.0))
                        .child(tile_el)
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .row()
                                .flex_col()
                                .gap(px(6.0))
                                .child(div().row().flex_wrap().items_center().gap(px(8.0)).child(div().text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).child(r.title)).child(chip_with_icon(glyph, label, color)))
                                .child(div().font_family(mono.clone()).text_size(px(LABEL_TEXT_PX + 0.5)).text_color(p.on_surface_variant.hsla()).child(r.subtitle))
                                .when_some(r.detail, |el, d| el.child(if r.detail_is_error { error_box(&d, p, mono) } else { div().text_size(px(BODY_TEXT_PX)).line_height(px(LINE_HEIGHT_PX)).text_color(p.on_surface_variant.hsla()).child(d) }))
                                .when(!r.problems.is_empty(), |el| {
                                    el.child(div().row().flex_col().gap(px(4.0)).children(r.problems.into_iter().map(|pr| {
                                        div().row().items_start().gap(px(8.0)).child(div().pt(px(2.0)).child(icon("alert", 12.0, p.warning_amber))).child(div().flex_1().min_w(px(0.0)).text_size(px(SMALL_TEXT_PX + 0.5)).text_color(p.warning_amber.hsla()).child(pr))
                                    })))
                                })
                                .children(test_el)
                                .child(actions),
                        )
                        .into_any_element()
                })
                .collect()
        };
        card(p)
            .child(section_header(LocalSection::Validate, t("Every folder Mandara found, with what it made of it: start here when a plugin doesn't show up."), p))
            .child(summary)
            .children(body)
    }

    /// A monospace code block with a copy button.
    #[allow(clippy::too_many_arguments)]
    fn code_block(&mut self, key: &str, label: &str, code: &str, lang: CodeLang, numbered: bool, p: &MandaraPalette, mono: &SharedString, cx: &mut Context<Self>) -> AnyElement {
        let copy_key = format!("code-{key}");
        let copied = self.local.copied.as_deref() == Some(copy_key.as_str());
        let text = code.trim_end().to_string();
        let lines: Vec<AnyElement> = text
            .lines()
            .enumerate()
            .map(|(i, line)| {
                let spans = highlight(line, lang, p);
                // Code reads left to right in every language.
                div()
                    .h(px(CODE_LINE_PX))
                    .ltr_row()
                    .items_center()
                    .whitespace_nowrap()
                    .when(numbered, |el| el.child(div().w(px(GUTTER_W_PX)).flex_none().text_color(p.on_surface_variant.with_alpha(0.5).hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(format!("{}", i + 1))))
                    .children(spans.into_iter().map(|(s, c)| div().flex_none().text_color(c.hsla()).child(s)))
                    .into_any_element()
            })
            .collect();
        let copy_text = text.clone();
        let copy = div()
            .id(SharedString::from(format!("{copy_key}-btn")))
            .h(px(26.0))
            .px(px(10.0))
            .rounded_full()
            .row()
            .items_center()
            .gap(px(6.0))
            .cursor_pointer()
            .bg(if copied { p.success_emerald.with_alpha(TINT_ALPHA).hsla() } else { p.surface_container_high.hsla() })
            .text_color(if copied { p.success_emerald.hsla() } else { p.on_surface.hsla() })
            .text_size(px(SMALL_TEXT_PX))
            .font_weight(FontWeight::MEDIUM)
            .hover(|s| s.opacity(HOVER_OPACITY))
            .child(icon(if copied { "check" } else { "copy" }, 12.0, if copied { p.success_emerald } else { p.on_surface_variant }))
            .child(if copied { t("Copied") } else { t("Copy") })
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.local_copy(copy_key.clone(), copy_text.clone(), cx)));
        div()
            .rounded(px(CODE_RADIUS_PX))
            .bg(p.surface_dim.hsla())
            .shadow(mandara_core::glass_rim(p.specular_rim_light.with_alpha(0.6), p.specular_rim_shadow.with_alpha(0.4)))
            .row()
            .flex_col()
            .child(
                div()
                    .px(px(CODE_PAD_PX))
                    .pt(px(10.0))
                    .row()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().text_size(px(LABEL_TEXT_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface_variant.hsla()).child(lang.label()))
                    .when(!label.is_empty(), |el| el.child(div().font_family(mono.clone()).text_size(px(LABEL_TEXT_PX)).text_color(p.on_surface_variant.with_alpha(0.8).hsla()).child(label.to_string())))
                    .child(div().flex_1())
                    .child(copy),
            )
            .child(
                div()
                    .id(SharedString::from(format!("code-scroll-{key}")))
                    .overflow_x_scroll()
                    .px(px(CODE_PAD_PX))
                    .pt(px(6.0))
                    .pb(px(12.0))
                    .font_family(mono.clone())
                    .text_size(px(CODE_TEXT_PX))
                    .text_left()
                    .child(div().flex().flex_col().items_start().children(lines)),
            )
            .into_any_element()
    }
}

// ── Building blocks ──────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum CodeLang {
    Toml,
    Wit,
    Shell,
}

impl CodeLang {
    fn label(self) -> &'static str {
        match self {
            CodeLang::Toml => "TOML",
            CodeLang::Wit => "WIT",
            CodeLang::Shell => "SHELL",
        }
    }

    fn comment(self) -> &'static str {
        match self {
            CodeLang::Toml | CodeLang::Shell => "#",
            CodeLang::Wit => "//",
        }
    }
}

/// Splits `line` at its comment marker (outside double quotes).
fn split_comment<'a>(line: &'a str, marker: &str) -> (&'a str, &'a str) {
    let mut quoted = false;
    for (i, c) in line.char_indices() {
        if c == '"' {
            quoted = !quoted;
        } else if !quoted && line[i..].starts_with(marker) {
            return (&line[..i], &line[i..]);
        }
    }
    (line, "")
}

/// Light syntax colors: comments muted, TOML keys / WIT keywords / shell
/// commands in the primary color, TOML tables in the tertiary one.
fn highlight(line: &str, lang: CodeLang, p: &MandaraPalette) -> Vec<(String, Color)> {
    let (code, comment) = split_comment(line, lang.comment());
    let mut out: Vec<(String, Color)> = Vec::new();
    let indent_len = code.len() - code.trim_start().len();
    let (indent, rest) = code.split_at(indent_len);
    if !indent.is_empty() {
        out.push((indent.to_string(), p.on_surface));
    }
    match lang {
        CodeLang::Toml if rest.starts_with('[') => out.push((rest.to_string(), p.tertiary)),
        CodeLang::Toml => match rest.split_once('=') {
            Some((key, value)) if !key.trim().is_empty() && !key.contains('{') => {
                out.push((key.to_string(), p.primary));
                out.push(("= ".to_string(), p.on_surface_variant));
                out.push((value.trim_start().to_string(), p.on_surface));
            }
            _ => out.push((rest.to_string(), p.on_surface)),
        },
        CodeLang::Wit | CodeLang::Shell => {
            let (first, tail) = rest.split_at(rest.find(' ').unwrap_or(rest.len()));
            const KEYWORDS: &[&str] = &["package", "interface", "record", "enum", "world", "use", "import", "export"];
            let keyword = match lang {
                CodeLang::Shell => !first.is_empty(),
                _ => KEYWORDS.contains(&first),
            };
            if keyword {
                out.push((first.to_string(), p.primary));
                out.push((tail.to_string(), p.on_surface));
            } else {
                out.push((rest.to_string(), p.on_surface));
            }
        }
    }
    if !comment.is_empty() {
        out.push((comment.to_string(), p.on_surface_variant.with_alpha(0.8)));
    }
    out.retain(|(s, _)| !s.is_empty());
    out
}

/// What each origin means.
fn source_info(source: Source, p: &MandaraPalette) -> (&'static str, &'static str, Color) {
    match source {
        Source::Development => (t("Development"), t("Your own plugins while you build them: enabled automatically and reloaded on every change."), p.tertiary),
        Source::Installed => (t("Installed"), t("Marketplace installs and plugins you copy in by hand: off until you turn them on."), p.primary),
        Source::System => (t("System"), t("Installed by distribution packages: off until you turn them on, and read-only."), p.secondary),
    }
}

/// An absolute folder path with a trailing slash.
fn display_dir(dir: &Path) -> String {
    let s = dir.display().to_string();
    if s.ends_with('/') { s } else { format!("{s}/") }
}

fn export_doc(name: &str) -> Option<&'static str> {
    Some(match name {
        "init" => t("Runs once after loading. Return an error to refuse to load."),
        "bar" => t("The bar item, or none for no bar item."),
        "panel" => t("The panel that drops from the bar item, or none."),
        "sidebar" => t("A card for the sidebar, or none."),
        "on-event" => t("A node with an action was clicked; receives that action."),
        "refresh-ms" => t("How often to re-render, in milliseconds (0 = only after events)."),
        _ => return None,
    })
}

fn import_doc(name: &str) -> Option<&'static str> {
    Some(match name {
        "log" => t("Writes to Mandara's log, tagged with your plugin's id."),
        "now-unix-ms" => t("The current time, in milliseconds since the Unix epoch."),
        "setting" => t("One of your settings as JSON: the user's value, or the declared default."),
        "command" => t("Queues a Mandara command, the same ones as mandara msg."),
        "locale" => t("The interface language as an ISO 639-1 code; right-to-left for ar."),
        "translate" => t("Looks text up in your locales/ catalog; labels in views are translated automatically."),
        _ => return None,
    })
}

fn kind_doc(name: &str) -> Option<&'static str> {
    Some(match name {
        "row" => t("Children side by side, gap apart"),
        "column" => t("Children stacked, gap apart"),
        "card" => t("A rounded surface around its children"),
        "text" => t("A label: size, bold and tone style it"),
        "icon" => t("A bundled Mandara icon by name, such as sun or timer"),
        "button" => t("A pill with a label that sends its action"),
        "meter" => t("A progress bar, value from 0 to 1"),
        "spacer" => t("Flexible empty space"),
        _ => return None,
    })
}

fn tone(name: &str) -> Option<wasm::Tone> {
    Some(match name {
        "normal" => wasm::Tone::Normal,
        "muted" => wasm::Tone::Muted,
        "accent" => wasm::Tone::Accent,
        "error" => wasm::Tone::Error,
        "success" => wasm::Tone::Success,
        "warning" => wasm::Tone::Warning,
        _ => return None,
    })
}

/// A glass section card: tonal surface with the specular rim.
fn card(p: &MandaraPalette) -> Div {
    div()
        .clip_rounded()
        .rounded(px(CARD_RADIUS_PX))
        .p(px(CARD_PAD_PX))
        .row()
        .flex_col()
        .gap(px(CARD_GAP_PX))
        .bg(p.surface_container.hsla())
        .shadow(mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow))
}

/// A recessed block inside a card.
fn inner(p: &MandaraPalette) -> Div {
    div().rounded(px(INNER_RADIUS_PX)).p(px(INNER_PAD_PX)).bg(p.surface_container_low.hsla())
}

fn tile(glyph: &'static str, color: Color, _p: &MandaraPalette) -> Div {
    div().flex_none().size(px(TILE_PX)).rounded(px(TILE_RADIUS_PX)).row().items_center().justify_center().bg(color.with_alpha(TINT_ALPHA).hsla()).child(icon(glyph, TILE_ICON_PX, color))
}

fn section_header(section: LocalSection, subtitle: &'static str, p: &MandaraPalette) -> Div {
    div()
        .row()
        .items_start()
        .gap(px(12.0))
        .child(tile(section.icon(), p.primary, p))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .row()
                .flex_col()
                .gap(px(2.0))
                .child(div().text_size(px(TITLE_TEXT_PX)).font_weight(FontWeight::SEMIBOLD).child(section.title()))
                .child(div().text_size(px(BODY_TEXT_PX)).line_height(px(LINE_HEIGHT_PX)).text_color(p.on_surface_variant.hsla()).child(subtitle)),
        )
}

fn chip(text: impl Into<SharedString>, color: Color) -> Div {
    div()
        .h(px(CHIP_H_PX))
        .px(px(CHIP_PAD_X_PX))
        .rounded_full()
        .row()
        .items_center()
        .bg(color.with_alpha(TINT_ALPHA).hsla())
        .text_color(color.hsla())
        .text_size(px(LABEL_TEXT_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

fn chip_with_icon(glyph: &'static str, text: impl Into<SharedString>, color: Color) -> Div {
    chip(text, color).gap(px(5.0)).pad_start(px(CHIP_PAD_X_PX - 2.0)).child(icon(glyph, 11.0, color))
}

fn button(id: String, glyph: &'static str, label: &'static str, primary: bool, p: &MandaraPalette) -> Stateful<Div> {
    let fg = if primary { p.on_primary } else { p.on_surface };
    div()
        .id(SharedString::from(id))
        .h(px(BUTTON_H_PX))
        .px(px(BUTTON_PAD_X_PX))
        .rounded_full()
        .row()
        .items_center()
        .gap(px(7.0))
        .cursor_pointer()
        .bg(if primary { p.primary.hsla() } else { p.surface_container_high.hsla() })
        .text_color(fg.hsla())
        .text_size(px(BODY_TEXT_PX))
        .font_weight(FontWeight::MEDIUM)
        .hover(|s| s.opacity(HOVER_OPACITY))
        .child(icon(glyph, BUTTON_ICON_PX, if primary { p.on_primary } else { p.on_surface_variant }))
        .child(label)
}

fn error_box(text: &str, p: &MandaraPalette, mono: &SharedString) -> Div {
    div()
        .px(px(12.0))
        .py(px(8.0))
        .rounded(px(10.0))
        .bg(p.urgent_coral.with_alpha(TINT_ALPHA - 0.04).hsla())
        .text_color(p.urgent_coral.hsla())
        .font_family(mono.clone())
        .text_size(px(SMALL_TEXT_PX))
        .line_height(px(16.0))
        .child(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_and_sections_round_trip() {
        for tab in PluginsTab::ALL {
            assert_eq!(PluginsTab::parse(tab.slug()), Some(tab));
        }
        assert_eq!(PluginsTab::parse("Market"), Some(PluginsTab::Marketplace));
        assert_eq!(PluginsTab::parse("community"), Some(PluginsTab::Marketplace));
        assert_eq!(PluginsTab::parse("library"), Some(PluginsTab::Installed));
        assert_eq!(PluginsTab::default(), PluginsTab::Discover);
        assert_eq!(PluginsTab::parse("weather"), None);
        for s in LocalSection::ALL {
            assert_eq!(LocalSection::parse(s.slug()), Some(s));
        }
        assert_eq!(LocalSection::parse("wit"), Some(LocalSection::Contract));
        assert_eq!(LocalSection::parse("quick_start"), Some(LocalSection::QuickStart));
    }

    #[test]
    fn comments_split_outside_strings() {
        assert_eq!(split_comment("a = \"#x\" # note", "#"), ("a = \"#x\" ", "# note"));
        assert_eq!(split_comment("log: func(); // hi", "//"), ("log: func(); ", "// hi"));
        assert_eq!(split_comment("plain", "#"), ("plain", ""));
        assert_eq!(display_dir(Path::new("/a/b")), "/a/b/");
    }
}
