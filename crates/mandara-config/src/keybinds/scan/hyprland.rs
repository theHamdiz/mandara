//! Legacy Hyprland rows share the validator's positional include traversal.

use super::*;
use crate::compositor::hyprland_probe::{self, Visitor};

pub(super) fn scan(roots: &[PathBuf], environment: &IncludeEnvironment) -> Scan {
    let mut collector = Collector::default();
    if let Err(error) = hyprland_probe::visit_sources(roots, environment, &mut collector) {
        collector.scan.errors.push((
            roots.first().cloned().unwrap_or_default(),
            format!("{error:#}"),
        ));
    }
    collector.scan
}

#[derive(Default)]
struct Collector {
    scan: Scan,
    bindings: usize,
    scope_bytes: usize,
    contexts: BTreeSet<(usize, usize)>,
}

impl Visitor for Collector {
    fn source(&mut self, file: &Path, target: &Path, source: &str) -> Result<()> {
        if is_generated(file) || is_generated(target) {
            if !self.scan.generated.iter().any(|path| path == file) {
                self.scan.generated.push(file.into());
            }
            return Ok(());
        }
        if self
            .scan
            .sources
            .iter()
            .any(|source| source.target == target)
        {
            return Ok(());
        }
        let bindings = super::super::parse(Target::Hyprland, target, source)?;
        self.bindings += bindings.len();
        ensure!(
            self.bindings <= MAX_BINDINGS,
            mandara_i18n::t("The compositor has too many shortcuts to edit at once.")
        );
        for row in &bindings {
            self.scope_bytes = self.scope_bytes.checked_add(row.scope.len())
                .filter(|bytes| *bytes <= MAX_TOTAL_BYTES)
                .context(mandara_i18n::t("The shortcut configuration is too large to edit."))?;
        }
        self.scan.sources.push(Source {
            file: file.into(),
            target: target.into(),
            fingerprint: fingerprint(source),
            bindings,
        });
        Ok(())
    }

    fn binding(&mut self, target: &Path, line: usize, scope: &str, expanded: &str) -> Result<()> {
        let Some((source_index, source)) = self
            .scan
            .sources
            .iter_mut()
            .enumerate()
            .find(|(_, source)| source.target == target)
        else {
            return Ok(());
        };
        let Ok(index) = source
            .bindings
            .binary_search_by_key(&line, |binding| binding.line)
        else {
            return Ok(());
        };
        let row = &mut source.bindings[index];
        let chord = hyprland_probe::chord(expanded);
        if self.contexts.insert((source_index, index)) {
            self.scope_bytes = self
                .scope_bytes
                .checked_sub(row.scope.len())
                .and_then(|bytes| bytes.checked_add(scope.len()))
                .filter(|bytes| *bytes <= MAX_TOTAL_BYTES)
                .context(mandara_i18n::t("The shortcut configuration is too large to edit."))?;
            row.scope = scope.into();
            row.chord = chord;
        } else if row.scope != scope || row.chord != chord {
            row.chord = None;
            row.read_only = Some(super::super::DYNAMIC);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keybinds::MAX_FIELD_BYTES;
    #[test]
    fn expanded_submap_copies_share_a_retained_byte_budget() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("hyprland.conf");
        let source = format!(
            "$mode = {}\nsubmap = $mode\n{}",
            "m".repeat(MAX_FIELD_BYTES),
            "bind = SUPER, Q, exec, foot\n".repeat(MAX_TOTAL_BYTES / MAX_FIELD_BYTES + 1)
        );
        assert!(source.len() < MAX_FILE_BYTES);
        std::fs::write(&file, source).unwrap();
        let found = scan(&[file], &IncludeEnvironment::captured([]));
        assert!(!found.errors.is_empty());
        assert!(
            found
                .sources
                .iter()
                .flat_map(|source| &source.bindings)
                .map(|row| row.scope.len())
                .sum::<usize>()
                <= MAX_TOTAL_BYTES
        );
    }
    #[test]
    fn config_variables_and_inherited_submaps_follow_includes_and_reassignments() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("hyprland.conf");
        let child = directory.path().join("keys.conf");
        std::fs::write(&root, "$dir = .\n$keys = $dir/keys.conf\n$mod = SUPER\nsubmap = resize\nsource = $keys\nsubmap = reset\nbind = $mod, W, exec, foot\n").unwrap();
        std::fs::write(&child, "bind = $mod, Q, exec, foot\n$mod = ALT\n").unwrap();
        let found = scan(
            std::slice::from_ref(&root),
            &IncludeEnvironment::captured([]),
        );
        assert!(found.errors.is_empty(), "{:?}", found.errors);
        let selected = found
            .sources
            .iter()
            .find(|source| source.file == child)
            .unwrap();
        assert_eq!(selected.bindings[0].scope, "resize");
        assert_eq!(selected.bindings[0].chord.as_deref(), Some("super+q"));
        assert_eq!(found.sources[0].bindings[0].chord.as_deref(), Some("alt+w"));
        assert!(selected.unchanged().unwrap());
        let generated = directory.path().join("mandara/generated/hyprland.conf");
        std::fs::create_dir_all(generated.parent().unwrap()).unwrap();
        let managed = directory.path().join("managed.conf");
        std::fs::write(&managed, "bind = SUPER, E, exec, mandara\n").unwrap();
        std::os::unix::fs::symlink(&managed, &generated).unwrap();
        let body = std::fs::read_to_string(&root).unwrap();
        std::fs::write(
            &root,
            format!("{body}source = mandara/generated/hyprland.conf\n"),
        )
        .unwrap();
        assert_eq!(
            crate::compositor::existing_chords_checked_in(
                Target::Hyprland,
                &root,
                &IncludeEnvironment::captured([])
            )
            .unwrap(),
            BTreeSet::from(["alt+w".into()])
        );
        let mut body = std::fs::read_to_string(&root).unwrap();
        body.push_str("source = $keys\n");
        std::fs::write(&root, body).unwrap();
        let found = scan(&[root], &IncludeEnvironment::captured([]));
        assert!(found.errors.is_empty(), "{:?}", found.errors);
        let selected = found
            .sources
            .iter()
            .find(|source| source.file == child)
            .unwrap();
        assert_eq!(selected.bindings.len(), 1);
        assert!(selected.bindings[0].read_only.is_some());
    }
}
