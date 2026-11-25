//! Whose bar items are whose: a bar preset or a Look lays out *its* items
//! on the bars, and the items you added yourself stay through every switch.
//!
//! Provenance is recorded, not guessed, wherever it can be:
//!
//! * [`crate::BarConfig::layout_items`] — the items the last preset or Look
//!   laid out (zone and place). The next one takes exactly those away.
//! * [`crate::bar_styles::StyleItems::added`] — what a bar style put in
//!   while keeping your items.
//! * [`crate::BarConfig::yours`] — items you placed or moved yourself
//!   (Studio's layout designer marks them).
//!
//! An item is **yours** when you placed or moved it, or when no preset or
//! Look put it where it is. A config written before any of this was
//! recorded (`layout_items` unset) counts the shipped bar and side bars,
//! the active Look's and the active bar style's own items as theirs, and
//! everything else as yours.
//!
//! On a switch ([`lay_out`]) the new layout's zones are filled with its own
//! items, then yours come back, never twice: an item already shown (the
//! new layout has it too) is skipped, and a plugin you removed stays away.
//! Your items keep their zone: the bar's zones always exist; an item on a
//! side bar stays there, unless that side bar showed before the switch and
//! no longer does, when it moves to the bar's zone in the same place
//! (top → start, middle → middle, bottom → end). In the start and middle
//! zones your items follow the layout's own; in the end zone they come
//! before them, so the layout's last items (the session button) keep the
//! far end. Switching back and forth is stable: nothing accumulates.

use crate::bar_styles::{ItemPlace, ItemZone};
use crate::ShellConfig;

/// A layout's items: the bar's three zones and, for a style with a rail,
/// the left rail's three groups (`None`: the rail is left as it is).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    pub bar: [Vec<String>; 3],
    pub rail: Option<[Vec<String>; 3]>,
}

impl Layout {
    pub fn from_ids(bar: [&[&str]; 3], rail: Option<[&[&str]; 3]>) -> Self {
        let own = |z: [&[&str]; 3]| z.map(|ids| ids.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        Layout { bar: own(bar), rail: rail.map(own) }
    }
}

/// Whether `id` sits in any of the zones (shown or not).
fn anywhere(cfg: &ShellConfig, id: &str) -> bool {
    ItemZone::ALL.iter().any(|z| z.get(cfg).iter().any(|x| x == id))
}

/// Whether `id` sits in a zone that shows.
fn shown(cfg: &ShellConfig, id: &str) -> bool {
    ItemZone::ALL.iter().any(|z| z.shows(cfg) && z.get(cfg).iter().any(|x| x == id))
}

/// Items a config with nothing recorded counts as the presets': the shipped
/// bar and side bars, the active Look's and the active bar style's own.
fn unrecorded_theirs(cfg: &ShellConfig) -> Vec<String> {
    let bar = crate::BarConfig::default();
    let side = crate::side_bars::SideBarsConfig::default();
    let mut out: Vec<String> = [&bar.left, &bar.center, &bar.right].into_iter().chain(side.left.zones()).chain(side.right.zones()).flatten().cloned().collect();
    if let Some(look) = crate::looks::find(&cfg.looks.active) {
        out.extend(look.zones.iter().flat_map(|z| z.iter()).map(|s| s.to_string()));
    }
    if let Some(style) = cfg.bar.style {
        out.extend(style.zones().iter().chain(style.rail().iter().flatten()).flat_map(|z| z.iter()).map(|s| s.to_string()));
    }
    out
}

/// Every item on the bars that is yours, where it is, in zone order.
pub fn yours(cfg: &ShellConfig) -> Vec<ItemPlace> {
    let theirs = if cfg.bar.layout_items.is_none() { unrecorded_theirs(cfg) } else { Vec::new() };
    let is_yours = |id: &String| {
        if cfg.bar.yours.contains(id) {
            return true;
        }
        if cfg.bar.style_items.added.iter().any(|a| a.id == *id) || cfg.bar.preset_kept.contains(id) {
            return false;
        }
        match &cfg.bar.layout_items {
            Some(placed) => !placed.iter().any(|p| p.id == *id),
            None => !theirs.contains(id),
        }
    };
    let mut out = Vec::new();
    for zone in ItemZone::ALL {
        for (index, id) in zone.get(cfg).iter().enumerate() {
            if is_yours(id) {
                out.push(ItemPlace { id: id.clone(), zone, index });
            }
        }
    }
    out
}

/// Which zones show now, in [`ItemZone::ALL`] order.
pub fn showing(cfg: &ShellConfig) -> [bool; 9] {
    ItemZone::ALL.map(|z| z.shows(cfg))
}

/// Brings `mine` (from [`yours`] before a switch, with [`showing`] then)
/// back after it: each where it was, or on the bar when its side bar
/// stopped showing; never twice; never a plugin you removed.
pub fn bring_back(cfg: &mut ShellConfig, mine: &[ItemPlace], shown_before: [bool; 9]) {
    // Your items in the end zone go before the layout's own, in order.
    let mut end_at = 0;
    for m in mine {
        if cfg.plugins.is_removed(&m.id) {
            continue;
        }
        let i = ItemZone::ALL.iter().position(|z| *z == m.zone).unwrap_or(0);
        let mut zone = m.zone;
        if !zone.shows(cfg) && shown_before[i] {
            // Its side bar went with the switch: onto the bar.
            zone.list(cfg).retain(|x| *x != m.id);
            zone = zone.on_bar();
        } else if zone.get(cfg).contains(&m.id) {
            continue;
        }
        if shown(cfg, &m.id) || (!zone.shows(cfg) && anywhere(cfg, &m.id)) {
            continue;
        }
        let list = zone.list(cfg);
        if zone == ItemZone::End {
            list.insert(end_at.min(list.len()), m.id.clone());
            end_at += 1;
        } else {
            list.push(m.id.clone());
        }
    }
    let present: Vec<String> = cfg.bar.yours.iter().filter(|id| anywhere(cfg, id)).cloned().collect();
    cfg.bar.yours = present;
}

/// Lays `layout` out on the bars: what the last preset or Look laid out
/// goes, the layout's zones get its items (skipping a plugin you removed
/// and an item of yours that still shows elsewhere), your items come back
/// ([`bring_back`]) and the layout's items are recorded. Set the side
/// bars' on / off state (a style's rail) before calling. Returns the ids
/// it placed.
pub fn lay_out(cfg: &mut ShellConfig, layout: &Layout, shown_before: [bool; 9], mine: &[ItemPlace]) -> Vec<String> {
    let redefined: Vec<ItemZone> = ItemZone::BAR.into_iter().chain(layout.rail.as_ref().map(|_| ItemZone::RAIL).into_iter().flatten()).collect();
    let mut placed = Vec::new();
    match cfg.bar.layout_items.take() {
        Some(prev) => {
            for p in prev {
                if !cfg.bar.yours.contains(&p.id) {
                    p.zone.list(cfg).retain(|x| *x != p.id);
                }
            }
        }
        // The first recorded layout: what sits in the zones it leaves alone
        // and isn't yours (a side bar's shipped items) stays the presets',
        // so it never turns into yours.
        None => {
            for zone in ItemZone::ALL.into_iter().filter(|z| !redefined.contains(z)) {
                let ids: Vec<String> = zone.get(cfg).iter().filter(|id| !mine.iter().any(|m| m.id == **id)).cloned().collect();
                cfg.bar.preset_kept.extend(ids);
            }
        }
    }
    // Yours that stay where they are (a zone this layout leaves alone).
    let kept_elsewhere = |cfg: &ShellConfig, id: &str| mine.iter().any(|m| m.id == id && !redefined.contains(&m.zone) && m.zone.shows(cfg) && m.zone.get(cfg).iter().any(|x| x == id));
    let zones = ItemZone::BAR.into_iter().zip(layout.bar.iter()).chain(layout.rail.iter().flat_map(|r| ItemZone::RAIL.into_iter().zip(r.iter())));
    let zones: Vec<(ItemZone, &Vec<String>)> = zones.collect();
    for (zone, _) in &zones {
        zone.list(cfg).clear();
    }
    for (zone, ids) in zones {
        for id in ids {
            if cfg.plugins.is_removed(id) || anywhere_in(cfg, &redefined, id) || kept_elsewhere(cfg, id) {
                continue;
            }
            let list = zone.list(cfg);
            let index = list.len();
            list.push(id.clone());
            placed.push(ItemPlace { id: id.clone(), zone, index });
        }
    }
    let ids = placed.iter().map(|p| p.id.clone()).collect();
    // An item of yours the layout has too stays yours.
    for m in mine {
        if placed.iter().any(|p| p.id == m.id) {
            mark_yours(cfg, &m.id);
        }
    }
    cfg.bar.layout_items = Some(placed);
    bring_back(cfg, mine, shown_before);
    ids
}

fn anywhere_in(cfg: &ShellConfig, zones: &[ItemZone], id: &str) -> bool {
    zones.iter().any(|z| z.get(cfg).iter().any(|x| x == id))
}

/// Marks `id` as yours (you placed or moved it).
pub fn mark_yours(cfg: &mut ShellConfig, id: &str) {
    if !cfg.bar.yours.iter().any(|x| x == id) {
        cfg.bar.yours.push(id.to_string());
    }
}

/// Forgets `id` as yours (you took it off every bar).
pub fn unmark_yours(cfg: &mut ShellConfig, id: &str) {
    if !anywhere(cfg, id) {
        cfg.bar.yours.retain(|x| x != id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(l: &[&str]) -> Vec<String> {
        l.iter().map(|s| s.to_string()).collect()
    }

    fn all(cfg: &ShellConfig) -> Vec<String> {
        ItemZone::ALL.iter().filter(|z| z.shows(cfg)).flat_map(|z| z.get(cfg).iter().cloned()).collect()
    }

    fn switch(cfg: &mut ShellConfig, layout: &Layout) {
        let (mine, before) = (yours(cfg), showing(cfg));
        lay_out(cfg, layout, before, &mine);
    }

    #[test]
    fn an_unrecorded_config_counts_only_what_you_added_as_yours() {
        let mut cfg = ShellConfig::default();
        assert!(yours(&cfg).is_empty(), "the shipped bar is nobody's own");
        cfg.bar.right.insert(0, "pomodoro".into());
        cfg.bar.left.push("my-plugin".into());
        let mine: Vec<String> = yours(&cfg).into_iter().map(|m| m.id).collect();
        assert_eq!(mine, ["my-plugin", "pomodoro"]);
    }

    #[test]
    fn your_items_survive_a_layout_and_come_once() {
        let mut cfg = ShellConfig::default();
        cfg.bar.left.push("my-plugin".into());
        cfg.bar.right.insert(0, "pomodoro".into());
        let layout = Layout::from_ids([&["launcher"], &["clock"], &["pomodoro", "session"]], None);
        switch(&mut cfg, &layout);
        assert_eq!(cfg.bar.left, ["launcher", "my-plugin"]);
        assert_eq!(cfg.bar.center, ["clock"]);
        assert_eq!(cfg.bar.right, ["pomodoro", "session"], "pomodoro is the layout's too: once");
        // Back and forth: stable.
        let other = Layout::from_ids([&["workspaces"], &[], &["audio"]], None);
        switch(&mut cfg, &other);
        let after_other = cfg.bar.clone();
        assert_eq!(cfg.bar.left, ["workspaces", "my-plugin"]);
        assert_eq!(cfg.bar.right, ["pomodoro", "audio"], "pomodoro was yours before the first layout");
        for _ in 0..3 {
            switch(&mut cfg, &layout);
            switch(&mut cfg, &other);
        }
        assert_eq!(cfg.bar, after_other, "no drift");
    }

    #[test]
    fn an_item_you_moved_is_yours_and_one_you_removed_stays_away() {
        let mut cfg = ShellConfig::default();
        let layout = Layout::from_ids([&["launcher", "workspaces"], &["clock"], &["session"]], None);
        switch(&mut cfg, &layout);
        mark_yours(&mut cfg, "clock");
        cfg.bar.center.clear();
        cfg.bar.right.insert(0, "clock".into());
        cfg.bar.left.push("weather".into());
        cfg.plugins.removed.push("weather".into());
        let other = Layout::from_ids([&["launcher"], &[], &["audio"]], None);
        switch(&mut cfg, &other);
        assert_eq!(cfg.bar.right, ["clock", "audio"]);
        assert!(!all(&cfg).contains(&"weather".to_string()));
    }

    #[test]
    fn a_side_bar_item_moves_to_the_bar_when_its_side_bar_goes() {
        let mut cfg = ShellConfig::default();
        cfg.side_bars.enabled = true;
        cfg.side_bars.left.enabled = true;
        cfg.side_bars.left.bottom = ids(&["my-plugin"]);
        let (mine, before) = (yours(&cfg), showing(&cfg));
        cfg.side_bars.enabled = false;
        lay_out(&mut cfg, &Layout::from_ids([&["launcher"], &[], &["session"]], None), before, &mine);
        assert!(cfg.side_bars.left.bottom.is_empty());
        assert_eq!(cfg.bar.right, ["my-plugin", "session"]);
        // A side bar that wasn't showing keeps its items.
        let mut off = ShellConfig::default();
        off.side_bars.left.top = ids(&["my-plugin"]);
        switch(&mut off, &Layout::from_ids([&["launcher"], &[], &[]], None));
        assert_eq!(off.side_bars.left.top, ["my-plugin"]);
        assert_eq!(off.bar.left, ["launcher"]);
    }
}
