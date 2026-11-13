//! Sway reads included files at the include's position, sharing variables and blocks.

use anyhow::{Result, ensure};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::{
    IncludeEnvironment, SwayBind, Target, include_path, is_generated, sway_binds_in,
    sway_include_files,
};

pub(crate) fn binds(files: &[PathBuf]) -> Vec<(PathBuf, SwayBind)> {
    let mut reader = Reader::default();
    for file in files {
        let _ = reader.read(file);
    }
    reader.binds
}

pub(super) fn checked_in(
    root: &Path,
    environment: &IncludeEnvironment,
) -> Result<BTreeSet<String>> {
    let mut reader = Reader {
        budget: Some(Default::default()),
        environment: Some(environment),
        ..Default::default()
    };
    reader.read(root)?;
    Ok(reader
        .binds
        .into_iter()
        .map(|(_, binding)| binding.chord)
        .collect())
}

#[derive(Default)]
pub(crate) struct ShortcutSources {
    pub files: Vec<(PathBuf, String)>,
    pub generated: Vec<PathBuf>,
    pub bindings: Vec<(PathBuf, SwayBind)>,
}

pub(crate) fn shortcut_sources_in(
    files: &[PathBuf],
    environment: &IncludeEnvironment,
) -> Result<ShortcutSources> {
    let mut reader = Reader {
        budget: Some(Default::default()),
        sources: Some(Default::default()),
        environment: Some(environment),
        ..Default::default()
    };
    for file in files {
        reader.read(file)?;
    }
    let mut sources = reader.sources.expect("source collection is enabled");
    sources.bindings = reader.binds;
    Ok(sources)
}

#[derive(Default)]
struct Reader<'a> {
    vars: Vec<(String, String)>,
    blocks: Vec<String>,
    visited: BTreeSet<PathBuf>,
    binds: Vec<(PathBuf, SwayBind)>,
    retained: usize,
    budget: Option<super::read_budget::ReadBudget>,
    sources: Option<ShortcutSources>,
    environment: Option<&'a IncludeEnvironment>,
}

impl Reader<'_> {
    fn chunk(&mut self, file: &Path, text: &str, offset: usize) -> Result<()> {
        self.binds.extend(
            sway_binds_in(text, &mut self.vars, &mut self.blocks, &mut self.retained)?
                .into_iter()
                .map(|mut bind| {
                    bind.line += offset;
                    (file.to_path_buf(), bind)
                }),
        );
        ensure!(
            self.budget.is_none() || self.binds.len() <= crate::keybinds::MAX_BINDINGS,
            "too many compositor shortcuts"
        );
        Ok(())
    }

    fn read(&mut self, file: &Path) -> Result<()> {
        self.read_file(file, true)
    }

    fn read_file(&mut self, file: &Path, main: bool) -> Result<()> {
        let key = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
        if file == Target::Sway.generated_file() || is_generated(file) || is_generated(&key) {
            if let Some(sources) = &mut self.sources
                && !sources.generated.iter().any(|path| path == file)
            {
                sources.generated.push(file.into());
            }
            return Ok(());
        }
        if !self.visited.insert(key.clone()) {
            return Ok(());
        }
        let text = match &mut self.budget {
            Some(budget) => budget.read(file)?,
            None => std::fs::read_to_string(file).ok(),
        };
        let Some(text) = text else {
            return Ok(());
        };
        let source_index = self.sources.as_mut().map(|sources| {
            let index = sources.files.len();
            sources.files.push((file.into(), String::new()));
            index
        });
        let (mut chunk, mut start) = (String::new(), 0);
        let containing = if main { file } else { key.as_path() };
        for (line_number, line) in text.lines().enumerate() {
            let included = (!line.trim_start().starts_with('#'))
                .then(|| include_path(Target::Sway, line))
                .flatten();
            if let Some(raw) = included {
                self.chunk(file, &chunk, start)?;
                chunk.clear();
                start = line_number + 1;
                let raw = super::sway_vars::expand(&raw, &self.vars)?;
                let files = match &self.budget {
                    Some(budget) => {
                        if let Some(environment) = self.environment {
                            super::sway_include_files_bounded_in(
                                containing,
                                &raw,
                                budget.remaining_files(),
                                environment,
                            )?
                        } else {
                            super::sway_include_files_bounded(
                                containing,
                                &raw,
                                budget.remaining_files(),
                            )?
                        }
                    }
                    None => sway_include_files(containing, &raw),
                };
                for child in files {
                    self.read_file(&child, false)?;
                }
            } else {
                chunk.push_str(line);
                chunk.push('\n');
            }
        }
        self.chunk(file, &chunk, start)?;
        if let (Some(sources), Some(index)) = (&mut self.sources, source_index) {
            sources.files[index].1 = text;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expanded_commands_share_a_retained_budget_across_bindings_and_includes() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("config");
        let child = directory.path().join("keys");
        let mut body = format!("set $command {}\n", "x".repeat(128 * 1024));
        for _ in 0..crate::keybinds::MAX_BINDINGS {
            body.push_str("bindsym Mod4+q exec $command\n");
        }
        std::fs::write(&root, &body).unwrap();
        assert!(body.len() < crate::keybinds::MAX_FILE_BYTES);
        assert!(
            shortcut_sources_in(std::slice::from_ref(&root), &IncludeEnvironment::default())
                .is_err()
        );
        assert!(crate::keybinds::parse(Target::Sway, &root, &body).is_err());
        body = format!("set $command {}\n", "x".repeat(128 * 1024));
        for _ in 0..20 {
            body.push_str("bindsym Mod4+q exec $command\n");
        }
        std::fs::write(&child, body.lines().skip(1).collect::<Vec<_>>().join("\n")).unwrap();
        body.push_str("include keys\n");
        std::fs::write(&root, body).unwrap();
        assert!(shortcut_sources_in(&[root], &IncludeEnvironment::default()).is_err());
    }

    #[test]
    fn exponentially_expanded_variables_fail_before_their_output_is_allocated() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("config");
        let mut body = String::from("set $v0 x\n");
        for index in 1..32 {
            body.push_str(&format!("set $v{index} $v{}$v{}\n", index - 1, index - 1));
        }
        body.push_str("bindsym Mod4+q exec $v31\n");
        std::fs::write(&root, &body).unwrap();
        assert!(body.len() < crate::keybinds::MAX_FILE_BYTES);
        assert!(
            shortcut_sources_in(std::slice::from_ref(&root), &IncludeEnvironment::default())
                .err()
                .unwrap()
                .to_string()
                .contains("budget")
        );
        assert!(checked_in(&root, &IncludeEnvironment::default()).is_err());
        assert!(crate::keybinds::parse(Target::Sway, &root, &body).is_err());
    }

    #[test]
    fn included_aliases_resolve_children_beside_the_canonical_source() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let links = directory.path().join("links");
        let real = directory.path().join("real");
        std::fs::create_dir_all(&links).unwrap();
        std::fs::create_dir_all(&real).unwrap();
        let root = directory.path().join("config");
        std::fs::write(&root, "include links/keys.conf\n").unwrap();
        std::fs::write(real.join("keys.conf"), "include extra.conf\n").unwrap();
        std::fs::write(real.join("extra.conf"), "bindsym Mod4+q kill\n").unwrap();
        std::fs::write(links.join("extra.conf"), "bindsym Mod4+w kill\n").unwrap();
        symlink(real.join("keys.conf"), links.join("keys.conf")).unwrap();
        let found = shortcut_sources_in(&[root], &IncludeEnvironment::default()).unwrap();
        assert_eq!(found.bindings.len(), 1);
        assert_eq!(found.bindings[0].1.chord, "super+q");
        assert!(
            found
                .files
                .iter()
                .any(|(file, _)| file == &links.join("keys.conf"))
        );

        let alias = links.join("main");
        std::fs::write(real.join("main"), "include extra.conf\n").unwrap();
        symlink(real.join("main"), &alias).unwrap();
        assert_eq!(
            shortcut_sources_in(&[alias], &IncludeEnvironment::default())
                .unwrap()
                .bindings[0]
                .1
                .chord,
            "super+w"
        );
    }

    #[test]
    fn includes_share_the_variables_and_mode_at_their_position() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("config");
        let child = dir.path().join("keys.conf");
        std::fs::write(&root, "set $mod Mod4\nset $menu wofi\nset $keys keys.conf\ninclude $keys\nset $menu mandara\nbindsym $mod+x exec $menu\nmode resize {\ninclude keys.conf\n}\n").unwrap();
        std::fs::write(&child, "bindsym $mod+space exec $menu\n").unwrap();
        let found = binds(&[root.clone(), child.clone()]);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].0, child);
        assert_eq!(found[0].1.command, "exec wofi");
        assert_eq!(found[1].0, root);
        assert_eq!(found[1].1.command, "exec mandara");
    }

    #[test]
    fn an_include_inside_a_mode_does_not_become_a_default_mode_bind() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("config");
        std::fs::write(
            &root,
            "mode resize {\ninclude mode.conf\n}\nbindsym Mod4+x exec foot\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("mode.conf"), "bindsym Mod4+r exec resize\n").unwrap();
        let found = binds(&[root]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1.chord, "super+x");
    }

    #[test]
    fn checked_includes_keep_variables_and_reject_oversized_or_missing_sources() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("config");
        let child = directory.path().join("keys");
        std::fs::write(
            &root,
            "set $mod Mod4\nset $keys keys\ninclude $keys\nbindsym Mod1+q kill\n",
        )
        .unwrap();
        std::fs::write(&child, "include config\nbindsym $mod+q kill\n").unwrap();
        assert_eq!(
            checked_in(&root, &IncludeEnvironment::default()).unwrap(),
            BTreeSet::from(["alt+q".into(), "super+q".into()])
        );
        std::fs::write(&child, vec![b'x'; crate::keybinds::MAX_FILE_BYTES + 1]).unwrap();
        assert!(checked_in(&root, &IncludeEnvironment::default()).is_err());
        std::fs::remove_file(&child).unwrap();
        assert!(checked_in(&root, &IncludeEnvironment::default()).is_err());
    }
}
