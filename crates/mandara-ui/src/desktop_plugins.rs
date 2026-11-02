//! The desktop widgets as first-party plugins.
//!
//! Every kind of desktop widget comes from a plugin registered here, like
//! any other built-in: clocks, the system monitor, decorations, the
//! visualizers, the now-playing and calendar cards, weather, world clocks
//! and the countdown ([`mandara_config::desktop_widgets::plugin_ids`]).
//! Each plugin offers its widget kinds through the registry
//! ([`PluginRegistry::desktop_widget`]); Studio's gallery and the desktop's
//! Add widget menu list the kinds of the plugins that are on. Switching a
//! plugin off hides its widgets (they stay in the list, ready to come back)
//! and takes its kinds out of the menus. Third-party WebAssembly plugins
//! offer theirs the same way ([`crate::plugins::external`]).
//!
//! Registration only stores these small tables: a plugin draws nothing and
//! costs nothing until one of its widgets is placed.

use gpui::App;
use mandara_config::ShellConfig;
use mandara_config::desktop_cards::{self as dc, CALENDAR, NOW_PLAYING};
use mandara_config::desktop_widgets::{
    AnalogOptions, Anchor, ClockFace, ClockWidget, CountdownWidget, DateStyle, DesktopWidget, DialMarks, FontRole, HandStyle, MonitorStyle, MonitorWidget, Period, Placement, PlaneVisualizer, Plate, PlateShape, Preset,
    ProgressStyle, ProgressWidget, SecondsStyle, ShadowKind, TemplateLine, Tone, Look, Chrome, MediaLayout, MediaWidget, MonitorRow, FetchWidget, FetchLayout, ScreenTimeWidget, PrayerWidget, PrayerLayout, JannahLayout, HadithLayout, SearchWidget, GlanceWidget, GithubWidget, CurrencyWidget, AiUsageWidget, GamesWidget, ProfileWidget, ShortcutsWidget, TileLayout, TogglesWidget, SlidersWidget, PaletteWidget, PetWidget, BubblesWidget, PhotoWidget, StickerWidget, DateLayout, DateWidget, SkyLayout, SkyWidget, TodoWidget, NoteWidget, NoteSheet, PomodoroWidget, QuoteWidget, NotebookWidget, VisualizerShape, WeatherLayout, WeatherWidget, WidgetKind, WorldClockWidget, plugin_ids as ids,
};
use mandara_i18n::tk;
use mandara_plugins::{DesktopWidgetType, PluginInstance, PluginManifest, PluginRegistry};

/// A new radial visualizer: inner diameter and spike length, reference px.
const RADIAL_DIAMETER_PX: f32 = 260.0;
const RADIAL_SPIKE_PX: f32 = 60.0;
/// Where a new system monitor goes: the top-right corner, inset.
const MONITOR_PLACE: (f32, f32) = (-0.03, 0.06);

/// Whether the plugin `id` is on (always, outside a shell or Studio with a
/// registry, e.g. in tests).
pub fn enabled(id: &str, cfg: &ShellConfig, cx: &App) -> bool {
    cx.try_global::<PluginRegistry>().is_none_or(|r| r.get(id).is_none() || r.is_enabled(id, &cfg.plugins))
}

/// Whether `w` is drawn: its plugin is on.
pub fn shows(w: &DesktopWidget, cfg: &ShellConfig, cx: &App) -> bool {
    enabled(w.kind.plugin(), cfg, cx)
}

/// One widget of `kind`, at the default place.
fn one(kind: WidgetKind) -> Vec<DesktopWidget> {
    vec![DesktopWidget { kind, ..DesktopWidget::default() }]
}

/// The name and icon of a widget in the list (the kind that made it).
pub fn label(w: &DesktopWidget) -> (&'static str, &'static str) {
    match &w.kind {
        WidgetKind::Clock(c) if c.face.depth_style().is_some() => (tk("Depth clock"), "clock"),
        WidgetKind::Clock(c) if c.face == ClockFace::Analog => (tk("Analog clock"), "clock"),
        WidgetKind::Clock(_) => (tk("Clock"), "clock"),
        WidgetKind::Monitor(m) if m.style == MonitorStyle::Gauges => (tk("System gauges"), "gauge"),
        WidgetKind::Monitor(m) if m.style == MonitorStyle::Meters => (tk("System meters"), "gauge"),
        WidgetKind::Monitor(m) if m.style == MonitorStyle::Bars => (tk("System bars"), "gauge"),
        WidgetKind::Monitor(m) if m.style == MonitorStyle::Thermals => (tk("Thermals"), "thermometer"),
        WidgetKind::Monitor(_) => (tk("System monitor"), "gauge"),
        WidgetKind::Decoration(_) => (tk("Decoration"), "leaf"),
        WidgetKind::Visualizer(_) => (tk("Visualizer"), "waves"),
        WidgetKind::Weather(_) => (tk("Weather"), "cloud-sun"),
        WidgetKind::WorldClock(_) => (tk("World clock"), "globe"),
        WidgetKind::Countdown(_) => (tk("Countdown"), "timer"),
        WidgetKind::Progress(_) => (tk("Progress"), "gauge"),
        WidgetKind::Todo(_) => (tk("To-do list"), "list-check"),
        WidgetKind::Note(_) => (tk("Sticky note"), "note"),
        WidgetKind::Pomodoro(_) => (tk("Focus timer"), "timer"),
        WidgetKind::Quote(_) => (tk("Quote"), "sparkles"),
        WidgetKind::Notebook(_) => (tk("Notebook tabs"), "note"),
        WidgetKind::Date(d) => match d.layout {
            DateLayout::Month => (tk("Month"), "calendar"),
            DateLayout::Week => (tk("Week strip"), "calendar"),
            DateLayout::Agenda => (tk("Coming up"), "calendar"),
            DateLayout::Tile => (tk("Date tile"), "calendar"),
        },
        WidgetKind::Media(m) => match m.layout {
            MediaLayout::Vinyl => (tk("Record player"), "music"),
            MediaLayout::Device => (tk("Pocket player"), "music"),
            MediaLayout::Shaped => (tk("Shaped cover"), "music"),
            MediaLayout::SidePill => (tk("Side pill"), "music"),
            MediaLayout::Halftone => (tk("Halftone cover"), "music"),
            MediaLayout::Lyrics => (tk("Lyrics"), "music"),
        },
        WidgetKind::Fetch(_) => (tk("Machine card"), "monitor"),
        WidgetKind::ScreenTime(_) => (tk("Screen time"), "timer"),
        WidgetKind::Prayer(_) => (tk("Prayer times"), "crescent"),
        WidgetKind::Jannah(j) => match j.layout {
            JannahLayout::Verse => (tk("Jannah verse"), "arch"),
            JannahLayout::Quote => (tk("Jannah quote"), "arch"),
            JannahLayout::Step => (tk("Today's step"), "arch"),
        },
        WidgetKind::Hadith(h) => match h.layout {
            HadithLayout::Hadith => (tk("Hadith of the day"), "hadith"),
            HadithLayout::Collection => (tk("Reading progress"), "hadith"),
        },
        WidgetKind::Search(_) => (tk("Search field"), "search"),
        WidgetKind::Glance(_) => (tk("At a glance"), "sparkles"),
        WidgetKind::Github(_) => (tk("GitHub contributions"), "grid"),
        WidgetKind::Currency(_) => (tk("Exchange rates"), "globe"),
        WidgetKind::AiUsage(_) => (tk("AI usage"), "sparkles"),
        WidgetKind::Games(_) => (tk("Games"), "dice"),
        WidgetKind::Profile(_) => (tk("Profile card"), "user"),
        WidgetKind::Shortcuts(s) if s.folders => (tk("Folders"), "folder"),
        WidgetKind::Shortcuts(_) => (tk("Shortcuts"), "grid"),
        WidgetKind::Toggles(_) => (tk("Quick toggles"), "sliders"),
        WidgetKind::Sliders(_) => (tk("Volume and brightness"), "sliders"),
        WidgetKind::Palette(_) => (tk("Palette"), "palette"),
        WidgetKind::Pet(_) => (tk("Desk cat"), "heart"),
        WidgetKind::Bubbles(_) => (tk("Bubbles"), "droplet"),
        WidgetKind::Photo(_) => (tk("Photo frame"), "image"),
        WidgetKind::Sticker(_) => (tk("Sticker"), "image"),
        WidgetKind::Sky(sk) => match sk.layout {
            SkyLayout::SunArc => (tk("Sun arc"), "sunset"),
            SkyLayout::Moon => (tk("Moon phase"), "crescent"),
            SkyLayout::Globe => (tk("Day and night globe"), "globe"),
        },
        WidgetKind::Card(c) => match c.card.as_str() {
            NOW_PLAYING => (tk("Now playing"), "music"),
            CALENDAR => (tk("Calendar"), "calendar"),
            dc::WAVE => (tk("Wave"), "waves"),
            dc::FRAMED_BARS => (tk("Framed bars"), "waves"),
            dc::MIRRORED_BARS => (tk("Mirrored bars"), "waves"),
            _ => (tk("Plugin widget"), "sparkles"),
        },
    }
}

/// The name of a clock face.
pub fn face_label(f: ClockFace) -> &'static str {
    match f {
        ClockFace::Stacked => tk("Depth · stacked"),
        ClockFace::Classic => tk("Depth · classic"),
        ClockFace::Arabic => tk("Depth · Arabic"),
        ClockFace::Superscript => tk("Superscript"),
        ClockFace::Headline => tk("Headline"),
        ClockFace::DayBlock => tk("Day block"),
        ClockFace::Ring => tk("Ring"),
        ClockFace::Greeting => tk("Greeting"),
        ClockFace::DayNumeral => tk("Day numeral"),
        ClockFace::Minimal => tk("Minimal"),
        ClockFace::Analog => tk("Analog"),
        ClockFace::Stack => tk("Stacked card"),
        ClockFace::Shape => tk("Time in a shape"),
        ClockFace::DotMatrix => tk("Dot matrix"),
        ClockFace::Pixel => tk("Pixel"),
        ClockFace::Boxes => tk("Digit boxes"),
        ClockFace::Hud => tk("Slanted HUD"),
        ClockFace::Template => tk("Text lines"),
        ClockFace::Numerals => tk("Hands over numerals"),
        ClockFace::Orbit => tk("Orbit"),
    }
}

/// The name of a visualizer shape.
pub fn vis_label(s: VisualizerShape) -> &'static str {
    match s {
        VisualizerShape::Bars => tk("Bars"),
        VisualizerShape::Mirrored => tk("Mirrored bars"),
        VisualizerShape::Wave => tk("Wave"),
        VisualizerShape::Radial => tk("Radial"),
        VisualizerShape::Led => tk("LED meter"),
    }
}

/// The name of a decoration preset.
pub fn preset_label(p: Preset) -> &'static str {
    match p {
        Preset::AlexandriaLine => tk("Alexandria line"),
        Preset::VerticalPoem => tk("Vertical poem"),
        Preset::Geometric => tk("Geometric set"),
    }
}

fn plugin(r: &mut PluginRegistry, id: &str, name: &'static str, description: &'static str) {
    r.register(PluginManifest::builtin(id, name, description, None), |_, _, _| PluginInstance::empty());
}

/// Registers the desktop widget plugins and the kinds they offer.
pub fn register(r: &mut PluginRegistry) {
    plugin(r, ids::CLOCKS, tk("Desktop clocks"), tk("Hero clocks on the desktop: analog and cookie dials, stacked cards, dot matrix, digit boxes, a slanted HUD, your own text lines, orbits, day and year progress, and the depth clock behind the wallpaper's subject"));
    for face in ClockFace::ALL.into_iter().filter(|f| f.depth_style().is_none() || *f == ClockFace::Stacked) {
        let mut kind = DesktopWidgetType::new(ids::CLOCKS, face.id(), face_label(face), tk("A clock on the desktop"), "clock", move || one(WidgetKind::Clock(clock_preset(face))));
        if matches!(face, ClockFace::Minimal | ClockFace::Analog) {
            kind.name = if face == ClockFace::Analog { tk("Analog clock") } else { tk("Clock") }.into();
            kind = kind.in_menu();
        }
        r.desktop_widget(kind);
    }
    for (id, name, description, make) in CLOCK_LOOKS {
        r.desktop_widget(DesktopWidgetType::new(ids::CLOCKS, id, name, description, "clock", move || one(WidgetKind::Clock(make()))));
    }
    for (id, name, period, style) in [
        ("day-ring", tk("Day progress"), Period::Day, ProgressStyle::Ring),
        ("week-wave", tk("Week progress"), Period::Week, ProgressStyle::WavyRing),
        ("month-bar", tk("Month progress"), Period::Month, ProgressStyle::Bar),
        ("year-dots", tk("Year in dots"), Period::Year, ProgressStyle::Dots),
    ] {
        let kind = DesktopWidgetType::new(ids::CLOCKS, id, name, tk("How much of it has passed"), "gauge", move || one(WidgetKind::Progress(ProgressWidget { period, style, ..ProgressWidget::default() })));
        r.desktop_widget(if period == Period::Day { kind.in_menu() } else { kind });
    }

    plugin(r, ids::MONITOR, tk("System monitor"), tk("Live CPU, GPU, memory, temperatures, disk and network on the desktop, as a column or ring gauges"));
    let monitor = |style: MonitorStyle| {
        move || vec![DesktopWidget { kind: WidgetKind::Monitor(MonitorWidget { style, ..MonitorWidget::default() }), place: Placement { anchor: Anchor::TopRight, x: MONITOR_PLACE.0, y: MONITOR_PLACE.1, ..Placement::default() }, ..DesktopWidget::default() }]
    };
    r.desktop_widget(DesktopWidgetType::new(ids::MONITOR, "column", tk("System monitor"), tk("A column of live readings"), "gauge", monitor(MonitorStyle::Column)).in_menu());
    r.desktop_widget(DesktopWidgetType::new(ids::MONITOR, "gauges", tk("System gauges"), tk("Ring gauges for CPU, GPU, memory, disk and battery"), "gauge", monitor(MonitorStyle::Gauges)).in_menu());
    r.desktop_widget(DesktopWidgetType::new(ids::MONITOR, "meters", tk("System meters"), tk("Segmented meters, like a battery's cells"), "gauge", monitor(MonitorStyle::Meters)));
    r.desktop_widget(DesktopWidgetType::new(ids::MONITOR, "bars", tk("System bars"), tk("Tall bars side by side"), "gauge", monitor(MonitorStyle::Bars)));
    r.desktop_widget(DesktopWidgetType::new(ids::MONITOR, "thermals", tk("Thermals"), tk("Arc gauges for the temperatures and the fan"), "thermometer", move || {
        vec![DesktopWidget {
            kind: WidgetKind::Monitor(MonitorWidget { style: MonitorStyle::Thermals, rows: vec![MonitorRow::CpuTemp, MonitorRow::GpuTemp, MonitorRow::Fan], ..MonitorWidget::default() }),
            place: Placement { anchor: Anchor::TopRight, x: MONITOR_PLACE.0, y: MONITOR_PLACE.1, ..Placement::default() },
            ..DesktopWidget::default()
        }]
    }));
    r.desktop_widget(DesktopWidgetType::new(ids::MONITOR, "wavy", tk("Wavy gauges"), tk("Ring gauges drawn as a wave"), "gauge", move || {
        vec![DesktopWidget {
            kind: WidgetKind::Monitor(MonitorWidget { style: MonitorStyle::Gauges, wavy: true, ring_width: WAVY_RING_PX, rows: vec![MonitorRow::Cpu, MonitorRow::Memory, MonitorRow::Disk], ..MonitorWidget::default() }),
            place: Placement { anchor: Anchor::TopRight, x: MONITOR_PLACE.0, y: MONITOR_PLACE.1, ..Placement::default() },
            ..DesktopWidget::default()
        }]
    }));

    plugin(r, ids::DECORATIONS, tk("Desktop decorations"), tk("Text and shapes on the desktop: lines of type, vertical poems, discs, stripes, waves and frames"));
    for p in Preset::ALL {
        let mut kind = DesktopWidgetType::new(ids::DECORATIONS, preset_id(p), preset_label(p), tk("Text and shapes"), "leaf", move || p.widgets());
        if p == Preset::AlexandriaLine {
            kind.name = tk("Decoration").into();
            kind = kind.in_menu();
        }
        r.desktop_widget(kind);
    }

    plugin(r, ids::VISUALIZERS, tk("Desktop visualizers"), tk("Audio visualizers on the desktop: bars, a mirrored strip, a wave or a ring, even behind the wallpaper's subject"));
    for s in [VisualizerShape::Bars, VisualizerShape::Mirrored, VisualizerShape::Wave, VisualizerShape::Radial, VisualizerShape::Led] {
        // A radial ring starts small: its plane is a square around it.
        let v = match s {
            VisualizerShape::Radial => PlaneVisualizer { shape: s, length: RADIAL_DIAMETER_PX, height: RADIAL_SPIKE_PX, ..PlaneVisualizer::default() },
            _ => PlaneVisualizer { shape: s, ..PlaneVisualizer::default() },
        };
        let mut kind = DesktopWidgetType::new(ids::VISUALIZERS, vis_id(s), vis_label(s), tk("Moves with the music"), "waves", move || one(WidgetKind::Visualizer(v.clone())));
        if s == VisualizerShape::Bars {
            kind.name = tk("Visualizer").into();
            kind = kind.in_menu();
        }
        r.desktop_widget(kind);
    }

    plugin(r, ids::NOW_PLAYING, tk("Now playing card"), tk("Album art, a wavy progress line and synced lyrics on the desktop while music plays"));
    r.desktop_widget(DesktopWidgetType::new(ids::NOW_PLAYING, "card", tk("Now playing"), tk("Shown while a player is active"), "music", || vec![dc::new_card(NOW_PLAYING, Anchor::Center)]).in_menu());

    let media: [(&str, &'static str, &'static str, MediaLayout, bool); 6] = [
        ("vinyl", tk("Record player"), tk("The cover on a turning record"), MediaLayout::Vinyl, true),
        ("device", tk("Pocket player"), tk("A little player with an LCD and buttons"), MediaLayout::Device, false),
        ("shaped", tk("Shaped cover"), tk("The cover cut to a new shape for every track"), MediaLayout::Shaped, false),
        ("side-pill", tk("Side pill"), tk("A tall pill with the track down its side"), MediaLayout::SidePill, false),
        ("halftone", tk("Halftone cover"), tk("The cover as a screen of dots"), MediaLayout::Halftone, false),
        ("lyrics", tk("Lyrics"), tk("Synced lyrics, the current line lit"), MediaLayout::Lyrics, false),
    ];
    for (id, name, description, layout, menu) in media {
        let kind = DesktopWidgetType::new(ids::NOW_PLAYING, id, name, description, "music", move || one(WidgetKind::Media(MediaWidget { layout, ..MediaWidget::default() })));
        r.desktop_widget(if menu { kind.in_menu() } else { kind });
    }

    plugin(r, ids::INFO, tk("Desktop info"), tk("Your machine at a glance, screen time, the next prayer, a search field, your GitHub year, exchange rates, AI usage and your games"));
    let info: [(&str, &'static str, &'static str, &'static str, fn() -> WidgetKind, bool); 11] = [
        ("fetch", tk("Machine card"), tk("System, kernel, processor, graphics, memory, uptime, like a terminal fetch"), "monitor", || WidgetKind::Fetch(FetchWidget::default()), true),
        ("fetch-grid", tk("Machine tiles"), tk("The same as a grid of tiles"), "monitor", || WidgetKind::Fetch(FetchWidget { layout: FetchLayout::Grid, logo: false, colors: false, ..FetchWidget::default() }), false),
        ("screen-time", tk("Screen time"), tk("Today's total, the last days and your top apps (needs the Screen time plugin)"), "timer", || WidgetKind::ScreenTime(ScreenTimeWidget::default()), false),
        ("prayer", tk("Prayer times"), tk("The next prayer and the time to it (needs the Prayer times plugin)"), "crescent", || WidgetKind::Prayer(PrayerWidget::default()), false),
        ("prayer-list", tk("Prayer times list"), tk("Every prayer time today, the next one lit"), "crescent", || WidgetKind::Prayer(PrayerWidget { layout: PrayerLayout::List, ..PrayerWidget::default() }), false),
        ("search", tk("Search field"), tk("A click opens the launcher"), "search", || WidgetKind::Search(SearchWidget::default()), true),
        ("glance", tk("At a glance"), tk("The date, the weather and the one thing worth knowing now"), "sparkles", || WidgetKind::Glance(GlanceWidget::default()), true),
        ("github", tk("GitHub contributions"), tk("Your contribution calendar (fetched from github.com once you switch it on)"), "grid", || WidgetKind::Github(GithubWidget::default()), false),
        ("currency", tk("Exchange rates"), tk("What one unit of your currency buys (fetched twice a day once you switch it on)"), "globe", || WidgetKind::Currency(CurrencyWidget::default()), false),
        ("ai-usage", tk("AI usage"), tk("Claude Code's tokens today and this week, read from its local logs once you switch it on"), "sparkles", || WidgetKind::AiUsage(AiUsageWidget::default()), false),
        ("games", tk("Games"), tk("Your Steam games as covers; a click starts one"), "dice", || WidgetKind::Games(GamesWidget::default()), false),
    ];
    for (id, name, description, icon, make, menu) in info {
        let kind = DesktopWidgetType::new(ids::INFO, id, name, description, icon, move || one(make()));
        r.desktop_widget(if menu { kind.in_menu() } else { kind });
    }

    plugin(r, ids::CALENDAR, tk("Calendar card"), tk("This month on the desktop, with your events"));
    r.desktop_widget(DesktopWidgetType::new(ids::CALENDAR, "card", tk("Calendar"), tk("This month at a glance"), "calendar", || vec![dc::new_card(CALENDAR, Anchor::Center)]).in_menu());

    for (id, layout, name, plate) in [
        ("month", DateLayout::Month, tk("Month"), true),
        ("week", DateLayout::Week, tk("Week strip"), false),
        ("agenda", DateLayout::Agenda, tk("Coming up"), false),
        ("tile", DateLayout::Tile, tk("Date tile"), true),
    ] {
        r.desktop_widget(DesktopWidgetType::new(ids::CALENDAR, id, name, tk("Painted on the desktop, with your events"), "calendar", move || {
            let look = if plate { Look { plate: Plate::Solid, plate_radius: DATE_RADIUS_PX, ..Look::default() } } else { Look::default() };
            one(WidgetKind::Date(DateWidget { layout, look, ..DateWidget::default() }))
        }));
    }

    plugin(r, ids::SKY, tk("Desktop sky"), tk("The sun's arc from sunrise to sunset, tonight's moon and a globe lit where the sun is now, worked out for your location"));
    for (id, layout, name, description) in [
        ("sun-arc", SkyLayout::SunArc, tk("Sun arc"), tk("The sun on its path, sunrise, sunset and what comes next")),
        ("moon", SkyLayout::Moon, tk("Moon phase"), tk("Tonight's moon and how much of it is lit")),
        ("globe", SkyLayout::Globe, tk("Day and night globe"), tk("Day and night where they are right now, your place and the moon")),
    ] {
        let kind = DesktopWidgetType::new(ids::SKY, id, name, description, "sunset", move || one(WidgetKind::Sky(SkyWidget { layout, ..SkyWidget::default() })));
        r.desktop_widget(if layout == SkyLayout::SunArc { kind.in_menu() } else { kind });
    }

    plugin(r, ids::IDENTITY, tk("Desktop shortcuts and you"), tk("A profile card, launch tiles for apps, folders and links, quick toggles, volume and brightness sliders and your theme's palette on the desktop"));
    let identity: [(&str, &'static str, &'static str, &'static str, fn() -> WidgetKind, bool); 7] = [
        ("profile", tk("Profile card"), tk("Your picture, name, user@host, uptime and a status"), "user", || WidgetKind::Profile(ProfileWidget::default()), true),
        ("apps", tk("Shortcuts"), tk("Your dock's apps, or any apps and links, as tiles"), "grid", || WidgetKind::Shortcuts(ShortcutsWidget::default()), true),
        ("folders", tk("Folders"), tk("Your home folders; a click opens one"), "folder", || WidgetKind::Shortcuts(ShortcutsWidget { folders: true, layout: TileLayout::List, colorful: false, ..ShortcutsWidget::default() }), false),
        ("toggles", tk("Quick toggles"), tk("Wi-Fi, Bluetooth, do not disturb and more; shapes that change when on"), "sliders", || WidgetKind::Toggles(TogglesWidget::default()), true),
        ("sliders", tk("Volume and brightness"), tk("Click or scroll to set them"), "sliders", || WidgetKind::Sliders(SlidersWidget::default()), false),
        ("palette", tk("Palette"), tk("Your theme's colours; a click copies one"), "palette", || WidgetKind::Palette(PaletteWidget::default()), false),
        ("social", tk("Link tiles"), tk("Web addresses as colourful tiles"), "globe", || WidgetKind::Shortcuts(ShortcutsWidget { items: vec!["https://github.com".into(), "https://www.openstreetmap.org".into(), "https://en.wikipedia.org".into()], columns: LINK_COLUMNS, ..ShortcutsWidget::default() }), false),
    ];
    for (id, name, description, icon, make, menu) in identity {
        let kind = DesktopWidgetType::new(ids::IDENTITY, id, name, description, icon, move || one(make()));
        r.desktop_widget(if menu { kind.in_menu() } else { kind });
    }

    plugin(r, ids::FUN, tk("Desktop fun"), tk("A little cat, soap bubbles, a photo frame and stickers on the desktop"));
    let fun: [(&str, &'static str, &'static str, &'static str, fn() -> WidgetKind, bool); 5] = [
        ("cat", tk("Desk cat"), tk("Breathes, blinks, sleeps at night, bobs to music, purrs when clicked"), "heart", || WidgetKind::Pet(PetWidget::default()), true),
        ("bubbles", tk("Bubbles"), tk("Soap bubbles drifting up while the desktop shows"), "droplet", || WidgetKind::Bubbles(BubblesWidget::default()), false),
        ("photo", tk("Photo frame"), tk("Pictures from a folder, a new one every so often"), "image", || WidgetKind::Photo(PhotoWidget::default()), true),
        ("polaroid", tk("Polaroid"), tk("A picture in a white print border, a little crooked"), "image", || WidgetKind::Photo(PhotoWidget { width: POLAROID_PX, height: POLAROID_PX, look: Look { chrome: Chrome::Polaroid, shadow: ShadowKind::Soft, ..Look::default() }, ..PhotoWidget::default() }), false),
        ("sticker", tk("Sticker"), tk("Any picture, cut out with a white border"), "image", || WidgetKind::Sticker(StickerWidget::default()), false),
    ];
    for (id, name, description, icon, make, menu) in fun {
        let kind = DesktopWidgetType::new(ids::FUN, id, name, description, icon, move || {
            let mut w = DesktopWidget { kind: make(), ..DesktopWidget::default() };
            if id == "polaroid" {
                w.place.rotation = POLAROID_TILT_DEG;
            }
            vec![w]
        });
        r.desktop_widget(if menu { kind.in_menu() } else { kind });
    }

    plugin(r, ids::WEATHER, tk("Desktop weather"), tk("The weather on the desktop: the temperature, the sky, the place and the days ahead"));
    for (id, layout, name) in [("hero", WeatherLayout::Hero, tk("Weather")), ("compact", WeatherLayout::Compact, tk("Weather line")), ("sentence", WeatherLayout::Sentence, tk("Weather sentence")), ("disc", WeatherLayout::Disc, tk("Weather disc")), ("hourly", WeatherLayout::Hourly, tk("Hourly curve"))] {
        let kind = DesktopWidgetType::new(ids::WEATHER, id, name, tk("Needs the Weather plugin's location"), "cloud-sun", move || one(WidgetKind::Weather(WeatherWidget { layout, ..WeatherWidget::default() })));
        r.desktop_widget(if layout == WeatherLayout::Hero { kind.in_menu() } else { kind });
    }

    plugin(r, ids::WORLD_CLOCK, tk("Desktop world clock"), tk("The time in other cities on the desktop, with how far ahead or behind they are"));
    for (id, horizontal, name) in [("column", false, tk("World clock")), ("row", true, tk("World clock row"))] {
        let kind = DesktopWidgetType::new(ids::WORLD_CLOCK, id, name, tk("Your World clock cities, or a list of its own"), "globe", move || one(WidgetKind::WorldClock(WorldClockWidget { horizontal, ..WorldClockWidget::default() })));
        r.desktop_widget(if horizontal { kind } else { kind.in_menu() });
    }

    plugin(r, ids::PRODUCTIVITY, tk("Desktop productivity"), tk("Your to-dos, a sticky note, the focus timer, a quote and notebook tabs on the desktop; tick tasks and run the timer right there"));
    let productivity: [(&str, &'static str, &'static str, &'static str, fn() -> WidgetKind, bool); 7] = [
        ("todo", tk("To-do list"), tk("Your tasks; click one to tick it"), "list-check", || WidgetKind::Todo(TodoWidget::default()), true),
        ("todo-paper", tk("Ruled to-do list"), tk("Tasks on lined paper with square boxes"), "list-check", || {
            WidgetKind::Todo(TodoWidget { ruled: true, checkbox: PlateShape::Rect, look: Look { plate: Plate::Solid, plate_shape: PlateShape::Deckle, chrome: Chrome::Tape, ..Look::default() }, ..TodoWidget::default() })
        }, false),
        ("note", tk("Sticky note"), tk("A note from Notes on a pastel sheet"), "note", || WidgetKind::Note(NoteWidget::default()), true),
        ("polaroid-note", tk("Taped note"), tk("A note on white paper held up with washi tape"), "note", || {
            WidgetKind::Note(NoteWidget { sheet: NoteSheet::Paper, look: Look { chrome: Chrome::Tape, shadow: ShadowKind::Soft, ..Look::default() }, ..NoteWidget::default() })
        }, false),
        ("pomodoro", tk("Focus timer"), tk("The Pomodoro timer; click to start or pause"), "timer", || WidgetKind::Pomodoro(PomodoroWidget::default()), true),
        ("quote", tk("Quote"), tk("A saying that changes every day, or your own"), "sparkles", || WidgetKind::Quote(QuoteWidget::default()), true),
        ("notebook", tk("Notebook tabs"), tk("Your recent notes as tabs on the screen's edge"), "note", || WidgetKind::Notebook(NotebookWidget::default()), false),
    ];
    for (id, name, description, icon, make, menu) in productivity {
        let kind = DesktopWidgetType::new(ids::PRODUCTIVITY, id, name, description, icon, move || {
            let mut w = DesktopWidget { kind: make(), ..DesktopWidget::default() };
            if matches!(w.kind, WidgetKind::Notebook(_)) {
                (w.place.anchor, w.place.x) = (Anchor::Right, 0.0);
            }
            vec![w]
        });
        r.desktop_widget(if menu { kind.in_menu() } else { kind });
    }

    plugin(r, ids::COUNTDOWN, tk("Countdown"), tk("Days until a date that matters, large on the desktop, in the calendar you choose"));
    r.desktop_widget(DesktopWidgetType::new(ids::COUNTDOWN, "days", tk("Countdown"), tk("Days until (or since) a date"), "timer", || one(WidgetKind::Countdown(CountdownWidget::default()))).in_menu());
}

/// A new clock with `face`, styled as the face is meant to be seen.
pub fn clock_preset(face: ClockFace) -> ClockWidget {
    let mut c = ClockWidget::with_face(face);
    match face {
        ClockFace::Stack => {
            c.look.plate = Plate::Solid;
            c.look.plate_radius = STACK_RADIUS_PX;
            c.look.plate_padding = STACK_PADDING_PX;
        }
        ClockFace::Boxes | ClockFace::Pixel | ClockFace::DotMatrix => c.look.font = FontRole::Mono,
        ClockFace::Hud => {
            c.look.shadow = ShadowKind::Hard;
            c.look.shadow_tone = Tone::Accent;
            c.look.shadow_x = HUD_SHADOW_PX;
            c.look.shadow_y = HUD_SHADOW_PX;
            c.look.shadow_opacity = 1.0;
        }
        _ => {}
    }
    c
}

/// The wavy gauges' ring width, reference px.
const WAVY_RING_PX: f32 = 9.0;
/// Link tiles per row, a new polaroid's size and tilt.
const LINK_COLUMNS: u32 = 3;
const POLAROID_PX: f32 = 260.0;
const POLAROID_TILT_DEG: f32 = -4.0;
/// A painted month's and a date tile's plate corners, reference px.
const DATE_RADIUS_PX: f32 = 28.0;
/// Plate of the stacked card clock, reference px.
const STACK_RADIUS_PX: f32 = 44.0;
const STACK_PADDING_PX: f32 = 30.0;
/// The HUD clock's hard shadow offset.
const HUD_SHADOW_PX: f32 = 6.0;
/// The framed HUD clock: its frame and corner cut.
const FRAME_OUTLINE_PX: f32 = 1.0;
const FRAME_CHAMFER_PX: f32 = 10.0;
const FRAME_PADDING_PX: f32 = 18.0;
/// The cookie analog clock: its lobes and their depth.
const COOKIE_LOBES: u32 = 14;
const COOKIE_DEPTH: f32 = 0.07;

/// Clocks that are a face plus a look, as seen in other setups.
type ClockLook = (&'static str, &'static str, &'static str, fn() -> ClockWidget);
const CLOCK_LOOKS: [ClockLook; 3] = [
    ("cookie", tk("Cookie clock"), tk("A scalloped dial, dots for hours, blade hands and the day in a bubble"), || {
        let mut c = clock_preset(ClockFace::Analog);
        c.analog = AnalogOptions { dial: PlateShape::Cookie, dial_fill: true, marks: DialMarks::Dots, hour_hand: HandStyle::Fill, seconds_style: SecondsStyle::Dot, date_style: DateStyle::Bubble, ..AnalogOptions::default() };
        c.look.plate_lobes = COOKIE_LOBES;
        c.look.plate_depth = COOKIE_DEPTH;
        c
    }),
    ("framed-hud", tk("Framed HUD clock"), tk("Stacked digits in a thin frame with a header"), || {
        let mut c = clock_preset(ClockFace::Stack);
        c.look = mandara_config::desktop_widgets::Look {
            font: FontRole::Mono,
            plate: Plate::Glass,
            plate_shape: PlateShape::Chamfer,
            plate_chamfer: FRAME_CHAMFER_PX,
            plate_padding: FRAME_PADDING_PX,
            outline_width: FRAME_OUTLINE_PX,
            outline_tone: Tone::Accent,
            header: "SYS.CLOCK".into(),
            ..mandara_config::desktop_widgets::Look::default()
        };
        c
    }),
    ("divided", tk("Divided text clock"), tk("The day large, a rule, the weekday and the time beside it"), || {
        let mut c = clock_preset(ClockFace::Template);
        c.face_options.divider = true;
        c.lines = vec![
            TemplateLine { text: "{day}".into(), size: 140.0, weight: 800, ..TemplateLine::default() },
            TemplateLine { text: "{weekday}".into(), size: 40.0, weight: 600, ..TemplateLine::default() },
            TemplateLine { text: "{month} {year}".into(), size: 22.0, weight: 400, opacity: 0.75, ..TemplateLine::default() },
            TemplateLine { text: "{time}".into(), size: 30.0, weight: 300, accent: true, ..TemplateLine::default() },
        ];
        c
    }),
];

fn preset_id(p: Preset) -> &'static str {
    match p {
        Preset::AlexandriaLine => "alexandria-line",
        Preset::VerticalPoem => "vertical-poem",
        Preset::Geometric => "geometric",
    }
}

fn vis_id(s: VisualizerShape) -> &'static str {
    match s {
        VisualizerShape::Bars => "bars",
        VisualizerShape::Mirrored => "mirrored",
        VisualizerShape::Wave => "wave",
        VisualizerShape::Radial => "radial",
        VisualizerShape::Led => "led",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_widget_kind_comes_from_a_registered_plugin() {
        let mut r = PluginRegistry::default();
        register(&mut r);
        for id in ids::ALL {
            assert!(r.get(id).is_some(), "{id} is registered");
            assert!(r.desktop_widgets().iter().any(|w| w.plugin == id), "{id} offers a widget");
        }
        // Every kind a plugin makes names that plugin.
        for t in r.desktop_widgets() {
            for w in (t.make)() {
                assert_eq!(w.kind.plugin(), t.plugin, "{}", t.key());
            }
        }
        let keys: Vec<String> = r.desktop_widgets().iter().map(DesktopWidgetType::key).collect();
        let mut unique = keys.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(keys.len(), unique.len(), "unique keys");
        assert!(r.desktop_widgets().iter().filter(|w| w.menu).count() >= ids::ALL.len());
    }

    #[test]
    fn switching_a_plugin_off_takes_its_kinds_out() {
        let mut r = PluginRegistry::default();
        register(&mut r);
        let mut cfg = mandara_config::PluginsConfig::default();
        let all = r.enabled_desktop_widgets(&cfg).count();
        cfg.disabled.push(ids::WEATHER.into());
        assert!(r.enabled_desktop_widgets(&cfg).count() < all);
        assert!(r.enabled_desktop_widgets(&cfg).all(|w| w.plugin != ids::WEATHER));
    }
}
