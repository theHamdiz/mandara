//! The shell's Sudoku: one game shared by the panel and the desktop widget,
//! its settings (resolved once per config change), the play clock and
//! saving. Puzzles are generated on the blocking pool; a newer request
//! makes an older one's result stale.

use std::path::PathBuf;
use std::time::Instant;

use gpui::{App, AppContext as _, Context, Entity};

use super::super::Accent;
use super::super::clock::PlayClock;
use super::super::save_io::{self, Persist, Read, Saver};
use super::grid::{self, CELLS, Difficulty};
use super::play::{Game, Stats};
use super::save::File;

/// Plugin id (`[plugins.settings.sudoku]`).
pub const ID: &str = "sudoku";
const FILE: &str = "sudoku.json";
/// Without a key or click on the desktop widget for this long, its clock
/// stops (you looked away, or a window covers it).
pub const DESKTOP_IDLE: std::time::Duration = std::time::Duration::from_secs(90);

/// `[plugins.settings.sudoku]`, resolved.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// What "New game" deals.
    pub difficulty: Difficulty,
    /// Shade the selected cell's row, column and box.
    pub peers: bool,
    /// Shade every cell holding the selected digit.
    pub same: bool,
    /// Mark digits that clash with their row, column or box.
    pub conflicts: bool,
    /// Show the clock.
    pub timer: bool,
    /// Offer hints.
    pub hints: bool,
    /// A digit clears its pencil marks from the cell's peers.
    pub clear_notes: bool,
    /// Count entries that disagree with the solution.
    pub mistakes: bool,
    pub accent: Accent,
    /// The bar item shows the clock while you play.
    pub bar_time: bool,
}

impl Settings {
    pub fn of(cfg: &mandara_config::ShellConfig) -> Self {
        let s = &cfg.plugins;
        let flag = |key: &str, default: bool| s.setting(ID, key, default);
        Self {
            difficulty: Difficulty::parse(&s.setting::<String>(ID, "difficulty", String::new())).unwrap_or(Difficulty::Medium),
            peers: flag("highlight_peers", true),
            same: flag("highlight_same", true),
            conflicts: flag("conflicts", true),
            timer: flag("timer", true),
            hints: flag("hints", true),
            clear_notes: flag("clear_notes", true),
            mistakes: flag("mistakes", true),
            accent: Accent::parse(&s.setting::<String>(ID, "accent", String::new())),
            bar_time: flag("bar_time", false),
        }
    }
}

pub struct Sudoku {
    pub loaded: bool,
    pub game: Option<Game>,
    pub stats: [Stats; 4],
    pub clock: PlayClock,
    pub selected: Option<u8>,
    /// Digits go in as pencil marks.
    pub notes_mode: bool,
    /// A puzzle is being dealt.
    pub generating: Option<Difficulty>,
    request: u64,
    /// When the current game was solved (the celebration's start).
    pub solved_at: Option<Instant>,
    pub settings: Settings,
    panel_open: bool,
    /// The desktop widget counts as played until then (each input there
    /// extends it; it ends when the widget lets go of the keyboard).
    desktop_until: Option<Instant>,
    saver: Saver,
}

impl Persist for Sudoku {
    fn saver(&mut self) -> &mut Saver {
        &mut self.saver
    }

    fn snapshot(&self) -> Vec<u8> {
        File::new(self.game.as_ref(), self.clock.elapsed_ms(Instant::now()), &self.stats).to_bytes()
    }

    fn file(&self) -> PathBuf {
        save_io::path(FILE)
    }
}

impl Sudoku {
    /// The model, loading its file on the blocking pool; config changes
    /// re-resolve the settings and the shell's panel state drives the clock.
    pub fn create(cx: &mut App) -> Entity<Self> {
        let settings = Settings::of(crate::state::config(cx).read(cx).get());
        let entity = cx.new(|cx| {
            cx.observe(&crate::state::config(cx), |m: &mut Self, store, cx| {
                let next = Settings::of(store.read(cx).get());
                if next != m.settings {
                    m.settings = next;
                    cx.notify();
                }
            })
            .detach();
            cx.observe(&crate::state::shell(cx), |m: &mut Self, _, cx| {
                let open = crate::state::shell(cx).read(cx).panel.as_deref() == Some(ID) || crate::floating::live_ids(cx).iter().any(|p| p == ID);
                if open != m.panel_open {
                    m.panel_open = open;
                    m.resync(cx);
                    cx.notify();
                }
            })
            .detach();
            Self {
                loaded: false,
                game: None,
                stats: [Stats::default(); 4],
                clock: PlayClock::default(),
                selected: None,
                notes_mode: false,
                generating: None,
                request: 0,
                solved_at: None,
                settings,
                panel_open: false,
                desktop_until: None,
                saver: Saver::default(),
            }
        });
        let weak = entity.downgrade();
        cx.spawn(async move |cx| {
            let file = save_io::path(FILE);
            let parsed = mandara_services::runtime::unblock(move || match save_io::read(&file) {
                Read::Missing => None,
                Read::Bytes(bytes) => {
                    let parsed = File::parse(&bytes);
                    if parsed.is_none() {
                        tracing::warn!("sudoku: {} isn't a save Mandara can read; starting fresh", file.display());
                        save_io::quarantine(&file);
                    }
                    parsed
                }
                Read::Unreadable(e) => {
                    tracing::warn!("sudoku: {}: {e}", file.display());
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
            if let Some(g) = game {
                self.clock = PlayClock::starting_at(g.elapsed_ms);
                self.solved_at = None;
                self.selected = first_open(&g);
                self.game = Some(g);
            }
        }
        if self.game.is_none() {
            self.new_game(self.settings.difficulty, None, cx);
        }
        self.sync_clock();
        cx.notify();
    }

    /// Saves and stops the clock (the plugin was switched off).
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.panel_open = false;
        self.desktop_until = None;
        self.sync_clock();
        save_io::flush_in_background(self, cx);
    }

    /// Deals a new puzzle (`seed`: a given one, else a fresh one).
    pub fn new_game(&mut self, difficulty: Difficulty, seed: Option<u64>, cx: &mut Context<Self>) {
        self.request += 1;
        let request = self.request;
        self.generating = Some(difficulty);
        self.sync_clock();
        cx.notify();
        let seed = seed.unwrap_or_else(super::super::rng::fresh_seed);
        cx.spawn(async move |this, cx| {
            let puzzle = mandara_services::runtime::unblock(move || grid::generate(difficulty, seed)).await;
            let _ = this.update(cx, |m, cx| {
                if m.request != request {
                    return;
                }
                let game = Game::new(difficulty, seed, &puzzle);
                m.selected = first_open(&game);
                m.game = Some(game);
                m.generating = None;
                m.notes_mode = false;
                m.solved_at = None;
                m.clock = PlayClock::default();
                let s = &mut m.stats[difficulty.index()];
                s.played = s.played.saturating_add(1);
                m.sync_clock();
                save_io::changed(m, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The clock runs while a panel shows the game or its desktop widget is
    /// being played, and the game is unsolved.
    fn sync_clock(&mut self) -> bool {
        let was = self.clock.running();
        let now = Instant::now();
        let playing = self.game.as_ref().is_some_and(|g| !g.won) && self.generating.is_none() && !crate::lock::is_locked();
        let desktop = self.desktop_until.filter(|t| *t > now);
        if self.desktop_until.is_some() && desktop.is_none() && !self.panel_open {
            // Stopped playing on the desktop: the clock stops when that happened.
            let until = self.desktop_until.take().unwrap_or(now);
            if self.clock.running() {
                self.clock.pause(until.min(now));
            }
        }
        self.desktop_until = desktop;
        self.clock.set(playing && (self.panel_open || desktop.is_some()), now);
        was && !self.clock.running()
    }

    /// Follows the clock and saves the time played when it stops (closing
    /// the panel, leaving the desktop widget), so a restart keeps it.
    fn resync(&mut self, cx: &mut Context<Self>) {
        if self.sync_clock() {
            save_io::changed(self, cx);
        }
    }

    /// Something was played on the desktop widget.
    pub fn desktop_input(&mut self, cx: &mut Context<Self>) {
        self.desktop_until = Some(Instant::now() + DESKTOP_IDLE);
        self.resync(cx);
        cx.notify();
    }

    /// The desktop widget let go of the keyboard (or play there went idle).
    pub fn desktop_left(&mut self, cx: &mut Context<Self>) {
        if self.desktop_until.take().is_some() {
            self.resync(cx);
            cx.notify();
        }
    }

    /// Re-checks the desktop's idle deadline (the view's clock tick).
    pub fn desktop_tick(&mut self, cx: &mut Context<Self>) {
        if self.desktop_until.is_some_and(|t| t <= Instant::now()) {
            self.resync(cx);
            cx.notify();
        }
    }

    pub fn set_panel_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.panel_open != open {
            self.panel_open = open;
            self.resync(cx);
            cx.notify();
        }
    }

    pub fn select(&mut self, cell: usize, cx: &mut Context<Self>) {
        if cell < CELLS && self.selected != Some(cell as u8) {
            self.selected = Some(cell as u8);
            cx.notify();
        }
    }

    /// Moves the selection, wrapping at the edges.
    pub fn step(&mut self, dx: i32, dy: i32, cx: &mut Context<Self>) {
        let at = self.selected.map_or(40, |c| c as i32);
        let (r, c) = ((at / 9 + dy).rem_euclid(9), (at % 9 + dx).rem_euclid(9));
        self.select((r * 9 + c) as usize, cx);
    }

    pub fn toggle_notes_mode(&mut self, cx: &mut Context<Self>) {
        self.notes_mode = !self.notes_mode;
        cx.notify();
    }

    /// A digit from the pad or the keyboard: a pencil mark in notes mode
    /// (or with `note`), else an entry.
    pub fn digit(&mut self, d: u8, note: bool, cx: &mut Context<Self>) -> bool {
        let (Some(cell), Some(game)) = (self.selected.map(usize::from), self.game.as_mut()) else { return false };
        let changed = if note || self.notes_mode {
            game.toggle_note(cell, d)
        } else {
            let e = game.enter(cell, d, self.settings.clear_notes);
            if e.won {
                self.solved();
            }
            e.changed
        };
        self.after(changed, cx)
    }

    pub fn erase(&mut self, cx: &mut Context<Self>) -> bool {
        let (Some(cell), Some(game)) = (self.selected.map(usize::from), self.game.as_mut()) else { return false };
        let changed = game.erase(cell);
        self.after(changed, cx)
    }

    pub fn hint(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.settings.hints {
            return false;
        }
        let prefer = self.selected.map(usize::from);
        let clear = self.settings.clear_notes;
        let Some(game) = self.game.as_mut() else { return false };
        let Some((cell, e)) = game.hint(prefer, clear) else { return false };
        self.selected = Some(cell as u8);
        if e.won {
            self.solved();
        }
        self.after(true, cx)
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(cell) = self.game.as_mut().and_then(Game::undo) else { return false };
        self.selected = Some(cell as u8);
        self.after(true, cx)
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(cell) = self.game.as_mut().and_then(Game::redo) else { return false };
        self.selected = Some(cell as u8);
        if self.game.as_ref().is_some_and(|g| g.won) && self.solved_at.is_none() {
            self.solved();
        }
        self.after(true, cx)
    }

    fn after(&mut self, changed: bool, cx: &mut Context<Self>) -> bool {
        if changed {
            save_io::changed(self, cx);
            cx.notify();
        }
        changed
    }

    fn solved(&mut self) {
        let now = Instant::now();
        self.clock.pause(now);
        self.solved_at = Some(now);
        if let Some(g) = &self.game {
            let ms = self.clock.elapsed_ms(now);
            self.stats[g.difficulty.index()].record_win(ms, g.mistakes == 0 && g.hints == 0);
        }
        self.sync_clock();
    }

    /// A summary for `mandara msg sudoku status`.
    pub fn status(&self) -> serde_json::Value {
        let now = Instant::now();
        match &self.game {
            None => serde_json::json!({ "loaded": self.loaded, "generating": self.generating.map(Difficulty::id) }),
            Some(g) => serde_json::json!({
                "difficulty": g.difficulty.id(),
                "seed": g.seed,
                "left": g.left(),
                "mistakes": g.mistakes,
                "hints": g.hints,
                "elapsed_ms": self.clock.elapsed_ms(now),
                "clock_running": self.clock.running(),
                "solved": g.won,
                "selected": self.selected,
                "notes_mode": self.notes_mode,
                "board": grid::format(&g.values),
                "generating": self.generating.map(Difficulty::id),
                "stats": self.stats[g.difficulty.index()],
            }),
        }
    }
}

/// The first cell you can write in (where the selection starts).
fn first_open(g: &Game) -> Option<u8> {
    (0..CELLS).find(|&i| !g.is_given(i) && g.values[i] == 0).map(|i| i as u8)
}
