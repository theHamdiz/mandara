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
            "too many shortcuts in included files"
        );
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
    #[test]
    fn config_variables_and_inherited_submaps_follow_includes_and_reassignments() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("hyprland.conf");
        let child = directory.path().join("keys.conf");
        std::fs::write(&root, "$dir = .\n$keys = $dir/keys.conf\n$mod = SUPER\nsubmap = resize\nsource = $keys\nsubmap = reset\nbind = $mod, W, exec, foot\n").unwrap();
        std::fs::write(&child, "bind = $mod, Q, exec, foot\n$mod = ALT\n").unwrap();
        let found = scan(std::slice::from_ref(&root), &IncludeEnvironment::captured([]));
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
        std::fs::write(&generated, "bind = SUPER, E, exec, mandara\n").unwrap();
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
