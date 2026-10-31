//! Mandara icon set: hand-authored 24-unit stroke icons (2 px, round caps
//! and joins) embedded as SVG assets. GPUI rasterizes them with resvg as
//! anti-aliased masks tinted by the element's text color, so they stay crisp
//! at any scale and follow the theme. Also provides a few procedural icons
//! (ring meters, battery) that encode live values.

use std::borrow::Cow;
use std::f32::consts::{FRAC_PI_2, TAU};

use gpui::{
    AssetSource, Bounds, IntoElement, ParentElement as _, PathBuilder, Pixels, SharedString, Styled, Window, canvas, div,
    point, px, svg,
};
use mandara_core::ToGpui;
use mandara_theme::Color;

const HEAD: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">"#;
const DIM: &str = r#" stroke-opacity="0.28""#;

/// The complete SVG document of a bundled icon (black strokes; tint by
/// coverage), for painting outside GPUI.
pub fn svg_source(name: &str) -> Option<String> {
    icon_body(name).map(|body| format!("{HEAD}{body}</svg>"))
}

/// `(name, body)` pairs; bodies are wrapped in [`HEAD`] when served.
fn icon_body(name: &str) -> Option<Cow<'static, str>> {
    let wifi = |tier: u8| -> String {
        let arc = |d: &str, lit: bool| format!(r#"<path d="{d}"{}/>"#, if lit { "" } else { DIM });
        format!(
            "{}{}{}<circle cx=\"12\" cy=\"19.2\" r=\"1.3\" fill=\"black\" stroke=\"none\"/>",
            arc("M2.5 8.8a14.5 14.5 0 0 1 19 0", tier >= 4),
            arc("M5.6 12.3a9.8 9.8 0 0 1 12.8 0", tier >= 3),
            arc("M8.8 15.8a5 5 0 0 1 6.4 0", tier >= 2),
        )
    };
    Some(match name {
        "wifi-0" => wifi(1).into(),
        "wifi-1" => wifi(2).into(),
        "wifi-2" => wifi(2).into(),
        "wifi-3" => wifi(3).into(),
        "wifi-4" => wifi(4).into(),
        "wifi-off" => format!("{}<path d=\"M3 3l18 18\"/>", wifi(0)).into(),
        "ethernet" => r#"<rect x="9" y="2.5" width="6" height="6" rx="1.6"/><rect x="2.5" y="15.5" width="6" height="6" rx="1.6"/><rect x="15.5" y="15.5" width="6" height="6" rx="1.6"/><path d="M5.5 15.5v-3.5h13v3.5M12 8.5v3.5"/>"#.into(),
        "bluetooth" => r#"<path d="M7 7l10 10-5 5V2l5 5L7 17"/>"#.into(),
        "bluetooth-off" => r#"<path d="M7 7l10 10-5 5V2l5 5L7 17" stroke-opacity="0.35"/><path d="M3 3l18 18"/>"#.into(),
        "volume-high" => r#"<path d="M11 5L6.5 9H3.5v6h3l4.5 4z"/><path d="M15.5 8.6a4.8 4.8 0 0 1 0 6.8"/><path d="M18.6 5.6a9 9 0 0 1 0 12.8"/>"#.into(),
        "volume-low" => r#"<path d="M11 5L6.5 9H3.5v6h3l4.5 4z"/><path d="M15.5 8.6a4.8 4.8 0 0 1 0 6.8"/>"#.into(),
        "volume-mute" => r#"<path d="M11 5L6.5 9H3.5v6h3l4.5 4z"/><path d="M16 9.5l5 5M21 9.5l-5 5"/>"#.into(),
        // Global mute: the speaker struck through.
        "volume-off" => r#"<path d="M11 5L6.5 9H3.5v6h3l4.5 4z"/><path d="M15.5 8.6a4.8 4.8 0 0 1 0 6.8"/><path d="M3 3l18 18"/>"#.into(),
        "mic" => r#"<rect x="9" y="2.5" width="6" height="11.5" rx="3"/><path d="M5.5 10.5a6.5 6.5 0 0 0 13 0M12 17v4.5"/>"#.into(),
        "mic-off" => r#"<rect x="9" y="2.5" width="6" height="11.5" rx="3" stroke-opacity="0.35"/><path d="M5.5 10.5a6.5 6.5 0 0 0 13 0M12 17v4.5" stroke-opacity="0.35"/><path d="M3 3l18 18"/>"#.into(),
        "power" => r#"<path d="M12 2.5v9"/><path d="M18.4 6.6a9 9 0 1 1-12.8 0"/>"#.into(),
        "restart" => r#"<path d="M3.5 12a8.5 8.5 0 1 0 2.6-6.1L3.5 8.5"/><path d="M3.5 3.5v5h5"/>"#.into(),
        "logout" => r#"<path d="M9.5 20.5h-4a2 2 0 0 1-2-2v-13a2 2 0 0 1 2-2h4"/><path d="M16 16.5l4.5-4.5L16 7.5"/><path d="M20.5 12H9.5"/>"#.into(),
        "lock" => r#"<rect x="4.5" y="10.5" width="15" height="10.5" rx="2.8"/><path d="M8 10.5V7a4 4 0 0 1 8 0v3.5"/>"#.into(),
        "moon" => r#"<path d="M20.5 14.2A8.5 8.5 0 0 1 9.8 3.5a8.5 8.5 0 1 0 10.7 10.7z"/>"#.into(),
        "sun" => r#"<circle cx="12" cy="12" r="4"/><path d="M12 2.5v2M12 19.5v2M5.3 5.3l1.4 1.4M17.3 17.3l1.4 1.4M2.5 12h2M19.5 12h2M5.3 18.7l1.4-1.4M17.3 6.7l1.4-1.4"/>"#.into(),
        "hibernate" => r#"<path d="M12 3v11"/><path d="M7.5 9.5L12 14l4.5-4.5"/><path d="M4.5 20.5h15"/>"#.into(),
        "play" => r#"<path d="M7.5 4.6v14.8a1 1 0 0 0 1.5.86l11.6-7.4a1 1 0 0 0 0-1.72L9 3.74a1 1 0 0 0-1.5.86z" fill="black"/>"#.into(),
        "pause" => r#"<rect x="6" y="4.5" width="4" height="15" rx="1.4" fill="black" stroke="none"/><rect x="14" y="4.5" width="4" height="15" rx="1.4" fill="black" stroke="none"/>"#.into(),
        "next" => r#"<path d="M4.5 6v12a1 1 0 0 0 1.55.83l8.6-6a1 1 0 0 0 0-1.66l-8.6-6A1 1 0 0 0 4.5 6z" fill="black"/><path d="M19 5v14"/>"#.into(),
        "previous" => r#"<path d="M19.5 6v12a1 1 0 0 1-1.55.83l-8.6-6a1 1 0 0 1 0-1.66l8.6-6A1 1 0 0 1 19.5 6z" fill="black"/><path d="M5 5v14"/>"#.into(),
        "music" => r#"<path d="M9 18V5.5l11-2v12.5"/><circle cx="6.5" cy="18" r="2.5"/><circle cx="17.5" cy="16" r="2.5"/>"#.into(),
        "cpu" => r#"<rect x="5" y="5" width="14" height="14" rx="3"/><rect x="9.25" y="9.25" width="5.5" height="5.5" rx="1.2"/><path d="M9 2v3M15 2v3M9 19v3M15 19v3M2 9h3M2 15h3M19 9h3M19 15h3"/>"#.into(),
        "memory" => r#"<rect x="2.5" y="6.5" width="19" height="11" rx="2.5"/><path d="M7 10.5v3M11 10.5v3M15 10.5v3M7 17.5v2M12 17.5v2M17 17.5v2"/>"#.into(),
        "disk" => r#"<ellipse cx="12" cy="5.5" rx="8" ry="3"/><path d="M4 5.5v13c0 1.66 3.6 3 8 3s8-1.34 8-3v-13"/><path d="M4 12c0 1.66 3.6 3 8 3s8-1.34 8-3"/>"#.into(),
        "crescent" => r#"<path d="M14.5 3a9 9 0 1 0 6.6 15.1A7.4 7.4 0 1 1 14.5 3z"/><path d="M18 5.6l.55 1.25 1.25.55-1.25.55L18 9.2l-.55-1.25-1.25-.55 1.25-.55z" fill="black" stroke-width="1"/>"#.into(),
        "calendar" => r#"<rect x="3.5" y="5" width="17" height="16" rx="3"/><path d="M3.5 10h17M8 3v4M16 3v4"/>"#.into(),
        "grid" => r#"<rect x="3.5" y="3.5" width="7" height="7" rx="2.2" fill="black" stroke="none"/><rect x="13.5" y="3.5" width="7" height="7" rx="2.2" fill="black" stroke="none" fill-opacity="0.45"/><rect x="3.5" y="13.5" width="7" height="7" rx="2.2" fill="black" stroke="none" fill-opacity="0.45"/><rect x="13.5" y="13.5" width="7" height="7" rx="2.2" fill="black" stroke="none" fill-opacity="0.45"/>"#.into(),
        "search" => r#"<circle cx="11" cy="11" r="6.5"/><path d="M20 20l-4.2-4.2"/>"#.into(),
        "bell" => r#"<path d="M6 8.5a6 6 0 1 1 12 0c0 6.5 2.5 8.5 2.5 8.5h-17S6 15 6 8.5"/><path d="M10.2 20.5a2 2 0 0 0 3.6 0"/>"#.into(),
        "coffee" => r#"<path d="M17 8.5h1.2a3.6 3.6 0 0 1 0 7.2H17"/><path d="M3.5 8.5H17v8a4 4 0 0 1-4 4H7.5a4 4 0 0 1-4-4z"/><path d="M7 2.5v2.5M10.5 2.5v2.5M14 2.5v2.5"/>"#.into(),
        "window" => r#"<rect x="3" y="4" width="18" height="16" rx="3"/><path d="M3 9h18"/><circle cx="6.5" cy="6.5" r=".6" fill="black"/>"#.into(),
        "sidebar" => r#"<rect x="3" y="4" width="18" height="16" rx="3"/><path d="M9.5 4v16"/>"#.into(),
        "headphones" => r#"<path d="M3.5 18v-6a8.5 8.5 0 0 1 17 0v6"/><path d="M20.5 19a2 2 0 0 1-2 2h-1a2 2 0 0 1-2-2v-3a2 2 0 0 1 2-2h3zM3.5 19a2 2 0 0 0 2 2h1a2 2 0 0 0 2-2v-3a2 2 0 0 0-2-2h-3z"/>"#.into(),
        "check" => r#"<path d="M5 12.5l4.5 4.5L19 7.5"/>"#.into(),
        "plus" => r#"<path d="M12 5v14M5 12h14"/>"#.into(),
        "minus" => r#"<path d="M5 12h14"/>"#.into(),
        "keyboard" => r#"<rect x="2.5" y="6" width="19" height="12" rx="3"/><path d="M6.5 10h1M10.5 10h1M14.5 10h1M18 10h-.5M7.5 14h9"/>"#.into(),
        "arrow" => r#"<path d="M5 19L19 5M10 5h9v9"/>"#.into(),
        "undo" => r#"<path d="M9 14L4 9l5-5"/><path d="M4 9h10.5a5.5 5.5 0 0 1 0 11H11"/>"#.into(),
        "pointer" => r#"<path d="M5 3.5l13 7.2-5.8 1.6-2.5 5.6z"/>"#.into(),
        "maximize" => r#"<rect x="5" y="5" width="14" height="14" rx="3"/>"#.into(),
        // A master–stack tiling layout (Studio → Compositor).
        "tiles" => r#"<rect x="3.5" y="4.5" width="17" height="15" rx="2.5"/><path d="M11 4.5v15M11 12h9.5"/>"#.into(),
        "image" => r#"<rect x="3.5" y="4.5" width="17" height="15" rx="3"/><circle cx="9" cy="10" r="1.6"/><path d="M20.5 15.5l-4.5-4.5-9 8.5"/>"#.into(),
        "cloud" => r#"<path d="M7 18.5h10a4 4 0 0 0 .6-7.95A5.5 5.5 0 0 0 7 10.1a4.2 4.2 0 0 0 0 8.4z"/>"#.into(),
        "cloud-sun" => r#"<path d="M8 2.8v1.4M2.8 8h1.4M4.3 4.3l1 1M11.7 4.3l-1 1"/><path d="M5.4 10.6A3.4 3.4 0 0 1 11.2 6.8"/><path d="M9 20h8.5a3.5 3.5 0 0 0 .5-6.96A4.8 4.8 0 0 0 9 12.6a3.7 3.7 0 0 0 0 7.4z"/>"#.into(),
        "cloud-moon" => r#"<path d="M11.4 3.2a3.6 3.6 0 0 0 3.9 5.2A4.3 4.3 0 1 1 11.4 3.2z"/><path d="M8.5 20.5H17a3.5 3.5 0 0 0 .5-6.96A4.8 4.8 0 0 0 8.5 13.1a3.7 3.7 0 0 0 0 7.4z"/>"#.into(),
        "rain" => r#"<path d="M7 15h10a3.8 3.8 0 0 0 .5-7.56A5.2 5.2 0 0 0 7 7a4 4 0 0 0 0 8z"/><path d="M8.5 18l-1 2.8M12.5 18l-1 2.8M16.5 18l-1 2.8"/>"#.into(),
        "snow" => r#"<path d="M7 15h10a3.8 3.8 0 0 0 .5-7.56A5.2 5.2 0 0 0 7 7a4 4 0 0 0 0 8z"/><circle cx="8.5" cy="19" r="1" fill="black" stroke="none"/><circle cx="12" cy="20.8" r="1" fill="black" stroke="none"/><circle cx="15.5" cy="19" r="1" fill="black" stroke="none"/>"#.into(),
        "fog" => r#"<path d="M4.5 8h11M7.5 12h12M4.5 16h11M9 20h7"/>"#.into(),
        "bolt" => r#"<path d="M7 14h10a3.8 3.8 0 0 0 .5-7.56A5.2 5.2 0 0 0 7 6a4 4 0 0 0 0 8z"/><path d="M12.8 15l-2.3 3.6h3.2l-2.3 3.6"/>"#.into(),
        "droplet" => r#"<path d="M12 3.5c3.2 4 5.5 7.2 5.5 10.2a5.5 5.5 0 0 1-11 0c0-3 2.3-6.2 5.5-10.2z"/>"#.into(),
        "wind" => r#"<path d="M3.5 9h11a2.8 2.8 0 1 0-2.8-2.8M3.5 13.5h15a2.8 2.8 0 1 1-2.8 2.8M3.5 18h7"/>"#.into(),
        "sunset" => r#"<path d="M3.5 18h17M7 18a5 5 0 0 1 10 0M12 4v5M9.5 6.5L12 9l2.5-2.5M5 11.5l1.2 1.2M19 11.5l-1.2 1.2"/>"#.into(),
        "thermometer" => r#"<path d="M10 14.2V5.5a2 2 0 0 1 4 0v8.7a4 4 0 1 1-4 0z"/><path d="M12 9.5v6.5"/>"#.into(),
        "note" => r#"<path d="M14.5 3H7a2.5 2.5 0 0 0-2.5 2.5v13A2.5 2.5 0 0 0 7 21h10a2.5 2.5 0 0 0 2.5-2.5V8z"/><path d="M14.5 3v5h5M8.5 13h7M8.5 16.5h4.5"/>"#.into(),
        "list-check" => r#"<path d="M3.5 6.5l1.8 1.8 3.2-3.3M3.5 15.5l1.8 1.8 3.2-3.3M12 7h8.5M12 16h8.5"/>"#.into(),
        "circle" => r#"<circle cx="12" cy="12" r="8"/>"#.into(),
        "skip" => r#"<path d="M5.5 5.5l9 6.5-9 6.5zM18.5 5.5v13"/>"#.into(),
        "chevron-right" => r#"<path d="M9 6l6 6-6 6"/>"#.into(),
        // Mirror of chevron-right for right-to-left layouts (`bidi::dir_icon`).
        "chevron-left" => r#"<path d="M15 6l-6 6 6 6"/>"#.into(),
        "refresh" => r#"<path d="M20.5 12a8.5 8.5 0 0 1-14.9 5.6M3.5 12A8.5 8.5 0 0 1 18.4 6.4"/><path d="M18.5 2.5v4h-4M5.5 21.5v-4h4"/>"#.into(),
        "lock-open" => r#"<rect x="4.5" y="10.5" width="15" height="10.5" rx="2.8"/><path d="M8 10.5V7a4 4 0 0 1 7.7-1.5"/>"#.into(),
        "camera" => r#"<path d="M3.5 8.5a2 2 0 0 1 2-2h2.2l1.6-2.5h5.4l1.6 2.5h2.2a2 2 0 0 1 2 2v9.5a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z"/><circle cx="12" cy="13" r="3.5"/>"#.into(),
        "globe" => r#"<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3c2.5 2.6 3.8 5.6 3.8 9s-1.3 6.4-3.8 9c-2.5-2.6-3.8-5.6-3.8-9S9.5 5.6 12 3z"/>"#.into(),
        "sliders" => r#"<path d="M4 7h10M18 7h2M4 17h4M12 17h8"/><circle cx="16" cy="7" r="2"/><circle cx="10" cy="17" r="2"/>"#.into(),
        "crosshair" => r#"<circle cx="12" cy="12" r="8"/><circle cx="12" cy="12" r="2.5"/><path d="M12 2v4M12 18v4M2 12h4M18 12h4"/>"#.into(),
        "text-scan" => r#"<path d="M3 8V5.5A2.5 2.5 0 0 1 5.5 3H8M16 3h2.5A2.5 2.5 0 0 1 21 5.5V8M21 16v2.5a2.5 2.5 0 0 1-2.5 2.5H16M8 21H5.5A2.5 2.5 0 0 1 3 18.5V16"/><path d="M8 8.5h8M12 8.5v7.5"/>"#.into(),
        "clipboard" => r#"<rect x="5" y="4.5" width="14" height="17" rx="2.5"/><rect x="8.5" y="2.5" width="7" height="4" rx="1.5"/><path d="M9 11.5h6M9 15.5h4"/>"#.into(),
        "crop" => r#"<path d="M6.5 2.5v14a1 1 0 0 0 1 1h14"/><path d="M2.5 6.5h14a1 1 0 0 1 1 1v14"/>"#.into(),
        "monitor" => r#"<rect x="2.5" y="3.5" width="19" height="13" rx="2.5"/><path d="M8.5 20.5h7M12 16.5v4"/>"#.into(),
        "trash" => r#"<path d="M4 7h16"/><path d="M9.5 7V4.5h5V7"/><path d="M6 7l1 13a1.5 1.5 0 0 0 1.5 1.4h7A1.5 1.5 0 0 0 17 20l1-13"/>"#.into(),
        "timer" => r#"<circle cx="12" cy="13.5" r="7.5"/><path d="M12 9.5v4l2.5 2M9.5 2.5h5"/>"#.into(),
        "pin" => r#"<path d="M9 3.5h6l-1 6 3.5 3.5H6.5L10 9.5z"/><path d="M12 13v7.5"/>"#.into(),
        "pin-off" => r#"<path d="M9 3.5h6l-1 6 3.5 3.5H6.5L10 9.5z" stroke-opacity="0.4"/><path d="M12 13v7.5" stroke-opacity="0.4"/><path d="M3 3l18 18"/>"#.into(),
        "close" => r#"<path d="M6 6l12 12M18 6L6 18"/>"#.into(),
        "user" => r#"<circle cx="12" cy="8" r="4"/><path d="M4.5 20.5a7.5 7.5 0 0 1 15 0"/>"#.into(),
        "user-plus" => r#"<circle cx="10" cy="8" r="4"/><path d="M3 20.5a7 7 0 0 1 14 0"/><path d="M19 7.5v6M16 10.5h6"/>"#.into(),
        "key" => r#"<circle cx="8" cy="15" r="4.5"/><path d="M11.2 11.8L20 3M16.5 6.5l2.5 2.5M14 9l2 2"/>"#.into(),
        // System plugins (drives, containers, services, power, updates).
        "eject" => r#"<path d="M12 5l7 8H5z"/><path d="M5 18.5h14"/>"#.into(),
        "usb" => r#"<path d="M12 3v13.5"/><path d="M9.5 5.5L12 3l2.5 2.5"/><path d="M12 12.5L7.5 10V7.5"/><path d="M12 14.5l4.5-2.5V9.5"/><circle cx="7.5" cy="6.5" r="1"/><rect x="15.5" y="7.5" width="2" height="2" rx=".4"/><circle cx="12" cy="18.5" r="2"/>"#.into(),
        "box" => r#"<path d="M12 2.8l8 4.4v9.6l-8 4.4-8-4.4V7.2z"/><path d="M4 7.2l8 4.4 8-4.4"/><path d="M12 11.6v9.6"/>"#.into(),
        "package" => r#"<path d="M12 2.8l8 4.4v9.6l-8 4.4-8-4.4V7.2z"/><path d="M4 7.2l8 4.4 8-4.4"/><path d="M12 11.6v9.6"/><path d="M8 5l8 4.4"/>"#.into(),
        "gauge" => r#"<path d="M4.2 17.5a9 9 0 1 1 15.6 0"/><path d="M12 13.5l4-4.5"/><circle cx="12" cy="13.5" r="1.4"/>"#.into(),
        "leaf" => r#"<path d="M20 4C10 4 5 9 5 16v3h3c7 0 12-5 12-15z"/><path d="M5 19c3.5-4.5 7-7.5 10.5-9.5"/>"#.into(),
        "download" => r#"<path d="M12 4v11"/><path d="M7.5 10.5L12 15l4.5-4.5"/><path d="M5 19.5h14"/>"#.into(),
        "server" => r#"<rect x="3.5" y="4" width="17" height="7" rx="2"/><rect x="3.5" y="13" width="17" height="7" rx="2"/><path d="M7.5 7.5h.01M7.5 16.5h.01"/>"#.into(),
        "terminal" => r#"<rect x="3" y="4.5" width="18" height="15" rx="3"/><path d="M7.5 10l3 2.5-3 2.5"/><path d="M13 15.5h4"/>"#.into(),
        "battery" => r#"<rect x="2.5" y="7" width="16.5" height="10" rx="2.5"/><path d="M21.5 10.5v3"/>"#.into(),
        "plug" => r#"<path d="M9 3v4.5M15 3v4.5"/><path d="M6.5 7.5h11v3a5.5 5.5 0 0 1-11 0z"/><path d="M12 16v5"/>"#.into(),
        "external" => r#"<path d="M14 4h6v6"/><path d="M20 4l-9 9"/><path d="M18 14v4.5a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 4 18.5v-11A1.5 1.5 0 0 1 5.5 6H10"/>"#.into(),
        "folder" => r#"<path d="M3.5 7a2 2 0 0 1 2-2h4l2 2.5h7a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z"/>"#.into(),
        "stop" => r#"<rect x="6" y="6" width="12" height="12" rx="2.5"/>"#.into(),
        "heart" => r#"<path d="M12 20s-7.5-4.6-7.5-10A4.3 4.3 0 0 1 12 7.4 4.3 4.3 0 0 1 19.5 10c0 5.4-7.5 10-7.5 10z"/>"#.into(),
        "alert" => r#"<path d="M10.3 4.5a2 2 0 0 1 3.4 0l7.6 13a2 2 0 0 1-1.7 3H4.4a2 2 0 0 1-1.7-3z"/><path d="M12 10v4"/><path d="M12 17.2h.01"/>"#.into(),
        "zap" => r#"<path d="M13.5 2.5L5 13.5h6.5l-1 8 8.5-11h-6.5z"/>"#.into(),
        "eyedropper" => r#"<path d="M14.6 5.4l1.9-1.9a2.4 2.4 0 0 1 3.4 3.4l-1.9 1.9"/><path d="M12.4 6.6l5 5"/><path d="M15.9 10.1l-8.2 8.2a2 2 0 0 1-1 .5l-2.2.4.4-2.2a2 2 0 0 1 .5-1l8.2-8.2"/>"#.into(),
        "waves" => r#"<path d="M2.5 7.5c1.6 0 1.6-1.5 3.2-1.5s1.6 1.5 3.2 1.5 1.6-1.5 3.1-1.5 1.6 1.5 3.2 1.5 1.6-1.5 3.2-1.5 1.6 1.5 3.1 1.5"/><path d="M2.5 12.5c1.6 0 1.6-1.5 3.2-1.5s1.6 1.5 3.2 1.5 1.6-1.5 3.1-1.5 1.6 1.5 3.2 1.5 1.6-1.5 3.2-1.5 1.6 1.5 3.1 1.5"/><path d="M2.5 17.5c1.6 0 1.6-1.5 3.2-1.5s1.6 1.5 3.2 1.5 1.6-1.5 3.1-1.5 1.6 1.5 3.2 1.5 1.6-1.5 3.2-1.5 1.6 1.5 3.1 1.5"/>"#.into(),
        "arrow-down" => r#"<path d="M12 4.5v15M6.5 14l5.5 5.5 5.5-5.5"/>"#.into(),
        "arrow-up" => r#"<path d="M12 19.5v-15M6.5 10L12 4.5 17.5 10"/>"#.into(),
        "shield" => r#"<path d="M12 2.8l7.5 3v5.6c0 4.6-3.2 8.4-7.5 9.8-4.3-1.4-7.5-5.2-7.5-9.8V5.8z"/><path d="M9 12l2.2 2.2L15.5 10"/>"#.into(),
        "copy" => r#"<rect x="8.5" y="8.5" width="12" height="12" rx="2.5"/><path d="M15.5 8.5V6a2.5 2.5 0 0 0-2.5-2.5H6A2.5 2.5 0 0 0 3.5 6v7A2.5 2.5 0 0 0 6 15.5h2.5"/>"#.into(),
        "clock" => r#"<circle cx="12" cy="12" r="8.5"/><path d="M12 7.5V12l3 2"/>"#.into(),
        "flame" => r#"<path d="M12 21c-3.6 0-6.5-2.6-6.5-6.2 0-3.4 2.5-5.3 3.6-8.3.3 1.9 1.2 3 2.4 3.6.1-2.8 1.3-5.3 3.4-7.1-.2 2.9 1 4.9 2.3 6.6 1 1.4 1.3 2.8 1.3 4.6 0 3.4-2.9 6.4-6.5 6.4z"/>"#.into(),
        "eye" => r#"<path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z"/><circle cx="12" cy="12" r="3"/>"#.into(),
        "eye-off" => r#"<path d="M9.9 5.8A9.6 9.6 0 0 1 12 5.5c6 0 9.5 6.5 9.5 6.5a17 17 0 0 1-2.6 3.4M6.6 6.6C4 8.3 2.5 12 2.5 12S6 18.5 12 18.5a9 9 0 0 0 5.4-1.9"/><path d="M9.9 9.9a3 3 0 0 0 4.2 4.2"/><path d="M3.5 3.5l17 17"/>"#.into(),
        "pencil" => r#"<path d="M15.5 4.5l4 4L8.5 19.5l-5 1 1-5z"/><path d="M13.5 6.5l4 4"/>"#.into(),
        // A paint brush: making art (the wallpaper generator).
        "brush" => r#"<path d="M20 4l-8.2 8.2"/><path d="M15.6 7.1l1.3 1.3"/><path d="M11.8 12.2c-2.1-1.3-4.9-.1-5.3 2.6-.3 1.9-1.2 3-2.7 3.6 2.8 1.6 6.5 1.1 8.1-1.2 1-1.4 1-3.4-.1-5z"/>"#.into(),
        // A die: something chosen at random.
        "dice" => r#"<rect x="4" y="4" width="16" height="16" rx="3.5"/><circle cx="8.8" cy="8.8" r="1.25" fill="black" stroke="none"/><circle cx="15.2" cy="8.8" r="1.25" fill="black" stroke="none"/><circle cx="12" cy="12" r="1.25" fill="black" stroke="none"/><circle cx="8.8" cy="15.2" r="1.25" fill="black" stroke="none"/><circle cx="15.2" cy="15.2" r="1.25" fill="black" stroke="none"/>"#.into(),
        // A page with lines and a version tag: release notes.
        "release-notes" => r#"<path d="M14.5 3H7a2.5 2.5 0 0 0-2.5 2.5v13A2.5 2.5 0 0 0 7 21h10a2.5 2.5 0 0 0 2.5-2.5V8z"/><path d="M14.5 3v5h5"/><path d="M8.5 12.5h7M8.5 16h4"/><path d="M8.5 7.5h2.5"/>"#.into(),
        "sparkles" => r#"<path d="M10.5 3.5l1.6 4.4a3 3 0 0 0 1.8 1.8l4.4 1.6-4.4 1.6a3 3 0 0 0-1.8 1.8l-1.6 4.4-1.6-4.4a3 3 0 0 0-1.8-1.8L2.7 11.3l4.4-1.6a3 3 0 0 0 1.8-1.8z"/><path d="M18.5 2.5v4M16.5 4.5h4M19 16.5v3M17.5 18h3"/>"#.into(),
        _ => return None,
    })
}

/// Serves `icons/<name>.svg` from the embedded set.
pub struct MandaraAssets;

impl AssetSource for MandaraAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        let Some(name) = path.strip_prefix("icons/").and_then(|p| p.strip_suffix(".svg")) else {
            return Ok(None);
        };
        Ok(icon_body(name).map(|body| Cow::Owned(format!("{HEAD}{body}</svg>").into_bytes())))
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        if path.trim_end_matches('/') == "icons" {
            Ok(ICON_NAMES.iter().map(|n| SharedString::from(format!("icons/{n}.svg"))).collect())
        } else {
            Ok(Vec::new())
        }
    }
}

/// Every icon name in the set (also used by tests).
pub const ICON_NAMES: &[&str] = &[
    "wifi-0", "wifi-1", "wifi-2", "wifi-3", "wifi-4", "wifi-off", "ethernet", "bluetooth", "bluetooth-off",
    "volume-high", "volume-low", "volume-mute", "volume-off", "mic", "mic-off", "power", "restart", "logout", "lock", "lock-open",
    "moon", "sun", "hibernate", "play", "pause", "next", "previous", "music", "cpu", "memory", "disk", "crescent",
    "calendar", "grid", "search", "bell", "coffee", "window", "sidebar", "headphones", "check", "chevron-right", "chevron-left",
    "refresh", "camera", "clipboard", "crop", "monitor", "trash", "timer", "pin", "pin-off", "close", "plus", "note",
    "list-check", "circle", "skip", "cloud", "cloud-sun", "cloud-moon", "rain", "snow", "fog", "bolt", "droplet", "wind",
    "sunset", "thermometer", "image", "minus", "maximize", "arrow", "undo", "pointer", "keyboard", "text-scan", "crosshair", "sliders", "globe",
    "eject", "usb", "box", "package", "gauge", "leaf", "download", "server", "terminal", "battery", "plug", "external", "folder",
    "stop", "heart", "alert", "zap",
    "eyedropper", "waves", "arrow-down", "arrow-up", "shield", "copy", "clock", "flame",
    "user", "user-plus", "key", "tiles", "eye", "eye-off", "pencil", "sparkles", "brush", "dice", "release-notes",
];

/// An icon element tinted with `color`.
pub fn icon(name: &'static str, size: f32, color: Color) -> impl IntoElement {
    svg().path(format!("icons/{name}.svg")).size(px(size)).flex_none().text_color(color.hsla())
}

fn circle_points(center: (f32, f32), r: f32, from: f32, sweep: f32) -> impl Iterator<Item = (f32, f32)> {
    // ~4° per segment keeps arcs smooth at bar sizes.
    let steps = ((sweep.abs() / TAU) * 90.0).ceil().max(2.0) as usize;
    (0..=steps).map(move |i| {
        let a = from + sweep * i as f32 / steps as f32;
        (center.0 + r * a.cos(), center.1 + r * a.sin())
    })
}

pub(crate) fn stroke_arc(window: &mut Window, bounds: Bounds<Pixels>, width: f32, from: f32, sweep: f32, color: Color) {
    let size = f32::from(bounds.size.width).min(f32::from(bounds.size.height));
    let r = (size - width) / 2.0;
    let c = (f32::from(bounds.origin.x) + size / 2.0, f32::from(bounds.origin.y) + size / 2.0);
    let mut builder = PathBuilder::stroke(px(width));
    for (i, (x, y)) in circle_points(c, r, from, sweep).enumerate() {
        if i == 0 {
            builder.move_to(point(px(x), px(y)));
        } else {
            builder.line_to(point(px(x), px(y)));
        }
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color.hsla());
    }
}

/// A circular progress ring: faint track plus a value arc from 12 o'clock.
pub fn ring(value: f32, diameter: f32, stroke: f32, color: Color, track: Color) -> impl IntoElement {
    let value = value.clamp(0.0, 1.0);
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            stroke_arc(window, bounds, stroke, 0.0, TAU, track);
            if value > 0.005 {
                stroke_arc(window, bounds, stroke, -FRAC_PI_2, TAU * value, color);
            }
        },
    )
    .size(px(diameter))
    .flex_none()
}

/// Horizontal battery with a fill proportional to `value`.
pub fn battery(value: f32, charging: bool, color: Color, outline: Color) -> impl IntoElement {
    let value = value.clamp(0.0, 1.0);
    div()
        .flex()
        .items_center()
        .flex_none()
        .child(
            div()
                .w(px(19.0))
                .h(px(10.0))
                .rounded(px(crate::style::radius(3.0)))
                .bg(outline.with_alpha(outline.alpha() * 0.35).hsla())
                .p(px(1.5))
                .child(
                    div()
                        .h_full()
                        .w(px(16.0 * value.max(0.1)))
                        .rounded(px(crate::style::radius(1.8)))
                        .bg(if charging { color.lighten(0.06).hsla() } else { color.hsla() }),
                ),
        )
        .child(div().ml(px(1.0)).w(px(1.5)).h(px(4.0)).rounded_r(px(crate::style::radius(1.0))).bg(outline.hsla()))
}

/// A tiny filled dot.
pub fn dot(color: Color, diameter: f32) -> impl IntoElement {
    div().size(px(diameter)).rounded_full().bg(color.hsla()).flex_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_valid_svg() {
        for name in ICON_NAMES {
            let bytes = MandaraAssets.load(&format!("icons/{name}.svg")).unwrap().unwrap_or_else(|| panic!("{name} missing"));
            let text = std::str::from_utf8(&bytes).unwrap();
            assert!(text.starts_with("<svg") && text.ends_with("</svg>"), "{name}");
            // Parse with the same usvg version GPUI renders with.
            let tree = usvg::Tree::from_data(&bytes, &usvg::Options::default())
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(tree.root().has_children(), "{name} renders nothing");
        }
        assert!(MandaraAssets.load("icons/nope.svg").unwrap().is_none());
        assert!(MandaraAssets.load("other/x.png").unwrap().is_none());
        assert_eq!(MandaraAssets.list("icons").unwrap().len(), ICON_NAMES.len());
    }
}

/// Renders a user-chosen icon spec: a bundled Mandara icon name, `distro`
/// (the os-release logo), an icon-theme name, or an image path. Symbolic
/// SVGs tint with `tint`; colour images render as they are.
pub fn any_icon(spec: &str, size: f32, tint: Color) -> gpui::AnyElement {
    any_icon_at(spec, size, size, tint)
}

/// [`any_icon`] drawn at `size` from a bitmap decoded for `decode_size`:
/// an icon that grows or shrinks in an animation passes its final size, so
/// it never waits on a decode per frame.
pub fn any_icon_at(spec: &str, size: f32, decode_size: f32, tint: Color) -> gpui::AnyElement {
    use gpui::{IntoElement as _, ObjectFit, StyledImage as _, img};
    let spec = spec.trim();
    if let Some(name) = ICON_NAMES.iter().find(|n| **n == spec) {
        return icon(name, size, tint).into_any_element();
    }
    let name = if spec == "distro" { crate::icon_theme::distro_logo().unwrap_or_else(|| "start-here".into()) } else { spec.to_string() };
    let resolved = if name.starts_with('~') || name.starts_with('/') {
        // Cached lookup (no per-render `stat`).
        let path = mandara_config::paths::expand_tilde(&name);
        crate::icon_theme::resolve(&path.to_string_lossy(), None, false)
            .map(|r| crate::icon_theme::ResolvedIcon { symbolic: r.path.to_string_lossy().contains("-symbolic"), ..r })
    } else {
        crate::icon_theme::resolve(&name, None, false)
    };
    match resolved {
        Some(r) if r.symbolic => svg().external_path(r.path.to_string_lossy().to_string()).size(px(size)).flex_none().text_color(tint.hsla()).into_any_element(),
        Some(r) if crate::icon_raster::handles(&r.path) => {
            div().size(px(size)).flex_none().child(img(crate::icon_raster::source(&r.path, decode_size.max(size))).size_full().object_fit(ObjectFit::Contain)).into_any_element()
        }
        Some(r) => div().size(px(size)).flex_none().child(img(r.path).size_full().object_fit(ObjectFit::Contain)).into_any_element(),
        None => icon("grid", size, tint).into_any_element(),
    }
}
