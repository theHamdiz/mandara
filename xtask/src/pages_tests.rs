//! The Pages theme must match transparent artwork and keep its links portable.

use std::collections::BTreeMap;

#[test]
fn the_favicon_and_stylesheet_resolve_under_a_project_base_url() {
    let head = std::fs::read_to_string(crate::root().join("_includes/head-custom.html")).unwrap();
    assert!(head.contains("head-custom-google-analytics.html"));
    assert!(head.contains("rel=\"icon\" type=\"image/png\" sizes=\"128x128\""));
    let paths: Vec<_> = head.split("{{ '").skip(1).map(|s| s.split("' | relative_url }}").next().unwrap()).collect();
    assert_eq!(paths.len(), 2);
    for path in paths {
        assert!(crate::root().join(path.trim_start_matches('/')).is_file(), "{path}");
    }
}

fn luminance(hex: &str) -> f64 {
    let bytes: Vec<_> = (1..7).step_by(2).map(|i| f64::from(u8::from_str_radix(&hex[i..i + 2], 16).unwrap()) / 255.0)
        .map(|v| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }).collect();
    bytes[0] * 0.2126 + bytes[1] * 0.7152 + bytes[2] * 0.0722
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
                let (a, b) = (luminance(theme[bg]), luminance(theme[ink]));
                let contrast = (a.max(b) + 0.05) / (a.min(b) + 0.05);
                assert!(contrast >= 4.5, "{ink} on {bg}: {contrast:.2}");
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
            let (a, b) = (luminance(&color), luminance(background));
            assert!((a.max(b) + 0.05) / (a.min(b) + 0.05) >= 7.0);
            solid += 1;
        }
        assert!(solid > 20_000, "{theme}: the wordmark lost its solid interior");
    }
}

fn homepage() -> String {
    let source = std::fs::read_to_string(crate::root().join("index.html")).unwrap();
    assert!(source.starts_with("---\nlayout: null\n---\n"));
    source.split_once("<!doctype html>").unwrap().1.trim().to_string()
}

#[test]
fn the_homepage_uses_existing_artwork_guides_and_section_targets() {
    let source = homepage();
    let page = roxmltree::Document::parse(&source).unwrap();
    let ids: std::collections::BTreeSet<_> = page.descendants().filter_map(|node| node.attribute("id")).collect();
    assert_eq!(page.descendants().filter(|node| node.has_tag_name("h1")).count(), 1);
    let mut pictures = 0;
    for node in page.descendants() {
        if node.has_tag_name("img") {
            pictures += 1;
            assert!(node.attribute("alt").is_some());
            assert!(node.attribute("width").is_some());
            assert!(node.attribute("height").is_some());
        }
        for attribute in node.attributes().filter(|attr| matches!(attr.name(), "href" | "src" | "srcset" | "content")) {
            let value = attribute.value();
            if let Some(id) = value.strip_prefix('#') {
                assert!(ids.contains(id), "missing section: {id}");
            } else if let Some(path) = value.strip_prefix("{{ '").and_then(|s| s.split_once("' | ").map(|(path, _)| path)) {
                let path = path.split('#').next().unwrap().trim_start_matches('/');
                if path.is_empty() { continue; }
                let file = crate::root().join(path);
                if file.is_dir() {
                    assert!(file.join("README.md").is_file() || file.join("index.html").is_file(), "missing directory index: {path}");
                } else {
                    assert!(file.is_file() || file.with_extension("md").is_file(), "missing page or asset: {path}");
                    assert!(!path.ends_with("README.html"), "Pages publishes README files as directory indexes: {path}");
                }
            }
        }
    }
    assert!(pictures >= 4);
    assert!(!source.contains("<script"));
}

#[test]
fn every_look_has_a_keyboard_control_and_light_and_dark_preview() {
    let source = homepage();
    let page = roxmltree::Document::parse(&source).unwrap();
    let css = std::fs::read_to_string(crate::root().join("assets/css/landing.css")).unwrap();
    let inputs: Vec<_> = page.descendants().filter(|node| node.has_tag_name("input") && node.attribute("name") == Some("look")).collect();
    assert_eq!(inputs.len(), 3);
    assert_eq!(inputs.iter().filter(|node| node.has_attribute("checked")).count(), 1);
    for input in inputs {
        assert_eq!(input.attribute("type"), Some("radio"));
        let id = input.attribute("id").unwrap();
        assert!(page.descendants().any(|node| node.has_tag_name("label") && node.attribute("for") == Some(id)));
        assert!(css.contains(&format!("#{id}:checked ~ .look-stage .{id}")));
        assert!(css.contains(&format!("#{id}:focus-visible")));
        let figure = page.descendants().find(|node| node.has_tag_name("figure") && node.attribute("class").is_some_and(|s| s.split_whitespace().any(|class| class == id))).unwrap();
        assert!(figure.descendants().any(|node| node.has_tag_name("source") && node.attribute("media") == Some("(prefers-color-scheme: light)")));
        assert!(figure.descendants().any(|node| node.has_tag_name("img") && node.attribute("src").is_some()));
        for (link, image, attribute) in [("preview-dark", "img", "src"), ("preview-light", "source", "srcset")] {
            let preview = figure.descendants().find(|node| node.has_tag_name(image)).unwrap().attribute(attribute).unwrap();
            let full = figure.descendants().find(|node| node.has_tag_name("a") && node.attribute("class") == Some(link)).unwrap().attribute("href").unwrap();
            assert_eq!(full, preview, "full-size {link} differs from its displayed image");
        }
    }
    assert!(css.contains("@media (prefers-reduced-motion: no-preference)"));
    assert!(css.contains("@media (prefers-reduced-motion: reduce)"));
    assert!(css.contains("@supports (animation-timeline: view())"));
    assert!(css.contains(".native-section a:focus-visible { outline-color: var(--page-background); }"), "the inverted native section needs a contrasting keyboard focus ring");
    assert!(css.contains(".questions details[open] summary:focus-visible,") && css.contains(".questions details[open] a:focus-visible { outline-color: var(--page-background); }"), "open FAQ items and their links need a contrasting keyboard focus ring");
}

#[test]
fn the_homepage_install_command_and_plugin_count_match_the_guides() {
    let source = homepage();
    let page = roxmltree::Document::parse(&source).unwrap();
    let guide = std::fs::read_to_string(crate::root().join("docs/install.md")).unwrap();
    let command = page.descendants().find(|node| node.has_tag_name("div") && node.attribute("class") == Some("install-command")).unwrap()
        .children().find(|node| node.has_tag_name("code")).unwrap().text().unwrap();
    assert!(guide.lines().any(|line| line == command));
    let plugins = std::fs::read_to_string(crate::root().join("docs/plugins.md")).unwrap();
    let count = page.descendants().filter(|node| node.has_tag_name("p")).filter_map(|node| node.text())
        .find(|text| text.contains(" built-in plugins")).unwrap().split_whitespace().next().unwrap();
    assert!(plugins.contains(&format!("The {count} first-party plugins")), "homepage plugin count differs from its guide");
}
