//! The shell's 2048: one game shared by the panel and the desktop widget,
//! its records per board size, its settings and saving. Views animate the
//! last move ([`Tiles::last`]) and snap to the board when they missed some.

use std::path::PathBuf;

use gpui::{App, AppContext as _, Context, Entity};

use super::super::Accent;
use super::super::save_io::{self, Persist, Read, Saver};
use super::board::{self, DEFAULT_SIDE, Dir, Game, Stats, Step, size_index};
use super::save::File;

/// Plugin id (`[plugins.settings.2048]`).
pub const ID: &str = "2048";
const FILE: &str = "2048.json";

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Board side for new games (3–6).
    pub side: u8,
    /// Tiles slide and pop (off, or reduced motion: they jump).
    pub animations: bool,
    /// One step of undo.
    pub undo: bool,
    pub accent: Accent,
    /// The bar item shows the score while a game is on.
    pub bar_score: bool,
}

impl Settings {
    pub fn of(cfg: &mandara_config::ShellConfig) -> Self {
        let s = &cfg.plugins;
        Self {
            side: board::clamp_side(s.setting(ID, "size", u32::from(DEFAULT_SIDE)).min(u32::from(board::MAX_SIDE)) as u8),
            animations: s.setting(ID, "animations", true),
            undo: s.setting(ID, "undo", true),
            accent: Accent::parse(&s.setting::<String>(ID, "accent", String::new())),
            bar_score: s.setting(ID, "bar_score", false),
        }
    }
}

pub struct Tiles {
    pub loaded: bool,
    pub game: Option<Game>,
    pub stats: [Stats; 4],
    /// The last move, numbered: views animate a move they haven't seen.
    pub last: Option<(u64, Step)>,
    /// Bumped on every change that isn't a plain move (new game, undo,
    /// load): views snap to the board.
    pub epoch: u64,
    seq: u64,
    /// This game beat the size's best score.
    pub new_best: bool,
    pub settings: Settings,
    saver: Saver,
}

impl Persist for Tiles {
    fn saver(&mut self) -> &mut Saver {
        &mut self.saver
    }

    fn snapshot(&self) -> Vec<u8> {
        File::new(self.game.as_ref(), &self.stats).to_bytes()
    }

    fn file(&self) -> PathBuf {
        save_io::path(FILE)
    }
}

impl Tiles {
    pub fn create(cx: &mut App) -> Entity<Self> {
        let settings = Settings::of(crate::state::config(cx).read(cx).get());
        let entity = cx.new(|cx| {
            cx.observe(&crate::state::config(cx), |m: &mut Self, store, cx| {
                let next = Settings::of(store.read(cx).get());
                if next != m.settings {
                    m.settings = next;
                    // A fresh board follows a new size at once; a game in
                    // play keeps its size until the next one.
                    if m.game.as_ref().is_some_and(|g| g.moves == 0 && g.side != next.side) {
                        m.new_game(None, cx);
                    }
                    cx.notify();
                }
            })
            .detach();
            Self { loaded: false, game: None, stats: [Stats::default(); 4], last: None, epoch: 0, seq: 0, new_best: false, settings, saver: Saver::default() }
        });
        let weak = entity.downgrade();
        cx.spawn(async move |cx| {
            let file = save_io::path(FILE);
            let parsed = mandara_services::runtime::unblock(move || match save_io::read(&file) {
                Read::Missing => None,
                Read::Bytes(bytes) => {
                    let parsed = File::parse(&bytes);
                    if parsed.is_none() {
                        tracing::warn!("2048: {} isn't a save Mandara can read; starting fresh", file.display());
                        save_io::quarantine(&file);
                    }
                    parsed
                }
                Read::Unreadable(e) => {
                    tracing::warn!("2048: {}: {e}", file.display());
                    None
                }
            })
            .await;
            let _ = weak.update(cx, |m, cx| m.loaded(parsed, cx));
        })
        .detach();
        entity
    }

    fn loaded(&mut self, parsed: Option<(Option<Game>, [Stats; 4])>, cx: &mut Context<Self>) {
        self.loaded = true;
        if let Some((game, stats)) = parsed {
            self.stats = stats;
            self.game = game;
        }
        if self.game.is_none() {
            self.new_game(None, cx);
        }
        self.epoch += 1;
        cx.notify();
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        save_io::flush_in_background(self, cx);
    }

    /// A new game (`side`: that size, else the setting's).
    pub fn new_game(&mut self, side: Option<u8>, cx: &mut Context<Self>) {
        let side = board::clamp_side(side.unwrap_or(self.settings.side));
        let game = Game::new(side, super::super::rng::fresh_seed());
        let s = &mut self.stats[size_index(side)];
        s.played = s.played.saturating_add(1);
        self.game = Some(game);
        self.new_best = false;
        self.last = None;
        self.epoch += 1;
        save_io::changed(self, cx);
        cx.notify();
    }

    /// Slides the tiles; whether anything moved.
    pub fn slide(&mut self, dir: Dir, cx: &mut Context<Self>) -> bool {
        let Some(game) = self.game.as_mut() else { return false };
        let was_won = game.won;
        let Some(step) = game.slide(dir) else { return false };
        let stats = &mut self.stats[size_index(game.side)];
        if stats.observe(game) {
            self.new_best = true;
        }
        if game.won && !was_won {
            stats.won = stats.won.saturating_add(1);
        }
        self.seq += 1;
        self.last = Some((self.seq, step));
        save_io::changed(self, cx);
        cx.notify();
        true
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.settings.undo || !self.game.as_mut().is_some_and(Game::take_back) {
            return false;
        }
        self.last = None;
        self.epoch += 1;
        save_io::changed(self, cx);
        cx.notify();
        true
    }

    /// Plays on past 2048.
    pub fn keep_going(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(g) = self.game.as_mut().filter(|g| g.showing_win()) else { return false };
        g.keep_going = true;
        save_io::changed(self, cx);
        cx.notify();
        true
    }

    pub fn best(&self) -> u64 {
        self.game.as_ref().map_or(0, |g| self.stats[size_index(g.side)].best)
    }

    pub fn status(&self) -> serde_json::Value {
        match &self.game {
            None => serde_json::json!({ "loaded": self.loaded }),
            Some(g) => {
                let n = usize::from(g.side);
                let rows: Vec<Vec<u64>> = (0..n).map(|r| (0..n).map(|c| board::value(g.cells[r * n + c])).collect()).collect();
                serde_json::json!({
                    "size": g.side,
                    "score": g.score,
                    "best": self.best(),
                    "moves": g.moves,
                    "largest": board::value(g.largest()),
                    "won": g.won,
                    "keep_going": g.keep_going,
                    "over": g.over(),
                    "can_undo": self.settings.undo && g.undo.is_some(),
                    "board": rows,
                    "stats": self.stats[size_index(g.side)],
                })
            }
        }
    }
}
