//! A Sudoku game in progress: the puzzle, what you entered, your pencil
//! marks, mistakes, hints and a bounded undo / redo history. Pure data (no
//! GPUI), so the rules are unit-tested; the shell's model wraps it with the
//! clock and saving.

use std::collections::VecDeque;

use super::grid::{self, CELLS, Cells, Difficulty, PEERS, bit};

/// Moves kept for undo (and saved with the game).
pub const MAX_UNDO: usize = 200;

/// One change to one cell, enough to undo and redo it exactly.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Move {
    pub cell: u8,
    pub before: (u8, u16),
    pub after: (u8, u16),
    /// Peers whose pencil mark for the placed digit was cleared with it.
    pub cleared: u128,
}

/// What an entry did.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    /// The board changed.
    pub changed: bool,
    /// The digit disagrees with the solution (counted once per entry).
    pub mistake: bool,
    /// This entry finished the puzzle.
    pub won: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Game {
    pub difficulty: Difficulty,
    pub seed: u64,
    /// Which cells are givens.
    pub givens: u128,
    pub solution: Cells,
    pub values: Cells,
    /// Pencil marks per cell (bit `d - 1`).
    pub notes: [u16; CELLS],
    pub mistakes: u32,
    pub hints: u32,
    /// Time played before the current run of the clock, ms.
    pub elapsed_ms: u64,
    pub won: bool,
    pub undo: VecDeque<Move>,
    pub redo: Vec<Move>,
}

pub fn is_set(mask: u128, i: usize) -> bool {
    mask & (1u128 << i) != 0
}

impl Game {
    pub fn new(difficulty: Difficulty, seed: u64, puzzle: &grid::Puzzle) -> Self {
        let givens = (0..CELLS).filter(|&i| puzzle.givens[i] != 0).fold(0u128, |m, i| m | 1u128 << i);
        Self {
            difficulty,
            seed,
            givens,
            solution: puzzle.solution,
            values: puzzle.givens,
            notes: [0; CELLS],
            mistakes: 0,
            hints: 0,
            elapsed_ms: 0,
            won: false,
            undo: VecDeque::new(),
            redo: Vec::new(),
        }
    }

    pub fn is_given(&self, i: usize) -> bool {
        is_set(self.givens, i)
    }

    /// The puzzle as dealt.
    pub fn puzzle(&self) -> Cells {
        std::array::from_fn(|i| if self.is_given(i) { self.solution[i] } else { 0 })
    }

    /// Whether `i` holds a digit that clashes with a peer.
    pub fn conflicts(&self, i: usize) -> bool {
        let d = self.values[i];
        d != 0 && PEERS[i].iter().any(|&p| self.values[p as usize] == d)
    }

    /// Whether `i` holds a digit that isn't the solution's.
    pub fn wrong(&self, i: usize) -> bool {
        self.values[i] != 0 && self.values[i] != self.solution[i]
    }

    /// How many of digit `d` are on the board (a digit with all nine is done).
    pub fn placed(&self, d: u8) -> usize {
        self.values.iter().filter(|&&v| v == d).count()
    }

    /// Empty cells left.
    pub fn left(&self) -> usize {
        self.values.iter().filter(|&&v| v == 0).count()
    }

    fn apply(&mut self, m: &Move, forward: bool) {
        let i = m.cell as usize;
        let (v, n) = if forward { m.after } else { m.before };
        self.values[i] = v;
        self.notes[i] = n;
        let placed = m.after.0;
        if placed != 0 && m.cleared != 0 {
            for p in (0..CELLS).filter(|&p| is_set(m.cleared, p)) {
                if forward {
                    self.notes[p] &= !bit(placed);
                } else {
                    self.notes[p] |= bit(placed);
                }
            }
        }
    }

    fn record(&mut self, m: Move) {
        self.apply(&m, true);
        self.undo.push_back(m);
        while self.undo.len() > MAX_UNDO {
            self.undo.pop_front();
        }
        self.redo.clear();
    }

    fn finish(&mut self) -> bool {
        if !self.won && self.values == self.solution {
            self.won = true;
            return true;
        }
        false
    }

    /// Writes `d` into cell `i` (the same digit again clears it). With
    /// `clear_notes`, the digit's pencil marks leave the cell's peers.
    pub fn enter(&mut self, i: usize, d: u8, clear_notes: bool) -> Entry {
        if self.won || i >= CELLS || self.is_given(i) || !(1..=9).contains(&d) {
            return Entry::default();
        }
        if self.values[i] == d {
            return Entry { changed: self.erase(i), ..Entry::default() };
        }
        let cleared = if clear_notes { PEERS[i].iter().filter(|&&p| self.notes[p as usize] & bit(d) != 0).fold(0u128, |m, &p| m | 1u128 << p) } else { 0 };
        self.record(Move { cell: i as u8, before: (self.values[i], self.notes[i]), after: (d, 0), cleared });
        let mistake = d != self.solution[i];
        if mistake {
            self.mistakes = self.mistakes.saturating_add(1);
        }
        Entry { changed: true, mistake, won: self.finish() }
    }

    /// Turns pencil mark `d` on or off in cell `i` (only in an empty cell).
    pub fn toggle_note(&mut self, i: usize, d: u8) -> bool {
        if self.won || i >= CELLS || self.is_given(i) || self.values[i] != 0 || !(1..=9).contains(&d) {
            return false;
        }
        let notes = self.notes[i] ^ bit(d);
        self.record(Move { cell: i as u8, before: (0, self.notes[i]), after: (0, notes), cleared: 0 });
        true
    }

    /// Clears the digit or the pencil marks of cell `i`.
    pub fn erase(&mut self, i: usize) -> bool {
        if self.won || i >= CELLS || self.is_given(i) || (self.values[i] == 0 && self.notes[i] == 0) {
            return false;
        }
        self.record(Move { cell: i as u8, before: (self.values[i], self.notes[i]), after: (0, 0), cleared: 0 });
        true
    }

    /// Fills one cell with its solution: `prefer` when it is empty or wrong,
    /// else the next logical single, else the first open cell. Returns the
    /// cell; counts a hint.
    pub fn hint(&mut self, prefer: Option<usize>, clear_notes: bool) -> Option<(usize, Entry)> {
        if self.won {
            return None;
        }
        let open = |g: &Self, i: usize| !g.is_given(i) && g.values[i] != g.solution[i];
        let cell = prefer.filter(|&i| i < CELLS && open(self, i)).or_else(|| {
            // Reason from the correct entries only.
            let correct: Cells = std::array::from_fn(|i| if self.values[i] == self.solution[i] { self.values[i] } else { 0 });
            grid::next_single(&correct).map(|(i, _)| i).filter(|&i| open(self, i))
        }).or_else(|| (0..CELLS).find(|&i| open(self, i)))?;
        let d = self.solution[cell];
        let before = (self.values[cell], self.notes[cell]);
        let cleared = if clear_notes { PEERS[cell].iter().filter(|&&p| self.notes[p as usize] & bit(d) != 0).fold(0u128, |m, &p| m | 1u128 << p) } else { 0 };
        self.record(Move { cell: cell as u8, before, after: (d, 0), cleared });
        self.hints = self.hints.saturating_add(1);
        Some((cell, Entry { changed: true, mistake: false, won: self.finish() }))
    }

    /// Takes the last move back; the cell it touched.
    pub fn undo(&mut self) -> Option<usize> {
        if self.won {
            return None;
        }
        let m = self.undo.pop_back()?;
        self.apply(&m, false);
        self.redo.push(m);
        Some(m.cell as usize)
    }

    pub fn redo(&mut self) -> Option<usize> {
        if self.won {
            return None;
        }
        let m = self.redo.pop()?;
        self.apply(&m, true);
        self.undo.push_back(m);
        let cell = m.cell as usize;
        self.finish();
        Some(cell)
    }

    /// Every peer of `i` (row, column and box), for highlighting.
    pub fn peers_of(i: usize) -> u128 {
        PEERS[i].iter().fold(0u128, |m, &p| m | 1u128 << p)
    }

    /// Checks a loaded game for consistency: the givens and entries fit the
    /// solution, the solution is a valid grid and the only one.
    /// A move this board could have made: on an open cell, with digits and
    /// pencil marks in range, clearing marks only from open cells (undo and
    /// redo would otherwise write over a given).
    fn move_fits(&self, m: &Move) -> bool {
        let i = m.cell as usize;
        i < CELLS
            && !self.is_given(i)
            && m.after.0 <= 9
            && m.before.0 <= 9
            && (m.before.1 | m.after.1) & !grid::ALL == 0
            && m.cleared & self.givens == 0
    }

    pub fn valid(&self) -> bool {
        let puzzle = self.puzzle();
        self.solution.iter().all(|&d| (1..=9).contains(&d))
            && (0..CELLS).all(|i| !self.is_given(i) || self.values[i] == self.solution[i])
            && self.values.iter().all(|&d| d <= 9)
            && self.notes.iter().all(|&n| n & !grid::ALL == 0)
            && grid::unique_solution(&puzzle) == Some(self.solution)
            && self.undo.len() <= MAX_UNDO
            && self.redo.len() <= MAX_UNDO
            && self.undo.iter().chain(&self.redo).all(|m| self.move_fits(m))
    }
}

/// Per-difficulty statistics.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Stats {
    pub played: u32,
    pub won: u32,
    /// Fastest solve, ms (0: none yet).
    pub best_ms: u64,
    /// All solves together, ms (the average is this over `won`).
    pub total_ms: u64,
    /// Solves without a mistake or a hint.
    pub perfect: u32,
}

impl Stats {
    pub fn record_win(&mut self, ms: u64, perfect: bool) {
        self.won = self.won.saturating_add(1);
        self.total_ms = self.total_ms.saturating_add(ms);
        if self.best_ms == 0 || ms < self.best_ms {
            self.best_ms = ms;
        }
        if perfect {
            self.perfect = self.perfect.saturating_add(1);
        }
    }

    pub fn average_ms(&self) -> Option<u64> {
        (self.won > 0).then(|| self.total_ms / u64::from(self.won))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> Game {
        Game::new(Difficulty::Easy, 9, &grid::generate(Difficulty::Easy, 9))
    }

    fn open_cell(g: &Game) -> usize {
        (0..CELLS).find(|&i| !g.is_given(i)).unwrap()
    }

    #[test]
    fn givens_are_fixed_and_entries_count_mistakes() {
        let mut g = game();
        let given = (0..CELLS).find(|&i| g.is_given(i)).unwrap();
        assert_eq!(g.enter(given, 1, true), Entry::default());
        let i = open_cell(&g);
        let right = g.solution[i];
        let wrong = (1..=9).find(|&d| d != right).unwrap();
        let e = g.enter(i, wrong, true);
        assert!(e.changed && e.mistake);
        assert_eq!(g.mistakes, 1);
        assert!(g.wrong(i));
        let e = g.enter(i, right, true);
        assert!(e.changed && !e.mistake);
        assert_eq!(g.mistakes, 1);
        // The same digit again clears the cell.
        assert!(g.enter(i, right, true).changed);
        assert_eq!(g.values[i], 0);
    }

    #[test]
    fn notes_toggle_and_clear_from_peers_on_entry() {
        let mut g = game();
        let i = open_cell(&g);
        let d = g.solution[i];
        let peer = PEERS[i].iter().map(|&p| p as usize).find(|&p| !g.is_given(p) && g.values[p] == 0).unwrap();
        assert!(g.toggle_note(peer, d));
        assert!(g.toggle_note(peer, (d % 9) + 1));
        assert_eq!(g.notes[peer], bit(d) | bit((d % 9) + 1));
        g.enter(i, d, true);
        assert_eq!(g.notes[peer], bit((d % 9) + 1), "the placed digit left the peer's notes");
        // Undo puts the note back with the digit.
        assert_eq!(g.undo(), Some(i));
        assert_eq!(g.values[i], 0);
        assert_eq!(g.notes[peer], bit(d) | bit((d % 9) + 1));
        assert_eq!(g.redo(), Some(i));
        assert_eq!(g.notes[peer], bit((d % 9) + 1));
        // Notes only go in empty cells.
        assert!(!g.toggle_note(i, 3));
    }

    #[test]
    fn undo_and_redo_walk_the_history_and_a_new_move_drops_redo() {
        let mut g = game();
        let start = g.clone();
        let i = open_cell(&g);
        g.enter(i, 1, false);
        g.enter(i, 2, false);
        g.toggle_note((i + 1..CELLS).find(|&j| !g.is_given(j)).unwrap(), 5);
        while g.undo().is_some() {}
        assert_eq!(g.values, start.values);
        assert_eq!(g.notes, start.notes);
        assert_eq!(g.redo.len(), 3);
        g.redo();
        assert_eq!(g.values[i], 1);
        g.enter(i, 3, false);
        assert!(g.redo.is_empty(), "a new move forgets what was undone");
    }

    #[test]
    fn the_history_is_bounded() {
        let mut g = game();
        let i = open_cell(&g);
        for k in 0..(MAX_UNDO * 2) {
            g.toggle_note(i, (k % 9) as u8 + 1);
        }
        assert_eq!(g.undo.len(), MAX_UNDO);
    }

    #[test]
    fn hints_fill_the_solution_count_and_finish_the_game() {
        let mut g = game();
        let i = open_cell(&g);
        let (cell, _) = g.hint(Some(i), true).unwrap();
        assert_eq!(cell, i);
        assert_eq!(g.values[i], g.solution[i]);
        assert_eq!(g.hints, 1);
        // Hints alone solve it.
        let mut last = None;
        while let Some((_, e)) = g.hint(None, true) {
            last = Some(e);
        }
        assert!(g.won && last.is_some_and(|e| e.won));
        assert_eq!(g.values, g.solution);
        // Nothing changes after the win.
        assert!(!g.enter(i, 1, true).changed);
        assert!(g.undo().is_none());
    }

    #[test]
    fn conflicts_are_peer_clashes() {
        let mut g = game();
        let i = open_cell(&g);
        let clash = PEERS[i].iter().map(|&p| g.values[p as usize]).find(|&d| d != 0).unwrap();
        g.enter(i, clash, true);
        assert!(g.conflicts(i));
        assert!(g.valid());
    }

    #[test]
    fn stats_track_best_and_average() {
        let mut s = Stats::default();
        s.record_win(90_000, false);
        s.record_win(60_000, true);
        assert_eq!((s.won, s.best_ms, s.perfect), (2, 60_000, 1));
        assert_eq!(s.average_ms(), Some(75_000));
    }
}
