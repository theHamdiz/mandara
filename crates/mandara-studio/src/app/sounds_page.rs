//! Studio → Themes → Sounds: the sound theme gallery, Mandara's own event
//! sounds and how they play (the opt-in `sound-themes` plugin).
//!
//! * **Off** — the tab still shows: the plugin's cover, one line on what it
//!   does, what it touches, and Enable. Nothing is read from disk.
//! * **Your sounds** — the theme playing now, the shell-sound settings
//!   (on, volume, quiet in do not disturb or under media, apply to apps)
//!   and every event with its switch, the file this theme plays for it and
//!   a test button.
//! * **Gallery** — the catalog (`mandara_services::sound_themes::catalog`)
//!   and the other installed themes as cards: a level outline that plays
//!   along with the preview, three signature sounds to try, Get →
//!   Installing (progress, cancel) → Apply → Active, Remove for what
//!   Mandara installed. Search and a filter narrow it.
//! * **Your own** — install a folder, an archive or a pasted link (links
//!   aren't pinned: Install anyway asks first).
//!
//! Previews play from Studio's own process (`sounds::preview_theme`, the
//! same bounded worker the shell uses, gone 30 s after the last sound).
//! Downloads, unpacking and lookups run off the UI thread; animation frames
//! are asked for only while a preview plays, and everything read from disk
//! goes when you leave the tab.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use gpui::{AnyElement, AppContext as _, ClickEvent, Context, Entity, FontWeight, Global, InteractiveElement, IntoElement, ParentElement, RenderImage, SharedString, StatefulInteractiveElement, Styled, StyledImage as _, div, img, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tr};
use mandara_services::sound_themes::catalog::{CATALOG, SIGNATURE, SoundTheme};
use mandara_services::sound_themes::{self, ThemeInfo, install};
use mandara_services::sounds::{self, PEAKS, Played, SoundEvent};
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;
use mandara_ui::plugins::lifecycle::Action;
use mandara_ui::plugins::sound_themes::ID;
use mandara_ui::state::update_config;
use mandara_ui::text_field::TextField;

use super::compositor_page::{badge, button, chip, tip};
use super::plugin_modal::ModalView;
use super::themes_page::section_head;
use super::{StudioView, grid};
use crate::registry;
use crate::tokens::*;

/// Theme cards.
const SOUND_GRID: CardGrid = CardGrid { min_w: 260.0, max_w: 400.0, gap: SPACE_16_PX };
/// A card's level outline.
const WAVE_H_PX: f32 = DIM_60_PX;
const WAVE_BAR_W_PX: f32 = 4.0;
/// The least room between two bars (they thin before they touch).
const WAVE_BAR_GAP_PX: f32 = 1.0;
/// A badge over a card's outline never covers more than this share of it.
const BADGE_MAX_SHARE: f32 = 0.6;
/// Bars never shrink below this share (silence still reads as a line).
const WAVE_FLOOR: f32 = 0.08;
/// How much a theme's colour tints its outline's backdrop.
const WAVE_TINT: f32 = 0.18;
const DIAGONAL_DEG: f32 = 135.0;
const HORIZONTAL_DEG: f32 = 90.0;
/// "Your sounds" and the event list sit side by side from this width.
const SPLIT_MIN_W_PX: f32 = 2.0 * DIM_420_PX;
const PANEL_SHARE: f32 = 0.42;
/// An event row's icon tile.
const EVENT_TILE_PX: f32 = DIM_32_PX;
/// The install progress bar.
const PROGRESS_H_PX: f32 = DIM_5_PX;
const PROGRESS_MIN: f32 = 0.03;
/// A finished preview's outline lingers this long.
const PREVIEW_TAIL: Duration = Duration::from_millis(400);
/// Previews stay audible with the volume slider low.
const PREVIEW_MIN_VOLUME: f32 = 0.3;
const PERMILLE: f32 = 1000.0;
const PERCENT_PER_PERMILLE: u32 = 10;
/// The gate's cover, decoded at this size once (16:9).
const GATE_COVER: (u32, u32) = (960, 540);
const GATE_COVER_MAX_W_PX: f32 = 560.0;
const GATE_COVER_ASPECT: f32 = 9.0 / 16.0;
/// Archives the file picker offers.
const ARCHIVE_GLOBS: [&str; 7] = ["*.tar.gz", "*.tgz", "*.tar.xz", "*.txz", "*.tar.bz2", "*.tar.zst", "*.zip"];
const ARCHIVE_MIME: [&str; 5] = ["application/x-compressed-tar", "application/x-xz-compressed-tar", "application/x-bzip-compressed-tar", "application/zstd", "application/zip"];

/// An install in flight.
struct Job {
    /// Permille done.
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
}

/// What the gallery shows.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
enum Filter {
    #[default]
    All,
    Installed,
    Get,
}

/// A preview playing: whose, which event, and its outline.
struct Preview {
    theme: String,
    event: SoundEvent,
    started: Instant,
    played: Played,
}

#[derive(Default)]
struct SoundStudio {
    /// Installed themes (every sound folder), read once per visit.
    installed: Option<Rc<[ThemeInfo]>>,
    /// The ones Mandara installed (and may remove).
    ours: Option<Rc<[String]>>,
    /// The manifest's and the user sounds folder's modification times when
    /// the lists were read (a `mandara msg sound install` shows up).
    stamp: (Option<std::time::SystemTime>, Option<std::time::SystemTime>),
    jobs: HashMap<String, Job>,
    /// The last failure per theme, shown on its card with Retry.
    errors: HashMap<String, String>,
    /// The file each configurable event plays, for one theme.
    coverage: Option<(String, Rc<[Option<PathBuf>]>)>,
    coverage_loading: Option<String>,
    preview: Option<Preview>,
    /// A preview asked for and not started yet.
    waiting: Option<(String, SoundEvent)>,
    confirm_remove: Option<String>,
    query: String,
    filter: Filter,
    search: Option<Entity<TextField>>,
    url: Option<Entity<TextField>>,
    /// A pasted link waiting for "Install anyway".
    confirm_url: Option<String>,
    import: Option<Job>,
    import_note: Option<Result<String, String>>,
    cover: Option<Arc<RenderImage>>,
    cover_loading: bool,
}
impl Global for SoundStudio {}

fn st(cx: &mut gpui::App) -> &mut SoundStudio {
    cx.default_global::<SoundStudio>()
}

/// Drops what the tab read from disk and its fields (on leaving it).
/// Running installs keep going.
pub(super) fn release(cx: &mut gpui::App) {
    let s = st(cx);
    s.installed = None;
    s.ours = None;
    s.coverage = None;
    s.coverage_loading = None;
    s.preview = None;
    s.waiting = None;
    s.confirm_remove = None;
    s.confirm_url = None;
    s.search = None;
    s.url = None;
    s.query.clear();
    let cover = s.cover.take();
    s.cover_loading = false;
    if let Some(cover) = cover {
        cx.drop_image(cover, None);
    }
}

fn swatch(rgb: u32) -> Color {
    let [_, r, g, b] = rgb.to_be_bytes();
    Color::from_rgba8(r, g, b, u8::MAX)
}

/// A gentle outline for a theme nobody has played yet (the same for the
/// same name, so cards don't shimmer between frames).
fn idle_peaks(seed: &str) -> [u8; PEAKS] {
    let h = seed.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    let phase = (h % 628) as f32 / 100.0;
    let mut out = [0u8; PEAKS];
    for (i, v) in out.iter_mut().enumerate() {
        let x = i as f32;
        let level = 0.42 + 0.3 * (x * 0.55 + phase).sin() * (x * 0.17 + phase * 0.5).cos();
        *v = (level.clamp(0.0, 1.0) * f32::from(u8::MAX)) as u8;
    }
    out
}

/// The short licence shown on a card ("CC-BY-SA-4.0", "GPL-2.0-or-later +").
fn short_license(spdx: &str) -> String {
    match spdx.split_once(" AND ") {
        Some((first, _)) => format!("{first} +"),
        None => spdx.to_string(),
    }
}

/// An event's icon in the list.
fn event_icon(e: &SoundEvent) -> &'static str {
    match e {
        SoundEvent::Notification | SoundEvent::NotificationLow | SoundEvent::Named(_) => "message",
        SoundEvent::NotificationCritical => "alert",
        SoundEvent::VolumeChange => "volume-high",
        SoundEvent::BrightnessChange => "sun",
        SoundEvent::Screenshot => "camera",
        SoundEvent::PowerPlug | SoundEvent::PowerUnplug => "plug",
        SoundEvent::BatteryLow | SoundEvent::BatteryCritical => "battery",
        SoundEvent::DeviceAdded | SoundEvent::DeviceRemoved => "usb",
        SoundEvent::TrashEmpty => "trash",
        SoundEvent::Complete => "check",
        SoundEvent::Login => "user",
        SoundEvent::Lock | SoundEvent::Unlock => "lock",
    }
}

/// A card's primary action.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum CardState {
    Get,
    Installing(u32),
    Failed,
    Apply,
    Active,
}

/// What a card shows: a catalog theme (installed or not) or another
/// installed one.
#[derive(Clone)]
enum Card {
    Catalog(&'static SoundTheme),
    Installed(ThemeInfo),
}

impl Card {
    fn theme(&self) -> &str {
        match self {
            Card::Catalog(t) => t.theme,
            Card::Installed(i) => &i.name,
        }
    }

    fn matches(&self, q: &str) -> bool {
        if q.is_empty() {
            return true;
        }
        let hit = |s: &str| s.to_lowercase().contains(q);
        match self {
            Card::Catalog(t) => hit(t.name) || hit(t.theme) || hit(t.author) || hit(&tr(t.description)) || hit(&tr(t.origin)) || hit(t.license),
            Card::Installed(i) => hit(&i.name) || hit(&i.title) || hit(&i.comment),
        }
    }
}

fn apply_theme(theme: Option<String>, cx: &mut gpui::App) {
    let next = theme.unwrap_or_default();
    update_config(cx, move |c| c.sounds.theme = next);
}

impl StudioView {
    fn sound_lists(cx: &mut Context<Self>) -> (Rc<[ThemeInfo]>, Rc<[String]>) {
        let env = install::Env::current();
        let mtime = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        let stamp = (mtime(&env.manifest), mtime(&env.sounds));
        let s = st(cx);
        if s.installed.is_none() || s.ours.is_none() || s.stamp != stamp {
            s.stamp = stamp;
            s.installed = Some(sound_themes::installed().into());
            s.ours = Some(install::installed(&env).into_iter().map(|i| i.theme).collect());
        }
        (s.installed.clone().unwrap_or_else(|| Rc::from([])), s.ours.clone().unwrap_or_else(|| Rc::from([])))
    }

    fn sound_refresh(cx: &mut gpui::App) {
        let s = st(cx);
        s.installed = None;
        s.ours = None;
        s.coverage = None;
    }

    /// The files `theme` plays for each configurable event, looked up off
    /// the UI thread on first use (`None` until ready).
    fn sound_coverage(theme: &str, cx: &mut Context<Self>) -> Option<Rc<[Option<PathBuf>]>> {
        if let Some((t, files)) = &st(cx).coverage
            && t == theme
        {
            return Some(files.clone());
        }
        if st(cx).coverage_loading.as_deref() == Some(theme) {
            return None;
        }
        st(cx).coverage_loading = Some(theme.to_string());
        let name = theme.to_string();
        let task = cx.background_executor().spawn(async move { sound_themes::coverage(&sounds::sound_roots(), &name, &SoundEvent::CONFIGURABLE) });
        let name = theme.to_string();
        cx.spawn(async move |this, cx| {
            let files = task.await;
            let _ = this.update(cx, |_, cx| {
                let s = st(cx);
                // Left meanwhile, or another theme asked for: dropped.
                if s.coverage_loading.as_deref() == Some(&name) {
                    s.coverage_loading = None;
                    s.coverage = Some((name, Rc::from(files)));
                    cx.notify();
                }
            });
        })
        .detach();
        None
    }

    /// Plays `event` from `theme` and animates its outline while it plays.
    fn sound_preview(&mut self, theme: &str, event: SoundEvent, cx: &mut Context<Self>) {
        let volume = mandara_ui::state::config(cx).read(cx).get().sounds.volume.max(PREVIEW_MIN_VOLUME);
        st(cx).waiting = Some((theme.to_string(), event.clone()));
        let (tx, rx) = async_channel::bounded::<Result<Played, String>>(1);
        let (name, ev) = (theme.to_string(), event.clone());
        // The lookup stats files: off the UI thread.
        let task = cx.background_executor().spawn(async move {
            sounds::preview_theme(&ev, &name, volume, Some(Box::new(move |r| {
                let _ = tx.send_blocking(r);
            })))
        });
        let name = theme.to_string();
        cx.spawn(async move |this, cx| {
            let file = task.await;
            let result = match file {
                Some(_) => rx.recv().await.unwrap_or_else(|_| Err(String::new())),
                None => Err(t("This theme has no sound for that").to_string()),
            };
            let _ = this.update(cx, |_, cx| {
                let s = st(cx);
                if s.waiting.as_ref().is_some_and(|(t, e)| *t == name && *e == event) {
                    s.waiting = None;
                }
                match result {
                    Ok(played) => s.preview = Some(Preview { theme: name, event, started: Instant::now(), played }),
                    Err(e) if !e.is_empty() && e != "played a moment ago" => {
                        s.errors.insert(name, e);
                    }
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The preview playing for `theme`: its outline and how far it got.
    fn sound_playing(theme: &str, window: &mut gpui::Window, cx: &mut Context<Self>) -> Option<([u8; PEAKS], f32, SoundEvent)> {
        let s = st(cx);
        let p = s.preview.as_ref()?;
        let length = Duration::from_millis(u64::from(p.played.duration_ms));
        let elapsed = p.started.elapsed();
        if elapsed > length + PREVIEW_TAIL {
            s.preview = None;
            return None;
        }
        if p.theme != theme {
            return None;
        }
        // Frames only while it plays.
        window.request_animation_frame();
        let progress = if length.is_zero() { 1.0 } else { (elapsed.as_secs_f32() / length.as_secs_f32()).min(1.0) };
        Some((p.played.peaks, progress, p.event.clone()))
    }

    fn sound_install(&mut self, theme: &'static SoundTheme, cx: &mut Context<Self>) {
        let name = theme.theme.to_string();
        if st(cx).jobs.contains_key(&name) {
            return;
        }
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let s = st(cx);
        s.errors.remove(&name);
        s.jobs.insert(name.clone(), Job { progress: progress.clone(), cancel: cancel.clone() });
        let task = cx.background_executor().spawn(async move { install::install(theme, &install::Env::current(), &|p| progress.store((p.fraction() * PERMILLE) as u32, Ordering::Relaxed), &cancel) });
        Self::repaint_while(cx, {
            let name = name.clone();
            move |cx| st(cx).jobs.contains_key(&name)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |_, cx| {
                let s = st(cx);
                s.jobs.remove(&name);
                match result {
                    Ok(_) => {
                        Self::sound_refresh(cx);
                        Self::finished_sound(cx);
                    }
                    Err(e) => {
                        s.errors.insert(name, e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The shell's "Finished" sound (it follows your switches).
    fn finished_sound(cx: &mut gpui::App) {
        if let Some(ipc) = cx.try_global::<crate::perf::ShellIpc>().cloned() {
            std::thread::spawn(move || {
                let _ = (ipc.0)(vec!["sounds".into(), "play".into(), "complete".into()]);
            });
        }
    }


    fn sound_remove(&mut self, name: String, cx: &mut Context<Self>) {
        if st(cx).confirm_remove.as_deref() != Some(&name) {
            st(cx).confirm_remove = Some(name);
            cx.notify();
            return;
        }
        st(cx).confirm_remove = None;
        let job = name.clone();
        let task = cx.background_executor().spawn(async move { install::remove(&job, &install::Env::current()) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |_, cx| {
                match result {
                    Ok(_) => {
                        // A removed theme can't stay the one playing.
                        if mandara_ui::state::config(cx).read(cx).get().sounds.theme.trim() == name {
                            apply_theme(None, cx);
                        }
                        Self::sound_refresh(cx);
                    }
                    Err(e) => {
                        st(cx).errors.insert(name, e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Installs your own theme (a folder, an archive or a link) off the UI
    /// thread; the note under "Your own" says how it went.
    fn sound_import(&mut self, run: impl FnOnce(&install::Env, &dyn Fn(mandara_services::cursors::install::Progress), &AtomicBool) -> Result<install::Installed, String> + Send + 'static, cx: &mut Context<Self>) {
        if st(cx).import.is_some() {
            return;
        }
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        st(cx).import = Some(Job { progress: progress.clone(), cancel: cancel.clone() });
        st(cx).import_note = None;
        let task = cx.background_executor().spawn(async move { run(&install::Env::current(), &|p| progress.store((p.fraction() * PERMILLE) as u32, Ordering::Relaxed), &cancel) });
        Self::repaint_while(cx, |cx| st(cx).import.is_some());
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |_, cx| {
                st(cx).import = None;
                st(cx).import_note = Some(match result {
                    Ok(i) => {
                        Self::sound_refresh(cx);
                        Self::finished_sound(cx);
                        Ok(tf("{theme} is installed: apply it from the gallery.", &[("theme", &i.theme)]))
                    }
                    Err(e) => Err(e),
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Asks the desktop's file picker for an archive (or a folder) off the
    /// UI thread, then installs it.
    fn sound_pick(&mut self, folder: bool, cx: &mut Context<Self>) {
        let title = if folder { t("Choose a sound theme folder") } else { t("Choose a sound theme archive") }.to_string();
        let filter = t("Sound theme archives").to_string();
        let task = cx.background_executor().spawn(async move {
            let downloads = dirs::download_dir();
            if folder {
                mandara_services::file_chooser::pick_folder(&title, downloads)
            } else {
                mandara_services::file_chooser::open_files(&title, &filter, &ARCHIVE_MIME, &ARCHIVE_GLOBS, downloads).map(|v| v.into_iter().next())
            }
        });
        cx.spawn(async move |this, cx| {
            let picked = task.await;
            let _ = this.update(cx, |this, cx| match picked {
                Ok(Some(path)) => this.sound_import(move |env, progress, cancel| install::import(&path, env, progress, cancel), cx),
                Ok(None) => {}
                Err(e) => {
                    st(cx).import_note = Some(Err(e));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn sound_url_field(cx: &mut Context<Self>) -> Entity<TextField> {
        if let Some(f) = &st(cx).url {
            return f.clone();
        }
        let view = cx.entity().downgrade();
        let field = cx.new(|cx| {
            TextField::new(
                String::new(),
                t("https://… link to a .tar.gz, .tar.xz or .zip"),
                false,
                Rc::new(move |value, _, cx| {
                    let _ = view.update(cx, |_, cx| {
                        st(cx).confirm_url = Some(value.trim().to_string()).filter(|v| !v.is_empty());
                        cx.notify();
                    });
                }),
                cx,
            )
        });
        st(cx).url = Some(field.clone());
        field
    }

    fn sound_search_field(cx: &mut Context<Self>) -> Entity<TextField> {
        if let Some(f) = &st(cx).search {
            return f.clone();
        }
        let view = cx.entity().downgrade();
        let field = cx.new(|cx| {
            TextField::new(
                String::new(),
                t("Search themes…"),
                true,
                Rc::new(move |value, _, cx| {
                    let _ = view.update(cx, |_, cx| {
                        st(cx).query = value.trim().to_lowercase();
                        cx.notify();
                    });
                }),
                cx,
            )
        });
        st(cx).search = Some(field.clone());
        field
    }

    /// Themes → Sounds.
    pub(super) fn sounds_tab(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut gpui::Window, cx: &mut Context<Self>) -> AnyElement {
        if !cfg.sound_themes_enabled() {
            return self.sounds_gate(p, window, cx);
        }
        let display = mandara_ui::fonts::fonts(cx).display.clone();
        let current = cfg.sounds.theme.trim().to_string();
        let reset = chip("sound-your-desktop", t("Your desktop's"), current.is_empty(), p)
            .tab_index(0)
            .on_click(cx.listener(|_, _: &ClickEvent, _, cx| apply_theme(None, cx)))
            .into_any_element();
        let head = section_head(
            t("Sounds"),
            tf("{count} open sound themes, downloaded on demand from their upstream releases, checked against a pinned checksum and installed to ~/.local/share/sounds", &[("count", &CATALOG.len())]),
            vec![reset],
            display,
            p,
        );
        let yours = self.sounds_yours(cfg, p, window, cx);
        let gallery = self.sounds_gallery(cfg, p, window, cx);
        let own = self.sounds_own(p, cx);
        div().row().flex_col().gap(px(SPACE_16_PX)).child(head).child(yours).child(gallery).child(own).into_any_element()
    }

    /// The plugin is off: what it is, what it touches, and Enable.
    fn sounds_gate(&mut self, p: &MandaraPalette, window: &mut gpui::Window, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        if st(cx).cover.is_none() && !st(cx).cover_loading {
            st(cx).cover_loading = true;
            let scale = window.scale_factor();
            let (w, h) = ((GATE_COVER.0 as f32 * scale.min(1.0)) as u32, (GATE_COVER.1 as f32 * scale.min(1.0)) as u32);
            let task = cx.background_executor().spawn(async move { mandara_ui::plugin_art::cover_thumb(ID, w.max(1), h.max(1)) });
            cx.spawn(async move |this, cx| {
                let cover = task.await;
                let _ = this.update(cx, |_, cx| {
                    let s = st(cx);
                    // Left meanwhile: not kept.
                    if s.cover_loading {
                        s.cover_loading = false;
                        s.cover = cover;
                    } else if let Some(c) = cover {
                        cx.drop_image(c, None);
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        let cover_w = (self.page.column_w - 2.0 * SPACE_22_PX).min(GATE_COVER_MAX_W_PX);
        let cover = div()
            .w(px(cover_w))
            .h(px(cover_w * GATE_COVER_ASPECT))
            .rounded(px(RADIUS_18_PX))
            .overflow_hidden()
            .bg(p.surface_container_high.hsla())
            .children(st(cx).cover.clone().map(|c| img(c).size_full().object_fit(gpui::ObjectFit::Cover)));
        let touch = |glyph: &'static str, text: String| {
            div()
                .row()
                .items_start()
                .gap(px(SPACE_10_PX))
                .child(div().pt(px(SPACE_3_PX)).child(icon(glyph, ICON_14_PX, p.primary)))
                .child(div().flex_1().min_w(px(0.0)).text_size(px(TEXT_12_5_PX)).line_height(px(LEADING_19_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(text)))
        };
        let enable = button("sound-gate-enable", "check", t("Enable Sound themes"), true, p).tab_index(0).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.plugin_lifecycle(ID, Action::Enable, cx)));
        let details = button("sound-gate-details", "info", t("Details"), false, p).tab_index(0).on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.open_plugin(ID, ModalView::Detail, window, cx)));
        div()
            .rounded(px(RADIUS_26_PX))
            .bg(mandara_core::oklab_gradient(DIAGONAL_DEG, p.surface_container, p.surface_container.mix(p.primary_container, ALPHA_35)))
            .p(px(SPACE_22_PX))
            .row()
            .flex_col()
            .items_center()
            .gap(px(SPACE_18_PX))
            .child(cover)
            .child(
                div()
                    .max_w(px(GATE_COVER_MAX_W_PX))
                    .w_full()
                    .row()
                    .flex_col()
                    .gap(px(SPACE_12_PX))
                    .child(div().font_family(fonts.display.clone()).text_size(px(TEXT_24_PX)).font_weight(FontWeight::SEMIBOLD).child(t("Sound themes")))
                    .child(div().text_size(px(TEXT_14_PX)).line_height(px(LEADING_21_PX)).text_color(p.on_surface.hsla()).child(mandara_ui::bidi::paragraph(t("Install, preview and apply desktop sound themes: the chimes for notifications, warnings, volume, devices and logging in, for Mandara and your apps."))))
                    .child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(t("What it touches")))
                    .child(touch("folder", t("Installs themes you pick into ~/.local/share/sounds, and removes only those.").to_string()))
                    .child(touch("volume-high", t("Sets the sound theme Mandara's own event sounds play from.").to_string()))
                    .child(touch("sliders", t("With Apply to apps: GTK's sound theme (gsettings and existing settings.ini files) and KDE's kdeglobals. Turning the plugin off puts them back.").to_string()))
                    .child(touch("eye-off", t("Off, it reads and writes nothing.").to_string()))
                    .child(div().row().items_center().gap(px(SPACE_8_PX)).pt(px(SPACE_6_PX)).child(enable).child(details)),
            )
            .into_any_element()
    }

    /// "Your sounds": the theme playing now with the shell-sound settings,
    /// and every event.
    fn sounds_yours(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut gpui::Window, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let chosen = cfg.sounds.theme.trim().to_string();
        let shown = if chosen.is_empty() { sounds::system_theme() } else { chosen.clone() };
        let (installed, _) = Self::sound_lists(cx);
        let info = installed.iter().find(|i| i.name == shown).cloned();
        let catalog = mandara_services::sound_themes::catalog::by_theme_name(&shown);
        let title = match (&info, catalog) {
            (_, Some(t)) => t.name.to_string(),
            (Some(i), None) => i.title.clone(),
            (None, None) => shown.clone(),
        };
        let line = if chosen.is_empty() { tf("Following your desktop ({theme})", &[("theme", &shown)]) } else { catalog.map(|t| tr(t.origin)).unwrap_or_else(|| info.as_ref().map(|i| i.comment.clone()).unwrap_or_default()) };
        let tint = catalog.map(|t| swatch(t.swatch)).unwrap_or(p.primary);
        let playing = Self::sound_playing(&shown, window, cx);
        let peaks = playing.as_ref().map(|(pk, ..)| *pk).unwrap_or_else(|| idle_peaks(&shown));
        let wave = wave_strip(&peaks, playing.as_ref().map(|(_, pr, _)| *pr), playing.is_some(), tint, p);
        let off = (!cfg.sounds.enabled).then(|| div().text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(p.warning_amber.hsla()).child(mandara_ui::bidi::paragraph(t("Mandara's own sounds are off: switch Shell sounds on below to hear them on events."))));
        let rows = self.rows_plain(registry::in_section(registry::Section::Audio).filter(|s| s.group == registry::alert_settings::SOUNDS && s.path != "/sounds/theme"), self.panel_w() - 2.0 * SPACE_16_PX, cfg, p, cx);
        let panel = div()
            .flex_1()
            .min_w(px(0.0))
            .rounded(px(RADIUS_22_PX))
            .bg(mandara_core::oklab_gradient(DIAGONAL_DEG, p.surface_container, p.surface_container.mix(p.primary_container, ALPHA_35)))
            .p(px(SPACE_16_PX))
            .row()
            .flex_col()
            .gap(px(SPACE_14_PX))
            .child(
                div()
                    .row()
                    .flex_col()
                    .gap(px(SPACE_3_PX))
                    .child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(t("Your sound theme")))
                    .child(div().truncate().font_family(fonts.display.clone()).text_size(px(TEXT_20_PX)).font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(div().truncate().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(line)),
            )
            .child(wave)
            .children(off)
            .child(rows);
        let events = self.sounds_events(cfg, &shown, p, cx);
        let split = self.page.column_w >= SPLIT_MIN_W_PX && !self.stack_rows;
        div()
            .row()
            .when(!split, |el| el.flex_col())
            .gap(px(SPACE_16_PX))
            .child(if split { panel.w(px(self.panel_w())).flex_none() } else { panel })
            .child(div().flex_1().min_w(px(0.0)).child(events))
            .into_any_element()
    }

    fn panel_w(&self) -> f32 {
        if self.page.column_w >= SPLIT_MIN_W_PX && !self.stack_rows { ((self.page.column_w - SPACE_16_PX) * PANEL_SHARE).floor() } else { self.page.column_w }
    }

    /// Every configurable event: its switch, what this theme plays and a
    /// test button.
    fn sounds_events(&mut self, cfg: &ShellConfig, theme: &str, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let files = Self::sound_coverage(theme, cx);
        let waiting = st(cx).waiting.clone();
        let mut list = div().row().flex_col().gap(px(SPACE_3_PX));
        for (i, e) in SoundEvent::CONFIGURABLE.iter().enumerate() {
            let on = e.enabled_in(&cfg.sounds);
            let file = files.as_ref().map(|f| f.get(i).cloned().flatten());
            let detail = match &file {
                None => t("Looking…").to_string(),
                Some(None) => t("Silent in this theme").to_string(),
                Some(Some(path)) => path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            };
            let silent = matches!(file, Some(None));
            let is_waiting = waiting.as_ref().is_some_and(|(t, w)| t == theme && w == e);
            let test = {
                let (ev, name) = (e.clone(), theme.to_string());
                div()
                    .id(SharedString::from(format!("sound-test-{i}")))
                    .size(px(DIM_30_PX))
                    .flex_none()
                    .rounded_full()
                    .row()
                    .items_center()
                    .justify_center()
                    .when(!silent, |el| el.cursor_pointer().hover(|s| s.bg(p.primary.with_alpha(ALPHA_14).hsla())))
                    .when(silent, |el| el.opacity(ALPHA_35))
                    .tooltip(tip(t("Play"), p))
                    .child(icon(if is_waiting { "waves" } else { "play" }, ICON_13_PX, p.primary))
                    .when(!silent, |el| el.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.sound_preview(&name, ev.clone(), cx))))
            };
            let toggle = {
                let ev = e.clone();
                div()
                    .id(SharedString::from(format!("sound-on-{i}")))
                    .flex_none()
                    .cursor_pointer()
                    .child(super::form::switch(on, p))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        let ev = ev.clone();
                        update_config(cx, move |c| ev.set_in(&mut c.sounds, !on));
                    }))
            };
            list = list.child(
                div()
                    .px(px(SPACE_10_PX))
                    .py(px(SPACE_7_PX))
                    .rounded(px(RADIUS_14_PX))
                    .row()
                    .items_center()
                    .gap(px(SPACE_12_PX))
                    .hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_3_5).hsla()))
                    .child(div().size(px(EVENT_TILE_PX)).flex_none().rounded(px(RADIUS_10_PX)).bg(p.primary.with_alpha(if on { ALPHA_16 } else { ALPHA_6 }).hsla()).row().items_center().justify_center().child(icon(event_icon(e), ICON_14_PX, if on { p.primary } else { p.on_surface_variant })))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .row()
                            .flex_col()
                            .child(div().truncate().text_size(px(TEXT_13_PX)).font_weight(FontWeight::MEDIUM).when(!on, |el| el.text_color(p.on_surface_variant.hsla())).child(tr(e.label())))
                            .child(div().truncate().text_size(px(TEXT_11_PX)).text_color(if silent { p.warning_amber.hsla() } else { p.on_surface_variant.hsla() }).child(detail)),
                    )
                    .child(test)
                    .child(toggle),
            );
        }
        div()
            .rounded(px(RADIUS_22_PX))
            .bg(p.surface_container.hsla())
            .p(px(SPACE_8_PX))
            .row()
            .flex_col()
            .gap(px(SPACE_6_PX))
            .child(div().px(px(SPACE_10_PX)).pt(px(SPACE_6_PX)).text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(t("Events")))
            .child(list)
            .into_any_element()
    }

    fn sounds_gallery(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut gpui::Window, cx: &mut Context<Self>) -> AnyElement {
        let (installed, ours) = Self::sound_lists(cx);
        let active = cfg.sounds.theme.trim().to_string();
        let filter = st(cx).filter;
        let query = st(cx).query.clone();
        let is_installed = |name: &str| installed.iter().any(|i| i.name == name);
        let mut cards: Vec<Card> = CATALOG.iter().map(Card::Catalog).collect();
        cards.extend(installed.iter().filter(|i| mandara_services::sound_themes::catalog::by_theme_name(&i.name).is_none()).cloned().map(Card::Installed));
        let shown: Vec<Card> = cards
            .into_iter()
            .filter(|c| match filter {
                Filter::All => true,
                Filter::Installed => is_installed(c.theme()),
                Filter::Get => !is_installed(c.theme()),
            })
            .filter(|c| c.matches(&query))
            .collect();
        let count = shown.len();
        let elements: Vec<AnyElement> = shown.into_iter().map(|c| self.sound_card(c, &active, &installed, &ours, p, window, cx)).collect();
        let search = Self::sound_search_field(cx);
        let chips = [(Filter::All, t("All")), (Filter::Installed, t("Installed")), (Filter::Get, t("To get"))].into_iter().map(|(f, label)| {
            chip(SharedString::from(format!("sound-filter-{f:?}")), label, filter == f, p)
                .tab_index(0)
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    st(cx).filter = f;
                    cx.notify();
                }))
                .into_any_element()
        });
        let head = div()
            .row()
            .when(self.stack_rows, |el| el.flex_col())
            .when(!self.stack_rows, |el| el.items_center())
            .gap(px(SPACE_12_PX))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .row()
                    .items_center()
                    .gap(px(SPACE_8_PX))
                    .child(div().text_size(px(TEXT_14_PX)).font_weight(FontWeight::SEMIBOLD).child(t("Gallery")))
                    .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(count.to_string()))
                    .children(chips),
            )
            .child(div().when(!self.stack_rows, |el| el.w(px(DIM_280_PX)).flex_none()).child(search));
        let body = if elements.is_empty() {
            div().py(px(SPACE_22_PX)).row().justify_center().text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).child(t("No theme matches")).into_any_element()
        } else {
            grid::fit(self.page.column_w, SOUND_GRID).render(elements)
        };
        div().row().flex_col().gap(px(SPACE_12_PX)).child(head).child(body).into_any_element()
    }

    fn sound_card(&mut self, card: Card, active: &str, installed: &[ThemeInfo], ours: &[String], p: &MandaraPalette, window: &mut gpui::Window, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let name = card.theme().to_string();
        let info = installed.iter().find(|i| i.name == name);
        let have = info.is_some();
        let job = st(cx).jobs.get(&name).map(|j| j.progress.load(Ordering::Relaxed));
        let error = st(cx).errors.get(&name).cloned();
        let state = match (job, error.is_some(), have, name == active) {
            (Some(pm), ..) => CardState::Installing(pm),
            (None, true, false, _) => CardState::Failed,
            (None, _, true, true) => CardState::Active,
            (None, _, true, false) => CardState::Apply,
            (None, _, false, _) => CardState::Get,
        };
        let removable = ours.contains(&name) && have && job.is_none();
        let arming = st(cx).confirm_remove.as_deref() == Some(name.as_str());
        let tint = match &card {
            Card::Catalog(t) => swatch(t.swatch),
            Card::Installed(_) => p.tertiary,
        };
        let (title, line, author, license) = match &card {
            Card::Catalog(t) => (t.name.to_string(), tr(t.origin), Some(t.author), Some(short_license(t.license))),
            Card::Installed(i) => (i.title.clone(), if i.user { t("Yours").to_string() } else { t("From your system").to_string() }, None, None),
        };
        let description = match &card {
            Card::Catalog(t) => tr(t.description),
            Card::Installed(i) => i.comment.clone(),
        };

        // The outline, playing along with a preview.
        let playing = Self::sound_playing(&name, window, cx);
        let peaks = playing.as_ref().map(|(pk, ..)| *pk).unwrap_or_else(|| idle_peaks(&name));
        let strip = div()
            .relative()
            .w_full()
            .rounded(px(RADIUS_16_PX))
            .overflow_hidden()
            .child(wave_strip(&peaks, playing.as_ref().map(|(_, pr, _)| *pr), playing.is_some(), tint, p))
            .when(state == CardState::Active, |el| {
                el.child(
                    div()
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
                        .bg(p.primary.hsla())
                        .text_color(p.on_primary.hsla())
                        .text_size(px(TEXT_10_5_PX))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(icon("check", ICON_11_PX, p.on_primary))
                        .child(t("Active")),
                )
            })
            .when(have && state != CardState::Active, |el| {
                let label = if info.is_some_and(|i| i.user) { t("Installed") } else { t("System") };
                el.child(div().absolute().top(px(SPACE_8_PX)).inset_end(px(SPACE_8_PX)).max_w(gpui::relative(BADGE_MAX_SHARE)).overflow_hidden().child(badge(label, p.secondary)))
            });

        // Try it: the signature sounds.
        let waiting = st(cx).waiting.clone();
        let tries = div().row().flex_wrap().items_center().gap(px(SPACE_6_PX)).children(SIGNATURE.iter().enumerate().map(|(i, e)| {
            let on = playing.as_ref().is_some_and(|(_, _, pe)| pe == e) || waiting.as_ref().is_some_and(|(t, w)| *t == name && w == e);
            let (ev, theme) = (e.clone(), name.clone());
            chip(SharedString::from(format!("sound-try-{name}-{i}")), tr(e.label()), on, p)
                .when(!have, |el| el.opacity(ALPHA_45).cursor_default())
                .child(icon(if on { "waves" } else { "play" }, ICON_11_PX, if on { p.on_primary } else { p.primary }))
                .when(have, |el| el.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.sound_preview(&theme, ev.clone(), cx))))
                .when(!have, |el| el.tooltip(tip(t("Preview after download"), p)))
        }));

        // The action row.
        let (label, glyph, filled): (String, &'static str, bool) = match (state, &card) {
            (CardState::Get, Card::Catalog(t)) => (tf("Get · {size}", &[("size", &mandara_ui::widgets::system::format_bytes(t.asset.size))]), "download", false),
            (CardState::Get, Card::Installed(_)) => (t("Get").to_string(), "download", false),
            (CardState::Installing(pm), _) => (tf("Installing {percent}% · Cancel", &[("percent", &(pm / PERCENT_PER_PERMILLE))]), "close", false),
            (CardState::Failed, _) => (t("Retry").to_string(), "refresh", false),
            (CardState::Apply, _) => (t("Apply").to_string(), "check", true),
            (CardState::Active, _) => (t("Active").to_string(), "check", false),
        };
        let act = {
            let card = card.clone();
            let name = name.clone();
            move |this: &mut StudioView, cx: &mut Context<StudioView>| match (state, &card) {
                (CardState::Get | CardState::Failed, Card::Catalog(t)) => this.sound_install(t, cx),
                (CardState::Installing(_), _) => {
                    if let Some(job) = st(cx).jobs.get(&name) {
                        job.cancel.store(true, Ordering::Relaxed);
                    }
                }
                (CardState::Apply, _) => apply_theme(Some(name.clone()), cx),
                _ => {}
            }
        };
        let act = Rc::new(act);
        let primary = {
            let act = act.clone();
            button(SharedString::from(format!("sound-act-{name}")), glyph, label, filled, p)
                .when(state == CardState::Active, |el| el.opacity(ALPHA_75).cursor_default())
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| act(this, cx)))
        };
        let remove = removable.then(|| {
            let name = name.clone();
            div()
                .id(SharedString::from(format!("sound-rm-{name}")))
                .h(px(DIM_34_PX))
                .px(px(if arming { SPACE_12_PX } else { SPACE_9_PX }))
                .rounded_full()
                .row()
                .items_center()
                .gap(px(SPACE_6_PX))
                .flex_none()
                .cursor_pointer()
                .bg(if arming { p.urgent_coral.hsla() } else { p.urgent_coral.with_alpha(ALPHA_14).hsla() })
                .text_color(if arming { p.on_primary.hsla() } else { p.urgent_coral.hsla() })
                .text_size(px(TEXT_12_5_PX))
                .font_weight(FontWeight::MEDIUM)
                .tooltip(tip(t("Remove (only what Mandara installed)"), p))
                .child(icon("trash", ICON_14_PX, if arming { p.on_primary } else { p.urgent_coral }))
                .when(arming, |el| el.child(t("Remove?")))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.sound_remove(name.clone(), cx)))
        });
        let homepage = match card {
            Card::Catalog(theme) => Some(
                div()
                    .id(SharedString::from(format!("sound-home-{}", theme.id)))
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
                    .on_click(move |_: &ClickEvent, _, _| super::open_link(theme.homepage)),
            ),
            Card::Installed(_) => None,
        };
        let progress = job.map(|pm| {
            div().h(px(PROGRESS_H_PX)).w_full().rounded_full().bg(p.surface_container_high.hsla()).child(div().h_full().w(gpui::relative((pm.min(PERMILLE as u32) as f32 / PERMILLE).max(PROGRESS_MIN))).rounded_full().bg(mandara_core::oklab_gradient(HORIZONTAL_DEG, p.accent_gradient_start, p.accent_gradient_end)))
        });
        let error_row = error.map(|e| {
            div().row().items_start().gap(px(SPACE_6_PX)).child(icon("alert", ICON_13_PX, p.urgent_coral)).child(div().flex_1().min_w(px(0.0)).text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(p.urgent_coral.hsla()).child(mandara_ui::bidi::paragraph(e)))
        });
        let preview_first = {
            let theme = name.clone();
            move |this: &mut StudioView, cx: &mut Context<StudioView>| this.sound_preview(&theme, SIGNATURE[0].clone(), cx)
        };
        div()
            .id(SharedString::from(format!("sound-card-{name}")))
            .flex_1()
            .min_w(px(0.0))
            .overflow_hidden()
            .tab_index(0)
            .p(px(SPACE_8_PX))
            .rounded(px(RADIUS_22_PX))
            .bg(p.surface_container.hsla())
            .border_2()
            .border_color(if state == CardState::Active { p.primary.hsla() } else { gpui::transparent_black() })
            .hover(|s| s.border_color(p.primary.with_alpha(ALPHA_35).hsla()))
            .focus_visible(|s| s.border_color(p.primary.with_alpha(ALPHA_80).hsla()))
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
                    .child(div().row().items_center().gap(px(SPACE_8_PX)).child(div().flex_1().min_w(px(0.0)).truncate().text_start().font_family(fonts.display.clone()).text_size(px(TEXT_15_PX)).font_weight(FontWeight::SEMIBOLD).child(title)).children(license.map(|l| badge(l, p.tertiary))))
                    .child(div().truncate().text_start().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(match author {
                        Some(a) => format!("{line} · {a}"),
                        None => line,
                    }))
                    .child(div().h(px(DIM_32_PX)).overflow_hidden().text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(p.on_surface_variant.hsla()).line_clamp(2).child(mandara_ui::bidi::paragraph(description)))
                    .child(tries)
                    .children(progress)
                    .children(error_row),
            )
            .child(div().px(px(SPACE_4_PX)).pb(px(SPACE_4_PX)).row().flex_wrap().items_center().gap(px(SPACE_6_PX)).child(primary).child(div().flex_1()).children(remove).children(homepage))
            .on_key_down(cx.listener(move |this, e: &gpui::KeyDownEvent, _, cx| {
                let k = &e.keystroke;
                if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
                    return;
                }
                match k.key.as_str() {
                    "enter" => act(this, cx),
                    "space" if have => preview_first(this, cx),
                    "delete" | "backspace" if removable => this.sound_remove(name.clone(), cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .into_any_element()
    }

    /// "Your own": a folder, an archive or a link.
    fn sounds_own(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let field = Self::sound_url_field(cx);
        let busy = st(cx).import.as_ref().map(|j| j.progress.load(Ordering::Relaxed));
        let pending = st(cx).confirm_url.clone();
        let note = st(cx).import_note.clone();
        let file = button("sound-own-file", "package", t("Install an archive…"), false, p).tab_index(0).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.sound_pick(false, cx)));
        let folder = button("sound-own-folder", "folder", t("Install a folder…"), false, p).tab_index(0).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.sound_pick(true, cx)));
        let go = {
            let field = field.clone();
            button("sound-own-url", "download", t("Install from link"), false, p).tab_index(0).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                let url = field.read(cx).value.trim().to_string();
                st(cx).confirm_url = Some(url).filter(|u| !u.is_empty());
                cx.notify();
            }))
        };
        let confirm = pending.map(|url| {
            let https = url.starts_with("https://");
            let host = url.trim_start_matches("https://").split('/').next().unwrap_or_default().to_string();
            let text = if https { tf("{host} isn't one of the catalog's pinned releases, so Mandara can't vouch for what it sends. Install it anyway?", &[("host", &host)]) } else { t("Only https:// links can be installed").to_string() };
            let anyway = https.then(|| {
                let url = url.clone();
                button("sound-own-anyway", "check", t("Install anyway"), true, p).tab_index(0).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    st(cx).confirm_url = None;
                    let url = url.clone();
                    this.sound_import(move |env, progress, cancel| install::import_url(&url, env, progress, cancel), cx);
                }))
            });
            let cancel = button("sound-own-cancel", "close", t("Cancel"), false, p).tab_index(0).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                st(cx).confirm_url = None;
                cx.notify();
            }));
            div()
                .rounded(px(RADIUS_16_PX))
                .bg(p.warning_amber.with_alpha(ALPHA_12).hsla())
                .p(px(SPACE_12_PX))
                .row()
                .flex_col()
                .gap(px(SPACE_10_PX))
                .child(div().row().items_start().gap(px(SPACE_8_PX)).child(icon("shield", ICON_14_PX, p.warning_amber)).child(div().flex_1().min_w(px(0.0)).text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).child(mandara_ui::bidi::paragraph(text))))
                .child(div().row().items_center().gap(px(SPACE_6_PX)).children(anyway).child(cancel))
        });
        let progress = busy.map(|pm| {
            let cancel = div()
                .id("sound-own-stop")
                .text_size(px(TEXT_12_PX))
                .text_color(p.primary.hsla())
                .cursor_pointer()
                .child(t("Cancel"))
                .on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                    if let Some(j) = &st(cx).import {
                        j.cancel.store(true, Ordering::Relaxed);
                    }
                }));
            div()
                .row()
                .items_center()
                .gap(px(SPACE_10_PX))
                .child(div().flex_1().h(px(PROGRESS_H_PX)).rounded_full().bg(p.surface_container_high.hsla()).child(div().h_full().w(gpui::relative((pm.min(PERMILLE as u32) as f32 / PERMILLE).max(PROGRESS_MIN))).rounded_full().bg(mandara_core::oklab_gradient(HORIZONTAL_DEG, p.accent_gradient_start, p.accent_gradient_end))))
                .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(tf("Installing {percent}%", &[("percent", &(pm / PERCENT_PER_PERMILLE))])))
                .child(cancel)
        });
        div()
            .rounded(px(RADIUS_22_PX))
            .bg(p.surface_container.hsla())
            .p(px(SPACE_16_PX))
            .row()
            .flex_col()
            .gap(px(SPACE_12_PX))
            .child(
                div()
                    .row()
                    .flex_col()
                    .gap(px(SPACE_3_PX))
                    .child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(t("Your own")))
                    .child(div().text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("A freedesktop sound theme: a folder with index.theme and stereo/ sounds, or an archive of one. Only sounds, index.theme and licence files are kept; nothing in it runs.")))),
            )
            .child(div().row().flex_wrap().items_center().gap(px(SPACE_8_PX)).child(file).child(folder))
            .child(div().row().items_center().gap(px(SPACE_8_PX)).child(div().flex_1().min_w(px(0.0)).child(field)).child(go))
            .children(confirm)
            .children(progress)
            .children(note.map(|n| match n {
                Ok(text) => super::form::note(&text, false, p),
                Err(text) => super::form::note(&text, true, p),
            }))
            .into_any_element()
    }
}

/// The level outline: [`PEAKS`] bars, the played part in the theme's
/// colour while a preview plays (`progress`), quiet otherwise.
fn wave_strip(peaks: &[u8; PEAKS], progress: Option<f32>, live: bool, tint: Color, p: &MandaraPalette) -> AnyElement {
    let base = p.surface_container_high;
    let bg = mandara_core::oklab_gradient(DIAGONAL_DEG, base, base.mix(tint, WAVE_TINT));
    let lit = progress.map(|pr| (pr * PEAKS as f32).ceil() as usize).unwrap_or(0);
    let bars = peaks.iter().enumerate().map(|(i, v)| {
        let level = (f32::from(*v) / f32::from(u8::MAX)).max(WAVE_FLOOR);
        let color = if !live {
            tint.with_alpha(ALPHA_30)
        } else if i < lit {
            tint
        } else {
            tint.with_alpha(ALPHA_25)
        };
        div().w(px(WAVE_BAR_W_PX)).flex_shrink(1.0).min_w(px(1.0)).h(px((WAVE_H_PX - 2.0 * SPACE_12_PX) * level)).rounded_full().bg(color.hsla())
    });
    div().w_full().min_w(px(0.0)).h(px(WAVE_H_PX)).flex_none().overflow_hidden().rounded(px(RADIUS_16_PX)).bg(bg).px(px(SPACE_14_PX)).row().items_center().justify_between().gap(px(WAVE_BAR_GAP_PX)).children(bars).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The level outline fits the narrowest card with full-width bars, and
    /// even a far narrower box with thinned ones (it is clipped past that).
    #[test]
    fn the_outline_fits_inside_every_card() {
        let inner = |card_w: f32| card_w - 2.0 * (SPACE_8_PX + BORDER_2_PX) - 2.0 * SPACE_14_PX;
        let full = PEAKS as f32 * WAVE_BAR_W_PX + (PEAKS - 1) as f32 * WAVE_BAR_GAP_PX;
        assert!(full <= inner(SOUND_GRID.min_w), "{full} > {}", inner(SOUND_GRID.min_w));
        let thinnest = PEAKS as f32 + (PEAKS - 1) as f32 * WAVE_BAR_GAP_PX;
        assert!(thinnest <= inner(SOUND_GRID.min_w * 0.6));
    }

    #[test]
    fn idle_outlines_are_stable_and_in_range() {
        assert_eq!(idle_peaks("ocean"), idle_peaks("ocean"));
        assert_ne!(idle_peaks("ocean"), idle_peaks("oxygen"));
        assert!(idle_peaks("x").iter().any(|v| *v > 0));
    }

    #[test]
    fn licences_shorten_and_cards_search() {
        assert_eq!(short_license("CC-BY-SA-4.0"), "CC-BY-SA-4.0");
        assert_eq!(short_license("LGPL-3.0-or-later AND CC-BY-3.0"), "LGPL-3.0-or-later +");
        let ocean = Card::Catalog(mandara_services::sound_themes::catalog::find("ocean").unwrap());
        assert!(ocean.matches("") && ocean.matches("ocean") && ocean.matches("kde") && !ocean.matches("zzz"));
        let mine = Card::Installed(ThemeInfo { name: "calm".into(), title: "Calm Bells".into(), comment: "soft".into(), dir: PathBuf::new(), user: true });
        assert!(mine.matches("bells") && mine.theme() == "calm");
    }

    #[test]
    fn every_event_has_an_icon_from_the_set() {
        for e in SoundEvent::ALL {
            assert!(mandara_ui::icons::ICON_NAMES.contains(&event_icon(&e)), "{e:?}");
        }
        for name in ["folder", "package", "download", "shield", "sliders", "eye-off", "volume-high", "info", "waves", "play"] {
            assert!(mandara_ui::icons::ICON_NAMES.contains(&name), "{name}");
        }
    }
}
