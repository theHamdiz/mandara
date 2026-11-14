//! Preserve Hyprlang's in-place variable and include order in a safe probe.

use anyhow::{Result, ensure};
use std::path::{Path, PathBuf};

use super::{IncludeEnvironment, glob_path_bounded_from, sway_vars};
use crate::keybinds::{MAX_BINDINGS, MAX_FILES, MAX_TOTAL_BYTES, read_source};

/// Only variables, submaps and the selected binding reach the native checker.
/// Repeated includes retain their separate variable contexts; no user script runs.
pub fn shortcut_probe(
    root: &Path,
    selected: &Path,
    line: usize,
    binding: &str,
    environment: &IncludeEnvironment,
) -> Result<String> {
    let mut probe = Probe {
        selected: Some(std::fs::canonicalize(selected)?),
        line,
        binding,
        environment,
        stack: Vec::new(),
        variables: Vec::new(),
        body: String::new(),
        visits: 0,
        bytes: 0,
        matches: 0,
        visitor: None,
        scope: "default".into(),
        expanded_bytes: 0,
    };
    probe.visit(root)?;
    ensure!(
        probe.matches > 0,
        mandara_i18n::t("The compositor shortcuts changed while they were being checked. Refresh the list and try again.")
    );
    Ok(probe.body)
}

/// Discovery and native validation share the same ordered traversal.
pub(crate) trait Visitor {
    fn source(&mut self, file: &Path, target: &Path, source: &str) -> Result<()>;
    fn binding(&mut self, target: &Path, line: usize, scope: &str, expanded: &str) -> Result<()>;
}

pub(crate) fn visit_sources(
    roots: &[PathBuf],
    environment: &IncludeEnvironment,
    visitor: &mut dyn Visitor,
) -> Result<()> {
    let mut reader = Probe {
        selected: None,
        line: 0,
        binding: "",
        environment,
        stack: Vec::new(),
        variables: Vec::new(),
        body: String::new(),
        visits: 0,
        bytes: 0,
        matches: 0,
        visitor: Some(visitor),
        scope: "default".into(),
        expanded_bytes: 0,
    };
    for root in roots {
        reader.visit(root)?;
    }
    Ok(())
}

pub(crate) fn chord(line: &str) -> Option<String> {
    let (_, fields) = line.split_once('=')?;
    let mut fields = fields.split(',');
    let modifiers = fields.next()?.trim();
    let key = fields.next()?.trim();
    if modifiers.contains('$') {
        return None;
    }
    crate::keybinds::normalize(&format!("{}+{key}", super::hypr_mods(modifiers).join("+")))
}

pub(crate) fn chords(
    root: &Path,
    environment: &IncludeEnvironment,
) -> Result<std::collections::BTreeSet<String>> {
    #[derive(Default)]
    struct Chords {
        values: std::collections::BTreeSet<String>,
        generated: std::collections::BTreeSet<PathBuf>,
    }
    impl Visitor for Chords {
        fn source(&mut self, file: &Path, target: &Path, _: &str) -> Result<()> {
            if super::is_generated(file) || super::is_generated(target) {
                self.generated.insert(target.into());
            }
            Ok(())
        }
        fn binding(&mut self, target: &Path, _: usize, scope: &str, expanded: &str) -> Result<()> {
            if scope == "default" && !self.generated.contains(target) {
                self.values.extend(chord(expanded));
            }
            Ok(())
        }
    }
    let mut chords = Chords::default();
    visit_sources(&[root.into()], environment, &mut chords)?;
    Ok(chords.values)
}

struct Probe<'a> {
    selected: Option<PathBuf>,
    line: usize,
    binding: &'a str,
    environment: &'a IncludeEnvironment,
    stack: Vec<PathBuf>,
    variables: Vec<(String, String)>,
    body: String,
    visits: usize,
    bytes: usize,
    matches: usize,
    visitor: Option<&'a mut dyn Visitor>,
    scope: String,
    expanded_bytes: usize,
}

impl Probe<'_> {
    fn append(&mut self, line: &str) -> Result<()> {
        if self.selected.is_none() {
            return Ok(());
        }
        ensure!(
            self.body
                .len()
                .checked_add(line.len())
                .and_then(|size| size.checked_add(1))
                .is_some_and(|size| size <= MAX_TOTAL_BYTES),
            mandara_i18n::t("The shortcut configuration is too large to edit.")
        );
        self.body.push_str(line);
        self.body.push('\n');
        Ok(())
    }

    fn expand(&self, input: &str) -> Result<String> {
        let mut value = input.to_owned();
        // Hyprlang permits at most 100 rounds, with longer variable names first.
        const MAX_EXPANSIONS: usize = 100;
        for _ in 0..MAX_EXPANSIONS {
            if !self.variables.iter().any(|(name, _)| value.contains(name)) {
                return Ok(value);
            }
            value = sway_vars::expand(&value, &self.variables)?.into_owned();
        }
        anyhow::bail!(mandara_i18n::t("The shortcut configuration is too large to edit."))
    }

    fn binding_expansion(&mut self, line: &str) -> Result<String> {
        let expanded = self.expand(line)?;
        self.expanded_bytes += expanded.len();
        ensure!(
            self.expanded_bytes <= MAX_TOTAL_BYTES,
            mandara_i18n::t("The shortcut configuration is too large to edit.")
        );
        Ok(expanded)
    }

    fn visit(&mut self, file: &Path) -> Result<()> {
        let real = std::fs::canonicalize(file)?;
        ensure!(
            !real.extension().is_some_and(|extension| extension == "lua"),
            "{}",
            mandara_i18n::t("Computed shortcuts must be edited in their source configuration.")
        );
        ensure!(!self.stack.contains(&real), mandara_i18n::t("The shortcut configuration contains an include cycle."));
        self.visits += 1;
        ensure!(self.visits <= MAX_FILES, mandara_i18n::t("The shortcut configuration includes too many files."));
        let source = read_source(file)?;
        self.bytes += source.len();
        ensure!(
            self.bytes <= MAX_TOTAL_BYTES,
            mandara_i18n::t("The shortcut configuration is too large to edit.")
        );
        self.stack.push(real.clone());
        if let Some(visitor) = &mut self.visitor {
            visitor.source(file, &real, &source)?;
        }
        for logical in crate::keybinds::logical_lines::LogicalLines::new(
            &source,
            crate::keybinds::logical_lines::Continuation::Hyprland,
        ) {
            let logical = logical?;
            let raw = logical.text.as_ref();
            let line = uncomment(raw);
            let line = line.trim();
            if self.selected.as_ref() == Some(&real) && logical.number == self.line {
                self.matches += 1;
                self.binding_expansion(self.binding)?;
                self.append(self.binding.trim_end())?;
                continue;
            }
            if let Some((name, value)) = line.split_once('=') {
                let name = name.trim();
                if let Some(key) = name.strip_prefix('$')
                    && !key.is_empty()
                    && key
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
                {
                    let value = self.expand(value.trim())?;
                    sway_vars::check_assignment(&self.variables, name, &value)?;
                    if let Some((_, previous)) =
                        self.variables.iter_mut().find(|(key, _)| key == name)
                    {
                        *previous = value;
                    } else {
                        ensure!(
                            self.variables.len() < MAX_BINDINGS,
                            mandara_i18n::t("The shortcut configuration is too large to edit.")
                        );
                        self.variables.push((name.into(), value));
                        self.variables
                            .sort_unstable_by_key(|(name, _)| std::cmp::Reverse(name.len()));
                    }
                    // Native parsing preserves expressions and comment escaping.
                    self.append(raw.trim())?;
                    continue;
                }
            }
            if let Some((name, value)) = line.split_once('=') {
                let name = name.trim();
                if name == "submap" {
                    self.scope = self.expand(value.trim())?;
                    if self.scope == "reset" {
                        self.scope = "default".into();
                    }
                    self.append(raw.trim())?;
                    continue;
                }
                if name.starts_with("bind")
                    && name["bind".len()..]
                        .chars()
                        .all(|ch| ch.is_ascii_alphabetic())
                {
                    let expanded = self.binding_expansion(line)?;
                    if let Some(visitor) = &mut self.visitor {
                        visitor.binding(&real, logical.number, &self.scope, &expanded)?;
                    }
                }
            }
            if let Some(include) = line
                .split_once('=')
                .filter(|(name, _)| name.trim() == "source")
                .map(|(_, value)| value.trim())
            {
                let expanded = self.expand(include)?;
                let path = self.environment.resolve(
                    file,
                    &self.environment.expand(expanded.trim().trim_matches('"'))?,
                )?;
                let paths = glob_path_bounded_from(
                    Path::new(""),
                    &path,
                    MAX_FILES.saturating_sub(self.visits),
                )?;
                ensure!(
                    !paths.is_empty(),
                    "included shortcut configuration is missing"
                );
                for path in paths {
                    if std::fs::metadata(&path)?.is_file() {
                        self.visit(&path)?;
                    }
                }
            }
        }
        self.stack.pop();
        Ok(())
    }
}

fn uncomment(line: &str) -> String {
    let mut output = String::new();
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '#' {
            if chars.peek() == Some(&'#') {
                chars.next();
            } else {
                break;
            }
        }
        output.push(ch);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn includes_and_reassignments_surround_the_selected_binding_in_native_order() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("hyprland.conf");
        let child = directory.path().join("keys.conf");
        std::fs::write(&root, "$mod = SUPER\n$keys = keys.conf\nsource = $keys\n$mod = CTRL\nsource = keys.conf\nexec-once = touch /never-run\n").unwrap();
        std::fs::write(&child, "bind = $mod, Q, exec, foot\n$mod = ALT\n").unwrap();
        let probe = shortcut_probe(
            &root,
            &child,
            1,
            "bind = $mod, W, exec, foot\nsubmap = reset",
            &IncludeEnvironment::default(),
        )
        .unwrap();
        assert_eq!(
            probe,
            "$mod = SUPER\n$keys = keys.conf\nbind = $mod, W, exec, foot\nsubmap = reset\n$mod = ALT\n$mod = CTRL\nbind = $mod, W, exec, foot\nsubmap = reset\n$mod = ALT\n"
        );
        assert!(!probe.contains("source =") && !probe.contains("exec-once"));
        std::fs::write(
            &child,
            "bind = SUPER, Q, exec, foot\nsource = hyprland.conf\n",
        )
        .unwrap();
        assert!(
            shortcut_probe(
                &root,
                &child,
                1,
                "bind = SUPER, Q, exec, foot",
                &IncludeEnvironment::default()
            )
            .is_err()
        );
    }

    #[test]
    fn variables_cannot_grow_beyond_the_budget_before_native_validation() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("hyprland.conf");
        let mut source = "$v0 = a\n".to_owned();
        for index in 1..32 {
            source.push_str(&format!("$v{index} = $v{}$v{}\n", index - 1, index - 1));
        }
        source.push_str("bind = SUPER, Q, exec, foot\n");
        std::fs::write(&file, source).unwrap();
        assert!(
            shortcut_probe(
                &file,
                &file,
                33,
                "bind = SUPER, W, exec, foot",
                &IncludeEnvironment::default()
            )
            .is_err()
        );
    }
    #[test]
    fn native_modifier_spellings_and_unresolved_keys_keep_their_conflict_meaning() {
        assert_eq!(
            chord("bind = SUPER_SHIFT, Q, exec, foot").as_deref(),
            Some("shift+super+q")
        );
        assert_eq!(
            chord("bind = CTRLALT, Q, exec, foot").as_deref(),
            Some("alt+ctrl+q")
        );
        assert!(chord("bind = $unknown, Q, exec, foot").is_none());
        assert!(chord("bind = SUPER, code:24, exec, foot").is_none());
    }
    #[test]
    fn legacy_sources_are_distinct_from_lua_words_inside_commands_and_paths() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("hyprland.conf");
        let child = directory.path().join("requires.conf");
        std::fs::write(&file, "exec-once = lua dofile('/never-read-this')\nsource = requires.conf\nbind = $mod, Q, exec, foot\n").unwrap();
        std::fs::write(&child, "$mod = SUPER\n").unwrap();
        let body = shortcut_probe(
            &file,
            &file,
            3,
            "bind = $mod, W, exec, foot",
            &IncludeEnvironment::default(),
        )
        .unwrap();
        assert_eq!(body, "$mod = SUPER\nbind = $mod, W, exec, foot\n");
        assert_eq!(
            chords(&file, &IncludeEnvironment::default()).unwrap(),
            std::collections::BTreeSet::from(["super+q".into()])
        );
    }
    #[test]
    fn continued_includes_and_non_regular_glob_matches_keep_the_native_file_order() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("hyprland.conf");
        let sources = directory.path().join("conf.d");
        std::fs::create_dir_all(sources.join("nested")).unwrap();
        std::fs::write(sources.join("keys.conf"), "$mod = SUPER\n").unwrap();
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            sources.join("fifo"),
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )
        .unwrap();
        std::fs::write(&root, "source = \\\nconf.d/*\nbind = $mod, Q, exec, foot\n").unwrap();
        assert_eq!(
            chords(&root, &IncludeEnvironment::default()).unwrap(),
            std::collections::BTreeSet::from(["super+q".into()])
        );
        assert_eq!(
            shortcut_probe(
                &root,
                &root,
                3,
                "bind = $mod, W, exec, foot",
                &IncludeEnvironment::default()
            )
            .unwrap(),
            "$mod = SUPER\nbind = $mod, W, exec, foot\n"
        );
        std::os::unix::fs::symlink("missing", sources.join("broken.conf")).unwrap();
        assert!(chords(&root, &IncludeEnvironment::default()).is_err());
    }
}
