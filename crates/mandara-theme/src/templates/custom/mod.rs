//! User-authored app themes, rendered from the same palette as built-ins.

mod colors;
mod command;
mod syntax;
pub use command::Command;

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;
use minijinja::{Environment, UndefinedBehavior, Value};
use serde::{Deserialize, Serialize};

use crate::{Color, MandaraPalette};

/// Limits keep a malformed template from monopolizing the theme worker.
pub const MAX_TEMPLATE_BYTES: usize = 1024 * 1024;
const RENDER_FUEL: u64 = 1_000_000;
const MAX_RENDER_BYTES: usize = 8 * MAX_TEMPLATE_BYTES;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Template {
    pub enabled: bool,
    /// Inline templates can be authored in Studio without creating a separate file.
    pub content: Option<std::sync::Arc<str>>,
    /// Base directory retained when a local template document is imported.
    pub directory: Option<PathBuf>,
    pub input_path: PathBuf,
    pub input_path_modes: BTreeMap<String, PathBuf>,
    pub output_path: Paths,
    pub input_path_dynamic: String,
    pub output_path_dynamic: String,
    pub requires_path: Option<PathBuf>,
    pub pre_hook: String,
    pub post_hook: String,
    pub undo_hook: String,
    pub post_action: String,
    pub gtk_extension: Option<String>,
    #[serde(default = "yes")]
    pub hook_async: bool,
    pub index: i32,
    pub compare_to: String,
    pub colors_to_compare: BTreeMap<String, String>,
}

impl Default for Template {
    fn default() -> Self {
        Self {
            enabled: false,
            content: None,
            directory: None,
            input_path: PathBuf::new(),
            input_path_modes: BTreeMap::new(),
            output_path: Paths::default(),
            input_path_dynamic: String::new(),
            output_path_dynamic: String::new(),
            requires_path: None,
            pre_hook: String::new(),
            post_hook: String::new(),
            undo_hook: String::new(),
            post_action: String::new(),
            gtk_extension: None,
            hook_async: true,
            index: 0,
            compare_to: String::new(),
            colors_to_compare: BTreeMap::new(),
        }
    }
}

/// Paths and commands are metadata, bounded separately from rendered theme bodies.
const MAX_PATH_BYTES: usize = 4096;
const MAX_COMMAND_BYTES: usize = 64 * 1024;
const MAX_METADATA_ENTRIES: usize = 128;
const MAX_OUTPUTS: usize = 32;

impl Template {
    pub fn validate(&self) -> Result<()> {
        use anyhow::ensure;
        ensure!(
            self.content
                .as_ref()
                .is_none_or(|body| body.len() <= MAX_TEMPLATE_BYTES),
            "inline template is too large"
        );
        ensure!(
            self.output_path.iter().count() <= MAX_OUTPUTS
                && self.input_path_modes.len() <= MAX_METADATA_ENTRIES
                && self.colors_to_compare.len() <= MAX_METADATA_ENTRIES,
            "too many template paths or comparison colors"
        );
        for command in [
            &self.input_path_dynamic,
            &self.output_path_dynamic,
            &self.pre_hook,
            &self.post_hook,
            &self.undo_hook,
        ] {
            ensure!(
                command.len() <= MAX_COMMAND_BYTES,
                "template command is too long"
            );
        }
        for path in std::iter::once(&self.input_path)
            .chain(self.directory.iter())
            .chain(self.requires_path.iter())
            .chain(self.output_path.iter())
            .chain(self.input_path_modes.values())
        {
            ensure!(
                path.as_os_str().len() <= MAX_PATH_BYTES,
                "template path is too long"
            );
        }
        for text in std::iter::once(self.compare_to.as_str())
            .chain(std::iter::once(self.post_action.as_str()))
            .chain(self.gtk_extension.as_deref())
            .chain(self.input_path_modes.keys().map(String::as_str))
            .chain(
                self.colors_to_compare
                    .iter()
                    .flat_map(|(key, value)| [key.as_str(), value.as_str()]),
            )
        {
            ensure!(
                text.len() <= MAX_PATH_BYTES,
                "template metadata is too long"
            );
        }
        let hook_only = !self.pre_hook.is_empty() || !self.post_hook.is_empty();
        ensure!(
            self.content.is_some()
                || !self.input_path.as_os_str().is_empty()
                || !self.input_path_modes.is_empty()
                || !self.input_path_dynamic.is_empty()
                || hook_only,
            "template has no input"
        );
        if let Some(extension) = &self.gtk_extension {
            ensure!(
                ["gtk3", "gtk4"].contains(&extension.as_str()),
                "unknown GTK extension"
            );
            ensure!(
                self.output_path.iter().next().is_none()
                    && self.output_path_dynamic.is_empty()
                    && !hook_only
                    && self.undo_hook.is_empty()
                    && self.post_action.is_empty()
                    && self.requires_path.is_none(),
                "GTK extensions cannot run hooks or choose another destination"
            );
        } else {
            ensure!(
                self.output_path.iter().next().is_some()
                    || !self.output_path_dynamic.is_empty()
                    || hook_only,
                "template has no output"
            );
        }
        Ok(())
    }

    pub fn text_bytes(&self) -> usize {
        self.content.as_ref().map_or(0, |body| body.len())
            + [
                &self.input_path_dynamic,
                &self.output_path_dynamic,
                &self.pre_hook,
                &self.post_hook,
                &self.undo_hook,
                &self.compare_to,
                &self.post_action,
            ]
            .into_iter()
            .map(|text| text.len())
            .sum::<usize>()
            + self.gtk_extension.as_ref().map_or(0, String::len)
            + self.input_path_modes.keys().map(String::len).sum::<usize>()
            + self
                .colors_to_compare
                .iter()
                .map(|(key, value)| key.len() + value.len())
                .sum::<usize>()
            + std::iter::once(&self.input_path)
                .chain(self.directory.iter())
                .chain(self.requires_path.iter())
                .chain(self.output_path.iter())
                .chain(self.input_path_modes.values())
                .map(|path| path.as_os_str().len())
                .sum::<usize>()
    }
}

/// A template can write one file or the same content to several clients.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Paths {
    One(PathBuf),
    Many(Vec<PathBuf>),
}

impl Default for Paths {
    fn default() -> Self {
        Self::Many(Vec::new())
    }
}

impl Paths {
    pub fn iter(&self) -> impl Iterator<Item = &PathBuf> {
        let slice = match self {
            Self::One(path) => std::slice::from_ref(path),
            Self::Many(paths) => paths,
        };
        slice.iter()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CustomColor {
    Hex(String),
    Options {
        color: Option<String>,
        color_dark: Option<String>,
        color_light: Option<String>,
        #[serde(default = "yes")]
        blend: bool,
    },
}

fn yes() -> bool {
    true
}

impl CustomColor {
    pub fn validate(&self) -> Result<()> {
        self.resolve(true)?;
        self.resolve(false)?;
        Ok(())
    }
    fn resolve(&self, dark: bool) -> Result<(Color, bool)> {
        let (hex, blend) = match self {
            Self::Hex(hex) => (hex.as_str(), true),
            Self::Options {
                color,
                color_dark,
                color_light,
                blend,
            } => {
                let (own, other) = if dark {
                    (color_dark, color_light)
                } else {
                    (color_light, color_dark)
                };
                (
                    own.as_ref()
                        .or(color.as_ref())
                        .or(other.as_ref())
                        .ok_or_else(|| anyhow::anyhow!("custom color has no source"))?
                        .as_str(),
                    *blend,
                )
            }
        };
        Ok((Color::from_hex(hex)?, blend))
    }
}

/// A render has both modes available, even when the shell uses just one.
pub struct Renderer {
    env: Environment<'static>,
    context: BTreeMap<String, Value>,
}

impl Renderer {
    pub fn new(
        current: &MandaraPalette,
        other: &MandaraPalette,
        custom: &BTreeMap<String, CustomColor>,
    ) -> Result<Self> {
        let mut env = Environment::new();
        env.set_undefined_behavior(UndefinedBehavior::Strict);
        env.set_fuel(Some(RENDER_FUEL));
        env.set_formatter(|out, state, value| {
            if value.kind() == minijinja::value::ValueKind::Bool {
                out.write_str(if value.is_true() { "true" } else { "false" })?;
                Ok(())
            } else if value.is_none() {
                Ok(())
            } else {
                minijinja::escape_formatter(out, state, value)
            }
        });
        env.set_syntax(
            minijinja::syntax::SyntaxConfig::builder()
                .block_delimiters("<*", "*>")
                .keep_trailing_newline(true)
                .build()?,
        );
        colors::filters(&mut env);
        let (dark, light) = if current.is_dark {
            (current, other)
        } else {
            (other, current)
        };
        let mut context = BTreeMap::new();
        let (tokens, palettes) = colors::context(dark, light, current.is_dark, custom)?;
        context.insert("colors".into(), tokens);
        context.insert("palettes".into(), palettes);
        context.insert(
            "mode".into(),
            Value::from(if current.is_dark { "dark" } else { "light" }),
        );
        context.insert(
            "config".into(),
            Value::from_object(BTreeMap::from([(
                "shell",
                Value::from_object(BTreeMap::from([("font_family", Value::from("sans-serif"))])),
            )])),
        );
        for key in ["image", "config_dir", "config_file", "closest_color"] {
            context.insert(key.into(), Value::from(""));
        }
        Ok(Self { env, context })
    }

    pub fn set(&mut self, name: &str, value: impl Into<Value>) {
        self.context.insert(name.into(), value.into());
    }

    pub fn set_font_family(&mut self, family: &str) {
        self.set(
            "config",
            Value::from_object(BTreeMap::from([(
                "shell",
                Value::from_object(BTreeMap::from([("font_family", Value::from(family))])),
            )])),
        );
    }

    pub fn render(&self, source: &str) -> Result<String> {
        anyhow::ensure!(
            source.len() <= MAX_TEMPLATE_BYTES,
            "template exceeds {MAX_TEMPLATE_BYTES} bytes"
        );
        let normalized = syntax::normalize(source)?;
        let template = self.env.template_from_str(&normalized)?;
        let mut output = Limited {
            text: String::with_capacity(source.len()),
            limit: MAX_RENDER_BYTES,
        };
        template.render_captured_to(Value::from_object(self.context.clone()), &mut output)?;
        Ok(output.text)
    }

    pub fn render_command(&self, source: &str) -> Result<Command> {
        command::render(&self.env, &self.context, source)
    }
}

struct Limited {
    text: String,
    limit: usize,
}
impl std::io::Write for Limited {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.text.len()) {
            return Err(std::io::Error::other("rendered theme is too large"));
        }
        self.text
            .push_str(std::str::from_utf8(bytes).map_err(std::io::Error::other)?);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn renderer() -> Renderer {
        Renderer::new(
            &crate::engine::default_palette(true),
            &crate::engine::default_palette(false),
            &BTreeMap::new(),
        )
        .unwrap()
    }

    #[test]
    fn template_metadata_counts_toward_storage_limits_and_is_bounded() {
        let mut template = Template {
            content: Some("body".into()),
            output_path: Paths::One("out".into()),
            ..Default::default()
        };
        let original = template.text_bytes();
        template
            .colors_to_compare
            .insert("brand".into(), "#ff0000".into());
        assert_eq!(template.text_bytes(), original + 12);
        template.validate().unwrap();
        template
            .colors_to_compare
            .insert("x".repeat(MAX_PATH_BYTES + 1), "#ff0000".into());
        assert!(template.validate().is_err());
        template.colors_to_compare.clear();
        template.post_hook = "x".repeat(MAX_COMMAND_BYTES + 1);
        assert!(template.validate().is_err());
    }

    #[test]
    fn modes_formats_filters_loops_and_conditionals_work_together() {
        let r = renderer();
        let text = r.render("<* for i in 0..3 *>\ncolor{{ i }}={{ colors.primary.default.hex | set_alpha: 0.5 }}\n<* endfor *>\n<* if {{ mode }} == \"dark\" *>\n{{ \"My Theme\" | snake_case }}\n<* else *>\nlight\n<* endif *>\n").unwrap();
        assert_eq!(text.lines().count(), 4, "{text}");
        assert!(
            text.starts_with("color0=") && text.ends_with("my_theme\n"),
            "{text}"
        );
        assert_ne!(
            r.render("{{ colors.surface.dark.hex }}").unwrap(),
            r.render("{{ colors.surface.light.hex }}").unwrap()
        );
        assert!(r.render("{{ colors.misspelled.default.hex }}").is_err());
    }

    #[test]
    fn loop_metadata_uses_zero_based_indexes_without_rewriting_quoted_names() {
        let render = renderer();
        assert_eq!(render.render("<* for value in 0..3 *>{{ loop.index }}:{{ loop.first }}:{{ loop.last }};<* endfor *>").unwrap(),"0:true:false;1:false:false;2:false:true;");
        assert_eq!(render.render("<* for value in 0..2 *><* if {{ loop.index }} == 0 *>first<* else *>next<* endif *>{{ 'loop.index' | replace: 'index', 'counter' }}<* endfor *>").unwrap(),"firstloop.counternextloop.counter");
        assert_eq!(render.render("<* for tone in palettes.primary *><* if {{ loop.last }} *>{{ loop.index }}:{{ tone.default }}<* endif *><* endfor *>").unwrap(),"17:#ffffff");
    }

    #[test]
    fn template_conditions_trim_false_strings_and_format_boolean_values_as_config_text() {
        let render = renderer();
        assert_eq!(
            render.render("{{ true }}:{{ false }}:{{ none }}").unwrap(),
            "true:false:"
        );
        for value in ["", "false", "0", "none", " FaLsE ", " NONE "] {
            let mut render = renderer();
            render.set("flag", value);
            assert_eq!(
                render
                    .render("<* if {{ flag }} *>on<* else *>off<* endif *>")
                    .unwrap(),
                "off"
            );
        }
    }

    #[test]
    fn custom_sources_and_tonal_palette_access_are_available_in_both_modes() {
        let custom = [(
            "brand".into(),
            CustomColor::Options {
                color: None,
                color_dark: Some("#ff4400".into()),
                color_light: Some("#0088ff".into()),
                blend: false,
            },
        )]
        .into_iter()
        .collect();
        let r = Renderer::new(
            &crate::engine::default_palette(true),
            &crate::engine::default_palette(false),
            &custom,
        )
        .unwrap();
        assert_eq!(
            r.render("{{ colors.brand_source.dark.hex }} {{ colors.brand_source.light.hex }}")
                .unwrap(),
            "#ff4400 #0088ff"
        );
        assert_eq!(
            r.render("{{ palettes.primary.0.hex }} {{ palettes.primary._100.hex }}")
                .unwrap(),
            "#000000 #ffffff"
        );
        assert_eq!(
            r.render("<* for tone in palettes.primary *>{{ tone.default }}\n<* endfor *>")
                .unwrap()
                .lines()
                .count(),
            18
        );
    }

    #[test]
    fn rendering_is_bounded_and_bad_syntax_does_not_write_partial_output() {
        let r = renderer();
        assert!(
            r.render("<* for i in 0..1000000000 *>x<* endfor *>")
                .is_err()
        );
        assert!(
            r.render("{{ colors.primary.default.hex | missing }}")
                .is_err()
        );
        assert!(r.render("<* if mode *>unterminated").is_err());
    }

    #[test]
    fn formats_channel_filters_and_declared_fonts_are_available_to_authored_templates() {
        let mut renderer = renderer();
        renderer.set_font_family("Inter Arabic");
        assert_eq!(
            renderer.render("{{ config.shell.font_family }}").unwrap(),
            "Inter Arabic"
        );
        assert_eq!(
            renderer
                .render("{{ '#ff8000' | to_color | rgb }} / {{ '#ff8000' | to_color | rgb_csv }}")
                .unwrap(),
            "rgb(255, 128, 0) / 255,128,0"
        );
        assert_eq!(
            renderer
                .render("{{ '#ff0000' | grayscale | hex }}")
                .unwrap(),
            "#4c4c4c"
        );
        assert_eq!(
            renderer
                .render("{{ '#ff0000' | rotate_hue: 120 | hex }}")
                .unwrap(),
            "#00ff00"
        );
        assert_eq!(
            renderer
                .render("{{ '#ff0000' | set_red: 0 | set_blue: 255 | hex }}")
                .unwrap(),
            "#0000ff"
        );
        assert_eq!(
            renderer
                .render("{{ '#ff8000' | set_alpha: 0.5 | rgba }}")
                .unwrap(),
            "rgba(255, 128, 0, 0.5)"
        );
        assert!(renderer.render("{{ '#ff8000' | lighten: 1e999 }}").is_err());
    }

    #[test]
    #[ignore = "set MANDARA_OFFICIAL_TEMPLATE_FIXTURES to downloaded template assets"]
    fn external_official_inputs_render_in_both_modes() {
        let root = std::path::PathBuf::from(
            std::env::var_os("MANDARA_OFFICIAL_TEMPLATE_FIXTURES")
                .expect("template assets directory"),
        );
        let mut sources = Vec::new();
        for directory in std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().is_dir())
        {
            for file in std::fs::read_dir(directory.path()).unwrap().flatten() {
                let path = file.path();
                if path.is_file()
                    && !matches!(
                        path.extension().and_then(|extension| extension.to_str()),
                        Some("sh" | "md" | "png" | "webp")
                    )
                {
                    sources.push(path);
                }
            }
        }
        assert!(sources.len() >= 20);
        let mut failures = Vec::new();
        for dark in [true, false] {
            let render = Renderer::new(
                &crate::engine::default_palette(dark),
                &crate::engine::default_palette(!dark),
                &BTreeMap::new(),
            )
            .unwrap();
            for path in &sources {
                let text = std::fs::read_to_string(path).unwrap();
                if let Err(error) = render.render(&text) {
                    failures.push(format!("{} ({dark}): {error:#}", path.display()));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    #[ignore = "set MANDARA_TEMPLATE_FIXTURES to a downloaded catalog; renders without running its hooks"]
    fn external_template_catalog_renders_without_missing_tokens_or_syntax() {
        let root = std::env::var_os("MANDARA_TEMPLATE_FIXTURES").expect("catalog directory");
        let mut count = 0;
        let mut failures = Vec::new();
        for entry in std::fs::read_dir(root).unwrap().flatten() {
            let dir = entry.path();
            let Ok(text) = std::fs::read_to_string(dir.join("template.toml")) else {
                continue;
            };
            #[derive(Deserialize)]
            struct Manifest {
                templates: BTreeMap<String, Template>,
            }
            let manifest: Manifest = toml::from_str(&text).unwrap();
            for (id, spec) in manifest.templates {
                for path in std::iter::once(&spec.input_path)
                    .chain(spec.input_path_modes.values())
                    .filter(|p| !p.as_os_str().is_empty())
                {
                    let path = dir.join(path);
                    let text = std::fs::read_to_string(&path)
                        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
                    count += 1;
                    for dark in [true, false] {
                        let render = Renderer::new(
                            &crate::engine::default_palette(dark),
                            &crate::engine::default_palette(!dark),
                            &BTreeMap::new(),
                        )
                        .unwrap();
                        if let Err(error) = render.render(&text) {
                            failures.push(format!("{id} {} ({dark}): {error:#}", path.display()));
                        }
                    }
                }
            }
        }
        assert!(count > 0, "no templates found");
        eprintln!("Rendered {count} catalog inputs in both modes");
        assert!(
            failures.is_empty(),
            "{} failures in {count} templates:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}
