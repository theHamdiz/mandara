//! The public pages state how many plugins Mandara ships and how many
//! compositors it supports. Those numbers drift whenever a plugin lands on
//! one branch and the docs on another, so they are checked here against the
//! sources of truth: the plugin catalog (`assets/catalog.json`, minus the
//! example plugins in `plugins/examples/`) and the eight compositor drivers.
//!
//! The website (`index.html`) has its own check in `pages_tests`.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// MangoWM, Niri, Hyprland, Umbriel, Sway, River Classic, KWin and Aqueous.
const COMPOSITORS: usize = 8;

/// Phrases that end in a plugin count: the number comes right before them.
const PLUGIN_COUNT_BEFORE: &[&str] = &[" built-in plugins", " first-party plugins", " plugins built in", " built-ins", "-plugin set", " plugins plus WebAssembly"];

/// Phrases that a plugin count comes right after.
const PLUGIN_COUNT_AFTER: &[&str] = &["| **Plugins** | ", "badge/plugins-"];

/// Number words a compositor count may be written in.
const NUMBER_WORDS: &[(&str, usize)] = &[("two", 2), ("three", 3), ("four", 4), ("five", 5), ("six", 6), ("seven", 7), ("eight", 8), ("nine", 9), ("ten", 10)];

/// The public text that states the counts: the README, the LLM summaries,
/// the guides, the artwork notes and the citation.
fn public_pages() -> Vec<PathBuf> {
    let root = crate::root();
    let mut pages: Vec<PathBuf> = ["README.md", "llms.txt", "llms-full.txt", "assets/README.md", "CITATION.cff"].iter().map(|p| root.join(p)).filter(|p| p.is_file()).collect();
    let mut docs: Vec<PathBuf> = std::fs::read_dir(root.join("docs")).unwrap().filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "md")).collect();
    docs.sort();
    pages.extend(docs);
    pages
}

/// The plugin ids in the catalog that are built in (not an example plugin).
fn builtin_ids() -> BTreeSet<String> {
    let root = crate::root();
    let catalog: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(root.join("assets/catalog.json")).unwrap()).unwrap();
    let examples: BTreeSet<String> = std::fs::read_dir(root.join("plugins/examples")).unwrap().filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    let ids: BTreeSet<String> = catalog.as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap().to_owned()).collect();
    assert!(examples.iter().all(|e| ids.contains(e)), "an example plugin is missing from the catalog");
    ids.difference(&examples).cloned().collect()
}

/// The number written right before byte `end` of `text`.
fn number_ending_at(text: &str, end: usize) -> Option<usize> {
    let start = text[..end].trim_end_matches(|c: char| c.is_ascii_digit()).len();
    text[start..end].parse().ok()
}

/// The number written right at the start of `text`.
fn number_at(text: &str) -> Option<usize> {
    let digits = text.len() - text.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    text[..digits].parse().ok()
}

/// Every plugin count `text` states.
fn plugin_counts(text: &str) -> Vec<usize> {
    let mut found = Vec::new();
    for phrase in PLUGIN_COUNT_BEFORE {
        found.extend(text.match_indices(phrase).filter_map(|(at, _)| number_ending_at(text, at)));
    }
    for phrase in PLUGIN_COUNT_AFTER {
        found.extend(text.match_indices(phrase).filter_map(|(at, _)| number_at(&text[at + phrase.len()..])));
    }
    found
}

/// Every compositor count `text` states: a number or number word right
/// before "compositors" (or "supported compositors").
fn compositor_counts(text: &str) -> Vec<usize> {
    let lower = text.to_lowercase();
    let mut found = Vec::new();
    for (at, _) in lower.match_indices(" compositors") {
        let before = lower[..at].trim_end_matches(" supported");
        let word = before.rsplit(|c: char| !c.is_ascii_alphanumeric()).next().unwrap_or_default();
        if let Ok(n) = word.parse() {
            found.push(n);
        } else if let Some((_, n)) = NUMBER_WORDS.iter().find(|(w, _)| *w == word) {
            found.push(*n);
        }
    }
    found
}

#[test]
fn the_scanners_read_every_way_a_count_is_written() {
    let text = "and 81 built-in plugins. | **Plugins** | 81 built in; badge/plugins-81%20built alt=\"81 plugins plus WebAssembly\" The 82 plugins below include 81 built-ins. The 81-plugin set uses";
    assert_eq!(plugin_counts(text).len(), 6);
    assert!(plugin_counts(text).iter().all(|n| *n == 81));
    assert_eq!(plugin_counts("2048 plugins on, 31 plugin panels"), Vec::<usize>::new());
    assert_eq!(compositor_counts("Seven compositors, all seven compositors, 6 supported compositors, many compositors"), vec![7, 7, 6]);
}

#[test]
fn public_pages_state_the_real_plugin_and_compositor_counts() {
    let builtins = builtin_ids().len();
    let art_index = std::fs::read_to_string(crate::root().join("crates/mandara-ui/assets/plugin-art/index.rs")).unwrap();
    let embedded = art_index.lines().filter(|l| l.trim_start().starts_with("(\"")).count();
    assert_eq!(embedded, builtins, "every built-in plugin in the catalog has embedded art (run `cargo xtask art`)");
    let mut wrong = Vec::new();
    let (mut plugin_mentions, mut compositor_mentions) = (0, 0);
    for page in public_pages() {
        let text = std::fs::read_to_string(&page).unwrap();
        let name = page.strip_prefix(crate::root()).unwrap().display().to_string();
        for n in plugin_counts(&text) {
            plugin_mentions += 1;
            if n != builtins {
                wrong.push(format!("{name}: {n} plugins, the catalog has {builtins} built in"));
            }
        }
        for n in compositor_counts(&text) {
            compositor_mentions += 1;
            if n != COMPOSITORS {
                wrong.push(format!("{name}: {n} compositors, Mandara supports {COMPOSITORS}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    assert!(plugin_mentions >= 5 && compositor_mentions >= 2, "the scanners found too little ({plugin_mentions} plugin and {compositor_mentions} compositor counts): did the wording change?");
}

/// The plugin guide's table lists every built-in plugin once, and the
/// number of opt-in plugins it states is the number it marks.
#[test]
fn the_plugin_guide_lists_every_builtin_and_counts_its_opt_ins() {
    let guide = std::fs::read_to_string(crate::root().join("docs/plugins.md")).unwrap();
    let rows: Vec<&str> = guide.lines().filter(|l| l.starts_with("| **")).collect();
    assert_eq!(rows.len(), builtin_ids().len(), "docs/plugins.md: one table row per built-in plugin");
    let marked = rows.iter().filter(|r| r.contains("*(opt-in)*")).count();
    let stated = guide.split_once(" first-party plugins (").and_then(|(_, rest)| number_at(rest)).expect("docs/plugins.md states how many plugins are opt-in");
    assert_eq!(stated, marked, "docs/plugins.md: the opt-in count differs from the rows marked opt-in");
}
