//! The desktop's right-click menu.
//!
//! The wallpaper canvas ([`crate::backdrop`]) takes the pointer on the
//! background layer; a right-click on empty desktop arrives here with its
//! output and position and opens this menu there, on a full-output overlay
//! surface (`mandara-desktop-menu`) that exists only while the menu is
//! shown: a click outside closes it, a right-click elsewhere moves it.
//!
//! * Edit desktop · Add widget ▸ · Show / hide desktop widgets
//! * Change wallpaper… · Next wallpaper (with a slideshow or its folder)
//! * Themes & looks… · Display settings…
//! * Open terminal · Take screenshot
//! * Mandara Studio…
//!
//! The menu card is chromatic glass (the frame colour, a faint vertical
//! sheen, the 1 px specular rim and a soft elevation) that springs open
//! from the click; rows tint toward the accent in OKLCH on hover or with
//! the arrow keys, keybinds and values sit right-aligned, and "Add widget"
//! opens a flyout beside it (to the reading side, flipped near an edge).
//! The rows, their renderer and the placement are shared with desktop edit
//! mode's own menus ([`crate::desktop_edit`]).

use std::rc::Rc;
use std::time::Duration;

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer};
use gpui::{
    App, AppContext as _, Context, Div, FocusHandle, FontWeight, Global, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, WindowHandle, div, prelude::FluentBuilder as _, px,
};
use mandara_config::ShellConfig;
use mandara_config::compositor::BindAction;
use mandara_core::{FrameClock, SpringConfig, SpringValue, SurfaceRole, ToGpui};
use mandara_i18n::{t, tf};
use mandara_theme::{Color, MandaraPalette};

use crate::bidi::Bidi as _;
use crate::icons::icon;
use crate::kit::text;
use crate::plugins::dock::sheet::Sheet;
use crate::plugins::panel_fx::{ANGLE_DOWN_DEG, elevation, readable, rim};
use crate::state::{config, update_config};

// ── Metrics ──────────────────────────────────────────────────────────────

/// Main card and flyout widths.
pub const MENU_W: f32 = 300.0;
pub const SUB_W: f32 = 244.0;
/// Row height, card padding, separator band and corner radii.
const ROW_H: f32 = 32.0;
const PAD: f32 = 6.0;
const SEP_H: f32 = 9.0;
const RADIUS: f32 = 14.0;
const ROW_RADIUS: f32 = 9.0;
/// Inner row padding, icon-label gap and glyph size.
const ROW_PAD: f32 = 10.0;
const ROW_GAP: f32 = 10.0;
const GLYPH: f32 = 15.0;
/// Distance from the pointer, and least distance from the screen edges.
const POINTER_GAP: f32 = 2.0;
const EDGE: f32 = 8.0;
/// Gap between the card and its flyout.
const SUB_GAP: f32 = 4.0;
/// Key caps of a shortcut hint: height, padding, radius, gap.
const KEY_H: f32 = 18.0;
const KEY_PAD: f32 = 5.0;
const KEY_RADIUS: f32 = 5.0;
const KEY_GAP: f32 = 3.0;
/// Hairline separators and key caps (alpha of the on-surface colour).
const DIVIDER_ALPHA: f32 = 0.08;
const KEY_ALPHA: f32 = 0.07;
/// Secondary text: hints and disabled rows.
const HINT_ALPHA: f32 = 0.62;
const DISABLED_ALPHA: f32 = 0.42;
/// OKLCH mix of the accent into the glass under a hovered row.
const HOVER_TINT: f32 = 0.16;
/// The glass: opacity and the sheen's lightness lift / drop.
const GLASS_ALPHA: f32 = 0.96;
const SHEEN: (f32, f32) = (0.022, -0.006);
/// Opening choreography: the card slides this far from the pointer as it
/// springs open; the flyout rises from this opacity.
const OPEN_SLIDE: f32 = 8.0;
const OPEN_SPRING: SpringConfig = SpringConfig::new(520.0, 34.0, 1.0);
const SUB_SPRING: SpringConfig = SpringConfig::new(640.0, 40.0, 1.0);
/// The snipping tool captures the screen as it opens: the menu's surface
/// must be gone by then.
const SCREENSHOT_SETTLE: Duration = Duration::from_millis(160);

// ── Model ────────────────────────────────────────────────────────────────

/// A widget the menus can add.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NewWidget {
    Clock,
    Monitor,
    Decoration,
    Visualizer,
    NowPlaying,
    Calendar,
}

impl NewWidget {
    pub const ALL: [NewWidget; 6] = [Self::Clock, Self::Monitor, Self::Decoration, Self::Visualizer, Self::NowPlaying, Self::Calendar];

    pub fn label(self) -> &'static str {
        match self {
            Self::Clock => t("Clock"),
            Self::Monitor => t("System monitor"),
            Self::Decoration => t("Decoration"),
            Self::Visualizer => t("Visualizer"),
            Self::NowPlaying => t("Now playing"),
            Self::Calendar => t("Calendar"),
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Clock => "clock",
            Self::Monitor => "gauge",
            Self::Decoration => "leaf",
            Self::Visualizer => "waves",
            Self::NowPlaying => "music",
            Self::Calendar => "calendar",
        }
    }
}

/// What a menu row does.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    EditDesktop,
    /// Opens the "Add widget" flyout.
    AddMenu,
    Add(NewWidget),
    ChangeWallpaper,
    NextWallpaper,
    ToggleWidgets,
    Themes,
    Displays,
    Terminal,
    Screenshot,
    Studio,
    // Edit mode: one widget.
    Remove(String),
    Duplicate(String),
    Hide(String),
    WidgetSettings,
}

/// What sits right-aligned on a row.
#[derive(Clone, Debug, PartialEq)]
pub enum Hint {
    /// A keybind, one cap per key.
    Keys(Vec<SharedString>),
    /// A value (an interval, a count, a display).
    Text(SharedString),
}

/// One menu row. Rows of different `group`s get a hairline between them.
#[derive(Clone, Debug)]
pub struct Entry {
    pub action: Action,
    pub label: SharedString,
    pub icon: &'static str,
    pub hint: Option<Hint>,
    pub submenu: bool,
    pub danger: bool,
    pub enabled: bool,
    pub group: u8,
}

pub fn entry(group: u8, icon: &'static str, label: impl Into<SharedString>, action: Action) -> Entry {
    Entry { action, label: label.into(), icon, hint: None, submenu: false, danger: false, enabled: true, group }
}

impl Entry {
    pub fn hint(mut self, hint: Option<Hint>) -> Self {
        self.hint = hint;
        self
    }
    pub fn submenu(mut self) -> Self {
        self.submenu = true;
        self
    }
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }
}

/// The keys of the first keybind running `mandara msg <command>…`, for
/// the row's hint.
fn keys_for(cfg: &ShellConfig, command: &str) -> Option<Hint> {
    keys_where(cfg, |a| matches!(a, BindAction::Mandara(words) if words.trim() == command || words.trim().starts_with(&format!("{command} "))))
}

/// The keys of the first keybind whose action passes `want`.
fn keys_where(cfg: &ShellConfig, want: impl Fn(&BindAction) -> bool) -> Option<Hint> {
    let bind = cfg.compositor.keybinds.iter().find(|b| want(&b.action))?;
    let mut keys: Vec<SharedString> = bind.mods.iter().map(|m| mod_label(m).into()).collect();
    let key = bind.key.trim();
    keys.push(if key.chars().count() == 1 { key.to_uppercase().into() } else { key.to_string().into() });
    Some(Hint::Keys(keys))
}

/// A modifier as printed on a key cap.
fn mod_label(m: &str) -> String {
    match m.trim().to_ascii_lowercase().as_str() {
        "super" | "win" | "logo" | "mod4" => t("Super").to_string(),
        "ctrl" | "control" => t("Ctrl").to_string(),
        "alt" | "mod1" => t("Alt").to_string(),
        "shift" => t("Shift").to_string(),
        other => other.to_string(),
    }
}

/// The desktop menu's rows for a right-click on `output`.
pub fn desktop_entries(cfg: &ShellConfig, output: &str) -> Vec<Entry> {
    let w = &cfg.wallpaper;
    let widgets = &cfg.desktop.widgets;
    let count = widgets.items.iter().filter(|i| i.enabled).count();
    let mut out = vec![
        entry(0, "pencil", t("Edit desktop"), Action::EditDesktop).hint(keys_for(cfg, "desktop edit")),
        entry(0, "plus", t("Add widget"), Action::AddMenu).submenu(),
    ];
    if count > 0 {
        let (icon, label) = if widgets.hidden { ("eye", t("Show desktop widgets")) } else { ("eye-off", t("Hide desktop widgets")) };
        out.push(entry(0, icon, label, Action::ToggleWidgets).hint(Some(Hint::Text(count.to_string().into()))));
    }
    out.push(entry(1, "image", t("Change wallpaper…"), Action::ChangeWallpaper).hint(keys_for(cfg, "wallpaper picker")));
    if w.slideshow.enabled || !w.slideshow.directory.trim().is_empty() {
        let every = w.slideshow.enabled.then(|| Hint::Text(tf("every {n} min", &[("n", &w.slideshow.interval_minutes)]).into()));
        out.push(entry(1, "skip", t("Next wallpaper"), Action::NextWallpaper).hint(every));
    }
    out.push(entry(2, "droplet", t("Themes & looks…"), Action::Themes).hint(keys_for(cfg, "studio themes")));
    out.push(entry(2, "monitor", t("Display settings…"), Action::Displays).hint((!output.is_empty()).then(|| Hint::Text(crate::bidi::ltr_isolate(output).into()))));
    let terminal = cfg.apps.terminal_command();
    let program = terminal.as_deref().and_then(|c| c.split_whitespace().next());
    let keys = keys_where(cfg, |a| matches!(a, BindAction::Spawn(cmd) if program.is_some() && cmd.split_whitespace().next() == program));
    out.push(entry(3, "terminal", t("Open terminal"), Action::Terminal).hint(keys).enabled(terminal.is_some()));
    out.push(entry(3, "crop", t("Take screenshot"), Action::Screenshot).hint(keys_for(cfg, "screenshot")));
    out.push(entry(4, "sliders", t("Mandara Studio…"), Action::Studio).hint(keys_where(cfg, |a| matches!(a, BindAction::Mandara(w) if w.trim() == "studio"))));
    out
}

/// The "Add widget" rows: what paints on the wallpaper, then the cards
/// (a hairline between the two says it without a word on every row).
pub fn add_entries() -> Vec<Entry> {
    NewWidget::ALL.into_iter().map(|k| entry(if matches!(k, NewWidget::NowPlaying | NewWidget::Calendar) { 1 } else { 0 }, k.icon(), k.label(), Action::Add(k))).collect()
}

/// Card height of `entries`.
pub fn height(entries: &[Entry]) -> f32 {
    let seps = entries.windows(2).filter(|w| w[0].group != w[1].group).count() as f32;
    PAD * 2.0 + entries.len() as f32 * ROW_H + seps * SEP_H
}

/// Top of row `i` inside its card.
pub fn row_top(entries: &[Entry], i: usize) -> f32 {
    let seps = entries.windows(2).take(i).filter(|w| w[0].group != w[1].group).count() as f32;
    PAD + i as f32 * ROW_H + seps * SEP_H
}

/// Where a `w × h` card opens for a pointer at `at` on a `screen`: below
/// and to the reading side of the pointer, flipped where it would leave
/// the screen (`up` prefers above). Returns the top-left and whether it
/// opens downward.
pub fn place(at: (f32, f32), (w, h): (f32, f32), screen: (f32, f32), rtl: bool, up: bool) -> ((f32, f32), bool) {
    let (sw, sh) = screen;
    let after = at.0 + POINTER_GAP;
    let before = at.0 - POINTER_GAP - w;
    let x = if rtl {
        if before >= EDGE { before } else { after }
    } else if after + w <= sw - EDGE {
        after
    } else {
        before
    };
    let below = at.1 + POINTER_GAP;
    let above = at.1 - POINTER_GAP - h;
    let down = if up { above < EDGE } else { below + h <= sh - EDGE || above < EDGE };
    let y = if down { below } else { above };
    let clamp = |v: f32, len: f32, total: f32| v.min(total - EDGE - len).max(EDGE);
    ((clamp(x, w, sw), clamp(y, h, sh)), down)
}

/// Where the flyout of the row at `row_y` (screen px) of a card at `card_x`
/// sits: beside the card on the reading side, flipped near an edge.
pub fn place_sub(card_x: f32, row_y: f32, (w, h): (f32, f32), screen: (f32, f32), rtl: bool) -> (f32, f32) {
    let right = card_x + MENU_W + SUB_GAP;
    let left = card_x - SUB_GAP - w;
    let x = if rtl {
        if left >= EDGE { left } else { right }
    } else if right + w <= screen.0 - EDGE {
        right
    } else {
        left
    };
    let y = (row_y - PAD).min(screen.1 - EDGE - h).max(EDGE);
    (x.min(screen.0 - EDGE - w).max(EDGE), y)
}

// ── Rendering (shared with edit mode) ────────────────────────────────────

/// Picks / hovers row `i` of a menu.
pub type OnRow<V> = Rc<dyn Fn(&mut V, usize, &mut Window, &mut Context<V>)>;
pub type OnHover<V> = Rc<dyn Fn(&mut V, usize, &mut Context<V>)>;

/// The glass a menu card sits on.
pub fn glass(palette: &MandaraPalette) -> Color {
    crate::shell::frame_color(palette).with_alpha(GLASS_ALPHA)
}

/// A menu card (glass, sheen, specular rim, elevation at openness `p`)
/// with its rows; the caller positions it. `key` keeps element ids apart
/// when two cards are shown (a flyout).
#[allow(clippy::too_many_arguments)]
pub fn card<V: 'static>(key: &'static str, entries: &[Entry], hover: Option<usize>, width: f32, p: f32, palette: &MandaraPalette, cx: &mut Context<V>, on_pick: OnRow<V>, on_hover: OnHover<V>) -> Div {
    let base = glass(palette);
    let (lift, drop) = SHEEN;
    let mut shadow = elevation(palette, p);
    shadow.extend(rim(palette));
    let mut list = div().flex().flex_col();
    for (i, e) in entries.iter().enumerate() {
        if i > 0 && entries[i - 1].group != e.group {
            list = list.child(div().h(px(SEP_H)).flex().items_center().px(px(ROW_PAD)).child(div().w_full().h(px(1.0)).bg(palette.on_surface.with_alpha(DIVIDER_ALPHA).hsla())));
        }
        list = list.child(row(key, i, e, hover == Some(i), palette, cx, on_pick.clone(), on_hover.clone()));
    }
    div()
        .w(px(width))
        .h(px(height(entries)))
        .p(px(PAD))
        .rounded(px(crate::style::radius(RADIUS)))
        .bg(mandara_core::oklab_gradient(ANGLE_DOWN_DEG, base.lighten(lift), base.lighten(drop)))
        .shadow(shadow)
        .overflow_hidden()
        .child(list)
}

#[allow(clippy::too_many_arguments)]
fn row<V: 'static>(key: &'static str, i: usize, e: &Entry, hovered: bool, palette: &MandaraPalette, cx: &mut Context<V>, on_pick: OnRow<V>, on_hover: OnHover<V>) -> impl IntoElement {
    let accent = readable(palette.primary, palette);
    let fg = if e.danger { palette.urgent_coral } else { palette.on_surface };
    let glyph = if e.danger {
        palette.urgent_coral
    } else if hovered && e.enabled {
        accent
    } else {
        palette.on_surface_variant
    };
    let hot = hovered && e.enabled;
    let tint = glass(palette).mix(if e.danger { palette.urgent_coral } else { accent }, HOVER_TINT).with_alpha(1.0);
    let hint = e.hint.clone().map(|h| hint_el(h, palette));
    div()
        .id((key, i))
        .h(px(ROW_H))
        .px(px(ROW_PAD))
        .rounded(px(crate::style::radius(ROW_RADIUS)))
        .row()
        .items_center()
        .gap(px(ROW_GAP))
        .when(hot, |el| el.bg(tint.hsla()))
        .when(!e.enabled, |el| el.opacity(DISABLED_ALPHA))
        .when(e.enabled, |el| el.cursor_pointer())
        .on_hover(cx.listener(move |this, h: &bool, _, cx| {
            if *h {
                on_hover(this, i, cx);
            }
        }))
        .when(e.enabled, |el| el.on_click(cx.listener(move |this, _, window, cx| on_pick(this, i, window, cx))))
        .child(icon(e.icon, GLYPH, glyph))
        .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(text::BODY)).text_color(fg.hsla()).when(hot, |el| el.font_weight(FontWeight::MEDIUM)).child(e.label.clone()))
        .children(hint)
        .when(e.submenu, |el| el.child(icon(crate::bidi::dir_icon("chevron-right"), GLYPH, if hot { accent } else { palette.on_surface_variant })))
}

fn hint_el(h: Hint, palette: &MandaraPalette) -> gpui::AnyElement {
    let dim = palette.on_surface_variant.with_alpha(HINT_ALPHA);
    match h {
        Hint::Text(s) => div().flex_none().text_size(px(text::LABEL)).text_color(dim.hsla()).font_features(crate::fonts::tabular_nums()).child(s).into_any_element(),
        Hint::Keys(keys) => div()
            .flex_none()
            .ltr_row()
            .gap(px(KEY_GAP))
            .children(keys.into_iter().map(|k| {
                div()
                    .h(px(KEY_H))
                    .px(px(KEY_PAD))
                    .rounded(px(KEY_RADIUS))
                    .bg(palette.on_surface.with_alpha(KEY_ALPHA).hsla())
                    .flex()
                    .items_center()
                    .text_size(px(text::LABEL))
                    .text_color(dim.hsla())
                    .child(k)
            }))
            .into_any_element(),
    }
}

// ── Running an action ────────────────────────────────────────────────────

/// Runs a desktop-menu action for a right-click at `at` on `output` (of
/// logical size `size`).
pub fn run(action: &Action, output: &str, at: (f32, f32), size: (f32, f32), cx: &mut App) {
    match action {
        Action::EditDesktop => crate::desktop_edit::enter_on(Some(output), cx),
        Action::Add(kind) => crate::desktop_edit::add_at(*kind, output, Some(at), cx),
        Action::ChangeWallpaper => {
            if config(cx).read(cx).get().wallpaper.picker {
                crate::overlays::picker_on(output, Some(size), cx);
                crate::state::shell(cx).update(cx, |s, cx| s.set_wallpaper_picker(true, cx));
            } else {
                crate::state::open_studio_at(Some("wallpaper".into()), cx);
            }
        }
        Action::NextWallpaper => {
            if let Err(err) = crate::slideshow::advance(None, cx) {
                tracing::warn!("desktop menu: {err}");
            }
        }
        Action::ToggleWidgets => update_config(cx, |c| c.desktop.widgets.hidden = !c.desktop.widgets.hidden),
        Action::Themes => crate::state::open_studio_at(Some("themes".into()), cx),
        Action::Displays => crate::state::open_studio_at(Some("displays".into()), cx),
        Action::Studio => crate::state::open_studio_at(None, cx),
        Action::Terminal => open_terminal(cx),
        Action::Screenshot => {
            cx.spawn(async move |cx| {
                cx.background_executor().timer(SCREENSHOT_SETTLE).await;
                cx.update(|cx| {
                    if let Err(err) = crate::plugins::screenshot::start("region", cx) {
                        tracing::warn!("desktop menu: {err}");
                    }
                });
            })
            .detach();
        }
        Action::AddMenu | Action::Remove(_) | Action::Duplicate(_) | Action::Hide(_) | Action::WidgetSettings => {}
    }
}

/// Starts the user's terminal (Studio → Applications, else the first one
/// installed) in the home folder.
fn open_terminal(cx: &mut App) {
    let Some(cmd) = config(cx).read(cx).get().apps.terminal_command() else { return };
    let argv: Vec<&str> = cmd.split_whitespace().collect();
    let Some((program, args)) = argv.split_first() else { return };
    use std::os::unix::process::CommandExt as _;
    let mut c = std::process::Command::new(program);
    c.args(args).process_group(0);
    if let Some(home) = std::env::var_os("HOME") {
        c.current_dir(home);
    }
    if let Err(err) = mandara_services::reaper::spawn(&mut c) {
        tracing::warn!("desktop menu: starting {program}: {err}");
    }
}

// ── The menu surface ─────────────────────────────────────────────────────

pub struct DesktopMenu {
    output: String,
    at: (f32, f32),
    /// The output's logical size (the surface's).
    size: (f32, f32),
    entries: Vec<Entry>,
    hover: Option<usize>,
    /// The "Add widget" flyout is open (and the row hovered in it).
    sub: bool,
    sub_hover: Option<usize>,
    sub_open: SpringValue<f32>,
    sub_clock: FrameClock,
    focus: FocusHandle,
    sheet: Sheet,
}

impl DesktopMenu {
    fn add_row(&self) -> Option<usize> {
        self.entries.iter().position(|e| e.action == Action::AddMenu)
    }

    fn pick(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(e) = self.entries.get(i).filter(|e| e.enabled) else { return };
        if e.action == Action::AddMenu {
            self.sub = !self.sub;
            self.sub_hover = self.sub.then_some(0);
            cx.notify();
            return;
        }
        let action = e.action.clone();
        self.finish(action, window, cx);
    }

    fn pick_sub(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(e) = add_entries().get(i) {
            let action = e.action.clone();
            self.finish(action, window, cx);
        }
    }

    /// Closes the menu, then runs `action`.
    fn finish(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        if action == Action::Screenshot {
            // No outro: the capture must not see the menu.
            window.remove_window();
        } else {
            self.sheet.close();
        }
        let (output, at, size) = (self.output.clone(), self.at, self.size);
        cx.defer(move |cx| run(&action, &output, at, size, cx));
        cx.notify();
    }

    fn hover_main(&mut self, i: usize, cx: &mut Context<Self>) {
        if self.hover != Some(i) || self.sub != (Some(i) == self.add_row()) {
            self.hover = Some(i);
            self.sub = Some(i) == self.add_row();
            self.sub_hover = None;
            cx.notify();
        }
    }

    fn key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let rtl = crate::bidi::rtl();
        let (open_key, close_key) = if rtl { ("left", "right") } else { ("right", "left") };
        let step = |cur: Option<usize>, n: usize, up: bool| -> Option<usize> {
            if n == 0 {
                return None;
            }
            Some(match (cur, up) {
                (None, false) => 0,
                (None, true) => n - 1,
                (Some(c), false) => (c + 1) % n,
                (Some(c), true) => (c + n - 1) % n,
            })
        };
        let key = e.keystroke.key.as_str();
        match key {
            "escape" if self.sub && self.sub_hover.is_some() => {
                self.sub = false;
                self.sub_hover = None;
            }
            "escape" => self.sheet.close(),
            "up" | "down" if self.sub && self.sub_hover.is_some() => self.sub_hover = step(self.sub_hover, add_entries().len(), key == "up"),
            "up" | "down" => {
                self.hover = step(self.hover, self.entries.len(), key == "up");
                self.sub = false;
            }
            k if k == open_key && self.hover.is_some() && self.hover == self.add_row() => {
                self.sub = true;
                self.sub_hover = Some(0);
            }
            k if k == close_key && self.sub => {
                self.sub = false;
                self.sub_hover = None;
            }
            "enter" | "space" => match (self.sub, self.sub_hover, self.hover) {
                (true, Some(i), _) => return self.pick_sub(i, window, cx),
                (_, _, Some(i)) => return self.pick(i, window, cx),
                _ => {}
            },
            _ => return,
        }
        cx.notify();
    }
}

impl Render for DesktopMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = mandara_core::frame_palette(window, cx);
        let f = crate::fonts::fonts(cx);
        let rtl = crate::bidi::rtl();
        let vp = window.viewport_size();
        let screen = (f32::from(vp.width), f32::from(vp.height));
        let p = self.sheet.step(false, window, cx);
        if self.sheet.gone() {
            return div().into_any_element();
        }
        let h = height(&self.entries);
        let ((x, y), down) = place(self.at, (MENU_W, h), screen, rtl, false);
        let slide = (1.0 - p.clamp(0.0, 1.0)) * OPEN_SLIDE * if down { -1.0 } else { 1.0 };
        let on_pick: OnRow<Self> = Rc::new(|this, i, window, cx| this.pick(i, window, cx));
        let on_hover: OnHover<Self> = Rc::new(|this, i, cx| this.hover_main(i, cx));
        let main = card("desktop-menu-row", &self.entries, self.hover, MENU_W, p, &palette, cx, on_pick, on_hover);

        // The flyout follows its own (quicker) spring.
        let reduce = mandara_core::spring::motion_profile().reduce;
        self.sub_open.set_instant(reduce);
        self.sub_open.set_target(if self.sub && !self.sheet.closing() { 1.0 } else { 0.0 });
        let moving = self.sub_open.step(self.sub_clock.tick(std::time::Instant::now()));
        if moving {
            window.request_animation_frame();
        } else {
            self.sub_clock.idle();
        }
        let q = self.sub_open.value().clamp(0.0, 1.0);
        let sub = (q > 0.0).then(|| {
            let rows = add_entries();
            let sh = height(&rows);
            let row_y = y + self.add_row().map_or(0.0, |i| row_top(&self.entries, i));
            let (sx, sy) = place_sub(x, row_y, (SUB_W, sh), screen, rtl);
            let toward = if sx < x { 1.0 } else { -1.0 };
            let pick: OnRow<Self> = Rc::new(|this, i, window, cx| this.pick_sub(i, window, cx));
            let hover: OnHover<Self> = Rc::new(|this, i, cx| {
                if this.sub_hover != Some(i) {
                    this.sub_hover = Some(i);
                    cx.notify();
                }
            });
            card("desktop-menu-sub", &rows, self.sub_hover, SUB_W, q, &palette, cx, pick, hover)
                .id("desktop-menu-sub")
                .occlude()
                .absolute()
                .left(px(sx + (1.0 - q) * OPEN_SLIDE * toward))
                .top(px(sy))
                .opacity(q * p.clamp(0.0, 1.0))
        });

        div()
            .id("desktop-menu-root")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .font_family(f.ui.clone())
            .text_color(palette.on_surface.hsla())
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| this.key(e, window, cx)))
            // Outside clicks close; a right-click moves the menu there.
            .child(
                div()
                    .id("desktop-menu-outside")
                    .absolute()
                    .inset_0()
                    .on_mouse_down(MouseButton::Right, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                        this.at = (f32::from(e.position.x), f32::from(e.position.y));
                        this.sub = false;
                        this.hover = None;
                        cx.notify();
                    }))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.sheet.close();
                        cx.notify();
                    }))
                    .on_mouse_down(MouseButton::Middle, cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.sheet.close();
                        cx.notify();
                    })),
            )
            .child(main.id("desktop-menu-card").occlude().absolute().left(px(x)).top(px(y + slide)).opacity(p.clamp(0.0, 1.0)))
            .children(sub)
            .into_any_element()
    }
}

#[derive(Default)]
struct Open(Option<WindowHandle<DesktopMenu>>);
impl Global for Open {}

/// Closes the desktop menu if it is open.
pub fn close(cx: &mut App) {
    if let Some(h) = cx.try_global::<Open>().and_then(|m| m.0) {
        let _ = h.update(cx, |_, window, _| window.remove_window());
        cx.set_global(Open(None));
    }
}

/// Opens the desktop menu at `at` (logical px) on `output`.
pub fn open(output: &str, at: (f32, f32), cx: &mut App) {
    open_sized(output, at, None, cx);
}

/// [`open`] with the output's logical size when the caller knows it (the
/// wallpaper canvas does). The startup output list misses a display
/// connected later, and a menu surface sized for another output is
/// centred on this one: the card then opened away from the pointer.
pub fn open_sized(output: &str, at: (f32, f32), known: Option<(f32, f32)>, cx: &mut App) {
    close(cx);
    if crate::desktop_edit::active(cx) || crate::lock::is_locked() {
        return;
    }
    let cfg = config(cx).read(cx).snapshot();
    let entries = desktop_entries(&cfg, output);
    let size = menu_surface_size(known, || crate::plugins::dock::output_size(output, cx));
    let display = crate::plugins::dock::display_for(output, cx);
    // Over the whole output (zones ignored, so the click point maps 1:1),
    // keyboard for the arrows and Esc.
    let options = crate::plugins::dock::layer_options(SurfaceRole::DesktopMenu, Layer::Overlay, Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT, size, Some(IGNORE_ZONES), KeyboardInteractivity::Exclusive, display);
    let output = output.to_string();
    let handle = cx.open_window(options, move |window, cx| {
        cx.new(|cx| {
            let focus = cx.focus_handle();
            focus.focus(window, cx);
            DesktopMenu { output, at, size, entries, hover: None, sub: false, sub_hover: None, sub_open: SpringValue::new(0.0, SUB_SPRING), sub_clock: FrameClock::new(), focus, sheet: Sheet::new(OPEN_SPRING) }
        })
    });
    match handle {
        Ok(h) => cx.set_global(Open(Some(h))),
        Err(err) => tracing::warn!("desktop menu: {err:#}"),
    }
}

/// Layer-shell exclusive zone `-1`: the whole output, whatever the bar
/// reserves.
pub const IGNORE_ZONES: f32 = -1.0;

/// Smallest side, logical px, accepted as an output size for the menu
/// surface: a layer surface asked for a 0 side is a protocol error.
const MIN_SURFACE_SIDE: f32 = 1.0;

/// The menu surface's size: the size the canvas reported for the clicked
/// output when it is usable, else `fallback` (the startup output list).
fn menu_surface_size(known: Option<(f32, f32)>, fallback: impl FnOnce() -> (f32, f32)) -> (f32, f32) {
    let usable = |(w, h): (f32, f32)| w.is_finite() && h.is_finite() && w >= MIN_SURFACE_SIDE && h >= MIN_SURFACE_SIDE;
    match known.filter(|s| usable(*s)) {
        Some(size) => size,
        None => {
            let (w, h) = fallback();
            (w.max(MIN_SURFACE_SIDE), h.max(MIN_SURFACE_SIDE))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actions(e: &[Entry]) -> Vec<Action> {
        e.iter().map(|e| e.action.clone()).collect()
    }

    #[test]
    fn the_menu_groups_its_rows_and_offers_next_only_with_a_slideshow() {
        let mut cfg = ShellConfig::default();
        let a = actions(&desktop_entries(&cfg, "DP-1"));
        assert_eq!(a[..2], [Action::EditDesktop, Action::AddMenu]);
        assert!(!a.contains(&Action::NextWallpaper));
        assert!(!a.contains(&Action::ToggleWidgets), "nothing to hide");
        assert_eq!(a.last(), Some(&Action::Studio));
        cfg.wallpaper.slideshow.enabled = true;
        cfg.desktop.widgets.add(mandara_config::desktop_widgets::DesktopWidget::default());
        let e = desktop_entries(&cfg, "DP-1");
        assert!(actions(&e).contains(&Action::NextWallpaper));
        assert!(actions(&e).contains(&Action::ToggleWidgets));
        // Groups are contiguous: one hairline per change.
        let changes = e.windows(2).filter(|w| w[0].group != w[1].group).count();
        assert_eq!(changes, 4);
        assert!(height(&e) > e.len() as f32 * ROW_H);
    }

    #[test]
    fn keybind_hints_come_from_the_compositor_binds() {
        let cfg = ShellConfig::default();
        // The default binds have Super+Shift+S for `screenshot region`.
        let e = desktop_entries(&cfg, "");
        let shot = e.iter().find(|e| e.action == Action::Screenshot).unwrap();
        assert!(matches!(&shot.hint, Some(Hint::Keys(k)) if k.last().map(|s| s.as_ref()) == Some("S")), "{:?}", shot.hint);
    }

    #[test]
    fn the_menu_surface_takes_the_clicked_outputs_size() {
        let startup_first = || (1707.0, 960.0);
        // A display connected after startup: the canvas's size wins.
        assert_eq!(menu_surface_size(Some((1920.0, 1080.0)), startup_first), (1920.0, 1080.0));
        // Unknown or unusable sizes fall back, never to a 0 side.
        assert_eq!(menu_surface_size(None, startup_first), (1707.0, 960.0));
        assert_eq!(menu_surface_size(Some((0.0, 1080.0)), startup_first), (1707.0, 960.0));
        assert_eq!(menu_surface_size(Some((f32::NAN, 1080.0)), startup_first), (1707.0, 960.0));
        assert_eq!(menu_surface_size(None, || (0.0, 0.0)), (MIN_SURFACE_SIDE, MIN_SURFACE_SIDE));
    }

    #[test]
    fn the_card_stays_on_screen_and_mirrors() {
        let screen = (1000.0, 800.0);
        let ((x, y), down) = place((100.0, 100.0), (200.0, 300.0), screen, false, false);
        assert!(down && x > 100.0 && y > 100.0);
        // Near the right / bottom edges it flips to the other side.
        let ((x, y), down) = place((950.0, 700.0), (200.0, 300.0), screen, false, false);
        assert!(!down && x + 200.0 <= 950.0 && y + 300.0 <= 700.0);
        // Right to left: it opens to the left of the pointer.
        let ((x, _), _) = place((500.0, 100.0), (200.0, 300.0), screen, true, false);
        assert!(x + 200.0 <= 500.0);
        let (sx, _) = place_sub(700.0, 100.0, (SUB_W, 200.0), screen, false);
        assert!(sx + SUB_W <= 700.0, "no room on the right: the flyout goes left");
    }
}
