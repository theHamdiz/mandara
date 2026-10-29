//! Facts for plugin authors, straight from the code so the docs can't
//! drift: the WIT contract (embedded and parsed), an annotated
//! `plugin.toml` that unit tests parse with the real manifest parser, the
//! folder layout, and the quick-start commands. Studio → Plugins → Develop
//! renders these; PLUGINS.md describes the same things.

use mandara_i18n::tk;

use crate::wasm::{ManifestFile, PERMISSION_COMMANDS};

/// The plugin interface, exactly as the host is generated from it.
pub const WIT: &str = include_str!("../wit/plugin.wit");

/// The WIT file's path in the Mandara repository.
pub const WIT_REPO_PATH: &str = "crates/mandara-plugins/wit/plugin.wit";

/// The WIT folder in the Mandara repository (copied next to a plugin crate).
pub const WIT_DIR: &str = "crates/mandara-plugins/wit";

/// Mandara's repository.
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// The authoring guide online.
pub const GUIDE_URL: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/blob/main/PLUGINS.md");

/// Where packages install the guide.
pub const GUIDE_LOCAL_PATHS: &[&str] = &["/usr/share/doc/mandara/PLUGINS.md", "/usr/local/share/doc/mandara/PLUGINS.md"];

/// The Rust target plugins are compiled for.
pub const WASM_TARGET: &str = "wasm32-wasip2";

/// The example plugin, relative to the repository root.
pub const EXAMPLE_DIR: &str = "plugins/examples/moon-phase";

/// The `wit-bindgen` version the example uses.
pub const WIT_BINDGEN_VERSION: &str = "0.57";

/// Recommended icon and cover sizes (px).
pub const ICON_SIDE_PX: u32 = 256;
pub const COVER_W_PX: u32 = 1600;
pub const COVER_H_PX: u32 = 900;

/// An annotated `plugin.toml` with every field and every setting type.
/// `example_manifest_parses` checks it with the real parser.
pub const EXAMPLE_MANIFEST: &str = r#"# Required. Lowercase letters, digits, - and _ (1–64); unique.
id = "moon-phase"
# Required. Shown in Studio, the bar and the sidebar.
name = "Moon phase"
description = "Tonight's moon: phase, illumination and days to the next full moon"
author = "Your Name"
version = "0.1.0"                # default "0.0.0"
zone = "right"                   # default bar zone: left | center | right
entry = "plugin.wasm"            # the component, relative to this folder (default)
permissions = []                 # ["commands"] to run Mandara commands
homepage = "https://github.com/you/mandara-moon-phase"

# Name and description per language (ISO 639-1 codes).
[i18n.ar]
name = "طور القمر"
description = "قمر الليلة: طوره ونسبة إضاءته والأيام حتى البدر"

# Settings: Studio shows them in the plugin's settings modal, stores them
# in [plugins.settings.moon-phase] and host::setting(key) returns JSON.
[[settings]]
key = "show_percent"             # lowercase letters, digits, - and _
type = "bool"                    # a switch (the default type)
label = "Illumination in the bar"
help = "Show the lit percentage next to the moon icon"
default = true

[[settings]]
key = "refresh"
type = "number"                  # min + max → a slider, else a number field
label = "Refresh every"
default = 15
min = 5
max = 60
step = 5
unit = " min"

[[settings]]
key = "greeting"
type = "text"
label = "Greeting"
default = "Good night"

[[settings]]
key = "hemisphere"
type = "choice"
label = "Hemisphere"
help = "Where you watch from: it decides which side of the moon is lit"
default = "north"
choices = [{ value = "north", label = "Northern" }, { value = "south", label = "Southern" }]

[[settings]]
key = "style"
type = "choice"
choices = ["emoji", "icon"]      # plain strings work too
"#;

/// One entry of a plugin folder.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LayoutEntry {
    /// Path inside the plugin folder (`/` suffix for folders).
    pub path: &'static str,
    /// Nesting depth (0 = top level).
    pub depth: u8,
    pub required: bool,
    /// English description (translated by the caller with `tr`).
    pub note: &'static str,
}

/// The plugin folder layout.
pub const LAYOUT: &[LayoutEntry] = &[
    LayoutEntry { path: "plugin.toml", depth: 0, required: true, note: tk("Manifest: id, name, zone, permissions, settings") },
    LayoutEntry { path: "plugin.wasm", depth: 0, required: true, note: tk("The compiled WebAssembly component (manifest `entry`)") },
    LayoutEntry { path: "locales/", depth: 0, required: false, note: tk("Translations of your labels") },
    LayoutEntry { path: "ar.toml", depth: 1, required: false, note: tk("\"English\" = \"Translation\" pairs, one file per language") },
    LayoutEntry { path: "icon.png", depth: 0, required: false, note: tk("256×256 rounded-square tile for Studio") },
    LayoutEntry { path: "cover.png", depth: 0, required: false, note: tk("1600×900 cover on top of the detail modal") },
];

/// A function in the WIT contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WitFunc {
    pub name: String,
    /// `func(…) -> …`
    pub signature: String,
    /// Its `///` doc comment (may be empty).
    pub doc: String,
}

/// A case of a WIT enum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WitCase {
    pub name: String,
    pub doc: String,
}

/// The contract, parsed from [`WIT`].
#[derive(Clone, Debug, Default)]
pub struct Contract {
    /// `package …;`
    pub package: String,
    /// What a plugin provides (`world plugin` exports).
    pub exports: Vec<WitFunc>,
    /// What Mandara provides (`interface host`).
    pub imports: Vec<WitFunc>,
    /// View node kinds (`enum kind`).
    pub kinds: Vec<WitCase>,
    /// Text / button tones (`enum tone`).
    pub tones: Vec<WitCase>,
    /// Fields of `record node`.
    pub node_fields: Vec<WitFunc>,
}

/// Parses the subset of WIT that [`WIT`] uses.
pub fn parse_contract(wit: &str) -> Contract {
    #[derive(PartialEq)]
    enum Block {
        None,
        Host,
        World,
        Kind,
        Node,
        Other,
    }
    let mut c = Contract::default();
    let mut block = Block::None;
    let mut depth = 0usize;
    let mut doc: Vec<String> = Vec::new();
    for raw in wit.lines() {
        let line = raw.trim();
        if let Some(d) = line.strip_prefix("///") {
            doc.push(d.trim().to_string());
            continue;
        }
        if line.starts_with("//") || line.is_empty() {
            continue;
        }
        if let Some(p) = line.strip_prefix("package ") {
            c.package = p.trim_end_matches(';').trim().to_string();
            doc.clear();
            continue;
        }
        let take_doc = |doc: &mut Vec<String>| std::mem::take(doc).join(" ");
        // One-line enum: `enum tone { normal, muted, … }`.
        if let (Some(rest), true) = (line.strip_prefix("enum "), line.ends_with('}')) {
            let (name, body) = rest.split_once('{').unwrap_or((rest, ""));
            let cases: Vec<WitCase> = body.trim_end_matches('}').split(',').map(str::trim).filter(|s| !s.is_empty()).map(|s| WitCase { name: s.into(), doc: String::new() }).collect();
            if name.trim() == "tone" {
                c.tones = cases;
            }
            doc.clear();
            continue;
        }
        if line.ends_with('{') {
            depth += 1;
            if depth <= 2 {
                block = match line.trim_end_matches('{').trim() {
                    "interface host" => Block::Host,
                    "world plugin" => Block::World,
                    "enum kind" => Block::Kind,
                    "record node" => Block::Node,
                    _ if depth == 1 => Block::None,
                    _ => Block::Other,
                };
            }
            doc.clear();
            continue;
        }
        if line.starts_with('}') {
            depth = depth.saturating_sub(1);
            block = if depth == 0 { Block::None } else { Block::Other };
            doc.clear();
            continue;
        }
        match block {
            Block::Host | Block::World => {
                let decl = if block == Block::World { line.strip_prefix("export ") } else { Some(line) };
                if let Some((name, sig)) = decl.and_then(|d| d.split_once(':')).filter(|(_, s)| s.trim_start().starts_with("func")) {
                    let f = WitFunc { name: name.trim().into(), signature: sig.trim().trim_end_matches(';').into(), doc: take_doc(&mut doc) };
                    if block == Block::World { c.exports.push(f) } else { c.imports.push(f) }
                }
            }
            Block::Kind => {
                for case in line.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    c.kinds.push(WitCase { name: case.into(), doc: take_doc(&mut doc) });
                }
            }
            Block::Node => {
                if let Some((name, ty)) = line.split_once(':') {
                    c.node_fields.push(WitFunc { name: name.trim().into(), signature: ty.trim().trim_end_matches(',').into(), doc: take_doc(&mut doc) });
                }
            }
            Block::None | Block::Other => {}
        }
        doc.clear();
    }
    c
}

/// The contract of this build.
pub fn contract() -> &'static Contract {
    static C: std::sync::OnceLock<Contract> = std::sync::OnceLock::new();
    C.get_or_init(|| parse_contract(WIT))
}

/// One quick-start step: a shell command block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// English title (translated by the caller with `tr`).
    pub title: &'static str,
    pub commands: String,
}

/// From nothing to a running plugin in the development folder `dev_dir`.
pub fn quick_start(dev_dir: &std::path::Path) -> Vec<Step> {
    let dest = dev_dir.join("my-plugin");
    let dest = dest.display();
    vec![
        Step { title: tk("Add the WebAssembly target"), commands: format!("rustup target add {WASM_TARGET}") },
        Step {
            title: tk("Start from the example plugin"),
            // The example's `generate!` points at the WIT inside the
            // repository; a copied plugin uses its own `wit/` folder.
            commands: format!(
                "git clone --depth 1 {REPOSITORY} mandara-src\ncp -r mandara-src/{EXAMPLE_DIR} my-plugin\ncp -r mandara-src/{WIT_DIR} my-plugin/wit\ncd my-plugin\nsed -i 's#\"../../../{WIT_DIR}\"#\"wit\"#' src/lib.rs\nsed -i 's/^name = \"moon-phase\"/name = \"my-plugin\"/' Cargo.toml\n# edit plugin.toml (id, name) and src/lib.rs"
            ),
        },
        Step { title: tk("Build the component"), commands: format!("cargo build --release --target {WASM_TARGET}") },
        Step {
            title: tk("Install it into the development folder"),
            commands: format!(
                "mkdir -p {dest}\ncp plugin.toml {dest}/\ncp -r locales icon.png cover.png {dest}/ 2>/dev/null\ncp target/{WASM_TARGET}/release/my_plugin.wasm {dest}/plugin.wasm"
            ),
        },
    ]
}

/// The permissions a manifest may list.
pub const PERMISSIONS: &[&str] = &[PERMISSION_COMMANDS];

/// Parses [`EXAMPLE_MANIFEST`] (always succeeds; tested).
pub fn example_manifest() -> ManifestFile {
    ManifestFile::parse(EXAMPLE_MANIFEST).expect("EXAMPLE_MANIFEST parses")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SettingKind;

    #[test]
    fn example_manifest_parses() {
        let m = example_manifest();
        assert!(m.problems().is_empty(), "{:?}", m.problems());
        assert_eq!(m.id, "moon-phase");
        assert!(!m.name.is_empty() && !m.description.is_empty() && !m.author.is_empty() && !m.homepage.is_empty());
        assert_eq!(m.zone, Some(crate::Zone::Right));
        assert_eq!(m.entry, "plugin.wasm");
        assert!(m.i18n.contains_key("ar"));
        let (valid, problems) = crate::split_settings(m.settings.clone());
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(valid.len(), m.settings.len());
        for kind in [SettingKind::Bool, SettingKind::Number, SettingKind::Text, SettingKind::Choice] {
            assert!(valid.iter().any(|s| s.kind == kind), "example covers {kind:?}");
        }
        assert!(valid.iter().any(|s| matches!(s.choices.first(), Some(crate::SettingChoice::Plain(_)))));
        assert!(valid.iter().any(|s| s.min.is_some() && s.max.is_some() && s.step.is_some() && !s.unit.is_empty()));
    }

    #[test]
    fn contract_matches_the_wit() {
        let c = contract();
        assert!(c.package.starts_with("mandara:plugin@"));
        let names = |v: &[WitFunc]| v.iter().map(|f| f.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&c.exports), ["init", "bar", "panel", "sidebar", "on-event", "refresh-ms"]);
        assert_eq!(names(&c.imports), ["log", "now-unix-ms", "setting", "command", "locale", "translate"]);
        let cases = |v: &[WitCase]| v.iter().map(|k| k.name.clone()).collect::<Vec<_>>();
        assert_eq!(cases(&c.kinds), ["row", "column", "card", "text", "icon", "button", "meter", "spacer"]);
        assert_eq!(cases(&c.tones), ["normal", "muted", "accent", "error", "success", "warning"]);
        assert_eq!(c.exports[0].signature, "func() -> result<_, string>");
        assert_eq!(c.imports[2].signature, "func(key: string) -> option<string>");
        assert!(c.imports[3].doc.contains("commands"));
        assert!(c.node_fields.iter().any(|f| f.name == "action" && f.signature == "string"));
    }

    /// PLUGINS.md states the same facts as the code (Studio's Develop tab
    /// renders them from the code).
    #[test]
    fn guide_matches_the_runtime() {
        let guide = include_str!("../../../PLUGINS.md");
        let call = crate::wasm::CALL_BUDGET.as_millis();
        let init = crate::wasm::INIT_BUDGET.as_secs();
        assert!(guide.contains(&format!("{call} ms time budget ({init} s for `init`)")), "PLUGINS.md must state the real call budgets");
        assert!(guide.contains(&format!("At most {} commands can be queued per call", crate::wasm::MAX_QUEUED_COMMANDS)));
        assert!(guide.contains(&format!("rustup target add {WASM_TARGET}")));
        assert!(guide.contains(EXAMPLE_DIR) && guide.contains(REPOSITORY));
        assert!(guide.contains("--tab develop"));
        assert!(guide.contains(&format!("cp -r mandara-src/{WIT_DIR} my-plugin/wit")), "PLUGINS.md's quick start copies the WIT like quick_start()");
        for f in &contract().exports {
            assert!(guide.contains(&format!("`{}(", f.name)), "PLUGINS.md documents export {}", f.name);
        }
        for f in &contract().imports {
            assert!(guide.contains(&format!("`{}(", f.name)), "PLUGINS.md documents host import {}", f.name);
        }
    }

    #[test]
    fn quick_start_installs_into_the_dev_folder() {
        let steps = quick_start(std::path::Path::new("/home/u/.local/share/mandara/plugins-dev"));
        assert_eq!(steps.len(), 4);
        assert!(steps[3].commands.contains("/home/u/.local/share/mandara/plugins-dev/my-plugin/plugin.wasm"));
        assert!(steps.iter().all(|s| !s.commands.is_empty()));
        assert!(GUIDE_URL.starts_with("https://") && GUIDE_URL.ends_with("/PLUGINS.md"));
    }
}
