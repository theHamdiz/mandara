//! Keep the AI symbol meaningful across first-party surfaces.

use std::path::{Path, PathBuf};

/// Collect production Rust sources recursively for the icon policy check.
fn rust_sources(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_sources(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

/// Only the icon definition and explicit AI features may select the sparkle symbol.
#[test]
fn sparkle_icons_only_represent_ai() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root.join("crates")).unwrap() {
        let src = entry.unwrap().path().join("src");
        if src.is_dir() {
            rust_sources(&src, &mut files);
        }
    }
    for file in files {
        let path = file.strip_prefix(&root).unwrap().to_str().unwrap();
        let source = std::fs::read_to_string(&file).unwrap();
        for (index, line) in source
            .lines()
            .enumerate()
            .filter(|(_, line)| line.contains("\"sparkles\""))
        {
            let permitted = match path {
                "crates/mandara-ui/src/icons.rs" => {
                    line.trim_start().starts_with("\"sparkles\" =>")
                        || line.contains("\"pencil\", \"sparkles\", \"brush\"")
                }
                "crates/mandara-ui/src/plugins/local_ai.rs" => true,
                "crates/mandara-ui/src/desktop_plugins.rs" => line.contains("WidgetKind::AiUsage"),
                // The AI usage widget's empty state.
                "crates/mandara-ui/src/backdrop/widgets/feeds.rs" => line.contains("tk(\"AI usage\")"),
                "crates/mandara-studio/src/app/store_page.rs" => {
                    line.trim() == "\"local-ai\" => \"sparkles\","
                }
                _ => false,
            };
            assert!(
                permitted,
                "{path}:{}: reserve sparkles for AI; choose a symbol for this function",
                index + 1
            );
        }
    }
}

#[test]
fn non_ai_icons_and_wave_ornament_have_no_sparkle_shape() {
    let crescent = mandara_ui::icons::svg_source("crescent").unwrap();
    assert_eq!(
        crescent.matches("<path").count(),
        1,
        "the crescent needs no sparkle ornament"
    );
    assert!(
        mandara_ui::icons::svg_source("quote")
            .unwrap()
            .contains("M4 6h6")
    );
    let ornament = include_str!("../src/plugins/main_menu/ornament.rs");
    assert!(!ornament.contains("fn sparkle("));
    assert!(!ornament.contains("SPARKLE_WAIST"));
}
