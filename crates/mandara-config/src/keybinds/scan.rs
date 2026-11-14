//! Bounded include discovery, source identities and row storage for Studio.

use std::{
    collections::BTreeSet,
    hash::{Hash, Hasher},
    io::Read as _,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, ensure};

use super::{Binding, MAX_BINDINGS, MAX_FILE_BYTES, MAX_FILES, MAX_TOTAL_BYTES};
use crate::compositor::{IncludeEnvironment, Target, included_files_bounded_in, is_generated};
mod hyprland;
mod niri;
pub use niri::{NiriModifiers, resolve_mod as resolve_niri_mod};

#[derive(Debug, serde::Serialize)]
pub struct Source {
    pub file: PathBuf,
    pub target: PathBuf,
    #[serde(with = "super::identity")]
    pub fingerprint: u64,
    pub bindings: Vec<Binding>,
}

impl Source {
    pub fn unchanged(&self) -> Result<bool> {
        Ok(std::fs::canonicalize(&self.file)? == self.target
            && fingerprint(&read(&self.file)?) == self.fingerprint)
    }

    pub fn binding_id(&self, index: usize) -> Option<super::BindingId> {
        self.bindings.get(index)?;
        Some(super::BindingId {
            file: self.file.clone(),
            target: self.target.clone(),
            fingerprint: self.fingerprint,
            index,
        })
    }
}

#[derive(Default, Debug, serde::Serialize)]
pub struct Scan {
    pub sources: Vec<Source>,
    pub generated: Vec<PathBuf>,
    pub errors: Vec<(PathBuf, String)>,
    pub niri: Option<Box<NiriModifiers>>,
}

pub(crate) fn fingerprint(text: &str) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hash);
    hash.finish()
}

/// Open only regular files and reject oversized input before parsing or retaining it.
pub fn read_source(path: &Path) -> Result<String> {
    let file = crate::regular::open(path).with_context(|| path.display().to_string())?;
    let mut text = String::new();
    file.take(MAX_FILE_BYTES as u64 + 1)
        .read_to_string(&mut text)?;
    ensure!(
        text.len() <= MAX_FILE_BYTES,
        "{}",
        mandara_i18n::t("The shortcut configuration is too large to edit.")
    );
    Ok(text)
}

pub(crate) use read_source as read;

pub fn scan(target: Target) -> Scan {
    scan_from(target, &default_root(target))
}

pub fn default_root(target: Target) -> PathBuf {
    let config = dirs::config_dir().unwrap_or_else(|| PathBuf::from("~/.config"));
    default_root_in(target, &config, &crate::paths::home())
}

pub fn default_root_in(target: Target, config: &Path, home: &Path) -> PathBuf {
    match target {
        Target::KWin => config.join("kglobalshortcutsrc"),
        Target::Cosmic => config.join("cosmic/com.system76.CosmicSettings.Shortcuts/v1/custom"),
        _ => target.user_config_in(config, home),
    }
}

pub fn scan_from(target: Target, root: &Path) -> Scan {
    scan_roots(target, [root.to_path_buf()])
}

/// A merged native config shares the same file, byte and binding budgets.
pub fn scan_roots(target: Target, roots: impl IntoIterator<Item = PathBuf>) -> Scan {
    scan_roots_in(target, roots, &IncludeEnvironment::default())
}

pub fn scan_roots_in(
    target: Target,
    roots: impl IntoIterator<Item = PathBuf>,
    environment: &IncludeEnvironment,
) -> Scan {
    let mut scan = Scan::default();
    let mut seen = BTreeSet::new();
    let mut queue: Vec<_> = roots.into_iter().take(MAX_FILES + 1).collect();
    if queue.len() > MAX_FILES {
        scan.errors.push((
            PathBuf::new(),
            mandara_i18n::t("The shortcut configuration includes too many files.").into(),
        ));
        return scan;
    }
    if target == Target::Sway {
        return scan_sway(&queue, environment);
    }
    if target == Target::Niri {
        return niri::scan(&queue, environment);
    }
    if target == Target::Hyprland
        && queue.iter().all(|file| {
            std::fs::canonicalize(file)
                .ok()
                .and_then(|real| {
                    read(file)
                        .ok()
                        .map(|text| !super::formats::hyprland_lua(&real, &text))
                })
                .unwrap_or(true)
        })
    {
        return hyprland::scan(&queue, environment);
    }
    queue.reverse();
    let mut total_bytes = 0;
    let mut total_bindings = 0;
    while let Some(file) = queue.pop() {
        if is_generated(&file) {
            if !scan.generated.contains(&file) {
                scan.generated.push(file);
            }
            continue;
        }
        let result = (|| -> Result<()> {
            let real = std::fs::canonicalize(&file)?;
            if is_generated(&real) {
                if !scan.generated.contains(&file) {
                    scan.generated.push(file.clone());
                }
                return Ok(());
            }
            if !seen.insert(real.clone()) {
                return Ok(());
            }
            ensure!(
                seen.len() <= MAX_FILES,
                mandara_i18n::t("The shortcut configuration includes too many files.")
            );
            let source = read(&file)?;
            total_bytes += source.len();
            ensure!(
                total_bytes <= MAX_TOTAL_BYTES,
                mandara_i18n::t("The shortcut configuration is too large to edit.")
            );
            let bindings = super::parse(target, &file, &source)?;
            total_bindings += bindings.len();
            ensure!(
                total_bindings <= MAX_BINDINGS,
                mandara_i18n::t("The compositor has too many shortcuts to edit at once.")
            );
            let mut children = included_files_bounded_in(
                target,
                &file,
                &source,
                MAX_FILES.saturating_sub(seen.len() + queue.len()),
                environment,
            )?;
            ensure!(
                seen.len() + queue.len() + children.len() <= MAX_FILES,
                mandara_i18n::t("The shortcut configuration includes too many files.")
            );
            children.reverse();
            queue.extend(children);
            scan.sources.push(Source {
                file: file.clone(),
                target: real,
                fingerprint: fingerprint(&source),
                bindings,
            });
            Ok(())
        })();
        if let Err(error) = result {
            scan.errors.push((file, format!("{error:#}")));
        }
        if total_bytes > MAX_TOTAL_BYTES || total_bindings > MAX_BINDINGS || seen.len() > MAX_FILES
        {
            break;
        }
    }
    scan
}

fn scan_sway(roots: &[PathBuf], environment: &IncludeEnvironment) -> Scan {
    let result = (|| -> Result<Scan> {
        let mut native = crate::compositor::sway_shortcut_sources_in(roots, environment)?;
        native
            .bindings
            .sort_unstable_by(|(path, binding), (other, next)| {
                (path, binding.line).cmp(&(other, next.line))
            });
        let mut scan = Scan {
            generated: native.generated,
            ..Default::default()
        };
        let mut count = 0;
        for (file, text) in native.files {
            let target = std::fs::canonicalize(&file)?;
            let mut bindings = super::parse(Target::Sway, &target, &text)?;
            count += bindings.len();
            ensure!(
                count <= MAX_BINDINGS,
                mandara_i18n::t("The compositor has too many shortcuts to edit at once.")
            );
            for binding in &mut bindings {
                binding.chord = native
                    .bindings
                    .binary_search_by(|(path, resolved)| {
                        (path, resolved.line).cmp(&(&file, binding.line - 1))
                    })
                    .ok()
                    .map(|index| native.bindings[index].1.chord.clone());
            }
            scan.sources.push(Source {
                file,
                target,
                fingerprint: fingerprint(&text),
                bindings,
            });
        }
        Ok(scan)
    })();
    result.unwrap_or_else(|error| Scan {
        errors: vec![(
            roots.first().cloned().unwrap_or_default(),
            format!("{error:#}"),
        )],
        ..Default::default()
    })
}

pub(crate) fn mango_includes(
    file: &Path,
    source: &str,
    environment: &IncludeEnvironment,
) -> Result<Vec<PathBuf>> {
    let document: toml::Value = toml::from_str(source)?;
    let mut paths = Vec::new();
    if let Some(global) = document.get("global") {
        for key in ["source", "source_optional"] {
            if let Some(value) = global.get(key) {
                let entries: Vec<_> = value
                    .as_array()
                    .map_or_else(|| vec![value], |values| values.iter().collect());
                for entry in entries {
                    if let Some(path) = entry.as_str() {
                        let path = environment.resolve(file, &environment.expand(path)?)?;
                        if key != "source_optional" || is_generated(&path) || path.try_exists()? {
                            paths.push(path);
                        }
                    }
                }
            }
        }
    }
    ensure!(paths.len() <= MAX_FILES, mandara_i18n::t("The shortcut configuration includes too many files."));
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_environment_drives_all_include_paths_and_optional_sources_stay_optional() {
        let directory = tempfile::tempdir().unwrap();
        let server = directory.path().join("server");
        std::fs::create_dir(&server).unwrap();
        let environment = IncludeEnvironment::captured([
            ("HOME".into(), server.clone().into_os_string()),
            ("KEY_DIR".into(), server.clone().into_os_string()),
        ]);
        for (target, name, root_text, child_name, child_text) in [
            (
                Target::Mango,
                "mango.conf",
                "source_optional=~/missing\nsource=$KEY_DIR/child.conf\n",
                "child.conf",
                "bind=SUPER,Q,killclient\n",
            ),
            (
                Target::Mango,
                "mango.toml",
                "[global]\nsource=['$KEY_DIR/child.conf']\nsource_optional=['~/missing']\n",
                "child.conf",
                "bind=SUPER,Q,killclient\n",
            ),
            (
                Target::Umbriel,
                "umbriel.toml",
                "[include]\nfiles=['${KEY_DIR}/child.toml']\n[include.optional]\nfiles=['~/missing']\n",
                "child.toml",
                "[keybinds]\n'Super+Q'='close'\n",
            ),
            (
                Target::Sway,
                "sway",
                "include $KEY_DIR/child.sway\n",
                "child.sway",
                "bindsym Mod4+q kill\n",
            ),
            (
                Target::Niri,
                "niri.kdl",
                "include \"~/child.kdl\"\n",
                "child.kdl",
                "binds { Super+Q { close-window; }; }\n",
            ),
        ] {
            let root = directory.path().join(name);
            let child = server.join(child_name);
            std::fs::write(&root, root_text).unwrap();
            std::fs::write(&child, child_text).unwrap();
            let scan = scan_roots_in(target, [root.clone()], &environment);
            assert!(scan.errors.is_empty(), "{name}: {:?}", scan.errors);
            assert!(scan.sources.iter().any(|source| source.file == child));
            if name != "mango.toml" && target != Target::Niri {
                assert!(
                    crate::compositor::existing_chords_checked_in(target, &root, &environment)
                        .unwrap()
                        .contains("super+q"),
                    "{name}: checked conflicts use the native environment"
                );
            }
            assert!(
                scan.sources
                    .iter()
                    .flat_map(|source| &source.bindings)
                    .any(|row| row.chord.as_deref() == Some("super+q")),
                "{name}"
            );
            std::fs::remove_file(child).unwrap();
            assert!(
                !scan_roots_in(target, [root], &environment)
                    .errors
                    .is_empty(),
                "{name}: missing required source"
            );
        }
    }

    #[test]
    fn sway_scan_uses_in_place_variables_and_only_resolves_default_mode_conflicts() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("config");
        let child = directory.path().join("keys");
        let mode = directory.path().join("mode");
        std::fs::write(&root, "set $mod Mod4\nset $keys keys\ninclude $keys\nmode resize {\ninclude mode\n}\nbindsym $mod+w kill\n").unwrap();
        std::fs::write(&child, "bindsym $mod+q kill\n").unwrap();
        std::fs::write(&mode, "bindsym Mod4+e mode default\n").unwrap();
        let scan = scan_from(Target::Sway, &root);
        assert!(scan.errors.is_empty(), "{:?}", scan.errors);
        assert_eq!(scan.sources.len(), 3);
        for source in &scan.sources {
            assert!(source.unchanged().unwrap());
            assert_eq!(source.bindings.len(), 1);
            let expected = if source.file == root {
                Some("super+w")
            } else if source.file == child {
                Some("super+q")
            } else {
                None
            };
            assert_eq!(source.bindings[0].chord.as_deref(), expected);
        }
    }

    #[test]
    fn merged_roots_share_limits_and_canonical_source_identity() {
        let directory = tempfile::tempdir().unwrap();
        let one = directory.path().join("one.xml");
        let two = directory.path().join("two.xml");
        let body = "<labwc_config><keyboard><keybind key=\"W-q\"><action name=\"Close\"/></keybind></keyboard></labwc_config>";
        std::fs::write(&one, body).unwrap();
        std::fs::write(&two, body).unwrap();
        let scan = scan_roots(Target::Labwc, [one.clone(), two.clone(), one.clone()]);
        assert!(scan.errors.is_empty());
        assert_eq!(
            scan.sources
                .iter()
                .map(|source| &source.file)
                .collect::<Vec<_>>(),
            [&one, &two]
        );
        assert!(
            !scan_roots(Target::Labwc, std::iter::repeat_n(one, MAX_FILES + 1))
                .errors
                .is_empty()
        );
    }

    #[test]
    fn scanning_follows_includes_deduplicates_symlinks_and_rejects_oversized_files() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("config.conf");
        let child = dir.path().join("keys.conf");
        std::fs::write(
            &root,
            "source=keys.conf\nsource=alias.conf\nbind=SUPER,Q,killclient\n",
        )
        .unwrap();
        std::fs::write(&child, "source=config.conf\nbind=SUPER,T,spawn,foot\n").unwrap();
        symlink(&child, dir.path().join("alias.conf")).unwrap();
        let scan = scan_from(Target::Mango, &root);
        assert!(scan.errors.is_empty(), "{:?}", scan.errors);
        assert_eq!(scan.sources.len(), 2);
        assert_eq!(
            scan.sources.iter().map(|s| s.bindings.len()).sum::<usize>(),
            2
        );
        std::fs::write(&child, vec![b'a'; MAX_FILE_BYTES + 1]).unwrap();
        assert_eq!(scan_from(Target::Mango, &root).errors.len(), 1);
    }

    #[test]
    fn bounded_globs_fail_visibly_instead_of_allocating_an_unbounded_file_list() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..10 {
            std::fs::write(dir.path().join(format!("{index}.conf")), "").unwrap();
        }
        let root = dir.path().join("config");
        assert!(
            included_files_bounded_in(
                Target::Sway,
                &root,
                "include *.conf",
                4,
                &IncludeEnvironment::default()
            )
            .is_err()
        );
        assert_eq!(
            included_files_bounded_in(
                Target::Sway,
                &root,
                "include *.conf",
                10,
                &IncludeEnvironment::default()
            )
            .unwrap()
            .len(),
            10
        );
    }

    #[test]
    fn lua_aliases_and_custom_names_keep_the_native_literal_parser() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bindings.lua");
        let alias = dir.path().join("custom.conf");
        let body = "os.execute('touch /never-run')\nhl.bind('SUPER + Q', hl.dsp.kill_active())\n";
        std::fs::write(&file, body).unwrap();
        symlink(&file, &alias).unwrap();
        for root in [&alias, &file] {
            let scan = scan_from(Target::Hyprland, root);
            assert!(scan.errors.is_empty(), "{:?}", scan.errors);
            assert_eq!(scan.sources[0].bindings.len(), 1);
            let id = scan.sources[0].binding_id(0).unwrap();
            assert!(
                super::super::prepare(
                    Target::Hyprland,
                    &id,
                    &["'SUPER + W'".into(), "kill_active".into()]
                )
                .unwrap()
                .changed()
            );
        }
        assert_eq!(
            super::super::parse(Target::Hyprland, Path::new("custom"), body)
                .unwrap()
                .len(),
            1
        );
    }
    #[test]
    fn a_legacy_source_alias_cannot_change_the_editing_dialect() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("bindings.conf");
        let alias = directory.path().join("alias.lua");
        let original = "bind = SUPER, Q, exec, foot # keep\n";
        std::fs::write(&file, original).unwrap();
        symlink(&file, &alias).unwrap();
        let scan = scan_from(Target::Hyprland, &alias);
        assert!(scan.errors.is_empty(), "{:?}", scan.errors);
        let source = &scan.sources[0];
        let id = source.binding_id(0).unwrap();
        let mut values: Vec<_> = source.bindings[0]
            .fields
            .iter()
            .map(|field| field.value.clone())
            .collect();
        values[2] = "W".into();
        let edit = super::super::prepare(Target::Hyprland, &id, &values).unwrap();
        assert_eq!(edit.body(), original.replace(", Q,", ", W,"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), original);
        assert!(alias.is_symlink());
    }
}
