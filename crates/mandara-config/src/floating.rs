//! Plugin panels popped out of the bar into floating cards
//! (`[[panels.floating]]`): where each plugin's card last sat on each
//! output, and whether it was floating when the shell last ran (restored
//! at startup while `panels.remember_floating` is on).
//!
//! One entry per plugin and output. A plugin floats on one output at a
//! time, so at most one of its entries is `open`. Positions are the card's
//! top-left corner in logical pixels, relative to the output.

use serde::{Deserialize, Serialize};

/// Most entries kept; the oldest (first) ones go when more are added.
pub const MAX_REMEMBERED: usize = 64;

/// Where a plugin's floating card sat on one output.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FloatingPanel {
    /// The plugin id (`media`, `weather`, …).
    pub plugin: String,
    /// The output's connector name (`DP-1`).
    pub output: String,
    /// Top-left corner, in logical px from the output's top-left.
    pub x: f32,
    pub y: f32,
    /// It was floating when the shell last ran.
    pub open: bool,
}

impl FloatingPanel {
    /// The stored position, or `None` when a hand edit made it unusable.
    pub fn position(&self) -> Option<(f32, f32)> {
        (self.x.is_finite() && self.y.is_finite()).then_some((self.x, self.y))
    }
}

fn find<'a>(list: &'a [FloatingPanel], plugin: &str, output: &str) -> Option<&'a FloatingPanel> {
    list.iter().find(|e| e.plugin == plugin && e.output == output)
}

/// Where `plugin`'s card last sat on `output`.
pub fn position(list: &[FloatingPanel], plugin: &str, output: &str) -> Option<(f32, f32)> {
    find(list, plugin, output).and_then(FloatingPanel::position)
}

/// Remembers `plugin`'s card at `(x, y)` on `output` (whole pixels) and
/// whether it is floating there now. Opening it on one output closes its
/// entries on the others. Returns whether anything changed.
pub fn remember(list: &mut Vec<FloatingPanel>, plugin: &str, output: &str, (x, y): (f32, f32), open: bool) -> bool {
    let before = list.clone();
    if open {
        for e in list.iter_mut().filter(|e| e.plugin == plugin && e.output != output) {
            e.open = false;
        }
    }
    let (x, y) = if x.is_finite() && y.is_finite() { (x.round(), y.round()) } else { (0.0, 0.0) };
    match list.iter_mut().find(|e| e.plugin == plugin && e.output == output) {
        Some(e) => {
            e.x = x;
            e.y = y;
            e.open = open;
        }
        None => {
            list.push(FloatingPanel { plugin: plugin.into(), output: output.into(), x, y, open });
            let extra = list.len().saturating_sub(MAX_REMEMBERED);
            list.drain(..extra);
        }
    }
    *list != before
}

/// Marks `plugin`'s card on `output` as no longer floating (its position
/// stays remembered). Returns whether anything changed.
pub fn close(list: &mut [FloatingPanel], plugin: &str, output: &str) -> bool {
    match list.iter_mut().find(|e| e.plugin == plugin && e.output == output && e.open) {
        Some(e) => {
            e.open = false;
            true
        }
        None => false,
    }
}

/// The cards to bring back at startup: the open entries on connected
/// `outputs`, one per plugin (the first, should a hand edit open more).
pub fn to_restore<'a>(list: &'a [FloatingPanel], outputs: &[&str]) -> Vec<&'a FloatingPanel> {
    let mut out: Vec<&FloatingPanel> = Vec::new();
    for e in list.iter().filter(|e| e.open && outputs.contains(&e.output.as_str()) && e.position().is_some()) {
        if !out.iter().any(|o| o.plugin == e.plugin) {
            out.push(e);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_per_plugin_per_output() {
        let mut list = Vec::new();
        assert!(remember(&mut list, "media", "DP-1", (100.4, 200.6), true));
        assert!(remember(&mut list, "media", "HDMI-A-1", (10.0, 20.0), false));
        assert_eq!(position(&list, "media", "DP-1"), Some((100.0, 201.0)), "stored in whole pixels");
        assert_eq!(position(&list, "media", "HDMI-A-1"), Some((10.0, 20.0)));
        assert_eq!(position(&list, "weather", "DP-1"), None);
        assert!(!remember(&mut list, "media", "DP-1", (100.0, 201.0), true), "an unchanged place is not a change");
        assert_eq!(list.len(), 2, "one entry per plugin and output");
    }

    #[test]
    fn a_plugin_floats_on_one_output_at_a_time() {
        let mut list = Vec::new();
        remember(&mut list, "media", "DP-1", (1.0, 2.0), true);
        remember(&mut list, "media", "HDMI-A-1", (3.0, 4.0), true);
        let open: Vec<&str> = list.iter().filter(|e| e.open).map(|e| e.output.as_str()).collect();
        assert_eq!(open, ["HDMI-A-1"]);
        assert!(close(&mut list, "media", "HDMI-A-1"));
        assert!(!close(&mut list, "media", "HDMI-A-1"), "closing twice changes nothing");
        assert_eq!(position(&list, "media", "HDMI-A-1"), Some((3.0, 4.0)), "a closed card keeps its place");
    }

    #[test]
    fn restores_open_cards_on_connected_outputs_only() {
        let mut list = Vec::new();
        remember(&mut list, "media", "DP-1", (1.0, 2.0), true);
        remember(&mut list, "weather", "DP-2", (5.0, 6.0), true);
        remember(&mut list, "clock", "DP-1", (7.0, 8.0), false);
        list.push(FloatingPanel { plugin: "notes".into(), output: "DP-1".into(), x: f32::NAN, y: 0.0, open: true });
        let back: Vec<&str> = to_restore(&list, &["DP-1"]).iter().map(|e| e.plugin.as_str()).collect();
        assert_eq!(back, ["media"], "closed, disconnected and broken entries stay put");
        // A hand edit opening one plugin twice restores it once.
        list.push(FloatingPanel { plugin: "media".into(), output: "DP-2".into(), x: 0.0, y: 0.0, open: true });
        assert_eq!(to_restore(&list, &["DP-1", "DP-2"]).iter().filter(|e| e.plugin == "media").count(), 1);
    }

    #[test]
    fn the_list_stays_bounded() {
        let mut list = Vec::new();
        for i in 0..MAX_REMEMBERED + 5 {
            remember(&mut list, &format!("p{i}"), "DP-1", (0.0, 0.0), false);
        }
        assert_eq!(list.len(), MAX_REMEMBERED);
        assert_eq!(list[0].plugin, "p5", "the oldest entries go first");
        assert!(remember(&mut list, "nan", "DP-1", (f32::NAN, 3.0), false));
        assert_eq!(position(&list, "nan", "DP-1"), Some((0.0, 0.0)), "a non-finite position is never stored");
    }
}
