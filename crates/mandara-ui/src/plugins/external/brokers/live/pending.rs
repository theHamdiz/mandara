//! Preparation belongs to the latest request on each display.

use std::collections::HashMap;
use std::rc::Rc;
use std::path::{Path, PathBuf};

use super::WallpaperKey;

pub(super) struct Start {
    plugin: String,
    root: PathBuf,
    generation: u64,
    wallpaper: WallpaperKey,
}

#[derive(Default)]
pub(super) struct Pending(HashMap<String, Rc<Start>>);

impl Pending {
    pub fn begin(
        &mut self,
        plugin: &str,
        root: &Path,
        generation: u64,
        outputs: &[String],
        wallpaper: WallpaperKey,
    ) -> Rc<Start> {
        let start = Rc::new(Start {
            plugin: plugin.into(),
            root: root.to_path_buf(),
            generation,
            wallpaper,
        });
        for output in outputs {
            self.0.insert(output.clone(), start.clone());
        }
        start
    }

    pub fn complete(
        &mut self,
        start: &Rc<Start>,
        mut outputs: Vec<String>,
        wallpaper: &WallpaperKey,
        open: bool,
    ) -> Vec<String> {
        outputs.retain(|output| {
            if !self
                .0
                .get(output)
                .is_some_and(|current| Rc::ptr_eq(current, start))
            {
                return false;
            }
            self.0.remove(output);
            open && start.wallpaper == *wallpaper
        });
        if self.0.is_empty() {
            self.0.shrink_to_fit();
        }
        outputs
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn owns(&self, start: &Rc<Start>) -> bool {
        self.0.values().any(|current| Rc::ptr_eq(current, start))
    }

    pub fn cancel(&mut self, start: &Rc<Start>) {
        self.0.retain(|_, current| !Rc::ptr_eq(current, start));
        if self.0.is_empty() { self.0.shrink_to_fit(); }
    }

    pub fn retain_allowed(&mut self, mut allowed: impl FnMut(&str, &Path, u64, &str) -> bool) -> bool {
        let before = self.0.len();
        self.0.retain(|output, start| allowed(&start.plugin, &start.root, start.generation, output));
        if self.0.is_empty() { self.0.shrink_to_fit(); }
        self.0.len() != before
    }

    pub fn wallpaper_changed(&mut self, wallpaper: &WallpaperKey) {
        self.0.retain(|_, start| start.wallpaper == *wallpaper);
        if self.0.is_empty() {
            self.0.shrink_to_fit();
        }
    }


    pub fn stop_plugin(&mut self, plugin: &str, only: Option<&str>) {
        self.0.retain(|output, start| {
            start.plugin != plugin || only.is_some_and(|only| only != output)
        });
        if self.0.is_empty() {
            self.0.shrink_to_fit();
        }
    }

    pub fn clear(&mut self) {
        self.0.clear();
        self.0.shrink_to_fit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wallpaper(path: &str) -> WallpaperKey {
        WallpaperKey {
            enabled: true,
            path: Some(path.into()),
            variants: Default::default(),
        }
    }

    #[test]
    fn revoked_scopes_cannot_be_restored_to_an_earlier_preparation() {
        let mut pending = Pending::default();
        let outputs = vec!["DP-1".to_string()];
        let request = pending.begin("a", Path::new("/approved"), 0, &outputs, wallpaper("/a"));
        assert!(pending.owns(&request));
        assert!(pending.retain_allowed(|plugin, root, _, _| plugin != "a" || root != Path::new("/approved")));
        assert!(!pending.owns(&request));
        pending.retain_allowed(|_, _, _, _| true);
        assert!(pending.complete(&request, outputs, &wallpaper("/a"), true).is_empty());
    }

    #[test]
    fn a_coalesced_permission_change_invalidates_only_its_earlier_preparation() {
        let mut pending = Pending::default();
        let old = pending.begin("a", Path::new("/approved"), 1, &["DP-1".into()], wallpaper("/a"));
        let other = pending.begin("b", Path::new("/approved"), 1, &["DP-2".into()], wallpaper("/a"));
        assert!(pending.retain_allowed(|plugin, _, generation, _| plugin != "a" || generation == 3));
        assert!(!pending.owns(&old));
        assert!(pending.owns(&other));
    }

    #[test]
    fn cancelling_a_superseded_request_keeps_the_newer_owner() {
        let mut pending = Pending::default();
        let outputs = vec!["DP-1".to_string()];
        let old = pending.begin("a", Path::new("/approved"), 0, &outputs, wallpaper("/a"));
        let new = pending.begin("b", Path::new("/other"), 0, &outputs, wallpaper("/a"));
        pending.cancel(&old);
        assert!(pending.owns(&new));
        pending.cancel(&new);
        assert!(pending.is_empty());
    }

    #[test]
    fn returning_to_an_earlier_wallpaper_does_not_revive_its_preparation() {
        let mut pending = Pending::default();
        let outputs = vec!["DP-1".to_string()];
        let request = pending.begin("a", Path::new("/root"), 0, &outputs, wallpaper("/a"));
        pending.wallpaper_changed(&wallpaper("/b"));
        pending.wallpaper_changed(&wallpaper("/a"));
        assert!(
            pending
                .complete(&request, outputs, &wallpaper("/a"), true)
                .is_empty()
        );
    }

    #[test]
    fn preparation_order_cannot_replace_a_newer_selection() {
        let mut pending = Pending::default();
        let outputs = vec!["DP-1".to_string(), "DP-2".to_string()];
        let old = pending.begin("a", Path::new("/root"), 0, &outputs, wallpaper("/a"));
        let new = pending.begin("b", Path::new("/root"), 0, &outputs[..1], wallpaper("/a"));
        assert_eq!(
            pending.complete(&old, outputs.clone(), &wallpaper("/a"), true),
            ["DP-2"]
        );
        assert_eq!(
            pending.complete(&new, vec!["DP-1".into()], &wallpaper("/a"), true),
            ["DP-1"]
        );
    }

    #[test]
    fn stop_cancel_and_wallpaper_changes_reject_late_preparations() {
        let outputs = vec!["DP-1".to_string()];
        let mut pending = Pending::default();
        let request = pending.begin("a", Path::new("/root"), 0, &outputs, wallpaper("/a"));
        pending.stop_plugin("a", Some("DP-1"));
        assert!(
            pending
                .complete(&request, outputs.clone(), &wallpaper("/a"), true)
                .is_empty()
        );
        let request = pending.begin("a", Path::new("/root"), 0, &outputs, wallpaper("/a"));
        assert!(
            pending
                .complete(&request, outputs.clone(), &wallpaper("/a"), false)
                .is_empty()
        );
        let request = pending.begin("a", Path::new("/root"), 0, &outputs, wallpaper("/a"));
        assert!(
            pending
                .complete(&request, outputs.clone(), &wallpaper("/b"), true)
                .is_empty()
        );
        let request = pending.begin("a", Path::new("/root"), 0, &outputs, wallpaper("/a"));
        pending.clear();
        assert!(
            pending
                .complete(&request, outputs, &wallpaper("/a"), true)
                .is_empty()
        );
    }

    #[test]
    fn stopping_one_plugins_display_preserves_the_other_requests() {
        let outputs = vec!["DP-1".to_string(), "DP-2".to_string()];
        let mut pending = Pending::default();
        let first = pending.begin("a", Path::new("/root"), 0, &outputs, wallpaper("/a"));
        let other = pending.begin("b", Path::new("/root"), 0, &["DP-3".into()], wallpaper("/a"));
        pending.stop_plugin("a", Some("DP-1"));
        assert_eq!(
            pending.complete(&first, outputs, &wallpaper("/a"), true),
            ["DP-2"]
        );
        assert_eq!(
            pending.complete(&other, vec!["DP-3".into()], &wallpaper("/a"), true),
            ["DP-3"]
        );
    }
}
