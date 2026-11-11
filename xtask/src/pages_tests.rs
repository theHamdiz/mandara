//! The Pages site: the guides' theme must match transparent artwork and keep
//! its links portable, and the homepage must show the shell as it really is
//! (its Looks and palettes, default keybinds, plugins and compositors).
//!
//! `MANDARA_PAGES_DUMP=1 cargo test -p xtask pages` rewrites the homepage's
//! generated palette blocks from the theme engine.

use std::collections::{BTreeMap, BTreeSet};

use mandara_theme::{Color, MandaraPalette};

#[test]
fn the_favicon_and_stylesheet_resolve_under_a_project_base_url() {
    let head = std::fs::read_to_string(crate::root().join("_includes/head-custom.html")).unwrap();
    assert!(head.contains("head-custom-google-analytics.html"));
    assert!(head.contains("rel=\"icon\" type=\"image/png\" sizes=\"128x128\""));
    let paths: Vec<_> = head.split("{{ '").skip(1).map(|s| s.split("' | relative_url }}").next().unwrap()).collect();
    assert_eq!(paths.len(), 3);
    for path in paths {
        assert!(crate::root().join(path.trim_start_matches('/')).is_file(), "{path}");
    }
}

/// One deferred handler serves the homepage command and fenced guide examples.
#[test]
fn copy_controls_are_shared_and_leave_commands_readable_without_scripts() {
    let head = std::fs::read_to_string(crate::root().join("_includes/head-custom.html")).unwrap();
    let source = format!("<head>{head}</head>");
    let page = roxmltree::Document::parse(&source).unwrap();
    let scripts: Vec<_> = page.descendants().filter(|node| node.has_tag_name("script")).collect();
    assert_eq!(scripts.len(), 1);
    assert_eq!(scripts[0].attribute("src"), Some("{{ '/assets/js/copy-code.js' | relative_url }}"));
    assert!(scripts[0].has_attribute("defer"));

    let source = homepage();
    let page = roxmltree::Document::parse(&source).unwrap();
    let copy = page.descendants().find(|node| node.has_tag_name("script") && node.attribute("src") == Some("assets/js/copy-code.js")).unwrap();
    assert!(copy.has_attribute("defer"));
    let commands: Vec<_> = page.descendants().filter(|node| node.has_tag_name("div") && node.attribute("class") == Some("install-command")).collect();
    assert!(!commands.is_empty());
    for command in commands {
        let code = command.children().find(|node| node.has_tag_name("code")).unwrap();
        assert!(code.text().is_some_and(|text| !text.trim().is_empty()));
        assert!(!command.has_attribute("hidden"));
        assert!(!code.has_attribute("hidden"));
        assert!(!command.descendants().any(|node| node.has_tag_name("button")), "copy controls are added only when the handler runs");
    }
}

/// Copy feedback reaches screen readers as a polite status that exists
/// before its first message. A repeated press clears it and writes the
/// message in a later task, so identical words are announced again. Success
/// clears after a fixed lifetime; blocked-copy instructions have none, so
/// they stay while someone follows them (#111).
#[test]
fn copy_feedback_is_a_polite_status_that_repeats_and_keeps_blocked_instructions() {
    let script = std::fs::read_to_string(crate::root().join("assets/js/copy-code.js")).unwrap();
    for attribute in [r#"setAttribute("role", "status")"#, r#"setAttribute("aria-live", "polite")"#, r#"setAttribute("aria-atomic", "true")"#] {
        assert!(script.contains(attribute), "{attribute}");
    }
    assert!(!script.contains("assertive") && !script.contains(r#""alert""#), "copy feedback must not interrupt");
    let constant = |name: &str| -> u32 {
        let line = script.lines().find(|line| line.trim_start().starts_with(&format!("const {name} = "))).unwrap_or_else(|| panic!("{name}"));
        line.split('=').nth(1).unwrap().trim().trim_end_matches(';').parse().unwrap()
    };
    let (success, delay) = (constant("SUCCESS_MS"), constant("ANNOUNCE_DELAY_MS"));
    assert!((2000..=10_000).contains(&success), "long enough to be read, short enough not to describe an older press: {success}");
    assert!(delay > 0 && delay < 500, "a later task than the clearing, still prompt: {delay}");
    // Each press clears first and stops the previous success timer.
    let click = script.split(r#"addEventListener("click""#).nth(1).unwrap();
    let clear = click.find(r#"status.textContent = """#).unwrap();
    assert!(click.find("clearTimeout(clearTimer)").unwrap() < clear && clear < click.find("writeText").unwrap());
    assert!(click.contains(r#"announce(current, "Copied to clipboard.", SUCCESS_MS)"#));
    assert!(click.contains(r#"announce(current, "Copy blocked. Select and copy the command manually.", 0)"#), "blocked instructions never expire");
    // Messages are written only through the delayed announcement, for the latest press.
    assert_eq!(script.matches("status.textContent = message").count(), 1);
    assert!(script.contains("if (current !== press) return;"));
}

fn luminance(hex: &str) -> f64 {
    let bytes: Vec<_> = (1..7).step_by(2).map(|i| f64::from(u8::from_str_radix(&hex[i..i + 2], 16).unwrap()) / 255.0)
        .map(|v| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }).collect();
    bytes[0] * 0.2126 + bytes[1] * 0.7152 + bytes[2] * 0.0722
}

fn contrast(a: &str, b: &str) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[test]
fn page_text_links_and_muted_ink_pass_contrast_in_both_themes() {
    let css = std::fs::read_to_string(crate::root().join("assets/css/pages.css")).unwrap();
    assert!(css.contains("@media (prefers-color-scheme: dark)"));
    assert!(css.contains(".markdown-body img { background-color: transparent; }"));
    assert!(!css.contains(".highlight span"), "token spans must retain their syntax colours");
    let themes: Vec<_> = css.split(":root {").skip(1).map(|block| {
        block.split('}').next().unwrap().split(';').filter_map(|decl| decl.trim().split_once(": "))
            .map(|(key, value)| (key.trim(), value.trim())).collect::<BTreeMap<_, _>>()
    }).collect();
    assert_eq!(themes.len(), 2);
    for theme in themes {
        for bg in ["--page-background", "--page-surface"] {
            for ink in ["--page-ink", "--page-muted", "--page-link", "--syntax-string", "--syntax-number", "--syntax-error"] {
                let ratio = contrast(theme[bg], theme[ink]);
                assert!(ratio >= 4.5, "{ink} on {bg}: {ratio:.2}");
            }
        }
        assert_ne!(theme["--syntax-string"], theme["--page-link"]);
        assert_ne!(theme["--syntax-number"], theme["--page-ink"]);
    }
}

#[test]
fn transparent_banners_keep_solid_readable_wordmarks() {
    for (theme, background) in [("light", "#ffffff"), ("dark", "#0d1117")] {
        let banner = image::open(crate::root().join(format!("assets/brand/mandara-banner-{theme}.webp"))).unwrap().to_rgba8();
        assert_eq!(banner.dimensions(), (2560, 800));
        assert_eq!(banner.get_pixel(0, 0)[3], 0);
        // The title's interior excludes the logo and the supporting text.
        let ink = image::imageops::crop_imm(&banner, 1180, 140, 1000, 180).to_image();
        let mut solid = 0;
        for pixel in ink.pixels().filter(|p| p[3] == u8::MAX) {
            let color = format!("#{:02x}{:02x}{:02x}", pixel[0], pixel[1], pixel[2]);
            assert!(contrast(&color, background) >= 7.0);
            solid += 1;
        }
        assert!(solid > 20_000, "{theme}: the wordmark lost its solid interior");
    }
}

// ── The homepage ─────────────────────────────────────────────────────────

/// The homepage is plain static HTML (no Liquid), so it previews from disk
/// and Pages copies it as is; it stays well-formed for these checks.
fn homepage() -> String {
    let source = std::fs::read_to_string(crate::root().join("index.html")).unwrap();
    assert!(source.starts_with("<!doctype html>\n"), "no front matter: the homepage is copied as is");
    assert!(!source.contains("{{") && !source.contains("{%"), "no Liquid: relative links work on Pages and from disk");
    source.split_once("<!doctype html>").unwrap().1.trim().to_string()
}

fn site_css() -> String {
    std::fs::read_to_string(crate::root().join("assets/css/site.css")).unwrap()
}

fn classes<'a>(node: roxmltree::Node<'a, '_>) -> impl Iterator<Item = &'a str> {
    node.attribute("class").unwrap_or_default().split_whitespace()
}

fn has_class(node: roxmltree::Node<'_, '_>, class: &str) -> bool {
    classes(node).any(|c| c == class)
}

fn text_of(node: roxmltree::Node<'_, '_>) -> String {
    node.descendants().filter(|n| n.is_text()).filter_map(|n| n.text()).collect::<String>()
}

/// Opt-in faith plugins never appear in the homepage's pictures or showcases.
const FAITH: &[&str] = &["prayer", "jannah", "quran", "qur'an", "hadith"];

#[test]
fn homepage_links_resolve_and_pictures_reserve_their_space() {
    let source = homepage();
    let page = roxmltree::Document::parse(&source).unwrap();
    let ids: BTreeSet<_> = page.descendants().filter_map(|node| node.attribute("id")).collect();
    assert_eq!(page.descendants().filter(|node| node.has_tag_name("h1")).count(), 1);
    let mut pictures = 0;
    for node in page.descendants() {
        if node.has_tag_name("img") {
            pictures += 1;
            let src = node.attribute("src").unwrap();
            assert!(node.attribute("alt").is_some(), "{src}: alt text");
            assert!(src.starts_with("docs/screenshots/") || src.starts_with("assets/brand/"), "{src}: only Mandara's own pictures");
            // Declared dimensions are the file's own, so nothing shifts as it loads.
            let (width, height) = image::image_dimensions(crate::root().join(src)).unwrap();
            assert_eq!(node.attribute("width"), Some(width.to_string().as_str()), "{src}");
            assert_eq!(node.attribute("height"), Some(height.to_string().as_str()), "{src}");
        }
        if node.has_tag_name("source") && let Some(srcset) = node.attribute("srcset") {
            let picture = node.parent().unwrap();
            let img = picture.children().find(|n| n.has_tag_name("img")).unwrap();
            let dims = |path: &str| image::image_dimensions(crate::root().join(path)).unwrap();
            assert_eq!(dims(srcset), dims(img.attribute("src").unwrap()), "{srcset}: light and dark pictures share a size");
        }
        if node.has_tag_name("script") && let Some(src) = node.attribute("src") {
            assert!(!src.contains("://"), "{src}: scripts are self-hosted");
        }
        for attribute in node.attributes().filter(|attr| matches!(attr.name(), "href" | "src" | "srcset" | "data-src" | "data-poster-dark" | "data-poster-light")) {
            let value = attribute.value();
            let lower = value.to_lowercase();
            assert!(!FAITH.iter().any(|word| lower.contains(word)), "{value}: no faith plugins on the homepage");
            if let Some(id) = value.strip_prefix('#') {
                assert!(ids.contains(id), "missing section: {id}");
            } else if value.contains("://") {
                assert!(value.starts_with("https://"), "{value}");
            } else if attribute.name() != "data-src" {
                let path = value.split('#').next().unwrap();
                let file = crate::root().join(path);
                if path.ends_with('/') || file.is_dir() {
                    assert!(file.join("README.md").is_file() || file.join("index.html").is_file(), "missing directory index: {path}");
                } else {
                    assert!(file.is_file() || file.with_extension("md").is_file(), "missing page or asset: {path}");
                }
            }
        }
    }
    assert!(pictures >= 10, "a page of real screenshots: {pictures}");
    // Every screenshot comes as a pair: the light one is shown in light mode
    // and the dark one in dark mode, never the other way round.
    for img in page.descendants().filter(|n| n.has_tag_name("img")) {
        let src = img.attribute("src").unwrap();
        if !src.starts_with("docs/screenshots/") { continue; }
        let picture = img.parent().filter(|p| p.has_tag_name("picture")).unwrap_or_else(|| panic!("{src}: a screenshot sits in a <picture> with its light partner"));
        let light = picture.children().find(|n| n.has_tag_name("source") && n.attribute("media") == Some("(prefers-color-scheme: light)")).unwrap_or_else(|| panic!("{src}: no light partner"));
        let (stem, ext) = src.rsplit_once('.').unwrap();
        assert_eq!(light.attribute("srcset"), Some(format!("{stem}-light.{ext}").as_str()), "{src}: the light partner is named after the dark shot");
    }
    let lower = source.to_lowercase();
    for tracker in ["google-analytics", "googletagmanager", "gtag(", "plausible", "umami", "fonts.googleapis"] {
        assert!(!lower.contains(tracker), "{tracker}: no tracking or third-party requests");
    }
    let hero = page.descendants().find(|n| n.has_tag_name("section") && has_class(*n, "hero")).unwrap();
    let hero_text = text_of(hero).to_lowercase();
    assert!(!FAITH.iter().any(|word| hero_text.contains(word)), "faith plugins stay out of the hero");
}

/// The promo film slot: a fixed frame with a light and dark poster, sources
/// that load only after the page has painted, and its own controls.
#[test]
fn the_hero_film_reserves_its_frame_and_loads_late() {
    let source = homepage();
    let page = roxmltree::Document::parse(&source).unwrap();
    let film = page.descendants().find(|n| n.has_tag_name("figure") && has_class(*n, "film")).unwrap();
    let video = film.descendants().find(|n| n.has_tag_name("video")).unwrap();
    for attribute in ["autoplay", "muted", "loop", "playsinline", "aria-label"] {
        assert!(video.has_attribute(attribute), "{attribute}");
    }
    assert_eq!(video.attribute("preload"), Some("metadata"));
    assert!(!video.has_attribute("src") && !video.has_attribute("poster"), "nothing loads before the page paints");
    let sources: Vec<_> = video.children().filter(|n| n.has_tag_name("source")).collect();
    assert_eq!(sources.iter().map(|n| n.attribute("type").unwrap()).collect::<Vec<_>>(), ["video/webm", "video/mp4"]);
    for source in sources {
        assert!(!source.has_attribute("src"));
        assert!(source.attribute("data-src").unwrap().starts_with("assets/video/mandara-promo."));
    }
    let picture = film.children().find(|n| n.has_tag_name("picture")).unwrap();
    assert!(picture.descendants().any(|n| n.has_tag_name("source") && n.attribute("media") == Some("(prefers-color-scheme: light)")));
    let img = picture.descendants().find(|n| n.has_tag_name("img")).unwrap();
    assert_eq!(img.attribute("fetchpriority"), Some("high"));
    assert!(!img.has_attribute("loading"), "the hero poster is never lazy");
    let controls = film.descendants().find(|n| has_class(*n, "film-controls")).unwrap();
    assert!(controls.has_attribute("hidden"), "controls appear only once the film has a frame");
    let css = site_css();
    assert!(css.contains(".film { padding: 0; aspect-ratio: 16 / 9;"));
    let script = std::fs::read_to_string(crate::root().join("assets/js/site.js")).unwrap();
    assert!(script.contains(r#"if (reduceMotion.matches) video.removeAttribute("autoplay");"#));
    assert!(script.contains(r#"window.addEventListener("load", later, { once: true });"#));
}

// ── Palettes ─────────────────────────────────────────────────────────────

const ROLES: [&str; 13] = ["ground", "tile", "tile-2", "soft", "on-soft", "ink", "muted", "accent", "on-accent", "second", "on-second", "third", "on-third"];

/// The page's colour roles, taken from a shell palette.
fn roles(p: &MandaraPalette) -> [(&'static str, String); 13] {
    // Text on a filled tile is whichever of the page's ink and ground reads best.
    let on = |fill: Color| if p.on_surface.contrast_ratio(fill) >= p.surface_base.contrast_ratio(fill) { p.on_surface } else { p.surface_base };
    let values = [
        p.surface_base, p.surface_container, p.surface_container_high, p.primary_container, on(p.primary_container),
        p.on_surface, p.on_surface_variant, p.primary, p.on_primary, p.secondary, on(p.secondary), p.tertiary, on(p.tertiary),
    ];
    std::array::from_fn(|i| (ROLES[i], values[i].to_hex()))
}

/// The homepage's palettes: the default theme, then every Look in order.
fn site_palettes() -> Vec<(String, String, [MandaraPalette; 2])> {
    let theme = mandara_theme::catalog::find(mandara_theme::DEFAULT_THEME).unwrap();
    let mut out = vec![(theme.id.clone(), theme.name.clone(), [theme.palette(true), theme.palette(false)])];
    for look in mandara_config::looks::LOOKS.iter() {
        let palette = |dark| mandara_theme::material::palette_from_seed(look.seed_color(), look.variant, dark);
        out.push((look.id.to_string(), look.name.to_string(), [palette(true), palette(false)]));
    }
    out
}

fn palettes_json() -> String {
    let scheme = |p: &MandaraPalette| roles(p).iter().map(|(role, hex)| format!("\"{role}\":\"{hex}\"")).collect::<Vec<_>>().join(",");
    let entries: Vec<_> = site_palettes().iter()
        .map(|(id, name, [dark, light])| format!("\"{id}\":{{\"name\":\"{name}\",\"dark\":{{{}}},\"light\":{{{}}}}}", scheme(dark), scheme(light)))
        .collect();
    format!("{{\"default\":\"{}\",\"palettes\":{{\n{}\n}}}}", mandara_theme::DEFAULT_THEME, entries.join(",\n"))
}

fn palette_css() -> String {
    let (_, _, [dark, light]) = &site_palettes()[0];
    let block = |p: &MandaraPalette, indent: &str| roles(p).iter().map(|(role, hex)| format!("{indent}--{role}: {hex};\n")).collect::<String>();
    format!(
        "/* palette:start */\n:root {{\n  color-scheme: dark;\n{}}}\n@media (prefers-color-scheme: light) {{\n  :root {{\n    color-scheme: light;\n{}  }}\n}}\n/* palette:end */",
        block(dark, "  "),
        block(light, "    "),
    )
}

fn replace_between(text: &str, start: &str, end: &str, with: &str, inclusive: bool) -> String {
    let from = text.find(start).unwrap_or_else(|| panic!("{start}"));
    let to = from + text[from..].find(end).unwrap_or_else(|| panic!("{end}"));
    if inclusive { format!("{}{with}{}", &text[..from], &text[to + end.len()..]) } else { format!("{}{with}{}", &text[..from + start.len()], &text[to..]) }
}

const PALETTE_SCRIPT: &str = r#"<script type="application/json" id="look-palettes">"#;

/// The page's default colours are Mandara Corniche's and each Look's are the
/// palette the shell builds for it, in both schemes, read and written by
/// `MANDARA_PAGES_DUMP=1`.
#[test]
fn the_homepage_palettes_are_the_shells_own() {
    let index = crate::root().join("index.html");
    let css_path = crate::root().join("assets/css/site.css");
    let (json, css) = (palettes_json(), palette_css());
    if std::env::var_os("MANDARA_PAGES_DUMP").is_some() {
        let page = std::fs::read_to_string(&index).unwrap();
        std::fs::write(&index, replace_between(&page, PALETTE_SCRIPT, "</script>", &json, false)).unwrap();
        let sheet = std::fs::read_to_string(&css_path).unwrap();
        std::fs::write(&css_path, replace_between(&sheet, "/* palette:start */", "/* palette:end */", &css, true)).unwrap();
    }
    let page = std::fs::read_to_string(&index).unwrap();
    let embedded = page.split_once(PALETTE_SCRIPT).unwrap().1.split_once("</script>").unwrap().0;
    assert_eq!(embedded, json, "regenerate with MANDARA_PAGES_DUMP=1 cargo test -p xtask pages");
    assert!(std::fs::read_to_string(&css_path).unwrap().contains(&css), "regenerate with MANDARA_PAGES_DUMP=1 cargo test -p xtask pages");
    let parsed: serde_json::Value = serde_json::from_str(embedded).unwrap();
    assert_eq!(parsed["palettes"].as_object().unwrap().len(), 1 + mandara_config::looks::LOOKS.len());
    let script = std::fs::read_to_string(crate::root().join("assets/js/site.js")).unwrap();
    let listed = script.split("const ROLES = [").nth(1).unwrap().split(']').next().unwrap();
    assert_eq!(listed.split(", ").map(|r| r.trim_matches('"')).collect::<Vec<_>>(), ROLES, "site.js paints every role");
}

/// Every palette the page can wear keeps text readable (WCAG AA) on every
/// surface it is set on, in both schemes.
#[test]
fn every_homepage_palette_passes_contrast() {
    for (id, _, palettes) in site_palettes() {
        for p in &palettes {
            let r: BTreeMap<_, _> = roles(p).into_iter().collect();
            let mode = if p.is_dark { "dark" } else { "light" };
            let check = |fg: &str, bg: &str, min: f64| {
                let ratio = contrast(&r[fg], &r[bg]);
                assert!(ratio >= min, "{id} {mode}: {fg} on {bg} is {ratio:.2}");
            };
            for bg in ["ground", "tile", "tile-2"] {
                check("ink", bg, 4.5);
                check("muted", bg, 4.5);
            }
            for (fg, bg) in [("on-soft", "soft"), ("on-accent", "accent"), ("on-second", "second"), ("on-third", "third"), ("ground", "ink")] {
                check(fg, bg, 4.5);
            }
            // The accent as large display type (the name band and marquee).
            check("accent", "tile", 3.0);
            check("accent", "ground", 3.0);
        }
    }
}

/// Each Look has a keyboard control, light and dark previews, the shell's
/// own description and command, and swatches of its real palette.
#[test]
fn every_look_has_a_control_previews_and_its_real_palette() {
    let source = homepage();
    let page = roxmltree::Document::parse(&source).unwrap();
    let css = site_css();
    let inputs: Vec<_> = page.descendants().filter(|node| node.has_tag_name("input") && node.attribute("name") == Some("look")).collect();
    assert_eq!(inputs.len(), mandara_config::looks::LOOKS.len());
    assert_eq!(inputs.iter().filter(|node| node.has_attribute("checked")).count(), 1);
    for (input, look) in inputs.iter().zip(mandara_config::looks::LOOKS.iter()) {
        let id = format!("look-{}", look.id);
        assert_eq!(input.attribute("type"), Some("radio"));
        assert_eq!(input.attribute("id"), Some(id.as_str()));
        assert_eq!(input.attribute("value"), Some(look.id));
        let label = page.descendants().find(|node| node.has_tag_name("label") && node.attribute("for") == Some(id.as_str())).unwrap();
        assert_eq!(text_of(label), look.name);
        let dark = mandara_theme::material::palette_from_seed(look.seed_color(), look.variant, true);
        let swatches: Vec<_> = label.descendants().filter(|n| n.has_tag_name("i")).map(|n| n.attribute("style").unwrap().to_string()).collect();
        let expected: Vec<_> = [dark.primary, dark.secondary, dark.primary_container].iter().map(|c| format!("background: {}", c.to_hex())).collect();
        assert_eq!(swatches, expected, "{id}: swatches show the Look's palette");
        assert!(css.contains(&format!("#{id}:checked ~ .look-stage .{id}")), "{id}: selection shows its preview");
        assert!(css.contains(&format!("#{id}:checked ~ .looks-list .look-choice[for=\"{id}\"]")), "{id}: the picked Look is marked");
        assert!(css.contains(&format!("#{id}:focus-visible ~ .looks-list .look-choice[for=\"{id}\"]")), "{id}: keyboard focus shows on its label");
        let figure = page.descendants().find(|node| node.has_tag_name("figure") && has_class(*node, &id)).unwrap();
        let light = figure.descendants().find(|node| node.has_tag_name("source")).unwrap();
        assert_eq!(light.attribute("media"), Some("(prefers-color-scheme: light)"));
        assert_eq!(light.attribute("srcset"), Some(format!("docs/screenshots/{id}-light.webp").as_str()));
        let img = figure.descendants().find(|node| node.has_tag_name("img")).unwrap();
        assert_eq!(img.attribute("src"), Some(format!("docs/screenshots/{id}.webp").as_str()));
        let blurb = figure.descendants().find(|n| has_class(*n, "look-blurb")).unwrap();
        assert_eq!(text_of(blurb), look.blurb, "{id}: the shell's own description");
        let command = figure.descendants().find(|n| n.has_tag_name("code")).unwrap();
        assert_eq!(text_of(command), format!("mandara msg look apply {}", look.id));
    }
    assert!(css.contains("@media (prefers-reduced-motion: no-preference)"));
    assert!(css.contains("@media (prefers-reduced-motion: reduce)"));
}

/// The bar styles: a keyboard control, a full-size screenshot at its fixed
/// path and a command per style; only the chosen shot is displayed, so the
/// others load when picked.
#[test]
fn every_bar_style_has_a_control_a_shot_and_its_command() {
    let source = homepage();
    let page = roxmltree::Document::parse(&source).unwrap();
    let css = site_css();
    let inputs: Vec<_> = page.descendants().filter(|n| n.has_tag_name("input") && n.attribute("name") == Some("bar-style")).collect();
    let ids: Vec<_> = inputs.iter().map(|n| n.attribute("value").unwrap()).collect();
    assert_eq!(ids, ["expressive", "tile-dock", "rail", "segmented", "system-monitor", "stanley-bridge", "pharos-beam", "raml-tram", "side-bars"]);
    assert_eq!(inputs.iter().filter(|n| n.has_attribute("checked")).count(), 1);
    assert!(css.contains(".bar-shot { display: none;"), "unpicked shots stay unloaded");
    for id in ids {
        let class = format!("bar-{id}");
        let label = page.descendants().find(|n| n.has_tag_name("label") && n.attribute("for") == Some(class.as_str())).unwrap();
        assert!(css.contains(&format!("#{class}:checked ~ .bar-stage .{class}")));
        assert!(css.contains(&format!("#{class}:focus-visible ~ .bar-tabs label[for=\"{class}\"]")));
        let figure = page.descendants().find(|n| n.has_tag_name("figure") && has_class(*n, &class)).unwrap();
        let img = figure.descendants().find(|n| n.has_tag_name("img")).unwrap();
        assert_eq!(img.attribute("src"), Some(format!("docs/screenshots/bar-style-{id}.webp").as_str()));
        assert_eq!(img.attribute("loading"), Some("lazy"));
        // Its light twin, for a page in light mode.
        let light = figure.descendants().find(|n| n.has_tag_name("source")).unwrap();
        assert_eq!(light.attribute("srcset"), Some(format!("docs/screenshots/bar-style-{id}-light.webp").as_str()));
        assert!(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(format!("docs/screenshots/bar-style-{id}-light.webp")).is_file());
        if id != "side-bars" {
            let command = label.descendants().find(|n| n.has_tag_name("code")).unwrap();
            assert_eq!(text_of(command), format!("mandara msg bar style {id}"));
        }
    }
}

/// The shortcut browser lists Mandara's default keybinds, in order.
#[test]
fn the_keybind_browser_lists_every_default_keybind() {
    use mandara_config::compositor::{BindAction, default_keybinds};
    let source = homepage();
    let page = roxmltree::Document::parse(&source).unwrap();
    let rows: Vec<_> = page.descendants().filter(|n| n.has_tag_name("li") && has_class(*n, "bind")).collect();
    let defaults = default_keybinds();
    assert_eq!(rows.len(), defaults.len());
    for (row, bind) in rows.iter().zip(&defaults) {
        let BindAction::Mandara(command) = &bind.action else { panic!("{bind:?}: default binds run Mandara commands") };
        assert_eq!(row.attribute("data-mods"), Some(bind.mods.join(" ").as_str()));
        assert_eq!(row.attribute("data-key"), Some(bind.key.as_str()));
        assert_eq!(row.attribute("data-command"), Some(command.as_str()));
        let code = row.descendants().find(|n| n.has_tag_name("code")).unwrap();
        assert_eq!(text_of(code), *command);
        let keys = row.descendants().filter(|n| n.has_tag_name("kbd")).count();
        assert_eq!(keys, bind.mods.len() + 1, "{command}: one key cap per key");
        assert!(["panels", "capture", "windows", "hardware"].contains(&row.attribute("data-group").unwrap()));
    }
    let count = page.descendants().find(|n| has_class(*n, "keys-count")).unwrap();
    assert_eq!(text_of(count), format!("{} shortcuts", defaults.len()));
}

/// The shelf holds every first-party plugin the guide lists, except the
/// opt-in faith plugins, which keep to their own guide section.
#[test]
fn the_plugin_shelf_matches_the_guide() {
    let guide = std::fs::read_to_string(crate::root().join("docs/plugins.md")).unwrap();
    let count: usize = guide.split("The ").find_map(|s| s.strip_suffix_before(" first-party plugins")).unwrap();
    let table = guide.split(&format!("The {count} first-party plugins")).nth(1).unwrap().split("\n\n").nth(1).unwrap();
    let names: BTreeSet<_> = table.lines().filter_map(|l| l.strip_prefix("| **")).map(|l| l.split("**").next().unwrap().to_string()).collect();
    assert_eq!(names.len(), count);
    let faith: BTreeSet<_> = ["Prayer times", "Jannah", "Qur'an", "Hadith"].map(String::from).into();
    assert!(faith.is_subset(&names));
    let source = homepage();
    let page = roxmltree::Document::parse(&source).unwrap();
    let shelf = page.descendants().find(|n| n.has_tag_name("ul") && has_class(*n, "shelf")).unwrap();
    let spines: Vec<_> = shelf.descendants().filter(|n| n.has_tag_name("a") && has_class(*n, "spine")).collect();
    let shown: BTreeSet<_> = spines.iter().filter(|n| !has_class(**n, "spine-more")).map(|n| text_of(n.first_element_child().unwrap())).collect();
    assert_eq!(shown, names.difference(&faith).cloned().collect(), "every plugin but the faith ones");
    assert_eq!(text_of(spines.last().unwrap().first_element_child().unwrap()), format!("All {count}, in the guide"));
    let heading = page.descendants().find(|n| n.attribute("id") == Some("plugins-title")).unwrap();
    assert!(text_of(heading).starts_with(&format!("{count} plugins")));
    let rail = page.descendants().find(|n| has_class(*n, "rail-3")).unwrap();
    assert!(text_of(rail).starts_with(&count.to_string()));
}

trait StripBefore {
    fn strip_suffix_before(&self, marker: &str) -> Option<usize>;
}

impl StripBefore for str {
    /// The number right before `marker` at the start of this text.
    fn strip_suffix_before(&self, marker: &str) -> Option<usize> {
        self.split_once(marker).and_then(|(number, _)| number.trim().parse().ok())
    }
}

/// Compositors and languages match the README and the locale catalogs.
#[test]
fn compositors_and_languages_match_the_project() {
    let source = homepage();
    let page = roxmltree::Document::parse(&source).unwrap();
    let names: Vec<_> = page.descendants().filter(|n| has_class(*n, "dep-name")).map(text_of).collect();
    assert_eq!(names, ["MangoWM", "Niri", "Hyprland", "Umbriel", "Sway", "River Classic", "KWin", "Aqueous"]);
    let readme = std::fs::read_to_string(crate::root().join("README.md")).unwrap();
    assert!(readme.contains("**MangoWM, Niri, Hyprland, Umbriel, Sway, River Classic, KWin and Aqueous**"));
    let rail = page.descendants().find(|n| has_class(*n, "rail-2")).unwrap();
    assert!(text_of(rail).starts_with(&names.len().to_string()));
    let catalogs = std::fs::read_dir(crate::root().join("crates/mandara-i18n/locales")).unwrap()
        .filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|x| x == "toml")).count();
    let languages: Vec<_> = page.descendants().filter(|n| n.has_tag_name("li") && has_class(*n, "lang")).collect();
    assert_eq!(languages.len(), catalogs + 1, "English plus one tile per catalog");
    for tile in &languages {
        let name = tile.first_element_child().unwrap();
        let lang = name.attribute("lang").unwrap();
        assert!(lang == "en" || crate::root().join(format!("crates/mandara-i18n/locales/{lang}.toml")).is_file(), "{lang}");
        assert_eq!(name.attribute("dir") == Some("rtl"), ["ar", "fa", "ur"].contains(&lang), "{lang}: direction");
    }
    let rail = page.descendants().find(|n| has_class(*n, "rail-4")).unwrap();
    assert!(text_of(rail).starts_with(&languages.len().to_string()));
}

#[test]
fn the_homepage_install_command_matches_the_guide() {
    let source = homepage();
    let page = roxmltree::Document::parse(&source).unwrap();
    let guide = std::fs::read_to_string(crate::root().join("docs/install.md")).unwrap();
    let command = page.descendants().find(|node| node.has_tag_name("div") && node.attribute("class") == Some("install-command")).unwrap()
        .children().find(|node| node.has_tag_name("code")).unwrap().text().unwrap();
    assert!(guide.lines().any(|line| line == command));
    assert!(guide.contains("nix run github:theHamdiz/mandara"));
    assert!(source.contains("<code>nix run github:theHamdiz/mandara</code>"));
}

/// Public descriptions keep local control discoverable without a false RAM ceiling.
#[test]
fn public_agent_descriptions_explain_local_control_and_qualify_performance() {
    for path in ["README.md", "index.html", "llms.txt", "CITATION.cff"] {
        let text = std::fs::read_to_string(crate::root().join(path)).unwrap();
        let lower = text.to_lowercase();
        assert!(lower.contains("natural language"), "{path}: missing natural language control");
        assert!(lower.contains("offline") || lower.contains("your words stay on your computer"), "{path}: missing local inference");
        assert!(lower.contains("historical") && lower.contains("frame"), "{path}: qualify frame measurements");
        assert!(!text.contains("uses under 100 MiB") && !text.contains("Performance: under 100 MiB"), "{path}: unsupported memory ceiling");
        if path != "CITATION.cff" {
            assert!(text.contains("docs/command-agent."), "{path}: missing agent guide");
        }
    }
    let page = homepage();
    roxmltree::Document::parse(&page).unwrap();
    let citation = std::fs::read_to_string(crate::root().join("CITATION.cff")).unwrap();
    let metadata: serde_yaml_ng::Value = serde_yaml_ng::from_str(&citation).unwrap();
    assert_eq!(metadata["cff-version"].as_str(), Some("1.2.0"));
    assert_eq!(metadata["type"].as_str(), Some("software"));
}

/// Every natural-language launcher example includes the opt-in trigger.
#[test]
fn launcher_examples_on_the_homepage_start_with_bang() {
    let source = homepage();
    let page = roxmltree::Document::parse(&source).unwrap();
    let examples: Vec<_> = page.descendants().filter(|n| n.has_tag_name("code")).filter_map(|n| n.text()).filter(|text| text.contains("switch to dark mode")).collect();
    assert!(examples.len() >= 2);
    assert!(examples.iter().all(|text| text.starts_with('!')));
    let agent = page.descendants().find(|n| has_class(*n, "agent-copy")).unwrap();
    assert!(text_of(agent).contains("download its required models"));
}

/// A phone never scrolls the page sideways: the root clips (with `hidden`
/// as the fallback where `clip` is missing), the hero clips its turning
/// blobs, and the shortcut search box may shrink below its natural width.
#[test]
fn the_page_is_never_wider_than_the_screen() {
    let css = site_css();
    assert!(css.contains("overflow-x: hidden; overflow-x: clip; }"), "the root clips sideways overflow");
    let hero = css.split("\n.hero {").nth(1).and_then(|r| r.split('}').next()).unwrap();
    assert!(hero.contains("overflow-x: clip;"), "the hero clips its blobs");
    let search = css.split("\n.keys-search {").nth(1).and_then(|r| r.split('}').next()).unwrap();
    assert!(search.contains("min-width: 0;"), "the search box can shrink on a narrow phone");
}

/// The agent card's typed line keeps its own ink on its own ground: the
/// card's `.agent p` colour (made for the card's tint) must not reach it,
/// or a Look whose card ink equals the page ground hides the line.
#[test]
fn the_agent_line_keeps_its_own_ink() {
    let css = site_css();
    assert!(css.contains(".agent p.agent-line, .agent-line { color: var(--ink); }"));
}
