//! Studio → Login screen: every Mandara Login setting, with a live preview
//! that is the real greeter view rendering the draft at a scaled size.
//!
//! * Controls on the bottom edit `~/.config/mandara/greeter.toml` (the
//!   draft the demo uses); each change re-renders the preview at once.
//! * On the preview, drag any element: it snaps to the nearest of the nine
//!   anchors (guides show while dragging) and keeps its offset; click to
//!   select it and edit its anchor, offset, scale, visibility and order.
//! * "Preview full screen" runs `mandara greeter --demo` with the draft;
//!   "Apply to login screen" installs it system-wide with
//!   `pkexec mandara greeter apply-config` (the real greeter reads
//!   `/etc/mandara/greeter.toml`).
//!
//! * Frames: the preview holds still (one picture of the backdrop, no
//!   drift or parallax) and draws frames only while something on it moves:
//!   a Try-it moment or an edit settling. Those frames re-render the views
//!   around it, so the page keeps its tabs and controls in a cached child
//!   view ([`GreeterControls`]) and the preview itself is cached too.
//!
//! Strings go through the greeter's catalog (`tl`), which falls back to the
//! shell's shared catalogs — the same language as the rest of Studio.

use mandara_ui::bidi::Bidi as _;
use mandara_ui::style::ClipRounded as _;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyElement, AppContext as _, Bounds, ClickEvent, Context, Entity, FontWeight, Global, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled, StyleRefinement,
    WeakEntity, Window, canvas, div, prelude::FluentBuilder as _, px,
};
use mandara_core::ToGpui;
use mandara_greeter::config::{
    Anchor, AvatarShape, AvatarSource, BackgroundMode, CardStyle, ClockFace, Element, FieldStyle, GreeterConfig, LOOKS, Overlay, PasswordMask, PowerStyle, SessionStyle, SubmitButton,
    UserList, UserSwitcher, VideoMode,
};
use mandara_greeter::i18n::{tl, tlf};
use mandara_greeter::view::{GreeterView, PreviewEvent, Simulate};
use mandara_theme::MandaraPalette;
use mandara_ui::icons::icon;
use mandara_ui::text_field::TextField;

use super::StudioView;
use super::choice::{ChoiceItem, GREETER_CHOICE, placed};
use crate::tokens::*;

mod look_thumb;
mod manager;

/// The Looks tab's cards: three to a row (six looks make 3 × 2), fewer
/// when the page is narrow.
const LOOK_GRID: CardGrid = CardGrid { min_w: 200.0, max_w: 640.0, gap: SPACE_10_PX };
const LOOK_COLUMNS: usize = 3;
/// Each card's preview, 16:9 like the screen.
const LOOK_THUMB_ASPECT: f32 = 9.0 / 16.0;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Group {
    Looks,
    Manager,
    Layout,
    Background,
    Clock,
    User,
    Session,
    Theme,
    Motion,
}

impl Group {
    /// In the order people work: pick a look, arrange it, fine-tune the
    /// parts, and finally the system-wide login manager.
    const ALL: [Group; 9] = [Group::Looks, Group::Layout, Group::Background, Group::Clock, Group::User, Group::Session, Group::Theme, Group::Motion, Group::Manager];

    fn label(self) -> String {
        match self {
            Group::Looks => tl("Looks"),
            Group::Manager => tl("Login manager"),
            Group::Layout => tl("Layout"),
            Group::Background => tl("Background"),
            Group::Clock => tl("Clock"),
            Group::User => tl("Sign-in"),
            Group::Session => tl("Session"),
            Group::Theme => tl("Theme"),
            Group::Motion => tl("Motion"),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Group::Looks => "brush",
            Group::Manager => "monitor",
            Group::Layout => "grid",
            Group::Background => "image",
            Group::Clock => "timer",
            Group::User => "lock",
            Group::Session => "power",
            Group::Theme => "sun",
            Group::Motion => "refresh",
        }
    }
}

/// Translated element name.
fn element_label(e: Element) -> String {
    match e {
        Element::Clock => tl("Clock"),
        Element::Date => tl("Date"),
        Element::User => tl("User card"),
        Element::Password => tl("Password field"),
        Element::Users => tl("User switcher"),
        Element::Session => tl("Session picker"),
        Element::Power => tl("Power buttons"),
        Element::Keyboard => tl("Keyboard layout"),
        Element::Banner => tl("Message"),
        Element::Logo => tl("Logo"),
        Element::Themes => tl("Colour schemes"),
    }
}

type SetF = fn(&mut GreeterConfig, f32);

/// A slider being dragged: (id, min, max, step, setter).
#[derive(Copy, Clone)]
struct SliderDrag {
    id: &'static str,
    min: f32,
    max: f32,
    step: f32,
    set: SetF,
}

pub struct GreeterPage {
    draft: GreeterConfig,
    preview: Entity<GreeterView>,
    controls: Entity<GreeterControls>,
    group: Group,
    selected: Option<Element>,
    message: Option<(bool, String)>,
    applying: bool,
    idle: bool,
    fields: HashMap<&'static str, Entity<TextField>>,
    /// Font pickers by row id.
    fonts: HashMap<&'static str, Entity<super::font_picker::FontSelect>>,
    tracks: Rc<RefCell<HashMap<&'static str, Bounds<Pixels>>>>,
    slider: Option<SliderDrag>,
    width: Rc<std::cell::Cell<f32>>,
    dirty: bool,
    /// Theme gallery (every theme with its dark palette), read from disk on
    /// first show instead of on every frame.
    themes: Option<Rc<[(mandara_theme::catalog::ThemeDef, MandaraPalette)]>>,
    /// Login manager tab: detection, pending plan, running switch.
    manager: manager::ManagerState,
}

/// The page's tabs and controls, cached: preview frames re-render the page
/// around the preview but reuse this subtree, which rebuilds only when the
/// page (any edit, tab or selection) or the theme changes.
pub struct GreeterControls {
    page: WeakEntity<GreeterPage>,
}

impl GreeterControls {
    fn new(page: WeakEntity<GreeterPage>, cx: &mut Context<Self>) -> Self {
        if let Some(entity) = page.upgrade() {
            cx.observe(&entity, |_, _, cx| cx.notify()).detach();
        }
        cx.observe(&mandara_core::theme_entity(cx), |_, _, cx| cx.notify()).detach();
        Self { page }
    }
}

impl Render for GreeterControls {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.page.update(cx, |page, cx| page.controls(window, cx)).unwrap_or_else(|_| div().into_any_element())
    }
}

struct PageHandle(Entity<GreeterPage>);

impl Global for PageHandle {}

/// The page element for Studio's body (one page entity per Studio process).
pub(super) fn element(cx: &mut Context<StudioView>) -> AnyElement {
    let page = match cx.try_global::<PageHandle>() {
        Some(h) => h.0.clone(),
        None => {
            let page = cx.new(GreeterPage::new);
            cx.set_global(PageHandle(page.clone()));
            page
        }
    };
    page.into_any_element()
}

/// The live preview is 16:9 like a screen…
const PREVIEW_ASPECT_W: f32 = 16.0;
const PREVIEW_ASPECT_H: f32 = 9.0;
/// …at most this share of the window's height…
const PREVIEW_MAX_SHARE: f32 = 0.4;
/// The column beside the preview (apply, full screen, try-it chips) on
/// pages at least this wide; narrower pages stack it under the preview.
const SIDE_W_PX: f32 = 236.0;
const SIDE_BY_SIDE_MIN_W_PX: f32 = 720.0;
/// The selected tab's underline.
const TAB_UNDERLINE_PX: f32 = 2.0;
/// …and never narrower than this.
const PREVIEW_MIN_W_PX: f32 = 320.0;
/// Preview width before the first layout measured the page.
const PREVIEW_INITIAL_W_PX: f32 = 880.0;
/// Page width assumed before the first layout.
const PAGE_INITIAL_W_PX: f32 = 900.0;
/// Window height taken by Studio's title bar and page header.
const PAGE_CHROME_H_PX: f32 = 130.0;
/// Shortest the editor column gets.
const PAGE_MIN_H_PX: f32 = 420.0;
/// Width of a slider track.
const TRACK_W_PX: f32 = 180.0;
/// Widest a choice grows beside its label (longer ones go under it).
const CHOICE_MAX_W_PX: f32 = DIM_420_PX;
/// Room a row keeps for its label beside a choice.
const CHOICE_LABEL_ROOM_PX: f32 = DIM_150_PX;
/// Slider steps at least this coarse show one decimal.
const ONE_DECIMAL_STEP: f32 = 0.1;
/// Theme swatches: gradient angle and the accent's lightness.
const SWATCH_GRADIENT_DEG: f32 = 135.0;
const SWATCH_ACCENT_LIGHTNESS: f32 = 0.45;

/// Drops the page — and its live preview with any video decoder and frame
/// queue — when Studio navigates away; the draft is already on disk, so a
/// later visit rebuilds it as it was. Kept while a system apply runs or a
/// slider drag hasn't saved yet.
pub(super) fn release(cx: &mut gpui::App) {
    let busy = cx.try_global::<PageHandle>().is_some_and(|h| {
        let page = h.0.read(cx);
        page.applying || page.dirty || page.manager.busy()
    });
    if !busy && let Some(handle) = cx.try_global::<PageHandle>().map(|h| h.0.clone()) {
        // Its pictures leave the atlas too (called outside window updates).
        let preview = handle.read(cx).preview.clone();
        for image in preview.update(cx, |view, _| view.take_images()) {
            cx.drop_image(image, None);
        }
        cx.remove_global::<PageHandle>();
    }
}

impl GreeterPage {
    fn new(cx: &mut Context<Self>) -> Self {
        let draft = GreeterConfig::load_from(&mandara_greeter::config::user_draft_path()).unwrap_or_else(|| {
            GreeterConfig::load_from(std::path::Path::new(mandara_greeter::config::SYSTEM_CONFIG)).unwrap_or_default()
        });
        let preview = cx.new(|cx| GreeterView::new_preview(draft.clone(), (PREVIEW_INITIAL_W_PX, PREVIEW_INITIAL_W_PX * PREVIEW_ASPECT_H / PREVIEW_ASPECT_W), cx));
        cx.subscribe(&preview, |this, _, ev: &PreviewEvent, cx| {
            match ev {
                PreviewEvent::Edited(cfg) => {
                    this.draft = cfg.clone();
                    this.save();
                }
                PreviewEvent::Selected(e) => {
                    this.selected = *e;
                    if e.is_some() {
                        this.group = Group::Layout;
                    }
                }
            }
            cx.notify();
        })
        .detach();
        let page = cx.entity().downgrade();
        let controls = cx.new(|cx| GreeterControls::new(page, cx));
        // The active login manager (plain file reads, off the UI thread).
        manager::spawn_detection(cx);
        Self {
            draft,
            preview,
            controls,
            // `MANDARA_STUDIO_LOGIN_GROUP=theme` opens another tab first
            // (screenshots, layout testing).
            group: std::env::var("MANDARA_STUDIO_LOGIN_GROUP").ok().and_then(|v| Group::ALL.into_iter().find(|g| format!("{g:?}").eq_ignore_ascii_case(&v))).unwrap_or(Group::Looks),
            selected: None,
            message: greetd_missing().map(|why| (false, why)),
            applying: false,
            idle: false,
            fields: HashMap::new(),
            fonts: HashMap::new(),
            tracks: Rc::new(RefCell::new(HashMap::new())),
            slider: None,
            width: Rc::new(std::cell::Cell::new(PAGE_INITIAL_W_PX)),
            dirty: false,
            themes: None,
            manager: manager::ManagerState::detecting(),
        }
    }

    fn save(&mut self) {
        if let Err(e) = self.draft.save_draft() {
            self.message = Some((false, tlf("Could not save the draft: {error}", &[("error", &e)])));
        }
        self.dirty = false;
    }

    /// Applies an edit: preview now, draft file now (or at the end of a
    /// slider drag).
    fn edit(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut GreeterConfig)) {
        f(&mut self.draft);
        self.draft.normalize();
        let cfg = self.draft.clone();
        self.preview.update(cx, |v, cx| v.set_config(cfg, cx));
        if self.slider.is_some() {
            self.dirty = true;
        } else {
            self.save();
        }
        cx.notify();
    }

    fn apply_system(&mut self, cx: &mut Context<Self>) {
        if self.applying {
            return;
        }
        if let Some(why) = greetd_missing() {
            self.message = Some((false, why));
            cx.notify();
            return;
        }
        self.save();
        self.applying = true;
        self.message = Some((true, super::admin_run::waiting_message()));
        let draft = mandara_greeter::config::user_draft_path();
        // As root through a root-owned Mandara, never this (user-writable)
        // binary.
        let task = cx.background_executor().spawn(async move { super::admin_run::as_root(["greeter", "apply-config", "--from"].map(std::ffi::OsString::from).into_iter().chain([draft.into_os_string()]).collect()) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.applying = false;
                this.message = Some(match result {
                    Ok(msg) => (true, if msg.is_empty() { tl("Installed for the login screen.") } else { msg }),
                    Err(e) => (false, e),
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn preview_full_screen(&mut self) {
        self.save();
        let exe = std::env::current_exe().unwrap_or_else(|_| "mandara".into());
        use std::os::unix::process::CommandExt as _;
        match std::process::Command::new(exe).args(["greeter", "--demo"]).stdin(std::process::Stdio::null()).process_group(0).spawn() {
            Ok(child) => mandara_services::reaper::adopt(child),
            Err(e) => self.message = Some((false, e.to_string())),
        }
    }

    // ── Controls ─────────────────────────────────────────────────────────

    fn row(p: &MandaraPalette, label: String, control: AnyElement) -> AnyElement {
        div()
            .min_h(px(DIM_40_PX))
            .px(px(SPACE_14_PX))
            .py(px(SPACE_6_PX))
            .row()
            .items_center()
            .justify_between()
            .gap(px(SPACE_16_PX))
            .child(div().flex_1().min_w(px(0.0)).text_color(p.on_surface.hsla()).child(label))
            .child(control)
            .into_any_element()
    }

    fn card(p: &MandaraPalette, title: String, rows: Vec<AnyElement>) -> AnyElement {
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_6_PX))
            .child(div().px(px(SPACE_6_PX)).text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface_variant.hsla()).child(title))
            .child(div().row().flex_col().py(px(SPACE_4_PX)).clip_rounded().rounded(px(RADIUS_18_PX)).bg(p.surface_container.hsla()).children(rows))
            .into_any_element()
    }

    fn toggle(&self, p: &MandaraPalette, id: &'static str, label: String, on: bool, set: fn(&mut GreeterConfig, bool), cx: &mut Context<Self>) -> AnyElement {
        let knob = div().size(px(DIM_18_PX)).rounded_full().bg(if on { p.on_primary.hsla() } else { p.on_surface_variant.hsla() });
        let switch = div()
            .id(id)
            .w(px(DIM_40_PX))
            .h(px(DIM_24_PX))
            .p(px(SPACE_3_PX))
            .rounded_full()
            .row()
            .when(on, |el| el.justify_end())
            .bg(if on { p.primary.hsla() } else { p.surface_container_high.hsla() })
            .cursor_pointer()
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.edit(cx, |c| set(c, !on))))
            .child(knob);
        Self::row(p, label, switch.into_any_element())
    }

    #[allow(clippy::too_many_arguments)]
    fn slider(&self, p: &MandaraPalette, id: &'static str, label: String, value: f32, min: f32, max: f32, step: f32, unit: &str, set: SetF, cx: &mut Context<Self>) -> AnyElement {
        let frac = ((value - min) / (max - min)).clamp(0.0, 1.0);
        let tracks = self.tracks.clone();
        let track = div()
            .id(id)
            .relative()
            .w(px(TRACK_W_PX))
            .h(px(DIM_22_PX))
            .row()
            .items_center()
            .cursor_pointer()
            .child(div().w_full().h(px(DIM_5_PX)).rounded_full().bg(p.surface_container_high.hsla()))
            .child(div().absolute().inset_start(px(0.0)).top(px(DIM_8_5_PX)).w(px(TRACK_W_PX * frac)).h(px(DIM_5_PX)).rounded_full().bg(p.primary.hsla()))
            .child(div().absolute().top(px(2.0)).inset_start(px(TRACK_W_PX * frac - DIM_18_PX / 2.0)).size(px(DIM_18_PX)).rounded_full().bg(p.on_surface.hsla()).shadow_sm())
            .child(canvas(move |b, _, _| { tracks.borrow_mut().insert(id, b); }, |_, _, _, _| {}).absolute().top_0().left_0().size_full())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    let drag = SliderDrag { id, min, max, step, set };
                    this.slider = Some(drag);
                    this.slide_to(drag, f32::from(e.position.x), cx);
                }),
            );
        let shown = if step >= 1.0 { format!("{value:.0}{unit}") } else if step >= ONE_DECIMAL_STEP { format!("{value:.1}{unit}") } else { format!("{value:.2}{unit}") };
        let control = div()
            .row()
            .items_center()
            .gap(px(SPACE_10_PX))
            .child(track)
            .child(div().w(px(DIM_56_PX)).text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(shown))
            .into_any_element();
        Self::row(p, label, control)
    }

    fn slide_to(&mut self, d: SliderDrag, x: f32, cx: &mut Context<Self>) {
        let Some(b) = self.tracks.borrow().get(d.id).copied() else { return };
        let frac = ((x - f32::from(b.origin.x)) / f32::from(b.size.width).max(1.0)).clamp(0.0, 1.0);
        // Right-to-left sliders fill from the right.
        let frac = if mandara_ui::bidi::rtl() { 1.0 - frac } else { frac };
        let raw = d.min + (d.max - d.min) * frac;
        let v = (raw / d.step).round() * d.step;
        self.edit(cx, |c| (d.set)(c, v));
    }

    fn choice<T: Copy + PartialEq + 'static>(&self, p: &MandaraPalette, id: &'static str, label: String, value: T, options: Vec<(String, T)>, set: fn(&mut GreeterConfig, T), cx: &mut Context<Self>) -> AnyElement {
        let items = options
            .into_iter()
            .enumerate()
            .map(|(i, (name, v))| ChoiceItem::new((id, i), name, v == value, cx.listener(move |this, _: &ClickEvent, _, cx| this.edit(cx, |c| set(c, v)))))
            .collect();
        self.choice_row(p, label, items)
    }

    /// A labelled choice: beside its label when it fits on one line in the
    /// room there (the page's width less the card's padding and some room
    /// for the label, at most [`CHOICE_MAX_W_PX`]), else a balanced grid
    /// under the label across the whole row.
    fn choice_row(&self, p: &MandaraPalette, label: String, items: Vec<ChoiceItem>) -> AnyElement {
        let under = (self.width.get() - 2.0 * SPACE_14_PX).max(0.0);
        let beside = (under - SPACE_16_PX - CHOICE_LABEL_ROOM_PX).clamp(0.0, CHOICE_MAX_W_PX);
        match placed(items, beside, under, &GREETER_CHOICE, p) {
            (control, false) => Self::row(p, label, control),
            (control, true) => div()
                .px(px(SPACE_14_PX))
                .py(px(SPACE_6_PX))
                .row()
                .flex_col()
                .cross_start()
                .gap(px(SPACE_8_PX))
                .child(div().text_color(p.on_surface.hsla()).child(label))
                .child(control)
                .into_any_element(),
        }
    }

    /// A font from the searchable picker; empty = `fallback` (shown as
    /// "Default").
    fn font(&mut self, p: &MandaraPalette, id: &'static str, label: String, value: String, fallback: String, set: fn(&mut GreeterConfig, String), cx: &mut Context<Self>) -> AnyElement {
        let picker = match self.fonts.get(id) {
            Some(f) => f.clone(),
            None => {
                let this = cx.entity().downgrade();
                let f = cx.new(|_| super::font_picker::FontSelect::new(format!("greeter-font-{id}"), Rc::new(move |v, _, cx| {
                    let _ = this.update(cx, |this, cx| this.edit(cx, |c| set(c, v)));
                })));
                self.fonts.insert(id, f.clone());
                f
            }
        };
        picker.update(cx, |f, _| f.sync(&value, Some(&fallback)));
        Self::row(p, label, picker.into_any_element())
    }

    fn text(&mut self, p: &MandaraPalette, id: &'static str, label: String, value: String, placeholder: String, set: fn(&mut GreeterConfig, String), cx: &mut Context<Self>) -> AnyElement {
        let field = match self.fields.get(id) {
            Some(f) => f.clone(),
            None => {
                let this = cx.entity().downgrade();
                let f = cx.new(|cx| {
                    TextField::new(
                        value.clone(),
                        placeholder,
                        false,
                        Rc::new(move |v, _, cx| {
                            let _ = this.update(cx, |this, cx| this.edit(cx, |c| set(c, v)));
                        }),
                        cx,
                    )
                });
                self.fields.insert(id, f.clone());
                f
            }
        };
        field.update(cx, |f, _| f.sync(&value));
        Self::row(p, label, div().w(px(DIM_260_PX)).child(field).into_any_element())
    }

    fn button(p: &MandaraPalette, id: &'static str, label: String, ic: &'static str, primary: bool) -> gpui::Stateful<gpui::Div> {
        use mandara_ui::components::buttons::{Ink, designed};
        // The classic primary fades on hover; the designs deepen its fill.
        let ink = if primary { Ink { bg: Some(p.primary), hover: p.primary.mix(p.on_primary, ALPHA_8), outline: None } } else { Ink { bg: Some(p.surface_container_high), hover: p.surface_container, outline: None } };
        let id = gpui::ElementId::from(id);
        let el = div().id(id.clone()).h(px(DIM_32_PX)).px(px(SPACE_12_PX));
        designed(el, id, DIM_32_PX, false, ink, Some(p), |el| {
            el.rounded(px(RADIUS_11_PX))
                .when(primary, |el| el.bg(p.primary.hsla()).hover(|s| s.opacity(ALPHA_90)))
                .when(!primary, |el| el.bg(p.surface_container_high.hsla()).hover(|s| s.bg(p.surface_container.hsla())))
        })
            .row()
            .items_center()
            .gap(px(SPACE_7_PX))
            .cursor_pointer()
            .text_size(px(TEXT_12_5_PX))
            .font_weight(FontWeight::MEDIUM)
            .when(primary, |el| el.text_color(p.on_primary.hsla()))
            .child(icon(ic, ICON_15_PX, if primary { p.on_primary } else { p.on_surface }))
            .child(label)
    }

    // ── Groups ───────────────────────────────────────────────────────────

    /// Keep sign-in placement and public decorations in the same live layout preview.
    fn layout_group(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        // Element list (z order, top first) with visibility + order.
        let mut order = self.draft.layout.clone();
        order.sort_by_key(|pl| std::cmp::Reverse(pl.z));
        let mut rows = Vec::new();
        for pl in order {
            let e = pl.element;
            let selected = self.selected == Some(e);
            rows.push(
                div()
                    .id(("layout-row", e as usize))
                    .h(px(DIM_40_PX))
                    .px(px(SPACE_10_PX))
                    .mx(px(SPACE_4_PX))
                    .rounded(px(RADIUS_12_PX))
                    .row()
                    .items_center()
                    .gap(px(SPACE_10_PX))
                    .cursor_pointer()
                    .when(selected, |el| el.bg(p.primary.with_alpha(ALPHA_16).hsla()))
                    .hover(|s| s.bg(p.surface_container_high.hsla()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.selected = Some(e);
                        this.preview.update(cx, |v, cx| v.select(Some(e), cx));
                        cx.notify();
                    }))
                    .child(
                        div()
                            .id(("vis", e as usize))
                            .size(px(DIM_26_PX))
                            .rounded(px(RADIUS_8_PX))
                            .row()
                            .items_center()
                            .justify_center()
                            .hover(|s| s.bg(p.surface_container.hsla()))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.edit(cx, |c| {
                                    let pl = c.placement_mut(e);
                                    pl.visible = !pl.visible;
                                });
                            }))
                            .child(icon(if pl.visible { "check" } else { "close" }, ICON_14_PX, if pl.visible { p.primary } else { p.on_surface_variant })),
                    )
                    .child(div().flex_1().text_color(if pl.visible { p.on_surface.hsla() } else { p.on_surface_variant.hsla() }).child(element_label(e)))
                    .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(anchor_label(pl.anchor)))
                    .child(
                        div()
                            .id(("up", e as usize))
                            .size(px(DIM_26_PX))
                            .rounded(px(RADIUS_8_PX))
                            .row()
                            .items_center()
                            .justify_center()
                            .hover(|s| s.bg(p.surface_container.hsla()))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.edit(cx, |c| c.placement_mut(e).z += 1);
                            }))
                            .child(icon("plus", ICON_14_PX, p.on_surface_variant)),
                    )
                    .child(
                        div()
                            .id(("down", e as usize))
                            .size(px(DIM_26_PX))
                            .rounded(px(RADIUS_8_PX))
                            .row()
                            .items_center()
                            .justify_center()
                            .hover(|s| s.bg(p.surface_container.hsla()))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.edit(cx, |c| c.placement_mut(e).z -= 1);
                            }))
                            .child(icon("minus", ICON_14_PX, p.on_surface_variant)),
                    )
                    .into_any_element(),
            );
        }
        out.push(Self::card(p, tl("Elements — drag them on the preview; + / − change the stacking order"), rows));

        if let Some(e) = self.selected {
            let pl = self.draft.placement(e);
            // 9-point anchor picker.
            let mut grid = div().row().flex_wrap().w(px(DIM_96_PX)).gap(px(SPACE_4_PX));
            for a in Anchor::ALL {
                let on = a == pl.anchor;
                grid = grid.child(
                    div()
                        .id(("anchor", a as usize))
                        .size(px(DIM_28_PX))
                        .rounded(px(RADIUS_8_PX))
                        .row()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .bg(if on { p.primary.hsla() } else { p.surface_container_high.hsla() })
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.edit(cx, |c| {
                                let pl = c.placement_mut(e);
                                pl.anchor = a;
                                pl.x = 0.0;
                                pl.y = 0.0;
                            })
                        }))
                        .child(div().size(px(DIM_6_PX)).rounded_full().bg(if on { p.on_primary.hsla() } else { p.on_surface_variant.hsla() })),
                );
            }
            let sel = e;
            let mut rows = vec![
                Self::row(p, tl("Anchor"), grid.into_any_element()),
                self.slider(p, "el-x", tl("Horizontal offset"), pl.x, -1000.0, 1000.0, 1.0, " px", |c, v| (sel_ref(c)).x = v, cx),
                self.slider(p, "el-y", tl("Vertical offset"), pl.y, -600.0, 600.0, 1.0, " px", |c, v| (sel_ref(c)).y = v, cx),
                self.slider(p, "el-scale", tl("Scale"), pl.scale, 0.25, 3.0, 0.05, "×", |c, v| (sel_ref(c)).scale = v, cx),
                self.toggle(p, "el-visible", tl("Visible"), pl.visible, |c, v| (sel_ref(c)).visible = v, cx),
            ];
            if sel == Element::Banner {
                rows.push(self.text(p, "banner-text", tl("Message text"), self.draft.banner.text.clone(), tl("e.g. Authorized use only"), |c, v| c.banner.text = v, cx));
                rows.push(self.slider(p, "banner-size", tl("Message size"), self.draft.banner.size, 9.0, 40.0, 1.0, " px", |c, v| c.banner.size = v, cx));
            }
            if sel == Element::Keyboard {
                rows.push(self.text(p, "kb-layout", tl("Layout shown"), self.draft.keyboard.layout.clone(), tl("Detected automatically"), |c, v| c.keyboard.layout = v, cx));
            }
            SELECTED.with(|s| s.set(Some(sel)));
            out.push(Self::card(p, tlf("Selected: {element}", &[("element", &element_label(sel))]), rows));
        } else {
            out.push(Self::card(p, tl("Selection"), vec![Self::row(p, tl("Click an element on the preview or in the list to edit it."), div().into_any_element())]));
        }
        out.push(Self::card(
            p,
            tl("Layout"),
            vec![Self::row(
                p,
                tl("Start over from the default arrangement"),
                Self::button(p, "reset-layout", tl("Reset layout"), "undo", false)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.edit(cx, |c| c.layout = mandara_greeter::config::default_layout())))
                    .into_any_element(),
            )],
        ));
        let page = cx.entity().downgrade();
        out.push(super::auth_widgets::panel("login", &self.draft.widgets, Rc::new(move |edit, cx| {
            let _ = page.update(cx, |page, cx| page.edit(cx, |cfg| edit.apply(&mut cfg.widgets)));
        }), p, cx));
        out
    }

    fn background_group(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let b = self.draft.background.clone();
        let modes = vec![
            (tl("Auto"), BackgroundMode::Auto),
            (tl("Videos"), BackgroundMode::Video),
            (tl("Stills"), BackgroundMode::Stills),
            (tl("Aurora"), BackgroundMode::Aurora),
            (tl("Solid"), BackgroundMode::Solid),
        ];
        // Only what the chosen backdrop uses (Auto plays videos, else
        // stills, so it shows both).
        let videos = matches!(b.mode, BackgroundMode::Auto | BackgroundMode::Video);
        let stills = matches!(b.mode, BackgroundMode::Auto | BackgroundMode::Stills);
        let mut source = vec![
            self.toggle(p, "bg-user-wallpaper", tl("Each person's own wallpaper"), b.user_wallpaper, |c, v| c.background.user_wallpaper = v, cx),
            self.choice(p, "bg-mode", tl("Backdrop"), b.mode, modes, |c, v| c.background.mode = v, cx),
        ];
        if videos {
            source.push(self.choice(p, "bg-video", tl("Video decoding"), b.video, vec![(tl("Auto"), VideoMode::Auto), (tl("Lite"), VideoMode::Lite), (tl("Off"), VideoMode::Off)], |c, v| c.background.video = v, cx));
            source.push(self.text(p, "bg-video-dirs", tl("Video folders"), join(&b.video_dirs), tl("Separate folders with ;"), |c, v| c.background.video_dirs = split_paths(&v), cx));
        }
        if stills {
            source.push(self.text(p, "bg-image-dirs", tl("Image folders"), join(&b.image_dirs), tl("Separate folders with ;"), |c, v| c.background.image_dirs = split_paths(&v), cx));
        }
        if videos || stills {
            source.push(self.text(p, "bg-playlist", tl("Playlist order"), b.playlist.join("; "), tl("File names in order (optional)"), |c, v| c.background.playlist = v.split(';').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(), cx));
            source.push(self.toggle(p, "bg-shuffle", tl("Shuffle"), b.shuffle, |c, v| c.background.shuffle = v, cx));
        }
        if b.mode == BackgroundMode::Solid {
            source.push(self.text(p, "bg-solid", tl("Solid color"), b.solid.clone(), tl("#rrggbb (theme surface when empty)"), |c, v| c.background.solid = v, cx));
        }
        let mut timing = Vec::new();
        if videos {
            timing.push(self.slider(p, "bg-clip", tl("Seconds per clip"), b.clip_seconds as f32, 5.0, 300.0, 1.0, " s", |c, v| c.background.clip_seconds = v as u32, cx));
            timing.push(self.slider(p, "bg-xfade", tl("Clip crossfade"), b.crossfade_ms as f32, 0.0, 5000.0, 50.0, " ms", |c, v| c.background.crossfade_ms = v as u32, cx));
            timing.push(self.slider(p, "bg-rate", tl("Playback speed"), b.playback_rate, 0.25, 2.0, 0.05, "×", |c, v| c.background.playback_rate = v, cx));
        }
        if stills {
            timing.push(self.slider(p, "bg-still", tl("Seconds per image"), b.still_seconds as f32, 4.0, 120.0, 1.0, " s", |c, v| c.background.still_seconds = v as u32, cx));
            timing.push(self.slider(p, "bg-still-fade", tl("Image crossfade"), b.still_fade_ms as f32, 0.0, 6000.0, 50.0, " ms", |c, v| c.background.still_fade_ms = v as u32, cx));
            timing.push(self.slider(p, "bg-kb", tl("Ken Burns zoom"), b.ken_burns, 0.0, 0.4, 0.01, "", |c, v| c.background.ken_burns = v, cx));
        }
        let mut look = vec![
            self.slider(p, "bg-dim", tl("Dim"), b.dim, 0.0, 0.9, 0.01, "", |c, v| c.background.dim = v, cx),
            self.slider(p, "bg-vignette", tl("Vignette"), b.vignette, 0.0, 1.0, 0.01, "", |c, v| c.background.vignette = v, cx),
            self.slider(p, "bg-blur", tl("Blur"), b.blur, 0.0, 1.0, 0.01, "", |c, v| c.background.blur = v, cx),
            self.slider(p, "bg-grain", tl("Grain"), b.grain, 0.0, 1.0, 0.01, "", |c, v| c.background.grain = v, cx),
            self.text(p, "bg-tint", tl("Tint color"), b.tint.clone(), "#rrggbb".into(), |c, v| c.background.tint = v, cx),
        ];
        if !b.tint.trim().is_empty() {
            look.push(self.slider(p, "bg-tint-k", tl("Tint strength"), b.tint_strength, 0.0, 1.0, 0.01, "", |c, v| c.background.tint_strength = v, cx));
        }
        look.push(self.toggle(p, "bg-orbs", tl("Drifting light orbs"), b.orbs, |c, v| c.background.orbs = v, cx));
        if b.orbs {
            look.push(self.slider(p, "bg-orb-k", tl("Orb intensity"), b.orb_intensity, 0.0, 1.0, 0.01, "", |c, v| c.background.orb_intensity = v, cx));
        }
        look.push(self.slider(p, "bg-parallax", tl("Pointer parallax"), b.parallax, 0.0, 60.0, 1.0, " px", |c, v| c.background.parallax = v, cx));
        look.push(self.choice(p, "bg-overlay", tl("Overlay"), b.overlay, vec![(tl("None"), Overlay::None), (tl("HUD frame"), Overlay::Hud), (tl("Prism light"), Overlay::Prism), (tl("Horizon"), Overlay::Horizon)], |c, v| c.background.overlay = v, cx));
        let mut cards = vec![Self::card(p, tl("Source"), source)];
        if !timing.is_empty() {
            cards.push(Self::card(p, tl("Timing"), timing));
        }
        cards.push(Self::card(p, tl("Look"), look));
        cards
    }

    fn clock_group(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = self.draft.clock.clone();
        let d = self.draft.date.clone();
        let shell_fonts = mandara_ui::fonts::fonts(cx);
        let clock = vec![
            self.toggle(p, "clk-24", tl("24-hour clock"), c.format_24h, |c, v| c.clock.format_24h = v, cx),
            self.toggle(p, "clk-sec", tl("Show seconds"), c.seconds, |c, v| c.clock.seconds = v, cx),
            self.toggle(p, "clk-roll", tl("Rolling digits"), c.roll_digits, |c, v| c.clock.roll_digits = v, cx),
            self.font(p, "clk-font", tl("Font"), c.font.clone(), shell_fonts.display.to_string(), |c, v| c.clock.font = v, cx),
            self.slider(p, "clk-size", tl("Size"), c.size, 40.0, 320.0, 1.0, " px", |c, v| c.clock.size = v, cx),
            self.slider(p, "clk-weight", tl("Weight"), c.weight as f32, 100.0, 900.0, 100.0, "", |c, v| c.clock.weight = v as u16, cx),
            self.slider(p, "clk-idle-weight", tl("Weight while idle (0 = the same)"), c.idle_weight as f32, 0.0, 900.0, 50.0, "", |c, v| c.clock.idle_weight = v as u16, cx),
            self.slider(p, "clk-opacity", tl("Opacity"), c.opacity, 0.2, 1.0, 0.01, "", |c, v| c.clock.opacity = v, cx),
            self.toggle(p, "clk-stack", tl("Hours over minutes"), c.stacked, |c, v| c.clock.stacked = v, cx),
            self.toggle(p, "clk-tint", tl("Hours in the accent colour"), c.tint, |c, v| c.clock.tint = v, cx),
            self.choice(p, "clk-face", tl("Clock face"), c.face, vec![(tl("Digits"), ClockFace::Digits), (tl("Day ruler"), ClockFace::Ruler), (tl("Halo ring"), ClockFace::Halo), (tl("Vast, behind everything"), ClockFace::Vast)], |c, v| c.clock.face = v, cx),
        ];
        let date = vec![
            self.text(p, "date-fmt", tl("Format"), d.format.clone(), tl("strftime, e.g. %A %-d %B (empty = language default)"), |c, v| c.date.format = v, cx),
            self.font(p, "date-font", tl("Font"), d.font.clone(), shell_fonts.ui.to_string(), |c, v| c.date.font = v, cx),
            self.slider(p, "date-size", tl("Size"), d.size, 10.0, 60.0, 1.0, " px", |c, v| c.date.size = v, cx),
            self.slider(p, "date-weight", tl("Weight"), d.weight as f32, 100.0, 900.0, 100.0, "", |c, v| c.date.weight = v as u16, cx),
            self.slider(p, "date-opacity", tl("Opacity"), d.opacity, 0.2, 1.0, 0.01, "", |c, v| c.date.opacity = v, cx),
        ];
        vec![Self::card(p, tl("Clock"), clock), Self::card(p, tl("Date"), date)]
    }

    fn user_group(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let u = self.draft.user.clone();
        let pw = self.draft.password.clone();
        let mut user = vec![
            self.choice(p, "usr-list", tl("Users"), u.list, vec![(tl("Everyone"), UserList::All), (tl("Last user"), UserList::LastUser), (tl("Type a name"), UserList::Manual)], |c, v| c.user.list = v, cx),
            self.choice(p, "usr-shape", tl("Avatar shape"), u.avatar_shape, vec![(tl("Circle"), AvatarShape::Circle), (tl("Squircle"), AvatarShape::Squircle), (tl("Rounded"), AvatarShape::Rounded), (tl("No frame"), AvatarShape::Bare), (tl("Scalloped"), AvatarShape::Cookie)], |c, v| c.user.avatar_shape = v, cx),
            self.choice(p, "usr-src", tl("Avatar"), u.avatar_source, vec![(tl("Picture"), AvatarSource::Auto), (tl("Initials"), AvatarSource::Monogram), (tl("Icon"), AvatarSource::Icon)], |c, v| c.user.avatar_source = v, cx),
            self.slider(p, "usr-size", tl("Avatar size"), u.avatar_size, 48.0, 240.0, 1.0, " px", |c, v| c.user.avatar_size = v, cx),
            self.toggle(p, "usr-ring", tl("Breathing glow ring"), u.glow_ring, |c, v| c.user.glow_ring = v, cx),
            self.toggle(p, "usr-name", tl("Show name"), u.show_name, |c, v| c.user.show_name = v, cx),
            self.choice(p, "usr-switcher", tl("Other users as"), u.switcher, vec![(tl("Faces"), UserSwitcher::Faces), (tl("Chips"), UserSwitcher::Chips)], |c, v| c.user.switcher = v, cx),
        ];
        if u.show_name {
            user.push(self.slider(p, "usr-name-size", tl("Name size"), u.name_size, 10.0, 48.0, 1.0, " px", |c, v| c.user.name_size = v, cx));
        }
        user.extend([
            self.text(p, "usr-hidden", tl("Hidden users"), u.hidden.join(", "), tl("Login names, comma-separated"), |c, v| c.user.hidden = v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(), cx),
        ]);
        let card = self.draft.card.clone();
        let mut card_rows = vec![self.toggle(p, "card-on", tl("Put the sign-in on a card"), card.enabled, |c, v| c.card.enabled = v, cx)];
        if card.enabled {
            card_rows.push(self.choice(p, "card-style", tl("Card style"), card.style, vec![(tl("Plain"), CardStyle::Plain), (tl("Wireframe"), CardStyle::Hud), (tl("Double bezel"), CardStyle::Bezel), (tl("On a line"), CardStyle::Horizon)], |c, v| c.card.style = v, cx));
            card_rows.extend([
                self.slider(p, "card-w", tl("Card width"), card.width, 280.0, 720.0, 2.0, " px", |c, v| c.card.width = v, cx),
                self.slider(p, "card-pad", tl("Padding"), card.padding, 8.0, 48.0, 1.0, " px", |c, v| c.card.padding = v, cx),
                self.slider(p, "card-r", tl("Corner radius"), card.radius, 0.0, 48.0, 1.0, " px", |c, v| c.card.radius = v, cx),
                self.text(p, "card-fill", tl("Fill color"), card.fill.clone(), tl("#rrggbb (theme surface when empty)"), |c, v| c.card.fill = v, cx),
                self.slider(p, "card-op", tl("Fill opacity"), card.opacity, 0.0, 1.0, 0.01, "", |c, v| c.card.opacity = v, cx),
                self.slider(p, "card-bd", tl("Border"), card.border, 0.0, 4.0, 0.5, " px", |c, v| c.card.border = v, cx),
                self.text(p, "card-bdc", tl("Border color"), card.border_color.clone(), tl("#rrggbb (theme outline when empty)"), |c, v| c.card.border_color = v, cx),
                self.toggle(p, "card-back", tl("Back button to the user switcher"), card.back_button, |c, v| c.card.back_button = v, cx),
                self.toggle(p, "card-sess", tl("Session picker inside the card"), card.session_inside, |c, v| c.card.session_inside = v, cx),
            ]);
        }
        let password = vec![
            self.choice(p, "pw-style", tl("Style"), pw.style, vec![(tl("Glass"), FieldStyle::Glass), (tl("Solid"), FieldStyle::Solid), (tl("Inset"), FieldStyle::Inset), (tl("Outline"), FieldStyle::Outline), (tl("Underline"), FieldStyle::Underline), (tl("Filled"), FieldStyle::Filled), (tl("Terminal"), FieldStyle::Terminal), (tl("Recessed"), FieldStyle::Recessed)], |c, v| c.password.style = v, cx),
            self.choice(p, "pw-submit", tl("Sign-in button"), pw.submit, vec![(tl("In the field"), SubmitButton::Inside), (tl("Beside it"), SubmitButton::Beside), (tl("None (Enter)"), SubmitButton::Hidden), (tl("A pill below"), SubmitButton::Pill)], |c, v| c.password.submit = v, cx),
            self.choice(p, "pw-mask", tl("Hide typing as"), pw.mask, vec![(tl("Shapes"), PasswordMask::Shapes), (tl("Dots"), PasswordMask::Dots)], |c, v| c.password.mask = v, cx),
            self.toggle(p, "pw-accent", tl("Accent border while focused"), pw.accent_border, |c, v| c.password.accent_border = v, cx),
            self.slider(p, "pw-w", tl("Width"), pw.width, 180.0, 560.0, 1.0, " px", |c, v| c.password.width = v, cx),
            self.slider(p, "pw-h", tl("Height"), pw.height, 30.0, 72.0, 1.0, " px", |c, v| c.password.height = v, cx),
            self.slider(p, "pw-text", tl("Text size"), pw.text_size, 10.0, 24.0, 0.5, " px", |c, v| c.password.text_size = v, cx),
            self.text(p, "pw-ph", tl("Placeholder"), pw.placeholder.clone(), tl("Enter password"), |c, v| c.password.placeholder = v, cx),
            self.toggle(p, "pw-caps", tl("Caps Lock hint"), pw.caps_hint, |c, v| c.password.caps_hint = v, cx),
            self.toggle(p, "pw-reveal", tl("Show / hide button"), pw.reveal_toggle, |c, v| c.password.reveal_toggle = v, cx),
            self.toggle(p, "pw-ripple", tl("Keystroke ripples"), pw.keystroke_ripple, |c, v| c.password.keystroke_ripple = v, cx),
            self.toggle(p, "pw-glow", tl("Glow while typing"), pw.glow, |c, v| c.password.glow = v, cx),
        ];
        vec![Self::card(p, tl("Card"), card_rows), Self::card(p, tl("User"), user), Self::card(p, tl("Password field"), password)]
    }

    fn session_group(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let s = self.draft.session.clone();
        let pw = self.draft.power.clone();
        let mut sessions: Vec<(String, String)> = vec![(tl("First"), String::new())];
        sessions.extend(mandara_greeter::sessions::list_sessions(mandara_i18n::language().posix()).into_iter().map(|s| (s.name, s.id)));
        let session_id = s.default.clone();
        let items = sessions
            .into_iter()
            .enumerate()
            .map(|(i, (name, id))| {
                let on = id == session_id;
                ChoiceItem::new(
                    ("sess", i),
                    name,
                    on,
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let id = id.clone();
                        this.edit(cx, move |c| c.session.default = id)
                    }),
                )
            })
            .collect();
        let session = vec![
            self.choice_row(p, tl("Default session"), items),
            self.toggle(p, "sess-x11", tl("List X11 sessions"), s.x11, |c, v| c.session.x11 = v, cx),
            self.choice(p, "sess-style", tl("Picker style"), s.style, vec![(tl("Pill"), SessionStyle::Pill), (tl("Full-width field"), SessionStyle::Field), (tl("Chip"), SessionStyle::Chip)], |c, v| c.session.style = v, cx),
        ];
        let power = vec![
            self.toggle(p, "pow-sleep", tl("Sleep"), pw.suspend, |c, v| c.power.suspend = v, cx),
            self.toggle(p, "pow-reboot", tl("Restart"), pw.reboot, |c, v| c.power.reboot = v, cx),
            self.toggle(p, "pow-off", tl("Shut Down"), pw.poweroff, |c, v| c.power.poweroff = v, cx),
            self.toggle(p, "pow-labels", tl("Labels under buttons"), pw.labels, |c, v| c.power.labels = v, cx),
            self.choice(p, "pow-style", tl("Button style"), pw.style, vec![(tl("Soft"), PowerStyle::Soft), (tl("Outlined"), PowerStyle::Outlined), (tl("Tonal"), PowerStyle::Tonal)], |c, v| c.power.style = v, cx),
            self.slider(p, "pow-size", tl("Button size"), pw.size, 28.0, 72.0, 1.0, " px", |c, v| c.power.size = v, cx),
        ];
        let logo = self.draft.logo.clone();
        let logo_on = self.draft.placement(Element::Logo).visible;
        let mut logo_rows = vec![self.toggle(p, "logo-on", tl("Show a logo"), logo_on, |c, v| c.placement_mut(Element::Logo).visible = v, cx)];
        if logo_on {
            logo_rows.extend([
                self.text(p, "logo-path", tl("Image"), logo.path.clone(), tl("Path to a PNG / SVG (Mandara's emblem when empty)"), |c, v| c.logo.path = v, cx),
                self.slider(p, "logo-size", tl("Size"), logo.size, 16.0, 200.0, 1.0, " px", |c, v| c.logo.size = v, cx),
                self.slider(p, "logo-op", tl("Opacity"), logo.opacity, 0.1, 1.0, 0.01, "", |c, v| c.logo.opacity = v, cx),
                self.toggle(p, "logo-accent", tl("Emblem in the accent colour"), logo.accent, |c, v| c.logo.accent = v, cx),
            ]);
        }
        let switcher = self.draft.theme.switcher.join(", ");
        let schemes_on = self.draft.placement(Element::Themes).visible;
        let mut schemes = vec![self.toggle(p, "th-switch-on", tl("Let people switch colour schemes"), schemes_on, |c, v| c.placement_mut(Element::Themes).visible = v, cx)];
        if schemes_on {
            schemes.push(self.text(p, "th-switcher", tl("Schemes offered"), switcher, tl("Theme ids, comma-separated (a few of Mandara's when empty)"), |c, v| c.theme.switcher = v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(), cx));
        }
        let mut langs = vec![(tl("Auto"), "auto")];
        langs.extend(mandara_i18n::Lang::ALL.iter().map(|l| (l.native_name().to_string(), l.code())));
        let lang = self.draft.language.clone();
        let items = langs
            .into_iter()
            .enumerate()
            .map(|(i, (name, code))| ChoiceItem::new(("lang", i), name, lang == code, cx.listener(move |this, _: &ClickEvent, _, cx| this.edit(cx, |c| c.language = code.to_string()))))
            .collect();
        vec![
            Self::card(p, tl("Session"), session),
            Self::card(p, tl("Power buttons"), power),
            Self::card(p, tl("Logo"), logo_rows),
            Self::card(p, tl("Colour scheme switcher"), schemes),
            Self::card(p, tl("Language"), vec![self.choice_row(p, tl("Login screen language"), items)]),
        ]
    }

    /// Ready-made looks: each applies a whole design in one click (who is
    /// listed, sessions, language and folders stay).
    fn looks_group(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let current = self.draft.current_look();
        let fit = super::grid::fit(self.width.get() - 2.0 * SPACE_10_PX, LOOK_GRID).at_most(LOOK_COLUMNS).balanced(LOOKS.len());
        let thumb_w = (fit.card_w - 2.0 * (SPACE_8_PX + BORDER_2_PX)).max(1.0);
        let thumb_h = (thumb_w * LOOK_THUMB_ASPECT).floor();
        let mut cards: Vec<AnyElement> = Vec::new();
        for (i, (id, name, summary)) in LOOKS.into_iter().enumerate() {
            let mut probe = self.draft.clone();
            probe.apply_look(id);
            // Meridian keeps the light colours as they are.
            if id == "meridian" && current == Some(id) {
                probe.theme.light = self.draft.theme.light;
            }
            let pal = mandara_greeter::view::palette_for(&probe);
            let on = current == Some(id);
            cards.push(
                div()
                    .id(("look", i))
                    .h_full()
                    .p(px(SPACE_8_PX))
                    .rounded(px(RADIUS_14_PX))
                    .cursor_pointer()
                    .border_2()
                    .border_color(if on { p.primary.hsla() } else { gpui::transparent_black() })
                    .bg(p.surface_container_high.hsla())
                    .hover(|s| s.bg(p.surface_container.hsla()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.selected = None;
                        this.edit(cx, move |c| {
                            c.apply_look(id);
                        })
                    }))
                    .child(look_thumb::thumb(&probe, &pal, thumb_w, thumb_h))
                    .child(div().pt(px(SPACE_8_PX)).row().items_center().gap(px(SPACE_6_PX)).child(div().text_size(px(TEXT_13_PX)).font_weight(gpui::FontWeight::SEMIBOLD).child(tl(name))).when(on, |el| el.child(div().text_size(px(TEXT_11_PX)).text_color(p.primary.hsla()).child(tl("In use")))))
                    .child(div().pt(px(SPACE_3_PX)).text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(tl(summary)))
                    .into_any_element(),
            );
        }
        let gallery = div().p(px(SPACE_10_PX)).child(fit.render(cards));
        vec![Self::card(p, tl("Start from a look, then change anything in the other tabs"), vec![gallery.into_any_element()])]
    }

    fn theme_group(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = self.draft.theme.clone();
        // Inside the group card, less the gallery's own padding.
        let fit = super::grid::fit(self.width.get() - 2.0 * SPACE_10_PX, GREETER_THEME_GRID);
        let swatch_h = fit.aspect_h(GREETER_SWATCH_ASPECT, SPACE_6_PX + BORDER_2_PX);
        let mut swatches: Vec<AnyElement> = Vec::new();
        let themes = self.themes.get_or_insert_with(|| mandara_theme::catalog::all().into_iter().map(|def| { let pal = def.palette(true); (def, pal) }).collect()).clone();
        for (i, (def, pal)) in themes.iter().enumerate() {
            let pal = pal.clone();
            let on = def.id == t.preset;
            let id = def.id.clone();
            swatches.push(
                div()
                    .id(("theme", i))
                    .flex_1()
                    .p(px(SPACE_6_PX))
                    .rounded(px(RADIUS_14_PX))
                    .cursor_pointer()
                    .border_2()
                    .border_color(if on { p.primary.hsla() } else { gpui::transparent_black() })
                    .bg(p.surface_container_high.hsla())
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let id = id.clone();
                        this.edit(cx, move |c| c.theme.preset = id)
                    }))
                    .child(
                        div()
                            .h(px(swatch_h))
                            .rounded(px(RADIUS_10_PX))
                            .bg(mandara_core::oklab_gradient(SWATCH_GRADIENT_DEG, pal.surface_dim, pal.primary.with_lightness(SWATCH_ACCENT_LIGHTNESS)))
                            .row()
                            .items_end()
                            .gap(px(SPACE_4_PX))
                            .p(px(SPACE_6_PX))
                            .child(div().size(px(DIM_10_PX)).rounded_full().bg(pal.primary.hsla()))
                            .child(div().size(px(DIM_10_PX)).rounded_full().bg(pal.secondary.hsla()))
                            .child(div().size(px(DIM_10_PX)).rounded_full().bg(pal.tertiary.hsla())),
                    )
                    .child(div().pt(px(SPACE_5_PX)).text_size(px(TEXT_11_5_PX)).truncate().child(def.name.clone()))
                    .into_any_element(),
            );
        }
        let gallery = div().p(px(SPACE_10_PX)).child(fit.render(swatches));
        let ui_font = self.draft.font.clone();
        let shell_ui = mandara_ui::fonts::fonts(cx).ui.to_string();
        let glass = vec![
            self.font(p, "th-font", tl("Text font"), ui_font, shell_ui, |c, v| c.font = v, cx),
            self.toggle(p, "th-dynamic", tl("Colours from the wallpaper"), t.from_wallpaper, |c, v| c.theme.from_wallpaper = v, cx),
            self.toggle(p, "th-light", tl("Light colours"), t.light, |c, v| c.theme.light = v, cx),
            self.text(p, "th-accent", tl("Accent color"), t.accent.clone(), tl("#rrggbb (theme primary when empty)"), |c, v| c.theme.accent = v, cx),
            self.slider(p, "th-glass", tl("Glass opacity"), t.glass, 0.0, 0.8, 0.01, "", |c, v| c.theme.glass = v, cx),
            self.slider(p, "th-rim", tl("Specular rim"), t.rim, 0.0, 1.0, 0.01, "", |c, v| c.theme.rim = v, cx),
            self.slider(p, "th-round", tl("Roundness"), t.roundness, 0.0, 1.0, 0.01, "", |c, v| c.theme.roundness = v, cx),
        ];
        vec![Self::card(p, tl("Theme"), vec![gallery.into_any_element()]), Self::card(p, tl("Glass"), glass)]
    }

    fn motion_group(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let m = self.draft.motion.clone();
        let mut entrance = vec![self.toggle(p, "mo-enter", tl("Staggered entrance"), m.entrance, |c, v| c.motion.entrance = v, cx)];
        if m.entrance {
            entrance.extend([
                self.slider(p, "mo-stagger", tl("Stagger"), m.stagger_ms as f32, 0.0, 300.0, 5.0, " ms", |c, v| c.motion.stagger_ms = v as u32, cx),
                self.slider(p, "mo-enter-k", tl("Entrance stiffness"), m.entrance_spring.stiffness, 20.0, 600.0, 5.0, "", |c, v| c.motion.entrance_spring.stiffness = v, cx),
                self.slider(p, "mo-enter-d", tl("Entrance damping"), m.entrance_spring.damping, 4.0, 60.0, 0.5, "", |c, v| c.motion.entrance_spring.damping = v, cx),
                self.slider(p, "mo-pop-k", tl("Avatar pop stiffness"), m.pop_spring.stiffness, 40.0, 900.0, 5.0, "", |c, v| c.motion.pop_spring.stiffness = v, cx),
                self.slider(p, "mo-pop-d", tl("Avatar pop damping"), m.pop_spring.damping, 4.0, 60.0, 0.5, "", |c, v| c.motion.pop_spring.damping = v, cx),
            ]);
        }
        let idle = vec![
            self.toggle(p, "mo-start-idle", tl("Start on the idle clock"), m.start_idle, |c, v| c.motion.start_idle = v, cx),
            self.slider(p, "mo-idle", tl("Back to idle after"), m.idle_seconds as f32, 0.0, 300.0, 5.0, " s", |c, v| c.motion.idle_seconds = v as u32, cx),
            self.slider(p, "mo-idle-k", tl("Wake stiffness"), m.idle_spring.stiffness, 20.0, 600.0, 5.0, "", |c, v| c.motion.idle_spring.stiffness = v, cx),
            self.slider(p, "mo-idle-d", tl("Wake damping"), m.idle_spring.damping, 4.0, 60.0, 0.5, "", |c, v| c.motion.idle_spring.damping = v, cx),
        ];
        let mut feedback = vec![self.toggle(p, "mo-shake", tl("Shake on a wrong password"), m.shake, |c, v| c.motion.shake = v, cx)];
        if m.shake {
            feedback.extend([
                self.slider(p, "mo-shake-k", tl("Shake strength"), m.shake_strength, 0.2, 2.5, 0.05, "×", |c, v| c.motion.shake_strength = v, cx),
                self.slider(p, "mo-shake-s", tl("Shake stiffness"), m.shake_spring.stiffness, 200.0, 2500.0, 10.0, "", |c, v| c.motion.shake_spring.stiffness = v, cx),
                self.slider(p, "mo-shake-d", tl("Shake damping"), m.shake_spring.damping, 4.0, 60.0, 0.5, "", |c, v| c.motion.shake_spring.damping = v, cx),
            ]);
        }
        feedback.extend([
            self.slider(p, "mo-succ-k", tl("Success stiffness"), m.success_spring.stiffness, 10.0, 300.0, 1.0, "", |c, v| c.motion.success_spring.stiffness = v, cx),
            self.slider(p, "mo-succ-d", tl("Success damping"), m.success_spring.damping, 4.0, 60.0, 0.5, "", |c, v| c.motion.success_spring.damping = v, cx),
            self.slider(p, "mo-zoom", tl("Success zoom"), m.success_zoom, 0.0, 0.4, 0.01, "", |c, v| c.motion.success_zoom = v, cx),
        ]);
        vec![Self::card(p, tl("Entrance"), entrance), Self::card(p, tl("Idle & wake"), idle), Self::card(p, tl("Feedback"), feedback)]
    }

    fn a11y_group(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let a = self.draft.accessibility.clone();
        vec![Self::card(
            p,
            tl("Accessibility"),
            vec![
                self.toggle(p, "a11y-motion", tl("Reduce motion"), a.reduced_motion, |c, v| c.accessibility.reduced_motion = v, cx),
                self.toggle(p, "a11y-large", tl("Larger text"), a.large_text, |c, v| c.accessibility.large_text = v, cx),
                self.toggle(p, "a11y-contrast", tl("High contrast"), a.high_contrast, |c, v| c.accessibility.high_contrast = v, cx),
            ],
        )]
    }
}

thread_local! {
    /// The element the "Selected" sliders edit (fn-pointer setters can't
    /// capture it).
    static SELECTED: std::cell::Cell<Option<Element>> = const { std::cell::Cell::new(None) };
}

/// Why Mandara Login can't be applied here: FreeBSD has no greetd package,
/// so until greetd is built from source the login screen stays locked.
#[cfg(target_os = "freebsd")]
fn greetd_missing() -> Option<String> {
    let installed = mandara_greeter::sessions::program_exists("greetd") || ["/usr/local/sbin/greetd", "/usr/local/bin/greetd"].iter().any(|p| std::path::Path::new(p).exists());
    (!installed).then(|| mandara_i18n::t("greetd isn't packaged for FreeBSD: build and install it from source first (Mandara's login manager guide has the steps), then apply").to_string())
}

#[cfg(not(target_os = "freebsd"))]
fn greetd_missing() -> Option<String> {
    None
}

fn sel_ref(c: &mut GreeterConfig) -> &mut mandara_greeter::config::Placement {
    let e = SELECTED.with(|s| s.get()).unwrap_or(Element::Clock);
    c.placement_mut(e)
}

fn anchor_label(a: Anchor) -> String {
    match a {
        Anchor::TopLeft => tl("Top left"),
        Anchor::Top => tl("Top"),
        Anchor::TopRight => tl("Top right"),
        Anchor::Left => tl("Left"),
        Anchor::Center => tl("Center"),
        Anchor::Right => tl("Right"),
        Anchor::BottomLeft => tl("Bottom left"),
        Anchor::Bottom => tl("Bottom"),
        Anchor::BottomRight => tl("Bottom right"),
    }
}

fn join(paths: &[std::path::PathBuf]) -> String {
    paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("; ")
}

fn split_paths(v: &str) -> Vec<std::path::PathBuf> {
    v.split(';').map(str::trim).filter(|s| !s.is_empty()).map(std::path::PathBuf::from).collect()
}

impl Render for GreeterPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = mandara_core::frame_palette(window, cx);
        // The page column's width, known from the window this frame (the
        // canvas below confirms it after layout).
        self.width.set(super::grid::PageMetrics::of(window, cx).column_w);
        // Preview size: the page's width (less the column beside it), 16:9,
        // at most 40 % of the window's height.
        let vh = f32::from(window.viewport_size().height);
        let side_by_side = self.width.get() >= SIDE_BY_SIDE_MIN_W_PX;
        let avail = (self.width.get() - if side_by_side { SIDE_W_PX + SPACE_16_PX } else { 0.0 }).max(PREVIEW_MIN_W_PX);
        let pw = avail.min((vh * PREVIEW_MAX_SHARE) * PREVIEW_ASPECT_W / PREVIEW_ASPECT_H).floor();
        let ph = (pw * PREVIEW_ASPECT_H / PREVIEW_ASPECT_W).floor();
        self.preview.update(cx, |v, cx| v.set_preview_size(pw, ph, cx));
        if let Some(e) = self.selected {
            SELECTED.with(|s| s.set(Some(e)));
        }

        // Try it: the preview plays a moment of the login.
        let chip = |id: &'static str, label: String, ic: &'static str, on: bool| {
            div()
                .id(id)
                .h(px(DIM_28_PX))
                .px(px(SPACE_10_PX))
                .row()
                .items_center()
                .gap(px(SPACE_6_PX))
                .rounded_full()
                .cursor_pointer()
                .text_size(px(TEXT_12_PX))
                .when(on, |el| el.bg(p.primary.with_alpha(ALPHA_16).hsla()).text_color(p.primary.hsla()))
                .when(!on, |el| el.bg(p.surface_container.hsla()).hover(|s| s.bg(p.surface_container_high.hsla())))
                .child(icon(ic, ICON_13_PX, if on { p.primary } else { p.on_surface_variant }))
                .child(label)
        };
        let sim = |id: &'static str, label: String, ic: &'static str, what: Simulate, cx: &mut Context<Self>| {
            chip(id, label, ic, false).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.preview.update(cx, |v, cx| v.simulate(what, cx))))
        };
        let idle = self.idle;
        let try_it = div()
            .row()
            .flex_wrap()
            .gap(px(SPACE_6_PX))
            .child(sim("sim-enter", tl("Entrance"), "refresh", Simulate::Entrance, cx))
            .child(sim("sim-type", tl("Typing"), "keyboard", Simulate::Typing, cx))
            .child(sim("sim-error", tl("Wrong password"), "close", Simulate::Error, cx))
            .child(sim("sim-ok", tl("Success"), "check", Simulate::Success, cx))
            .child(chip("sim-idle", if idle { tl("Show login") } else { tl("Show idle clock") }, "timer", idle).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.idle = !this.idle;
                let idle = this.idle;
                this.preview.update(cx, |v, cx| v.set_preview_idle(idle, cx));
                cx.notify();
            })));
        let apply = Self::button(&p, "apply", if self.applying { tl("Applying…") } else { tl("Apply to login screen") }, "lock", true).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.apply_system(cx)));
        let full = Self::button(&p, "full", tl("Preview full screen"), "maximize", false).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
            this.preview_full_screen();
            cx.notify();
        }));
        let reset = div()
            .id("reset")
            .row()
            .items_center()
            .gap(px(SPACE_6_PX))
            .cursor_pointer()
            .text_size(px(TEXT_12_PX))
            .text_color(p.on_surface_variant.hsla())
            .hover(|s| s.text_color(p.on_surface.hsla()))
            .child(icon("undo", ICON_13_PX, p.on_surface_variant))
            .child(tl("Reset to defaults"))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.selected = None;
                this.edit(cx, |c| *c = GreeterConfig::default());
            }));
        let message = self.message.clone().map(|(ok, msg)| {
            div()
                .px(px(SPACE_10_PX))
                .py(px(SPACE_8_PX))
                .rounded(px(RADIUS_12_PX))
                .text_size(px(TEXT_12_PX))
                .bg(if ok { p.success_emerald.with_alpha(ALPHA_16).hsla() } else { p.urgent_coral.with_alpha(ALPHA_16).hsla() })
                .child(SharedString::from(msg))
        });
        let caption = |text: String| div().text_size(px(TEXT_11_5_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface_variant.hsla()).child(text);
        let preview = div()
            .flex_none()
            .rounded(px(RADIUS_16_PX))
            .overflow_hidden()
            .border_1()
            .border_color(p.outline.with_alpha(ALPHA_35).hsla())
            .shadow_lg()
            .child(self.preview.clone().cached(StyleRefinement::default().w(px(pw)).h(px(ph))));
        let top = if side_by_side {
            // Preview, and beside it what you do with it.
            let side = div()
                .w(px(SIDE_W_PX))
                .h(px(ph))
                .flex_none()
                .row()
                .flex_col()
                .gap(px(SPACE_8_PX))
                .child(apply.w_full().justify_center())
                .child(full.w_full().justify_center())
                .children(message)
                .child(div().pt(px(SPACE_8_PX)).child(caption(tl("Try it"))))
                .child(try_it)
                .child(div().flex_1())
                .child(reset);
            div().row().gap(px(SPACE_16_PX)).child(preview).child(side).into_any_element()
        } else {
            div()
                .row()
                .flex_col()
                .gap(px(SPACE_10_PX))
                .child(div().row().justify_center().child(preview))
                .child(div().row().flex_wrap().items_center().gap(px(SPACE_6_PX)).child(apply).child(full).child(div().flex_1()).child(reset))
                .children(message)
                .child(try_it)
                .into_any_element()
        };
        let width = self.width.clone();
        let height = (vh - PAGE_CHROME_H_PX).max(PAGE_MIN_H_PX);
        div()
            .relative()
            .w_full()
            .h(px(height))
            .row()
            .flex_col()
            .gap(px(SPACE_12_PX))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                if let Some(d) = this.slider {
                    this.slide_to(d, f32::from(e.position.x), cx);
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| {
                if this.slider.take().is_some() && this.dirty {
                    this.save();
                    cx.notify();
                }
            }))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| {
                if this.slider.take().is_some() && this.dirty {
                    this.save();
                    cx.notify();
                }
            }))
            .child(canvas(move |b, _, _| width.set(f32::from(b.size.width)), |_, _, _, _| {}).absolute().top_0().left_0().w_full().h(px(1.0)))
            .child(top)
            .child(self.controls.clone().cached(StyleRefinement::default().w_full().flex_1().min_h(px(0.0))))
    }
}

impl GreeterPage {
    /// The group tabs and the selected group's controls (rendered inside
    /// [`GreeterControls`]).
    fn controls(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = mandara_core::frame_palette(window, cx);
        let groups = Group::ALL;
        // One slim row of tabs, underlined (it scrolls sideways when narrow).
        let mut tabs = div().id("greeter-tabs").row().overflow_x_scroll().border_b_1().border_color(p.outline.with_alpha(ALPHA_35).hsla());
        for g in groups {
            let on = g == self.group;
            tabs = tabs.child(
                div()
                    .id(("group", g as usize))
                    .relative()
                    .flex_none()
                    .h(px(DIM_36_PX))
                    .px(px(SPACE_10_PX))
                    .row()
                    .items_center()
                    .gap(px(SPACE_6_PX))
                    .cursor_pointer()
                    .text_size(px(TEXT_13_PX))
                    .when(on, |el| el.text_color(p.on_surface.hsla()).font_weight(FontWeight::SEMIBOLD))
                    .when(!on, |el| el.text_color(p.on_surface_variant.hsla()).hover(|s| s.text_color(p.on_surface.hsla())))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.group = g;
                        cx.notify();
                    }))
                    .child(icon(g.icon(), ICON_14_PX, if on { p.primary } else { p.on_surface_variant }))
                    .child(g.label())
                    .when(on, |el| el.child(div().absolute().bottom_0().left(px(SPACE_8_PX)).right(px(SPACE_8_PX)).h(px(TAB_UNDERLINE_PX)).rounded_full().bg(p.primary.hsla()))),
            );
        }
        let body = match self.group {
            Group::Looks => self.looks_group(&p, cx),
            Group::Manager => self.manager_group(&p, cx),
            Group::Layout => self.layout_group(&p, cx),
            Group::Background => self.background_group(&p, cx),
            Group::Clock => self.clock_group(&p, cx),
            Group::User => self.user_group(&p, cx),
            Group::Session => self.session_group(&p, cx),
            Group::Theme => self.theme_group(&p, cx),
            Group::Motion => {
                let mut cards = self.motion_group(&p, cx);
                cards.extend(self.a11y_group(&p, cx));
                cards
            }
        };

        div()
            .size_full()
            .row()
            .flex_col()
            .gap(px(SPACE_12_PX))
            .child(tabs)
            .child({
                let (scroll, bar) = mandara_ui::scrollbar::keyed("greeter-controls");
                let controls = div().id("greeter-controls").flex_1().min_h(px(0.0)).overflow_y_scroll().track_scroll(&scroll).child(div().row().flex_col().gap(px(SPACE_16_PX)).pb(px(SPACE_24_PX)).children(body));
                mandara_ui::scrollbar::with_scrollbar(controls, &scroll, &bar, &p).flex_1().min_h(px(0.0)).flex().flex_col()
            })
            .into_any_element()
    }
}
