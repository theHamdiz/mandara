//! Sway reads included files at the include's position, sharing variables and blocks.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::{SwayBind, Target, include_path, is_generated, sway_binds_in, sway_include_files};

pub(crate) fn binds(files: &[PathBuf]) -> Vec<(PathBuf, SwayBind)> {
    let mut reader = Reader::default();
    for file in files { reader.read(file); }
    reader.binds
}

#[derive(Default)]
struct Reader {
    vars: Vec<(String, String)>,
    blocks: Vec<String>,
    visited: BTreeSet<PathBuf>,
    binds: Vec<(PathBuf, SwayBind)>,
}

impl Reader {
    fn chunk(&mut self, file: &Path, text: &str, offset: usize) {
        self.binds.extend(sway_binds_in(text, &mut self.vars, &mut self.blocks).into_iter().map(|mut bind| {
            bind.line += offset;
            (file.to_path_buf(), bind)
        }));
    }

    fn read(&mut self, file: &Path) {
        let key = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
        if file == Target::Sway.generated_file() || is_generated(file) || !self.visited.insert(key) { return; }
        let Ok(text) = std::fs::read_to_string(file) else { return };
        let (mut chunk, mut start) = (String::new(), 0);
        for (line_number, line) in text.lines().enumerate() {
            let included = (!line.trim_start().starts_with('#')).then(|| include_path(Target::Sway, line)).flatten();
            if let Some(raw) = included {
                self.chunk(file, &chunk, start);
                chunk.clear();
                start = line_number + 1;
                let raw = self.vars.iter().fold(raw, |value, (name, replacement)| value.replace(name, replacement));
                for child in sway_include_files(file, &raw) { self.read(&child); }
            } else {
                chunk.push_str(line);
                chunk.push('\n');
            }
        }
        self.chunk(file, &chunk, start);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        std::fs::write(&root, "mode resize {\ninclude mode.conf\n}\nbindsym Mod4+x exec foot\n").unwrap();
        std::fs::write(dir.path().join("mode.conf"), "bindsym Mod4+r exec resize\n").unwrap();
        let found = binds(&[root]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1.chord, "super+x");
    }
}
