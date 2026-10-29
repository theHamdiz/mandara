//! Plugin manifests, instances and the registry.
//!
//! A plugin contributes any of: a bar item (an `AnyView` rendered inside a
//! host-drawn chip), a panel (an `AnyView` the host morphs out of that chip),
//! and documented [`CommandSpec`]s. Shipped plugins are compiled in and
//! enabled by default (a few are opt-in, see [`PluginManifest::opt_in`]);
//! third-party plugins are discovered from
//! `~/.config/mandara/plugins/<id>/plugin.toml`.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{AnyView, App, Global, Pixels, Size, Window};
use serde::{Deserialize, Serialize};

use crate::command::{CommandRegistry, CommandSpec};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PluginOrigin {
    /// Shipped with Mandara.
    Builtin,
    /// Installed by the user from a directory.
    ThirdParty { path: PathBuf },
}

/// Default bar zone for a plugin's bar item.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Zone {
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub version: String,
    pub origin: PluginOrigin,
    pub enabled_by_default: bool,
    /// Where the bar item goes unless the user places it elsewhere.
    pub zone: Option<Zone>,
    /// Third-party name / description per language code (`[i18n.ar]` in
    /// plugin.toml). Built-in plugins are translated by Mandara's catalogs.
    #[serde(default)]
    pub translations: std::collections::BTreeMap<String, ManifestText>,
    /// Third-party permissions from plugin.toml (`commands`).
    #[serde(default)]
    pub permissions: Vec<String>,
    /// Third-party project page.
    #[serde(default)]
    pub homepage: String,
    /// Third-party settings declared with `[[settings]]` (stored in
    /// `[plugins.settings.<id>]`, read with `host::setting`).
    #[serde(default)]
    pub settings: Vec<ManifestSetting>,
}

/// Control type of a [`ManifestSetting`].
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingKind {
    /// On / off switch.
    #[default]
    Bool,
    /// A number: a slider with `min` and `max`, otherwise a text field.
    Number,
    /// Free text.
    Text,
    /// One of `choices`.
    Choice,
}

/// One option of a choice setting: `"north"` or `{ value = "north", label = "Northern" }`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SettingChoice {
    Plain(String),
    Labeled { value: String, label: String },
}

impl SettingChoice {
    pub fn value(&self) -> &str {
        match self {
            SettingChoice::Plain(v) | SettingChoice::Labeled { value: v, .. } => v,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            SettingChoice::Plain(v) => v,
            SettingChoice::Labeled { label, .. } => label,
        }
    }
}

/// A setting a third-party plugin declares in plugin.toml:
///
/// ```toml
/// [[settings]]
/// key = "hemisphere"
/// type = "choice"
/// label = "Hemisphere"
/// help = "Which side of the moon is lit"
/// default = "north"
/// choices = [{ value = "north", label = "Northern" }, { value = "south", label = "Southern" }]
/// ```
///
/// Studio renders it in the plugin's settings modal and stores the value in
/// `[plugins.settings.<id>]`, where the plugin reads it with
/// `host::setting(key)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ManifestSetting {
    pub key: String,
    #[serde(rename = "type", default)]
    pub kind: SettingKind,
    /// Shown in Studio (falls back to the key); translated through the
    /// plugin's `locales/` catalog.
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub help: String,
    #[serde(default)]
    pub default: Option<toml::Value>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub step: Option<f64>,
    /// Number suffix, e.g. `" min"`.
    #[serde(default)]
    pub unit: String,
    #[serde(default)]
    pub choices: Vec<SettingChoice>,
}

/// Longest accepted setting key.
const MAX_SETTING_KEY: usize = 64;

impl ManifestSetting {
    /// Why this declaration can't be shown, if it can't.
    pub fn validate(&self) -> Result<(), String> {
        let key_ok = !self.key.is_empty() && self.key.len() <= MAX_SETTING_KEY && self.key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
        if !key_ok {
            return Err(format!("setting key `{}` must be lowercase letters, digits, - or _", self.key));
        }
        if self.kind == SettingKind::Choice && self.choices.is_empty() {
            return Err(format!("choice setting `{}` has no choices", self.key));
        }
        if let (Some(min), Some(max)) = (self.min, self.max)
            && min >= max
        {
            return Err(format!("setting `{}`: min must be below max", self.key));
        }
        if self.step.is_some_and(|s| s <= 0.0) {
            return Err(format!("setting `{}`: step must be positive", self.key));
        }
        let typed = match (&self.default, self.kind) {
            (None, _) => true,
            (Some(toml::Value::Boolean(_)), SettingKind::Bool) => true,
            (Some(toml::Value::Integer(_) | toml::Value::Float(_)), SettingKind::Number) => true,
            (Some(toml::Value::String(_)), SettingKind::Text) => true,
            (Some(toml::Value::String(v)), SettingKind::Choice) => self.choices.iter().any(|c| c.value() == v),
            _ => false,
        };
        if !typed {
            return Err(format!("setting `{}`: default doesn't match its type", self.key));
        }
        Ok(())
    }

    /// The value used until the user changes it.
    pub fn default_json(&self) -> serde_json::Value {
        if let Some(v) = self.default.as_ref().and_then(|v| serde_json::to_value(v).ok()) {
            return v;
        }
        match self.kind {
            SettingKind::Bool => serde_json::Value::Bool(false),
            SettingKind::Number => serde_json::Value::from(self.min.unwrap_or(0.0)),
            SettingKind::Text => serde_json::Value::String(String::new()),
            SettingKind::Choice => serde_json::Value::String(self.choices.first().map(|c| c.value().to_string()).unwrap_or_default()),
        }
    }
}

/// Splits declarations into the valid ones and why the others are skipped
/// (invalid, or a duplicate key).
pub fn split_settings(settings: Vec<ManifestSetting>) -> (Vec<ManifestSetting>, Vec<String>) {
    let mut seen = std::collections::BTreeSet::new();
    let (mut valid, mut problems) = (Vec::new(), Vec::new());
    for s in settings {
        match s.validate() {
            Ok(()) if seen.insert(s.key.clone()) => valid.push(s),
            Ok(()) => problems.push(format!("duplicate setting `{}` ignored", s.key)),
            Err(e) => problems.push(e),
        }
    }
    (valid, problems)
}

/// Keeps the valid declarations, reporting the others.
pub fn valid_settings(plugin: &str, settings: Vec<ManifestSetting>) -> Vec<ManifestSetting> {
    let (valid, problems) = split_settings(settings);
    for p in problems {
        tracing::warn!("plugin `{plugin}`: {p}");
    }
    valid
}

/// A plugin's name and description in one language.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ManifestText {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
}

impl PluginManifest {
    /// Shipped, but off until the user enables it (e.g. faith-specific
    /// plugins such as prayer times).
    pub fn opt_in(mut self) -> Self {
        self.enabled_by_default = false;
        self
    }

    pub fn builtin(id: &str, name: &str, description: &str, zone: Option<Zone>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            author: "Mandara".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            origin: PluginOrigin::Builtin,
            enabled_by_default: true,
            zone,
            translations: Default::default(),
            permissions: Vec::new(),
            homepage: String::new(),
            settings: Vec::new(),
        }
    }

    /// The name in the interface language (English fallback).
    pub fn display_name(&self) -> String {
        let lang = mandara_i18n::language();
        match self.origin {
            PluginOrigin::Builtin => mandara_i18n::tr(&self.name),
            PluginOrigin::ThirdParty { .. } => self.translations.get(lang.code()).map(|t| t.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| self.name.clone()),
        }
    }

    /// The description in the interface language (English fallback).
    pub fn display_description(&self) -> String {
        let lang = mandara_i18n::language();
        match self.origin {
            PluginOrigin::Builtin => mandara_i18n::tr(&self.description),
            PluginOrigin::ThirdParty { .. } => self.translations.get(lang.code()).map(|t| t.description.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| self.description.clone()),
        }
    }

    pub fn disabled_by_default(mut self) -> Self {
        self.enabled_by_default = false;
        self
    }
}

/// Per-surface context shared from the host to a plugin: the output it
/// lives on and the live morph progress of its panel.
#[derive(Clone, Default)]
pub struct PanelCx {
    progress: Rc<Cell<f32>>,
    monitor: Rc<str>,
}

/// Per-element reveal parameters for staggered panel entrances.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Reveal {
    pub opacity: f32,
    /// Vertical offset in pixels (slides up into place).
    pub offset: f32,
}

impl PanelCx {
    pub fn new() -> Self {
        Self::default()
    }

    /// Context for a surface on output `monitor` (compositor output name).
    pub fn for_monitor(monitor: &str) -> Self {
        Self { progress: Rc::default(), monitor: Rc::from(monitor) }
    }

    /// Compositor output name of the surface (empty if unknown).
    pub fn monitor(&self) -> &str {
        &self.monitor
    }

    /// A fresh progress cell on the same monitor (one per panel).
    pub fn fork(&self) -> Self {
        Self { progress: Rc::default(), monitor: self.monitor.clone() }
    }

    /// Morph progress of the panel, `0` (collapsed) to `1` (open).
    pub fn progress(&self) -> f32 {
        self.progress.get()
    }

    /// Host-side setter, called every frame.
    pub fn set_progress(&self, progress: f32) {
        self.progress.set(progress.clamp(0.0, 1.0));
    }

    /// Staggered entrance for the `index`-th block of a panel: each block
    /// starts a little later than the previous one (capped so deep lists
    /// still begin promptly), slides up 10px with a smoothstep ease, and is
    /// always fully revealed once the morph completes.
    pub fn reveal(&self, index: usize) -> Reveal {
        let start = (0.3 + index as f32 * 0.05).min(0.7);
        let t = ((self.progress() - start) / (1.0 - start)).clamp(0.0, 1.0);
        let eased = t * t * (3.0 - 2.0 * t);
        Reveal { opacity: eased, offset: (1.0 - eased) * 10.0 }
    }
}

/// A panel contributed by a plugin.
pub struct PanelSpec {
    pub view: AnyView,
    /// Desired panel size; plugins may change it at any time (e.g. a Wi-Fi
    /// list growing) and the host springs to the new size.
    pub size: Rc<Cell<Size<Pixels>>>,
}

pub type ClickHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// What a plugin instantiates for one bar surface.
pub struct PluginInstance {
    pub bar: Option<AnyView>,
    pub panel: Option<PanelSpec>,
    /// A card for the left sidebar drawer.
    pub sidebar: Option<AnyView>,
    /// Primary click when the plugin has no panel.
    pub on_click: Option<ClickHandler>,
    /// Right-click quick action (mute, toggle radio, play/pause, …).
    pub on_secondary_click: Option<ClickHandler>,
}

pub type BuildFn = Rc<dyn Fn(&mut Window, &mut App, PanelCx) -> PluginInstance>;

pub struct PluginFactory {
    pub manifest: PluginManifest,
    pub build: BuildFn,
    /// Bumped each time this plugin is (re)registered, so hosts can tell a
    /// hot-reloaded plugin from the instance they already hold.
    pub generation: u64,
}

impl PluginInstance {
    pub fn empty() -> Self {
        Self { bar: None, panel: None, sidebar: None, on_click: None, on_secondary_click: None }
    }
}

/// Registry of every known plugin and the command catalog they contribute.
#[derive(Default)]
pub struct PluginRegistry {
    factories: Vec<PluginFactory>,
    pub commands: CommandRegistry,
    /// Bumped on every (re)registration or removal.
    pub generation: u64,
}

impl Global for PluginRegistry {}

impl PluginRegistry {
    /// Registers a plugin with its bar/panel builder.
    pub fn register(
        &mut self,
        manifest: PluginManifest,
        build: impl Fn(&mut Window, &mut App, PanelCx) -> PluginInstance + 'static,
    ) {
        self.factories.retain(|f| f.manifest.id != manifest.id);
        self.generation += 1;
        self.factories.push(PluginFactory { manifest, build: Rc::new(build), generation: self.generation });
    }

    /// Removes a plugin (hot unload). Returns whether it existed.
    pub fn unregister(&mut self, id: &str) -> bool {
        let before = self.factories.len();
        self.factories.retain(|f| f.manifest.id != id);
        let removed = self.factories.len() != before;
        if removed {
            self.generation += 1;
        }
        removed
    }

    /// Registers a command contributed by `owner`.
    pub fn command(&mut self, owner: &str, spec: CommandSpec) {
        self.commands.register(owner, spec);
    }

    pub fn get(&self, id: &str) -> Option<&PluginFactory> {
        self.factories.iter().find(|f| f.manifest.id == id)
    }

    pub fn manifests(&self) -> impl Iterator<Item = &PluginManifest> {
        self.factories.iter().map(|f| &f.manifest)
    }

    /// Whether `id` is enabled under the user's config. Essential built-ins
    /// are always on; removed and disabled plugins are off; otherwise the
    /// manifest's default unless the user switched it on.
    pub fn is_enabled(&self, id: &str, config: &mandara_config::PluginsConfig) -> bool {
        match self.get(id) {
            Some(f) => {
                if f.manifest.origin == PluginOrigin::Builtin && crate::lifecycle::essential_reason(id).is_some() {
                    true
                } else if config.is_removed(id) || config.disabled.iter().any(|d| d == id) {
                    false
                } else {
                    f.manifest.enabled_by_default || config.enabled.iter().any(|e| e == id)
                }
            }
            None => false,
        }
    }
}

/// Reads third-party manifests from `<dir>/<id>/plugin.toml`. Invalid
/// manifests are reported and skipped.
pub fn discover_third_party(dir: &std::path::Path) -> Vec<PluginManifest> {
    #[derive(Deserialize)]
    struct Raw {
        id: String,
        name: String,
        #[serde(default)]
        description: String,
        #[serde(default)]
        author: String,
        #[serde(default = "default_version")]
        version: String,
        #[serde(default)]
        zone: Option<Zone>,
        #[serde(default)]
    i18n: std::collections::BTreeMap<String, ManifestText>,
        #[serde(default)]
        permissions: Vec<String>,
        #[serde(default)]
        homepage: String,
        #[serde(default)]
        settings: Vec<ManifestSetting>,
}
    fn default_version() -> String {
        "0.0.0".into()
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<PluginManifest> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path().join("plugin.toml");
            let text = std::fs::read_to_string(&path).ok()?;
            match toml::from_str::<Raw>(&text) {
                Ok(raw) => Some(PluginManifest {
                    settings: valid_settings(&raw.id, raw.settings),
                    id: raw.id,
                    name: raw.name,
                    description: raw.description,
                    author: raw.author,
                    version: raw.version,
                    origin: PluginOrigin::ThirdParty { path: entry.path() },
                    enabled_by_default: false,
                    zone: raw.zone,
                    translations: raw.i18n,
                    permissions: raw.permissions,
                    homepage: raw.homepage,
                }),
                Err(err) => {
                    tracing::warn!("skipping plugin manifest {}: {err}", path.display());
                    None
                }
            }
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reveal_staggers_blocks() {
        let cx = PanelCx::new();
        cx.set_progress(0.5);
        let first = cx.reveal(0);
        let third = cx.reveal(2);
        assert!(first.opacity > third.opacity);
        assert!(first.offset < third.offset);
        cx.set_progress(1.0);
        assert_eq!(cx.reveal(0), Reveal { opacity: 1.0, offset: 0.0 });
        for i in [5, 12, 40] {
            assert_eq!(cx.reveal(i).opacity, 1.0, "item {i} must be fully visible at the end of the morph");
        }
        cx.set_progress(0.0);
        assert_eq!(cx.reveal(0).opacity, 0.0);
    }

    fn registry() -> PluginRegistry {
        let mut r = PluginRegistry::default();
        r.register(PluginManifest::builtin("weather", "Weather", "", Some(Zone::Right)), |_, _, _| PluginInstance::empty());
        r.register(PluginManifest::builtin("prayer-times", "Prayer times", "", Some(Zone::Right)).opt_in(), |_, _, _| PluginInstance::empty());
        r.register(PluginManifest::builtin("session", "Session", "", Some(Zone::Right)), |_, _, _| PluginInstance::empty());
        r
    }

    #[test]
    fn opt_in_plugins_stay_off_until_enabled() {
        let r = registry();
        let mut c = mandara_config::PluginsConfig::default();
        assert!(!r.is_enabled("prayer-times", &c), "off by default");
        c.enabled.push("prayer-times".into());
        assert!(r.is_enabled("prayer-times", &c), "an explicit enable keeps it");
        c.disabled.push("prayer-times".into());
        assert!(!r.is_enabled("prayer-times", &c), "a disable wins");
    }

    #[test]
    fn removed_plugins_are_off_and_essential_ones_always_on() {
        let r = registry();
        let mut c = mandara_config::PluginsConfig::default();
        assert!(r.is_enabled("weather", &c));
        c.removed.push("weather".into());
        c.enabled.push("weather".into());
        assert!(!r.is_enabled("weather", &c), "removed beats enabled");
        c.disabled.push("session".into());
        c.removed.push("session".into());
        assert!(r.is_enabled("session", &c), "essential plugins can't be switched off by hand edits either");
    }

    #[test]
    fn discovers_third_party_manifests() {
        let dir = std::env::temp_dir().join(format!("mandara-plugins-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("weather")).unwrap();
        std::fs::write(dir.join("weather/plugin.toml"), "id = \"weather\"\nname = \"Weather\"\nzone = \"right\"\n").unwrap();
        std::fs::create_dir_all(dir.join("broken")).unwrap();
        std::fs::write(dir.join("broken/plugin.toml"), "nope = = 1").unwrap();
        let found = discover_third_party(&dir);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "weather");
        assert_eq!(found[0].zone, Some(Zone::Right));
        assert!(!found[0].enabled_by_default);
        assert!(matches!(found[0].origin, PluginOrigin::ThirdParty { .. }));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
