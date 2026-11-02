//! The launcher's icon ledger: every colour app icon the launcher draws is
//! decoded at the size it is drawn ([`crate::icon_raster`]) and recorded
//! here, so the launcher can free what it no longer shows.
//!
//! * **Bounded while open.** The decoded bitmaps it holds are capped at
//!   [`BUDGET_BYTES`]; scrolling a long grid evicts the least recently
//!   drawn icons first. Icons drawn in the frame just painted are never
//!   evicted (the visible set always stays decoded, so nothing blinks).
//! * **Nothing left when it closes.** The launcher drains the ledger and
//!   drops every bitmap (and its atlas texture) it decoded.
//!
//! Plain data: no GPUI state, so the bounds are unit-tested.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Most decoded icon memory the launcher keeps while open, bytes (the
/// visible set may exceed it on its own: it is never evicted).
pub const BUDGET_BYTES: usize = 8 * 1024 * 1024;

/// Bytes of one decoded icon bitmap `edge` device pixels square (BGRA).
pub fn bitmap_bytes(edge: u32) -> usize {
    (edge as usize).pow(2) * 4
}

/// Which icons were decoded, at which edge, and when they were last drawn.
#[derive(Debug)]
pub struct IconLedger {
    /// `(path, edge)` → the frame it was last drawn in.
    seen: HashMap<(PathBuf, u32), u64>,
    frame: u64,
    budget: usize,
    bytes: usize,
}

impl Default for IconLedger {
    fn default() -> Self {
        Self::new(BUDGET_BYTES)
    }
}

impl IconLedger {
    pub fn new(budget: usize) -> Self {
        Self { seen: HashMap::new(), frame: 0, budget, bytes: 0 }
    }

    /// Records that `path` is drawn this frame from a bitmap `edge` px square.
    pub fn touch(&mut self, path: &Path, edge: u32) {
        let frame = self.frame;
        match self.seen.get_mut(&(path.to_path_buf(), edge)) {
            Some(last) => *last = frame,
            None => {
                self.seen.insert((path.to_path_buf(), edge), frame);
                self.bytes += bitmap_bytes(edge);
            }
        }
    }

    /// Starts a frame: returns the icons to evict (least recently drawn
    /// first, never one drawn in the frame that just ended) until the
    /// ledger is back under its budget.
    pub fn begin_frame(&mut self) -> Vec<(PathBuf, u32)> {
        let mut out = Vec::new();
        if self.bytes > self.budget {
            let protected = self.frame;
            let mut old: Vec<((PathBuf, u32), u64)> = self.seen.iter().filter(|(_, last)| **last < protected).map(|(k, l)| (k.clone(), *l)).collect();
            old.sort_by_key(|(_, last)| *last);
            for (key, _) in old {
                if self.bytes <= self.budget {
                    break;
                }
                self.seen.remove(&key);
                self.bytes -= bitmap_bytes(key.1);
                out.push(key);
            }
        }
        self.frame += 1;
        out
    }

    /// Everything still decoded (the launcher is closing).
    pub fn drain(&mut self) -> Vec<(PathBuf, u32)> {
        self.bytes = 0;
        self.seen.drain().map(|(k, _)| k).collect()
    }

    /// Decoded bytes the launcher holds now.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icon(i: usize) -> PathBuf {
        PathBuf::from(format!("/usr/share/icons/hicolor/scalable/apps/app{i}.svg"))
    }

    /// Scrolling a 1000-app grid a row at a time keeps the decoded icons
    /// under the budget plus one screenful, and closing frees them all.
    #[test]
    fn scrolling_a_huge_grid_stays_bounded_and_closing_frees_everything() {
        // 96 px icons on a 2× output: 192² × 4 B = 144 KiB each.
        let edge = crate::icon_raster::edge_for(96.0, 2.0);
        let (cols, rows) = (8, 6);
        let screenful = cols * rows * bitmap_bytes(edge);
        let mut ledger = IconLedger::default();
        let mut peak = 0;
        for first_row in 0..(1000 / cols) {
            let evicted = ledger.begin_frame();
            assert!(evicted.iter().all(|(p, _)| {
                let i: usize = p.file_stem().unwrap().to_str().unwrap()[3..].parse().unwrap();
                i < first_row * cols
            }), "only icons scrolled away are evicted");
            for i in first_row * cols..((first_row + rows) * cols).min(1000) {
                ledger.touch(&icon(i), edge);
            }
            peak = peak.max(ledger.bytes());
        }
        eprintln!("launcher icons: peak {} KiB for 1000 apps at 96 px × 2 (budget {} KiB, one screen {} KiB)", peak / 1024, BUDGET_BYTES / 1024, screenful / 1024);
        assert!(peak <= BUDGET_BYTES + screenful, "peak {peak} B");
        let drained = ledger.drain();
        assert!(!drained.is_empty() && ledger.is_empty() && ledger.bytes() == 0);
    }

    #[test]
    fn the_visible_set_is_never_evicted_even_over_budget() {
        let edge = 64;
        let mut ledger = IconLedger::new(bitmap_bytes(edge) * 2);
        ledger.begin_frame();
        for i in 0..10 {
            ledger.touch(&icon(i), edge);
        }
        // The same ten stay on screen: nothing to evict.
        assert!(ledger.begin_frame().is_empty());
        for i in 0..10 {
            ledger.touch(&icon(i), edge);
        }
        assert_eq!(ledger.len(), 10);
        // Redrawing one icon twice in a frame counts it once.
        ledger.touch(&icon(0), edge);
        assert_eq!(ledger.bytes(), 10 * bitmap_bytes(edge));
        // Two of them stay on screen: the other eight go, the two stay.
        ledger.begin_frame();
        ledger.touch(&icon(3), edge);
        ledger.touch(&icon(4), edge);
        let gone = ledger.begin_frame();
        assert_eq!(gone.len(), 8);
        assert!(!gone.iter().any(|(p, _)| *p == icon(3) || *p == icon(4)));
        assert_eq!(ledger.bytes(), 2 * bitmap_bytes(edge));
    }

    #[test]
    fn sizes_are_separate_bitmaps() {
        let mut ledger = IconLedger::default();
        ledger.touch(&icon(1), 48);
        ledger.touch(&icon(1), 72);
        assert_eq!(ledger.len(), 2);
        assert_eq!(ledger.bytes(), bitmap_bytes(48) + bitmap_bytes(72));
    }
}
