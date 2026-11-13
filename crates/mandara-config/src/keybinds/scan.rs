//! Bounded include discovery, source identities and row storage for Studio.

use std::{
    collections::BTreeSet,
    hash::{Hash, Hasher},
    io::Read as _,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, ensure};

use super::{Binding, MAX_BINDINGS, MAX_FILE_BYTES, MAX_FILES, MAX_TOTAL_BYTES};
use crate::compositor::{Target, included_files_bounded, is_generated};
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

pub(crate) fn read(path: &Path) -> Result<String> {
    ensure!(
        std::fs::metadata(path)?.is_file(),
        "configuration is not a regular file"
    );
    let file = std::fs::File::open(path).with_context(|| path.display().to_string())?;
    ensure!(
        file.metadata()?.is_file(),
        "configuration is not a regular file"
    );
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
        Target::Mango
            if !target.user_config_in(config, home).exists()
                && config.join("mango/config.toml").exists() =>
        {
            config.join("mango/config.toml")
        }
        _ => target.user_config_in(config, home),
    }
}

pub fn scan_from(target: Target, root: &Path) -> Scan {
    scan_roots(target, [root.to_path_buf()])
}

/// A merged native config shares the same file, byte and binding budgets.
pub fn scan_roots(target: Target, roots: impl IntoIterator<Item = PathBuf>) -> Scan {
    let mut scan = Scan::default();
    let mut seen = BTreeSet::new();
    let mut queue: Vec<_> = roots.into_iter().take(MAX_FILES + 1).collect();
    if queue.len() > MAX_FILES {
        scan.errors.push((
            PathBuf::new(),
            "too many shortcut configuration files".into(),
        ));
        return scan;
    }
    if target == Target::Sway {
        return scan_sway(&queue);
    }
    if target == Target::Niri {
        return niri::scan(&queue);
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
                "too many shortcut configuration files"
            );
            let source = read(&file)?;
            total_bytes += source.len();
            ensure!(
                total_bytes <= MAX_TOTAL_BYTES,
                "shortcut configuration exceeds the scan budget"
            );
            let bindings = super::parse(target, &real, &source)?;
            total_bindings += bindings.len();
            ensure!(
                total_bindings <= MAX_BINDINGS,
                "too many shortcuts in included files"
            );
            let mut children =
                if target == Target::Mango && file.extension().is_some_and(|ext| ext == "toml") {
                    mango_includes(&file, &source)?
                } else {
                    included_files_bounded(
                        target,
                        &file,
                        &source,
                        MAX_FILES.saturating_sub(seen.len() + queue.len()),
                    )?
                };
            ensure!(
                seen.len() + queue.len() + children.len() <= MAX_FILES,
                "too many included shortcut files"
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

fn scan_sway(roots: &[PathBuf]) -> Scan {
    let result = (|| -> Result<Scan> {
        let mut native = crate::compositor::sway_shortcut_sources(roots)?;
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
                "too many shortcuts in included files"
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

fn mango_includes(file: &Path, source: &str) -> Result<Vec<PathBuf>> {
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
                        let path = crate::compositor::resolve_include(file, path);
                        if key != "source_optional" || path.exists() {
                            paths.push(path);
                        }
                    }
                }
            }
        }
    }
    ensure!(paths.len() <= MAX_FILES, "too many included shortcut files");
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(included_files_bounded(Target::Sway, &root, "include *.conf", 4).is_err());
        assert_eq!(
            included_files_bounded(Target::Sway, &root, "include *.conf", 10)
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
}
