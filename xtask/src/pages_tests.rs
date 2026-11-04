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
