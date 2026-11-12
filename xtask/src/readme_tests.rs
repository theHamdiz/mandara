//! Keep the README's visual layout and feature coverage when editing its copy.

use sha2::{Digest as _, Sha256};

fn readme() -> String {
    std::fs::read_to_string(crate::root().join("README.md")).unwrap()
}

#[test]
fn the_readme_keeps_its_images_badges_and_gallery_layout() {
    let source = readme();
    let mut tags = Vec::new();
    for (start, _) in source.match_indices('<') {
        let Some(end) = source[start..].find('>') else { continue };
        let tag = &source[start..=start + end];
        let name = tag.trim_start_matches('<').trim_start_matches('/').split([' ', '\t', '\n', '/', '>']).next().unwrap();
        if ["p", "picture", "source", "img", "br", "sub", "b", "i", "table", "tr", "td", "a", "code"].contains(&name) {
            tags.push(tag);
        }
    }
    // Includes source URLs, alt text, scheme variants, cell widths and tag order.
    // Update only when a deliberate visual change has been reviewed.
    assert_eq!(format!("{:x}", Sha256::digest(tags.join("\n"))), "e6bc233c89d1a185a92e86c71963c1c3918a32d95a158bbad10d0aa741d5202e");
}

fn section<'a>(source: &'a str, heading: &str) -> &'a str {
    source.split_once(&format!("## {heading}\n")).unwrap().1.split("\n## ").next().unwrap()
}

#[test]
fn the_readme_keeps_its_feature_lists_and_key_capabilities() {
    let source = readme();
    let detailed = section(&source, "What sets Mandara apart");
    let numbered: Vec<_> = detailed.lines().filter_map(|line| line.split_once(". **")).filter_map(|(number, _)| number.parse::<usize>().ok()).collect();
    assert_eq!(numbered, (1..=12).collect::<Vec<_>>());
    assert_eq!(section(&source, "Why you'll like it").lines().filter(|line| line.starts_with("- **")).count(), 7);
    assert_eq!(section(&source, "At a glance").lines().filter(|line| line.starts_with("| **")).count(), 10);
    let lower = source.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    for feature in [
        "dynamic island", "on-screen displays", "offline ocr", "polkit", "clipboard", "all displays", "panel images",
        "profiles", "19 bar presets", "32 pages", "86 built-in plugins", "webassembly", "natural language",
        "ghostty", "gtk 3/4", "qt 5/6", "kvantum", "zed", "kitty", "foot", "alacritty", "btop", "cava", "tmux", "zellij", "spicetify",
        "grub", "limine", "refind", "plymouth", "nixos", "freebsd", "steamos", "hijri", "eastern arabic", "yasser al-dossary", "abdulrashid sufi", "abdul basit abdul samad",
    ] {
        assert!(lower.contains(feature), "missing feature: {feature}");
    }
}
