//! Stacks (folders on the right side of the dock: Downloads and any you
//! add) and the Trash.
//!
//! A stack's files are read lazily — the first time its icon or popover
//! needs them — and cached until `notify` reports a change in the folder,
//! so an idle dock never touches the disk. Clicking a stack opens its
//! popover (its own overlay surface) as
//! * a **fan** (macOS): the newest files on a gentle arc ([`fan_point`]),
//!   each with its name in a pill beside it, popping up out of the stack
//!   icon as one unit on a single spring ([`fan_progress`]), and "Open
//!   Downloads" (the folder, in your file manager, with how many more files
//!   it holds) at the top;
//! * a **grid**: a glass panel that grows out of the icon, the folder's
//!   name and "Open in Files" on top, icons with two-line names, arrow
//!   keys, Enter, type-to-filter, a right-click menu;
//! * or a **list**. "Automatic" is the fan: however big the folder, it
//!   shows the newest `fan_items` files and leaves the rest to the file
//!   manager.
//!
//! Image previews are decoded off the UI thread at the size they're shown
//! and belong to the popover (freed with it); the dock tile's preview of
//! the newest file is a single small thumbnail.
//!
//! The Trash follows the freedesktop.org Trash spec: files dropped on it
//! move to `$XDG_DATA_HOME/Trash` (via `app_uninstall::trash`, which never
//! deletes); emptying asks for confirmation first.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::layer_shell::KeyboardInteractivity;
use gpui::{AnyElement, App, AppContext as _, Context, FocusHandle, FontWeight, Global, ImageSource, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, ObjectFit, ParentElement, Render, RenderImage, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, StyledImage as _, Window, WindowHandle, div, img, prelude::FluentBuilder as _, px};
use mandara_config::workflow::{DockPosition, DockStack, StackSort, StackView};
use mandara_core::{SpringConfig, ToGpui};
use mandara_i18n::{t, tf};
use mandara_theme::{Color, MandaraPalette};

use super::sheet::{self, MenuRow, PREVIEW_MAX_BYTES, Sheet, Thumbs};
use crate::icons::icon;
use crate::kit::text;
use crate::plugins::panel_fx::{WASH_SOFT, readable, rim, wash};
use crate::state::config;

/// Nanoseconds per second (file times are compared to the nanosecond).
const NANOS: i64 = 1_000_000_000;
/// Partial downloads never show in a stack.
const PARTIAL_SUFFIXES: &[&str] = &[".part", ".crdownload", ".download", ".partial"];
/// Grid and list panel geometry (logical px).
const POP_PAD: f32 = 16.0;
const POP_RADIUS: f32 = 22.0;
const POP_GAP: f32 = 12.0;
const POP_HEADER_H: f32 = 44.0;
const BUTTON_H: f32 = 28.0;
const BUTTON_PAD: f32 = 12.0;
const FILTER_H: f32 = 22.0;
const FILTER_PAD: f32 = 9.0;
const FILTER_GAP: f32 = 6.0;
const GRID_TILE_W: f32 = 104.0;
const GRID_TILE_H: f32 = 112.0;
const GRID_TILE_PAD: f32 = 10.0;
const GRID_ICON: f32 = 56.0;
const GRID_NAME_GAP: f32 = 7.0;
const GRID_NAME_PAD: f32 = 6.0;
const GRID_NAME_LINE_H: f32 = 14.0;
const GRID_NAME_LINES: usize = 2;
const GRID_COLUMNS: usize = 5;
/// Rows a grid shows before it scrolls.
const GRID_MAX_ROWS: usize = 4;
const LIST_ROW_H: f32 = 38.0;
const LIST_W: f32 = 360.0;
const LIST_ICON: f32 = 24.0;
const LIST_MAX_ROWS: usize = 12;
const ROW_RADIUS: f32 = 12.0;
const ROW_PAD: f32 = 10.0;
const ROW_GAP: f32 = 10.0;
const HOVER_ALPHA: f32 = 0.07;
const SELECTED_ALPHA: f32 = 0.16;
const SCROLLBAR_W: f32 = 4.0;
const SCROLLBAR_INSET: f32 = 5.0;
const SCROLLBAR_MIN: f32 = 24.0;
const SCROLLBAR_ALPHA: f32 = 0.22;
/// A growing panel's content fades in from this openness.
const CONTENT_FROM: f32 = 0.35;
/// Opening spring of the grid / list panel.
const OPEN_SPRING: SpringConfig = SpringConfig::new(420.0, 30.0, 1.0);
/// Previews are decoded this much sharper than shown (crisp at the
/// pill's rim), and the dock tile's at up to this output scale.
const PREVIEW_OVERSAMPLE: f32 = 1.0;
const TILE_THUMB_SCALE: f32 = 2.0;
const PREVIEW_RADIUS_FRAC: f32 = 0.18;
/// Fan geometry (logical px): icon size, gap between icons along the arc,
/// gap above the stack icon, and the arc radius in items — 58 items of
/// radius bend a 12-item fan's top about 1.2 icons sideways, like macOS.
const FAN_ICON: f32 = 48.0;
const FAN_ROW_GAP: f32 = 8.0;
const FAN_GAP: f32 = 14.0;
const FAN_ARC_ROWS: f32 = 58.0;
/// Fan labels: pill height, padding, gap to the icon, width estimate per
/// character and bounds, name length.
const FAN_LABEL_H: f32 = 26.0;
const FAN_LABEL_PAD: f32 = 11.0;
const FAN_LABEL_GAP: f32 = 10.0;
const FAN_CHAR_PX: f32 = 7.0;
const FAN_LABEL_MIN_W: f32 = 48.0;
const FAN_LABEL_MAX_W: f32 = 280.0;
const FAN_NAME_CHARS: usize = 34;
const FAN_GLYPH_FRAC: f32 = 0.42;
/// Fan choreography (macOS): the whole fan, laid out and ready, pops up
/// out of the stack icon as one unit on a single spring (damping ratio
/// 0.72 ≈ 4 % overshoot, ω = 26 rad/s ≈ 0.25 s), growing from 60 % about
/// the icon; the names fade in with it. Closing folds it back the same way.
const FAN_DAMPING_RATIO: f32 = 0.72;
const FAN_OMEGA: f32 = 26.0;
const FAN_SETTLE_S: f32 = 0.4;
const FAN_CLOSE_S: f32 = 0.16;
const FAN_START_SCALE: f32 = 0.6;
const FAN_LABEL_FROM: f32 = 0.3;
const FAN_FADE_GAIN: f32 = 2.5;
/// Time a trash confirmation stays armed.
const CONFIRM_FOR: Duration = Duration::from_secs(6);

// ── Listing ──────────────────────────────────────────────────────────────

/// One file of a stack.
#[derive(Clone, Debug, PartialEq)]
pub struct StackEntry {
    pub path: PathBuf,
    pub name: String,
    pub dir: bool,
    /// When it appeared in the folder (status-change time: moving or
    /// downloading a file in sets it), nanoseconds.
    pub added: i64,
    pub modified: i64,
    /// Lower-case extension ("" for folders and plain names).
    pub ext: String,
    pub bytes: u64,
}

fn visible_name(name: &str) -> bool {
    !name.starts_with('.') && !PARTIAL_SUFFIXES.iter().any(|s| name.ends_with(s))
}

/// The files of `dir`, sorted by `sort` (dates newest first), at most
/// `limit` (0 = all). Hidden files and partial downloads are skipped.
pub fn list(dir: &Path, sort: StackSort, limit: usize) -> Vec<StackEntry> {
    use std::os::unix::fs::MetadataExt as _;
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<StackEntry> = read
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !visible_name(&name) {
                return None;
            }
            let meta = e.metadata().ok()?;
            let dir = meta.is_dir();
            let ext = if dir { String::new() } else { Path::new(&name).extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default() };
            Some(StackEntry { path: e.path(), name, dir, added: meta.ctime() * NANOS + meta.ctime_nsec(), modified: meta.mtime() * NANOS + meta.mtime_nsec(), ext, bytes: meta.len() })
        })
        .collect();
    sort_entries(&mut out, sort);
    if limit > 0 {
        out.truncate(limit);
    }
    out
}

pub fn sort_entries(list: &mut [StackEntry], sort: StackSort) {
    let by_name = |a: &StackEntry, b: &StackEntry| a.name.to_lowercase().cmp(&b.name.to_lowercase());
    match sort {
        StackSort::DateAdded => list.sort_by(|a, b| b.added.cmp(&a.added).then_with(|| by_name(a, b))),
        StackSort::DateModified => list.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| by_name(a, b))),
        StackSort::Name => list.sort_by(by_name),
        StackSort::Kind => list.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.ext.cmp(&b.ext)).then_with(|| by_name(a, b))),
    }
}

/// The popover layout `view` opens as: Automatic is the fan of the newest
/// files, whatever the folder holds (its top row opens the rest).
pub fn effective_view(view: StackView) -> StackView {
    match view {
        StackView::Auto => StackView::Fan,
        v => v,
    }
}

/// Icon-theme names for a file, best first (themes differ in which
/// mime-type icons they ship; the generic ones always come last).
pub fn file_icon_names(e: &StackEntry) -> &'static [&'static str] {
    if e.dir {
        return &["folder", "inode-directory"];
    }
    match e.ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "avif" | "heic" | "tiff" => &["image-x-generic"],
        "mp4" | "mkv" | "webm" | "mov" | "avi" => &["video-x-generic"],
        "mp3" | "flac" | "ogg" | "opus" | "wav" | "m4a" => &["audio-x-generic"],
        "pdf" => &["application-pdf", "x-office-document", "text-x-generic"],
        "zip" | "7z" | "rar" => &["application-zip", "package-x-generic"],
        "tar" | "gz" | "xz" | "zst" | "bz2" | "tgz" => &["application-x-compressed-tar", "package-x-generic"],
        "deb" | "rpm" | "appimage" | "flatpak" | "flatpakref" | "run" => &["application-x-executable", "application-x-sharedlib", "package-x-generic"],
        "bin" | "fw" | "rom" => &["application-x-firmware", "application-x-executable"],
        "html" | "htm" => &["text-html", "text-x-generic"],
        "md" | "markdown" => &["text-markdown", "text-x-generic"],
        "json" | "toml" | "yaml" | "yml" | "xml" => &["text-x-script", "application-json", "text-x-generic"],
        "doc" | "docx" | "odt" | "rtf" => &["x-office-document", "text-x-generic"],
        "xls" | "xlsx" | "ods" | "csv" => &["x-office-spreadsheet", "text-x-generic"],
        "ppt" | "pptx" | "odp" => &["x-office-presentation", "text-x-generic"],
        "iso" | "img" => &["media-optical", "drive-optical", "package-x-generic"],
        "ttf" | "otf" | "woff" | "woff2" => &["font-x-generic", "text-x-generic"],
        _ => &["text-x-generic"],
    }
}

/// The primary icon-theme name for a file.
pub fn file_icon_name(e: &StackEntry) -> &'static str {
    file_icon_names(e)[0]
}

pub fn is_image(e: &StackEntry) -> bool {
    matches!(e.ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp")
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

pub fn stack_dir(s: &DockStack) -> PathBuf {
    s.resolve(&home())
}

/// The folder's display name ("Downloads").
pub fn stack_name(s: &DockStack) -> String {
    stack_dir(s).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| s.path.clone())
}

// ── Cache + watcher ──────────────────────────────────────────────────────

/// Listed folders, invalidated by the watcher.
#[derive(Default)]
struct Cache {
    lists: HashMap<(PathBuf, u8), Vec<StackEntry>>,
    watched: Vec<PathBuf>,
    watcher: Option<notify::RecommendedWatcher>,
    trash: Option<usize>,
    /// Dock tile previews by folder: the newest file and its thumbnail.
    tiles: HashMap<PathBuf, (PathBuf, Option<Arc<RenderImage>>)>,
}
impl Global for Cache {}

fn sort_key(s: StackSort) -> u8 {
    s as u8
}

/// The newest files of stack `s` (cached; read on first use).
pub fn entries(s: &DockStack, limit: usize, cx: &mut App) -> Vec<StackEntry> {
    let dir = stack_dir(s);
    let key = (dir.clone(), sort_key(s.sort));
    if let Some(list) = cx.try_global::<Cache>().and_then(|c| c.lists.get(&key)) {
        return list.iter().take(limit.max(1)).cloned().collect();
    }
    // Cache as many as any stack can show, whoever asks first.
    let list = list(&dir, s.sort, mandara_config::dock::ranges::STACK_ITEMS.max as usize);
    watch(&dir, cx);
    cx.default_global::<Cache>().lists.insert(key, list.clone());
    list.into_iter().take(limit.max(1)).collect()
}

/// Watches `dir` (once) and drops its cached listings when it changes.
fn watch(dir: &Path, cx: &mut App) {
    use notify::Watcher as _;
    if cx.default_global::<Cache>().watched.iter().any(|d| d == dir) {
        return;
    }
    if cx.global::<Cache>().watcher.is_none() {
        let (tx, rx) = async_channel::unbounded::<PathBuf>();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(e) = res
                && !matches!(e.kind, notify::EventKind::Access(_))
            {
                for p in e.paths {
                    let _ = tx.try_send(p);
                }
            }
        });
        match watcher {
            Ok(w) => cx.global_mut::<Cache>().watcher = Some(w),
            Err(err) => {
                tracing::warn!("dock stacks: not watching folders: {err}");
                return;
            }
        }
        cx.spawn(async move |cx| {
            while let Ok(first) = rx.recv().await {
                let mut changed = vec![first];
                while let Ok(p) = rx.try_recv() {
                    changed.push(p);
                }
                cx.update(|cx| {
                    let trash = trash_files_dir();
                    let cache = cx.default_global::<Cache>();
                    cache.lists.retain(|(d, _), _| !changed.iter().any(|p| p.parent() == Some(d.as_path()) || p == d));
                    if changed.iter().any(|p| p.starts_with(&trash)) {
                        cache.trash = None;
                    }
                    super::refresh_all(cx);
                });
            }
        })
        .detach();
    }
    let cache = cx.global_mut::<Cache>();
    if let Some(w) = cache.watcher.as_mut() {
        match w.watch(dir, notify::RecursiveMode::NonRecursive) {
            Ok(()) => cache.watched.push(dir.to_path_buf()),
            Err(err) => tracing::debug!("dock stacks: watching {}: {err}", dir.display()),
        }
    }
}

// ── Trash ────────────────────────────────────────────────────────────────

pub fn trash_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| home().join(".local/share")).join("Trash")
}

fn trash_files_dir() -> PathBuf {
    trash_dir().join("files")
}

/// Items in the Trash (cached until the folder changes).
pub fn trash_count(cx: &mut App) -> usize {
    if let Some(n) = cx.try_global::<Cache>().and_then(|c| c.trash) {
        return n;
    }
    let dir = trash_files_dir();
    let n = std::fs::read_dir(&dir).map(|r| r.count()).unwrap_or(0);
    if dir.is_dir() {
        watch(&dir, cx);
    }
    cx.default_global::<Cache>().trash = Some(n);
    n
}

/// Moves `paths` to the Trash. Returns how many moved.
pub fn move_to_trash(paths: &[PathBuf]) -> Result<usize, String> {
    let env = mandara_services::app_uninstall::Env::with_home(&home());
    let env = mandara_services::app_uninstall::Env { data_home: dirs::data_dir().unwrap_or(env.data_home.clone()), ..env };
    let mut moved = 0;
    for p in paths {
        mandara_services::app_uninstall::trash(p, &env)?;
        moved += 1;
    }
    Ok(moved)
}

/// Empties the Trash folder `trash` (its `files`, `info` and
/// `directorysizes`). Only called after the user confirmed. Returns how
/// many items went.
pub fn empty_trash_in(trash: &Path) -> Result<usize, String> {
    let mut n = 0;
    for sub in ["files", "info"] {
        let Ok(read) = std::fs::read_dir(trash.join(sub)) else { continue };
        for e in read.flatten() {
            let p = e.path();
            let r = if e.file_type().is_ok_and(|t| t.is_dir()) { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
            r.map_err(|err| tf("Could not remove {path}: {error}", &[("path", &p.display()), ("error", &err)]))?;
            if sub == "files" {
                n += 1;
            }
        }
    }
    let _ = std::fs::remove_file(trash.join("directorysizes"));
    Ok(n)
}

/// Trash confirmation: armed by the first "Empty Trash…" click.
#[derive(Default)]
pub struct TrashConfirm(pub Option<Instant>);
impl Global for TrashConfirm {}

/// "Empty Trash…": the first call arms a confirmation, a second one
/// within [`CONFIRM_FOR`] empties. Returns `Some(count)` once emptied.
pub fn empty_trash_confirmed(confirmed: bool, cx: &mut App) -> Result<Option<usize>, String> {
    let armed = cx.try_global::<TrashConfirm>().and_then(|c| c.0).is_some_and(|t| t.elapsed() < CONFIRM_FOR);
    if !(armed || confirmed) {
        cx.set_global(TrashConfirm(Some(Instant::now())));
        return Ok(None);
    }
    cx.set_global(TrashConfirm(None));
    let n = empty_trash_in(&trash_dir())?;
    cx.default_global::<Cache>().trash = Some(0);
    Ok(Some(n))
}

// ── Fan geometry and choreography ────────────────────────────────────────

/// Where fan item `i` (0 = the newest, just above the stack icon) sits
/// relative to the first item: `(sideways, along)` px on a circular arc of
/// `radius` with `step` px between items. The arc is tangent to the dock's
/// normal at the icon, so the column rises straight out of the stack and
/// bends gently sideways the higher it goes (macOS: about a tenth of an
/// icon per item at the top of a 12-item fan).
pub fn fan_point(i: usize, step: f32, radius: f32) -> (f32, f32) {
    let theta = i as f32 * step / radius.max(step);
    (radius * (1.0 - theta.cos()), radius * theta.sin())
}

/// Opening progress of an item `t` seconds after its own start: a damped
/// spring's step response (0 → 1 with a slight overshoot), 0 before it.
pub fn fan_rise(t: f32) -> f32 {
    if t <= 0.0 {
        return 0.0;
    }
    let (z, w) = (FAN_DAMPING_RATIO, FAN_OMEGA);
    let wd = w * (1.0 - z * z).sqrt();
    1.0 - (-z * w * t).exp() * ((wd * t).cos() + z * w / wd * (wd * t).sin())
}

/// Closing progress (1 → 0, eased in, no overshoot past the icon).
pub fn fan_fall(t: f32) -> f32 {
    let u = (t / FAN_CLOSE_S).clamp(0.0, 1.0);
    1.0 - u * u * (3.0 - 2.0 * u)
}

/// The whole fan's progress: rising `age` s after opening or, while
/// closing for `closing` s, folding back into the icon.
pub fn fan_progress(age: f32, closing: Option<f32>) -> f32 {
    match closing {
        None => fan_rise(age),
        Some(t) => fan_rise(age).min(1.0) * fan_fall(t),
    }
}

/// Whether the fan is still moving.
pub fn fan_moving(age: f32, closing: Option<f32>) -> bool {
    match closing {
        None => age < FAN_SETTLE_S,
        Some(t) => t < FAN_CLOSE_S,
    }
}

// ── Popover ──────────────────────────────────────────────────────────────

pub struct StackPopover {
    stack: DockStack,
    files: Vec<StackEntry>,
    /// Files in the folder beyond the ones shown (the fan's "+N more").
    more: usize,
    view: StackView,
    anchor: (f32, f32, f32, f32),
    position: DockPosition,
    names: bool,
    focus: FocusHandle,
    sheet: Sheet,
    thumbs: Thumbs,
    scroll: ScrollHandle,
    /// Keyboard selection (index into the visible files).
    selected: Option<usize>,
    /// Type-to-filter (grid and list).
    filter: String,
    /// The item menu: file index and where (surface coordinates).
    menu: Option<(usize, (f32, f32))>,
}

impl StackPopover {
    fn close(&mut self, cx: &mut Context<Self>) {
        self.sheet.close();
        cx.notify();
    }

    fn open_file(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(e) = self.files.get(i) {
            open_path(&e.path);
        }
        self.close(cx);
    }

    /// Indices of the files the filter lets through.
    fn visible(&self) -> Vec<usize> {
        let q = self.filter.to_lowercase();
        (0..self.files.len()).filter(|&i| q.is_empty() || self.files[i].name.to_lowercase().contains(&q)).collect()
    }

    fn on_key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        let shown = self.visible();
        let cols = if self.view == StackView::Grid { grid_columns(self.files.len()) } else { 1 };
        // A fan grows away from the dock: "up" moves to older files.
        let up_is_older = self.view == StackView::Fan;
        let pos = self.selected.and_then(|s| shown.iter().position(|&i| i == s));
        let key = e.keystroke.key.as_str();
        let step: Option<isize> = match key {
            "left" => Some(-1),
            "right" => Some(1),
            "up" => Some(if up_is_older { 1 } else { -(cols as isize) }),
            "down" => Some(if up_is_older { -1 } else { cols as isize }),
            "home" => Some(isize::MIN),
            "end" => Some(isize::MAX),
            _ => None,
        };
        if self.menu.take().is_some() && key == "escape" {
            cx.notify();
            return;
        }
        match key {
            "escape" if !self.filter.is_empty() => self.filter.clear(),
            "escape" => return self.close(cx),
            "enter" => {
                if let Some(i) = self.selected.or_else(|| shown.first().copied()) {
                    self.open_file(i, cx);
                }
                return;
            }
            "backspace" => {
                self.filter.pop();
            }
            _ if step.is_some() && !shown.is_empty() => {
                let last = shown.len() as isize - 1;
                let next = match (pos, step.unwrap_or(0)) {
                    (_, isize::MIN) => 0,
                    (_, isize::MAX) => last,
                    (None, _) => 0,
                    (Some(p), d) => (p as isize + d).clamp(0, last),
                };
                self.selected = shown.get(next as usize).copied();
                if let Some(s) = self.selected {
                    self.scroll.scroll_to_item(shown.iter().position(|&i| i == s).unwrap_or(0));
                }
            }
            _ => {
                // Type to filter (grid and list; a fan is too short to need it).
                if self.view != StackView::Fan
                    && let Some(ch) = e.keystroke.key_char.as_deref().filter(|c| !c.chars().any(char::is_control))
                    && !e.keystroke.modifiers.control
                    && !e.keystroke.modifiers.alt
                {
                    self.filter.push_str(ch);
                }
            }
        }
        let shown = self.visible();
        if self.selected.is_some_and(|s| !shown.contains(&s)) {
            self.selected = shown.first().copied();
        }
        cx.notify();
    }

    fn menu_rows() -> Vec<MenuRow> {
        vec![MenuRow::new(t("Open"), "play"), MenuRow::new(t("Show in Files"), "folder"), MenuRow::new(t("Move to Trash"), "trash").sep().danger()]
    }

    fn run_menu(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some((i, _)) = self.menu.take() else { return };
        let Some(e) = self.files.get(i).cloned() else { return };
        match row {
            0 => return self.open_file(i, cx),
            1 => open_path(e.path.parent().unwrap_or(&e.path)),
            _ => match move_to_trash(std::slice::from_ref(&e.path)) {
                Ok(_) => {
                    self.files.remove(i);
                    self.selected = None;
                    cx.notify();
                    return;
                }
                Err(err) => tracing::warn!("dock stack: {err}"),
            },
        }
        self.close(cx);
    }

    /// A file's icon at `side` px: its preview (images) or its type icon.
    /// File `i`'s icon or preview at `side`, decoded for `final_side` (the
    /// fan's items grow as they fly out: decoding at every size on the way
    /// asked for a new thumbnail each frame and made the fan stutter).
    fn file_icon(&mut self, i: usize, side: f32, final_side: f32, scale: f32, palette: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let e = &self.files[i];
        if is_image(e) && e.bytes <= PREVIEW_MAX_BYTES {
            let path = e.path.clone();
            let dev = (final_side.max(side) * scale * PREVIEW_OVERSAMPLE).ceil() as u32;
            if let Some(thumb) = self.thumbs.get(&path, dev, |v: &mut StackPopover| &mut v.thumbs, cx) {
                return div()
                    .size(px(side))
                    .flex_none()
                    .rounded(px(crate::style::radius(side * PREVIEW_RADIUS_FRAC)))
                    .overflow_hidden()
                    .shadow(rim(palette))
                    .child(img(ImageSource::Render(thumb)).size_full().object_fit(ObjectFit::Cover))
                    .into_any_element();
            }
        }
        super::themed_at(file_icon_names(e), "note", side, final_side, palette, palette.on_surface_variant)
    }

    fn render_fan(&mut self, p: f32, screen: (f32, f32), palette: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let scale = window.scale_factor();
        let n = self.files.len();
        let closing = self.sheet.closing().then(|| self.sheet.closing_for());
        let age = self.sheet.age();
        let reduce = mandara_core::spring::motion_profile().reduce;
        let (ax, ay, aw, ah) = self.anchor;
        let origin = (ax + aw / 2.0, ay + ah / 2.0);
        // Bend away from the reading start on a bottom dock (right in LTR),
        // away from the screen edge on a side dock; labels on the other side.
        let rtl = crate::bidi::rtl();
        let (along, bend): ((f32, f32), (f32, f32)) = match self.position {
            DockPosition::Bottom => ((0.0, -1.0), (if rtl { -1.0 } else { 1.0 }, 0.0)),
            DockPosition::Left => ((0.0, if origin.1 > screen.1 / 2.0 { -1.0 } else { 1.0 }), (1.0, 0.0)),
            DockPosition::Right => ((0.0, if origin.1 > screen.1 / 2.0 { -1.0 } else { 1.0 }), (-1.0, 0.0)),
        };
        // The first item's centre: just past the icon (bottom) or beside it.
        let first = match self.position {
            DockPosition::Bottom => (origin.0, ay - FAN_GAP - FAN_ICON / 2.0),
            DockPosition::Left => (ax + aw + FAN_GAP + FAN_ICON / 2.0, origin.1),
            DockPosition::Right => (ax - FAN_GAP - FAN_ICON / 2.0, origin.1),
        };
        // Labels sit on the side the arc bends from (macOS: left of the
        // icons in LTR), and never over a side dock.
        let labels_left = match self.position {
            DockPosition::Bottom => bend.0 > 0.0,
            DockPosition::Left => false,
            DockPosition::Right => true,
        };
        let step = FAN_ICON + FAN_ROW_GAP;
        let radius = step * FAN_ARC_ROWS;
        let accent = readable(palette.primary, palette);
        let on_accent = accent.best_on(Color::hex("#ffffff"), Color::hex("#101010"));
        let pill = sheet::glass(palette, &config(cx).read(cx).get().dock);
        let mut out = Vec::with_capacity(n + 1);
        let mut moving = false;
        for k in 0..=n {
            let q = if reduce { if closing.is_some() { 0.0 } else { 1.0 } } else { fan_progress(age, closing) };
            moving |= q > 0.0 && q < 1.0;
            let (side, up) = fan_point(k, step, radius);
            let target = (first.0 + bend.0 * side + along.0 * up, first.1 + bend.1 * side + along.1 * up);
            // The laid-out fan scales about the stack icon as one piece
            // (from FAN_START_SCALE, overshooting slightly) while it fades in.
            let g = FAN_START_SCALE + (1.0 - FAN_START_SCALE) * q;
            let c = (origin.0 + (target.0 - origin.0) * g, origin.1 + (target.1 - origin.1) * g);
            let s = FAN_ICON * g.min(1.0);
            let label_alpha = ((q - FAN_LABEL_FROM) / (1.0 - FAN_LABEL_FROM)).clamp(0.0, 1.0) * p.min(1.0);
            let item_alpha = (q * FAN_FADE_GAIN).clamp(0.0, 1.0);
            let is_folder_row = k == n;
            let (label, hot): (SharedString, bool) = if is_folder_row {
                let folder = stack_name(&self.stack);
                let l = if self.more > 0 { tf("Open {folder} · {n} more", &[("folder", &folder), ("n", &self.more)]) } else { tf("Open {folder}", &[("folder", &folder)]) };
                (l.into(), false)
            } else {
                (crate::kit::ellipsize(&self.files[k].name, FAN_NAME_CHARS).into(), self.selected == Some(k))
            };
            let tile: AnyElement = if is_folder_row {
                div().size(px(s)).rounded_full().bg(pill.hsla()).shadow(sheet::panel_shadow(palette, 1.0)).flex().items_center().justify_center().child(icon(crate::bidi::dir_icon("external"), s * FAN_GLYPH_FRAC, accent)).into_any_element()
            } else {
                self.file_icon(k, s, FAN_ICON, scale, palette, cx)
            };
            let label_w = (label.chars().count() as f32 * FAN_CHAR_PX + FAN_LABEL_PAD * 2.0).clamp(FAN_LABEL_MIN_W, FAN_LABEL_MAX_W);
            let label_el = div()
                .id(("stack-fan-label", k))
                .h(px(FAN_LABEL_H))
                .max_w(px(FAN_LABEL_MAX_W))
                .px(px(FAN_LABEL_PAD))
                .rounded_full()
                .flex()
                .items_center()
                .opacity(label_alpha)
                .text_size(px(text::BODY_S))
                .font_weight(FontWeight::MEDIUM)
                .truncate()
                .when(hot, |el| el.bg(accent.hsla()).text_color(on_accent.hsla()))
                .when(!hot, |el| el.bg(pill.hsla()).shadow(rim(palette)))
                .child(label);
            let w = s + FAN_LABEL_GAP + label_w;
            let x = if labels_left { c.0 + s / 2.0 - w } else { c.0 - s / 2.0 };
            let mut row = div().id(("stack-fan", k)).absolute().left(px(x)).top(px(c.1 - s / 2.0)).h(px(s)).w(px(w)).flex().items_center().gap(px(FAN_LABEL_GAP)).opacity(item_alpha).cursor_pointer();
            row = if labels_left { row.flex_row().justify_end().child(label_el).child(tile) } else { row.flex_row().child(tile).child(label_el) };
            if is_folder_row {
                let dir = stack_dir(&self.stack);
                row = row.on_click(cx.listener(move |this, _, _, cx| {
                    open_path(&dir);
                    this.close(cx);
                }));
            } else {
                row = row
                    // Hover highlights the label like the keyboard selection.
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.selected = Some(k);
                        } else if this.selected == Some(k) {
                            this.selected = None;
                        }
                        cx.notify();
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_file(k, cx)))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.menu = Some((k, (f32::from(e.position.x), f32::from(e.position.y))));
                        cx.notify();
                    }));
            }
            out.push(row.into_any_element());
        }
        if !self.sheet.gone() && (moving || (!reduce && fan_moving(age, closing))) {
            window.request_animation_frame();
        }
        out
    }

    fn file_tile(&mut self, i: usize, palette: &MandaraPalette, scale: f32, hover: gpui::Hsla, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.selected == Some(i);
        let accent = readable(palette.primary, palette);
        let name = self.files[i].name.clone();
        let icon_el = self.file_icon(i, GRID_ICON, GRID_ICON, scale, palette, cx);
        div()
            .id(("stack-file", i))
            .w(px(GRID_TILE_W))
            .h(px(GRID_TILE_H))
            .flex()
            .flex_col()
            .items_center()
            .pt(px(GRID_TILE_PAD))
            .gap(px(GRID_NAME_GAP))
            .rounded(px(crate::style::radius(ROW_RADIUS)))
            .cursor_pointer()
            .when(selected, |el| el.bg(accent.with_alpha(SELECTED_ALPHA).hsla()))
            .hover(|s| s.bg(hover))
            .child(icon_el)
            .when(self.names, |el| {
                el.child(div().w_full().px(px(GRID_NAME_PAD)).text_center().line_clamp(GRID_NAME_LINES).text_ellipsis().text_size(px(text::LABEL)).line_height(px(GRID_NAME_LINE_H)).when(selected, |el| el.text_color(accent.hsla())).child(name))
            })
            .on_click(cx.listener(move |this, _, _, cx| this.open_file(i, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.selected = Some(i);
                this.menu = Some((i, (f32::from(e.position.x), f32::from(e.position.y))));
                cx.notify();
            }))
            .into_any_element()
    }
}

/// Grid columns for `n` files.
pub fn grid_columns(n: usize) -> usize {
    n.clamp(1, GRID_COLUMNS)
}

impl Render for StackPopover {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = mandara_core::frame_palette(window, cx);
        let f = crate::fonts::fonts(cx);
        let vp = window.viewport_size();
        let screen = (f32::from(vp.width), f32::from(vp.height));
        let fan = self.view == StackView::Fan;
        let hold = fan && !mandara_core::spring::motion_profile().reduce && fan_moving(self.sheet.age(), Some(self.sheet.closing_for())) && self.sheet.closing();
        let p = self.sheet.step(hold, window, cx);
        if self.sheet.closing() && p <= 0.0 && !hold {
            self.thumbs.clear(window, cx);
        }
        let dock = config(cx).read(cx).get().dock.clone();
        let fill = sheet::glass(&palette, &dock);
        let accent = readable(palette.primary, &palette);
        let hover = palette.on_surface.with_alpha(HOVER_ALPHA).hsla();
        let scale = window.scale_factor();
        let mut root = div()
            .id("stack-root")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .font_family(f.ui.clone())
            .text_color(palette.on_surface.hsla())
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| this.on_key(e, cx)))
            .child(sheet::outside("stack-outside", cx, |this: &mut Self, cx| {
                if this.menu.take().is_some() {
                    cx.notify();
                } else {
                    this.close(cx);
                }
            }));
        if fan {
            root = root.children(self.render_fan(p, screen, &palette, window, cx));
        } else {
            let shown = self.visible();
            let n = self.files.len();
            let (w, body_h) = match self.view {
                StackView::Grid => {
                    let cols = grid_columns(n);
                    let rows = n.div_ceil(cols).clamp(1, GRID_MAX_ROWS);
                    (cols as f32 * GRID_TILE_W + POP_PAD * 2.0, rows as f32 * GRID_TILE_H)
                }
                _ => (LIST_W, (n.clamp(1, LIST_MAX_ROWS)) as f32 * LIST_ROW_H),
            };
            let h = body_h + POP_PAD * 2.0 + POP_HEADER_H;
            let (x, y) = sheet::place_card(self.position, self.anchor, (w, h), screen, POP_GAP);
            let e = p.clamp(0.0, 1.0);
            let card = sheet::grow_from(self.anchor, (x, y, w, h), e);
            let content_alpha = ((p - CONTENT_FROM) / (1.0 - CONTENT_FROM)).clamp(0.0, 1.0);
            let mut body = div().flex();
            match self.view {
                StackView::Grid => {
                    body = body.flex_row().flex_wrap().w(px(w - POP_PAD * 2.0));
                    for &i in &shown {
                        body = body.child(self.file_tile(i, &palette, scale, hover, cx));
                    }
                }
                _ => {
                    body = body.flex_col();
                    for &i in &shown {
                        let selected = self.selected == Some(i);
                        let icon_el = self.file_icon(i, LIST_ICON, LIST_ICON, scale, &palette, cx);
                        let e = &self.files[i];
                        body = body.child(
                            div()
                                .id(("stack-row", i))
                                .h(px(LIST_ROW_H))
                                .px(px(ROW_PAD))
                                .flex()
                                .items_center()
                                .gap(px(ROW_GAP))
                                .rounded(px(crate::style::radius(ROW_RADIUS)))
                                .cursor_pointer()
                                .when(selected, |el| el.bg(accent.with_alpha(SELECTED_ALPHA).hsla()))
                                .hover(|s| s.bg(hover))
                                .child(icon_el)
                                .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(text::BODY_S)).child(e.name.clone()))
                                .on_click(cx.listener(move |this, _, _, cx| this.open_file(i, cx)))
                                .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.selected = Some(i);
                                    this.menu = Some((i, (f32::from(ev.position.x), f32::from(ev.position.y))));
                                    cx.notify();
                                })),
                        );
                    }
                }
            }
            if shown.is_empty() {
                let msg = if self.files.is_empty() { t("This folder is empty").to_string() } else { tf("No files match “{query}”", &[("query", &self.filter)]) };
                body = body.child(div().p(px(ROW_PAD)).text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(msg));
            }
            // A thin scrollbar once the files overflow.
            let (offset, max) = (f32::from(self.scroll.offset().y).abs(), f32::from(self.scroll.max_offset().y));
            let track = body_h;
            let bar = (max > 0.0).then(|| {
                let len = (track * track / (track + max)).max(SCROLLBAR_MIN);
                let top = (track - len) * (offset / max).clamp(0.0, 1.0);
                div().absolute().right(px(SCROLLBAR_INSET)).top(px(POP_HEADER_H + POP_PAD + top)).w(px(SCROLLBAR_W)).h(px(len)).rounded_full().bg(palette.on_surface.with_alpha(SCROLLBAR_ALPHA).hsla())
            });
            let dir = stack_dir(&self.stack);
            let title = stack_name(&self.stack);
            let header = div()
                .h(px(POP_HEADER_H))
                .flex()
                .items_center()
                .gap(px(ROW_GAP))
                .child(icon("folder", text::TITLE, accent))
                .child(div().flex_1().min_w(px(0.0)).truncate().font_weight(FontWeight::SEMIBOLD).text_size(px(text::TITLE)).child(title))
                .when(!self.filter.is_empty(), |el| el.child(div().px(px(FILTER_PAD)).h(px(FILTER_H)).rounded_full().flex().items_center().gap(px(FILTER_GAP)).bg(accent.with_alpha(SELECTED_ALPHA).hsla()).text_size(px(text::LABEL)).child(icon("search", text::LABEL, accent)).child(self.filter.clone())))
                .child(
                    div()
                        .id("stack-open-folder")
                        .h(px(BUTTON_H))
                        .px(px(BUTTON_PAD))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .gap(px(FILTER_GAP))
                        .cursor_pointer()
                        .bg(wash(WASH_SOFT, &palette).hsla())
                        .hover(|s| s.bg(hover))
                        .text_color(accent.hsla())
                        .text_size(px(text::BODY_S))
                        .font_weight(FontWeight::MEDIUM)
                        .child(icon(crate::bidi::dir_icon("external"), text::BODY_S, accent))
                        .child(t("Open in Files"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            open_path(&dir);
                            this.close(cx);
                        })),
                );
            let content = div()
                .absolute()
                .left(px(x - card.0))
                .top(px(y - card.1))
                .w(px(w))
                .h(px(h))
                .p(px(POP_PAD))
                .opacity(content_alpha)
                .flex()
                .flex_col()
                .child(header)
                .child(div().id("stack-scroll").h(px(body_h)).overflow_y_scroll().track_scroll(&self.scroll).child(body))
                .children(bar);
            let radius = POP_RADIUS + (self.anchor.2 * super::TILE_RADIUS_FRAC - POP_RADIUS) * (1.0 - e);
            root = root.child(
                div()
                    .id("stack-card")
                    .occlude()
                    .absolute()
                    .left(px(card.0))
                    .top(px(card.1))
                    .w(px(card.2))
                    .h(px(card.3))
                    .opacity(p.clamp(0.0, 1.0).sqrt())
                    .rounded(px(crate::style::radius(radius)))
                    .overflow_hidden()
                    .bg(fill.hsla())
                    .shadow(sheet::panel_shadow(&palette, p))
                    .on_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        if this.menu.take().is_some() {
                            cx.notify();
                        }
                    }))
                    .child(content),
            );
        }
        if let Some((_, at)) = self.menu {
            let on: sheet::MenuHandler<Self> = Rc::new(|this, row, _, cx| this.run_menu(row, cx));
            root = root.child(sheet::item_menu(Self::menu_rows(), at, screen, &palette, fill, on, cx));
        }
        root
    }
}

#[derive(Default)]
struct Popover(Option<(usize, WindowHandle<StackPopover>)>);
impl Global for Popover {}

/// Opens a file or folder with its default app, off the UI thread.
pub fn open_path(path: &Path) {
    let uri = format!("file://{}", mandara_services::app_uninstall::percent_encode(path));
    std::thread::spawn(move || {
        if let Err(err) = mandara_services::session::open_uri(&uri) {
            tracing::warn!("dock: opening {uri}: {err}");
        }
    });
}

/// Whether stack `index`'s popover is open (and not closing).
pub fn popover_open(index: usize, cx: &mut App) -> bool {
    match cx.try_global::<Popover>().and_then(|p| p.0) {
        Some((i, h)) if i == index => h.read_with(cx, |v, _| !v.sheet.closing()).unwrap_or(false),
        _ => false,
    }
}

/// Closes an open stack popover. Returns whether one was open.
pub fn close_popover(cx: &mut App) -> bool {
    let Some((_, h)) = cx.try_global::<Popover>().and_then(|p| p.0) else { return false };
    h.update(cx, |v, _, cx| {
        let was = !v.sheet.closing();
        v.close(cx);
        was
    })
    .unwrap_or(false)
}

/// Opens stack `index`'s popover over `anchor` (output coordinates), or
/// closes it when it is already open (clicking the stack again).
pub fn toggle_popover(index: usize, monitor: &str, anchor: (f32, f32, f32, f32), cx: &mut App) {
    if popover_open(index, cx) {
        close_popover(cx);
    } else {
        open_popover(index, monitor, anchor, cx);
    }
}

/// Opens stack `index`'s popover over `anchor` (output coordinates).
pub fn open_popover(index: usize, monitor: &str, anchor: (f32, f32, f32, f32), cx: &mut App) {
    let cfg = config(cx).read(cx).get().dock_sanitized();
    let Some(stack) = cfg.stacks.get(index).cloned() else { return };
    let all = entries(&stack, mandara_config::dock::ranges::STACK_ITEMS.max as usize, cx);
    let total = folder_len(&stack, all.len());
    let view = effective_view(stack.view);
    let shown = if view == StackView::Fan { cfg.fan_items as usize } else { cfg.stack_items as usize };
    let files: Vec<StackEntry> = all.into_iter().take(shown).collect();
    let more = total.saturating_sub(files.len());
    if let Some((_, h)) = cx.try_global::<Popover>().and_then(|m| m.0) {
        let _ = h.update(cx, |_, window, _| window.remove_window());
        super::popover_closed(h.into(), cx);
    }
    let options = sheet::popover_options(monitor, KeyboardInteractivity::OnDemand, cx);
    let (position, names) = (cfg.position, cfg.stack_names);
    let handle = cx.open_window(options, move |window, cx| {
        cx.new(|cx| {
            let focus = cx.focus_handle();
            focus.focus(window, cx);
            StackPopover { stack, files, more, view, anchor, position, names, focus, sheet: Sheet::new(OPEN_SPRING), thumbs: Thumbs::default(), scroll: ScrollHandle::new(), selected: None, filter: String::new(), menu: None }
        })
    });
    match handle {
        Ok(h) => {
            cx.set_global(Popover(Some((index, h))));
            super::popover_opened(monitor, h.into(), cx);
        }
        Err(err) => tracing::warn!("dock stack: {err:#}"),
    }
}

/// How many files stack `s` holds (a cached listing is capped; count the
/// folder only when the cap was reached).
fn folder_len(s: &DockStack, listed: usize) -> usize {
    if listed < mandara_config::dock::ranges::STACK_ITEMS.max as usize {
        return listed;
    }
    std::fs::read_dir(stack_dir(s)).map(|r| r.flatten().filter(|e| visible_name(&e.file_name().to_string_lossy())).count()).unwrap_or(listed)
}

/// The stack's dock tile: its newest file over a folder (Stack), or the
/// folder icon.
pub fn stack_tile(stack: &DockStack, size: f32, palette: &MandaraPalette, cx: &mut App) -> AnyElement {
    use mandara_config::workflow::StackDisplay;
    let base = div().size(px(size)).relative().flex().items_center().justify_center();
    let is_downloads = stack_dir(stack).file_name().is_some_and(|n| n == "Downloads");
    let names: &[&str] = if is_downloads { &["folder-download", "folder-downloads", "folder"] } else { &["folder"] };
    let folder_icon = super::themed(names, if is_downloads { "download" } else { "folder" }, size, palette, palette.primary);
    if stack.display == StackDisplay::Folder {
        return base.child(folder_icon).into_any_element();
    }
    let top = entries(stack, 1, cx).into_iter().next();
    match top {
        Some(e) => {
            let inner = size * super::STACK_TOP_FRAC;
            let preview: AnyElement = match tile_thumb(&stack_dir(stack), &e, cx) {
                Some(thumb) => div().size(px(inner)).rounded(px(crate::style::radius(inner * super::TILE_RADIUS_FRAC))).overflow_hidden().child(img(ImageSource::Render(thumb)).size_full().object_fit(ObjectFit::Cover)).into_any_element(),
                None => super::themed(file_icon_names(&e), "note", inner, palette, palette.on_surface),
            };
            base.child(div().absolute().inset_0().opacity(super::STACK_FOLDER_ALPHA).child(folder_icon)).child(preview).into_any_element()
        }
        None => base.child(folder_icon).into_any_element(),
    }
}

/// A small preview of the newest file for the dock tile (decoded once at
/// tile size, replaced when the folder changes; never the full image).
fn tile_thumb(dir: &Path, e: &StackEntry, cx: &mut App) -> Option<Arc<RenderImage>> {
    if !is_image(e) || e.bytes > PREVIEW_MAX_BYTES {
        return None;
    }
    let cache = cx.default_global::<Cache>();
    match cache.tiles.get(dir) {
        Some((p, img)) if *p == e.path => return img.clone(),
        _ => {}
    }
    if let Some((_, Some(old))) = cache.tiles.insert(dir.to_path_buf(), (e.path.clone(), None)) {
        cx.drop_image(old, None);
    }
    let (path, key) = (e.path.clone(), dir.to_path_buf());
    let side = tile_thumb_px(cx);
    let task = cx.background_executor().spawn(async move { sheet::decode_thumb(&path, side) });
    let want = e.path.clone();
    cx.spawn(async move |cx| {
        let img = task.await;
        cx.update(|cx| {
            let cache = cx.default_global::<Cache>();
            if let Some(slot) = cache.tiles.get_mut(&key)
                && slot.0 == want
            {
                slot.1 = img;
                super::refresh_all(cx);
            }
        });
    })
    .detach();
    None
}

/// Device px of a dock tile's preview: the magnified icon at the
/// largest output scale.
fn tile_thumb_px(cx: &App) -> u32 {
    let d = config(cx).read(cx).get().dock_sanitized();
    (d.icon_size * d.magnify.max(1.0) * super::STACK_TOP_FRAC * TILE_THUMB_SCALE).ceil() as u32
}

/// The stack `name` means: its index, its folder name ("Downloads", any
/// case) or its path as configured; empty = the first.
pub fn find_stack(stacks: &[DockStack], name: &str) -> Option<usize> {
    let name = name.trim().trim_end_matches('/');
    if name.is_empty() {
        return (!stacks.is_empty()).then_some(0);
    }
    if let Ok(i) = name.parse::<usize>() {
        return (i < stacks.len()).then_some(i);
    }
    stacks.iter().position(|s| s.path.trim().trim_end_matches('/') == name).or_else(|| stacks.iter().position(|s| stack_name(s).eq_ignore_ascii_case(name)))
}

/// `n` items, for menus and tooltips.
pub fn items_label(n: usize) -> SharedString {
    tf("{n} items", &[("n", &n)]).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, added: i64, modified: i64, dir: bool) -> StackEntry {
        StackEntry { path: PathBuf::from(name), name: name.into(), dir, added, modified, bytes: 0, ext: Path::new(name).extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default() }
    }

    #[test]
    fn stacks_sort_newest_first_or_by_name_or_kind() {
        let base = vec![entry("b.pdf", 5, 9, false), entry("A.png", 7, 1, false), entry("c", 1, 5, true), entry("d.png", 7, 3, false)];
        let names = |s: StackSort| {
            let mut l = base.clone();
            sort_entries(&mut l, s);
            l.into_iter().map(|e| e.name).collect::<Vec<_>>()
        };
        assert_eq!(names(StackSort::DateAdded), ["A.png", "d.png", "b.pdf", "c"], "ties fall back to the name");
        assert_eq!(names(StackSort::DateModified), ["b.pdf", "c", "d.png", "A.png"]);
        assert_eq!(names(StackSort::Name), ["A.png", "b.pdf", "c", "d.png"]);
        assert_eq!(names(StackSort::Kind), ["c", "b.pdf", "A.png", "d.png"], "folders first, then by type");
    }

    #[test]
    fn listing_skips_hidden_and_partial_files() {
        let dir = std::env::temp_dir().join(format!("mandara-dock-stack-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join("sub"));
        for f in ["one.txt", ".hidden", "movie.mkv.part", "two.png"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let mut names: Vec<String> = list(&dir, StackSort::Name, 0).into_iter().map(|e| e.name).collect();
        names.sort();
        assert_eq!(names, ["one.txt", "sub", "two.png"]);
        assert_eq!(list(&dir, StackSort::Name, 2).len(), 2, "limited");
        assert!(list(&dir.join("missing"), StackSort::Name, 0).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn automatic_view_is_the_fan_and_explicit_views_stay() {
        assert_eq!(effective_view(StackView::Auto), StackView::Fan, "a big folder still fans its newest files");
        assert_eq!(effective_view(StackView::Grid), StackView::Grid);
        assert_eq!(effective_view(StackView::List), StackView::List);
        assert_eq!(effective_view(StackView::Fan), StackView::Fan);
        assert_eq!(file_icon_name(&entry("x.PDF", 0, 0, false)), "application-pdf", "extensions are lower-cased when listed");
        assert_eq!(file_icon_name(&entry("x.pdf", 0, 0, false)), "application-pdf");
        assert_eq!(file_icon_name(&entry("d", 0, 0, true)), "folder");
    }

    #[test]
    fn emptying_a_trash_removes_files_and_info_only() {
        let trash = std::env::temp_dir().join(format!("mandara-dock-trash-{}", std::process::id()));
        std::fs::create_dir_all(trash.join("files/dir")).unwrap();
        std::fs::create_dir_all(trash.join("info")).unwrap();
        std::fs::write(trash.join("files/a.txt"), b"x").unwrap();
        std::fs::write(trash.join("info/a.txt.trashinfo"), b"[Trash Info]").unwrap();
        std::fs::write(trash.join("info/dir.trashinfo"), b"[Trash Info]").unwrap();
        assert_eq!(empty_trash_in(&trash).unwrap(), 2);
        assert_eq!(std::fs::read_dir(trash.join("files")).unwrap().count(), 0);
        assert_eq!(std::fs::read_dir(trash.join("info")).unwrap().count(), 0);
        assert!(trash.is_dir(), "the Trash folder itself stays");
        let _ = std::fs::remove_dir_all(&trash);
    }

    #[test]
    fn the_fan_rises_straight_then_bends_gently() {
        let step = FAN_ICON + FAN_ROW_GAP;
        let r = step * FAN_ARC_ROWS;
        assert_eq!(fan_point(0, step, r), (0.0, 0.0), "the newest file sits right above the stack");
        let (s1, u1) = fan_point(1, step, r);
        assert!(s1 < 1.0 && (u1 - step).abs() < 0.5, "starts almost straight up: {s1} {u1}");
        let (s11, u11) = fan_point(11, step, r);
        assert!(s11 > FAN_ICON * 0.8 && s11 < FAN_ICON * 1.6, "a 12-item fan's top bends about an icon sideways: {s11}");
        assert!(u11 > step * 10.5 && u11 < step * 11.0, "items stay about one step apart along the arc: {u11}");
        let mut last = 0.0;
        for i in 1..20 {
            let (s, _) = fan_point(i, step, r);
            assert!(s > last, "the bend only grows");
            last = s;
        }
    }

    #[test]
    fn the_fan_pops_up_as_one_unit_with_a_small_overshoot_and_folds_back() {
        assert_eq!(fan_rise(0.0), 0.0);
        let peak = (1..100).map(|k| fan_rise(k as f32 * 0.005)).fold(0.0f32, f32::max);
        assert!(peak > 1.01 && peak < 1.08, "a slight overshoot: {peak}");
        assert!((fan_rise(FAN_SETTLE_S) - 1.0).abs() < 0.02, "settled");
        // One spring for the whole fan: already moving on the first frame.
        assert!(fan_progress(0.016, None) > 0.0);
        assert!(fan_moving(0.1, None) && !fan_moving(FAN_SETTLE_S, None));
        // Closing ends at the icon and never jumps past the arc.
        let open = 5.0;
        assert_eq!(fan_progress(open, Some(FAN_CLOSE_S)), 0.0);
        assert!(!fan_moving(open, Some(FAN_CLOSE_S)));
        assert!(fan_progress(open, Some(0.0)) <= 1.0, "no jump past the arc when closing");
    }

    #[test]
    fn grids_keep_to_five_columns() {
        assert_eq!(grid_columns(0), 1);
        assert_eq!(grid_columns(3), 3);
        assert_eq!(grid_columns(24), GRID_COLUMNS);
        assert_eq!(file_icon_names(&entry("a.tar.gz", 0, 0, false))[0], "application-x-compressed-tar");
        assert_eq!(*file_icon_names(&entry("a.weird", 0, 0, false)).last().unwrap(), "text-x-generic");
    }
}
