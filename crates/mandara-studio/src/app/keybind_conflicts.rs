//! Keyboard shortcuts ↔ the user's own compositor binds: which chords the
//! user's config (and every file it sources / includes, never Mandara's
//! drop-in) already uses, where, and whether the bound tool is one Mandara
//! replaces (`mandara_config::conflicts`). Read at most every few seconds.

use mandara_ui::bidi::Bidi as _;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{AnyElement, Context, Global, InteractiveElement, IntoElement, ListSizingBehavior, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px, uniform_list};
use mandara_config::compositor::{CompositorConfig, Target};
use mandara_config::conflicts::{self, Kind};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf};
use mandara_theme::{Color, MandaraPalette};

use super::StudioView;
use crate::tokens::*;

/// One bind from the user's own config.
#[derive(Clone, Debug, PartialEq)]
pub struct UserBind {
    pub chord: String,
    pub file: PathBuf,
    /// 1-based line number.
    pub line: usize,
    pub text: String,
    /// A tool Mandara replaces (old launcher, wallpaper picker…).
    pub replaces: Option<String>,
}

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

/// Binds in `text` (one file) with their line numbers: the same whole-file
/// parse as the conflict scanner (`conflicts::file_binds`), so Hyprland's
/// `$mainMod` variables and Lua `hl.bind` chords built from `local`s count.
pub fn binds_in(target: Target, file: &std::path::Path, text: &str) -> Vec<UserBind> {
    let lines: Vec<&str> = text.lines().collect();
    conflicts::file_binds(target, text)
        .into_iter()
        .map(|(start, _, chord)| UserBind { chord, file: file.to_path_buf(), line: start + 1, text: lines.get(start).map_or_else(String::new, |l| l.trim().to_string()), replaces: None })
        .collect()
}

fn scan(target: Target, cfg: &CompositorConfig) -> UserBinds {
    let mut map = UserBinds::new();
    for file in conflicts::config_files(target) {
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        for b in binds_in(target, &file, &text) {
            // Later files win, like the compositor (and Mandara's drop-in last).
            map.insert(b.chord.clone(), b);
        }
    }
    for c in conflicts::scan(target, cfg) {
        if let Kind::Keybind { chord, replaces: Some(tool), .. } = c.kind
            && let Some(b) = map.get_mut(&chord)
        {
            b.replaces = Some(tool);
        }
    }
    map
}

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
    let scan = cx.background_executor().spawn(async move { target.map(|t| scan(t, &job)).unwrap_or_default() });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_binds_with_lines_per_compositor() {
        let f = std::path::Path::new("/x/config.conf");
        let mango = binds_in(Target::Mango, f, "blur=1\nbind=SUPER,Return,spawn,ghostty\n# bind=SUPER,x,nope\nbind=CTRL+ALT,Delete,quit\n");
        assert_eq!(mango.iter().map(|b| (b.chord.as_str(), b.line)).collect::<Vec<_>>(), vec![("super+return", 2), ("alt+ctrl+delete", 4)]);
        let niri = binds_in(Target::Niri, f, "binds {\n    Mod+T { spawn \"alacritty\"; }\n    // Mod+X { quit; }\n    Mod+Shift+E {\n        quit;\n    }\n}\nlayout { gaps 8; }\n");
        assert_eq!(niri.iter().map(|b| (b.chord.as_str(), b.line)).collect::<Vec<_>>(), vec![("super+t", 2), ("shift+super+e", 4)]);
        let hypr = binds_in(Target::Hyprland, f, "bind = SUPER, Q, exec, kitty\n#bind = SUPER, W, exec, x\n");
        assert_eq!(hypr.len(), 1);
        assert_eq!(hypr[0].text, "bind = SUPER, Q, exec, kitty");
    }

    #[test]
    fn hyprland_variables_and_lua_locals_resolve() {
        let f = std::path::Path::new("/x/hyprland.conf");
        let conf = "$mainMod = SUPER\n$alt = ALT\nbind = $mainMod, Q, exec, kitty\nbind = $mainMod SHIFT, E, exit\n# bind = $mainMod, W, exec, x\nbindl = $alt, F1, exec, y\n";
        let got: Vec<(String, usize)> = binds_in(Target::Hyprland, f, conf).into_iter().map(|b| (b.chord, b.line)).collect();
        assert_eq!(got, vec![("super+q".to_string(), 3), ("shift+super+e".to_string(), 4), ("alt+f1".to_string(), 6)]);
        let lua = std::path::Path::new("/x/hyprland.lua");
        let text = "local mod = \"SUPER\"\nlocal smod = mod .. \" + SHIFT\"\nhl.bind(mod .. \" + Return\", hl.exec(\"kitty\"))\n-- hl.bind(mod .. \" + X\", hl.exec(\"x\"))\nhl.bind(smod .. \" + Q\", hl.kill())\n";
        let got: Vec<(String, usize)> = binds_in(Target::Hyprland, lua, text).into_iter().map(|b| (b.chord, b.line)).collect();
        assert_eq!(got, vec![("super+return".to_string(), 3), ("shift+super+q".to_string(), 5)]);
    }
}
