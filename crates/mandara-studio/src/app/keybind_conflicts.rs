//! Keyboard shortcuts ↔ the user's own compositor binds: which chords the
//! user's config (and every file it sources / includes, never Mandara's
//! drop-in) already uses, where, and whether the bound tool is one Mandara
//! replaces (`mandara_config::conflicts`). Read at most every few seconds.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use mandara_ui::bidi::Bidi as _;

use gpui::{
    AnyElement, Context, Global, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, div, px,
};
use mandara_config::compositor::{CompositorConfig, Keybind, Target};
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

#[derive(Clone, PartialEq)]
struct Key {
    target: Option<Target>,
    bindings: Vec<Keybind>,
}
#[derive(Default)]
struct Cache {
    requests: crate::latest::Latest<Key>,
    binds: Rc<UserBinds>,
    at: Option<Instant>,
}
impl Global for Cache {}

/// Refresh on a visible-page interaction, with no idle timer.
const REFRESH: Duration = Duration::from_secs(3);

pub fn release(cx: &mut Context<StudioView>) {
    if cx.has_global::<Cache>() {
        let cache = cx.global_mut::<Cache>();
        cache.requests.set(None);
        cache.binds = Rc::default();
        cache.at = None;
    }
}

pub fn user_binds(
    target: Option<Target>,
    cfg: &CompositorConfig,
    cx: &mut Context<StudioView>,
) -> Rc<UserBinds> {
    if !cx.has_global::<Cache>() {
        cx.set_global(Cache::default());
    }
    let cache = cx.global_mut::<Cache>();
    let same = cache
        .requests
        .desired()
        .is_some_and(|key| key.target == target && key.bindings == cfg.keybinds);
    if !same {
        cache.requests.set(Some(Key {
            target,
            bindings: cfg.keybinds.clone(),
        }));
    } else if cache.at.is_some_and(|at| at.elapsed() >= REFRESH) {
        cache.requests.refresh();
        cache.at = None;
    }
    let current = cache.binds.clone();
    let Some(request) = cache.requests.begin() else {
        return current;
    };
    let key = request.key.clone();
    cx.spawn(async move |view, cx| {
        let binds = mandara_services::runtime::unblock(move || {
            let config = CompositorConfig {
                keybinds: key.bindings,
                ..Default::default()
            };
            key.target
                .map(|target| conflicts::user_binds(target, &config))
                .unwrap_or_default()
        })
        .await;
        let _ = view.update(cx, |_, cx| {
            let cache = cx.global_mut::<Cache>();
            if cache.requests.finish(request) {
                cache.binds = Rc::new(binds);
                cache.at = Some(Instant::now());
            }
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
        Some(tool) => (
            tf("Used by {tool}", &[("tool", tool)]),
            p.urgent_coral,
            t("Mandara replaces this tool: retire the line in Maintenance to take the key over")
                .to_string(),
        ),
        None => (
            t("Used by your config").to_string(),
            p.warning_amber,
            t("Your bind wins; Mandara leaves this chord out of its drop-in").to_string(),
        ),
    };
    let detail = format!(
        "{}:{}  {}\n{why}",
        short_path(&bind.file),
        bind.line,
        bind.text
    );
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
