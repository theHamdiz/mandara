//! Niri includes are positional; Mod is resolved after all input sections merge.

use super::*;

#[derive(Default, Debug, serde::Serialize)]
pub struct NiriModifiers {
    normal: Option<String>,
    nested: Option<String>,
    pub nested_session: bool,
}

pub fn resolve_mod(scan: &mut Scan, nested: bool) {
    let Some(modifiers) = &mut scan.niri else {
        return;
    };
    modifiers.nested_session = nested;
    let normal = modifiers.normal.as_deref().unwrap_or("Super");
    // Niri swaps Alt back to Super in a nested window unless explicitly set.
    let modifier = if nested {
        modifiers
            .nested
            .as_deref()
            .unwrap_or(if normal.eq_ignore_ascii_case("Alt") {
                "Super"
            } else {
                "Alt"
            })
    } else {
        normal
    };
    for row in scan
        .sources
        .iter_mut()
        .flat_map(|source| &mut source.bindings)
    {
        if let Some(field) = row.fields.iter().find(|field| field.label == "Shortcut") {
            let mut value = String::with_capacity(field.value.len() + modifier.len());
            for (index, part) in field.value.split('+').enumerate() {
                if index != 0 {
                    value.push('+');
                }
                value.push_str(if part.eq_ignore_ascii_case("Mod") {
                    modifier
                } else {
                    part
                });
            }
            row.chord = super::super::normalize(&value);
        }
    }
}

pub(super) fn scan(roots: &[PathBuf]) -> Scan {
    let mut scan = Scan {
        niri: Some(Box::default()),
        ..Default::default()
    };
    let mut visited = BTreeSet::new();
    let mut bytes = 0;
    let mut count = 0;
    for root in roots {
        if let Err(error) = walk(root, &mut scan, &mut visited, &mut bytes, &mut count) {
            scan.errors.push((root.clone(), format!("{error:#}")));
            break;
        }
    }
    resolve_mod(&mut scan, false);
    scan
}

fn walk(
    file: &Path,
    scan: &mut Scan,
    visited: &mut BTreeSet<PathBuf>,
    bytes: &mut usize,
    count: &mut usize,
) -> Result<()> {
    let real = std::fs::canonicalize(file)?;
    if is_generated(file) || is_generated(&real) {
        if !scan.generated.iter().any(|path| path == file) {
            scan.generated.push(file.into());
        }
        return Ok(());
    }
    if !visited.insert(real.clone()) {
        return Ok(());
    }
    ensure!(
        visited.len() <= MAX_FILES,
        "too many shortcut configuration files"
    );
    let text = read(file)?;
    *bytes += text.len();
    ensure!(
        *bytes <= MAX_TOTAL_BYTES,
        "shortcut configuration exceeds the scan budget"
    );
    let document = kdl::KdlDocument::parse_v1(&text)?;
    let bindings = super::super::formats::niri::from_document(&text, &document);
    super::super::validate_bindings(&bindings)?;
    *count += bindings.len();
    ensure!(
        *count <= MAX_BINDINGS,
        "too many shortcuts in included files"
    );
    scan.sources.push(Source {
        file: file.into(),
        target: real,
        fingerprint: fingerprint(&text),
        bindings,
    });
    for node in document.nodes() {
        match node.name().value() {
            "include" => {
                let path = node
                    .get(0)
                    .and_then(kdl::KdlValue::as_string)
                    .ok_or_else(|| anyhow::anyhow!("Niri include has no literal path"))?;
                let path = crate::compositor::resolve_include(file, path);
                let optional = node
                    .get("optional")
                    .and_then(kdl::KdlValue::as_bool)
                    .unwrap_or(false);
                if path.exists() || !optional {
                    walk(&path, scan, visited, bytes, count)?;
                }
            }
            "input" => {
                if let Some(input) = node.children() {
                    for setting in input.nodes() {
                        let modifier = match setting.name().value() {
                            "mod-key" => &mut scan.niri.as_mut().expect("Niri scan").normal,
                            "mod-key-nested" => &mut scan.niri.as_mut().expect("Niri scan").nested,
                            _ => continue,
                        };
                        *modifier = setting
                            .get(0)
                            .and_then(kdl::KdlValue::as_string)
                            .map(str::to_owned);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inline_includes_merge_modifiers_in_place_and_nested_defaults_follow_niri() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("config.kdl");
        let child = directory.path().join("keys.kdl");
        std::fs::write(&root, "input { mod-key \"Ctrl\"; }; include \"keys.kdl\"; input { mod-key \"Alt\"; }; include optional=true \"missing\"\n").unwrap();
        std::fs::write(
            &child,
            "input { mod-key \"Super\"; }; binds { Mod+Q { close-window; }; }\n",
        )
        .unwrap();
        let mut scan = scan(&[root]);
        assert!(scan.errors.is_empty(), "{:?}", scan.errors);
        let row = |scan: &Scan| scan.sources[1].bindings[0].chord.clone();
        assert_eq!(row(&scan).as_deref(), Some("alt+q"));
        resolve_mod(&mut scan, true);
        assert_eq!(row(&scan).as_deref(), Some("super+q"));
        scan.niri.as_mut().unwrap().nested = Some("Mod5".into());
        resolve_mod(&mut scan, true);
        assert_eq!(row(&scan).as_deref(), Some("mod5+q"));
        assert!(
            scan.sources
                .iter()
                .all(|source| source.unchanged().unwrap())
        );
    }
}
