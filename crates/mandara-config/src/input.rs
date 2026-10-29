//! Input devices (keyboard, mouse, touchpad, cursor) and their emitters.
//!
//! Like the rest of `[compositor]`, every option is optional: `None` keeps
//! whatever the user's own compositor config says, so Mandara's drop-in only
//! carries what was changed in Studio → Input. Each compositor gets the
//! option in its own dialect:
//!
//! * MangoWM — flat `key=value` lines (`repeat_rate=25`, `tap_to_click=1`)
//! * Niri — an `input { keyboard { … } touchpad { … } mouse { … } }` block
//!   and a `cursor { … }` block (flags take an explicit boolean, so the
//!   drop-in can switch one *off* over the user's file)
//! * Hyprland — `input { … touchpad { … } }` and `cursor { … }` sections
//!   plus `env = XCURSOR_*` (or the Lua equivalents)
//!
//! The cursor theme also reaches X11 / toolkit apps through
//! `~/.icons/default/index.theme`, which is only ever *created* (never
//! overwritten) and removed again when the theme goes back to "your
//! config" — see [`sync_default_cursor`] — and through `XCURSOR_THEME` /
//! `XCURSOR_SIZE` in each drop-in (MangoWM `env=`, Hyprland `env =`; niri
//! exports them itself from its `cursor {}` block).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Ranges the emitters clamp to.
pub mod limits {
    /// Key repeats per second.
    pub const REPEAT_RATE: (u32, u32) = (1, 100);
    /// Delay before a held key repeats, ms.
    pub const REPEAT_DELAY_MS: (u32, u32) = (100, 2000);
    /// libinput pointer acceleration speed.
    pub const SPEED: (f32, f32) = (-1.0, 1.0);
    /// Scroll distance multiplier.
    pub const SCROLL_FACTOR: (f32, f32) = (0.1, 10.0);
    /// Cursor size, px.
    pub const CURSOR_SIZE: (u32, u32) = (8, 256);
    /// Hide the cursor after this long without motion, seconds.
    pub const CURSOR_HIDE_SECS: (u32, u32) = (1, 3600);
}

/// Milliseconds per second (niri's inactivity timeout is in ms).
const MS_PER_SECOND: u32 = 1000;

/// One keyboard layout (`us`, `ara` with variant `digits`, …).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyboardLayout {
    pub layout: String,
    pub variant: String,
}

/// libinput pointer acceleration profile.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccelProfile {
    /// Speed-dependent acceleration (libinput's default).
    Adaptive,
    /// Constant factor: raw, predictable motion (gaming, design work).
    Flat,
}

/// How scrolling is produced.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScrollMethod {
    TwoFinger,
    Edge,
    OnButtonDown,
    NoScroll,
}

/// How a touchpad without physical buttons decides which button clicked.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClickMethod {
    /// Bottom-right corner = right click.
    ButtonAreas,
    /// Two fingers = right click, three = middle.
    Clickfinger,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyboardInput {
    /// Layouts in switching order; empty keeps your compositor's.
    pub layouts: Vec<KeyboardLayout>,
    /// xkb option toggling between layouts (`grp:alt_shift_toggle`).
    pub switch_shortcut: Option<String>,
    /// Further xkb options (`ctrl:nocaps,compose:ralt`).
    pub options: Option<String>,
    pub model: Option<String>,
    /// Key repeats per second.
    pub repeat_rate: Option<u32>,
    /// Delay before repeating, ms.
    pub repeat_delay: Option<u32>,
    /// Num Lock on when the session starts.
    pub numlock: Option<bool>,
    /// Remember the layout per window (Niri).
    pub per_window_layout: Option<bool>,
    /// Short labels the bar shows per layout (`us` → `EN`, `ara` → `ع`).
    pub labels: BTreeMap<String, String>,
}

impl KeyboardInput {
    /// The xkb options string: the switch shortcut first, then the rest.
    pub fn xkb_options(&self) -> Option<String> {
        let parts: Vec<&str> = [self.switch_shortcut.as_deref(), self.options.as_deref()].into_iter().flatten().flat_map(|s| s.split(',')).map(str::trim).filter(|s| !s.is_empty()).collect();
        if self.switch_shortcut.is_none() && self.options.is_none() {
            return None;
        }
        let mut seen: Vec<&str> = Vec::new();
        for p in parts {
            if !seen.contains(&p) {
                seen.push(p);
            }
        }
        // Only separators left: nothing to write (MangoWM rejects an empty
        // `xkb_rules_options=` line), so the user's own options stay.
        (!seen.is_empty()).then(|| seen.join(","))
    }

    /// `(layouts, variants)` as comma lists, when layouts are set.
    pub fn xkb_layouts(&self) -> Option<(String, String)> {
        let list: Vec<&KeyboardLayout> = self.layouts.iter().filter(|l| !l.layout.trim().is_empty()).collect();
        if list.is_empty() {
            return None;
        }
        let layouts = list.iter().map(|l| l.layout.trim()).collect::<Vec<_>>().join(",");
        let variants = list.iter().map(|l| l.variant.trim()).collect::<Vec<_>>().join(",");
        // An all-empty variant list is written as "".
        let variants = if variants.chars().all(|c| c == ',') { String::new() } else { variants };
        Some((layouts, variants))
    }

    /// The bar label for an xkb layout name or description: the user's own
    /// label, else `None` (the plugin falls back to its default).
    pub fn label_for(&self, layout: &str) -> Option<&str> {
        let key = layout.trim().to_ascii_lowercase();
        self.labels.iter().find(|(k, _)| k.to_ascii_lowercase() == key).map(|(_, v)| v.as_str()).filter(|v| !v.is_empty())
    }
}

/// Shared pointer settings (mouse and touchpad).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PointerInput {
    pub accel_profile: Option<AccelProfile>,
    /// `-1..=1`.
    pub speed: Option<f32>,
    pub natural_scroll: Option<bool>,
    pub left_handed: Option<bool>,
    pub scroll_method: Option<ScrollMethod>,
    pub scroll_factor: Option<f32>,
    /// Left + right pressed together = middle click.
    pub middle_emulation: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TouchpadInput {
    #[serde(flatten)]
    pub pointer: PointerInput,
    pub tap_to_click: Option<bool>,
    pub tap_and_drag: Option<bool>,
    pub drag_lock: Option<bool>,
    pub disable_while_typing: Option<bool>,
    pub click_method: Option<ClickMethod>,
    /// Turn the touchpad off while a mouse is plugged in (MangoWM, Niri).
    pub disable_with_mouse: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CursorInput {
    /// XCursor theme name (`Bibata-Modern-Ice`).
    pub theme: Option<String>,
    pub size: Option<u32>,
    pub hide_when_typing: Option<bool>,
    /// Hide after this many seconds without motion.
    pub hide_after_secs: Option<u32>,
    /// Draw the cursor in software (Hyprland `cursor:no_hardware_cursors`):
    /// fixes invisible or flickering cursors on some GPUs.
    pub no_hardware_cursors: Option<bool>,
    /// Hide the cursor while the touchscreen is used (Hyprland).
    pub hide_on_touch: Option<bool>,
}

/// The `[compositor.input]` section.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InputConfig {
    pub keyboard: KeyboardInput,
    pub mouse: PointerInput,
    pub touchpad: TouchpadInput,
    pub cursor: CursorInput,
    /// Focus follows the pointer.
    pub focus_follows_mouse: Option<bool>,
}

impl InputConfig {
    pub fn is_empty(&self) -> bool {
        let mut cmp = self.clone();
        // Labels are a bar setting, not compositor output.
        cmp.keyboard.labels.clear();
        cmp == Self::default()
    }
}

/// Input options and the compositors whose drop-in honours them
/// (`[MangoWM, Niri, Hyprland]`), for Studio's support dots.
pub const INPUT_OPTIONS: &[(&str, [bool; 3])] = &[
    ("keyboard.layouts", [true, true, true]),
    ("keyboard.repeat", [true, true, true]),
    ("keyboard.numlock", [true, true, true]),
    ("keyboard.per_window_layout", [false, true, false]),
    ("mouse.accel_profile", [true, true, true]),
    ("mouse.speed", [true, true, true]),
    ("mouse.natural_scroll", [true, true, true]),
    ("mouse.left_handed", [true, true, true]),
    ("mouse.scroll_method", [true, true, true]),
    ("mouse.scroll_factor", [true, true, true]),
    ("mouse.middle_emulation", [true, true, false]),
    ("touchpad.accel_profile", [true, true, false]),
    ("touchpad.speed", [true, true, false]),
    ("touchpad.natural_scroll", [true, true, true]),
    ("touchpad.left_handed", [true, true, false]),
    ("touchpad.scroll_method", [true, true, false]),
    ("touchpad.scroll_factor", [true, true, true]),
    ("touchpad.middle_emulation", [true, true, true]),
    ("touchpad.tap_to_click", [true, true, true]),
    ("touchpad.tap_and_drag", [true, true, true]),
    ("touchpad.drag_lock", [true, true, true]),
    ("touchpad.disable_while_typing", [true, true, true]),
    ("touchpad.click_method", [true, true, true]),
    ("touchpad.disable_with_mouse", [true, true, false]),
    ("cursor.theme", [true, true, true]),
    ("cursor.size", [true, true, true]),
    ("cursor.hide_when_typing", [true, true, true]),
    ("cursor.hide_after_secs", [true, true, true]),
    ("cursor.no_hardware_cursors", [false, false, true]),
    ("cursor.hide_on_touch", [false, false, true]),
    ("focus_follows_mouse", [true, true, true]),
];

fn clamp_u(v: u32, (lo, hi): (u32, u32)) -> u32 {
    v.clamp(lo, hi)
}

fn clamp_f(v: f32, (lo, hi): (f32, f32)) -> f32 {
    v.clamp(lo, hi)
}

fn b(v: bool) -> u8 {
    v as u8
}

/// MangoWM's integer for libinput enums.
fn mango_profile(p: AccelProfile) -> u8 {
    // LIBINPUT_CONFIG_ACCEL_PROFILE_FLAT = 1, ADAPTIVE = 2
    match p {
        AccelProfile::Flat => 1,
        AccelProfile::Adaptive => 2,
    }
}

fn mango_scroll(m: ScrollMethod) -> u8 {
    // LIBINPUT_CONFIG_SCROLL_NO_SCROLL = 0, 2FG = 1, EDGE = 2, ON_BUTTON_DOWN = 4
    match m {
        ScrollMethod::NoScroll => 0,
        ScrollMethod::TwoFinger => 1,
        ScrollMethod::Edge => 2,
        ScrollMethod::OnButtonDown => 4,
    }
}

fn mango_click(m: ClickMethod) -> u8 {
    // LIBINPUT_CONFIG_CLICK_METHOD_BUTTON_AREAS = 1, CLICKFINGER = 2
    match m {
        ClickMethod::ButtonAreas => 1,
        ClickMethod::Clickfinger => 2,
    }
}

fn kebab_profile(p: AccelProfile) -> &'static str {
    match p {
        AccelProfile::Adaptive => "adaptive",
        AccelProfile::Flat => "flat",
    }
}

fn niri_scroll(m: ScrollMethod) -> &'static str {
    match m {
        ScrollMethod::TwoFinger => "two-finger",
        ScrollMethod::Edge => "edge",
        ScrollMethod::OnButtonDown => "on-button-down",
        ScrollMethod::NoScroll => "no-scroll",
    }
}

fn hypr_scroll(m: ScrollMethod) -> &'static str {
    match m {
        ScrollMethod::TwoFinger => "2fg",
        ScrollMethod::Edge => "edge",
        ScrollMethod::OnButtonDown => "on_button_down",
        ScrollMethod::NoScroll => "no_scroll",
    }
}

fn niri_click(m: ClickMethod) -> &'static str {
    match m {
        ClickMethod::ButtonAreas => "button-areas",
        ClickMethod::Clickfinger => "clickfinger",
    }
}

/// libinput `send_events` modes as MangoWM's `trackpad_send_events_mode`
/// takes them: events on, or off while an external mouse is plugged in.
const MANGO_SEND_EVENTS_ENABLED: u8 = 0;
const MANGO_SEND_EVENTS_OFF_WITH_MOUSE: u8 = 2;

const INPUT_HEADER: &str = "# Input (Studio → Input)\n";
const CURSOR_HEADER: &str = "# Cursor (Studio → Themes → Cursor)\n";

/// MangoWM lines (no header when nothing is set).
pub fn emit_mango(input: &InputConfig) -> String {
    if input.is_empty() {
        return String::new();
    }
    let mut s = String::from(INPUT_HEADER);
    let k = &input.keyboard;
    if let Some((layouts, variants)) = k.xkb_layouts() {
        let _ = writeln!(s, "xkb_rules_layout={layouts}");
        // `mango -p` rejects an empty `xkb_rules_variant=` line.
        if !variants.is_empty() {
            let _ = writeln!(s, "xkb_rules_variant={variants}");
        }
    }
    if let Some(o) = k.xkb_options() {
        let _ = writeln!(s, "xkb_rules_options={o}");
    }
    if let Some(m) = k.model.as_deref().filter(|m| !m.trim().is_empty()) {
        let _ = writeln!(s, "xkb_rules_model={}", m.trim());
    }
    if let Some(v) = k.repeat_rate {
        let _ = writeln!(s, "repeat_rate={}", clamp_u(v, limits::REPEAT_RATE));
    }
    if let Some(v) = k.repeat_delay {
        let _ = writeln!(s, "repeat_delay={}", clamp_u(v, limits::REPEAT_DELAY_MS));
    }
    if let Some(v) = k.numlock {
        let _ = writeln!(s, "numlock_on={}", b(v));
    }
    let pointer = |s: &mut String, prefix: &str, p: &PointerInput, natural_key: &str| {
        if let Some(v) = p.accel_profile {
            let _ = writeln!(s, "{prefix}_accel_profile={}", mango_profile(v));
        }
        if let Some(v) = p.speed {
            let _ = writeln!(s, "{prefix}_accel_speed={:.2}", clamp_f(v, limits::SPEED));
        }
        if let Some(v) = p.natural_scroll {
            let _ = writeln!(s, "{natural_key}={}", b(v));
        }
        if let Some(v) = p.left_handed {
            let _ = writeln!(s, "{prefix}_left_handed={}", b(v));
        }
        if let Some(v) = p.scroll_method {
            let _ = writeln!(s, "{prefix}_scroll_method={}", mango_scroll(v));
        }
        if let Some(v) = p.middle_emulation {
            let _ = writeln!(s, "{prefix}_middle_button_emulation={}", b(v));
        }
    };
    pointer(&mut s, "mouse", &input.mouse, "mouse_natural_scrolling");
    if let Some(v) = input.mouse.scroll_factor {
        let _ = writeln!(s, "axis_scroll_factor={:.2}", clamp_f(v, limits::SCROLL_FACTOR));
    }
    let t = &input.touchpad;
    pointer(&mut s, "trackpad", &t.pointer, "trackpad_natural_scrolling");
    if let Some(v) = t.pointer.scroll_factor {
        let _ = writeln!(s, "trackpad_scroll_factor={:.2}", clamp_f(v, limits::SCROLL_FACTOR));
    }
    for (key, v) in [("tap_to_click", t.tap_to_click), ("tap_and_drag", t.tap_and_drag), ("drag_lock", t.drag_lock), ("trackpad_disable_while_typing", t.disable_while_typing)] {
        if let Some(v) = v {
            let _ = writeln!(s, "{key}={}", b(v));
        }
    }
    if let Some(v) = t.click_method {
        let _ = writeln!(s, "trackpad_click_method={}", mango_click(v));
    }
    if let Some(v) = t.disable_with_mouse {
        let mode = if v { MANGO_SEND_EVENTS_OFF_WITH_MOUSE } else { MANGO_SEND_EVENTS_ENABLED };
        let _ = writeln!(s, "trackpad_send_events_mode={mode}");
    }
    if let Some(v) = input.focus_follows_mouse {
        let _ = writeln!(s, "sloppy_focus={}", b(v));
    }
    // The cursor is set on Studio → Themes → Cursor.
    let c = &input.cursor;
    let cursor_at = s.len();
    // `env=` reaches the apps MangoWM starts (X11 / older toolkit apps
    // draw their own cursor from these).
    if let Some(theme) = c.theme.as_deref().filter(|t| !t.trim().is_empty()) {
        let _ = writeln!(s, "cursor_theme={}", theme.trim());
        let _ = writeln!(s, "env=XCURSOR_THEME,{}", theme.trim());
    }
    if let Some(v) = c.size {
        let _ = writeln!(s, "cursor_size={}", clamp_u(v, limits::CURSOR_SIZE));
        let _ = writeln!(s, "env=XCURSOR_SIZE,{}", clamp_u(v, limits::CURSOR_SIZE));
    }
    if let Some(v) = c.hide_when_typing {
        let _ = writeln!(s, "cursor_hide_on_keypress={}", b(v));
    }
    if let Some(v) = c.hide_after_secs {
        let _ = writeln!(s, "cursor_hide_timeout={}", clamp_u(v, limits::CURSOR_HIDE_SECS));
    }
    if s.len() > cursor_at {
        s.insert_str(cursor_at, CURSOR_HEADER);
    }
    // Only cursor options set: no empty Input heading.
    if s.starts_with(INPUT_HEADER) && s[INPUT_HEADER.len()..].starts_with(CURSOR_HEADER) {
        s.drain(..INPUT_HEADER.len());
    }
    s.push('\n');
    s
}

fn kdl_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A niri flag that takes an optional value (`numlock false`,
/// `hide-when-typing true`): keyboard and cursor flags, which merge one by
/// one across includes.
fn flag(out: &mut String, indent: &str, name: &str, v: Option<bool>) {
    if let Some(v) = v {
        let _ = writeln!(out, "{indent}{name} {v}");
    }
}

/// A niri touchpad / mouse switch. These are bare flags (`tap`; niri 26.04
/// rejects `tap false`), and an included `touchpad {}` / `mouse {}` block
/// replaces the user's whole block, so leaving the flag out means off.
fn switch(out: &mut String, indent: &str, name: &str, v: Option<bool>) {
    if v == Some(true) {
        let _ = writeln!(out, "{indent}{name}");
    }
}

/// Whether any of these touchpad / mouse options is set (switched off
/// counts: the block still has to be written to replace the user's).
fn any_set(values: &[Option<bool>]) -> bool {
    values.iter().any(Option::is_some)
}

fn niri_pointer(out: &mut String, p: &PointerInput) {
    const IN: &str = "        ";
    switch(out, IN, "natural-scroll", p.natural_scroll);
    if let Some(v) = p.speed {
        let _ = writeln!(out, "{IN}accel-speed {:.2}", clamp_f(v, limits::SPEED));
    }
    if let Some(v) = p.accel_profile {
        let _ = writeln!(out, "{IN}accel-profile {}", kdl_str(kebab_profile(v)));
    }
    if let Some(v) = p.scroll_method {
        let _ = writeln!(out, "{IN}scroll-method {}", kdl_str(niri_scroll(v)));
    }
    if let Some(v) = p.scroll_factor {
        let _ = writeln!(out, "{IN}scroll-factor {:.2}", clamp_f(v, limits::SCROLL_FACTOR));
    }
    switch(out, IN, "left-handed", p.left_handed);
    switch(out, IN, "middle-emulation", p.middle_emulation);
}

/// Pointer switches written by [`niri_pointer`].
fn niri_pointer_switches(p: &PointerInput) -> [Option<bool>; 3] {
    [p.natural_scroll, p.left_handed, p.middle_emulation]
}

/// Closes a niri touchpad / mouse block. Every option is written as a line,
/// except switches that are off (bare flags can't say "off"): a block with
/// only those is still written, empty, so it replaces the user's block.
fn niri_device_block(body: &mut String, name: &str, lines: &str, any_switch: bool) {
    if !lines.is_empty() || any_switch {
        let _ = write!(body, "    {name} {{\n        // niri replaces your whole {name} block with this one\n{lines}    }}\n");
    }
}

/// Niri `input {}` and `cursor {}` blocks (empty when nothing is set).
pub fn emit_niri(input: &InputConfig) -> String {
    if input.is_empty() {
        return String::new();
    }
    let mut body = String::new();
    let k = &input.keyboard;
    let mut kb = String::new();
    let mut xkb = String::new();
    if let Some((layouts, variants)) = k.xkb_layouts() {
        let _ = writeln!(xkb, "            layout {}", kdl_str(&layouts));
        if !variants.is_empty() {
            let _ = writeln!(xkb, "            variant {}", kdl_str(&variants));
        }
    }
    if let Some(o) = k.xkb_options() {
        let _ = writeln!(xkb, "            options {}", kdl_str(&o));
    }
    if let Some(m) = k.model.as_deref().filter(|m| !m.trim().is_empty()) {
        let _ = writeln!(xkb, "            model {}", kdl_str(m.trim()));
    }
    if !xkb.is_empty() {
        let _ = write!(kb, "        xkb {{\n{xkb}        }}\n");
    }
    if let Some(v) = k.repeat_delay {
        let _ = writeln!(kb, "        repeat-delay {}", clamp_u(v, limits::REPEAT_DELAY_MS));
    }
    if let Some(v) = k.repeat_rate {
        let _ = writeln!(kb, "        repeat-rate {}", clamp_u(v, limits::REPEAT_RATE));
    }
    flag(&mut kb, "        ", "numlock", k.numlock);
    if let Some(v) = k.per_window_layout {
        let _ = writeln!(kb, "        track-layout {}", kdl_str(if v { "window" } else { "global" }));
    }
    if !kb.is_empty() {
        let _ = write!(body, "    keyboard {{\n{kb}    }}\n");
    }
    let t = &input.touchpad;
    let mut tp = String::new();
    switch(&mut tp, "        ", "tap", t.tap_to_click);
    switch(&mut tp, "        ", "dwt", t.disable_while_typing);
    // `drag` takes a value.
    flag(&mut tp, "        ", "drag", t.tap_and_drag);
    switch(&mut tp, "        ", "drag-lock", t.drag_lock);
    switch(&mut tp, "        ", "disabled-on-external-mouse", t.disable_with_mouse);
    if let Some(v) = t.click_method {
        let _ = writeln!(tp, "        click-method {}", kdl_str(niri_click(v)));
    }
    niri_pointer(&mut tp, &t.pointer);
    let touch_switches = [t.tap_to_click, t.disable_while_typing, t.drag_lock, t.disable_with_mouse];
    niri_device_block(&mut body, "touchpad", &tp, any_set(&touch_switches) || any_set(&niri_pointer_switches(&t.pointer)));
    let mut mouse = String::new();
    niri_pointer(&mut mouse, &input.mouse);
    niri_device_block(&mut body, "mouse", &mouse, any_set(&niri_pointer_switches(&input.mouse)));
    match input.focus_follows_mouse {
        Some(true) => body.push_str("    focus-follows-mouse\n"),
        // An include can switch it on but not off (niri 26.04).
        Some(false) => body.push_str("    // focus-follows-mouse can't be switched off from an include: remove it from config.kdl\n"),
        None => {}
    }
    let mut s = String::new();
    if !body.is_empty() {
        let _ = write!(s, "\ninput {{\n{body}}}\n");
    }
    let c = &input.cursor;
    let mut cursor = String::new();
    if let Some(theme) = c.theme.as_deref().filter(|t| !t.trim().is_empty()) {
        let _ = writeln!(cursor, "    xcursor-theme {}", kdl_str(theme.trim()));
    }
    if let Some(v) = c.size {
        let _ = writeln!(cursor, "    xcursor-size {}", clamp_u(v, limits::CURSOR_SIZE));
    }
    flag(&mut cursor, "    ", "hide-when-typing", c.hide_when_typing);
    if let Some(v) = c.hide_after_secs {
        let _ = writeln!(cursor, "    hide-after-inactive-ms {}", clamp_u(v, limits::CURSOR_HIDE_SECS) * MS_PER_SECOND);
    }
    if !cursor.is_empty() {
        let _ = write!(s, "\ncursor {{\n{cursor}}}\n");
    }
    s
}

/// Hyprland `follow_mouse`: 1 = focus follows the pointer, 0 = click.
fn hypr_follow(v: bool) -> u8 {
    b(v)
}

/// `(key, value)` pairs of Hyprland's `input` section, its `touchpad`
/// subsection and its `cursor` section, shared by both dialects.
struct HyprInput {
    input: Vec<(&'static str, String)>,
    touchpad: Vec<(&'static str, String)>,
    cursor: Vec<(&'static str, String)>,
    env: Vec<(&'static str, String)>,
}

fn hypr_input(input: &InputConfig) -> HyprInput {
    let mut h = HyprInput { input: Vec::new(), touchpad: Vec::new(), cursor: Vec::new(), env: Vec::new() };
    let k = &input.keyboard;
    if let Some((layouts, variants)) = k.xkb_layouts() {
        h.input.push(("kb_layout", layouts));
        h.input.push(("kb_variant", variants));
    }
    if let Some(o) = k.xkb_options() {
        h.input.push(("kb_options", o));
    }
    if let Some(m) = k.model.as_deref().filter(|m| !m.trim().is_empty()) {
        h.input.push(("kb_model", m.trim().to_string()));
    }
    if let Some(v) = k.repeat_rate {
        h.input.push(("repeat_rate", clamp_u(v, limits::REPEAT_RATE).to_string()));
    }
    if let Some(v) = k.repeat_delay {
        h.input.push(("repeat_delay", clamp_u(v, limits::REPEAT_DELAY_MS).to_string()));
    }
    if let Some(v) = k.numlock {
        h.input.push(("numlock_by_default", v.to_string()));
    }
    let m = &input.mouse;
    if let Some(v) = m.speed {
        h.input.push(("sensitivity", format!("{:.2}", clamp_f(v, limits::SPEED))));
    }
    if let Some(v) = m.accel_profile {
        h.input.push(("accel_profile", kebab_profile(v).to_string()));
    }
    if let Some(v) = m.natural_scroll {
        h.input.push(("natural_scroll", v.to_string()));
    }
    if let Some(v) = m.left_handed {
        h.input.push(("left_handed", v.to_string()));
    }
    if let Some(v) = m.scroll_method {
        h.input.push(("scroll_method", hypr_scroll(v).to_string()));
    }
    if let Some(v) = m.scroll_factor {
        h.input.push(("scroll_factor", format!("{:.2}", clamp_f(v, limits::SCROLL_FACTOR))));
    }
    if let Some(v) = input.focus_follows_mouse {
        h.input.push(("follow_mouse", hypr_follow(v).to_string()));
    }
    let t = &input.touchpad;
    for (key, v) in [
        ("natural_scroll", t.pointer.natural_scroll),
        ("disable_while_typing", t.disable_while_typing),
        ("tap-to-click", t.tap_to_click),
        ("tap-and-drag", t.tap_and_drag),
        ("middle_button_emulation", t.pointer.middle_emulation),
    ] {
        if let Some(v) = v {
            h.touchpad.push((key, v.to_string()));
        }
    }
    // An int in Hyprland (0 off, 1 on with timeout, 2 sticky): a boolean is
    // a type error in the Lua config.
    if let Some(v) = t.drag_lock {
        h.touchpad.push(("drag_lock", b(v).to_string()));
    }
    if let Some(v) = t.pointer.scroll_factor {
        h.touchpad.push(("scroll_factor", format!("{:.2}", clamp_f(v, limits::SCROLL_FACTOR))));
    }
    if let Some(v) = t.click_method {
        h.touchpad.push(("clickfinger_behavior", (v == ClickMethod::Clickfinger).to_string()));
    }
    let c = &input.cursor;
    if let Some(v) = c.hide_when_typing {
        h.cursor.push(("hide_on_key_press", v.to_string()));
    }
    if let Some(v) = c.hide_after_secs {
        h.cursor.push(("inactive_timeout", clamp_u(v, limits::CURSOR_HIDE_SECS).to_string()));
    }
    // An int since Hyprland 0.41 (0 off, 1 on, 2 auto): a boolean is a type
    // error in the Lua config.
    if let Some(v) = c.no_hardware_cursors {
        h.cursor.push(("no_hardware_cursors", b(v).to_string()));
    }
    if let Some(v) = c.hide_on_touch {
        h.cursor.push(("hide_on_touch", v.to_string()));
    }
    if let Some(theme) = c.theme.as_deref().filter(|t| !t.trim().is_empty()) {
        h.env.push(("XCURSOR_THEME", theme.trim().to_string()));
        h.env.push(("HYPRCURSOR_THEME", theme.trim().to_string()));
    }
    if let Some(v) = c.size {
        let size = clamp_u(v, limits::CURSOR_SIZE).to_string();
        h.env.push(("XCURSOR_SIZE", size.clone()));
        h.env.push(("HYPRCURSOR_SIZE", size));
    }
    h
}

/// Hyprland (hyprlang) `input {}` / `cursor {}` sections and cursor env.
pub fn emit_hyprland_conf(input: &InputConfig) -> String {
    if input.is_empty() {
        return String::new();
    }
    let h = hypr_input(input);
    let mut s = String::new();
    for (k, v) in &h.env {
        let _ = writeln!(s, "env = {k},{v}");
    }
    if !h.input.is_empty() || !h.touchpad.is_empty() {
        s.push_str("\ninput {\n");
        for (k, v) in &h.input {
            let _ = writeln!(s, "    {k} = {v}");
        }
        if !h.touchpad.is_empty() {
            s.push_str("    touchpad {\n");
            for (k, v) in &h.touchpad {
                let _ = writeln!(s, "        {k} = {v}");
            }
            s.push_str("    }\n");
        }
        s.push_str("}\n");
    }
    if !h.cursor.is_empty() {
        s.push_str("\ncursor {\n");
        for (k, v) in &h.cursor {
            let _ = writeln!(s, "    {k} = {v}");
        }
        s.push_str("}\n");
    }
    s
}

fn lua_value(v: &str) -> String {
    if v == "true" || v == "false" || v.parse::<f64>().is_ok() { v.to_string() } else { format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\"")) }
}

/// Hyprland's Lua option names spell hyprlang's dashes as underscores
/// (`tap-to-click` → `tap_to_click`; the dashed key is "unknown").
fn lua_key(k: &str) -> String {
    k.replace('-', "_")
}

/// Hyprland (Lua) `hl.config({ input = …, cursor = … })` table entries,
/// without the surrounding call, plus `pcall(hl.env, …)` lines.
pub fn hyprland_lua_sections(input: &InputConfig) -> (Vec<String>, Vec<String>) {
    if input.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let h = hypr_input(input);
    let mut sections = Vec::new();
    let mut fields: Vec<String> = h.input.iter().map(|(k, v)| format!("{} = {}", lua_key(k), lua_value(v))).collect();
    if !h.touchpad.is_empty() {
        fields.push(format!("touchpad = {{ {} }}", h.touchpad.iter().map(|(k, v)| format!("{} = {}", lua_key(k), lua_value(v))).collect::<Vec<_>>().join(", ")));
    }
    if !fields.is_empty() {
        sections.push(format!("    input = {{ {} }},", fields.join(", ")));
    }
    if !h.cursor.is_empty() {
        sections.push(format!("    cursor = {{ {} }},", h.cursor.iter().map(|(k, v)| format!("{} = {}", lua_key(k), lua_value(v))).collect::<Vec<_>>().join(", ")));
    }
    let env = h.env.iter().map(|(k, v)| format!("pcall(hl.env, \"{k}\", {})", lua_value(v))).collect();
    (sections, env)
}

// ── Cursor themes ──────────────────────────────────────────────────────────

/// Directories searched for cursor themes, most specific first.
pub fn icon_dirs() -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    let mut dirs = vec![home.join(".icons")];
    if let Some(data) = dirs::data_dir() {
        dirs.push(data.join("icons"));
    }
    let system = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.extend(system.split(':').filter(|d| !d.is_empty()).map(|d| PathBuf::from(d).join("icons")));
    dirs.push(PathBuf::from("/usr/share/pixmaps"));
    dirs
}

/// Installed cursor themes (folders with a `cursors/` directory), sorted
/// and deduplicated. `default` is skipped: it only points at another theme.
pub fn cursor_themes_in(dirs: &[PathBuf]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name != "default" && e.path().join("cursors").is_dir() && !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out.sort_by_key(|n| n.to_lowercase());
    out
}

pub fn cursor_themes() -> Vec<String> {
    cursor_themes_in(&icon_dirs())
}

/// Marker in the `index.theme` Mandara writes (so it may update its own
/// file later, but never one the user wrote).
const DEFAULT_CURSOR_MARKER: &str = "X-Mandara-Generated=true";

/// `~/.icons/default/index.theme`.
pub fn default_cursor_file() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".icons/default/index.theme")
}

/// Makes X11 and toolkit apps inherit `theme` through
/// `~/.icons/default/index.theme`: created when absent, updated only when
/// Mandara wrote it. Returns whether the file now names `theme`.
pub fn ensure_default_cursor(theme: &str) -> std::io::Result<bool> {
    ensure_default_cursor_at(&default_cursor_file(), theme)
}

/// Undoes [`ensure_default_cursor`]: removes `~/.icons/default/index.theme`
/// only when Mandara wrote it (X11 apps then fall back to their default).
/// Returns whether a file was removed.
pub fn clear_default_cursor() -> std::io::Result<bool> {
    clear_default_cursor_at(&default_cursor_file())
}

pub fn clear_default_cursor_at(path: &Path) -> std::io::Result<bool> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.contains(DEFAULT_CURSOR_MARKER) => std::fs::remove_file(path).map(|()| true),
        _ => Ok(false),
    }
}

/// Points `~/.icons/default` at `theme`, or clears Mandara's file when the
/// theme goes back to "your config" (`None`).
pub fn sync_default_cursor(theme: Option<&str>) -> std::io::Result<bool> {
    match theme.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => ensure_default_cursor(t),
        None => clear_default_cursor().map(|_| true),
    }
}

pub fn ensure_default_cursor_at(path: &Path, theme: &str) -> std::io::Result<bool> {
    let theme = theme.trim();
    if theme.is_empty() {
        return Ok(false);
    }
    if let Ok(existing) = std::fs::read_to_string(path)
        && !existing.contains(DEFAULT_CURSOR_MARKER)
    {
        // The user's own file: leave it alone.
        return Ok(existing.lines().any(|l| l.trim() == format!("Inherits={theme}")));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = format!("[Icon Theme]\nName=Default\nComment=Default cursor theme (set by Mandara Studio → Themes → Cursor)\nInherits={theme}\n{DEFAULT_CURSOR_MARKER}\n");
    std::fs::write(path, text)?;
    Ok(true)
}

// ── xkb catalog ─────────────────────────────────────────────────────────────

/// One layout from the xkb rules list, with its variants.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct XkbLayout {
    pub name: String,
    pub description: String,
    /// `(variant, description)`.
    pub variants: Vec<(String, String)>,
}

/// Layouts and `grp:` (layout switching) options from `evdev.lst`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct XkbCatalog {
    pub layouts: Vec<XkbLayout>,
    /// `(option, description)` of the `grp` group.
    pub switch_options: Vec<(String, String)>,
}

pub const XKB_RULES: &str = "/usr/share/X11/xkb/rules/evdev.lst";

/// Parses the `! layout`, `! variant` and `! option` sections of an
/// `evdev.lst`.
pub fn parse_xkb_list(text: &str) -> XkbCatalog {
    let mut cat = XkbCatalog::default();
    let mut section = "";
    for line in text.lines() {
        if let Some(name) = line.strip_prefix('!') {
            section = match name.trim() {
                "layout" => "layout",
                "variant" => "variant",
                "option" => "option",
                _ => "",
            };
            continue;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (key, desc) = match line.split_once(char::is_whitespace) {
            Some((k, d)) => (k.trim(), d.trim()),
            None => (line, ""),
        };
        match section {
            "layout" => cat.layouts.push(XkbLayout { name: key.into(), description: desc.into(), variants: Vec::new() }),
            "variant" => {
                // `variant  layout: Description`
                if let Some((layout, d)) = desc.split_once(':')
                    && let Some(l) = cat.layouts.iter_mut().find(|l| l.name == layout.trim())
                {
                    l.variants.push((key.into(), d.trim().into()));
                }
            }
            "option" if key.starts_with("grp:") => cat.switch_options.push((key.into(), desc.into())),
            _ => {}
        }
    }
    cat
}

pub fn xkb_catalog() -> XkbCatalog {
    std::fs::read_to_string(XKB_RULES).map(|t| parse_xkb_list(&t)).unwrap_or_default()
}

impl XkbCatalog {
    /// The xkb layout name of a description the compositor reports
    /// (`English (US)` → `us`, `Arabic (Eastern Arabic numerals)` → `ara`).
    pub fn name_for(&self, description: &str) -> Option<&str> {
        let d = description.trim();
        self.layouts.iter().find(|l| l.description == d || l.variants.iter().any(|(_, vd)| vd == d)).map(|l| l.name.as_str())
    }
}

/// The system catalog, parsed once.
pub fn shared_catalog() -> &'static XkbCatalog {
    static CATALOG: std::sync::OnceLock<XkbCatalog> = std::sync::OnceLock::new();
    CATALOG.get_or_init(xkb_catalog)
}

impl KeyboardInput {
    /// The user's bar label for the layout the compositor reports (by its
    /// description or xkb name).
    pub fn label_for_reported(&self, reported: &str) -> Option<&str> {
        if self.labels.is_empty() {
            return None;
        }
        self.label_for(reported).or_else(|| shared_catalog().name_for(reported).and_then(|name| self.label_for(name)))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn full() -> InputConfig {
        InputConfig {
            keyboard: KeyboardInput {
                layouts: vec![KeyboardLayout { layout: "us".into(), variant: String::new() }, KeyboardLayout { layout: "ara".into(), variant: "digits".into() }],
                switch_shortcut: Some("grp:alt_shift_toggle".into()),
                options: Some("ctrl:nocaps".into()),
                model: Some("pc105".into()),
                repeat_rate: Some(30),
                repeat_delay: Some(400),
                numlock: Some(true),
                per_window_layout: Some(true),
                labels: [("ara".to_string(), "ع".to_string())].into(),
            },
            mouse: PointerInput { accel_profile: Some(AccelProfile::Flat), speed: Some(-0.25), natural_scroll: Some(false), left_handed: Some(false), scroll_method: Some(ScrollMethod::OnButtonDown), scroll_factor: Some(1.5), middle_emulation: Some(true) },
            touchpad: TouchpadInput {
                pointer: PointerInput { accel_profile: Some(AccelProfile::Adaptive), speed: Some(0.3), natural_scroll: Some(true), left_handed: Some(false), scroll_method: Some(ScrollMethod::TwoFinger), scroll_factor: Some(0.8), middle_emulation: Some(false) },
                tap_to_click: Some(true),
                tap_and_drag: Some(false),
                drag_lock: Some(true),
                disable_while_typing: Some(true),
                click_method: Some(ClickMethod::Clickfinger),
                disable_with_mouse: Some(true),
            },
            cursor: CursorInput { theme: Some("Bibata-Modern-Ice".into()), size: Some(32), hide_when_typing: Some(true), hide_after_secs: Some(5), no_hardware_cursors: Some(true), hide_on_touch: Some(false) },
            focus_follows_mouse: Some(true),
        }
    }

    #[test]
    fn empty_input_emits_nothing() {
        let mut i = InputConfig::default();
        assert!(emit_mango(&i).is_empty() && emit_niri(&i).is_empty() && emit_hyprland_conf(&i).is_empty());
        i.keyboard.labels.insert("us".into(), "EN".into());
        assert!(i.is_empty(), "labels are bar-only");
        assert!(emit_niri(&i).is_empty());
    }

    #[test]
    fn xkb_option_and_layout_lists() {
        let k = full().keyboard;
        assert_eq!(k.xkb_options().as_deref(), Some("grp:alt_shift_toggle,ctrl:nocaps"));
        assert_eq!(k.xkb_layouts(), Some(("us,ara".into(), ",digits".into())));
        let plain = KeyboardInput { layouts: vec![KeyboardLayout { layout: "us".into(), ..Default::default() }, KeyboardLayout { layout: "eg".into(), ..Default::default() }], ..Default::default() };
        assert_eq!(plain.xkb_layouts(), Some(("us,eg".into(), String::new())));
        assert_eq!(plain.xkb_options(), None);
        assert_eq!(k.label_for("ARA"), Some("ع"));
        assert_eq!(k.label_for("us"), None);
    }

    #[test]
    fn mango_keys() {
        let text = emit_mango(&full());
        for needle in [
            "xkb_rules_layout=us,ara",
            "xkb_rules_variant=,digits",
            "xkb_rules_options=grp:alt_shift_toggle,ctrl:nocaps",
            "repeat_rate=30",
            "repeat_delay=400",
            "numlock_on=1",
            "mouse_accel_profile=1",
            "mouse_accel_speed=-0.25",
            "mouse_scroll_method=4",
            "axis_scroll_factor=1.50",
            "trackpad_natural_scrolling=1",
            "trackpad_accel_profile=2",
            "tap_to_click=1",
            "tap_and_drag=0",
            "trackpad_click_method=2",
            "cursor_theme=Bibata-Modern-Ice",
            "cursor_size=32",
            "cursor_hide_on_keypress=1",
            "cursor_hide_timeout=5",
            "env=XCURSOR_THEME,Bibata-Modern-Ice",
            "env=XCURSOR_SIZE,32",
            "sloppy_focus=1",
        ] {
            assert!(text.lines().any(|l| l == needle), "missing {needle}\n{text}");
        }
    }

    #[test]
    fn cursor_lines_get_their_own_heading() {
        let mut i = InputConfig::default();
        i.cursor.theme = Some("Banana".into());
        let text = emit_mango(&i);
        assert!(text.starts_with(CURSOR_HEADER) && !text.contains(INPUT_HEADER), "{text}");
        i.keyboard.repeat_rate = Some(30);
        let text = emit_mango(&i);
        assert!(text.starts_with(INPUT_HEADER) && text.contains(&format!("repeat_rate=30\n{CURSOR_HEADER}cursor_theme=Banana")), "{text}");
    }

    #[test]
    fn values_are_clamped() {
        let mut i = InputConfig::default();
        i.keyboard.repeat_rate = Some(10_000);
        i.mouse.speed = Some(4.0);
        i.cursor.size = Some(1);
        let text = emit_mango(&i);
        assert!(text.contains("repeat_rate=100") && text.contains("mouse_accel_speed=1.00") && text.contains("cursor_size=8"), "{text}");
    }

    #[test]
    fn niri_blocks_are_valid_kdl() {
        let text = emit_niri(&full());
        kdl::KdlDocument::parse_v1(&text).unwrap_or_else(|e| panic!("invalid KDL v1: {e:?}\n{text}"));
        for needle in ["layout \"us,ara\"", "variant \",digits\"", "repeat-rate 30", "numlock true", "track-layout \"window\"", "tap\n", "drag false", "click-method \"clickfinger\"", "accel-profile \"flat\"", "scroll-method \"on-button-down\"", "focus-follows-mouse", "xcursor-theme \"Bibata-Modern-Ice\"", "hide-after-inactive-ms 5000"] {
            assert!(text.contains(needle), "missing {needle}\n{text}");
        }
    }

    #[test]
    fn hyprland_sections() {
        let text = emit_hyprland_conf(&full());
        for needle in ["env = XCURSOR_THEME,Bibata-Modern-Ice", "env = HYPRCURSOR_SIZE,32", "no_hardware_cursors = 1", "hide_on_touch = false", "hide_on_key_press = true", "kb_layout = us,ara", "kb_options = grp:alt_shift_toggle,ctrl:nocaps", "sensitivity = -0.25", "follow_mouse = 1", "tap-to-click = true", "clickfinger_behavior = true", "inactive_timeout = 5", "middle_button_emulation = false"] {
            assert!(text.contains(needle), "missing {needle}\n{text}");
        }
        assert_eq!(text.matches('{').count(), text.matches('}').count());
        let (sections, env) = hyprland_lua_sections(&full());
        let joined = sections.join("\n");
        // Hyprland's Lua names: dashes become underscores; drag_lock is an int.
        assert!(joined.contains("tap_to_click = true") && !joined.contains("tap-to-click") && joined.contains("drag_lock = 1") && joined.contains("kb_layout = \"us,ara\""), "{joined}");
        assert!(joined.contains("no_hardware_cursors = 1") && joined.contains("hide_on_touch = false"), "{joined}");
        assert_eq!(joined.matches('{').count(), joined.matches('}').count());
        assert!(env.iter().any(|l| l.contains("XCURSOR_SIZE")));
    }

    #[test]
    fn default_cursor_file_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("icons/default/index.theme");
        assert!(ensure_default_cursor_at(&path, "Bibata").unwrap());
        assert!(std::fs::read_to_string(&path).unwrap().contains("Inherits=Bibata"));
        // Mandara's own file is updated…
        assert!(ensure_default_cursor_at(&path, "Adwaita").unwrap());
        assert!(std::fs::read_to_string(&path).unwrap().contains("Inherits=Adwaita"));
        // …and removed again when the theme is unset.
        assert!(clear_default_cursor_at(&path).unwrap());
        assert!(!path.exists());
        assert!(!clear_default_cursor_at(&path).unwrap(), "nothing left to remove");
        // …the user's is not.
        let mine = "[Icon Theme]\nInherits=capitaine-cursors\n";
        std::fs::write(&path, mine).unwrap();
        assert!(!ensure_default_cursor_at(&path, "Bibata").unwrap());
        assert!(!clear_default_cursor_at(&path).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), mine);
    }

    #[test]
    fn finds_cursor_themes() {
        let dir = tempfile::tempdir().unwrap();
        for t in ["Zed", "alpha", "default"] {
            std::fs::create_dir_all(dir.path().join(t).join("cursors")).unwrap();
        }
        std::fs::create_dir_all(dir.path().join("Papirus/48x48")).unwrap();
        assert_eq!(cursor_themes_in(&[dir.path().to_path_buf()]), vec!["alpha", "Zed"]);
    }

    #[test]
    fn parses_evdev_list() {
        let lst = "! model\n  pc105 Generic 105-key PC\n\n! layout\n  us              English (US)\n  ara             Arabic\n\n! variant\n  intl            us: English (US, intl., with dead keys)\n  digits          ara: Arabic (Eastern Arabic numerals)\n\n! option\n  grp                  Switching to another layout\n  grp:alt_shift_toggle Alt+Shift\n  ctrl:nocaps          Caps Lock as Ctrl\n";
        let cat = parse_xkb_list(lst);
        assert_eq!(cat.name_for("Arabic (Eastern Arabic numerals)"), Some("ara"));
        assert_eq!(cat.name_for("English (US)"), Some("us"));
        assert_eq!(cat.name_for("Klingon"), None);
        assert_eq!(cat.layouts.len(), 2);
        assert_eq!(cat.layouts[1].variants, vec![("digits".to_string(), "Arabic (Eastern Arabic numerals)".to_string())]);
        assert_eq!(cat.switch_options, vec![("grp:alt_shift_toggle".to_string(), "Alt+Shift".to_string())]);
    }

    #[test]
    fn system_xkb_list_parses_when_present() {
        let cat = xkb_catalog();
        if std::path::Path::new(XKB_RULES).exists() {
            assert!(cat.layouts.iter().any(|l| l.name == "us"));
            assert!(!cat.switch_options.is_empty());
        }
    }

    #[test]
    fn toml_round_trip() {
        let i = full();
        let text = toml::to_string(&i).unwrap();
        let back: InputConfig = toml::from_str(&text).unwrap();
        assert_eq!(back, i);
        let partial: InputConfig = toml::from_str("[touchpad]\ntap_to_click = true\nspeed = 0.5\n").unwrap();
        assert_eq!(partial.touchpad.tap_to_click, Some(true));
        assert_eq!(partial.touchpad.pointer.speed, Some(0.5));
    }
}

/// The input options inside each compositor's whole drop-in.
#[cfg(test)]
mod dropin_tests {
    use super::tests::full;
    use super::{AccelProfile, ClickMethod, INPUT_OPTIONS, InputConfig, KeyboardLayout, ScrollMethod};
    use crate::compositor::*;

    fn managed(config: &CompositorConfig) -> ManagedConfig<'_> {
        ManagedConfig { config, borders: None, existing_chords: Default::default(), mandara_bin: "mandara".into(), version: Some((26, 4)), user_autostarts: false }
    }

    fn with_extras() -> CompositorConfig {
        let mut c = CompositorConfig { input: full(), ..Default::default() };
        c.hyprland = HyprlandOptions {
            layout: Some("master".into()),
            dwindle_preserve_split: Some(true),
            dwindle_smart_split: Some(false),
            dwindle_force_split: Some(9),
            master_orientation: Some("center".into()),
            master_mfact: Some(0.6),
            master_new_on_top: Some(true),
            resize_on_border: Some(true),
            border_grab_area: Some(12),
            groupbar: Some(true),
            groupbar_titles: Some(false),
            groupbar_gradients: Some(true),
            groupbar_height: Some(18),
            ..Default::default()
        };
        c
    }

    /// Every Studio → Input option on its own, with each of its values:
    /// `(INPUT_OPTIONS key, setter)`.
    fn single_options() -> Vec<(&'static str, InputConfig)> {
        type Set = fn(&mut InputConfig);
        let cases: Vec<(&str, Set)> = vec![
            ("keyboard.layouts", |i| i.keyboard.layouts = vec![KeyboardLayout { layout: "us".into(), variant: String::new() }, KeyboardLayout { layout: "ara".into(), variant: String::new() }]),
            ("keyboard.layouts", |i| i.keyboard.layouts = vec![KeyboardLayout { layout: "us".into(), variant: "intl".into() }]),
            ("keyboard.layouts", |i| i.keyboard.switch_shortcut = Some("grp:win_space_toggle".into())),
            ("keyboard.layouts", |i| i.keyboard.options = Some("ctrl:nocaps,compose:ralt".into())),
            ("keyboard.layouts", |i| i.keyboard.model = Some("pc105".into())),
            ("keyboard.repeat", |i| i.keyboard.repeat_rate = Some(40)),
            ("keyboard.repeat", |i| i.keyboard.repeat_delay = Some(250)),
            ("keyboard.numlock", |i| i.keyboard.numlock = Some(true)),
            ("keyboard.numlock", |i| i.keyboard.numlock = Some(false)),
            ("keyboard.per_window_layout", |i| i.keyboard.per_window_layout = Some(true)),
            ("mouse.accel_profile", |i| i.mouse.accel_profile = Some(AccelProfile::Flat)),
            ("mouse.accel_profile", |i| i.mouse.accel_profile = Some(AccelProfile::Adaptive)),
            ("mouse.speed", |i| i.mouse.speed = Some(-0.5)),
            ("mouse.natural_scroll", |i| i.mouse.natural_scroll = Some(true)),
            ("mouse.left_handed", |i| i.mouse.left_handed = Some(true)),
            ("mouse.scroll_method", |i| i.mouse.scroll_method = Some(ScrollMethod::TwoFinger)),
            ("mouse.scroll_method", |i| i.mouse.scroll_method = Some(ScrollMethod::Edge)),
            ("mouse.scroll_method", |i| i.mouse.scroll_method = Some(ScrollMethod::OnButtonDown)),
            ("mouse.scroll_method", |i| i.mouse.scroll_method = Some(ScrollMethod::NoScroll)),
            ("mouse.scroll_factor", |i| i.mouse.scroll_factor = Some(2.5)),
            ("mouse.middle_emulation", |i| i.mouse.middle_emulation = Some(true)),
            ("touchpad.accel_profile", |i| i.touchpad.pointer.accel_profile = Some(AccelProfile::Flat)),
            ("touchpad.speed", |i| i.touchpad.pointer.speed = Some(0.4)),
            ("touchpad.natural_scroll", |i| i.touchpad.pointer.natural_scroll = Some(false)),
            ("touchpad.left_handed", |i| i.touchpad.pointer.left_handed = Some(true)),
            ("touchpad.scroll_method", |i| i.touchpad.pointer.scroll_method = Some(ScrollMethod::Edge)),
            ("touchpad.scroll_factor", |i| i.touchpad.pointer.scroll_factor = Some(0.5)),
            ("touchpad.middle_emulation", |i| i.touchpad.pointer.middle_emulation = Some(true)),
            ("touchpad.tap_to_click", |i| i.touchpad.tap_to_click = Some(true)),
            ("touchpad.tap_and_drag", |i| i.touchpad.tap_and_drag = Some(false)),
            ("touchpad.drag_lock", |i| i.touchpad.drag_lock = Some(true)),
            ("touchpad.disable_while_typing", |i| i.touchpad.disable_while_typing = Some(true)),
            ("touchpad.click_method", |i| i.touchpad.click_method = Some(ClickMethod::ButtonAreas)),
            ("touchpad.click_method", |i| i.touchpad.click_method = Some(ClickMethod::Clickfinger)),
            ("touchpad.disable_with_mouse", |i| i.touchpad.disable_with_mouse = Some(true)),
            ("touchpad.disable_with_mouse", |i| i.touchpad.disable_with_mouse = Some(false)),
            ("cursor.theme", |i| i.cursor.theme = Some("Bibata-Modern-Ice".into())),
            ("cursor.size", |i| i.cursor.size = Some(32)),
            ("cursor.hide_when_typing", |i| i.cursor.hide_when_typing = Some(true)),
            ("cursor.hide_after_secs", |i| i.cursor.hide_after_secs = Some(5)),
            ("cursor.no_hardware_cursors", |i| i.cursor.no_hardware_cursors = Some(true)),
            ("cursor.no_hardware_cursors", |i| i.cursor.no_hardware_cursors = Some(false)),
            ("cursor.hide_on_touch", |i| i.cursor.hide_on_touch = Some(true)),
            ("focus_follows_mouse", |i| i.focus_follows_mouse = Some(true)),
            ("focus_follows_mouse", |i| i.focus_follows_mouse = Some(false)),
        ];
        cases
            .into_iter()
            .map(|(key, set)| {
                let mut input = InputConfig::default();
                set(&mut input);
                (key, input)
            })
            .collect()
    }

    /// Each option alone: written exactly for the compositors
    /// [`INPUT_OPTIONS`] lists (so Studio's support notes are true), in
    /// that compositor's syntax, and never as an empty `key=` line.
    #[test]
    fn every_option_alone_matches_its_support_and_syntax() {
        // A setting line: not blank, a comment, or bare section punctuation.
        let has_settings = |text: &str| text.lines().map(str::trim).any(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("//") && l != "}" && !l.ends_with('{'));
        let mut mango_all = String::new();
        for (key, input) in single_options() {
            let support = INPUT_OPTIONS.iter().find(|(k, _)| *k == key).unwrap_or_else(|| panic!("{key} missing from INPUT_OPTIONS")).1;
            let c = CompositorConfig { input: input.clone(), ..Default::default() };
            let mango = emit_mango(&managed(&c)).text;
            let mango_input = super::emit_mango(&input);
            assert_eq!(has_settings(&mango_input), support[0], "MangoWM support of {key}:\n{mango_input}");
            for line in mango_input.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')) {
                let (k, v) = line.split_once('=').unwrap_or_else(|| panic!("{key}: not key=value: {line}"));
                assert!(!k.is_empty() && !v.is_empty() && !k.contains(' '), "{key}: bad MangoWM line {line:?}");
            }
            mango_all.push_str(&mango);
            let niri_input = super::emit_niri(&input);
            // Niri writes a switched-off touchpad / mouse switch as its block
            // (bare flags can't say "off"), and explains the one setting an
            // include can't switch off.
            let niri_written = has_settings(&niri_input) || niri_input.contains("niri replaces your whole") || niri_input.contains("can't be switched off");
            assert_eq!(niri_written, support[1], "Niri support of {key}:\n{niri_input}");
            let niri = emit_niri(&managed(&c)).text;
            kdl::KdlDocument::parse_v1(&niri).unwrap_or_else(|e| panic!("{key}: invalid KDL v1: {e:?}\n{niri}"));
            let hypr_input = super::emit_hyprland_conf(&input);
            assert_eq!(has_settings(&hypr_input), support[2], "Hyprland support of {key}:\n{hypr_input}");
            let hypr = emit_hyprland_conf(&managed(&c)).text;
            let mut depth = 0i32;
            for line in hypr.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
                if line == "}" {
                    depth -= 1;
                } else if let Some(name) = line.strip_suffix('{') {
                    assert!(!name.trim().is_empty() && !name.contains('='), "{key}: bad section {line:?}");
                    depth += 1;
                } else {
                    let (k, v) = line.split_once('=').unwrap_or_else(|| panic!("{key}: not `key = value`: {line:?}\n{hypr}"));
                    // Hyprland's own template writes empty `kb_*` strings
                    // (`kb_variant =`); every other key needs a value.
                    assert!(!k.trim().is_empty() && (!v.trim().is_empty() || k.trim().starts_with("kb_")), "{key}: empty Hyprland key or value {line:?}");
                }
                assert!(depth >= 0, "{key}: unbalanced\n{hypr}");
            }
            assert_eq!(depth, 0, "{key}: unbalanced\n{hypr}");
        }
        // MangoWM's own parser, over every case at once (later lines win).
        if std::process::Command::new("mango").arg("-v").output().is_ok() {
            let path = std::env::temp_dir().join(format!("mandara-mango-each-input-{}.conf", std::process::id()));
            std::fs::write(&path, &mango_all).unwrap();
            let out = std::process::Command::new("mango").arg("-p").arg("-c").arg(&path).output().unwrap();
            let report = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
            std::fs::remove_file(&path).ok();
            assert!(!report.contains("ERROR"), "mango -p rejected an input option:\n{report}");
        }
    }

    /// A layout list without variants (the common `us,ara`) and options
    /// made only of separators never produce an empty line MangoWM rejects.
    #[test]
    fn no_empty_mango_xkb_lines() {
        let mut input = InputConfig::default();
        input.keyboard.layouts = vec![KeyboardLayout { layout: "us".into(), variant: String::new() }, KeyboardLayout { layout: "ara".into(), variant: String::new() }];
        input.keyboard.options = Some(" , ,".into());
        let text = super::emit_mango(&input);
        assert!(text.contains("xkb_rules_layout=us,ara"), "{text}");
        assert!(!text.contains("xkb_rules_variant") && !text.contains("xkb_rules_options"), "{text}");
        assert_eq!(input.keyboard.xkb_options(), None);
    }

    /// MangoWM's own checker accepts every input key Mandara writes.
    #[test]
    fn mango_accepts_input_options() {
        if std::process::Command::new("mango").arg("-v").output().is_err() {
            return;
        }
        let c = with_extras();
        let path = std::env::temp_dir().join(format!("mandara-mango-input-{}.conf", std::process::id()));
        let text = emit_mango(&managed(&c)).text;
        std::fs::write(&path, &text).unwrap();
        let out = std::process::Command::new("mango").arg("-p").arg("-c").arg(&path).output().unwrap();
        let report = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
        std::fs::remove_file(&path).ok();
        assert!(!report.contains("ERROR"), "mango -p rejected the drop-in:\n{report}\n{text}");
        assert!(text.contains("tap_to_click=1"));
    }

    #[test]
    fn niri_dropin_with_input_parses() {
        let c = with_extras();
        let text = emit_niri(&managed(&c)).text;
        let doc = kdl::KdlDocument::parse_v1(&text).unwrap_or_else(|e| panic!("invalid KDL v1: {e:?}\n{text}"));
        assert!(doc.nodes().iter().any(|n| n.name().value() == "input"));
        assert!(doc.nodes().iter().any(|n| n.name().value() == "cursor"));
    }

    #[test]
    fn hyprland_layout_and_input_sections() {
        let c = with_extras();
        let text = emit_hyprland_conf(&managed(&c)).text;
        for needle in ["layout = master", "resize_on_border = true", "extend_border_grab_area = 12", "dwindle {\n    preserve_split = true\n    smart_split = false\n    force_split = 2\n}", "orientation = center", "mfact = 0.60", "group {\n    groupbar {\n        enabled = true", "height = 18", "kb_layout = us,ara", "env = XCURSOR_SIZE,32"] {
            assert!(text.contains(needle), "missing {needle}\n{text}");
        }
        assert_eq!(text.matches('{').count(), text.matches('}').count(), "balanced\n{text}");
        let lua = emit_hyprland_lua(&managed(&c)).text;
        for needle in ["layout = \"master\"", "dwindle = { preserve_split = true", "group = { groupbar = { enabled = true", "input = { kb_layout = \"us,ara\"", "pcall(hl.env, \"XCURSOR_THEME\", \"Bibata-Modern-Ice\")"] {
            assert!(lua.contains(needle), "missing {needle}\n{lua}");
        }
        assert_eq!(lua.matches('{').count(), lua.matches('}').count(), "balanced\n{lua}");
        // Invalid values are dropped, not written.
        let bad = CompositorConfig { hyprland: HyprlandOptions { layout: Some("spiral".into()), master_orientation: Some("diagonal".into()), ..Default::default() }, ..Default::default() };
        let text = emit_hyprland_conf(&managed(&bad)).text;
        assert!(!text.contains("spiral") && !text.contains("diagonal"));
    }

    #[test]
    fn gaps_follow_the_bar() {
        let mut cfg = crate::ShellConfig::default();
        assert!(matches!(cfg.effective_compositor(), std::borrow::Cow::Borrowed(_)));
        cfg.compositor.look.gaps_follow_bar = true;
        cfg.bar.side_gap = 9.6;
        let eff = cfg.effective_compositor();
        assert_eq!((eff.look.gaps_inner, eff.look.gaps_outer), (Some(10), Some(10)));
    }
}
