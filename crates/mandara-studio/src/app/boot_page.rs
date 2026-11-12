//! Studio → Themes → Boot (the opt-in Boot look plugin): the boot menu and
//! the boot splash.
//!
//! * **Your boot** — what this machine boots with (found read-only:
//!   `LoaderInfo`, the firmware's boot entry, the installed configs), the
//!   splash and whether it can show, and Restore for the last change.
//! * **Boot menu** — GRUB and rEFInd theme galleries from the pinned
//!   catalog, or Limine looks in Mandara's own themes; systemd-boot has no
//!   theming and says so.
//! * **Splash** — Plymouth themes from the catalog and the ones installed
//!   here.
//! * **Use my image** — your SVG or picture as the splash, a GRUB theme
//!   and a Limine wallpaper, with a live preview of the boot screen.
//!
//! Applying always opens the plan first (every file and command), then
//! asks for administrator approval; the root side backs everything up and
//! checks with the tools' own checkers (`mandara_services::boot`).
//! Downloads, detection, decoding and applying run off the UI thread;
//! everything this page holds is dropped when you leave it.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use gpui::{AnyElement, ClickEvent, Context, FontWeight, Global, InteractiveElement, IntoElement, ParentElement, RenderImage, SharedString, StatefulInteractiveElement, Styled, StyledImage as _, anchored, deferred, div, img, point, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_core::ToGpui as _;
use mandara_i18n::{t, tf, tk, tr};
use mandara_services::boot::catalog::{self, BootTheme, Variant};
use mandara_services::boot::detect::{Detection, Firmware, Loader};
use mandara_services::boot::helper::{Outcome, Record};
use mandara_services::boot::image::{self, Options, Position, Spinner};
use mandara_services::boot::plan::{self, Approved, LimineStyle, Plan, Request, SplashChoice, Staged, Step};
use mandara_services::boot::stage::{self, FromImage};
use mandara_services::boot::theme::Kind;
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;
use mandara_ui::plugins::boot_look::{DEFAULT_SCREEN, ID};

use super::StudioView;
use super::compositor_page::{badge, button, chip, tip};
use super::plugin_gate::Gate;
use super::themes_page::section_head;
use crate::tokens::*;

/// Theme cards.
const CARD_GRID: CardGrid = CardGrid { min_w: 250.0, max_w: 360.0, gap: SPACE_16_PX };
/// Limine look cards.
const LOOK_GRID: CardGrid = CardGrid { min_w: 210.0, max_w: 300.0, gap: SPACE_14_PX };
const SCREEN_ASPECT: f32 = 9.0 / 16.0;
/// Card previews are decoded at this size (device px).
const PREVIEW_DECODE: (u32, u32) = (640, 360);
/// "Use my image" preview size (device px).
const IMAGE_PREVIEW: (u32, u32) = (960, 540);
/// GRUB / Limine menu resolutions offered.
const MENU_SIZES: [(u32, u32); 3] = [(1920, 1080), (2560, 1440), (3840, 2160)];
/// Image sizes offered (share of the screen height).
const IMAGE_SCALES: [(f32, &str); 3] = [(0.18, tk("Small")), (0.28, tk("Medium")), (0.42, tk("Large"))];
const JOB_REPAINT: Duration = Duration::from_millis(120);
const PERMILLE: f32 = 1000.0;
const PERCENT_PER_PERMILLE: u32 = 10;
const DIAGONAL_DEG: f32 = 135.0;
const HORIZONTAL_DEG: f32 = 90.0;
const PROGRESS_H_PX: f32 = DIM_5_PX;
const PROGRESS_MIN: f32 = 0.03;
/// The status card and the image card split from this column width.
const SPLIT_FROM_PX: f32 = 2.0 * DIM_380_PX;
const PREVIEW_SHARE: f32 = 0.56;
/// The plan dialog.
const DIALOG_W_PX: f32 = 620.0;
const DIALOG_MARGIN_PX: f32 = 28.0;
const SCRIM_DARK: f32 = 0.6;
const SCRIM_LIGHT: f32 = 0.4;
const DIALOG_SHADOW_Y_PX: f32 = 18.0;
const DIALOG_SHADOW_BLUR_PX: f32 = 48.0;
/// Above this many variants, a card's dots wrap on a row of their own.
const MANY_VARIANTS: usize = 6;
/// "Your boot": the pictures of the splash and menu in use.
const NOW_THUMB_W_PX: f32 = 176.0;
/// A badge over a picture never covers more than this share of it.
const BADGE_MAX_SHARE: f32 = 0.6;
/// The mock menu drawn over a theme's picture.
const MOCK_ENTRIES: usize = 3;
const MOCK_LINE_H_PX: f32 = DIM_6_PX;

/// The Boot tab while the plugin is off.
pub(super) const GATE: Gate = Gate {
    will: &[
        tk("Shows the exact plan first: every file it changes and every command it runs"),
        tk("Asks for your administrator approval each time; nothing else ever runs as root"),
        tk("Backs up every file before changing it, and Restore puts everything back in one step"),
        tk("Checks with GRUB's, Limine's and Plymouth's own tools before anything is replaced, and rolls back if a step fails"),
        tk("Downloads only pinned themes from their authors, checked against a checksum; nothing in them is ever run"),
        tk("Never touches your kernel command line, GRUB itself, shim or any signed file"),
    ],
    wont: &[tk("Run anything that comes with a theme")],
};

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
enum Sub {
    #[default]
    Menu,
    Splash,
}

/// A download in flight.
struct Job {
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct ImageFlow {
    path: Option<PathBuf>,
    opts: Options,
    /// The checked picture, shared with preview jobs.
    source: Option<Arc<image::Source>>,
    preview: Option<Arc<RenderImage>>,
    /// Bumped on every change: older preview results are dropped.
    generation: u64,
    busy: bool,
    error: Option<String>,
}

enum DialogState {
    Planning,
    Review(Result<Plan, String>),
    Running,
    Done(Outcome),
    Failed(String, Option<Plan>),
}

struct Dialog {
    title: String,
    /// `None`: a restore of `restore`.
    request: Option<Request>,
    restore: Option<Record>,
    state: DialogState,
}

#[derive(Default)]
struct BootStudio {
    det: Option<Arc<Detection>>,
    records: Vec<Record>,
    detecting: bool,
    sub: Sub,
    /// Staged theme folders `(kind, name)`.
    staged: HashSet<(Kind, String)>,
    picked: HashMap<&'static str, usize>,
    jobs: HashMap<&'static str, Job>,
    errors: HashMap<String, String>,
    previews: HashMap<(Kind, String), Arc<RenderImage>>,
    loading: HashSet<(Kind, String)>,
    /// Themes whose picture can't be read (an unreadable `/boot`, no
    /// image): not tried again until the tab is opened again.
    no_preview: HashSet<(Kind, String)>,
    image: ImageFlow,
    menu_size: usize,
    limine_image: bool,
    dialog: Option<Dialog>,
}
impl Global for BootStudio {}

fn st(cx: &mut gpui::App) -> &mut BootStudio {
    cx.default_global::<BootStudio>()
}

/// Drops everything the tab read or decoded (leaving it, or Themes).
/// Downloads keep going; their results land in the cache.
pub(super) fn release(cx: &mut gpui::App) {
    let s = st(cx);
    s.det = None;
    s.records = Vec::new();
    s.staged = HashSet::new();
    s.loading = HashSet::new();
    s.no_preview = HashSet::new();
    s.errors = HashMap::new();
    let mut images: Vec<Arc<RenderImage>> = s.previews.drain().map(|(_, i)| i).collect();
    s.previews.shrink_to_fit();
    images.extend(s.image.preview.take());
    s.image.source = None;
    s.image.generation += 1;
    if !matches!(s.dialog.as_ref().map(|d| &d.state), Some(DialogState::Running)) {
        s.dialog = None;
    }
    for i in images {
        cx.drop_image(i, None);
    }
    super::plugin_gate::release(cx);
}

fn rgb(c: Color) -> [u8; 3] {
    let [r, g, b, _] = c.to_rgba8();
    [r, g, b]
}

fn swatch(v: u32) -> Color {
    let [_, r, g, b] = v.to_be_bytes();
    Color::from_rgba8(r, g, b, u8::MAX)
}

/// What a card's action does.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum CardState {
    Get,
    Downloading(u32),
    Failed,
    Apply,
    Active,
}

/// The splash theme / GRUB theme in effect now.
fn active(det: &Detection, kind: Kind, install: &str) -> bool {
    match kind {
        Kind::Plymouth => det.plymouth.as_ref().and_then(|p| p.current.as_deref()) == Some(install),
        Kind::Grub => det.grub.as_ref().and_then(|g| g.current.theme.as_deref()).is_some_and(|t| t.contains(&format!("/{install}/"))),
        Kind::Refind => false,
    }
}

/// Reads the staged folders (in a job).
fn read_staged() -> HashSet<(Kind, String)> {
    let env = stage::Env::current();
    let mut out = HashSet::new();
    for kind in [Kind::Grub, Kind::Refind, Kind::Plymouth] {
        for e in std::fs::read_dir(env.stage(kind)).into_iter().flatten().flatten() {
            if e.path().is_dir() {
                out.insert((kind, e.file_name().to_string_lossy().into_owned()));
            }
        }
    }
    out
}

/// Where a theme's files are read from for its picture: Mandara's staged
/// copy, else where it is installed on this system.
#[derive(Clone, Debug, PartialEq)]
enum PreviewFrom {
    Staged(PathBuf),
    Installed(Arc<Detection>),
}

impl PreviewFrom {
    /// The folder (touches the disk: in a job).
    fn dir(&self, kind: Kind, name: &str) -> Option<PathBuf> {
        match self {
            PreviewFrom::Staged(dir) => Some(dir.clone()),
            PreviewFrom::Installed(det) => mandara_services::boot::theme::installed_dir(det, kind, name),
        }
    }
}

/// Where `(kind, name)`'s picture can come from, if anywhere: staged
/// themes first (what Apply installs), then the ones on this system.
fn preview_from(s: &BootStudio, kind: Kind, name: &str) -> Option<PreviewFrom> {
    if s.staged.contains(&(kind, name.to_string())) {
        return Some(PreviewFrom::Staged(stage::Env::current().stage(kind).join(name)));
    }
    let det = s.det.as_ref()?;
    mandara_services::boot::theme::on_system(det, kind, name).then(|| PreviewFrom::Installed(det.clone()))
}

/// A theme picture decoded to fill a card (in a job).
fn decode_preview(kind: Kind, dir: PathBuf) -> Option<Arc<RenderImage>> {
    let pick = mandara_services::boot::theme::preview(kind, &dir)?;
    let mut reader = ::image::ImageReader::open(&pick.file).ok()?.with_guessed_format().ok()?;
    let mut limits = ::image::Limits::default();
    limits.max_alloc = Some(256 << 20);
    reader.limits(limits);
    let picture = reader.decode().ok()?.into_rgba8();
    let src = image::Source::Raster(picture);
    let (w, h) = PREVIEW_DECODE;
    let out = match pick.screen {
        // A splash element sits on the dark screen it is drawn on.
        false => {
            let mut canvas = ::image::RgbaImage::from_pixel(w, h, ::image::Rgba([0, 0, 0, u8::MAX]));
            let frame = src.fit(w as f32 * 0.6, h as f32 * 0.6);
            ::image::imageops::overlay(&mut canvas, &frame, (i64::from(w) - i64::from(frame.width())) / 2, (i64::from(h) - i64::from(frame.height())) / 2);
            canvas
        }
        true => src.cover(w, h),
    };
    drop(src);
    Some(mandara_ui::backdrop::widgets::render_image(out))
}

impl StudioView {
    fn boot_detect(&mut self, cx: &mut Context<Self>) {
        if st(cx).detecting {
            return;
        }
        st(cx).detecting = true;
        let reload_image = st(cx).image.source.is_none();
        let task = cx.background_executor().spawn(async move {
            let det = mandara_services::boot::detect::detect_now();
            let records = mandara_services::boot::helper::records(&mandara_services::boot::Sys::real());
            // The picture used last time, ready again.
            let last = reload_image.then(|| stage::last_image(&stage::Env::current())).flatten().and_then(|p| image::load(&p).ok().map(|src| (p, src)));
            (det, records, read_staged(), last)
        });
        cx.spawn(async move |this, cx| {
            let (det, records, staged, last) = task.await;
            let _ = this.update(cx, |this, cx| {
                let s = st(cx);
                s.detecting = false;
                s.det = Some(Arc::new(det));
                s.records = records;
                s.staged = staged;
                if let Some((path, src)) = last
                    && s.image.source.is_none()
                {
                    s.image.path = Some(path);
                    s.image.source = Some(Arc::new(src));
                    this.boot_image_preview(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn boot_preview(kind: Kind, name: &str, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        let key = (kind, name.to_string());
        if let Some(p) = st(cx).previews.get(&key) {
            return Some(p.clone());
        }
        if st(cx).no_preview.contains(&key) {
            return None;
        }
        let from = preview_from(st(cx), kind, name)?;
        if !st(cx).loading.insert(key.clone()) {
            return None;
        }
        let name = name.to_string();
        let task = cx.background_executor().spawn(async move { decode_preview(kind, from.dir(kind, &name)?) });
        cx.spawn(async move |this, cx| {
            let image = task.await;
            let _ = this.update(cx, |_, cx| {
                // Left the tab meanwhile: dropped, never cached.
                if !st(cx).loading.remove(&key) {
                    if let Some(i) = image {
                        cx.drop_image(i, None);
                    }
                    return;
                }
                match image {
                    Some(i) => {
                        st(cx).previews.insert(key, i);
                    }
                    None => {
                        st(cx).no_preview.insert(key);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        None
    }

    fn boot_download(&mut self, theme: &'static BootTheme, variant: &'static Variant, cx: &mut Context<Self>) {
        let name = variant.install;
        if st(cx).jobs.contains_key(name) {
            return;
        }
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let s = st(cx);
        s.errors.remove(name);
        s.jobs.insert(name, Job { progress: progress.clone(), cancel: cancel.clone() });
        let task = cx.background_executor().spawn(async move { stage::stage_catalog(theme, variant, &stage::Env::current(), &|f| progress.store((f * PERMILLE) as u32, Ordering::Relaxed), &cancel) });
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(JOB_REPAINT).await;
            let running = this.update(cx, |_, cx| {
                cx.notify();
                st(cx).jobs.contains_key(name)
            });
            if !matches!(running, Ok(true)) {
                break;
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |_, cx| {
                let s = st(cx);
                s.jobs.remove(name);
                match result {
                    Ok(staged) => {
                        let key = (theme.kind, staged.name);
                        s.no_preview.remove(&key);
                        if let Some(old) = s.previews.remove(&key) {
                            cx.drop_image(old, None);
                        }
                        st(cx).staged.insert(key);
                    }
                    Err(e) => {
                        s.errors.insert(name.to_string(), e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Opens the plan dialog for `request` (planned off the UI thread).
    fn boot_review(&mut self, title: String, request: Request, cx: &mut Context<Self>) {
        st(cx).dialog = Some(Dialog { title, request: Some(request.clone()), restore: None, state: DialogState::Planning });
        let task = cx.background_executor().spawn(async move {
            let det = mandara_services::boot::detect::detect_now();
            plan::build(&det, &request, None)
        });
        cx.spawn(async move |this, cx| {
            let plan = task.await;
            let _ = this.update(cx, |_, cx| {
                if let Some(d) = st(cx).dialog.as_mut()
                    && matches!(d.state, DialogState::Planning)
                {
                    d.state = DialogState::Review(plan);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn boot_restore_review(&mut self, record: Record, cx: &mut Context<Self>) {
        st(cx).dialog = Some(Dialog { title: tf("Restore the setup from before {what}", &[("what", &record.what)]), request: None, restore: Some(record), state: DialogState::Review(Err(String::new())) });
        cx.notify();
    }

    /// Runs the reviewed request (or restore) with administrator approval.
    fn boot_run(&mut self, cx: &mut Context<Self>) {
        let Some(d) = st(cx).dialog.as_mut() else { return };
        let job: Box<dyn FnOnce() -> Result<Outcome, String> + Send> = match (&d.request, &d.restore, &d.state) {
            (Some(req), _, DialogState::Review(Ok(plan)) | DialogState::Failed(_, Some(plan))) => {
                let approved = Approved { request: req.clone(), digest: plan.digest() };
                Box::new(move || {
                    let env = stage::Env::current();
                    let path = stage::write_request(&env, &approved)?;
                    let out = stage::run_admin(&["apply", &path.display().to_string()]);
                    let _ = std::fs::remove_file(&path);
                    out
                })
            }
            (None, Some(record), _) => {
                let id = record.id.clone();
                Box::new(move || stage::run_admin(&["restore", &id]))
            }
            _ => return,
        };
        d.state = DialogState::Running;
        let task = cx.background_executor().spawn(async move { job() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(d) = st(cx).dialog.as_mut() {
                    d.state = match result {
                        Ok(o) if o.ok => DialogState::Done(o),
                        Ok(o) => DialogState::Failed(o.message, o.plan),
                        Err(e) => DialogState::Failed(e, None),
                    };
                }
                this.boot_detect(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Themes → Boot.
    pub(super) fn boot_tab(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut gpui::Window, cx: &mut Context<Self>) -> AnyElement {
        if !cfg.plugins.opted_in(ID) {
            return self.plugin_gate(ID, &GATE, p, window, cx);
        }
        if st(cx).det.is_none() {
            self.boot_detect(cx);
        }
        let display = mandara_ui::fonts::fonts(cx).display.clone();
        let det = st(cx).det.clone();
        let head = section_head(t("Boot look"), t("The boot menu and the splash before your desktop: themes from their authors or your own image. Every change shows its plan first, is backed up and can be restored."), Vec::new(), display.clone(), p);
        let Some(det) = det else {
            return div().row().flex_col().gap(px(SPACE_14_PX)).child(head).child(div().p(px(SPACE_16_PX)).text_size(px(TEXT_13_PX)).text_color(p.on_surface_variant.hsla()).child(t("Looking at how this machine boots…"))).into_any_element();
        };
        let status = self.boot_status(&det, p, cx);
        let sub = st(cx).sub;
        let tabs = div().row().gap(px(SPACE_8_PX)).children([(Sub::Menu, "monitor", t("Boot menu")), (Sub::Splash, "sun", t("Splash"))].map(|(s, glyph, label)| {
            let on = s == sub;
            div()
                .id(SharedString::from(format!("boot-sub-{s:?}")))
                .h(px(DIM_36_PX))
                .px(px(SPACE_16_PX))
                .rounded_full()
                .row()
                .items_center()
                .gap(px(SPACE_8_PX))
                .cursor_pointer()
                .tab_index(0)
                .text_size(px(TEXT_13_PX))
                .font_weight(FontWeight::MEDIUM)
                .when(on, |el| el.bg(mandara_core::oklab_gradient(HORIZONTAL_DEG, p.accent_gradient_start, p.accent_gradient_end)).text_color(p.on_primary.hsla()))
                .when(!on, |el| el.bg(p.surface_container.hsla()).hover(|h| h.bg(p.surface_container_high.hsla())))
                .child(icon(glyph, ICON_14_PX, if on { p.on_primary } else { p.on_surface_variant }))
                .child(label)
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    st(cx).sub = s;
                    cx.notify();
                }))
        }));
        let body = match sub {
            Sub::Menu => self.boot_menu(&det, p, cx),
            Sub::Splash => self.boot_splash(&det, p, cx),
        };
        let image = self.boot_image(&det, sub, p, cx);
        let dialog = self.boot_dialog(p, window, cx);
        div().row().flex_col().gap(px(PAGE_SECTION_GAP_PX)).child(head).child(status).child(tabs).child(image).child(body).children(dialog).into_any_element()
    }

    /// "Your boot": the bootloader, firmware, splash and Restore.
    fn boot_status(&mut self, det: &Detection, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let row = |glyph: &'static str, label: String, value: String, tone: Color| {
            div()
                .row()
                .items_center()
                .gap(px(SPACE_10_PX))
                .child(div().size(px(DIM_30_PX)).flex_none().rounded_full().bg(tone.with_alpha(ALPHA_14).hsla()).row().items_center().justify_center().child(icon(glyph, ICON_14_PX, tone)))
                .child(div().row().flex_col().min_w(px(0.0)).child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(label)).child(div().text_size(px(TEXT_13_5_PX)).font_weight(FontWeight::MEDIUM).truncate().child(value)))
        };
        let loader = match det.in_use {
            Some(l) => l.name().to_string(),
            None if !det.supported => t("Not supported on FreeBSD").to_string(),
            None => t("Not sure which one").to_string(),
        };
        let firmware = match (det.firmware, det.secure_boot) {
            (Firmware::Uefi, Some(true)) => t("UEFI · Secure Boot on").to_string(),
            (Firmware::Uefi, _) => t("UEFI").to_string(),
            (Firmware::Bios, _) => t("BIOS (legacy)").to_string(),
        };
        let splash = match &det.plymouth {
            Some(pl) => {
                let mut v = tf("Plymouth · {theme}", &[("theme", &pl.current.clone().unwrap_or_else(|| t("none").to_string()))]);
                if pl.in_initramfs == Some(false) || !pl.splash_on_cmdline {
                    v.push_str(" · ");
                    v.push_str(t("may not show"));
                }
                v
            }
            None => t("No Plymouth").to_string(),
        };
        let latest = st(cx).records.iter().find(|r| r.state == mandara_services::boot::helper::State::Applied).cloned();
        let restore = latest.map(|r| {
            let label = tf("Restore before {what}", &[("what", &r.what)]);
            button("boot-restore", "undo", label, false, p).tab_index(0).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.boot_restore_review(r.clone(), cx)))
        });
        let refresh = div()
            .id("boot-refresh")
            .size(px(DIM_34_PX))
            .rounded_full()
            .row()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_8).hsla()))
            .tooltip(tip(t("Look again"), p))
            .child(icon("refresh", ICON_14_PX, p.on_surface_variant))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.boot_detect(cx)));
        let warn = match (det.supported, det.in_use) {
            (false, _) => Some(t("FreeBSD's loader is out of scope: nothing here changes it.").to_string()),
            (true, None) => Some(tf("Mandara can't tell which bootloader starts this machine ({why}), so the menu can't be changed safely. The splash still can.", &[("why", &det.evidence)])),
            (true, Some(Loader::SystemdBoot)) => Some(t("systemd-boot has no themes or colours, so only the splash can change.").to_string()),
            _ => None,
        };
        // What shows now: the splash in effect and the menu's picture.
        let now_splash = det.plymouth.as_ref().and_then(|pl| pl.current.clone());
        let now_menu = mandara_services::boot::theme::grub_theme_in_use(det).map(|(n, _)| n.to_string());
        let thumb = |label: String, glyph: &'static str, image: Option<Arc<RenderImage>>| {
            let pic = match image {
                Some(i) => img(i).size_full().object_fit(gpui::ObjectFit::Cover).into_any_element(),
                None => div().size_full().bg(mandara_core::oklab_gradient(DIAGONAL_DEG, p.surface_dim, p.surface_container_high)).row().items_center().justify_center().child(icon(glyph, ICON_17_PX, p.on_surface_variant)).into_any_element(),
            };
            div()
                .w(px(NOW_THUMB_W_PX))
                .flex_none()
                .row()
                .flex_col()
                .gap(px(SPACE_4_PX))
                .child(div().w_full().h(px((NOW_THUMB_W_PX * SCREEN_ASPECT).floor())).rounded(px(RADIUS_12_PX)).overflow_hidden().border_1().border_color(p.specular_rim_light.with_alpha(ALPHA_12).hsla()).child(pic))
                .child(div().truncate().text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.hsla()).child(label))
        };
        let splash_thumb = now_splash.map(|n| {
            let image = Self::boot_preview(Kind::Plymouth, &n, cx);
            thumb(format!("{} · {n}", t("Splash")), "sun", image)
        });
        let menu_thumb = now_menu.map(|n| {
            let image = Self::boot_preview(Kind::Grub, &n, cx);
            thumb(format!("{} · {n}", t("Boot menu")), "monitor", image)
        });
        let thumbs = (splash_thumb.is_some() || menu_thumb.is_some()).then(|| div().row().flex_wrap().gap(px(SPACE_12_PX)).children(splash_thumb).children(menu_thumb));
        div()
            .rounded(px(RADIUS_22_PX))
            .p(px(SPACE_16_PX))
            .bg(mandara_core::oklab_gradient(DIAGONAL_DEG, p.surface_container, p.surface_container.mix(p.primary_container, ALPHA_35)))
            .border_1()
            .border_color(p.specular_rim_light.with_alpha(ALPHA_12).hsla())
            .row()
            .flex_col()
            .gap(px(SPACE_14_PX))
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_10_PX))
                    .child(div().flex_1().min_w(px(0.0)).row().flex_col().gap(px(SPACE_3_PX)).child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(t("Your boot"))).child(div().truncate().font_family(fonts.display.clone()).text_size(px(TEXT_20_PX)).font_weight(FontWeight::SEMIBOLD).child(det.os.pretty.clone())))
                    .children(restore)
                    .child(refresh),
            )
            .child(
                div()
                    .row()
                    .flex_wrap()
                    .gap(px(SPACE_20_PX))
                    .child(row("monitor", t("Boot menu").to_string(), loader, p.primary))
                    .child(row("shield", t("Firmware").to_string(), firmware, p.secondary))
                    .child(row("sun", t("Splash").to_string(), splash, p.tertiary)),
            )
            .children(thumbs)
            .child(div().text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(p.on_surface_variant.hsla()).child(tf("Found by: {evidence}", &[("evidence", &det.evidence)])))
            .children(warn.map(|w| div().row().items_start().gap(px(SPACE_8_PX)).child(icon("alert", ICON_13_PX, p.warning_amber)).child(div().flex_1().text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).text_color(p.warning_amber.hsla()).child(mandara_ui::bidi::paragraph(w)))))
            .into_any_element()
    }

    fn boot_menu(&mut self, det: &Detection, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        match det.menu_target() {
            Some(Loader::Grub) => {
                let sizes = self.boot_menu_sizes(p, cx);
                let gallery = self.boot_gallery(Kind::Grub, det, p, cx);
                div().row().flex_col().gap(px(SPACE_14_PX)).child(sizes).child(gallery).into_any_element()
            }
            Some(Loader::Refind) => self.boot_gallery(Kind::Refind, det, p, cx),
            Some(Loader::Limine) => self.boot_limine(p, cx),
            Some(Loader::SystemdBoot) | None => div().into_any_element(),
        }
    }

    fn boot_menu_sizes(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let picked = st(cx).menu_size;
        div()
            .row()
            .flex_wrap()
            .items_center()
            .gap(px(SPACE_6_PX))
            .child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::MEDIUM).text_color(p.on_surface_variant.hsla()).mr(px(SPACE_4_PX)).child(t("Menu resolution")))
            .children(MENU_SIZES.iter().enumerate().map(|(i, (w, h))| {
                chip(SharedString::from(format!("boot-size-{i}")), format!("{w} × {h}"), i == picked, p).tab_index(0).font_features(mandara_ui::fonts::tabular_nums()).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    st(cx).menu_size = i;
                    cx.notify();
                }))
            }))
            .into_any_element()
    }

    fn boot_gallery(&mut self, kind: Kind, det: &Detection, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let fit = super::grid::fit(self.page.column_w, CARD_GRID);
        // The picture's size inside a card (its padding and border off).
        let pic_w = (fit.card_w - 2.0 * (SPACE_8_PX + BORDER_2_PX)).max(0.0).floor();
        let cards: Vec<AnyElement> = catalog::of(kind).map(|t| self.boot_card(t, det, pic_w, p, cx)).collect();
        let mut out = div().row().flex_col().gap(px(SPACE_14_PX)).child(fit.render(cards));
        if kind == Kind::Plymouth
            && let Some(pl) = &det.plymouth
        {
            let others: Vec<String> = pl.installed.iter().filter(|n| catalog::by_install(n).is_none()).cloned().collect();
            if !others.is_empty() {
                let current = pl.current.clone();
                let look = super::grid::fit(self.page.column_w, LOOK_GRID);
                let look_w = (look.card_w - 2.0 * (SPACE_6_PX + BORDER_2_PX)).max(0.0).floor();
                let cards: Vec<AnyElement> = others.into_iter().map(|name| self.boot_installed_card(name, current.as_deref(), look_w, p, cx)).collect();
                out = out.child(
                    div()
                        .row()
                        .flex_col()
                        .gap(px(SPACE_10_PX))
                        .child(div().px(px(SPACE_4_PX)).text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(t("Installed on this system")))
                        .child(look.render(cards)),
                );
            }
        }
        out.into_any_element()
    }

    /// A splash installed on this system outside the catalog: its picture,
    /// its name, Active or Review & apply.
    fn boot_installed_card(&mut self, name: String, current: Option<&str>, pic_w: f32, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let on = current == Some(name.as_str());
        let preview = Self::boot_preview(Kind::Plymouth, &name, cx);
        let pic = match preview {
            Some(i) => img(i).size_full().object_fit(gpui::ObjectFit::Cover).into_any_element(),
            None => div().size_full().bg(mandara_core::oklab_gradient(DIAGONAL_DEG, p.surface_dim, p.surface_container_high)).row().items_center().justify_center().child(icon("sun", ICON_22_PX, p.on_surface_variant)).into_any_element(),
        };
        let strip = div().w_full().h(px((pic_w * SCREEN_ASPECT).floor())).flex_none().rounded(px(RADIUS_14_PX)).overflow_hidden().relative().child(pic).children(state_badge(on, false, p));
        let (label, glyph) = if on { (t("Active"), "check") } else { (t("Review & apply"), "check") };
        let n = name.clone();
        let action = button(SharedString::from(format!("boot-installed-act-{name}")), glyph, label, !on, p)
            .tab_index(0)
            .when(on, |el| el.opacity(ALPHA_75).cursor_default())
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if !on {
                    this.boot_review(n.clone(), Request::Splash { theme: SplashChoice::Installed { name: n.clone() } }, cx);
                }
            }));
        div()
            .id(SharedString::from(format!("boot-installed-{name}")))
            .flex_1()
            .min_w(px(0.0))
            .overflow_hidden()
            .p(px(SPACE_6_PX))
            .rounded(px(RADIUS_20_PX))
            .bg(p.surface_container.hsla())
            .border_2()
            .border_color(if on { p.primary.hsla() } else { gpui::transparent_black() })
            .hover(|s| s.border_color(p.primary.with_alpha(ALPHA_35).hsla()))
            .row()
            .flex_col()
            .gap(px(SPACE_8_PX))
            .child(strip)
            .child(div().px(px(SPACE_6_PX)).min_w(px(0.0)).truncate().text_start().text_size(px(TEXT_13_5_PX)).font_weight(FontWeight::SEMIBOLD).child(name))
            .child(div().mt_auto().px(px(SPACE_4_PX)).pb(px(SPACE_4_PX)).row().items_center().child(action))
            .into_any_element()
    }

    fn boot_card(&mut self, theme: &'static BootTheme, det: &Detection, pic_w: f32, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let kind = theme.kind;
        let staged = |s: &BootStudio, v: &Variant| s.staged.contains(&(kind, v.install.to_string()));
        let picked = st(cx).picked.get(theme.id).copied().or_else(|| theme.variants.iter().position(|v| active(det, kind, v.install))).unwrap_or(0).min(theme.variants.len() - 1);
        let variant: &'static Variant = &theme.variants[picked];
        let job = st(cx).jobs.get(variant.install).map(|j| j.progress.load(Ordering::Relaxed));
        let error = st(cx).errors.get(variant.install).cloned();
        let is_staged = staged(st(cx), variant);
        // A splash already on this system applies without a download.
        let on_system = mandara_services::boot::theme::on_system(det, kind, variant.install);
        let ready = is_staged || (kind == Kind::Plymouth && on_system);
        let state = match (job, error.is_some(), ready, active(det, kind, variant.install)) {
            (Some(pm), ..) => CardState::Downloading(pm),
            (None, _, _, true) => CardState::Active,
            (None, true, false, _) => CardState::Failed,
            (None, _, true, false) => CardState::Apply,
            (None, _, false, _) => CardState::Get,
        };
        let tint = swatch(variant.swatch);
        let preview = if is_staged || on_system { Self::boot_preview(kind, variant.install, cx) } else { None };
        let pic_h = (pic_w * SCREEN_ASPECT).floor();
        let pic = match preview {
            Some(i) => img(i).size_full().object_fit(gpui::ObjectFit::Cover).into_any_element(),
            None => div()
                .size_full()
                .bg(mandara_core::oklab_gradient(DIAGONAL_DEG, tint.mix(p.surface_dim, ALPHA_60), tint.mix(p.surface_dim, ALPHA_85)))
                .row()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(SPACE_6_PX))
                .child(icon(if kind == Kind::Plymouth { "sun" } else { "monitor" }, ICON_22_PX, tint.best_on(Color::from_rgba8(u8::MAX, u8::MAX, u8::MAX, u8::MAX), Color::from_rgba8(0, 0, 0, u8::MAX))))
                .child(div().text_size(px(TEXT_11_5_PX)).text_color(Color::from_rgba8(u8::MAX, u8::MAX, u8::MAX, u8::MAX).with_alpha(ALPHA_80).hsla()).child(t("Preview after download")))
                .into_any_element(),
        };
        // A boot menu card shows where the menu sits on the picture.
        let mock = (kind != Kind::Plymouth && preview_is_some(st(cx), kind, variant.install)).then(|| {
            div()
                .absolute()
                .left(gpui::relative(0.3))
                .top(gpui::relative(0.42))
                .w(gpui::relative(0.4))
                .p(px(SPACE_6_PX))
                .rounded(px(RADIUS_6_PX))
                .bg(gpui::black().opacity(ALPHA_35))
                .row()
                .flex_col()
                .gap(px(SPACE_4_PX))
                .children((0..MOCK_ENTRIES).map(|i| div().h(px(MOCK_LINE_H_PX)).w(gpui::relative(if i == 0 { 0.9 } else { 0.7 })).rounded_full().bg(if i == 0 { tint.hsla() } else { gpui::white().opacity(ALPHA_55) })))
        });
        let strip = div()
            .w_full()
            .h(px(pic_h))
            .flex_none()
            .rounded(px(RADIUS_16_PX))
            .overflow_hidden()
            .relative()
            .child(pic)
            .children(mock)
            .children(state_badge(state == CardState::Active, on_system || is_staged, p));
        let named = theme.variants.windows(2).all(|w| w[0].swatch == w[1].swatch);
        let dots = div().row().items_center().gap(px(SPACE_6_PX)).when(theme.variants.len() > 1 && named, |el| {
            el.children(theme.variants.iter().enumerate().map(|(i, v)| {
                let id = theme.id;
                chip(SharedString::from(format!("boot-var-{}-{}", theme.id, v.id)), tr(v.label), i == picked, p).font_features(mandara_ui::fonts::tabular_nums()).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    st(cx).picked.insert(id, i);
                    cx.notify();
                }))
            }))
        });
        let dots = dots.when(theme.variants.len() > 1 && !named, |el| {
            el.children(theme.variants.iter().enumerate().map(|(i, v)| {
                let id = theme.id;
                div()
                    .id(SharedString::from(format!("boot-dot-{}-{}", theme.id, v.id)))
                    .size(px(DIM_18_PX))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(swatch(v.swatch).hsla())
                    .border_2()
                    .border_color(if i == picked { p.primary.hsla() } else { p.outline.with_alpha(ALPHA_30).hsla() })
                    .tooltip(tip(tr(v.label), p))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        st(cx).picked.insert(id, i);
                        cx.notify();
                    }))
            }))
        });
        let (label, glyph, filled): (String, &'static str, bool) = match state {
            CardState::Get => (tf("Get · {size}", &[("size", &mandara_ui::widgets::system::format_bytes(variant.asset.size))]), "download", false),
            CardState::Downloading(pm) => (tf("Downloading {percent}% · Cancel", &[("percent", &(pm / PERCENT_PER_PERMILLE))]), "close", false),
            CardState::Failed => (t("Retry").to_string(), "refresh", false),
            CardState::Apply => (t("Review & apply").to_string(), "check", true),
            CardState::Active => (t("Active").to_string(), "check", false),
        };
        let menu_size = MENU_SIZES[st(cx).menu_size.min(MENU_SIZES.len() - 1)];
        let primary = button(SharedString::from(format!("boot-act-{}", variant.install)), glyph, label, filled, p).tab_index(0).when(state == CardState::Active, |el| el.opacity(ALPHA_75).cursor_default()).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| match state {
            CardState::Get | CardState::Failed => this.boot_download(theme, variant, cx),
            CardState::Downloading(_) => {
                if let Some(j) = st(cx).jobs.get(variant.install) {
                    j.cancel.store(true, Ordering::Relaxed);
                }
            }
            CardState::Apply => {
                let dir = stage::Env::current().stage(kind).join(variant.install);
                let staged_here = st(cx).staged.contains(&(kind, variant.install.to_string()));
                let staged = Staged { name: variant.install.to_string(), dir, include: variant.include.map(str::to_string) };
                let title = format!("{} · {}", theme.name, tr(variant.label));
                let request = match kind {
                    Kind::Plymouth if !staged_here => Request::Splash { theme: SplashChoice::Installed { name: variant.install.to_string() } },
                    Kind::Plymouth => Request::Splash { theme: SplashChoice::Staged(staged) },
                    Kind::Refind => Request::MenuTheme { theme: staged, gfxmode: None },
                    // grub2-themes are made for one size; others take the chosen one.
                    Kind::Grub => Request::MenuTheme { theme: staged, gfxmode: Some(match variant.id { "2k" => "2560x1440".into(), "4k" => "3840x2160".into(), "1080p" => "1920x1080".into(), _ => format!("{}x{}", menu_size.0, menu_size.1) }) },
                };
                this.boot_review(title, request, cx);
            }
            CardState::Active => {}
        }));
        let homepage = div()
            .id(SharedString::from(format!("boot-home-{}", theme.id)))
            .size(px(DIM_34_PX))
            .rounded_full()
            .flex_none()
            .row()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_8).hsla()))
            .tooltip(tip(theme.homepage, p))
            .child(icon("external", ICON_14_PX, p.on_surface_variant))
            .on_click(move |_: &ClickEvent, _, _| super::open_link(theme.homepage));
        let progress = job.map(|pm| div().h(px(PROGRESS_H_PX)).w_full().rounded_full().bg(p.surface_container_high.hsla()).child(div().h_full().w(gpui::relative((pm.min(PERMILLE as u32) as f32 / PERMILLE).max(PROGRESS_MIN))).rounded_full().bg(mandara_core::oklab_gradient(HORIZONTAL_DEG, p.accent_gradient_start, p.accent_gradient_end))));
        let error_row = error.map(|e| div().row().items_start().gap(px(SPACE_6_PX)).child(icon("alert", ICON_13_PX, p.urgent_coral)).child(div().flex_1().min_w(px(0.0)).text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(p.urgent_coral.hsla()).child(mandara_ui::bidi::paragraph(e))));
        div()
            .id(SharedString::from(format!("boot-card-{}", theme.id)))
            .flex_1()
            .min_w(px(0.0))
            .overflow_hidden()
            .p(px(SPACE_8_PX))
            .rounded(px(RADIUS_22_PX))
            .bg(p.surface_container.hsla())
            .border_2()
            .border_color(if state == CardState::Active { p.primary.hsla() } else { gpui::transparent_black() })
            .hover(|s| s.border_color(p.primary.with_alpha(ALPHA_35).hsla()))
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .child(strip)
            .child(
                div()
                    .px(px(SPACE_6_PX))
                    .min_w(px(0.0))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_6_PX))
                    .child(div().row().items_center().gap(px(SPACE_8_PX)).child(div().flex_1().min_w(px(0.0)).truncate().text_start().font_family(fonts.display.clone()).text_size(px(TEXT_15_PX)).font_weight(FontWeight::SEMIBOLD).child(theme.name)).child(badge(theme.license, p.tertiary)))
                    .map(|el| {
                        let label = (!named || theme.variants.len() == 1).then(|| div().flex_1().min_w(px(0.0)).truncate().text_start().text_size(px(TEXT_12_PX)).font_weight(FontWeight::MEDIUM).child(tr(variant.label)));
                        let author = div().flex_none().max_w(gpui::relative(0.5)).text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.hsla()).truncate().child(theme.author);
                        // Many variants (a collection) get a row of their own.
                        if theme.variants.len() > MANY_VARIANTS {
                            el.child(dots.flex_wrap()).child(div().row().items_center().gap(px(SPACE_8_PX)).children(label).when(named, |r| r.child(div().flex_1())).child(author))
                        } else {
                            el.child(div().row().flex_wrap().items_center().gap(px(SPACE_8_PX)).child(dots).children(label).when(named && theme.variants.len() > 1, |r| r.child(div().flex_1())).child(author))
                        }
                    })
                    .child(div().h(px(DIM_32_PX)).overflow_hidden().text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(p.on_surface_variant.hsla()).line_clamp(2).child(mandara_ui::bidi::paragraph(tr(theme.description))))
                    .children(progress)
                    .children(error_row),
            )
            .child(div().mt_auto().px(px(SPACE_4_PX)).pb(px(SPACE_4_PX)).row().flex_wrap().items_center().gap(px(SPACE_6_PX)).child(primary).child(div().flex_1()).child(homepage))
            .into_any_element()
    }

    /// Limine: looks in Mandara's own themes.
    fn boot_limine(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let fit = super::grid::fit(self.page.column_w, LOOK_GRID);
        let with_image = st(cx).limine_image;
        let wallpaper = stage::Env::current().wallpaper();
        let has_image = st(cx).image.source.is_some() || wallpaper.is_file();
        let toggle = chip("boot-limine-image", t("With my image as the wallpaper"), with_image && has_image, p).tab_index(0).when(!has_image, |el| el.opacity(ALPHA_45)).tooltip(tip(if has_image { t("Uses the picture made with Use my image") } else { t("Make one with Use my image first") }, p)).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
            if has_image {
                let s = st(cx);
                s.limine_image = !s.limine_image;
                cx.notify();
            }
        }));
        let cards: Vec<AnyElement> = mandara_theme::catalog::builtin()
            .iter()
            .map(|def| {
                let lp = def.palette(true);
                let id = def.id.clone();
                let name = def.name.clone();
                let line = |w: f32, c: Color| div().h(px(MOCK_LINE_H_PX)).w(gpui::relative(w)).rounded_full().bg(c.hsla());
                let mock = div()
                    .w_full()
                    .h(px(DIM_110_PX))
                    .rounded(px(RADIUS_14_PX))
                    .bg(lp.surface_dim.hsla())
                    .p(px(SPACE_10_PX))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_6_PX))
                    .child(line(0.35, lp.primary))
                    .child(line(0.55, lp.secondary.with_alpha(ALPHA_60)))
                    .child(div().mt(px(SPACE_4_PX)).p(px(SPACE_4_PX)).rounded(px(RADIUS_4_PX)).bg(lp.primary.with_alpha(ALPHA_30).hsla()).child(line(0.6, lp.on_surface)))
                    .child(line(0.5, lp.on_surface_variant))
                    .child(line(0.42, lp.on_surface_variant));
                div()
                    .id(SharedString::from(format!("boot-limine-{id}")))
                    .flex_1()
                    .p(px(SPACE_8_PX))
                    .rounded(px(RADIUS_20_PX))
                    .bg(p.surface_container.hsla())
                    .cursor_pointer()
                    .tab_index(0)
                    .border_2()
                    .border_color(gpui::transparent_black())
                    .hover(|s| s.border_color(p.primary.with_alpha(ALPHA_35).hsla()))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_8_PX))
                    .child(mock)
                    .child(div().px(px(SPACE_4_PX)).row().items_center().gap(px(SPACE_6_PX)).child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(TEXT_13_PX)).font_weight(FontWeight::MEDIUM).child(name.clone())).child(icon("chevron-right", ICON_12_PX, p.on_surface_variant)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let wp = (st(cx).limine_image && has_image).then(|| stage::Env::current().wallpaper());
                        let style = LimineStyle::from_palette(&lp, wp);
                        this.boot_review(tf("Limine in {theme}", &[("theme", &name)]), Request::LimineStyle { style }, cx);
                    }))
                    .into_any_element()
            })
            .collect();
        div().row().flex_col().gap(px(SPACE_12_PX)).child(div().row().flex_wrap().items_center().gap(px(SPACE_10_PX)).child(div().flex_1().min_w(px(DIM_240_PX)).text_size(px(TEXT_12_5_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Limine has no theme folders: it takes colours, a branding line and a wallpaper. Pick a Mandara theme to dress it in.")))).child(toggle)).child(fit.render(cards)).into_any_element()
    }

    fn boot_splash(&mut self, det: &Detection, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        if det.plymouth.is_none() {
            return div().p(px(SPACE_16_PX)).rounded(px(RADIUS_22_PX)).bg(p.surface_container.hsla()).text_size(px(TEXT_13_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Plymouth, the boot splash, isn't installed. Install it with your package manager to choose a splash here."))).into_any_element();
        }
        self.boot_gallery(Kind::Plymouth, det, p, cx)
    }

    /// Re-renders the "Use my image" preview off the UI thread.
    fn boot_image_preview(&mut self, cx: &mut Context<Self>) {
        let s = st(cx);
        s.image.generation += 1;
        let generation = s.image.generation;
        let Some(src) = s.image.source.clone() else { return };
        let opts = s.image.opts.clone();
        let task = cx.background_executor().spawn(async move { mandara_ui::backdrop::widgets::render_image(image::preview(&src, &opts, IMAGE_PREVIEW)) });
        cx.spawn(async move |this, cx| {
            let preview = task.await;
            let _ = this.update(cx, |_, cx| {
                let s = st(cx);
                if s.image.generation != generation || s.image.source.is_none() {
                    cx.drop_image(preview, None);
                    return;
                }
                if let Some(old) = s.image.preview.replace(preview) {
                    cx.drop_image(old, None);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn boot_pick_image(&mut self, cx: &mut Context<Self>) {
        st(cx).image.busy = true;
        cx.notify();
        let title = t("Choose an image for the boot screen");
        let filter = t("Images");
        let task = cx.background_executor().spawn(async move {
            let filters = [mandara_services::file_chooser::Filter { name: filter, mime_types: &["image/svg+xml", "image/png", "image/jpeg", "image/webp"] }];
            match mandara_services::file_chooser::open_file(title, &filters, dirs::picture_dir()) {
                Ok(Some(path)) => image::load(&path).map(|src| Some((path, src))),
                Ok(None) => Ok(None),
                Err(e) => Err(e),
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let s = st(cx);
                s.image.busy = false;
                match result {
                    Ok(Some((path, src))) => {
                        stage::remember_image(&stage::Env::current(), &path);
                        s.image.path = Some(path);
                        s.image.source = Some(Arc::new(src));
                        s.image.error = None;
                        this.boot_image_preview(cx);
                    }
                    Ok(None) => {}
                    Err(e) => s.image.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Makes the splash / GRUB theme / wallpaper from the image, then opens
    /// the plan for `target`.
    fn boot_use_image(&mut self, target: Kind, det: &Detection, p: &MandaraPalette, cx: &mut Context<Self>) {
        let Some(path) = st(cx).image.path.clone() else { return };
        let opts = st(cx).image.opts.clone();
        let screen = det.screen.unwrap_or(DEFAULT_SCREEN);
        let menu = MENU_SIZES[st(cx).menu_size.min(MENU_SIZES.len() - 1)];
        let limine = det.menu_target() == Some(Loader::Limine);
        let palette = p.clone();
        st(cx).image.busy = true;
        let task = cx.background_executor().spawn(async move { stage::from_image(&path, &opts, screen, menu, &stage::Env::current()) });
        cx.spawn(async move |this, cx| {
            let made: Result<FromImage, String> = task.await;
            let _ = this.update(cx, |this, cx| {
                st(cx).image.busy = false;
                match made {
                    Ok(made) => {
                        let (title, request) = match (target, limine) {
                            (Kind::Plymouth, _) => (t("Your image as the splash").to_string(), Request::Splash { theme: SplashChoice::Staged(made.splash) }),
                            (_, true) => (t("Your image in the Limine menu").to_string(), Request::LimineStyle { style: LimineStyle::from_palette(&palette, Some(made.wallpaper)) }),
                            _ => (t("Your image as the GRUB theme").to_string(), Request::MenuTheme { theme: made.grub, gfxmode: Some(format!("{}x{}", menu.0, menu.1)) }),
                        };
                        st(cx).staged.insert((Kind::Grub, mandara_services::boot::GRUB_IMAGE_NAME.into()));
                        this.boot_review(title, request, cx);
                    }
                    Err(e) => st(cx).image.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// "Use my image": choose, tune with a live preview, use.
    fn boot_image(&mut self, det: &Detection, sub: Sub, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let (has, busy, error, opts) = {
            let s = &st(cx).image;
            (s.source.is_some(), s.busy, s.error.clone(), s.opts.clone())
        };
        let preview = st(cx).image.preview.clone();
        let split = self.page.column_w >= SPLIT_FROM_PX && !self.stack_rows;
        let pw = if split { (self.page.column_w * PREVIEW_SHARE).floor() } else { self.page.column_w - 2.0 * SPACE_16_PX };
        let screen = div()
            .w(px(pw))
            .h(px((pw * SCREEN_ASPECT).floor()))
            .flex_none()
            .rounded(px(RADIUS_16_PX))
            .overflow_hidden()
            .border_1()
            .border_color(p.outline.with_alpha(ALPHA_20).hsla())
            .bg(Color::from_rgba8(opts.background[0], opts.background[1], opts.background[2], u8::MAX).hsla())
            .child(match preview {
                Some(i) => img(i).w(px(pw)).h(px((pw * SCREEN_ASPECT).floor())).object_fit(gpui::ObjectFit::Cover).into_any_element(),
                None => div().size_full().row().flex_col().items_center().justify_center().gap(px(SPACE_8_PX)).child(icon("image", ICON_26_PX, p.on_surface_variant)).child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("SVG, PNG, JPEG or WebP"))).into_any_element(),
            });
        let backgrounds = [p.surface_dim, p.surface_base, p.primary_container, Color::from_rgba8(0, 0, 0, u8::MAX), Color::from_rgba8(u8::MAX, u8::MAX, u8::MAX, u8::MAX)];
        let swatches = div().row().gap(px(SPACE_6_PX)).children(backgrounds.into_iter().enumerate().map(|(i, c)| {
            let on = rgb(c) == opts.background && opts.fill != Some(true);
            div()
                .id(SharedString::from(format!("boot-bg-{i}")))
                .size(px(DIM_26_PX))
                .rounded_full()
                .cursor_pointer()
                .bg(c.hsla())
                .border_2()
                .border_color(if on { p.primary.hsla() } else { p.outline.with_alpha(ALPHA_30).hsla() })
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let o = &mut st(cx).image.opts;
                    o.background = rgb(c);
                    o.fill = None;
                    this.boot_image_preview(cx);
                    cx.notify();
                }))
        }));
        let label = |text: String| div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::MEDIUM).text_color(p.on_surface_variant.hsla()).child(text);
        let spinners = div().row().flex_wrap().gap(px(SPACE_6_PX)).children(Spinner::ALL.map(|s| {
            let name = match s {
                Spinner::Ring => t("Ring"),
                Spinner::Dots => t("Dots"),
                Spinner::None => t("None"),
            };
            chip(SharedString::from(format!("boot-spin-{}", s.slug())), name, opts.spinner == s, p).tab_index(0).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                st(cx).image.opts.spinner = s;
                this.boot_image_preview(cx);
                cx.notify();
            }))
        }));
        let sizes = div().row().flex_wrap().gap(px(SPACE_6_PX)).children(IMAGE_SCALES.iter().map(|(scale, name)| {
            let scale = *scale;
            chip(SharedString::from(format!("boot-scale-{name}")), tr(name), (opts.scale - scale).abs() < 0.01, p).tab_index(0).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                st(cx).image.opts.scale = scale;
                this.boot_image_preview(cx);
                cx.notify();
            }))
        }));
        let positions = div().row().flex_wrap().gap(px(SPACE_6_PX)).children([(Position::Center, t("Centre")), (Position::Upper, t("Upper third"))].map(|(pos, name)| {
            chip(SharedString::from(format!("boot-pos-{pos:?}")), name, opts.position == pos, p).tab_index(0).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                st(cx).image.opts.position = pos;
                this.boot_image_preview(cx);
                cx.notify();
            }))
        }));
        let fill = chip("boot-fill", t("Fill the screen"), opts.fill == Some(true), p).tab_index(0).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            let o = &mut st(cx).image.opts;
            o.fill = if o.fill == Some(true) { None } else { Some(true) };
            this.boot_image_preview(cx);
            cx.notify();
        }));
        let pick = button("boot-pick", "image", if has { t("Choose another…") } else { t("Choose an image…") }, !has, p).tab_index(0).when(busy, |el| el.opacity(ALPHA_60)).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
            if !st(cx).image.busy {
                this.boot_pick_image(cx);
            }
        }));
        let det_c = det.clone();
        let palette = p.clone();
        let menu_ok = matches!(det.menu_target(), Some(Loader::Grub | Loader::Limine));
        let use_here = match sub {
            Sub::Splash => det.plymouth.is_some().then_some((t("Use as the splash"), Kind::Plymouth)),
            Sub::Menu => menu_ok.then_some((t("Use in the boot menu"), Kind::Grub)),
        };
        let apply = use_here.filter(|_| has).map(|(text, target)| {
            button("boot-use-image", "check", text, true, p).tab_index(0).when(busy, |el| el.opacity(ALPHA_60)).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if !st(cx).image.busy {
                    this.boot_use_image(target, &det_c, &palette, cx);
                }
            }))
        });
        let file_name = st(cx).image.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
        let controls = div()
            .flex_1()
            .min_w(px(0.0))
            .row()
            .flex_col()
            .gap(px(SPACE_12_PX))
            .child(div().row().flex_col().gap(px(SPACE_3_PX)).child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(t("Use my image"))).child(div().font_family(fonts.display.clone()).text_size(px(TEXT_18_PX)).font_weight(FontWeight::SEMIBOLD).truncate().child(file_name.unwrap_or_else(|| t("Your logo or picture at boot").to_string()))))
            .child(div().text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Made into a splash with a proper password prompt, a GRUB theme and a Limine wallpaper, drawn sharp at your screen's size. SVGs that load files or run scripts are refused."))))
            .child(div().row().flex_col().gap(px(SPACE_6_PX)).child(label(t("Background").to_string())).child(div().row().flex_wrap().items_center().gap(px(SPACE_10_PX)).child(swatches).child(fill)))
            .child(div().row().flex_col().gap(px(SPACE_6_PX)).child(label(t("Spinner").to_string())).child(spinners))
            .child(div().row().flex_col().gap(px(SPACE_6_PX)).child(label(t("Size").to_string())).child(sizes))
            .child(div().row().flex_col().gap(px(SPACE_6_PX)).child(label(t("Position").to_string())).child(positions))
            .children(error.map(|e| div().text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(p.urgent_coral.hsla()).child(mandara_ui::bidi::paragraph(e))))
            .child(div().row().flex_wrap().items_center().gap(px(SPACE_8_PX)).child(pick).children(apply));
        div()
            .rounded(px(RADIUS_22_PX))
            .p(px(SPACE_16_PX))
            .bg(p.surface_container.hsla())
            .row()
            .when(!split, |el| el.flex_col())
            .gap(px(SPACE_18_PX))
            .child(screen)
            .child(controls)
            .into_any_element()
    }

    /// The plan dialog: review, approve, progress, result.
    fn boot_dialog(&mut self, p: &MandaraPalette, window: &mut gpui::Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let fonts = mandara_ui::fonts::fonts(cx);
        let d = st(cx).dialog.as_ref()?;
        let viewport = window.viewport_size();
        let w = DIALOG_W_PX.min(f32::from(viewport.width) - 2.0 * DIALOG_MARGIN_PX).max(DIM_240_PX);
        let h = (f32::from(viewport.height) - 2.0 * DIALOG_MARGIN_PX).max(0.0);
        let step_row = |s: &Step| {
            let glyph = match s {
                Step::GrubPreflight { .. } => "eye",
                Step::Backup { .. } => "copy",
                Step::InstallTheme { .. } | Step::CopyFile { .. } => "download",
                Step::MakeFont { .. } => "text-scan",
                Step::GrubKeys { .. } | Step::GrubDropIn { .. } | Step::LimineOptions { .. } | Step::RefindInclude { .. } => "pencil",
                Step::GrubConfig { .. } | Step::Initramfs { .. } | Step::LimineEnroll { .. } => "refresh",
                Step::PlymouthDefault { .. } => "sun",
            };
            let tone = if s.read_only() { p.on_surface_variant } else { p.primary };
            div()
                .w_full()
                .row()
                .items_start()
                .gap(px(SPACE_10_PX))
                .py(px(SPACE_6_PX))
                .child(div().size(px(DIM_28_PX)).flex_none().rounded_full().bg(tone.with_alpha(ALPHA_14).hsla()).row().items_center().justify_center().child(icon(glyph, ICON_13_PX, tone)))
                .child(div().flex_1().min_w(px(0.0)).row().flex_col().gap(px(SPACE_3_PX)).child(div().text_size(px(TEXT_13_PX)).font_weight(FontWeight::MEDIUM).child(tr(s.label()))).child(div().text_size(px(TEXT_11_5_PX)).font_family(fonts.mono.clone()).text_color(p.on_surface_variant.hsla()).child(s.detail())))
                .into_any_element()
        };
        let note = |text: String, tone: Color| div().w_full().pt(px(SPACE_4_PX)).row().items_start().gap(px(SPACE_8_PX)).child(icon("info", ICON_13_PX, tone)).child(div().flex_1().min_w(px(0.0)).text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).text_color(tone.hsla()).child(mandara_ui::bidi::paragraph(text))).into_any_element();
        let mut body: Vec<AnyElement> = Vec::new();
        let mut can_apply = false;
        match (&d.state, &d.restore) {
            (DialogState::Planning, _) => body.push(div().py(px(SPACE_16_PX)).text_size(px(TEXT_13_PX)).text_color(p.on_surface_variant.hsla()).child(t("Working out the plan with your system…")).into_any_element()),
            (DialogState::Review(_), Some(r)) => {
                can_apply = true;
                body.push(note(t("These files go back to how they were before (files you edited since keep your edits; only Mandara's lines are reverted), and the menu or initramfs is rebuilt.").to_string(), p.on_surface_variant));
                body.extend(r.files.iter().map(|f| step_row(&Step::Backup { path: f.path.clone() })));
                body.extend(r.regenerate.iter().map(step_row));
            }
            (DialogState::Review(Ok(plan)), None) => {
                can_apply = true;
                body.extend(plan.steps.iter().map(step_row));
                body.extend(plan.notes.iter().map(|n| note(tr(n.text()).to_string(), p.warning_amber)));
                body.push(note(t("You'll be asked for an administrator password. Every file is backed up first, and Restore on this page puts it back.").to_string(), p.on_surface_variant));
            }
            (DialogState::Review(Err(e)), None) => body.push(note(e.clone(), p.urgent_coral)),
            (DialogState::Running, _) => body.push(div().py(px(SPACE_16_PX)).row().items_center().gap(px(SPACE_10_PX)).child(icon("refresh", ICON_16_PX, p.primary)).child(div().text_size(px(TEXT_13_PX)).child(t("Waiting for approval, then applying… Rebuilding an initramfs can take a minute."))).into_any_element()),
            (DialogState::Done(o), _) => {
                body.push(note(o.message.clone(), p.success_emerald));
                if let Some(id) = &o.backup {
                    body.push(div().text_size(px(TEXT_11_5_PX)).font_family(fonts.mono.clone()).text_color(p.on_surface_variant.hsla()).child(tf("Backup: {id}", &[("id", id)])).into_any_element());
                }
            }
            (DialogState::Failed(msg, plan), _) => {
                body.push(note(msg.clone(), p.urgent_coral));
                if let Some(plan) = plan {
                    can_apply = true;
                    body.push(div().pt(px(SPACE_8_PX)).text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).child(t("The plan with administrator rights")).into_any_element());
                    body.extend(plan.steps.iter().map(step_row));
                }
            }
        }
        let title = d.title.clone();
        let running = matches!(d.state, DialogState::Running);
        let done = matches!(d.state, DialogState::Done(_));
        let is_restore = d.restore.is_some();
        let close = button("boot-dialog-close", "close", if done { t("Close") } else { t("Cancel") }, false, p).tab_index(0).when(running, |el| el.opacity(ALPHA_45)).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
            if !running {
                st(cx).dialog = None;
                cx.notify();
            }
        }));
        let approve = (can_apply && !running && !done).then(|| button("boot-dialog-apply", if is_restore { "undo" } else { "check" }, if is_restore { t("Approve & restore") } else { t("Approve & apply") }, true, p).tab_index(0).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.boot_run(cx))));
        let (scroll, bar) = mandara_ui::scrollbar::keyed("boot-dialog-body".to_string());
        let body = div().id("boot-dialog-body").flex_1().min_h(px(0.0)).overflow_y_scroll().track_scroll(&scroll).px(px(SPACE_22_PX)).pb(px(SPACE_8_PX)).row().flex_col().gap(px(SPACE_4_PX)).children(body);
        let body = mandara_ui::scrollbar::with_scrollbar(body, &scroll, &bar, p).flex_1().min_h(px(0.0)).flex().flex_col();
        Some(
            deferred(
                anchored().position(point(px(0.0), px(0.0))).child(
                    div()
                        .id("boot-dialog-scrim")
                        .occlude()
                        .w(viewport.width)
                        .h(viewport.height)
                        .bg(gpui::black().opacity(if p.is_dark { SCRIM_DARK } else { SCRIM_LIGHT }))
                        .row()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .id("boot-dialog")
                                .w(px(w))
                                .max_h(px(h))
                                .rounded(px(RADIUS_26_PX))
                                .bg(p.surface_container_low.hsla())
                                .border_1()
                                .border_color(p.specular_rim_light.with_alpha(ALPHA_14).hsla())
                                .shadow(vec![gpui::BoxShadow { color: gpui::black().opacity(ALPHA_35), offset: point(px(0.0), px(DIALOG_SHADOW_Y_PX)), blur_radius: px(DIALOG_SHADOW_BLUR_PX), spread_radius: px(0.0), inset: false }])
                                .row()
                                .flex_col()
                                .text_color(p.on_surface.hsla())
                                .child(div().row().flex_col().gap(px(SPACE_3_PX)).px(px(SPACE_22_PX)).pt(px(SPACE_18_PX)).pb(px(SPACE_10_PX)).child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(if is_restore { t("Restore") } else { t("Review the plan") })).child(div().font_family(fonts.display.clone()).text_size(px(TEXT_18_PX)).font_weight(FontWeight::SEMIBOLD).child(title)))
                                .child(body)
                                .child(div().row().flex_wrap().items_center().justify_end().gap(px(SPACE_10_PX)).px(px(SPACE_22_PX)).py(px(SPACE_16_PX)).child(close).children(approve)),
                        ),
                ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}

/// The pill over a picture's corner: Active, else Installed (on this
/// system or downloaded), else nothing.
fn state_badge(active: bool, installed: bool, p: &MandaraPalette) -> Option<gpui::Div> {
    if !active && !installed {
        return None;
    }
    let (bg, fg, label) = if active { (p.primary, p.on_primary, t("Active")) } else { (p.surface_container_high.with_alpha(ALPHA_88), p.on_surface, t("Installed")) };
    Some(div()
        .absolute()
        .top(px(SPACE_8_PX))
        .inset_end(px(SPACE_8_PX))
        .max_w(gpui::relative(BADGE_MAX_SHARE))
        .overflow_hidden()
        .px(px(SPACE_8_PX))
        .h(px(DIM_20_PX))
        .rounded_full()
        .row()
        .items_center()
        .gap(px(SPACE_4_PX))
        .bg(bg.hsla())
        .text_color(fg.hsla())
        .text_size(px(TEXT_10_5_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .when(active, |el| el.child(icon("check", ICON_11_PX, fg)))
        .child(div().truncate().child(label)))
}

fn preview_is_some(s: &BootStudio, kind: Kind, name: &str) -> bool {
    s.previews.contains_key(&(kind, name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gate_says_what_it_will_and_wont_do() {
        assert!(GATE.will.len() >= 4);
        assert!(GATE.will.iter().any(|t| t.contains("kernel command line")));
    }

    #[test]
    fn grub_card_states_follow_what_is_installed() {
        let mut det = Detection::unsupported();
        assert!(!active(&det, Kind::Grub, "mandara-vimix-2k"));
        det.plymouth = None;
        assert!(!active(&det, Kind::Plymouth, "rings"));
        assert!(IMAGE_SCALES.iter().all(|(s, _)| (image::SCALE_RANGE.0..=image::SCALE_RANGE.1).contains(s)));
        assert!(MENU_SIZES.iter().all(|(w, h)| plan::resolution_ok(&format!("{w}x{h}"))));
    }

    /// The splash in effect is previewed from where it is installed when
    /// Mandara has no staged copy (it used to show nothing).
    #[test]
    fn installed_splashes_have_a_preview_source() {
        let mut s = BootStudio::default();
        assert_eq!(preview_from(&s, Kind::Plymouth, "wolf"), None, "nothing detected yet");
        let mut det = Detection::unsupported();
        det.plymouth = Some(mandara_services::boot::detect::Plymouth {
            themes_dir: PathBuf::from("/usr/share/plymouth/themes"),
            set_default: PathBuf::new(),
            current: Some("wolf".into()),
            installed: vec!["wolf".into(), "spinner".into()],
            modules: Vec::new(),
            initramfs: None,
            in_initramfs: None,
            splash_on_cmdline: true,
        });
        s.det = Some(Arc::new(det));
        assert!(matches!(preview_from(&s, Kind::Plymouth, "wolf"), Some(PreviewFrom::Installed(_))));
        assert_eq!(preview_from(&s, Kind::Plymouth, "rings"), None, "neither staged nor installed");
        s.staged.insert((Kind::Plymouth, "wolf".into()));
        assert!(matches!(preview_from(&s, Kind::Plymouth, "wolf"), Some(PreviewFrom::Staged(_))), "the staged copy wins");
    }

    #[test]
    fn late_previews_never_refill_a_released_tab() {
        let mut s = BootStudio::default();
        let key = (Kind::Grub, "mandara-x".to_string());
        s.loading.insert(key.clone());
        s.loading.clear();
        assert!(!s.loading.remove(&key), "a result landing after release is dropped");
        assert!(!preview_is_some(&s, Kind::Grub, "mandara-x"));
    }
}
