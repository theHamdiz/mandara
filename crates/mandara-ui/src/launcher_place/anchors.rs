//! Output-local bar geometry used while the launcher is open.

use gpui::{App, Global};
use smallvec::SmallVec;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BarAnchor {
    /// Visible horizontal extent, in output-local logical pixels.
    pub span: (f32, f32),
    pub button: Option<(f32, f32)>,
}

#[derive(Default)]
struct Anchors(SmallVec<[(String, BarAnchor); 2]>);
impl Global for Anchors {}

impl Anchors {
    fn get(&self, output: Option<&str>) -> Option<BarAnchor> {
        match output {
            Some(name) => self.0.iter().find(|(output, _)| output == name),
            None if self.0.len() == 1 => self.0.first(),
            None => None,
        }.map(|(_, anchor)| *anchor)
    }

    fn set(&mut self, output: &str, anchor: Option<BarAnchor>) {
        if let Some(index) = self.0.iter().position(|(name, _)| name == output) {
            match anchor {
                Some(anchor) => self.0[index].1 = anchor,
                None => { self.0.remove(index); self.0.shrink_to_fit(); }
            }
        } else if let Some(anchor) = anchor {
            self.0.push((output.to_owned(), anchor));
        }
    }
}

/// Updating an existing output retains its name allocation. Closing removes it.
pub fn publish_anchor(output: &str, anchor: Option<BarAnchor>, cx: &mut App) {
    let current = cx.try_global::<Anchors>().and_then(|anchors| anchors.get(Some(output)));
    if current == anchor { return; }
    cx.default_global::<Anchors>().set(output, anchor);
}

pub fn anchor_on(output: Option<&str>, cx: &App) -> Option<BarAnchor> {
    cx.try_global::<Anchors>()?.get(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_updates_reuse_storage_and_close_removes_stale_geometry() {
        let mut anchors = Anchors::default();
        let first = BarAnchor { span: (600.0, 700.0), button: Some((620.0, 30.0)) };
        anchors.set("output-a", Some(first));
        let name = anchors.0[0].0.as_ptr();
        let moved = BarAnchor { span: (650.0, 600.0), ..first };
        anchors.set("output-a", Some(moved));
        assert_eq!(anchors.0[0].0.as_ptr(), name);
        assert_eq!(anchors.get(None), Some(moved));
        anchors.set("output-b", Some(first));
        assert_eq!(anchors.get(None), None, "unknown output must not use another display's coordinates");
        assert_eq!(anchors.get(Some("output-a")), Some(moved));
        assert_eq!(anchors.get(Some("output-b")), Some(first));
        anchors.set("output-a", None);
        anchors.set("output-b", None);
        assert!(anchors.0.is_empty());
        assert!(!anchors.0.spilled());
        assert_eq!(anchors.get(Some("output-a")), None);
    }
}
