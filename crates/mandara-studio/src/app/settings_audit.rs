//! Settings audit: every setting Studio shows reaches code that reads it,
//! and every plugin setting the shell reads is shown in Studio.
//!
//! The scans read the consumer crates' sources (tests stripped), so a
//! setting that loses its reader, or a reader without a Studio control,
//! fails here instead of silently doing nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use mandara_config::ShellConfig;
use mandara_plugins::{PluginOrigin, PluginRegistry};
use serde_json::Value;

use crate::registry::{self, Control, Section, Setting};

/// Crates whose code reads settings (Studio itself only writes them).
const CONSUMER_CRATES: &[&str] = &[
    "mandara-ui/src",
    "mandara-services/src",
    "mandara-app/src",
    "mandara-config/src",
    "mandara-compositor/src",
    "mandara-core/src",
    "mandara-theme/src",
    "mandara-greeter/src",
    "mandara-plugins/src",
];

/// Plugin keys the shell reads that Studio deliberately doesn't show,
/// with the reason.
const UNEXPOSED_READS: &[(&str, &str, &str)] = &[
    // Retired by `general.units`; still read so old configs keep working.
    ("weather", "fahrenheit", "legacy flag replaced by General → Units"),
];

/// Plugin tables in the registry that belong to no built-in (the
/// third-party example used by the declared-settings tests).
const NON_BUILTIN_TABLES: &[&str] = &["moon-test", "weathervane"];

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// A source file with its `#[cfg(test)]` tail removed.
struct Source {
    path: PathBuf,
    text: String,
}

impl Source {
    fn stem(&self) -> &str {
        self.path.file_stem().and_then(|s| s.to_str()).unwrap_or_default()
    }
}

fn collect(dir: &Path, out: &mut Vec<Source>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let full = std::fs::read_to_string(&path).unwrap_or_default();
            out.push(Source { path, text: without_test_module(&full).to_string() });
        }
    }
}

/// `text` up to its `#[cfg(test)] mod …` (test-only helpers elsewhere in
/// the file stay: they are rare and harmless).
fn without_test_module(text: &str) -> &str {
    let mut from = 0;
    while let Some(at) = text[from..].find("#[cfg(test)]") {
        let at = from + at;
        let rest = text[at + "#[cfg(test)]".len()..].trim_start();
        if rest.starts_with("mod ") || rest.starts_with("pub mod ") || rest.starts_with("pub(crate) mod ") {
            return &text[..at];
        }
        from = at + 1;
    }
    text
}

fn sources() -> &'static [Source] {
    static ALL: OnceLock<Vec<Source>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut out = Vec::new();
        for krate in CONSUMER_CRATES {
            collect(&crates_dir().join(krate), &mut out);
        }
        assert!(out.len() > 50, "consumer sources not found under {}", crates_dir().display());
        out
    })
}

/// The string value of `const <name>: &str = "<value>";` in `text`.
fn const_value<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let at = text.find(&format!("const {name}: &str = \""))?;
    let rest = &text[at..];
    let start = rest.find('"')? + 1;
    let end = rest[start..].find('"')?;
    Some(&rest[start..start + end])
}

/// Resolves the plugin-id argument of a `setting(...)` call in `file`.
fn resolve_id(arg: &str, file: &Source) -> Option<String> {
    let arg = arg.trim();
    if let Some(lit) = arg.strip_prefix('"').and_then(|a| a.strip_suffix('"')) {
        return Some(lit.to_string());
    }
    // `crate::plugins::prayer::ID`, `super::prayer::ID`, `screenshot::ID`.
    if let Some(path) = arg.strip_suffix("::ID") {
        let module = path.rsplit("::").next()?;
        let owner = sources().iter().find(|s| s.stem() == module && const_value(&s.text, "ID").is_some())?;
        return const_value(&owner.text, "ID").map(str::to_string);
    }
    if arg.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
        return const_value(&file.text, arg).or_else(|| sources().iter().find_map(|s| const_value(&s.text, arg))).map(str::to_string);
    }
    // `let id = crate::plugins::screenshot::ID;`
    let at = file.text.find(&format!("let {arg} = "))?;
    let rest = &file.text[at + format!("let {arg} = ").len()..];
    let expr = &rest[..rest.find(';')?];
    resolve_id(expr, file)
}

/// The first two arguments of every `.setting(` call: `(id, key literal)`.
fn setting_reads() -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    for file in sources() {
        let mut text = file.text.as_str();
        while let Some(at) = text.find(".setting(") {
            text = &text[at + ".setting(".len()..];
            let Some(comma) = text.find(',') else { break };
            let id_arg = &text[..comma];
            let after = text[comma + 1..].trim_start();
            let Some(key) = after.strip_prefix('"').and_then(|k| k.split('"').next()) else { continue };
            if let Some(id) = resolve_id(id_arg, file) {
                out.insert((id, key.to_string()));
            }
        }
        // `settings.get(ID).and_then(|t| t.get("cities"))`.
        let mut text = file.text.as_str();
        while let Some(at) = text.find("settings.get(") {
            text = &text[at + "settings.get(".len()..];
            let Some(close) = text.find(')') else { break };
            let id_arg = &text[..close];
            let line_end = text.find('\n').unwrap_or(text.len());
            let line = &text[..line_end];
            if let (Some(id), Some(key_at)) = (resolve_id(id_arg, file), line.find(".get(\"")) {
                let key = line[key_at + ".get(\"".len()..].split('"').next().unwrap_or_default();
                out.insert((id, key.to_string()));
            }
        }
    }
    out
}

/// Files that belong to plugin `id`: they name it, define it, or use the
/// module constant that holds it.
fn plugin_files(id: &str) -> Vec<&'static Source> {
    let literal = format!("\"{id}\"");
    let modules: Vec<&str> = sources().iter().filter(|s| const_value(&s.text, "ID") == Some(id)).map(Source::stem).collect();
    sources()
        .iter()
        .filter(|s| s.text.contains(&literal) || modules.iter().any(|m| s.stem() == *m || s.text.contains(&format!("{m}::ID"))))
        .collect()
}

/// Whether some file of plugin `id` reads `key` (a literal, or a field of
/// a settings struct deserialized from the plugin's table).
fn plugin_reads(id: &str, key: &str) -> bool {
    let (lit, field) = (format!("\"{key}\""), format!("pub {key}:"));
    plugin_files(id).iter().any(|s| s.text.contains(&lit) || s.text.contains(&field))
}

fn builtin_registry() -> PluginRegistry {
    let mut r = PluginRegistry::default();
    mandara_ui::plugins::register_builtin(&mut r);
    r
}

/// `(plugin, key)` of a `/plugins/settings/<plugin>/<key>` path.
fn plugin_key(path: &str) -> Option<(&str, &str)> {
    path.strip_prefix("/plugins/settings/")?.split_once('/')
}

#[test]
fn every_plugin_setting_in_studio_is_read_by_its_plugin() {
    let mut dead = Vec::new();
    for s in registry::settings() {
        let Some((id, key)) = plugin_key(s.path) else { continue };
        if NON_BUILTIN_TABLES.contains(&id) {
            continue;
        }
        if !plugin_reads(id, key) {
            dead.push(s.path);
        }
    }
    assert!(dead.is_empty(), "Studio shows plugin settings no code reads: {dead:?}");
}

#[test]
fn every_plugin_setting_the_shell_reads_is_in_studio() {
    let shown: BTreeSet<(String, String)> = registry::settings().iter().filter_map(|s| plugin_key(s.path)).map(|(i, k)| (i.to_string(), k.to_string())).collect();
    let builtins: BTreeSet<String> = builtin_registry().manifests().filter(|m| m.origin == PluginOrigin::Builtin).map(|m| m.id.clone()).collect();
    let reads = setting_reads();
    assert!(reads.len() >= 45, "the scan found only {} reads; did the accessor change?", reads.len());
    let missing: Vec<_> = reads
        .iter()
        .filter(|(id, key)| builtins.contains(id) && !shown.contains(&(id.clone(), key.clone())))
        .filter(|(id, key)| !UNEXPOSED_READS.iter().any(|(i, k, _)| i == id && k == key))
        .collect();
    assert!(missing.is_empty(), "plugin settings read by the shell but missing from Studio: {missing:?}");
}

#[test]
fn every_builtin_plugin_settings_modal_lists_only_live_settings() {
    let registry = builtin_registry();
    let mut checked = 0;
    for m in registry.manifests().filter(|m| m.origin == PluginOrigin::Builtin) {
        for s in super::plugin_modal::plugin_settings(m) {
            checked += 1;
            if let Some((id, key)) = plugin_key(s.path) {
                assert_eq!(id, m.id, "{} lists another plugin's setting {}", m.id, s.path);
                assert!(plugin_reads(id, key), "{}: modal shows {} but the plugin never reads it", m.id, s.path);
            } else {
                // A shell setting the plugin drives: some consumer reads it.
                assert!(shell_setting_is_read(&s), "{}: modal shows {} but nothing reads it", m.id, s.path);
            }
        }
    }
    assert!(checked > 100, "only {checked} modal settings found");
}

#[test]
fn builtin_plugins_are_all_registered() {
    let registry = builtin_registry();
    let ids: Vec<&str> = registry.manifests().filter(|m| m.origin == PluginOrigin::Builtin).map(|m| m.id.as_str()).collect();
    assert!(ids.len() >= 38, "{} built-ins: {ids:?}", ids.len());
    // Every registry plugin table belongs to a built-in (or the example).
    for s in registry::settings() {
        if let Some((id, _)) = plugin_key(s.path) {
            assert!(ids.contains(&id) || NON_BUILTIN_TABLES.contains(&id), "{} has no plugin", s.path);
        }
    }
}

/// The screenshot plugin reads its table as one struct: every field is a
/// Studio control and every control a field.
#[test]
fn screenshot_struct_and_studio_agree() {
    let fields: BTreeSet<String> = toml::Table::try_from(mandara_config::workflow::ScreenshotSettings::default()).unwrap().keys().cloned().collect();
    let shown: BTreeSet<String> = registry::settings().iter().filter_map(|s| plugin_key(s.path)).filter(|(id, _)| *id == "screenshot").map(|(_, k)| k.to_string()).collect();
    let recorder: BTreeSet<String> = ["record_directory", "record_fps", "record_width"].map(String::from).into();
    let missing: Vec<_> = fields.difference(&shown).collect();
    assert!(missing.is_empty(), "screenshot fields without a Studio control: {missing:?}");
    let extra: Vec<_> = shown.difference(&fields).filter(|k| !recorder.contains(*k)).collect();
    assert!(extra.is_empty(), "Studio screenshot controls without a field: {extra:?}");
}

/// Every screenshot control round-trips into the struct the plugin reads.
#[test]
fn screenshot_controls_reach_the_struct() {
    use mandara_config::workflow::ScreenshotSettings;
    let cfg = ShellConfig::default();
    for s in registry::settings().iter().filter(|s| plugin_key(s.path).is_some_and(|(id, _)| id == "screenshot")) {
        let Some(value) = changed_value(s, &registry::get(&cfg, s)) else { continue };
        let next = registry::set(&cfg, s, value).unwrap_or_else(|e| panic!("{}: {e}", s.path));
        let (_, key) = plugin_key(s.path).unwrap();
        if ["record_directory", "record_fps", "record_width"].contains(&key) {
            continue;
        }
        assert_ne!(ScreenshotSettings::from_plugins(&next.plugins), ScreenshotSettings::default(), "{} doesn't change ScreenshotSettings", s.path);
    }
}

/// The last path segment as a field name.
fn field(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or_default()
}

/// Whether consumer code reads a shell (non-plugin-table) setting: its
/// field is accessed somewhere outside its definition.
fn shell_setting_is_read(s: &Setting) -> bool {
    if matches!(s.control, Control::Command { .. }) {
        return true;
    }
    let name = field(s.path);
    let access = format!(".{name}");
    sources().iter().any(|src| {
        src.text.match_indices(&access).any(|(at, _)| {
            let next = src.text[at + access.len()..].chars().next().unwrap_or(' ');
            !(next.is_alphanumeric() || next == '_')
        })
    })
}

/// Sections audited for dead shell settings (Look & feel, Desktop's
/// surfaces, Security & accounts, Extend, Mandara and Home's quick
/// settings).
const AUDITED: &[Section] = &[
    Section::Themes,
    Section::Typography,
    Section::Motion,
    Section::Bar,
    Section::Sidebar,
    Section::Launcher,
    Section::Dock,
    Section::WindowSwitcher,
    Section::Notifications,
    Section::Osd,
    Section::Wallpaper,
    Section::General, Section::Privacy, Section::LockScreen, Section::LoginScreen, Section::Users, Section::Plugins, Section::Compositor, Section::Profiles, Section::Performance, Section::Maintenance, Section::About];

#[test]
fn every_audited_shell_setting_has_a_reader() {
    let dead: Vec<&str> = registry::settings().iter().filter(|s| AUDITED.contains(&s.section) && plugin_key(s.path).is_none()).filter(|s| !shell_setting_is_read(s)).map(|s| s.path).collect();
    assert!(dead.is_empty(), "settings nothing reads: {dead:?}");
}

/// A valid value different from `current` for `s`, if the control has one.
pub(super) fn changed_value(s: &Setting, current: &Value) -> Option<Value> {
    match &s.control {
        Control::Toggle => Some(Value::from(!current.as_bool().unwrap_or(false))),
        Control::Slider { min, max, step, .. } | Control::OptionalSlider { min, max, step, .. } => {
            let now = current.as_f64();
            // Two steps above the minimum, or the maximum when that's current.
            let candidate = (min + 2.0 * step).min(*max);
            let v = if now.is_some_and(|n| (n - candidate).abs() < f64::EPSILON) { *max } else { candidate };
            Some(registry::integral(Value::from(v)))
        }
        Control::Number { min, max } => {
            let v = if current.as_f64() == Some(12.5_f64.clamp(*min, *max)) { 13.5_f64.clamp(*min, *max) } else { 12.5_f64.clamp(*min, *max) };
            Some(Value::from(v))
        }
        Control::Choice(choices) => choices.iter().map(|(raw, _)| registry::choice_value(raw)).find(|v| v != current && !v.is_null()),
        Control::Text { .. } | Control::OptionalText { .. } => Some(Value::from(format!("{}-audit", current.as_str().unwrap_or("x")))),
        Control::List { .. } => Some(Value::from(vec!["audit.example"])),
        Control::Time => Some(Value::from(if current.as_str() == Some("06:15") { "06:45" } else { "06:15" })),
        Control::Icon | Control::Command { .. } => None,
    }
}

/// Every registry setting writes, survives a flush to disk through the
/// `ConfigStore` and reads back the same (what a Studio edit + shell
/// restart does).
#[test]
fn every_setting_survives_a_config_store_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut failures = BTreeMap::new();
    for s in registry::settings() {
        let base = ShellConfig::default();
        let Some(value) = changed_value(s, &registry::get(&base, s)) else { continue };
        // Values the registry rejects (validated fields) never reach disk.
        let Ok(next) = registry::set(&base, s, value.clone()) else { continue };
        let expected = registry::get(&next, s);
        let mut store = mandara_config::ConfigStore::open(&path).unwrap();
        store.replace(next);
        store.flush().unwrap();
        let back = mandara_config::ConfigStore::open(&path).unwrap();
        let got = registry::get(back.get(), s);
        if !same(&got, &expected) {
            failures.insert(s.path, format!("wrote {expected}, read {got}"));
        }
    }
    assert!(failures.is_empty(), "settings that don't survive a restart: {failures:#?}");
}

/// JSON equality with float tolerance (TOML round-trips f32 through f64).
fn same(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() < 1e-4,
        _ => a == b,
    }
}

#[test]
fn home_quick_settings_resolve() {
    for path in super::nav::QUICK_SETTINGS {
        assert!(registry::by_path(path).is_some(), "Home quick setting {path} isn't a registry setting");
    }
}

// ── Profiles & backup ───────────────────────────────────────────────────────

/// Every setting survives an export / import (changes only and full), as
/// Studio → Profiles & backup does it.
#[test]
fn every_setting_survives_export_and_import() {
    use mandara_config::profiles::{ExportOptions, export_text, import_text};
    let mut failures = BTreeMap::new();
    for s in registry::settings() {
        let base = ShellConfig::default();
        let Some(value) = changed_value(s, &registry::get(&base, s)) else { continue };
        let Ok(next) = registry::set(&base, s, value) else { continue };
        let expected = registry::get(&next, s);
        for changes_only in [true, false] {
            let (text, _) = export_text(&next, ExportOptions { changes_only, redact: false }).unwrap();
            match import_text(&text) {
                Ok(back) if same(&registry::get(&back, s), &expected) => {}
                Ok(back) => {
                    failures.insert((s.path, changes_only), format!("exported {expected}, imported {}", registry::get(&back, s)));
                }
                Err(e) => {
                    failures.insert((s.path, changes_only), format!("{e:#}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "settings lost by export / import: {failures:#?}");
}

/// A setting switched back to "unset" (optional values) survives a
/// changes-only export too.
#[test]
fn unset_optionals_survive_a_changes_only_export() {
    use mandara_config::profiles::{ExportOptions, export_text, import_text};
    let mut failures = Vec::new();
    for s in registry::settings().iter().filter(|s| matches!(s.control, Control::OptionalSlider { .. } | Control::OptionalText { .. })) {
        let base = ShellConfig::default();
        if registry::get(&base, s).is_null() {
            continue; // already unset by default: nothing to lose
        }
        let Ok(next) = registry::set(&base, s, Value::Null) else { continue };
        let (text, _) = export_text(&next, ExportOptions { changes_only: true, redact: false }).unwrap();
        let back = import_text(&text).unwrap();
        if !registry::get(&back, s).is_null() {
            failures.push(s.path);
        }
    }
    assert!(failures.is_empty(), "unset values come back set after import: {failures:?}");
}

/// A look profile carries exactly the look paths: applying it to defaults
/// restores look settings and leaves the rest alone.
#[test]
fn look_profiles_carry_only_the_look() {
    use mandara_config::profiles::{LOOK_PATHS, ProfileScope, apply, snapshot};
    let in_look = |path: &str| {
        let dotted = path.trim_start_matches('/').replace('/', ".");
        LOOK_PATHS.iter().any(|p| dotted == *p || dotted.starts_with(&format!("{p}.")))
    };
    let mut checked = 0;
    for s in registry::settings() {
        let base = ShellConfig::default();
        let Some(value) = changed_value(s, &registry::get(&base, s)) else { continue };
        let Ok(next) = registry::set(&base, s, value) else { continue };
        let profile = snapshot(&next, "Audit", "", ProfileScope::Look, "").unwrap();
        let applied = apply(&base, &profile).unwrap();
        let carried = same(&registry::get(&applied, s), &registry::get(&next, s));
        assert_eq!(carried, in_look(s.path), "{}: look profile {} it", s.path, if carried { "carries" } else { "drops" });
        let full = snapshot(&next, "Audit", "", ProfileScope::Full, "").unwrap();
        assert!(same(&registry::get(&apply(&base, &full).unwrap(), s), &registry::get(&next, s)), "{}: full profile drops it", s.path);
        checked += 1;
    }
    assert!(checked > 300, "only {checked} settings checked");
}

/// Resetting a page's section puts every one of its settings back.
#[test]
fn page_reset_restores_every_setting_of_the_section() {
    use mandara_config::profiles::reset_section;
    for s in registry::settings() {
        let base = ShellConfig::default();
        let Some(value) = changed_value(s, &registry::get(&base, s)) else { continue };
        let Ok(next) = registry::set(&base, s, value) else { continue };
        let section = s.path.trim_start_matches('/').split('/').next().unwrap();
        let reset = reset_section(&next, section).unwrap();
        assert!(same(&registry::get(&reset, s), &registry::get(&base, s)), "{}: reset of [{section}] doesn't restore it", s.path);
    }
}

// ── Compositor ──────────────────────────────────────────────────────────────

/// Every compositor drop-in, as the shell would write it.
fn emitted(cfg: &ShellConfig) -> Vec<(mandara_config::compositor::Target, String)> {
    use mandara_config::compositor::{ManagedConfig, Target, emit_hyprland_lua};
    let effective = cfg.effective_compositor();
    let borders = cfg.integrations.compositor_borders.then(|| super::compositor_page::preview_borders(&mandara_theme::engine::default_palette(true)));
    let mut out: Vec<_> = Target::ALL.into_iter().map(|t| (t, super::compositor_page::preview_text(t, &effective, borders.clone()))).collect();
    // Both Hyprland formats (hyprlang and 0.55+ Lua) must carry it.
    let m = ManagedConfig { config: &effective, borders, existing_chords: Default::default(), mandara_bin: "mandara".into(), version: Some((26, 4)), user_autostarts: false };
    out.push((Target::Hyprland, emit_hyprland_lua(&m).text));
    out.push((Target::Hyprland, mandara_config::compositor::emit_hyprland_conf(&m).text));
    out
}

/// Which drop-ins a compositor registry group must change.
fn group_targets(group: &str) -> &'static [mandara_config::compositor::Target] {
    use mandara_config::compositor::Target;
    match group {
        "MangoWM" => &[Target::Mango],
        "Niri" => &[Target::Niri],
        g if g == "Hyprland" || g == crate::registry::system_settings::HYPR_LAYOUT => &[Target::Hyprland],
        _ => &Target::ALL,
    }
}

/// Every Compositor-page setting changes the drop-in of each compositor it
/// claims to drive (the only exception is the master switch, which gates
/// the writing itself).
#[test]
fn every_compositor_setting_reaches_its_drop_ins() {
    let base = ShellConfig::default();
    let before = emitted(&base);
    let mut checked = 0;
    for s in registry::settings().iter().filter(|s| s.section == Section::Compositor) {
        if s.path == "/compositor/manage" {
            continue;
        }
        let Some(value) = changed_value(s, &registry::get(&base, s)) else { continue };
        let next = registry::set(&base, s, value.clone()).unwrap_or_else(|e| panic!("{}: {e}", s.path));
        let after = emitted(&next);
        for (t, text) in &after {
            if *t == mandara_config::compositor::Target::Niri {
                kdl::KdlDocument::parse_v1(text).unwrap_or_else(|e| panic!("{} = {value}: niri drop-in isn't valid KDL: {e:?}\n{text}", s.path));
            } else {
                let (open, close) = (text.matches('{').count(), text.matches('}').count());
                assert_eq!(open, close, "{} = {value}: unbalanced braces in the {t:?} drop-in\n{text}", s.path);
            }
        }
        for target in group_targets(s.group) {
            // Niri only has an "off" switch for animations: "on" keeps the
            // user's own block (documented in the emitter).
            if *target == mandara_config::compositor::Target::Niri && s.path == "/compositor/look/animations" && value == Value::from(true) {
                continue;
            }
            // Niri can't blur before 26.04: the version-gated note counts.
            let changed = before.iter().zip(&after).filter(|((t, _), _)| t == target).any(|((_, a), (_, b))| a != b);
            assert!(changed, "{} = {value} doesn't change the {target:?} drop-in", s.path);
        }
        checked += 1;
    }
    assert!(checked >= 35, "only {checked} compositor settings checked");
}

/// The rules editors (window rules, tag rules, Niri columns / outputs)
/// write config the emitters turn into lines.
#[test]
fn rule_editors_reach_the_drop_ins() {
    use mandara_config::compositor::{OutputRule, TagRule, Target, WindowRule};
    let mut cfg = ShellConfig::default();
    cfg.compositor.window_rules.push(WindowRule { app_id: Some("^audit$".into()), floating: Some(true), focused_opacity: Some(0.9), ..Default::default() });
    for (n, (layout, _)) in super::compositor_page::MANGO_LAYOUTS.iter().enumerate().take(9) {
        cfg.compositor.tag_rules.push(TagRule { id: n as u32 + 1, layout: Some((*layout).into()), master_factor: Some(0.6), ..Default::default() });
    }
    cfg.compositor.niri.preset_column_widths = vec![0.5, 0.75];
    cfg.compositor.niri.outputs.push(OutputRule { name: "AUDIT-1".into(), scale: Some(1.25), ..Default::default() });
    let out = emitted(&cfg);
    let text = |t: Target| out.iter().filter(|(x, _)| *x == t).map(|(_, s)| s.as_str()).collect::<Vec<_>>().join("\n");
    let mango = text(Target::Mango);
    assert!(mango.contains("window_rule=is_floating:1,focused_opacity:0.90,app_id:^audit$"), "{mango}");
    for (n, (layout, _)) in super::compositor_page::MANGO_LAYOUTS.iter().enumerate().take(9) {
        assert!(mango.contains(&format!("tag_rule=id:{},layout_name:{layout},master_factor:0.60", n + 1)), "{layout}\n{mango}");
    }
    let niri = text(Target::Niri);
    assert!(niri.contains("app-id=\"^audit$\"") && niri.contains("proportion 0.75") && niri.contains("output \"AUDIT-1\""), "{niri}");
    let hypr = text(Target::Hyprland);
    assert!(hypr.contains("match:class ^audit$") && hypr.contains("float"), "{hypr}");
}

/// MangoWM's own checker (`mango -p`, read-only) accepts the drop-in with
/// every Compositor setting changed at once, every tag layout and a full
/// window rule. Skipped where MangoWM isn't installed.
#[test]
fn mango_accepts_every_studio_option() {
    use mandara_config::compositor::{TagRule, Target, WindowRule};
    if std::process::Command::new("mango").arg("-v").output().is_err() {
        return;
    }
    let mut cfg = ShellConfig::default();
    for s in registry::settings().iter().filter(|s| s.section == Section::Compositor && group_targets(s.group).contains(&Target::Mango)) {
        let Some(value) = changed_value(s, &registry::get(&cfg, s)) else { continue };
        if let Ok(next) = registry::set(&cfg, s, value) {
            cfg = next;
        }
    }
    cfg.compositor.window_rules.push(WindowRule {
        app_id: Some("^audit$".into()),
        title: Some("Audit".into()),
        floating: Some(true),
        global: Some(true),
        overlay: Some(true),
        focused_opacity: Some(0.9),
        unfocused_opacity: Some(0.8),
        no_blur: Some(true),
        no_shadow: Some(true),
        confine_pointer: Some(true),
        force_tiled_state: Some(true),
    });
    let dir = tempfile::tempdir().unwrap();
    for chunk in super::compositor_page::MANGO_LAYOUTS.chunks(9) {
        let mut c = cfg.clone();
        c.compositor.tag_rules = chunk.iter().enumerate().map(|(n, (l, _))| TagRule { id: n as u32 + 1, layout: Some((*l).into()), master_factor: Some(0.6), scroller_default_proportion: Some(0.8), master_count: Some(2), ..Default::default() }).collect();
        let text = emitted(&c).into_iter().find(|(t, _)| *t == Target::Mango).unwrap().1;
        let path = dir.path().join("mango.conf");
        std::fs::write(&path, &text).unwrap();
        let out = std::process::Command::new("mango").arg("-p").arg("-c").arg(&path).output().unwrap();
        let report = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
        assert!(!report.contains("ERROR"), "mango -p rejected Studio's drop-in:\n{report}\n{text}");
    }
}

/// The scanners themselves: a made-up key is unread, real reads resolve
/// through literals, module constants and `let id = …::ID`.
#[test]
fn scanner_sees_reads_and_misses() {
    assert!(!plugin_reads("weather", "no_such_audit_key"));
    assert!(plugin_reads("prayer-times", "location_name"), "literal id + key");
    assert!(plugin_reads("pomodoro", "focus_minutes"), "key passed through a helper");
    assert!(plugin_reads("screenshot", "capture_on_release"), "struct field read");
    let reads = setting_reads();
    for (id, key) in [("screenshot", "record_fps"), ("weather", "animate"), ("prayer-times", "hijri_offset"), ("world-clock", "cities"), ("pomodoro", "long_break_every")] {
        assert!(reads.contains(&(id.to_string(), key.to_string())), "{id}/{key} not resolved");
    }
    let fake = Setting { section: Section::General, group: "", path: "/general/no_such_audit_field", label: "", help: "", control: Control::Toggle, default: None };
    assert!(!shell_setting_is_read(&fake));
}
