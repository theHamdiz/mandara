//! The compliance check every community plugin release passes before it
//! installs or updates (`mandara msg plugins vet <dir>` runs it on a
//! folder). It is a handful of cheap checks on the unpacked folder, run in
//! the real host with its normal time budgets, stopping at the first
//! failure:
//!
//! * **Broken:** plugin.toml doesn't parse or has problems; the component
//!   is missing, isn't a component or doesn't compile or start; `init` or a
//!   render traps or runs past its budget.
//! * **Malicious:** its id or name poses as a built-in plugin; it imports
//!   interfaces the host doesn't link; it queues commands while loading or
//!   rendering, before any user event.
//! * **Placeholder:** the unchanged template or example, or a bar item,
//!   panel and sidebar card that all show nothing.
//!
//! A failing release is flagged per version by the marketplace
//! ([`mandara_services::marketplace::Vetting`]); the user can still install
//! it anyway.

use std::path::Path;

use mandara_i18n::{t, tf};
use mandara_services::marketplace::{Class, Finding};

use crate::wasm::mandara::plugin::types::Palette;
use crate::wasm::{self, Kind, ManifestFile, View, WasmPlugin};
use crate::{PluginOrigin, PluginRegistry};

/// Import namespaces the host links: the plugin world, and WASI preview 2
/// with an empty context (no files, network, environment or arguments).
/// Anything else is an attempt to reach outside the sandbox.
const LINKED_IMPORTS: &[&str] = &["mandara:plugin/", "wasi:io/", "wasi:clocks/", "wasi:random/", "wasi:cli/", "wasi:filesystem/", "wasi:sockets/"];
/// A WebAssembly binary starts with this magic number…
const WASM_MAGIC: &[u8] = b"\0asm";
/// …then a version and a layer; the layer (these bytes) is 1 for a
/// component and 0 for a core module.
const LAYER_BYTES: std::ops::Range<usize> = 6..8;
const COMPONENT_LAYER: [u8; 2] = [1, 0];
/// The quick start's template folder and crate name (PLUGINS.md).
const TEMPLATE_NAME: &str = "my-plugin";
/// The palette renders are probed with, as `#rrggbb` (any palette works;
/// in the shell plugins get the live one): a dark surface, a teal primary.
const PROBE_PRIMARY: &str = "#80cbc4";
const PROBE_ON_PRIMARY: &str = "#00201d";
const PROBE_SURFACE: &str = "#101418";
const PROBE_ON_SURFACE: &str = "#e0e3e8";
const PROBE_ON_SURFACE_VARIANT: &str = "#c0c7cd";
const PROBE_CONTAINER: &str = "#1c2024";
const PROBE_ERROR: &str = "#ffb4ab";
const PROBE_SUCCESS: &str = "#7dd3a0";
const PROBE_WARNING: &str = "#f5c26b";

fn probe_palette() -> Palette {
    Palette {
        dark: true,
        primary: PROBE_PRIMARY.into(),
        on_primary: PROBE_ON_PRIMARY.into(),
        surface: PROBE_SURFACE.into(),
        on_surface: PROBE_ON_SURFACE.into(),
        on_surface_variant: PROBE_ON_SURFACE_VARIANT.into(),
        container: PROBE_CONTAINER.into(),
        error: PROBE_ERROR.into(),
        success: PROBE_SUCCESS.into(),
        warning: PROBE_WARNING.into(),
    }
}

/// The built-in plugins' ids (what community plugins may not pose as).
pub fn builtin_ids(registry: &PluginRegistry) -> Vec<String> {
    registry.manifests().filter(|m| m.origin == PluginOrigin::Builtin).map(|m| m.id.clone()).collect()
}

/// Mandara's own repository (`owner/repo`), whose example releases are
/// the original, not a copy.
fn official_repo() -> &'static str {
    crate::authoring::REPOSITORY.split("github.com/").nth(1).unwrap_or_default().trim_end_matches('/')
}

/// Checks the plugin folder `dir`. `reserved_ids` are the built-in plugin
/// ids ([`builtin_ids`]); `repo` is the repository it comes from, if any.
pub fn vet(dir: &Path, reserved_ids: &[&str], repo: Option<&str>) -> Result<(), Finding> {
    let broken = |reason: String| Finding::new(Class::Broken, reason);
    let text = std::fs::read_to_string(dir.join("plugin.toml")).map_err(|e| broken(tf("plugin.toml can't be read: {error}", &[("error", &e)])))?;
    let m = ManifestFile::parse(&text).map_err(|e| broken(tf("plugin.toml is invalid: {error}", &[("error", &e)])))?;
    if let Some(problem) = m.problems().into_iter().next() {
        return Err(broken(tf("plugin.toml is invalid: {error}", &[("error", &problem)])));
    }
    impersonation(&m, reserved_ids)?;
    unchanged_scaffold(&m, repo)?;

    let entry = dir.join(&m.entry);
    let bytes = std::fs::read(&entry).map_err(|_| broken(tf("{file} is missing", &[("file", &m.entry)])))?;
    if !bytes.starts_with(WASM_MAGIC) || bytes.get(LAYER_BYTES) != Some(&COMPONENT_LAYER[..]) {
        return Err(broken(tf("{file} isn't a WebAssembly component (build it for wasm32-wasip2)", &[("file", &m.entry)])));
    }
    let compile_error = |e: anyhow::Error| broken(tf("{file} doesn't compile: {error}", &[("file", &m.entry), ("error", &format!("{e:#}"))]));
    let component = wasm::load_component(&entry).map_err(compile_error)?;
    let engine = wasm::engine().map_err(compile_error)?;
    if let Some(name) = unlinked_import(component.component_type().imports(engine).map(|(name, _)| name)) {
        return Err(Finding::new(Class::Malicious, tf("It asks for {interface}, which plugins can't use", &[("interface", &name)])));
    }

    // Every command is captured (none runs here), so any attempt shows.
    let settings = crate::split_settings(m.settings.clone()).0.iter().map(|s| (s.key.clone(), s.default_json())).collect();
    let mut plugin = WasmPlugin::instantiate(&m.id, dir, &component, settings, true).map_err(|e| broken(tf("It fails to start: {error}", &[("error", &format!("{e:#}"))])))?;
    behaved(&mut plugin)?;
    let theme = probe_palette();
    let mut shows_something = false;
    for render in [WasmPlugin::bar, WasmPlugin::panel, WasmPlugin::sidebar] {
        let view = render(&mut plugin, &theme);
        behaved(&mut plugin)?;
        shows_something |= view.as_ref().is_some_and(has_content);
    }
    if !shows_something {
        return Err(Finding::new(Class::Placeholder, t("It shows nothing: its bar item, panel and sidebar card are all empty")));
    }
    Ok(())
}

/// After a call: it neither trapped nor ran past its budget, and queued
/// no command (no user event has happened).
fn behaved(plugin: &mut WasmPlugin) -> Result<(), Finding> {
    if let Some(reason) = plugin.disabled.clone() {
        return Err(Finding::new(Class::Broken, tf("It crashes or runs past its time budget: {error}", &[("error", &reason)])));
    }
    match plugin.take_commands().first() {
        Some(command) => Err(Finding::new(Class::Malicious, tf("It runs commands without being asked ({command})", &[("command", command)]))),
        None => Ok(()),
    }
}

/// The id or name of a built-in plugin, compared case-insensitively (a
/// name's spaces count as `-`).
fn impersonation(m: &ManifestFile, reserved_ids: &[&str]) -> Result<(), Finding> {
    let name = m.name.trim().replace(' ', "-");
    match reserved_ids.iter().find(|id| id.eq_ignore_ascii_case(&m.id) || id.eq_ignore_ascii_case(&name)) {
        Some(id) => Err(Finding::new(Class::Malicious, tf("It poses as Mandara's built-in {plugin} plugin", &[("plugin", id)]))),
        None => Ok(()),
    }
}

/// The quick-start template, or the example republished as it is.
fn unchanged_scaffold(m: &ManifestFile, repo: Option<&str>) -> Result<(), Finding> {
    if m.id.eq_ignore_ascii_case(TEMPLATE_NAME) || m.name.trim().eq_ignore_ascii_case(TEMPLATE_NAME) {
        return Err(Finding::new(Class::Placeholder, tf("It's the unchanged plugin template (still named {name})", &[("name", &TEMPLATE_NAME)])));
    }
    let example = crate::authoring::example_manifest();
    let copy = m.description.trim() == example.description && (m.id == example.id || m.name.trim() == example.name);
    if copy && repo != Some(official_repo()) {
        return Err(Finding::new(Class::Placeholder, tf("It's Mandara's {example} example, republished unchanged", &[("example", &example.id)])));
    }
    Ok(())
}

/// The first import outside [`LINKED_IMPORTS`].
fn unlinked_import<'a>(mut names: impl Iterator<Item = &'a str>) -> Option<String> {
    names.find(|name| !LINKED_IMPORTS.iter().any(|ns| name.starts_with(ns))).map(str::to_string)
}

/// Some visible text, icon or meter.
fn has_content(view: &View) -> bool {
    view.nodes.iter().any(|n| match n.kind {
        Kind::Text | Kind::Button | Kind::Icon => !n.text.trim().is_empty(),
        Kind::Meter => true,
        Kind::Row | Kind::Column | Kind::Card | Kind::Spacer => false,
    })
}

/// The verdict as JSON (`mandara msg plugins vet`): `{"ok": true}` or
/// `{"ok": false, "class": "broken", "reason": "…"}`.
pub fn verdict_json(verdict: &Result<(), Finding>) -> serde_json::Value {
    match verdict {
        Ok(()) => serde_json::json!({ "ok": true }),
        Err(f) => serde_json::json!({ "ok": false, "class": f.class, "label": f.class.label(), "reason": f.reason }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_outside_the_world_are_named() {
        let linked = ["mandara:plugin/host@0.1.0", "mandara:plugin/types@0.1.0", "wasi:io/streams@0.2.6", "wasi:cli/stderr@0.2.6", "wasi:filesystem/preopens@0.2.6"];
        assert_eq!(unlinked_import(linked.into_iter()), None);
        assert_eq!(unlinked_import(["wasi:io/poll@0.2.6", "wasi:http/outgoing-handler@0.2.6"].into_iter()).as_deref(), Some("wasi:http/outgoing-handler@0.2.6"));
        assert!(unlinked_import(["evil:net/raw"].into_iter()).is_some());
    }

    #[test]
    fn manifest_checks() {
        let parse = |s: &str| ManifestFile::parse(s).unwrap();
        let reserved = ["weather", "media-player"];
        assert_eq!(impersonation(&parse("id = \"Weather\"\nname = \"Sky\""), &reserved).unwrap_err().class, Class::Malicious);
        assert!(impersonation(&parse("id = \"sky\"\nname = \"Media Player\""), &reserved).is_err());
        assert!(impersonation(&parse("id = \"weather-pro\"\nname = \"Weather Pro\""), &reserved).is_ok());
        assert_eq!(unchanged_scaffold(&parse("id = \"my-plugin\"\nname = \"x\""), None).unwrap_err().class, Class::Placeholder);
        let example = crate::authoring::example_manifest();
        let copy = parse(&format!("id = \"lunar\"\nname = \"{}\"\ndescription = \"{}\"", example.name, example.description));
        assert!(unchanged_scaffold(&copy, Some("someone/mandara-moon")).is_err());
        assert!(unchanged_scaffold(&copy, Some(official_repo())).is_ok(), "the original isn't a copy");
        assert!(official_repo().contains('/'), "owner/repo from the crate's repository URL");
    }
}
