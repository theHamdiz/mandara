//! The feeds behind the GitHub, exchange rate, AI usage and games
//! widgets, fetched off the UI thread and cached: network answers on disk
//! too (a restart doesn't ask again), each kept for its time to live.
//! Nothing is asked for while no such widget is placed, or while the
//! desktop can't be seen.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::NaiveDate;
use gpui::{App, Global};
use mandara_services::desktop_feeds::{self as feeds, ContributionDay, DayTokens, Rates, SteamGame};

use crate::backdrop::widgets::Feed;

/// How long answers stay fresh.
const GITHUB_TTL: Duration = Duration::from_secs(6 * 3600);
const RATES_TTL: Duration = Duration::from_secs(12 * 3600);
const AI_TTL: Duration = Duration::from_secs(300);
const GAMES_TTL: Duration = Duration::from_secs(600);
/// After a failure, wait this long before asking again.
const RETRY: Duration = Duration::from_secs(900);

struct Entry<T> {
    value: Option<T>,
    error: Option<String>,
    at: Option<Instant>,
    loading: bool,
}

impl<T> Default for Entry<T> {
    fn default() -> Self {
        Self { value: None, error: None, at: None, loading: false }
    }
}

impl<T: Clone> Entry<T> {
    fn feed(&self) -> Feed<T> {
        Feed { value: self.value.clone(), error: self.error.clone(), loading: self.loading }
    }

    /// Due for a new fetch (never fetched, stale, or a failure long ago).
    fn due(&self, ttl: Duration) -> bool {
        !self.loading && self.at.is_none_or(|t| t.elapsed() >= if self.error.is_some() { RETRY } else { ttl })
    }
}

#[derive(Default)]
struct Feeds {
    github: HashMap<String, Entry<Vec<ContributionDay>>>,
    rates: HashMap<String, Entry<Rates>>,
    ai: Entry<BTreeMap<NaiveDate, DayTokens>>,
    ai_cache: Arc<Mutex<feeds::UsageCache>>,
    games: Entry<Vec<SteamGame>>,
}

impl Global for Feeds {}

fn state(cx: &mut App) -> &mut Feeds {
    if !cx.has_global::<Feeds>() {
        cx.set_global(Feeds::default());
    }
    cx.global_mut::<Feeds>()
}

/// Runs `work` in the background and stores its answer with `store`.
fn spawn<T: Send + 'static>(cx: &mut App, work: impl FnOnce() -> Result<T, String> + Send + 'static, store: impl FnOnce(&mut Feeds, Result<T, String>) + 'static) {
    cx.spawn(async move |cx| {
        let result = cx.background_executor().spawn(async move { work() }).await;
        cx.update(|cx| {
            store(state(cx), result);
            crate::backdrop::refresh(cx);
        });
    })
    .detach();
}

fn finish<T>(e: &mut Entry<T>, r: Result<T, String>) {
    e.loading = false;
    e.at = Some(Instant::now());
    match r {
        Ok(v) => {
            e.value = Some(v);
            e.error = None;
        }
        Err(err) => e.error = Some(err),
    }
}

fn disk(name: &str) -> std::path::PathBuf {
    feeds::cache_dir().join(name)
}

/// The GitHub calendar of `user` (`fetch`: may ask github.com now).
pub fn github(user: &str, fetch: bool, cx: &mut App) -> Feed<Vec<ContributionDay>> {
    let user = user.trim().to_string();
    let s = state(cx);
    let e = s.github.entry(user.clone()).or_default();
    if e.value.is_none()
        && e.at.is_none()
        && let Some(days) = read_github(&user)
    {
        e.value = Some(days);
    }
    if fetch && e.due(GITHUB_TTL) {
        e.loading = true;
        let u = user.clone();
        spawn(
            cx,
            move || {
                let days = feeds::github_contributions(&u)?;
                write_github(&u, &days);
                Ok(days)
            },
            {
                let user = user.clone();
                move |f, r| finish(f.github.entry(user).or_default(), r)
            },
        );
    }
    state(cx).github.get(&user).map(Entry::feed).unwrap_or_default()
}

fn github_file(user: &str) -> std::path::PathBuf {
    disk(&format!("github-{user}.json"))
}

fn read_github(user: &str) -> Option<Vec<ContributionDay>> {
    if !feeds::valid_github_user(user) {
        return None;
    }
    let raw: Vec<(String, u8, u32)> = serde_json::from_slice(&std::fs::read(github_file(user)).ok()?).ok()?;
    Some(raw.into_iter().filter_map(|(d, level, count)| Some(ContributionDay { date: NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok()?, level, count })).collect())
}

fn write_github(user: &str, days: &[ContributionDay]) {
    let raw: Vec<(String, u8, u32)> = days.iter().map(|d| (d.date.to_string(), d.level, d.count)).collect();
    if let Ok(json) = serde_json::to_vec(&raw) {
        let _ = std::fs::create_dir_all(feeds::cache_dir());
        let _ = mandara_config::write_atomic(&github_file(user), &json);
    }
}

/// Today's exchange rates against `base` (`fetch`: may ask now).
pub fn rates(base: &str, fetch: bool, cx: &mut App) -> Feed<Rates> {
    let base = base.trim().to_ascii_uppercase();
    let file = disk(&format!("rates-{base}.json"));
    let s = state(cx);
    let e = s.rates.entry(base.clone()).or_default();
    if e.value.is_none()
        && e.at.is_none()
        && feeds::valid_currency(&base)
        && let Some(r) = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice::<Rates>(&b).ok())
    {
        e.value = Some(r);
    }
    if fetch && e.due(RATES_TTL) {
        e.loading = true;
        let b = base.clone();
        spawn(
            cx,
            move || {
                let r = feeds::exchange_rates(&b)?;
                if let Ok(json) = serde_json::to_vec(&r) {
                    let _ = std::fs::create_dir_all(feeds::cache_dir());
                    let _ = mandara_config::write_atomic(&file, &json);
                }
                Ok(r)
            },
            {
                let base = base.clone();
                move |f, r| finish(f.rates.entry(base).or_default(), r)
            },
        );
    }
    state(cx).rates.get(&base).map(Entry::feed).unwrap_or_default()
}

/// Claude Code's tokens per day over the last `days` days.
pub fn ai_usage(days: u32, fetch: bool, cx: &mut App) -> Feed<BTreeMap<NaiveDate, DayTokens>> {
    let s = state(cx);
    if fetch && s.ai.due(AI_TTL) {
        s.ai.loading = true;
        let cache = s.ai_cache.clone();
        spawn(
            cx,
            move || {
                let mut c = cache.lock().map_err(|e| e.to_string())?;
                Ok(feeds::claude_usage(days, &mut c))
            },
            |f, r| finish(&mut f.ai, r),
        );
    }
    state(cx).ai.feed()
}

/// Installed Steam games, most recently played first.
pub fn games(fetch: bool, cx: &mut App) -> Feed<Vec<SteamGame>> {
    let s = state(cx);
    if fetch && s.games.due(GAMES_TTL) {
        s.games.loading = true;
        spawn(cx, || Ok(feeds::steam_games()), |f, r| finish(&mut f.games, r));
    }
    state(cx).games.feed()
}

/// Forgets what every feed fetched, so the next look asks again (a
/// widget's Refresh).
pub fn refresh(cx: &mut App) {
    let s = state(cx);
    for e in s.github.values_mut() {
        e.at = None;
    }
    for e in s.rates.values_mut() {
        e.at = None;
    }
    s.ai.at = None;
    s.games.at = None;
    crate::backdrop::refresh(cx);
}
