//! Keyboard shortcuts ↔ the user's own compositor binds: which chords the
//! user's config (and every file it sources / includes, never Mandara's
//! drop-in) already uses, where, and whether the bound tool is one Mandara
//! replaces (`mandara_config::conflicts`). Read at most every few seconds.

use mandara_ui::bidi::Bidi as _;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{AnyElement, Context, Global, InteractiveElement, IntoElement, ListSizingBehavior, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px, uniform_list};
use mandara_config::compositor::{CompositorConfig, Target};
use mandara_config::conflicts;
use mandara_core::ToGpui;
use mandara_i18n::{t, tf};
use mandara_theme::{Color, MandaraPalette};

use super::StudioView;
use crate::tokens::*;

/// One bind from the user's own config (shared with the shortcuts
/// cheatsheet in the shell).
pub use mandara_config::conflicts::UserBind;

pub type UserBinds = BTreeMap<String, UserBind>;

struct Cache {
    /// What was scanned: the compositor and Mandara's own compositor config.
    key: (Option<Target>, CompositorConfig),
    binds: Rc<UserBinds>,
    /// When the scan finished (`None`: never).
    at: Option<Instant>,
    /// A scan is running on the background executor.
    pending: bool,
}
impl Global for Cache {}

/// How long a scan of the user's config files stays fresh.
const REFRESH: Duration = Duration::from_secs(3);

/// The user's own binds for `target`. The files are read off the UI thread:
/// this returns the last scan at once (empty before the first finishes) and
/// re-renders the view when a fresh one lands — rescanning when Mandara's
/// binds change (that changes which conflicts matter) or after [`REFRESH`].
pub fn user_binds(target: Option<Target>, cfg: &CompositorConfig, cx: &mut Context<StudioView>) -> Rc<UserBinds> {
    let (current, stale, pending) = match cx.try_global::<Cache>() {
        Some(c) => (c.binds.clone(), c.key.0 != target || c.key.1 != *cfg || c.at.is_none_or(|at| at.elapsed() >= REFRESH), c.pending),
        None => (Rc::default(), true, false),
    };
    if !stale || pending {
        return current;
    }
    let key = (target, cfg.clone());
    let job = cfg.clone();
    cx.set_global(Cache { key: key.clone(), binds: current.clone(), at: None, pending: true });
    let scan = cx.background_executor().spawn(async move { target.map(|t| conflicts::user_binds(t, &job)).unwrap_or_default() });
    cx.spawn(async move |this, cx| {
        let binds = scan.await;
        let _ = this.update(cx, |_, cx| {
            cx.set_global(Cache { key, binds: Rc::new(binds), at: Some(Instant::now()), pending: false });
            cx.notify();
        });
    })
    .detach();
    current
}

fn short_path(path: &std::path::Path) -> String {
    let s = path.display().to_string();
    match dirs::home_dir() {
        Some(home) => s.replacen(&home.display().to_string(), "~", 1),
        None => s,
    }
}

/// The "used by your config" badge for a Mandara bind whose chord the
/// user's config already binds (hover shows the file, line and bind).
pub fn badge(row: usize, bind: &UserBind, p: &MandaraPalette) -> AnyElement {
    let (label, color, why): (String, Color, String) = match &bind.replaces {
        Some(tool) => (tf("Used by {tool}", &[("tool", tool)]), p.urgent_coral, t("Mandara replaces this tool: retire the line in Maintenance to take the key over").to_string()),
        None => (t("Used by your config").to_string(), p.warning_amber, t("Your bind wins; Mandara leaves this chord out of its drop-in").to_string()),
    };
    let detail = format!("{}:{}  {}\n{why}", short_path(&bind.file), bind.line, bind.text);
    div()
        .id(("bind-used", row))
        .px(px(SPACE_10_PX))
        .h(px(DIM_24_PX))
        .rounded_full()
        .row()
        .items_center()
        .gap(px(SPACE_6_PX))
        .bg(color.with_alpha(ALPHA_16).hsla())
        .text_color(color.hsla())
        .text_size(px(TEXT_11_PX))
        .child(label)
        .tooltip(super::compositor_page::tip(detail, p))
        .into_any_element()
}

/// Tallest the own-binds list grows before it scrolls.
const OWN_BINDS_MAX_H: f32 = 300.0;
/// Width of the chord column.
const CHORD_COL_W: f32 = 170.0;

/// A compact, read-only list of the user's own binds — virtualized, so a
/// config with hundreds of binds lays out only the rows on screen.
pub fn own_binds(binds: Rc<UserBinds>, mandara_chords: Rc<[String]>, mono: SharedString, p: &MandaraPalette) -> AnyElement {
    if binds.is_empty() {
        return div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("No shortcuts found in your compositor config.")).into_any_element();
    }
    let count = binds.len();
    let pal = p.clone();
    let rows = uniform_list("own-binds-list", count, move |range, _, _| {
        let p = &pal;
        binds
            .values()
            .skip(range.start)
            .take(range.len())
            .map(|b| {
                let clash = mandara_chords.contains(&b.chord);
                div()
                    .px(px(SPACE_8_PX))
                    .py(px(SPACE_5_PX))
                    .rounded(px(RADIUS_8_PX))
                    .row()
                    .items_center()
                    .gap(px(SPACE_12_PX))
                    .when(clash, |el| el.bg(p.warning_amber.with_alpha(ALPHA_10).hsla()))
                    .child(div().w(px(CHORD_COL_W)).flex_none().font_family(mono.clone()).text_size(px(TEXT_12_PX)).child(b.chord.replace('+', " + ")))
                    .child(div().flex_1().min_w(px(0.0)).overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(b.text.clone()))
                    .child(div().flex_none().text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.with_alpha(ALPHA_70).hsla()).child(format!("{}:{}", b.file.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(), b.line)))
            })
            .collect::<Vec<_>>()
    })
    .with_sizing_behavior(ListSizingBehavior::Infer)
    .max_h(px(OWN_BINDS_MAX_H));
    div().rounded(px(RADIUS_14_PX)).bg(p.surface_container.hsla()).p(px(SPACE_8_PX)).child(rows).into_any_element()
}

