//! 2048's save file, `~/.local/state/mandara/games/2048.json`: the board in
//! progress (one hex pair per cell), score, the generator's state (so the
//! next tile is the one it would have been), the undo step and the records
//! for every board size. Versioned; anything that doesn't check out is
//! ignored and the records kept when they parse.

use serde::{Deserialize, Serialize};

use super::super::rng::Rng;
use super::board::{Game, MAX_CELLS, MAX_EXPONENT, Snapshot, Stats};

pub const VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct File {
    pub v: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game: Option<GameFile>,
    /// Sizes 3, 4, 5 and 6.
    #[serde(default)]
    pub stats: [Stats; 4],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GameFile {
    pub side: u8,
    pub cells: String,
    pub score: u64,
    pub moves: u32,
    pub rng: u64,
    pub won: bool,
    pub keep_going: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo: Option<UndoFile>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UndoFile {
    pub cells: String,
    pub score: u64,
    pub moves: u32,
    pub rng: u64,
}

fn cells_hex(cells: &[u8; MAX_CELLS], n: usize) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(n * 2);
    for &c in &cells[..n] {
        let _ = write!(s, "{c:02x}");
    }
    s
}

fn parse_cells(s: &str, n: usize) -> Option<[u8; MAX_CELLS]> {
    if s.len() != n * 2 || !s.is_ascii() {
        return None;
    }
    let mut cells = [0u8; MAX_CELLS];
    for (i, c) in cells.iter_mut().take(n).enumerate() {
        *c = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok().filter(|&e| e <= MAX_EXPONENT)?;
    }
    Some(cells)
}

impl GameFile {
    pub fn of(g: &Game) -> Self {
        let n = g.len();
        Self {
            side: g.side,
            cells: cells_hex(&g.cells, n),
            score: g.score,
            moves: g.moves,
            rng: g.rng.0,
            won: g.won,
            keep_going: g.keep_going,
            undo: g.undo.map(|u| UndoFile { cells: cells_hex(&u.cells, n), score: u.score, moves: u.moves, rng: u.rng }),
        }
    }

    pub fn game(&self) -> Option<Game> {
        let mut g = Game::empty(self.side, self.rng);
        if g.side != self.side {
            return None;
        }
        let n = g.len();
        g.cells = parse_cells(&self.cells, n)?;
        g.score = self.score;
        g.moves = self.moves;
        g.rng = Rng(self.rng);
        g.won = self.won;
        g.keep_going = self.keep_going;
        g.undo = match &self.undo {
            Some(u) => Some(Snapshot { cells: parse_cells(&u.cells, n)?, score: u.score, rng: u.rng, moves: u.moves }),
            None => None,
        };
        g.assign_ids();
        g.valid().then_some(g)
    }
}

impl File {
    pub fn new(game: Option<&Game>, stats: &[Stats; 4]) -> Self {
        Self { v: VERSION, game: game.map(GameFile::of), stats: *stats }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

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
    use super::super::board::{Dir, size_index};
    use super::*;

    #[test]
    fn a_game_round_trips_with_its_undo_and_generator() {
        let mut g = Game::new(5, 42);
        for d in [Dir::Left, Dir::Up, Dir::Right, Dir::Down, Dir::Left] {
            g.slide(d);
        }
        let mut stats = [Stats::default(); 4];
        stats[size_index(5)].observe(&g);
        let bytes = File::new(Some(&g), &stats).to_bytes();
        assert!(bytes.len() < 512, "{} bytes", bytes.len());
        let (back, s) = File::parse(&bytes).unwrap();
        let mut back = back.unwrap();
        assert_eq!(s, stats);
        assert_eq!((back.cells, back.score, back.moves, back.rng, back.undo), (g.cells, g.score, g.moves, g.rng, g.undo));
        // The next move deals the same tile it would have.
        let dir = Dir::ALL.into_iter().find(|&d| g.can_move(d)).unwrap();
        assert_eq!(g.slide(dir).and_then(|s| s.spawned).map(|(_, c)| c), back.slide(dir).and_then(|s| s.spawned).map(|(_, c)| c));
    }

    #[test]
    fn corrupted_files_are_rejected_safely() {
        assert!(File::parse(b"\x00\x01garbage").is_none());
        assert!(File::parse(br#"{"v":2}"#).is_none());
        let g = Game::new(4, 1);
        let stats = [Stats { best: 77, ..Stats::default() }; 4];
        let good = File::new(Some(&g), &stats);
        let tampers: [fn(&mut GameFile); 5] = [
            |f| f.cells = "zz".repeat(16),
            |f| f.cells.truncate(4),
            |f| f.side = 9,
            |f| f.cells = "00".repeat(16),
            |f| f.cells = "ff".repeat(16),
        ];
        for tamper in tampers {
            let mut bad = good.clone();
            tamper(bad.game.as_mut().unwrap());
            let (game, s) = File::parse(&bad.to_bytes()).unwrap();
            assert!(game.is_none(), "{bad:?}");
            assert_eq!(s, stats);
        }
    }
}
