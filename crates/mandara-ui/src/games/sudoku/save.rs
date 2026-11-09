//! Sudoku's save file, `~/.local/state/mandara/games/sudoku.json`: the game
//! in progress (board, pencil marks, clock, counters and the undo / redo
//! history) and the statistics, versioned and compact (digits as strings,
//! marks and moves as hex). A file that doesn't parse, or a game that
//! doesn't check out, is never trusted: the game starts fresh and the
//! statistics survive when they can.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use super::grid::{self, CELLS, Difficulty};
use super::play::{Game, MAX_UNDO, Move, Stats};

/// The format's version (a newer file from a future Mandara is ignored).
pub const VERSION: u32 = 1;
/// Hex digits of one pencil-mark mask (9 bits).
const NOTE_HEX: usize = 3;
/// Hex digits of one move: cell 2, before 1 + 3, after 1 + 3, cleared 21.
const MOVE_HEX: usize = 31;
const CLEARED_HEX: usize = 21;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct File {
    pub v: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game: Option<GameFile>,
    #[serde(default)]
    pub stats: [Stats; 4],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GameFile {
    pub difficulty: Difficulty,
    pub seed: u64,
    pub puzzle: String,
    pub solution: String,
    pub values: String,
    pub notes: String,
    pub mistakes: u32,
    pub hints: u32,
    pub ms: u64,
    pub won: bool,
    #[serde(default)]
    pub undo: String,
    #[serde(default)]
    pub redo: String,
}

fn hex(out: &mut String, v: u128, digits: usize) {
    use std::fmt::Write as _;
    let _ = write!(out, "{v:0digits$x}");
}

fn moves_hex<'a>(moves: impl Iterator<Item = &'a Move>) -> String {
    let mut s = String::new();
    for m in moves {
        hex(&mut s, u128::from(m.cell), 2);
        hex(&mut s, u128::from(m.before.0), 1);
        hex(&mut s, u128::from(m.before.1), NOTE_HEX);
        hex(&mut s, u128::from(m.after.0), 1);
        hex(&mut s, u128::from(m.after.1), NOTE_HEX);
        hex(&mut s, m.cleared, CLEARED_HEX);
    }
    s
}

fn parse_moves(s: &str) -> Option<Vec<Move>> {
    if !s.len().is_multiple_of(MOVE_HEX) || !s.is_ascii() {
        return None;
    }
    let num = |a: &str| u128::from_str_radix(a, 16).ok();
    s.as_bytes()
        .chunks(MOVE_HEX)
        .map(|c| {
            let c = std::str::from_utf8(c).ok()?;
            let cell = u8::try_from(num(&c[0..2])?).ok()?;
            let before = (u8::try_from(num(&c[2..3])?).ok()?, u16::try_from(num(&c[3..6])?).ok()?);
            let after = (u8::try_from(num(&c[6..7])?).ok()?, u16::try_from(num(&c[7..10])?).ok()?);
            let cleared = num(&c[10..10 + CLEARED_HEX])?;
            ((cell as usize) < CELLS && cleared >> CELLS == 0).then_some(Move { cell, before, after, cleared })
        })
        .collect()
}

impl GameFile {
    pub fn of(g: &Game, ms: u64) -> Self {
        let mut notes = String::with_capacity(CELLS * NOTE_HEX);
        for &n in &g.notes {
            hex(&mut notes, u128::from(n), NOTE_HEX);
        }
        Self {
            difficulty: g.difficulty,
            seed: g.seed,
            puzzle: grid::format(&g.puzzle()),
            solution: grid::format(&g.solution),
            values: grid::format(&g.values),
            notes,
            mistakes: g.mistakes,
            hints: g.hints,
            ms,
            won: g.won,
            undo: moves_hex(g.undo.iter()),
            redo: moves_hex(g.redo.iter()),
        }
    }

    /// The game, when every part parses and checks out.
    pub fn game(&self) -> Option<Game> {
        let puzzle = grid::parse(&self.puzzle)?;
        let solution = grid::parse(&self.solution)?;
        let values = grid::parse(&self.values)?;
        if self.notes.len() != CELLS * NOTE_HEX || !self.notes.is_ascii() {
            return None;
        }
        let mut notes = [0u16; CELLS];
        for (i, n) in notes.iter_mut().enumerate() {
            *n = u16::from_str_radix(&self.notes[i * NOTE_HEX..(i + 1) * NOTE_HEX], 16).ok()?;
        }
        let undo: VecDeque<Move> = parse_moves(&self.undo)?.into();
        let redo = parse_moves(&self.redo)?;
        if undo.len() > MAX_UNDO || redo.len() > MAX_UNDO {
            return None;
        }
        let givens = (0..CELLS).filter(|&i| puzzle[i] != 0).fold(0u128, |m, i| m | 1u128 << i);
        let g = Game {
            difficulty: self.difficulty,
            seed: self.seed,
            givens,
            solution,
            values,
            notes,
            mistakes: self.mistakes,
            hints: self.hints,
            elapsed_ms: self.ms,
            won: self.won,
            undo,
            redo,
        };
        (g.valid() && g.puzzle() == puzzle).then_some(g)
    }
}

impl File {
    pub fn new(game: Option<&Game>, ms: u64, stats: &[Stats; 4]) -> Self {
        Self { v: VERSION, game: game.map(|g| GameFile::of(g, ms)), stats: *stats }
    }

    /// Compact JSON.
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// The file's contents, or `None` when it can't be trusted at all (not
    /// JSON, or another version). A game that fails its checks is dropped
    /// and the statistics kept.
    pub fn parse(bytes: &[u8]) -> Option<(Option<Game>, [Stats; 4])> {
        let f: File = serde_json::from_slice(bytes).ok()?;
        if f.v != VERSION {
            return None;
        }
        Some((f.game.as_ref().and_then(GameFile::game), f.stats))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn played() -> Game {
        let mut g = Game::new(Difficulty::Medium, 77, &grid::generate(Difficulty::Medium, 77));
        let open: Vec<usize> = (0..CELLS).filter(|&i| !g.is_given(i)).collect();
        g.toggle_note(open[0], 4);
        g.toggle_note(open[0], 7);
        g.enter(open[1], g.solution[open[1]], true);
        g.enter(open[2], (g.solution[open[2]] % 9) + 1, true);
        g.hint(Some(open[3]), true);
        g.undo();
        g
    }

    #[test]
    fn a_game_round_trips_exactly() {
        let g = played();
        let mut stats = [Stats::default(); 4];
        stats[1].record_win(123_456, true);
        stats[1].played = 3;
        let bytes = File::new(Some(&g), 98_765, &stats).to_bytes();
        let (back, s) = File::parse(&bytes).unwrap();
        let back = back.unwrap();
        assert_eq!(s, stats);
        assert_eq!(back.elapsed_ms, 98_765);
        assert_eq!(Game { elapsed_ms: g.elapsed_ms, ..back }, g);
        // Compact: a whole game with history stays a few kilobytes.
        assert!(bytes.len() < 2048, "{} bytes", bytes.len());
    }

    #[test]
    fn corrupted_files_are_rejected_safely() {
        assert!(File::parse(b"").is_none());
        assert!(File::parse(b"{not json").is_none());
        assert!(File::parse(&[0xFF, 0x00, 0x13]).is_none());
        // A future version isn't guessed at.
        assert!(File::parse(br#"{"v":99,"stats":[]}"#).is_none());
        let g = played();
        let stats = [Stats { played: 5, ..Stats::default() }; 4];
        let good = File::new(Some(&g), 1, &stats);
        // A tampered board drops the game but keeps the statistics.
        let tampers: [fn(&mut GameFile); 6] = [
            |f: &mut GameFile| f.values = "1".repeat(81),
            |f: &mut GameFile| f.solution.replace_range(0..1, "0"),
            |f: &mut GameFile| f.notes.truncate(10),
            |f: &mut GameFile| f.undo.push('z'),
            |f: &mut GameFile| f.puzzle = "0".repeat(81),
            |f: &mut GameFile| f.redo = "f".repeat(31),
        ];
        for tamper in tampers {
            let mut bad = good.clone();
            tamper(bad.game.as_mut().unwrap());
            let (game, s) = File::parse(&bad.to_bytes()).unwrap();
            assert!(game.is_none());
            assert_eq!(s, stats);
        }
    }

    #[test]
    fn a_file_without_a_game_still_has_its_statistics() {
        let stats = [Stats { won: 2, best_ms: 5, total_ms: 9, played: 4, perfect: 1 }; 4];
        let (game, s) = File::parse(&File::new(None, 0, &stats).to_bytes()).unwrap();
        assert!(game.is_none());
        assert_eq!(s, stats);
    }
}
