//! A fresh reminder each time Jannah opens: a uniform random pick from the
//! chosen items that never brings back the one on screen or one shown
//! lately ([`window`] of them, at most [`RING`]). What was shown lately
//! (and the desktop widget's pick for its period) is remembered in
//! `~/.local/state/mandara/jannah-recent.json`, a few hundred bytes, so a
//! restart doesn't repeat it either. Pure: the caller draws the randomness
//! ([`os_draw`] in the shell) and writes the file off the UI thread.

use std::collections::VecDeque;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::catalog::{Catalog, ItemId};
use super::rotation::Sources;

/// Most recently shown items remembered (newest kept).
pub const RING: usize = 16;
/// The file's format; any other version is ignored (a fresh start).
pub const VERSION: u32 = 1;
/// Items a pick may avoid besides the recent ones (what is on screen).
const AVOID_MAX: usize = 4;

/// How many of the latest items a pool of `n` keeps clear of: a third of
/// the pool, at most [`RING`]. One item is all there is; two alternate.
pub fn window(n: usize) -> usize {
    if n < 2 { 0 } else { RING.min(n / 3).max(1) }
}

/// The items a surface picks from: one kind (or both) of the catalog, only
/// those of at most `limit` characters when the surface has little room.
/// A kind with nothing that fits gives way to the other.
#[derive(Clone, Debug)]
pub struct Pool<'a> {
    cat: &'a Catalog,
    range: Range<u32>,
    limit: Option<usize>,
}

impl<'a> Pool<'a> {
    pub fn new(cat: &'a Catalog, sources: Sources, limit: Option<usize>) -> Self {
        let verses = cat.verses as u32;
        let (quran, hadith, both) = (0..verses, verses..cat.items.len() as u32, 0..cat.items.len() as u32);
        let has = |r: &Range<u32>| r.clone().any(|i| fits(cat, ItemId(i), limit));
        let range = match sources {
            Sources::Both => both,
            Sources::Quran if has(&quran) || !has(&hadith) => quran,
            Sources::Hadith if has(&hadith) || !has(&quran) => hadith,
            Sources::Quran => hadith,
            Sources::Hadith => quran,
        };
        Pool { cat, range, limit }
    }

    pub fn contains(&self, id: ItemId) -> bool {
        self.range.contains(&id.0) && fits(self.cat, id, self.limit)
    }

    /// The pool's items, in catalog order.
    pub fn iter(&self) -> impl Iterator<Item = ItemId> + '_ {
        self.range.clone().map(ItemId).filter(|id| fits(self.cat, *id, self.limit))
    }

    pub fn len(&self) -> usize {
        match self.limit {
            None => self.range.len(),
            Some(_) => self.iter().count(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.iter().next().is_none()
    }

    /// Where `id` stands in the pool, `(1-based number, pool size)`: fixed
    /// for an item (catalog order), whatever was picked before it. `None`
    /// when it isn't in the pool.
    pub fn number(&self, id: ItemId) -> Option<(usize, usize)> {
        if !self.contains(id) {
            return None;
        }
        let n = match self.limit {
            None => (id.0 - self.range.start) as usize + 1,
            Some(_) => self.iter().take_while(|x| *x != id).count() + 1,
        };
        Some((n, self.len()))
    }
}

fn fits(cat: &Catalog, id: ItemId, limit: Option<usize>) -> bool {
    limit.is_none_or(|l| cat.get(id).is_some_and(|i| i.len <= l))
}

/// The items shown lately, oldest first (at most [`RING`], each once).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Recent {
    ids: VecDeque<ItemId>,
}

impl Recent {
    /// `id` was shown: it becomes the newest (forgetting the oldest).
    pub fn push(&mut self, id: ItemId) {
        if let Some(at) = self.ids.iter().position(|x| *x == id) {
            self.ids.remove(at);
        }
        if self.ids.len() == RING {
            self.ids.pop_front();
        }
        self.ids.push_back(id);
    }

    /// The item shown `back` items before the newest (`0`: the newest).
    pub fn nth_newest(&self, back: usize) -> Option<ItemId> {
        self.ids.len().checked_sub(back + 1).and_then(|at| self.ids.get(at).copied())
    }

    pub fn newest_first(&self) -> impl Iterator<Item = ItemId> + '_ {
        self.ids.iter().rev().copied()
    }

    pub fn oldest_first(&self) -> impl Iterator<Item = ItemId> + '_ {
        self.ids.iter().copied()
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

/// A uniform number below `n` (`n > 0`) from `draw`, without modulo bias.
pub fn uniform(n: usize, draw: &mut impl FnMut() -> u64) -> usize {
    let n = n as u64;
    let zone = u64::MAX - u64::MAX % n;
    loop {
        let x = draw();
        if x < zone {
            return (x % n) as usize;
        }
    }
}

/// A fresh item from `pool`, uniformly at random among those that are
/// neither one of `recent`'s latest [`window`] (counting only the pool's
/// own items) nor in `avoid` (what is on screen now). A pool too small for
/// all of that only keeps clear of the newest and `avoid`, then of the
/// newest alone; one item is always itself. `None` for an empty pool.
pub fn pick(pool: &Pool, recent: &Recent, avoid: &[ItemId], draw: &mut impl FnMut() -> u64) -> Option<ItemId> {
    let n = pool.len();
    if n <= 1 {
        return pool.iter().next();
    }
    // Strict first, then gentler where the pool can't satisfy it.
    for strictness in [window(n), 1, 0] {
        let mut skip = [ItemId(u32::MAX); RING + AVOID_MAX];
        let mut len = 0;
        let mut add = |id: ItemId, len: &mut usize| {
            if *len < skip.len() && pool.contains(id) && !skip[..*len].contains(&id) {
                skip[*len] = id;
                *len += 1;
            }
        };
        for id in recent.newest_first().filter(|id| pool.contains(*id)).take(strictness) {
            add(id, &mut len);
        }
        if strictness > 0 {
            for id in avoid.iter().take(AVOID_MAX) {
                add(*id, &mut len);
            }
        }
        let skip = &skip[..len];
        let left = n - len;
        if left == 0 {
            continue;
        }
        let k = uniform(left, draw);
        return pool.iter().filter(|id| !skip.contains(id)).nth(k);
    }
    pool.iter().next()
}

/// Randomness from the operating system (a clock-seeded mix if it ever
/// refuses).
pub fn os_draw() -> u64 {
    let mut buf = [0u8; 8];
    if getrandom::fill(&mut buf).is_ok() {
        return u64::from_le_bytes(buf);
    }
    use std::sync::atomic::{AtomicU64, Ordering};
    static FALLBACK: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    // SplitMix64 over the clock and a counter.
    let mut z = nanos ^ FALLBACK.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// What Jannah remembers between sessions: the items shown lately and the
/// desktop widget's pick for its period.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    pub recent: Recent,
    /// The period (see [`super::rotation::period`]) and the item the
    /// desktop widgets show through it.
    pub widget: Option<(i64, ItemId)>,
}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    /// Item keys, oldest first.
    #[serde(default)]
    recent: Vec<String>,
    #[serde(default)]
    widget: Option<WidgetFile>,
}

#[derive(Serialize, Deserialize)]
struct WidgetFile {
    period: i64,
    key: String,
}

/// `~/.local/state/mandara/jannah-recent.json`.
pub fn path() -> PathBuf {
    mandara_config::paths::state_dir().join("jannah-recent.json")
}

impl Memory {
    /// The memory at `path`; empty when missing, unreadable, of another
    /// version or naming items the catalog no longer has.
    pub fn load(path: &Path, cat: &Catalog) -> Self {
        let Some(file) = std::fs::read(path).ok().and_then(|b| serde_json::from_slice::<File>(&b).ok()).filter(|f| f.version == VERSION) else {
            return Memory::default();
        };
        let mut recent = Recent::default();
        for id in file.recent.iter().filter_map(|k| find_key(cat, k)) {
            recent.push(id);
        }
        let widget = file.widget.and_then(|w| Some((w.period, find_key(cat, &w.key)?)));
        Memory { recent, widget }
    }

    /// Writes the memory to `path` (atomically, compact).
    pub fn save(&self, path: &Path, cat: &Catalog) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let key = |id: ItemId| cat.get(id).map(|i| i.key.clone());
        let file = File {
            version: VERSION,
            recent: self.recent.oldest_first().filter_map(key).collect(),
            widget: self.widget.and_then(|(period, id)| Some(WidgetFile { period, key: key(id)? })),
        };
        let json = serde_json::to_vec(&file).map_err(|e| e.to_string())?;
        mandara_config::store::write_atomic(path, &json).map_err(|e| format!("{}: {e:#}", path.display()))
    }
}

/// Surfaces opening within this long of each other show one reminder.
pub const OPEN_TOGETHER: Duration = Duration::from_millis(250);

/// What the panel and sidebar show, and the desktop widgets' pick: the
/// rules without a window. Every surface reads the one current reminder,
/// so surfaces open together agree; opening one again draws a new one.
#[derive(Clone, Debug, Default)]
pub struct Reminders {
    pub memory: Memory,
    current: Option<ItemId>,
    /// How far Previous stepped back through what was shown (`0`: the
    /// newest).
    back: usize,
    /// A surface just opened: the next look draws a fresh reminder.
    fresh: bool,
    /// The panel and the sidebar as last seen (open or not).
    open: (bool, bool),
    /// When a surface last opened (or one was held open on an item).
    last_open: Option<Instant>,
}

impl Reminders {
    /// The panel and sidebar as they are now; `true` when one of them just
    /// opened, so the next look draws a fresh reminder (not when it opens
    /// together with the other, within [`OPEN_TOGETHER`]).
    pub fn surfaces(&mut self, panel: bool, sidebar: bool, now: Instant) -> bool {
        let opened = (panel && !self.open.0) || (sidebar && !self.open.1);
        self.open = (panel, sidebar);
        if !opened {
            return false;
        }
        let together = self.last_open.is_some_and(|t| now.saturating_duration_since(t) < OPEN_TOGETHER);
        self.last_open = Some(now);
        if !together {
            self.fresh = true;
        }
        !together
    }

    /// A surface is about to open on purpose to show an item (the reader):
    /// the reminder the others show stays.
    pub fn hold(&mut self, now: Instant) {
        self.last_open = Some(now);
    }

    /// The reminder on screen, without drawing one.
    pub fn shown(&self) -> Option<ItemId> {
        self.current
    }

    /// The reminder to show: the current one, or a fresh one when a surface
    /// just opened, none was shown yet or the current one left `pool` (the
    /// sources changed). `true` with it when it was just drawn.
    pub fn current(&mut self, pool: &Pool, draw: &mut impl FnMut() -> u64) -> (Option<ItemId>, bool) {
        match self.current {
            Some(id) if !self.fresh && pool.contains(id) => (Some(id), false),
            _ => (self.draw(pool, draw), true),
        }
    }

    /// Next: forward again over what Previous stepped back through, then a
    /// fresh reminder.
    pub fn next(&mut self, pool: &Pool, draw: &mut impl FnMut() -> u64) -> Option<ItemId> {
        if self.back > 0
            && let Some(id) = self.memory.recent.nth_newest(self.back - 1)
        {
            self.back -= 1;
            self.current = Some(id);
            self.fresh = false;
            return self.current;
        }
        self.draw(pool, draw)
    }

    /// Previous: the reminder shown before this one (`None`, and nothing
    /// moves, when there is none).
    pub fn prev(&mut self) -> Option<ItemId> {
        let id = self.memory.recent.nth_newest(self.back + 1)?;
        self.back += 1;
        self.current = Some(id);
        self.fresh = false;
        Some(id)
    }

    pub fn has_prev(&self) -> bool {
        self.memory.recent.nth_newest(self.back + 1).is_some()
    }

    /// A look only: the reminder shown `-by` before this one (`by < 0`),
    /// this one (`0`), or another the next draw could bring (`by > 0`).
    pub fn look(&self, pool: &Pool, by: i64, draw: &mut impl FnMut() -> u64) -> Option<ItemId> {
        match by {
            0 => self.current,
            b if b < 0 => self.memory.recent.nth_newest(self.back + b.unsigned_abs() as usize),
            _ => pick(pool, &self.memory.recent, &self.on_screen(), draw),
        }
    }

    /// The desktop widgets' reminder through `period`: kept for the period,
    /// then a fresh one that is neither shown lately nor on screen. `true`
    /// with it when it was just drawn.
    pub fn widget(&mut self, pool: &Pool, period: i64, draw: &mut impl FnMut() -> u64) -> (Option<ItemId>, bool) {
        if let Some((p, id)) = self.memory.widget
            && p == period
            && pool.contains(id)
        {
            return (Some(id), false);
        }
        let id = pick(pool, &self.memory.recent, &self.on_screen(), draw);
        self.memory.widget = id.map(|id| (period, id));
        (id, true)
    }

    /// What was remembered on disk, read after the first picks: it goes
    /// before them.
    pub fn merge(&mut self, loaded: Memory) {
        let mine = std::mem::replace(&mut self.memory.recent, loaded.recent);
        for id in mine.oldest_first() {
            self.memory.recent.push(id);
        }
        if self.memory.widget.is_none() {
            self.memory.widget = loaded.widget;
        }
    }

    fn on_screen(&self) -> [ItemId; 2] {
        let none = ItemId(u32::MAX);
        [self.current.unwrap_or(none), self.memory.widget.map_or(none, |(_, id)| id)]
    }

    fn draw(&mut self, pool: &Pool, draw: &mut impl FnMut() -> u64) -> Option<ItemId> {
        self.fresh = false;
        self.back = 0;
        let id = pick(pool, &self.memory.recent, &self.on_screen(), draw);
        self.current = id;
        if let Some(id) = id {
            self.memory.recent.push(id);
        }
        id
    }
}

/// An item by its exact key (`quran-2:25`, `bukhari-3245`).
fn find_key(cat: &Catalog, key: &str) -> Option<ItemId> {
    cat.items.iter().find(|i| i.key == key).map(|i| i.id)
}

#[cfg(test)]
mod tests {
    use super::super::catalog::catalog;
    use super::*;
    use std::collections::HashSet;

    /// A deterministic stand-in for the OS (SplitMix64).
    fn seeded(seed: u64) -> impl FnMut() -> u64 {
        let mut state = seed;
        move || {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
    }

    /// Opens `times` times: each shows a pick, which becomes the newest.
    fn opens(pool: &Pool, times: usize, seed: u64) -> Vec<ItemId> {
        let mut draw = seeded(seed);
        let mut recent = Recent::default();
        let mut shown = Vec::new();
        for _ in 0..times {
            let current = recent.nth_newest(0);
            let id = pick(pool, &recent, current.as_slice(), &mut draw).unwrap();
            recent.push(id);
            shown.push(id);
        }
        shown
    }

    #[test]
    fn many_opens_never_repeat_the_current_or_a_recent_item() {
        let c = catalog();
        for sources in [Sources::Both, Sources::Quran, Sources::Hadith] {
            let pool = Pool::new(c, sources, None);
            let w = window(pool.len());
            assert_eq!(w, RING.min(pool.len() / 3), "{sources:?}");
            let shown = opens(&pool, 2000, 7);
            for (k, id) in shown.iter().enumerate() {
                assert!(pool.contains(*id));
                // Nothing among the previous `w` opens comes back.
                assert!(!shown[k.saturating_sub(w)..k].contains(id), "{sources:?}: open {k} repeats one of the last {w}");
            }
            // Random, not a walk: plenty of different items turn up.
            let distinct: HashSet<_> = shown.iter().collect();
            assert!(distinct.len() > pool.len().min(2000) / 2, "{sources:?}: only {} distinct", distinct.len());
        }
    }

    #[test]
    fn picks_spread_uniformly_over_what_is_allowed() {
        let c = catalog();
        let pool = Pool::new(c, Sources::Both, None);
        let mut recent = Recent::default();
        for id in pool.iter().take(RING) {
            recent.push(id);
        }
        let mut draw = seeded(11);
        let mut first_half = 0;
        let rounds = 20_000;
        let allowed = pool.len() - window(pool.len());
        let half = pool.iter().skip(RING).nth(allowed / 2).unwrap();
        for _ in 0..rounds {
            let id = pick(&pool, &recent, &[], &mut draw).unwrap();
            assert!(!recent.newest_first().take(window(pool.len())).any(|r| r == id));
            if id < half {
                first_half += 1;
            }
        }
        let share = first_half as f64 / rounds as f64;
        assert!((0.45..0.55).contains(&share), "share {share}");
    }

    #[test]
    fn tiny_pools_stay_and_alternate() {
        let c = catalog();
        let mut lens: Vec<(usize, ItemId)> = c.items.iter().map(|i| (i.len, i.id)).collect();
        lens.sort_unstable();
        let mut found = 0;
        // Limits that leave one, two, three and five items in the pool.
        for want in [1usize, 2, 3, 5] {
            let Some(limit) = (lens[want - 1].0 < lens[want].0).then_some(lens[want - 1].0) else { continue };
            found += 1;
            let pool = Pool::new(c, Sources::Both, Some(limit));
            assert_eq!(pool.len(), want);
            let shown = opens(&pool, 40, 3);
            match want {
                1 => assert!(shown.iter().all(|id| *id == shown[0])),
                _ => assert!(shown.windows(2).all(|w| w[0] != w[1]), "{want}: {shown:?}"),
            }
            if want == 2 {
                assert!(shown.windows(3).all(|w| w[0] == w[2]), "two items alternate");
            }
            // Whatever is on screen besides is still avoided where possible.
            let other = shown[0];
            let recent = Recent::default();
            let mut draw = seeded(5);
            for _ in 0..20 {
                let id = pick(&pool, &recent, &[other], &mut draw).unwrap();
                assert!(want == 1 || id != other);
            }
        }
        assert!(found >= 2, "the catalog's shortest items should give tiny pools");
        // Nothing fits: nothing to show.
        let none = Pool::new(c, Sources::Both, Some(0));
        assert!(none.is_empty());
        assert_eq!(none.len(), 0);
        assert_eq!(pick(&none, &Recent::default(), &[], &mut seeded(1)), None);
    }

    #[test]
    fn a_kind_with_nothing_that_fits_gives_way() {
        let c = catalog();
        let shortest_hadith = c.items[c.verses..].iter().map(|i| i.len).min().unwrap();
        let shortest_verse = c.items[..c.verses].iter().map(|i| i.len).min().unwrap();
        let quran = Pool::new(c, Sources::Quran, None);
        assert!(quran.iter().all(|id| c.get(id).unwrap().is_verse()));
        assert_eq!(quran.len(), c.verses);
        if shortest_hadith < shortest_verse {
            let pool = Pool::new(c, Sources::Quran, Some(shortest_hadith));
            assert!(!pool.is_empty() && pool.iter().all(|id| !c.get(id).unwrap().is_verse()));
        }
    }

    #[test]
    fn every_item_keeps_its_number_in_its_pool() {
        let c = catalog();
        let first_hadith = ItemId(c.verses as u32);
        let both = Pool::new(c, Sources::Both, None);
        let hadith = Pool::new(c, Sources::Hadith, None);
        let quran = Pool::new(c, Sources::Quran, None);
        assert_eq!(both.number(ItemId(0)), Some((1, c.items.len())));
        assert_eq!(both.number(first_hadith), Some((c.verses + 1, c.items.len())));
        assert_eq!(hadith.number(first_hadith), Some((1, c.items.len() - c.verses)));
        assert_eq!(quran.number(first_hadith), None);
        // Numbers run 1..=size once each, in catalog order, and never
        // depend on what was drawn.
        for pool in [&both, &hadith, &quran, &Pool::new(c, Sources::Both, Some(320))] {
            let numbers: Vec<usize> = pool.iter().map(|id| pool.number(id).unwrap().0).collect();
            assert_eq!(numbers, (1..=pool.len()).collect::<Vec<_>>());
        }
        let mut r = Reminders::default();
        let mut draw = seeded(3);
        let id = r.current(&both, &mut draw).0.unwrap();
        let before = both.number(id);
        for _ in 0..20 {
            r.next(&both, &mut draw);
        }
        assert_eq!(both.number(id), before);
    }

    #[test]
    fn the_ring_keeps_the_newest_once_each() {
        let mut r = Recent::default();
        for i in 0..(RING as u32 + 5) {
            r.push(ItemId(i));
        }
        assert_eq!(r.len(), RING);
        assert_eq!(r.nth_newest(0), Some(ItemId(RING as u32 + 4)));
        assert_eq!(r.nth_newest(RING - 1), Some(ItemId(5)));
        assert_eq!(r.nth_newest(RING), None);
        r.push(ItemId(10));
        assert_eq!(r.len(), RING);
        assert_eq!(r.nth_newest(0), Some(ItemId(10)));
        assert_eq!(r.newest_first().filter(|id| *id == ItemId(10)).count(), 1);
        assert_eq!(window(0), 0);
        assert_eq!(window(1), 0);
        assert_eq!(window(2), 1);
        assert_eq!(window(9), 3);
        assert_eq!(window(1000), RING);
    }

    #[test]
    fn uniform_has_no_bias_and_stays_below_n() {
        let mut draw = seeded(9);
        let mut counts = [0usize; 3];
        for _ in 0..30_000 {
            counts[uniform(3, &mut draw)] += 1;
        }
        assert!(counts.iter().all(|c| (9_400..10_600).contains(c)), "{counts:?}");
        // A draw in the biased zone is redrawn.
        let mut seq = [u64::MAX, 4].into_iter();
        assert_eq!(uniform(3, &mut || seq.next().unwrap()), 1);
        assert_eq!(uniform(1, &mut draw), 0);
        // The OS source works.
        let (a, b) = (os_draw(), os_draw());
        assert!(a != b || os_draw() != a);
    }

    /// The panel opens and closes `times` times, with time between.
    fn panel_opens(r: &mut Reminders, pool: &Pool, times: usize, draw: &mut impl FnMut() -> u64) -> Vec<ItemId> {
        let mut now = r.last_open.unwrap_or_else(Instant::now);
        let mut shown = Vec::new();
        for _ in 0..times {
            now += Duration::from_secs(2);
            assert!(r.surfaces(true, false, now), "an open draws afresh");
            let (id, new) = r.current(pool, draw);
            assert!(new);
            // Every later look while it stays open shows the same.
            assert_eq!(r.current(pool, draw), (id, false));
            shown.push(id.unwrap());
            now += Duration::from_secs(1);
            assert!(!r.surfaces(false, false, now), "closing draws nothing");
        }
        shown
    }

    #[test]
    fn every_open_shows_a_fresh_reminder() {
        let c = catalog();
        let pool = Pool::new(c, Sources::Both, None);
        let mut r = Reminders::default();
        let mut draw = seeded(21);
        let shown = panel_opens(&mut r, &pool, 300, &mut draw);
        let w = window(pool.len());
        for (k, id) in shown.iter().enumerate() {
            assert!(!shown[k.saturating_sub(w)..k].contains(id), "open {k} brings back one of the last {w}");
        }
        assert_eq!(r.memory.recent.len(), RING);
        assert_eq!(r.shown(), shown.last().copied());
    }

    #[test]
    fn surfaces_open_together_share_one_reminder() {
        let c = catalog();
        let pool = Pool::new(c, Sources::Both, None);
        let mut r = Reminders::default();
        let mut draw = seeded(4);
        let t0 = Instant::now();
        // The sidebar opens; its card draws.
        assert!(r.surfaces(false, true, t0));
        let (card, _) = r.current(&pool, &mut draw);
        // The panel opens a moment later: both show the same.
        assert!(!r.surfaces(true, true, t0 + Duration::from_millis(100)));
        assert_eq!(r.current(&pool, &mut draw), (card, false));
        // Both in one change: one draw.
        let t1 = t0 + Duration::from_secs(5);
        r.surfaces(false, false, t1);
        assert!(r.surfaces(true, true, t1 + Duration::from_secs(1)));
        let (both, new) = r.current(&pool, &mut draw);
        assert!(new && both != card);
        assert_eq!(r.current(&pool, &mut draw), (both, false), "the second surface reads the same");
        // The panel opens again later while the sidebar stays: a new one, which the sidebar follows.
        let t2 = t1 + Duration::from_secs(4);
        r.surfaces(false, true, t2);
        assert!(r.surfaces(true, true, t2 + Duration::from_secs(1)));
        let (again, new) = r.current(&pool, &mut draw);
        assert!(new && again != both);
        // Opening the reader on purpose keeps what the sidebar shows.
        let t3 = t2 + Duration::from_secs(5);
        r.surfaces(false, true, t3);
        r.hold(t3 + Duration::from_secs(1));
        assert!(!r.surfaces(true, true, t3 + Duration::from_secs(1)));
        assert_eq!(r.current(&pool, &mut draw), (again, false));
    }

    #[test]
    fn previous_and_next_walk_what_was_shown_then_draw() {
        let c = catalog();
        let pool = Pool::new(c, Sources::Both, None);
        let mut r = Reminders::default();
        let mut draw = seeded(8);
        assert!(!r.has_prev() && r.prev().is_none());
        let a = r.current(&pool, &mut draw).0.unwrap();
        let b = r.next(&pool, &mut draw).unwrap();
        let c2 = r.next(&pool, &mut draw).unwrap();
        assert!(a != b && b != c2 && a != c2);
        assert_eq!(r.prev(), Some(b));
        assert_eq!(r.look(&pool, -1, &mut draw), Some(a));
        assert_eq!(r.look(&pool, 0, &mut draw), Some(b));
        assert_eq!(r.prev(), Some(a));
        assert!(!r.has_prev() && r.prev().is_none(), "Previous stops at the oldest");
        assert_eq!(r.shown(), Some(a));
        assert_eq!(r.next(&pool, &mut draw), Some(b));
        assert_eq!(r.next(&pool, &mut draw), Some(c2));
        let d = r.next(&pool, &mut draw).unwrap();
        assert!(![a, b, c2].contains(&d), "then a fresh one");
        // A look ahead never moves anything and never shows what's on screen.
        for _ in 0..50 {
            let ahead = r.look(&pool, 1, &mut draw).unwrap();
            assert!(![a, b, c2, d].contains(&ahead));
        }
        assert_eq!(r.shown(), Some(d));
        // Changing the sources draws again from the new pool.
        let quran = Pool::new(c, Sources::Quran, None);
        let hadith = Pool::new(c, Sources::Hadith, None);
        let (now, _) = r.current(&hadith, &mut draw);
        assert!(hadith.contains(now.unwrap()));
        let (back, new) = r.current(&quran, &mut draw);
        assert!(new && quran.contains(back.unwrap()));
    }

    #[test]
    fn the_widget_keeps_its_period_and_avoids_what_was_shown() {
        let c = catalog();
        let limited = Pool::new(c, Sources::Both, Some(320));
        let all = Pool::new(c, Sources::Both, None);
        let mut r = Reminders::default();
        let mut draw = seeded(13);
        let mut last = None;
        for period in 0..200 {
            let opened = panel_opens(&mut r, &all, 3, &mut draw);
            let (w, new) = r.widget(&limited, period, &mut draw);
            let w = w.unwrap();
            assert!(new && limited.contains(w));
            assert!(!r.memory.recent.newest_first().take(window(limited.len())).any(|x| x == w));
            assert!(!opened.contains(&w) && Some(w) != last);
            // The same period keeps it, whatever opens meanwhile.
            panel_opens(&mut r, &all, 2, &mut draw);
            assert_eq!(r.widget(&limited, period, &mut draw), (Some(w), false));
            last = Some(w);
        }
    }

    #[test]
    fn what_was_remembered_goes_before_the_first_picks() {
        let c = catalog();
        let pool = Pool::new(c, Sources::Both, None);
        let mut loaded = Memory::default();
        for i in 0..5 {
            loaded.recent.push(ItemId(i));
        }
        loaded.widget = Some((7, ItemId(9)));
        let mut r = Reminders::default();
        let mut draw = seeded(1);
        let first = r.current(&pool, &mut draw).0.unwrap();
        r.merge(loaded);
        assert_eq!(r.memory.recent.nth_newest(0), Some(first));
        assert_eq!(r.memory.recent.len(), if first.0 < 5 { 5 } else { 6 });
        assert_eq!(r.memory.widget, Some((7, ItemId(9))));
        assert_eq!(r.prev(), r.memory.recent.nth_newest(1));
    }

    #[test]
    fn the_memory_round_trips_and_a_bad_file_is_ignored() {
        let c = catalog();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("jannah-recent.json");
        assert_eq!(Memory::load(&path, c), Memory::default());
        let mut m = Memory::default();
        for id in [ItemId(3), ItemId(c.verses as u32 + 2), ItemId(0)] {
            m.recent.push(id);
        }
        m.widget = Some((739_000, ItemId(5)));
        m.save(&path, c).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() < 1024, "a small file");
        let back = Memory::load(&path, c);
        assert_eq!(back, m);
        assert_eq!(back.recent.nth_newest(0), Some(ItemId(0)));
        // A restart keeps clear of what was just shown.
        let pool = Pool::new(c, Sources::Both, None);
        let mut draw = seeded(2);
        for _ in 0..200 {
            let id = pick(&pool, &back.recent, &[], &mut draw).unwrap();
            assert!(!back.recent.newest_first().any(|r| r == id));
        }
        // Corrupt, another version, unknown keys: a fresh start or dropped.
        std::fs::write(&path, b"{ not json").unwrap();
        assert_eq!(Memory::load(&path, c), Memory::default());
        std::fs::write(&path, br#"{"version":2,"recent":["quran-1:1"]}"#).unwrap();
        assert_eq!(Memory::load(&path, c), Memory::default());
        std::fs::write(&path, format!(r#"{{"version":1,"recent":["gone-1","{}"],"widget":{{"period":4,"key":"gone-2"}},"later":true}}"#, c.items[1].key)).unwrap();
        let partial = Memory::load(&path, c);
        assert_eq!(partial.recent.newest_first().collect::<Vec<_>>(), [ItemId(1)]);
        assert_eq!(partial.widget, None);
        std::fs::write(&path, br#"{"version":1}"#).unwrap();
        assert_eq!(Memory::load(&path, c), Memory::default());
    }
}
