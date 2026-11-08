//! Labelled examples for the Desktop info widgets: the machine card, screen
//! time, prayer times, at a glance, GitHub, exchange rates, AI usage and
//! games. Studio's gallery pictures and its arrangement preview use them;
//! live desktop scenes never do.
//!
//! Everything here is a constant or arithmetic on today's date: nothing
//! reads the machine, a log, a store or the network, so painting a preview
//! starts no loader and touches no personal data.

use chrono::{Duration, Local, NaiveTime};
use mandara_config::desktop_widgets::{DesktopWidget, WidgetKind};
use mandara_config::widget_kinds::{count, ranges};
use mandara_i18n::{t, tf};
use mandara_services::desktop_feeds::{ContributionDay, DayTokens, Rates};
use mandara_services::prayer::Prayer;
use mandara_theme::MandaraPalette;
use tiny_skia::{Pixmap, PixmapPaint, Transform};

use super::super::{Feed, FetchInfo, GameItem, GlanceInfo, PrayerInfo, ScreenInfo, WidgetScene, feeds, lingo};

/// The sample machine: the sandbox's demo identity and plausible parts.
const USER: &str = "demo";
const HOST: &str = "mandara-demo";
const OS: &str = "Arch Linux";
const COMPOSITOR: &str = "MangoWM";
const CPU: &str = "AMD Ryzen 7 7840U";
const GPU: &str = "AMD Radeon 780M";
const SHELL: &str = "fish";
const SCREEN: &str = "2560×1440";
const MEMORY: (u64, u64) = (11_400_000_000, 32_000_000_000);
const UPTIME_S: u64 = 3 * 3600 + 25 * 60;
/// Today's screen time last, minutes a day.
const SCREEN_MINUTES: [u64; 7] = [190, 245, 160, 320, 235, 110, 155];
/// Today's apps, shares of today's total.
const APP_SHARES: [f32; 3] = [0.46, 0.27, 0.15];
const SECS_PER_MINUTE: u64 = 60;
/// A typical day's prayer times `(hour, minute)`, in [`Prayer::ALL`] order.
const PRAYER_TIMES: [(u32, u32); 6] = [(4, 52), (6, 18), (11, 47), (14, 53), (17, 16), (18, 36)];
const TOMORROW_FAJR: (u32, u32) = (4, 53);
/// The glance's example: a few unread notifications.
const UNREAD: u32 = 3;
/// The user name a sample GitHub widget is given (never fetched).
const GITHUB_USER: &str = "octocat";
/// Weeks of contribution days, and the levels' spread.
const CONTRIBUTION_WEEKS: i64 = 53;
const LEVELS: u32 = 5;
const COUNT_PER_LEVEL: u32 = 3;
/// Mixes a day's index into a stable level (a multiplicative hash).
const HASH: u32 = 2_654_435_761;
const HASH_SHIFT: u32 = 27;
/// Example rates per US dollar; other codes get a made-up rate.
const RATES: [(&str, f64); 8] = [("USD", 1.0), ("EUR", 0.92), ("GBP", 0.79), ("EGP", 48.7), ("SAR", 3.75), ("AED", 3.67), ("JPY", 151.3), ("TRY", 34.2)];
const OTHER_RATE: f64 = 1.37;
/// Claude Code's tokens on the last days, millions (today last).
const TOKENS_M: [f64; 14] = [0.8, 1.6, 2.1, 0.0, 1.2, 2.9, 1.7, 2.4, 1.1, 3.2, 2.6, 0.9, 1.8, 2.2];
const MILLION: f64 = 1_000_000.0;
const OUTPUT_SHARE: f64 = 0.08;
const INPUT_SHARE: f64 = 0.04;
/// Example games: real titles, plain covers in the theme's colours.
const GAMES: [(u32, &str); 4] = [(504_230, "Celeste"), (1_145_360, "Hades"), (620, "Portal 2"), (413_150, "Stardew Valley")];
const COVER_RATIO: f32 = 1.5;
const COVER_MIN_PX: u32 = 16;
const COVER_MAX_PX: u32 = 256;
/// A cover is this share of the picture's long side.
const COVER_SHARE: u32 = 5;
/// The glyph on a cover, as a share of its width.
const COVER_GLYPH: f32 = 0.5;
/// The search field's width in a gallery picture, reference px: the
/// default 520 would shrink its words to a few pixels in a tile.
const SEARCH_PREVIEW_W: f32 = 320.0;

/// Gallery pictures show every widget at its best: a feed is shown
/// switched on (with a sample user), and the search field narrower so it
/// fills its tile. Only the picture's own copy of the widget changes.
pub(super) fn prepare(widget: &mut DesktopWidget) {
    match &mut widget.kind {
        WidgetKind::Github(g) => {
            if g.user.trim().is_empty() {
                g.user = GITHUB_USER.into();
            }
            g.online = true;
        }
        WidgetKind::Currency(c) => c.online = true,
        WidgetKind::AiUsage(a) => a.read_logs = true,
        WidgetKind::Search(s) => s.width = s.width.min(SEARCH_PREVIEW_W),
        _ => {}
    }
}

/// Whether a widget's preview shows sample data (Studio labels it).
pub fn has_sample(kind: &WidgetKind) -> bool {
    matches!(kind, WidgetKind::Fetch(_) | WidgetKind::ScreenTime(_) | WidgetKind::Prayer(_) | WidgetKind::Glance(_) | WidgetKind::Github(_) | WidgetKind::Currency(_) | WidgetKind::AiUsage(_) | WidgetKind::Games(_))
}

/// Fills `scene` with the example `widget` shows. A feed that still needs
/// setting up (no GitHub user, fetching off) keeps its empty state, so a
/// preview of your own widget shows what it will really say. `palette`
/// colours the game covers (none: covers are plain cards with the name).
pub fn populate(scene: &mut WidgetScene, widget: &DesktopWidget, palette: Option<&MandaraPalette>, picture_px: u32) {
    let d = &mut scene.data;
    match &widget.kind {
        WidgetKind::Fetch(_) => d.fetch = Some(fetch()),
        WidgetKind::ScreenTime(s) => d.screen = Some(screen(count(s.days, ranges::SCREEN_DAYS) as usize)),
        WidgetKind::Prayer(_) => d.prayer = Some(prayer()),
        WidgetKind::Glance(_) => d.glance = Some(GlanceInfo { icon: "bell", text: tf("{count} unread notifications", &[("count", &UNREAD)]).to_string(), action: None }),
        WidgetKind::Github(g) if feeds::github_off(g).is_none() => {
            d.github.insert(widget.id.clone(), ready(contributions()));
        }
        WidgetKind::Currency(c) if c.online => {
            d.rates.insert(widget.id.clone(), ready(rates(&c.base, &c.pairs)));
        }
        WidgetKind::AiUsage(a) if a.read_logs => {
            d.ai_usage.insert(widget.id.clone(), ready(tokens()));
        }
        WidgetKind::Games(g) => d.games = ready(games(count(g.count, ranges::GAMES) as usize, palette, picture_px)),
        _ => {}
    }
}

fn ready<T>(value: T) -> Feed<T> {
    Feed { value: Some(value), error: None, loading: false }
}

fn fetch() -> FetchInfo {
    FetchInfo {
        user: USER.into(), host: HOST.into(), os: OS.into(), kernel: crate::plugins::resources::demo::KERNEL.into(), desktop: COMPOSITOR.into(),
        cpu: CPU.into(), gpu: GPU.into(), memory: MEMORY, uptime_s: UPTIME_S, shell: SHELL.into(), screen: SCREEN.into(), logo: None,
    }
}

/// The last `days` days (today last, flagged) and today's top apps.
fn screen(days: usize) -> ScreenInfo {
    let today = Local::now().date_naive();
    let days = days.clamp(1, SCREEN_MINUTES.len());
    let shown = &SCREEN_MINUTES[SCREEN_MINUTES.len() - days..];
    let days = shown.iter().enumerate().map(|(i, minutes)| {
        let date = today - Duration::days((shown.len() - 1 - i) as i64);
        (crate::widgets::clock::localized(&date, "%a"), minutes * SECS_PER_MINUTE, i + 1 == shown.len())
    }).collect();
    let total = shown.last().copied().unwrap_or_default() * SECS_PER_MINUTE;
    let top = [t("Browser"), t("Files"), t("Terminal")].into_iter().zip(APP_SHARES).map(|(name, share)| (name.to_string(), (total as f32 * share) as u64)).collect();
    ScreenInfo { days, top }
}

fn at((h, m): (u32, u32)) -> NaiveTime {
    NaiveTime::from_hms_opt(h, m, 0).unwrap_or_default()
}

fn prayer() -> PrayerInfo {
    PrayerInfo {
        times: Prayer::ALL.into_iter().zip(PRAYER_TIMES).map(|(p, time)| (p.name(), p.name_ar(), at(time))).collect(),
        tomorrow_fajr: at(TOMORROW_FAJR),
        hijri: Some(lingo::hijri_of(Local::now().date_naive())),
        lang: None,
    }
}

/// A year of days, busier on weekdays, ending today.
fn contributions() -> Vec<ContributionDay> {
    let today = Local::now().date_naive();
    (0..CONTRIBUTION_WEEKS * 7).rev().map(|back| {
        let date = today - Duration::days(back);
        let level = ((back as u32).wrapping_mul(HASH) >> HASH_SHIFT) % LEVELS;
        ContributionDay { date, level: level as u8, count: level * COUNT_PER_LEVEL }
    }).collect()
}

fn rates(base: &str, pairs: &[String]) -> Rates {
    let per_usd = |code: &str| RATES.iter().find(|(c, _)| c.eq_ignore_ascii_case(code)).map(|(_, r)| *r);
    let base = if base.trim().is_empty() { "USD".to_string() } else { base.trim().to_ascii_uppercase() };
    let base_rate = per_usd(&base).unwrap_or(1.0);
    let rates = pairs.iter().map(|code| {
        let code = code.trim().to_ascii_uppercase();
        let rate = per_usd(&code).unwrap_or(OTHER_RATE) / base_rate;
        (code, rate)
    }).collect();
    Rates { base, rates, updated: Local::now().timestamp() }
}

fn tokens() -> std::collections::BTreeMap<chrono::NaiveDate, DayTokens> {
    let today = Local::now().date_naive();
    TOKENS_M.iter().rev().enumerate().map(|(back, m)| {
        let total = m * MILLION;
        let (output, input) = ((total * OUTPUT_SHARE) as u64, (total * INPUT_SHARE) as u64);
        (today - Duration::days(back as i64), DayTokens { input, output, cache: (total as u64).saturating_sub(input + output) })
    }).collect()
}

/// `n` example games; with a palette, each gets a cover of its own colour.
fn games(n: usize, palette: Option<&MandaraPalette>, picture_px: u32) -> Vec<GameItem> {
    let w = (picture_px / COVER_SHARE).clamp(COVER_MIN_PX, COVER_MAX_PX);
    let h = (w as f32 * COVER_RATIO).round() as u32;
    GAMES.iter().take(n).enumerate().map(|(i, (appid, name))| {
        let cover = palette.and_then(|p| {
            let bg = [p.primary, p.secondary, p.tertiary, p.accent_gradient_end][i % 4];
            // The theme's text or surface colour, whichever stands out more.
            let fg = if (bg.lightness() - p.on_surface.lightness()).abs() > (bg.lightness() - p.surface_base.lightness()).abs() { p.on_surface } else { p.surface_base };
            let mut pm = Pixmap::new(w, h)?;
            let [r, g, b, a] = bg.to_rgba8();
            pm.fill(tiny_skia::Color::from_rgba8(r, g, b, a));
            let glyph = (w as f32 * COVER_GLYPH) as u32;
            if let Some(icon) = crate::backdrop::paint::icon_pixmap("dice", glyph, fg) {
                pm.draw_pixmap(((w - glyph) / 2) as i32, ((h - glyph) / 2) as i32, icon.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
            }
            Some(super::super::layout::Picture { key: super::super::key((bg.to_rgba8(), fg.to_rgba8(), w, i)), pixmap: std::sync::Arc::new(pm) })
        });
        GameItem { appid: *appid, name: (*name).into(), cover }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::desktop_widgets::{AiUsageWidget, CurrencyWidget, FetchWidget, GamesWidget, GithubWidget, GlanceWidget, PrayerWidget, ScreenTimeWidget, SearchWidget};

    fn kinds() -> [WidgetKind; 8] {
        [
            WidgetKind::Fetch(FetchWidget::default()),
            WidgetKind::ScreenTime(ScreenTimeWidget::default()),
            WidgetKind::Prayer(PrayerWidget::default()),
            WidgetKind::Glance(GlanceWidget::default()),
            WidgetKind::Github(GithubWidget::default()),
            WidgetKind::Currency(CurrencyWidget::default()),
            WidgetKind::AiUsage(AiUsageWidget::default()),
            WidgetKind::Games(GamesWidget::default()),
        ]
    }

    #[test]
    fn sample_data_has_no_empty_fields() {
        let f = fetch();
        for (name, value) in [("user", &f.user), ("host", &f.host), ("os", &f.os), ("kernel", &f.kernel), ("desktop", &f.desktop), ("cpu", &f.cpu), ("gpu", &f.gpu), ("shell", &f.shell), ("screen", &f.screen)] {
            assert!(!value.trim().is_empty(), "machine {name} is empty");
        }
        assert!(f.memory.0 > 0 && f.memory.1 > f.memory.0 && f.uptime_s >= SECS_PER_MINUTE);
        let s = screen(7);
        assert_eq!(s.days.len(), 7);
        assert!(s.days.iter().all(|(name, secs, _)| !name.is_empty() && *secs > 0) && s.days.last().is_some_and(|d| d.2));
        assert!(s.top.iter().all(|(name, secs)| !name.is_empty() && *secs > 0));
        assert_eq!(screen(1).days.len(), 1);
        let p = prayer();
        assert_eq!(p.times.len(), Prayer::ALL.len());
        assert!(p.times.windows(2).all(|w| w[0].2 < w[1].2), "times in order");
        assert!(contributions().iter().any(|d| d.level > 0) && contributions().len() as i64 == CONTRIBUTION_WEEKS * 7);
        let r = rates("USD", &["EUR".into(), "egp".into(), "XYZ".into()]);
        assert_eq!(r.rates.len(), 3);
        assert!(r.rates.values().all(|v| *v > 0.0) && r.rates.contains_key("EGP"));
        let t = tokens();
        assert!(t.get(&Local::now().date_naive()).is_some_and(|d| d.total() > 0));
    }

    #[test]
    fn every_info_kind_gets_its_sample_and_live_scenes_stay_empty() {
        let palette = mandara_theme::engine::default_palette(false);
        for kind in kinds() {
            assert!(has_sample(&kind));
            let mut widget = DesktopWidget { kind, ..Default::default() };
            prepare(&mut widget);
            let mut scene = WidgetScene::default();
            populate(&mut scene, &widget, Some(&palette), 400);
            let d = &scene.data;
            let filled = d.fetch.is_some() || d.screen.is_some() || d.prayer.is_some() || d.glance.is_some() || !d.github.is_empty() || !d.rates.is_empty() || !d.ai_usage.is_empty() || d.games.value.is_some();
            assert!(filled, "{:?} has no sample", widget.kind.id());
        }
        let games = games(4, Some(&palette), 400);
        assert!(games.iter().all(|g| g.cover.as_ref().is_some_and(|c| c.pixmap.width() == 400 / COVER_SHARE)));
        assert!(!has_sample(&WidgetKind::Search(SearchWidget::default())));
        let live = WidgetScene::default().data;
        assert!(live.fetch.is_none() && live.screen.is_none() && live.prayer.is_none() && live.glance.is_none() && live.github.is_empty() && live.games.value.is_none());
    }

    /// Paints every info kind's gallery picture with the real painters, in
    /// light and dark; `MANDARA_INFO_PREVIEW_DUMP=<dir>` saves them.
    #[test]
    fn info_thumbnails_fill_their_tile_in_light_and_dark() {
        use mandara_config::desktop_widgets::{FetchLayout, PrayerLayout};
        let fonts = crate::backdrop::paint::FontNames::new("Inter", "Alexandria", "Amiri", "JetBrains Mono");
        let mut painter = crate::lock::paint::TextPainter::new();
        let mut all: Vec<(&str, WidgetKind)> = kinds().into_iter().map(|k| (k.id(), k)).collect();
        all.push(("fetch-grid", WidgetKind::Fetch(FetchWidget { layout: FetchLayout::Grid, logo: false, colors: false, ..Default::default() })));
        all.push(("prayer-list", WidgetKind::Prayer(PrayerWidget { layout: PrayerLayout::List, ..Default::default() })));
        all.push(("search", WidgetKind::Search(SearchWidget::default())));
        let size = (400, 300);
        for dark in [false, true] {
            let palette = mandara_theme::engine::default_palette(dark);
            for (name, kind) in &all {
                let widget = DesktopWidget { kind: kind.clone(), ..Default::default() };
                let image = super::super::thumbnail(&widget, size, &fonts, &palette, &mut painter).expect(name);
                // Content reaches across most of the tile in one direction.
                let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
                for (x, y, p) in image.enumerate_pixels() {
                    if p.0[3] > 0 {
                        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
                    }
                }
                assert!(x1 > x0 && y1 > y0, "{name}: empty thumbnail");
                let fill = ((x1 - x0) as f32 / size.0 as f32).max((y1 - y0) as f32 / size.1 as f32);
                assert!(fill > 0.75, "{name}: fills only {fill:.2} of its tile");
                if let Some(dir) = std::env::var_os("MANDARA_INFO_PREVIEW_DUMP") {
                    let dir = std::path::PathBuf::from(dir);
                    std::fs::create_dir_all(&dir).unwrap();
                    image.save(dir.join(format!("{name}-{}.png", if dark { "dark" } else { "light" }))).unwrap();
                }
            }
        }
    }

    #[test]
    fn a_feed_that_needs_setting_up_keeps_its_empty_state_outside_the_gallery() {
        let widget = DesktopWidget { id: "github-1".into(), kind: WidgetKind::Github(GithubWidget::default()), ..Default::default() };
        let mut scene = WidgetScene::default();
        populate(&mut scene, &widget, None, 400);
        assert!(scene.data.github.is_empty(), "no user: the preview says so");
        let mut gallery = widget.clone();
        prepare(&mut gallery);
        populate(&mut scene, &gallery, None, 400);
        assert!(scene.data.github.contains_key("github-1"));
        let mut search = DesktopWidget { kind: WidgetKind::Search(SearchWidget::default()), ..Default::default() };
        prepare(&mut search);
        assert!(matches!(search.kind, WidgetKind::Search(s) if s.width == SEARCH_PREVIEW_W));
    }
}
