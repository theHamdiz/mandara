//! The Sudoku engine: a bitmask solver that counts solutions (the
//! uniqueness check), a logical solver that grades a puzzle by the hardest
//! technique it needs, and a seeded generator that removes givens from a
//! random full grid while the solution stays unique.
//!
//! Cells are `u8` (0 = empty, 1–9), candidates `u16` masks (bit `d - 1`
//! for digit `d`), everything in fixed arrays: no allocation in the solver.

use super::super::rng::Rng;

pub const CELLS: usize = 81;
pub const SIDE: usize = 9;
/// Every digit's bit.
pub const ALL: u16 = 0x1FF;

pub type Cells = [u8; CELLS];

/// Row, column and box of each cell.
pub const fn row(i: usize) -> usize {
    i / SIDE
}
pub const fn col(i: usize) -> usize {
    i % SIDE
}
pub const fn boxi(i: usize) -> usize {
    (i / 27) * 3 + (i % SIDE) / 3
}

pub const fn bit(d: u8) -> u16 {
    1 << (d - 1)
}

/// The 27 units (9 rows, 9 columns, 9 boxes), each nine cell indices.
pub const UNITS: [[u8; SIDE]; 27] = {
    let mut u = [[0u8; SIDE]; 27];
    let mut i = 0;
    while i < SIDE {
        let mut j = 0;
        while j < SIDE {
            u[i][j] = (i * SIDE + j) as u8;
            u[SIDE + i][j] = (j * SIDE + i) as u8;
            u[2 * SIDE + i][j] = ((i / 3) * 27 + (i % 3) * 3 + (j / 3) * SIDE + j % 3) as u8;
            j += 1;
        }
        i += 1;
    }
    u
};

/// The 20 peers of every cell (same row, column or box).
pub const PEERS: [[u8; 20]; CELLS] = {
    let mut p = [[0u8; 20]; CELLS];
    let mut i = 0;
    while i < CELLS {
        let mut n = 0;
        let mut j = 0;
        while j < CELLS {
            if j != i && (row(i) == row(j) || col(i) == col(j) || boxi(i) == boxi(j)) {
                p[i][n] = j as u8;
                n += 1;
            }
            j += 1;
        }
        i += 1;
    }
    p
};

/// Whether `a` and `b` share a unit (and are different cells).
pub fn peers(a: usize, b: usize) -> bool {
    a != b && (row(a) == row(b) || col(a) == col(b) || boxi(a) == boxi(b))
}

/// Digits already placed in each row, column and box.
#[derive(Clone, Copy)]
struct Used {
    rows: [u16; SIDE],
    cols: [u16; SIDE],
    boxes: [u16; SIDE],
}

impl Used {
    /// `None` when two givens clash.
    fn of(cells: &Cells) -> Option<Self> {
        let mut u = Self { rows: [0; SIDE], cols: [0; SIDE], boxes: [0; SIDE] };
        for (i, &d) in cells.iter().enumerate() {
            if d == 0 {
                continue;
            }
            if d > 9 || u.free(i) & bit(d) == 0 {
                return None;
            }
            u.set(i, d);
        }
        Some(u)
    }

    fn free(&self, i: usize) -> u16 {
        !(self.rows[row(i)] | self.cols[col(i)] | self.boxes[boxi(i)]) & ALL
    }

    fn set(&mut self, i: usize, d: u8) {
        let b = bit(d);
        self.rows[row(i)] |= b;
        self.cols[col(i)] |= b;
        self.boxes[boxi(i)] |= b;
    }

    fn clear(&mut self, i: usize, d: u8) {
        let b = !bit(d);
        self.rows[row(i)] &= b;
        self.cols[col(i)] &= b;
        self.boxes[boxi(i)] &= b;
    }
}

/// The empty cell with the fewest candidates, and those candidates
/// (`None` when the grid is full).
fn most_constrained(cells: &Cells, used: &Used) -> Option<(usize, u16)> {
    let mut best: Option<(usize, u16)> = None;
    for (i, &d) in cells.iter().enumerate() {
        if d != 0 {
            continue;
        }
        let free = used.free(i);
        let n = free.count_ones();
        if best.is_none_or(|(_, b)| n < b.count_ones()) {
            best = Some((i, free));
            if n <= 1 {
                break;
            }
        }
    }
    best
}

fn search(cells: &mut Cells, used: &mut Used, limit: u32, found: &mut u32, first: &mut Option<Cells>) {
    let Some((i, mut free)) = most_constrained(cells, used) else {
        *found += 1;
        if first.is_none() {
            *first = Some(*cells);
        }
        return;
    };
    while free != 0 && *found < limit {
        let d = free.trailing_zeros() as u8 + 1;
        free &= free - 1;
        cells[i] = d;
        used.set(i, d);
        search(cells, used, limit, found, first);
        used.clear(i, d);
        cells[i] = 0;
    }
}

/// How many solutions `cells` has, counting no further than `limit`.
pub fn count_solutions(cells: &Cells, limit: u32) -> u32 {
    let Some(mut used) = Used::of(cells) else { return 0 };
    let (mut grid, mut found, mut first) = (*cells, 0, None);
    search(&mut grid, &mut used, limit, &mut found, &mut first);
    found
}

/// The solution, when there is exactly one.
pub fn unique_solution(cells: &Cells) -> Option<Cells> {
    let mut used = Used::of(cells)?;
    let (mut grid, mut found, mut first) = (*cells, 0, None);
    search(&mut grid, &mut used, 2, &mut found, &mut first);
    if found == 1 { first } else { None }
}

/// A random complete grid.
pub fn random_full(rng: &mut Rng) -> Cells {
    fn fill(cells: &mut Cells, used: &mut Used, rng: &mut Rng) -> bool {
        let Some((i, free)) = most_constrained(cells, used) else { return true };
        let mut digits = [0u8; SIDE];
        let mut n = 0;
        for d in 1..=9u8 {
            if free & bit(d) != 0 {
                digits[n] = d;
                n += 1;
            }
        }
        rng.shuffle(&mut digits[..n]);
        for &d in &digits[..n] {
            cells[i] = d;
            used.set(i, d);
            if fill(cells, used, rng) {
                return true;
            }
            used.clear(i, d);
            cells[i] = 0;
        }
        false
    }
    let mut cells = [0u8; CELLS];
    let mut used = Used { rows: [0; SIDE], cols: [0; SIDE], boxes: [0; SIDE] };
    fill(&mut cells, &mut used, rng);
    cells
}

// ── Grading ──────────────────────────────────────────────────────────────

/// The techniques the logical solver knows, easiest first.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Technique {
    /// A cell with one candidate left.
    NakedSingle,
    /// A digit with one place left in a row, column or box.
    HiddenSingle,
    /// A box's candidates for a digit lie in one line (or a line's in one
    /// box): pointing and claiming.
    Locked,
    /// Naked and hidden pairs and triples.
    Subset,
    /// None of the above helps: the puzzle needs trial and error.
    Guess,
}

/// Candidates of every cell (0 for placed cells).
#[derive(Clone)]
pub struct Candidates {
    pub cells: Cells,
    pub cand: [u16; CELLS],
}

impl Candidates {
    pub fn of(cells: &Cells) -> Option<Self> {
        let used = Used::of(cells)?;
        let mut cand = [0u16; CELLS];
        for i in 0..CELLS {
            if cells[i] == 0 {
                cand[i] = used.free(i);
            }
        }
        Some(Self { cells: *cells, cand })
    }

    fn place(&mut self, i: usize, d: u8) {
        self.cells[i] = d;
        self.cand[i] = 0;
        for &p in &PEERS[i] {
            self.cand[p as usize] &= !bit(d);
        }
    }

    fn solved(&self) -> bool {
        self.cells.iter().all(|&d| d != 0)
    }

    /// An empty cell with no candidate: a contradiction.
    fn broken(&self) -> bool {
        (0..CELLS).any(|i| self.cells[i] == 0 && self.cand[i] == 0)
    }

    /// A naked single: `(cell, digit)`.
    pub fn naked_single(&self) -> Option<(usize, u8)> {
        (0..CELLS).find(|&i| self.cells[i] == 0 && self.cand[i].count_ones() == 1).map(|i| (i, self.cand[i].trailing_zeros() as u8 + 1))
    }

    /// A hidden single: `(cell, digit)`.
    pub fn hidden_single(&self) -> Option<(usize, u8)> {
        for unit in &UNITS {
            for d in 1..=9u8 {
                let mut at = None;
                let mut n = 0;
                for &c in unit {
                    if self.cand[c as usize] & bit(d) != 0 {
                        at = Some(c as usize);
                        n += 1;
                    }
                }
                if n == 1 {
                    return at.map(|i| (i, d));
                }
            }
        }
        None
    }

    /// Pointing and claiming; whether anything was eliminated.
    fn locked(&mut self) -> bool {
        let mut changed = false;
        for d in 1..=9u8 {
            let b = bit(d);
            // Box → line (pointing), then line → box (claiming).
            for (a, others) in [(18..27usize, 0..18usize), (0..18, 18..27)] {
                for ua in a.clone() {
                    let cells: [u8; SIDE] = UNITS[ua];
                    let mask = in_unit(&self.cand, &cells, b);
                    if mask.count_ones() < 2 {
                        continue;
                    }
                    for ub in others.clone() {
                        let other = UNITS[ub];
                        // Every candidate of `ua` lies in `ub` too…
                        if cells.iter().filter(|&&c| self.cand[c as usize] & b != 0).all(|c| other.contains(c)) {
                            // …so `ub`'s other cells can't hold it.
                            for &c in &other {
                                if !cells.contains(&c) && self.cand[c as usize] & b != 0 {
                                    self.cand[c as usize] &= !b;
                                    changed = true;
                                }
                            }
                        }
                    }
                }
            }
        }
        changed
    }

    /// Naked and hidden pairs and triples; whether anything was eliminated.
    fn subsets(&mut self) -> bool {
        let mut changed = false;
        for unit in &UNITS {
            let open: [usize; SIDE] = unit.map(|c| c as usize);
            let empty: u16 = (0..SIDE).fold(0, |m, k| if self.cells[open[k]] == 0 { m | 1 << k } else { m });
            // Every 2- or 3-cell subset of the unit's empty cells.
            let mut pick = empty;
            while pick != 0 {
                let set = pick;
                pick = (pick - 1) & empty;
                let size = set.count_ones();
                if !(2..=3).contains(&size) {
                    continue;
                }
                // Naked: `size` cells whose candidates span `size` digits.
                let union = (0..SIDE).filter(|k| set & 1 << k != 0).fold(0, |m, k| m | self.cand[open[k]]);
                if union.count_ones() == size {
                    for k in (0..SIDE).filter(|k| empty & 1 << k != 0 && set & 1 << k == 0) {
                        if self.cand[open[k]] & union != 0 {
                            self.cand[open[k]] &= !union;
                            changed = true;
                        }
                    }
                }
                // Hidden: `size` digits that fit only in these cells.
                let outside = (0..SIDE).filter(|k| empty & 1 << k != 0 && set & 1 << k == 0).fold(0, |m, k| m | self.cand[open[k]]);
                let only = union & !outside;
                if only.count_ones() == size {
                    for k in (0..SIDE).filter(|k| set & 1 << k != 0) {
                        if self.cand[open[k]] & !only != 0 {
                            self.cand[open[k]] &= only;
                            changed = true;
                        }
                    }
                }
            }
        }
        changed
    }
}

fn in_unit(cand: &[u16; CELLS], cells: &[u8; SIDE], b: u16) -> u16 {
    cells.iter().enumerate().fold(0, |m, (k, &c)| if cand[c as usize] & b != 0 { m | 1 << k } else { m })
}

/// The hardest technique the puzzle needs (`Guess` when the logical
/// solver gets stuck; also for an invalid grid).
pub fn grade(puzzle: &Cells) -> Technique {
    let Some(mut c) = Candidates::of(puzzle) else { return Technique::Guess };
    let mut hardest = Technique::NakedSingle;
    loop {
        if c.solved() {
            return hardest;
        }
        if c.broken() {
            return Technique::Guess;
        }
        if let Some((i, d)) = c.naked_single() {
            c.place(i, d);
            continue;
        }
        if let Some((i, d)) = c.hidden_single() {
            hardest = hardest.max(Technique::HiddenSingle);
            c.place(i, d);
            continue;
        }
        if c.locked() {
            hardest = hardest.max(Technique::Locked);
            continue;
        }
        if c.subsets() {
            hardest = hardest.max(Technique::Subset);
            continue;
        }
        return Technique::Guess;
    }
}

// ── Generation ───────────────────────────────────────────────────────────

/// The four difficulties.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Difficulty {
    #[default]
    Easy,
    Medium,
    Hard,
    Expert,
}

impl Difficulty {
    pub const ALL: [Difficulty; 4] = [Self::Easy, Self::Medium, Self::Hard, Self::Expert];

    pub fn id(self) -> &'static str {
        match self {
            Self::Easy => "easy",
            Self::Medium => "medium",
            Self::Hard => "hard",
            Self::Expert => "expert",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.id().eq_ignore_ascii_case(s.trim()))
    }

    pub fn index(self) -> usize {
        self as usize
    }

    /// Removal stops at this many givens.
    fn floor(self) -> usize {
        match self {
            Self::Easy => 38,
            Self::Medium => 31,
            Self::Hard | Self::Expert => 17,
        }
    }

    /// The hardest technique a removal may require on the way down.
    fn ceiling(self) -> Technique {
        match self {
            Self::Easy => Technique::HiddenSingle,
            Self::Medium => Technique::Locked,
            Self::Hard => Technique::Subset,
            Self::Expert => Technique::Guess,
        }
    }

    /// Whether a finished puzzle with `givens` graded `t` is this difficulty.
    fn accepts(self, t: Technique, givens: usize) -> bool {
        match self {
            Self::Easy => t <= Technique::HiddenSingle,
            Self::Medium => t <= Technique::Locked && givens <= 35,
            Self::Hard => t == Technique::Subset || (t == Technique::Locked && givens <= 26),
            Self::Expert => t == Technique::Guess,
        }
    }
}

/// A generated puzzle and its unique solution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Puzzle {
    pub givens: Cells,
    pub solution: Cells,
    /// What the logical solver found it needs.
    pub technique: Technique,
}

/// Fresh grids tried before taking the closest puzzle found: bounds the
/// generator's time (the cost of one try is a few hundred uniqueness checks).
pub const MAX_ATTEMPTS: u32 = 160;

/// A puzzle of `difficulty` from `seed`: always a unique solution, the
/// same puzzle for the same seed. Bounded by [`MAX_ATTEMPTS`]; past it the
/// try closest to the difficulty is used.
pub fn generate(difficulty: Difficulty, seed: u64) -> Puzzle {
    let mut rng = Rng::new(seed);
    let mut best: Option<(u32, Puzzle)> = None;
    for _ in 0..MAX_ATTEMPTS {
        let solution = random_full(&mut rng);
        let givens = carve(&solution, difficulty, &mut rng);
        let technique = grade(&givens);
        let count = givens.iter().filter(|&&d| d != 0).count();
        let puzzle = Puzzle { givens, solution, technique };
        if difficulty.accepts(technique, count) {
            return puzzle;
        }
        let distance = (technique as i32 - difficulty.target() as i32).unsigned_abs();
        if best.as_ref().is_none_or(|(d, _)| distance < *d) {
            best = Some((distance, puzzle));
        }
    }
    best.map(|(_, p)| p).unwrap_or_else(|| {
        // Unreachable (MAX_ATTEMPTS > 0); a full grid is still a valid game.
        let solution = random_full(&mut rng);
        Puzzle { givens: solution, solution, technique: Technique::NakedSingle }
    })
}

impl Difficulty {
    fn target(self) -> Technique {
        match self {
            Self::Easy => Technique::NakedSingle,
            Self::Medium => Technique::Locked,
            Self::Hard => Technique::Subset,
            Self::Expert => Technique::Guess,
        }
    }
}

/// Removes givens in symmetric pairs (random order) while the solution
/// stays unique and the logic stays under the difficulty's ceiling.
fn carve(solution: &Cells, difficulty: Difficulty, rng: &mut Rng) -> Cells {
    let mut order: [u8; 41] = std::array::from_fn(|i| i as u8);
    rng.shuffle(&mut order);
    let mut grid = *solution;
    let mut count = CELLS;
    for &i in &order {
        let (a, b) = (i as usize, CELLS - 1 - i as usize);
        let removing = if a == b { 1 } else { 2 };
        if count - removing < difficulty.floor() {
            continue;
        }
        let keep = (grid[a], grid[b]);
        grid[a] = 0;
        grid[b] = 0;
        let ok = count_solutions(&grid, 2) == 1 && (difficulty.ceiling() == Technique::Guess || grade(&grid) <= difficulty.ceiling());
        if ok {
            count -= removing;
        } else {
            (grid[a], grid[b]) = keep;
        }
    }
    grid
}

/// The next logical step from `values` (a single), for hints.
pub fn next_single(values: &Cells) -> Option<(usize, u8)> {
    let c = Candidates::of(values)?;
    c.naked_single().or_else(|| c.hidden_single())
}

/// Parses 81 digits (`0` or `.` for empty); `None` for anything else.
pub fn parse(s: &str) -> Option<Cells> {
    let mut cells = [0u8; CELLS];
    let mut n = 0;
    for ch in s.chars() {
        let d = match ch {
            '1'..='9' => ch as u8 - b'0',
            '0' | '.' => 0,
            _ => return None,
        };
        *cells.get_mut(n)? = d;
        n += 1;
    }
    (n == CELLS).then_some(cells)
}

/// 81 digits (`0` for empty).
pub fn format(cells: &Cells) -> String {
    cells.iter().map(|&d| (b'0' + d.min(9)) as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A well-known puzzle with one solution.
    const CLASSIC: &str = "530070000600195000098000060800060003400803001700020006060000280000419005000080079";
    const CLASSIC_SOLUTION: &str = "534678912672195348198342567859761423426853791713924856961537284287419635345286179";

    #[test]
    fn units_and_peers_are_right() {
        assert_eq!(UNITS[0], [0, 1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(UNITS[9], [0, 9, 18, 27, 36, 45, 54, 63, 72]);
        assert_eq!(UNITS[18 + 4], [30, 31, 32, 39, 40, 41, 48, 49, 50]);
        for (i, list) in PEERS.iter().enumerate() {
            assert_eq!(list.iter().filter(|&&p| p as usize != i).count(), 20);
            assert!(list.iter().all(|&p| peers(i, p as usize)));
        }
    }

    #[test]
    fn solver_finds_the_unique_solution() {
        let p = parse(CLASSIC).unwrap();
        assert_eq!(count_solutions(&p, 2), 1);
        assert_eq!(unique_solution(&p).map(|s| format(&s)), Some(CLASSIC_SOLUTION.to_string()));
    }

    #[test]
    fn solver_sees_several_solutions_and_none() {
        let mut p = parse(CLASSIC).unwrap();
        // Two givens fewer: more than one solution.
        p[0] = 0;
        p[1] = 0;
        p[4] = 0;
        p[9] = 0;
        p[13] = 0;
        assert!(count_solutions(&[0; CELLS], 2) == 2);
        assert!(unique_solution(&[0; CELLS]).is_none());
        // Clashing givens: no solution at all.
        let mut bad = parse(CLASSIC).unwrap();
        bad[2] = 5;
        assert_eq!(count_solutions(&bad, 2), 0);
        let _ = p;
    }

    #[test]
    fn full_grids_are_valid_and_seeded() {
        let a = random_full(&mut Rng::new(1));
        assert_eq!(a, random_full(&mut Rng::new(1)));
        assert_ne!(a, random_full(&mut Rng::new(2)));
        assert!(a.iter().all(|&d| (1..=9).contains(&d)));
        assert_eq!(count_solutions(&a, 2), 1);
    }

    #[test]
    fn the_classic_puzzle_needs_only_singles() {
        assert!(grade(&parse(CLASSIC).unwrap()) <= Technique::HiddenSingle);
    }

    #[test]
    fn a_puzzle_beyond_the_known_techniques_is_graded_guess() {
        // "Platinum Blonde", a famously hard puzzle.
        let hard = parse("000000012000000003002300400001800005060070800000009000008500000900040500470006000").unwrap();
        assert_eq!(count_solutions(&hard, 2), 1);
        assert_eq!(grade(&hard), Technique::Guess);
    }

    #[test]
    fn every_difficulty_generates_unique_deterministic_puzzles_of_its_level() {
        for d in Difficulty::ALL {
            for seed in [11u64, 2026] {
                let p = generate(d, seed);
                assert_eq!(p, generate(d, seed), "{d:?} {seed}: deterministic");
                assert_eq!(count_solutions(&p.givens, 2), 1, "{d:?} {seed}: unique");
                assert_eq!(unique_solution(&p.givens), Some(p.solution), "{d:?} {seed}");
                // Givens agree with the solution.
                assert!(p.givens.iter().zip(&p.solution).all(|(&g, &s)| g == 0 || g == s));
                let givens = p.givens.iter().filter(|&&g| g != 0).count();
                assert!(d.accepts(p.technique, givens), "{d:?} {seed}: graded {:?} with {givens} givens", p.technique);
                assert_eq!(grade(&p.givens), p.technique);
            }
        }
    }

    #[test]
    fn harder_levels_have_fewer_givens() {
        let givens = |d| generate(d, 5).givens.iter().filter(|&&g| g != 0).count();
        assert!(givens(Difficulty::Easy) > givens(Difficulty::Medium));
        assert!(givens(Difficulty::Medium) > givens(Difficulty::Hard));
    }

    #[test]
    fn hints_find_a_single() {
        let p = parse(CLASSIC).unwrap();
        let s = parse(CLASSIC_SOLUTION).unwrap();
        let (i, d) = next_single(&p).unwrap();
        assert_eq!(p[i], 0);
        assert_eq!(s[i], d);
    }

    #[test]
    fn parse_and_format_round_trip() {
        let p = parse(CLASSIC).unwrap();
        assert_eq!(format(&p), CLASSIC);
        assert!(parse("12").is_none());
        assert!(parse(&"x".repeat(81)).is_none());
        assert_eq!(parse(&".".repeat(81)), Some([0; CELLS]));
    }
}
