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
    ProgressStyle, ProgressWidget, SecondsStyle, ShadowKind, TemplateLine, Tone, Look, Chrome, MediaLayout, MediaWidget, MonitorRow, FetchWidget, FetchLayout, ScreenTimeWidget, PrayerWidget, PrayerLayout, JannahLayout, HadithLayout, QuranLayout, SearchWidget, GlanceWidget, GithubWidget, CurrencyWidget, AiUsageWidget, GamesWidget, ProfileWidget, ShortcutsWidget, TileLayout, TogglesWidget, SlidersWidget, PaletteWidget, PetWidget, BubblesWidget, PhotoWidget, StickerWidget, DateLayout, DateWidget, SkyLayout, SkyWidget, TodoWidget, NoteWidget, NoteSheet, PomodoroWidget, QuoteWidget, NotebookWidget, VisualizerShape, WeatherLayout, WeatherWidget, WidgetKind, WorldClockWidget, plugin_ids as ids,
};
use mandara_i18n::tk;
use mandara_plugins::{DesktopWidgetStyle, DesktopWidgetType, PluginInstance, PluginManifest, PluginRegistry};

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
            DateLayout::Ticket => (tk("Date ticket"), "calendar"),
            DateLayout::Perpetual => (tk("Perpetual calendar"), "calendar"),
            DateLayout::MoonCalendar => (tk("Moon calendar"), "calendar"),
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
        WidgetKind::Quran(q) => match q.layout {
            QuranLayout::Ayah | QuranLayout::Quote => (tk("Ayah of the day"), "book"),
            QuranLayout::Progress => (tk("Khatm progress"), "book"),
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
        ClockFace::Word => tk("Word clock"),
        ClockFace::SplitFlap => tk("Split-flap clock"),
        ClockFace::Nixie => tk("Glass tube clock"),
        ClockFace::Binary => tk("Binary clock"),
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
    // One entry per family; a placed widget switches between the family's
    // styles in Studio. Each family's id is its first style's (and each
    // style's id the kind it used to be), so the old ids still add the same
    // widgets ([`PluginRegistry::make_desktop_widgets`]).
    let face = |face: ClockFace, name: &'static str| DesktopWidgetStyle::new(face.id(), name, move || one(WidgetKind::Clock(clock_preset(face))));
    let look = |id: &str, name: &'static str, make: fn() -> ClockWidget| DesktopWidgetStyle::new(id, name, move || one(WidgetKind::Clock(make())));
    let single = |f: ClockFace| DesktopWidgetType::new(ids::CLOCKS, f.id(), face_label(f), tk("A clock on the desktop"), "clock", move || one(WidgetKind::Clock(clock_preset(f))));
    let family = |name: &'static str, styles: Vec<DesktopWidgetStyle>| {
        let id = styles.first().map(|s| s.id.clone()).unwrap_or_default();
        DesktopWidgetType::family(ids::CLOCKS, &id, name, tk("A clock on the desktop"), "clock", styles)
    };
    // The depth clock behind the wallpaper's subject (its other faces are
    // its own styles in Studio).
    r.desktop_widget(single(ClockFace::Stacked));
    r.desktop_widget(family(tk("Stacked"), vec![face(ClockFace::Superscript, tk("Superscript")), face(ClockFace::Stack, tk("Stacked card")), look("framed-hud", tk("Framed HUD"), framed_hud_clock)]));
    r.desktop_widget(single(ClockFace::Headline));
    r.desktop_widget(single(ClockFace::DayBlock));
    r.desktop_widget(
        family(tk("Analog"), vec![face(ClockFace::Analog, tk("Classic analog")), look("cookie", tk("Cookie"), cookie_clock), face(ClockFace::Orbit, tk("Orbit")), face(ClockFace::Numerals, tk("Hands over numerals"))]).in_menu(),
    );
    r.desktop_widget(
        family(
            tk("Digital"),
            vec![
                face(ClockFace::Minimal, tk("Clock")),
                face(ClockFace::Ring, tk("Ring")),
                face(ClockFace::Shape, tk("Time in a shape")),
                face(ClockFace::DotMatrix, tk("Dot matrix")),
                face(ClockFace::Pixel, tk("Pixel")),
                face(ClockFace::Boxes, tk("Digit boxes")),
                face(ClockFace::Hud, tk("Slanted HUD")),
                face(ClockFace::Word, tk("Word clock")),
                face(ClockFace::SplitFlap, tk("Split-flap clock")),
                face(ClockFace::Nixie, tk("Glass tube clock")),
                face(ClockFace::Binary, tk("Binary clock")),
            ],
        )
        .in_menu(),
    );
    let progress = [
        ("day-ring", tk("Day ring"), Period::Day, ProgressStyle::Ring),
        ("week-wave", tk("Wavy week"), Period::Week, ProgressStyle::WavyRing),
        ("month-bar", tk("Month bar"), Period::Month, ProgressStyle::Bar),
        ("year-dots", tk("Year in dots"), Period::Year, ProgressStyle::Dots),
    ]
    .map(|(id, name, period, style)| DesktopWidgetStyle::new(id, name, move || one(WidgetKind::Progress(ProgressWidget { period, style, ..ProgressWidget::default() }))));
    r.desktop_widget(DesktopWidgetType::family(ids::CLOCKS, "day-ring", tk("Progress"), tk("How much of it has passed"), "gauge", progress.into()).in_menu());
    r.desktop_widget(single(ClockFace::Greeting));
    r.desktop_widget(family(tk("Text"), vec![face(ClockFace::Template, tk("Text lines")), look("divided", tk("Divided text"), divided_clock), face(ClockFace::DayNumeral, tk("Day numeral"))]));

    plugin(r, ids::MONITOR, tk("System monitor"), tk("Live CPU, GPU, memory, temperatures, disk and network on the desktop, as a column or ring gauges"));
    let monitor = |id: &str, name: &'static str, m: MonitorWidget| {
        DesktopWidgetStyle::new(id, name, move || vec![DesktopWidget { kind: WidgetKind::Monitor(m.clone()), place: Placement { anchor: Anchor::TopRight, x: MONITOR_PLACE.0, y: MONITOR_PLACE.1, ..Placement::default() }, ..DesktopWidget::default() }])
    };
    let style = |style: MonitorStyle| MonitorWidget { style, ..MonitorWidget::default() };
    let monitors = vec![
        monitor("column", tk("Column"), style(MonitorStyle::Column)),
        monitor("gauges", tk("Gauges"), style(MonitorStyle::Gauges)),
        monitor("meters", tk("Meters"), style(MonitorStyle::Meters)),
        monitor("bars", tk("Bars"), style(MonitorStyle::Bars)),
        monitor("wavy", tk("Wavy gauges"), MonitorWidget { style: MonitorStyle::Gauges, wavy: true, ring_width: WAVY_RING_PX, rows: vec![MonitorRow::Cpu, MonitorRow::Memory, MonitorRow::Disk], ..MonitorWidget::default() }),
    ];
    r.desktop_widget(DesktopWidgetType::family(ids::MONITOR, "column", tk("System monitor"), tk("Live CPU, GPU, memory, temperatures, disk and network on the desktop, as a column or ring gauges"), "gauge", monitors).in_menu());
    let thermals = monitor("thermals", tk("Thermals"), MonitorWidget { style: MonitorStyle::Thermals, rows: vec![MonitorRow::CpuTemp, MonitorRow::GpuTemp, MonitorRow::Fan], ..MonitorWidget::default() });
    r.desktop_widget(DesktopWidgetType::new(ids::MONITOR, "thermals", tk("Thermals"), tk("Arc gauges for the temperatures and the fan"), "thermometer", move || (thermals.make)()));

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
        ("ticket", DateLayout::Ticket, tk("Date ticket"), true),
        ("perpetual", DateLayout::Perpetual, tk("Perpetual calendar"), false),
        ("moon-calendar", DateLayout::MoonCalendar, tk("Moon calendar"), false),
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
    for (id, layout, name) in [("hero", WeatherLayout::Hero, tk("Weather")), ("compact", WeatherLayout::Compact, tk("Weather line")), ("sentence", WeatherLayout::Sentence, tk("Weather sentence")), ("disc", WeatherLayout::Disc, tk("Weather disc")), ("hourly", WeatherLayout::Hourly, tk("Hourly curve")), ("ribbon", WeatherLayout::Ribbon, tk("Weather ribbon")), ("thermometer", WeatherLayout::Thermometer, tk("Thermometer")), ("humidity-rose", WeatherLayout::HumidityRose, tk("Humidity rose"))] {
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

/// The cookie analog clock: a scalloped dial, dots for hours, blade hands
/// and the day in a bubble.
fn cookie_clock() -> ClockWidget {
    let mut c = clock_preset(ClockFace::Analog);
    c.analog = AnalogOptions { dial: PlateShape::Cookie, dial_fill: true, marks: DialMarks::Dots, hour_hand: HandStyle::Fill, seconds_style: SecondsStyle::Dot, date_style: DateStyle::Bubble, ..AnalogOptions::default() };
    c.look.plate_lobes = COOKIE_LOBES;
    c.look.plate_depth = COOKIE_DEPTH;
    c
}

/// The framed HUD clock: stacked digits in a thin frame with a header.
fn framed_hud_clock() -> ClockWidget {
    let mut c = clock_preset(ClockFace::Stack);
    c.look = Look {
        font: FontRole::Mono,
        plate: Plate::Glass,
        plate_shape: PlateShape::Chamfer,
        plate_chamfer: FRAME_CHAMFER_PX,
        plate_padding: FRAME_PADDING_PX,
        outline_width: FRAME_OUTLINE_PX,
        outline_tone: Tone::Accent,
        header: "SYS.CLOCK".into(),
        ..Look::default()
    };
    c
}

/// The divided text clock: the day large, a rule, the weekday and the
/// time beside it.
fn divided_clock() -> ClockWidget {
    let mut c = clock_preset(ClockFace::Template);
    c.face_options.divider = true;
    c.lines = vec![
        TemplateLine { text: "{day}".into(), size: 140.0, weight: 800, ..TemplateLine::default() },
        TemplateLine { text: "{weekday}".into(), size: 40.0, weight: 600, ..TemplateLine::default() },
        TemplateLine { text: "{month} {year}".into(), size: 22.0, weight: 400, opacity: 0.75, ..TemplateLine::default() },
        TemplateLine { text: "{time}".into(), size: 30.0, weight: 300, accent: true, ..TemplateLine::default() },
    ];
    c
}

// ── Styles ───────────────────────────────────────────────────────────────

/// What every style a widget could have come from shares with it: its
/// kind, a clock's face, whether a monitor shows the thermals.
fn shape(kind: &WidgetKind) -> String {
    match kind {
        WidgetKind::Clock(c) => format!("clock/{}", c.face.id()),
        WidgetKind::Monitor(m) => format!("monitor/{}", m.style == MonitorStyle::Thermals),
        k => serde_json::to_value(k).ok().and_then(|v| v.get("kind").and_then(|x| x.as_str()).map(String::from)).unwrap_or_default(),
    }
}

/// How many of `preset`'s settings `kind` has the same.
fn agreement(preset: &serde_json::Value, kind: &serde_json::Value) -> usize {
    match (preset, kind) {
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => a.iter().map(|(k, v)| b.get(k).map_or(0, |w| agreement(v, w))).sum(),
        (a, b) => usize::from(a == b),
    }
}

/// A placed widget's family and the style it has.
pub struct StyleOf {
    /// The family (a kind with styles).
    pub family: DesktopWidgetType,
    /// Each style's kind, as it adds it.
    pub presets: Vec<WidgetKind>,
    /// The style it matches, or the closest one when it was changed since.
    pub current: usize,
    /// Whether it is that style unchanged.
    pub exact: bool,
}

/// The family `w` belongs to (one of `r`'s kinds with styles) and the
/// style it has: the one it equals, else the closest of those with its
/// face. Nothing for widgets of a kind without styles.
pub fn style_of(w: &DesktopWidget, r: &PluginRegistry) -> Option<StyleOf> {
    let shape_w = shape(&w.kind);
    let json_w = serde_json::to_value(&w.kind).ok()?;
    r.desktop_widgets().iter().filter(|t| t.plugin == w.kind.plugin() && t.styles.len() > 1).find_map(|t| {
        let presets: Vec<WidgetKind> = t.styles.iter().map(|s| (s.make)().into_iter().next().map(|w| w.kind)).collect::<Option<_>>()?;
        let (current, _) = presets
            .iter()
            .enumerate()
            .filter(|(_, k)| shape(k) == shape_w)
            .map(|(i, k)| (i, serde_json::to_value(k).map_or(0, |v| agreement(&v, &json_w))))
            // The first of equally close styles.
            .rev()
            .max_by_key(|(_, score)| *score)?;
        let exact = presets[current] == w.kind;
        Some(StyleOf { family: t.clone(), presets, current, exact })
    })
}

/// Merges, setting by setting: the user's value where the styles `from`
/// and `to` agree and the user changed it, `to`'s otherwise.
fn merge(user: &serde_json::Value, from: &serde_json::Value, to: &serde_json::Value) -> serde_json::Value {
    match to {
        serde_json::Value::Object(t) => serde_json::Value::Object(
            t.iter()
                .map(|(k, tv)| {
                    let v = match (user.get(k), from.get(k)) {
                        (Some(u), Some(f)) => merge(u, f, tv),
                        _ => tv.clone(),
                    };
                    (k.clone(), v)
                })
                .collect(),
        ),
        _ if user != from && to == from => user.clone(),
        _ => to.clone(),
    }
}

/// `kind` (of style `from`) switched to style `to`: what `to` sets (its
/// face, dial, frame, font…) with what the user changed that `to` leaves
/// as `from` had it (colours, digits, rows…). The placement isn't part of
/// a kind: a restyled widget stays where it is.
pub fn restyle(kind: &WidgetKind, from: &WidgetKind, to: &WidgetKind) -> WidgetKind {
    let json = |k: &WidgetKind| serde_json::to_value(k).ok();
    let (Some(user), Some(f), Some(t)) = (json(kind), json(from), json(to)) else { return to.clone() };
    serde_json::from_value(merge(&user, &f, &t)).unwrap_or_else(|_| to.clone())
}

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
    fn twins_are_one_family_and_old_ids_still_add_them() {
        let mut r = PluginRegistry::default();
        register(&mut r);
        let count = |plugin: &str| r.desktop_widgets().iter().filter(|t| t.plugin == plugin).count();
        // Eight clock families and the depth clock; the monitor and thermals.
        assert_eq!(count(ids::CLOCKS), 9);
        assert_eq!(count(ids::MONITOR), 2);
        // Every kind a family replaced still adds what it added.
        let clock = |key: &str| match r.make_desktop_widgets(&format!("{}/{key}", ids::CLOCKS)).as_deref() {
            Some([w]) => w.kind.clone(),
            other => panic!("{key}: {other:?}"),
        };
        for f in ClockFace::ALL.into_iter().filter(|f| f.depth_style().is_none() || *f == ClockFace::Stacked) {
            assert_eq!(clock(f.id()), WidgetKind::Clock(clock_preset(f)), "{}", f.id());
        }
        assert_eq!(clock("cookie"), WidgetKind::Clock(cookie_clock()));
        assert_eq!(clock("framed-hud"), WidgetKind::Clock(framed_hud_clock()));
        assert_eq!(clock("divided"), WidgetKind::Clock(divided_clock()));
        assert!(matches!(clock("year-dots"), WidgetKind::Progress(p) if p.period == Period::Year && p.style == ProgressStyle::Dots));
        for id in ["column", "gauges", "meters", "bars", "thermals", "wavy"] {
            assert!(r.make_desktop_widgets(&format!("{}/{id}", ids::MONITOR)).is_some_and(|w| w.len() == 1), "{id}");
        }
        assert!(r.make_desktop_widgets("desktop-clocks/nope").is_none());
        // Style ids are unique within a plugin, and a family adds its first.
        for t in r.desktop_widgets().iter().filter(|t| !t.styles.is_empty()) {
            assert_eq!(t.id, t.styles[0].id);
            assert_eq!((t.make)(), (t.styles[0].make)());
        }
        let mut style_ids: Vec<String> = r.desktop_widgets().iter().flat_map(|t| t.styles.iter().map(move |s| format!("{}/{}", t.plugin, s.id))).collect();
        let n = style_ids.len();
        style_ids.sort();
        style_ids.dedup();
        assert_eq!(n, style_ids.len());
    }

    #[test]
    fn a_style_switch_keeps_the_place_and_what_the_user_changed() {
        let mut r = PluginRegistry::default();
        register(&mut r);
        let placed = |kind: WidgetKind| DesktopWidget { id: "w".into(), kind, place: Placement { x: 0.3, scale: 1.4, displays: vec!["DP-1".into()], ..Placement::default() }, ..DesktopWidget::default() };
        // Unchanged: exactly its style.
        let s = style_of(&placed(WidgetKind::Clock(cookie_clock())), &r).expect("a family");
        assert_eq!((s.family.id.as_str(), s.family.styles[s.current].id.as_str(), s.exact), ("analog", "cookie", true));
        // Changed: the closest style, not rewritten.
        let mut c = cookie_clock();
        c.look.tone = Tone::Accent;
        c.look.font = FontRole::Display;
        let mut w = placed(WidgetKind::Clock(c));
        let s = style_of(&w, &r).expect("a family");
        assert_eq!((s.family.styles[s.current].id.as_str(), s.exact), ("cookie", false));
        // To the orbit: its face and dial, the user's colour and font kept.
        let orbit = s.family.styles.iter().position(|x| x.id == "orbit").expect("orbit");
        let place = w.place.clone();
        w.kind = restyle(&w.kind, &s.presets[s.current], &s.presets[orbit]);
        assert_eq!(w.place, place);
        let WidgetKind::Clock(c) = &w.kind else { panic!("a clock") };
        assert_eq!((c.face, c.analog.dial, c.look.tone, c.look.font), (ClockFace::Orbit, PlateShape::Circle, Tone::Accent, FontRole::Display));
        // A font the style sets wins over the user's.
        let digital = style_of(&placed(WidgetKind::Clock(ClockWidget { look: Look { font: FontRole::Display, ..Look::default() }, ..clock_preset(ClockFace::Minimal) })), &r).expect("a family");
        let dots = digital.family.styles.iter().position(|x| x.id == "dot-matrix").expect("dot matrix");
        let WidgetKind::Clock(c) = restyle(&placed(WidgetKind::Clock(clock_preset(ClockFace::Minimal))).kind, &digital.presets[digital.current], &digital.presets[dots]) else { panic!("a clock") };
        assert_eq!((c.face, c.look.font), (ClockFace::DotMatrix, FontRole::Mono));
        // Progress: the style's period, the user's line width.
        let p = WidgetKind::Progress(ProgressWidget { period: Period::Hour, thickness: 12.0, ..ProgressWidget::default() });
        let s = style_of(&placed(p.clone()), &r).expect("a family");
        assert_eq!(s.family.styles[s.current].id, "day-ring");
        let month = s.family.styles.iter().position(|x| x.id == "month-bar").expect("month bar");
        let WidgetKind::Progress(p) = restyle(&p, &s.presets[s.current], &s.presets[month]) else { panic!("progress") };
        assert_eq!((p.period, p.style, p.thickness), (Period::Month, ProgressStyle::Bar, 12.0));
        // Monitors: wavy gauges are their own style; thermals have none.
        let wavy = style_of(&placed(r.make_desktop_widgets("desktop-monitor/wavy").expect("wavy").remove(0).kind), &r).expect("a family");
        assert_eq!((wavy.family.styles[wavy.current].id.as_str(), wavy.exact), ("wavy", true));
        assert!(style_of(&placed(r.make_desktop_widgets("desktop-monitor/thermals").expect("thermals").remove(0).kind), &r).is_none());
        // Kinds without styles have no Style row.
        assert!(style_of(&placed(WidgetKind::Clock(clock_preset(ClockFace::Headline))), &r).is_none());
        assert!(style_of(&placed(WidgetKind::Clock(clock_preset(ClockFace::Classic))), &r).is_none());
    }

    /// One widget of every gallery entry that became a style (#23), as
    /// v0.7.0 wrote them: `(widget id, its TOML, family, style)`.
    const OLD_ENTRIES: [(&str, &str, &str, &str); 27] = [
        ("superscript", "kind = \"clock\"\nface = \"superscript\"\nseconds = true", "superscript", "superscript"),
        ("stack", "kind = \"clock\"\nface = \"stack\"\n[desktop.widgets.items.look]\nplate = \"solid\"\nplate_radius = 44.0\nplate_padding = 30.0", "superscript", "stack"),
        ("framed-hud", "kind = \"clock\"\nface = \"stack\"\n[desktop.widgets.items.look]\nfont = \"mono\"\nplate = \"glass\"\nplate_shape = \"chamfer\"\nplate_chamfer = 10.0\nplate_padding = 18.0\noutline_width = 1.0\noutline_tone = \"accent\"\nheader = \"SYS.CLOCK\"", "superscript", "framed-hud"),
        ("analog", "kind = \"clock\"\nface = \"analog\"", "analog", "analog"),
        ("cookie", "kind = \"clock\"\nface = \"analog\"\n[desktop.widgets.items.analog]\ndial = \"cookie\"\ndial_fill = true\nmarks = \"dots\"\nhour_hand = \"fill\"\nseconds_style = \"dot\"\ndate_style = \"bubble\"\n[desktop.widgets.items.look]\nplate_lobes = 14\nplate_depth = 0.07", "analog", "cookie"),
        ("orbit", "kind = \"clock\"\nface = \"orbit\"", "analog", "orbit"),
        ("numerals", "kind = \"clock\"\nface = \"numerals\"", "analog", "numerals"),
        ("minimal", "kind = \"clock\"\nface = \"minimal\"", "minimal", "minimal"),
        ("ring", "kind = \"clock\"\nface = \"ring\"\nring_progress = \"day\"", "minimal", "ring"),
        ("shape", "kind = \"clock\"\nface = \"shape\"", "minimal", "shape"),
        ("dot-matrix", "kind = \"clock\"\nface = \"dot-matrix\"\n[desktop.widgets.items.look]\nfont = \"mono\"", "minimal", "dot-matrix"),
        ("pixel", "kind = \"clock\"\nface = \"pixel\"\n[desktop.widgets.items.look]\nfont = \"mono\"", "minimal", "pixel"),
        ("boxes", "kind = \"clock\"\nface = \"boxes\"\n[desktop.widgets.items.look]\nfont = \"mono\"", "minimal", "boxes"),
        ("hud", "kind = \"clock\"\nface = \"hud\"\n[desktop.widgets.items.look]\nshadow = \"hard\"\nshadow_tone = \"accent\"\nshadow_x = 6.0\nshadow_y = 6.0\nshadow_opacity = 1.0", "minimal", "hud"),
        ("day-ring", "kind = \"progress\"\nperiod = \"day\"\nstyle = \"ring\"", "day-ring", "day-ring"),
        ("week-wave", "kind = \"progress\"\nperiod = \"week\"\nstyle = \"wavy-ring\"", "day-ring", "week-wave"),
        ("month-bar", "kind = \"progress\"\nperiod = \"month\"\nstyle = \"bar\"", "day-ring", "month-bar"),
        ("year-dots", "kind = \"progress\"\nperiod = \"year\"\nstyle = \"dots\"", "day-ring", "year-dots"),
        ("template", "kind = \"clock\"\nface = \"template\"", "template", "template"),
        ("divided", "kind = \"clock\"\nface = \"template\"\n[desktop.widgets.items.face_options]\ndivider = true\n[[desktop.widgets.items.lines]]\ntext = \"{day}\"\nsize = 140.0\nweight = 800\n[[desktop.widgets.items.lines]]\ntext = \"{time}\"\nsize = 30.0\naccent = true", "template", "divided"),
        ("day-numeral", "kind = \"clock\"\nface = \"day-numeral\"", "template", "day-numeral"),
        ("column", "kind = \"monitor\"\nstyle = \"column\"", "column", "column"),
        ("gauges", "kind = \"monitor\"\nstyle = \"gauges\"", "column", "gauges"),
        ("meters", "kind = \"monitor\"\nstyle = \"meters\"\nsegments = 8", "column", "meters"),
        ("bars", "kind = \"monitor\"\nstyle = \"bars\"", "column", "bars"),
        ("wavy", "kind = \"monitor\"\nstyle = \"gauges\"\nwavy = true\nring_width = 9.0\nrows = [\"cpu\", \"memory\", \"disk\"]", "column", "wavy"),
        ("thermals", "kind = \"monitor\"\nstyle = \"thermals\"\nrows = [\"cpu-temp\", \"gpu-temp\", \"fan\"]", "", ""),
    ];

    /// Widgets placed from the old gallery load, survive a save and find
    /// their family and style: nobody's desktop breaks over the folding.
    #[test]
    fn widgets_of_every_old_gallery_entry_load_and_find_their_style() {
        let mut r = PluginRegistry::default();
        register(&mut r);
        // Each widget's own keys, its place, then its sub-tables.
        let text = OLD_ENTRIES.iter().fold(String::from("[desktop.widgets]\nmigrated = true\n"), |mut out, (id, body, _, _)| {
            let (head, tables) = body.split_once("\n[").map_or((*body, String::new()), |(h, t)| (h, format!("\n[{t}")));
            out.push_str(&format!("\n[[desktop.widgets.items]]\nid = \"{id}\"\n{head}\n[desktop.widgets.items.place]\nanchor = \"top-right\"\nx = -0.03\nscale = 1.2\ndisplays = [\"DP-1\"]{tables}\n"));
            out
        });
        let mut cfg: ShellConfig = toml::from_str(&text).expect("an old widget list parses");
        mandara_config::upgrade::upgrade(&mut cfg);
        assert_eq!(cfg.desktop.widgets.items.len(), OLD_ENTRIES.len());
        // Saved and read back, nothing changes.
        let saved = toml::to_string(&cfg).expect("it saves");
        let back: ShellConfig = toml::from_str(&saved).expect("and reads back");
        assert_eq!(back.desktop.widgets, cfg.desktop.widgets);
        for (w, (id, _, family, style)) in cfg.desktop.widgets.items.iter().zip(OLD_ENTRIES) {
            assert_eq!(w.id, id);
            assert_eq!((w.place.anchor, w.place.scale, w.place.displays.as_slice()), (Anchor::TopRight, 1.2, ["DP-1".to_string()].as_slice()), "{id}: its place");
            match style_of(w, &r) {
                Some(s) => {
                    assert_eq!((s.family.id.as_str(), s.family.styles[s.current].id.as_str()), (family, style), "{id}");
                    // Every switch from it works.
                    for to in &s.presets {
                        let _ = restyle(&w.kind, &s.presets[s.current], to);
                    }
                }
                None => assert_eq!(family, "", "{id} has a family"),
            }
        }
        // As the old gallery added them (all of each preset written), they
        // are exactly their style.
        for (id, _, family, style) in OLD_ENTRIES {
            let key = if matches!(id, "column" | "gauges" | "meters" | "bars" | "wavy" | "thermals") { ids::MONITOR } else { ids::CLOCKS };
            let made = r.make_desktop_widgets(&format!("{key}/{id}")).expect("the old id adds it").remove(0);
            let mut cfg = ShellConfig::default();
            cfg.desktop.widgets.items.push(DesktopWidget { id: id.into(), ..made });
            let read: ShellConfig = toml::from_str(&toml::to_string(&cfg).expect("it saves")).expect("it reads back");
            assert_eq!(read.desktop.widgets.items, cfg.desktop.widgets.items, "{id} round-trips");
            match style_of(&read.desktop.widgets.items[0], &r) {
                Some(s) => assert_eq!((s.family.id.as_str(), s.family.styles[s.current].id.as_str(), s.exact), (family, style, true), "{id}"),
                None => assert_eq!(family, "", "{id}"),
            }
        }
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
