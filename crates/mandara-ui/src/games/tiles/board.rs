//! 2048: slide every tile one way, equal neighbours merge once per move,
//! a new 2 (or, one time in ten, a 4) lands on a random empty cell from a
//! seeded generator. Tiles are exponents in a fixed `u8` array (1 is 2, 11
//! is 2048), sides 3 to 6. Each tile also carries a runtime id so a view
//! can animate where it went (ids aren't saved).

use smallvec::SmallVec;

use super::super::rng::Rng;

pub const MIN_SIDE: u8 = 3;
pub const MAX_SIDE: u8 = 6;
pub const MAX_CELLS: usize = (MAX_SIDE as usize) * (MAX_SIDE as usize);
pub const DEFAULT_SIDE: u8 = 4;
/// 2048 = 2¹¹.
pub const WIN_EXPONENT: u8 = 11;
/// Exponents stop here (2¹⁷ = 131072 fills a 6 × 6 board's dreams).
pub const MAX_EXPONENT: u8 = 17;
/// One spawn in this many is a 4.
const FOUR_ODDS: u32 = 10;
/// Undo steps kept (the classic single step).
pub const UNDO_STEPS: usize = 1;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

impl Dir {
    pub const ALL: [Dir; 4] = [Self::Up, Self::Down, Self::Left, Self::Right];

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "up" | "u" | "w" => Some(Self::Up),
            "down" | "d" | "s" => Some(Self::Down),
            "left" | "l" | "a" => Some(Self::Left),
            "right" | "r" => Some(Self::Right),
            _ => None,
        }
    }
}

/// Where one tile went in a move.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Slide {
    pub id: u32,
    pub from: u8,
    pub to: u8,
    /// Merged into the tile now at `to` (it disappears there).
    pub absorbed: bool,
}

/// What a move did, for the view's animation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Step {
    pub slides: SmallVec<[Slide; 16]>,
    /// Ids of the tiles that doubled.
    pub merged: SmallVec<[u32; 8]>,
    /// The new tile: `(id, cell)`.
    pub spawned: Option<(u32, u8)>,
    pub gained: u64,
}

/// A position: what undo restores.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub cells: [u8; MAX_CELLS],
    pub score: u64,
    pub rng: u64,
    pub moves: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Game {
    pub side: u8,
    pub cells: [u8; MAX_CELLS],
    pub ids: [u32; MAX_CELLS],
    next_id: u32,
    pub score: u64,
    pub moves: u32,
    pub rng: Rng,
    /// 2048 was reached (once per game).
    pub won: bool,
    /// Playing on past 2048.
    pub keep_going: bool,
    pub undo: Option<Snapshot>,
}

/// Value of an exponent (`0` for empty).
pub fn value(exp: u8) -> u64 {
    if exp == 0 { 0 } else { 1u64 << exp }
}

pub fn clamp_side(side: u8) -> u8 {
    side.clamp(MIN_SIDE, MAX_SIDE)
}

impl Game {
    /// A new game: two tiles on an empty board.
    pub fn new(side: u8, seed: u64) -> Self {
        let mut g = Self::empty(side, seed);
        g.spawn();
        g.spawn();
        g
    }

    pub fn empty(side: u8, seed: u64) -> Self {
        Self { side: clamp_side(side), cells: [0; MAX_CELLS], ids: [0; MAX_CELLS], next_id: 1, score: 0, moves: 0, rng: Rng::new(seed), won: false, keep_going: false, undo: None }
    }

    pub fn len(&self) -> usize {
        usize::from(self.side) * usize::from(self.side)
    }

    pub fn is_empty(&self) -> bool {
        self.cells[..self.len()].iter().all(|&c| c == 0)
    }

    pub fn largest(&self) -> u8 {
        self.cells[..self.len()].iter().copied().max().unwrap_or(0)
    }

    fn fresh_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        id
    }

    /// Gives every tile an id (after loading).
    pub fn assign_ids(&mut self) {
        for i in 0..self.len() {
            self.ids[i] = if self.cells[i] == 0 { 0 } else { self.fresh_id() };
        }
    }

    /// Puts a new tile on a random empty cell: `(id, cell)`.
    pub fn spawn(&mut self) -> Option<(u32, u8)> {
        let n = self.len();
        let empty = self.cells[..n].iter().filter(|&&c| c == 0).count() as u32;
        if empty == 0 {
            return None;
        }
        let mut pick = self.rng.below(empty);
        let exp = if self.rng.below(FOUR_ODDS) == 0 { 2 } else { 1 };
        for i in 0..n {
            if self.cells[i] == 0 {
                if pick == 0 {
                    self.cells[i] = exp;
                    self.ids[i] = self.fresh_id();
                    return Some((self.ids[i], i as u8));
                }
                pick -= 1;
            }
        }
        None
    }

    /// Cell index of the `k`-th cell of line `line` when moving `dir`
    /// (`k = 0` is the edge the tiles move toward).
    fn at(&self, dir: Dir, line: usize, k: usize) -> usize {
        let s = usize::from(self.side);
        match dir {
            Dir::Left => line * s + k,
            Dir::Right => line * s + (s - 1 - k),
            Dir::Up => k * s + line,
            Dir::Down => (s - 1 - k) * s + line,
        }
    }

    /// Whether `dir` would move anything.
    pub fn can_move(&self, dir: Dir) -> bool {
        let s = usize::from(self.side);
        (0..s).any(|line| {
            (1..s).any(|k| {
                let (a, b) = (self.cells[self.at(dir, line, k - 1)], self.cells[self.at(dir, line, k)]);
                b != 0 && (a == 0 || a == b)
            })
        })
    }

    /// No move is left.
    pub fn over(&self) -> bool {
        !Dir::ALL.into_iter().any(|d| self.can_move(d))
    }

    /// 2048 is on the board and you haven't chosen to keep going.
    pub fn showing_win(&self) -> bool {
        self.won && !self.keep_going
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot { cells: self.cells, score: self.score, rng: self.rng.0, moves: self.moves }
    }

    /// Slides `dir`; a move that changes nothing returns `None` (and
    /// spawns nothing).
    pub fn slide(&mut self, dir: Dir) -> Option<Step> {
        if self.showing_win() || !self.can_move(dir) {
            return None;
        }
        let before = self.snapshot();
        let s = usize::from(self.side);
        let (old_cells, old_ids) = (self.cells, self.ids);
        self.cells = [0; MAX_CELLS];
        self.ids = [0; MAX_CELLS];
        let mut step = Step::default();
        for line in 0..s {
            // The output slot being filled and whether it already merged.
            let mut out = 0usize;
            let mut open_merge = false;
            for k in 0..s {
                let from = self.at(dir, line, k);
                let exp = old_cells[from];
                if exp == 0 {
                    continue;
                }
                let id = old_ids[from];
                if out > 0 && open_merge {
                    let dest = self.at(dir, line, out - 1);
                    if self.cells[dest] == exp && exp < MAX_EXPONENT {
                        self.cells[dest] = exp + 1;
                        step.gained += value(exp + 1);
                        step.merged.push(self.ids[dest]);
                        step.slides.push(Slide { id, from: from as u8, to: dest as u8, absorbed: true });
                        open_merge = false;
                        continue;
                    }
                }
                let dest = self.at(dir, line, out);
                self.cells[dest] = exp;
                self.ids[dest] = id;
                step.slides.push(Slide { id, from: from as u8, to: dest as u8, absorbed: false });
                out += 1;
                open_merge = true;
            }
        }
        self.score += step.gained;
        self.moves = self.moves.saturating_add(1);
        if !self.won && self.largest() >= WIN_EXPONENT {
            self.won = true;
        }
        step.spawned = self.spawn();
        self.undo = Some(before);
        Some(step)
    }

    /// Takes the last move back (one step); the ids are new.
    pub fn take_back(&mut self) -> bool {
        let Some(u) = self.undo.take() else { return false };
        self.cells = u.cells;
        self.score = u.score;
        self.rng = Rng(u.rng);
        self.moves = u.moves;
        if self.largest() < WIN_EXPONENT {
            self.won = false;
            self.keep_going = false;
        }
        self.assign_ids();
        true
    }

    /// Checks a loaded game.
    pub fn valid(&self) -> bool {
        let n = self.len();
        (MIN_SIDE..=MAX_SIDE).contains(&self.side) && self.cells.iter().all(|&c| c <= MAX_EXPONENT) && self.cells[n..].iter().all(|&c| c == 0) && !self.is_empty()
            && self.undo.is_none_or(|u| u.cells.iter().all(|&c| c <= MAX_EXPONENT) && u.cells[n..].iter().all(|&c| c == 0))
    }
}

/// Per-size statistics.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Stats {
    pub played: u32,
    /// Games that reached 2048.
    pub won: u32,
    pub best: u64,
    /// Highest tile ever, as an exponent.
    pub top: u8,
}

impl Stats {
    /// Folds a game's score and tile into the records; whether the score is
    /// a new best.
    pub fn observe(&mut self, g: &Game) -> bool {
        self.top = self.top.max(g.largest());
        if g.score > self.best {
            self.best = g.score;
            return true;
        }
        false
    }
}

/// The index of a size in the statistics (3 → 0 … 6 → 3).
pub fn size_index(side: u8) -> usize {
    usize::from(clamp_side(side) - MIN_SIDE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(side: u8, rows: &[&[u64]]) -> Game {
        let mut g = Game::empty(side, 1);
        for (r, row) in rows.iter().enumerate() {
            for (c, &v) in row.iter().enumerate() {
                g.cells[r * usize::from(side) + c] = if v == 0 { 0 } else { v.trailing_zeros() as u8 };
            }
        }
        g.assign_ids();
        g
    }

    fn row(g: &Game, r: usize) -> Vec<u64> {
        let s = usize::from(g.side);
        g.cells[r * s..(r + 1) * s].iter().map(|&e| value(e)).collect()
    }

    /// A move without the random spawn, to check the rule alone.
    fn slide_only(g: &mut Game, d: Dir) -> Step {
        let step = g.slide(d).unwrap();
        if let Some((_, cell)) = step.spawned {
            g.cells[usize::from(cell)] = 0;
        }
        step
    }

    #[test]
    fn tiles_merge_once_per_move_from_the_edge_they_move_to() {
        let mut g = with(4, &[&[2, 2, 2, 2], &[2, 2, 4, 0], &[4, 2, 2, 0], &[2, 0, 2, 2]]);
        let step = slide_only(&mut g, Dir::Left);
        assert_eq!(row(&g, 0), [4, 4, 0, 0], "no double merge");
        assert_eq!(row(&g, 1), [4, 4, 0, 0], "a new 4 doesn't merge again with the 4");
        assert_eq!(row(&g, 2), [4, 4, 0, 0]);
        assert_eq!(row(&g, 3), [4, 2, 0, 0], "the pair nearest the edge merges");
        assert_eq!(step.gained, 4 * 5);
        assert_eq!(g.score, 20);
        let mut g = with(4, &[&[2, 2, 2, 0]]);
        slide_only(&mut g, Dir::Right);
        assert_eq!(row(&g, 0), [0, 0, 2, 4]);
    }

    #[test]
    fn up_and_down_work_on_columns() {
        let mut g = with(3, &[&[2, 0, 0], &[2, 0, 0], &[4, 0, 0]]);
        slide_only(&mut g, Dir::Down);
        assert_eq!((value(g.cells[0]), value(g.cells[3]), value(g.cells[6])), (0, 4, 4));
        let mut g = with(3, &[&[2, 0, 0], &[2, 0, 0], &[4, 0, 0]]);
        slide_only(&mut g, Dir::Up);
        assert_eq!((value(g.cells[0]), value(g.cells[3]), value(g.cells[6])), (4, 4, 0));
    }

    #[test]
    fn a_move_that_changes_nothing_spawns_nothing() {
        let mut g = with(4, &[&[2, 4, 8, 16]]);
        let before = g.clone();
        assert!(g.slide(Dir::Left).is_none());
        assert_eq!(g, before);
        assert!(g.slide(Dir::Down).is_some());
    }

    #[test]
    fn spawns_are_seeded_on_empty_cells_and_mostly_twos() {
        let a = Game::new(4, 99);
        assert_eq!(a, Game::new(4, 99));
        assert_ne!(a.cells, Game::new(4, 100).cells);
        assert_eq!(a.cells.iter().filter(|&&c| c != 0).count(), 2);
        let mut g = Game::empty(6, 5);
        let mut fours = 0;
        for _ in 0..36 {
            let (_, cell) = g.spawn().unwrap();
            fours += usize::from(g.cells[usize::from(cell)] == 2);
        }
        assert!(g.spawn().is_none(), "a full board takes no tile");
        assert!((1..12).contains(&fours), "{fours} fours of 36");
    }

    #[test]
    fn game_over_needs_a_full_board_without_pairs() {
        let g = with(3, &[&[2, 4, 2], &[4, 2, 4], &[2, 4, 2]]);
        assert!(g.over());
        let g = with(3, &[&[2, 4, 2], &[4, 2, 4], &[2, 4, 4]]);
        assert!(!g.over(), "a pair can still merge");
        let g = with(3, &[&[2, 4, 2], &[4, 0, 4], &[2, 4, 2]]);
        assert!(!g.over(), "an empty cell");
    }

    #[test]
    fn reaching_2048_wins_once_and_you_can_keep_going() {
        let mut g = with(4, &[&[1024, 1024, 0, 0]]);
        g.slide(Dir::Left).unwrap();
        assert!(g.won && g.showing_win());
        assert!(g.slide(Dir::Right).is_none(), "the win waits for a choice");
        g.keep_going = true;
        assert!(g.slide(Dir::Right).is_some());
        assert!(g.won);
    }

    #[test]
    fn undo_takes_one_step_back_with_the_generator() {
        let mut g = Game::new(4, 3);
        let before = (g.cells, g.score, g.rng);
        let dir = Dir::ALL.into_iter().find(|&d| g.can_move(d)).unwrap();
        g.slide(dir).unwrap();
        assert!(g.take_back());
        assert_eq!((g.cells, g.score, g.rng), before);
        assert!(!g.take_back(), "one step only");
        // The same move again deals the same tile.
        let mut h = g.clone();
        assert_eq!(g.slide(dir).map(|s| s.spawned.map(|(_, c)| c)), h.slide(dir).map(|s| s.spawned.map(|(_, c)| c)));
    }

    #[test]
    fn slides_tell_the_view_where_every_tile_went() {
        let mut g = with(4, &[&[2, 0, 2, 4]]);
        let ids = (g.ids[0], g.ids[2], g.ids[3]);
        let step = g.slide(Dir::Left).unwrap();
        let find = |id| step.slides.iter().find(|s| s.id == id).copied().unwrap();
        assert_eq!(find(ids.0), Slide { id: ids.0, from: 0, to: 0, absorbed: false });
        assert_eq!(find(ids.1), Slide { id: ids.1, from: 2, to: 0, absorbed: true });
        assert_eq!(find(ids.2), Slide { id: ids.2, from: 3, to: 1, absorbed: false });
        assert_eq!(step.merged.as_slice(), [ids.0]);
        assert_eq!(g.ids[0], ids.0, "the tile at the edge survives the merge");
    }

    #[test]
    fn stats_keep_the_best_score_and_top_tile() {
        let mut s = Stats::default();
        let mut g = with(4, &[&[512, 2]]);
        g.score = 1000;
        assert!(s.observe(&g));
        g.score = 500;
        assert!(!s.observe(&g));
        assert_eq!((s.best, s.top), (1000, 9));
        assert_eq!(size_index(3), 0);
        assert_eq!(size_index(9), 3);
    }
}
