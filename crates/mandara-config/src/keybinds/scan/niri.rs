//! Niri includes are positional; Mod is resolved after all input sections merge.

use super::*;

#[derive(Default, Debug, serde::Serialize)]
pub struct NiriModifiers {
    normal: Option<&'static str>,
    nested: Option<&'static str>,
    pub nested_session: bool,
}

fn modifier_name(value: &str) -> Option<&'static str> {
    [
        ("Ctrl", "Ctrl"),
        ("Control", "Ctrl"),
        ("Shift", "Shift"),
        ("Alt", "Alt"),
        ("Super", "Super"),
        ("Win", "Super"),
        ("ISO_Level3_Shift", "Mod5"),
        ("Mod5", "Mod5"),
        ("ISO_Level5_Shift", "Mod3"),
        ("Mod3", "Mod3"),
    ]
    .into_iter()
    .find_map(|(name, canonical)| name.eq_ignore_ascii_case(value).then_some(canonical))
}

pub fn resolve_mod(scan: &mut Scan, nested: bool) {
    let Some(modifiers) = &mut scan.niri else {
        return;
    };
    modifiers.nested_session = nested;
    let normal = modifiers.normal.unwrap_or("Super");
    // Niri swaps Alt back to Super in a nested window unless explicitly set.
    let modifier = if nested {
        modifiers
            .nested
            .unwrap_or(if normal.eq_ignore_ascii_case("Alt") {
                "Super"
            } else {
                "Alt"
            })
    } else {
        normal
    };
    let mut retained = 0_usize;
    for source in &mut scan.sources {
        for row in &mut source.bindings {
            let Some(field) = row.fields.iter().find(|field| field.label == "Shortcut") else {
                continue;
            };
            let bytes =
                field
                    .value
                    .split('+')
                    .enumerate()
                    .try_fold(0_usize, |bytes, (index, part)| {
                        bytes.checked_add(usize::from(index != 0))?.checked_add(
                            if part.eq_ignore_ascii_case("Mod") {
                                modifier.len()
                            } else {
                                part.len()
                            },
                        )
                    });
            let Some(bytes) = bytes.filter(|bytes| {
                *bytes <= super::super::MAX_FIELD_BYTES
                    && retained
                        .checked_add(*bytes)
                        .is_some_and(|sum| sum <= MAX_TOTAL_BYTES)
            }) else {
                row.chord = None;
                scan.errors.push((
                    source.file.clone(),
                    mandara_i18n::t("The shortcut configuration is too large to edit.").into(),
                ));
                return;
            };
            retained += bytes;
            let mut value = String::with_capacity(bytes);
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

pub(super) fn scan(roots: &[PathBuf], environment: &IncludeEnvironment) -> Scan {
    let mut scan = Scan {
        niri: Some(Box::default()),
        ..Default::default()
    };
    let mut visited = BTreeSet::new();
    let mut bytes = 0;
    let mut count = 0;
    let mut visits = 0;
    for root in roots {
        if let Err(error) = walk(
            root,
            &mut scan,
            &mut visited,
            &mut bytes,
            &mut count,
            &mut visits,
            environment,
        ) {
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
    visits: &mut usize,
    environment: &IncludeEnvironment,
) -> Result<()> {
    *visits += 1;
    ensure!(
        *visits <= MAX_BINDINGS,
        mandara_i18n::t("The shortcut configuration includes too many files.")
    );
    let real = std::fs::canonicalize(file)?;
    if is_generated(file) || is_generated(&real) {
        if !scan.generated.iter().any(|path| path == file) {
            scan.generated.push(file.into());
        }
        return Ok(());
    }
    ensure!(
        visited.insert(real.clone()),
        mandara_i18n::t("The shortcut configuration contains an include cycle.")
    );
    ensure!(
        visited.len() <= MAX_FILES,
        mandara_i18n::t("The shortcut configuration includes too many files.")
    );
    let text = read(file)?;
    *bytes += text.len();
    ensure!(
        *bytes <= MAX_TOTAL_BYTES,
        mandara_i18n::t("The shortcut configuration is too large to edit.")
    );
    let document = kdl::KdlDocument::parse_v1(&text)?;
    if !scan.sources.iter().any(|source| source.target == real) {
        ensure!(
            scan.sources.len() < MAX_FILES,
            mandara_i18n::t("The shortcut configuration includes too many files.")
        );
        let bindings = super::super::formats::niri::from_document(&text, &document);
        super::super::validate_bindings(&bindings)?;
        *count += bindings.len();
        ensure!(
            *count <= MAX_BINDINGS,
            mandara_i18n::t("The compositor has too many shortcuts to edit at once.")
        );
        scan.sources.push(Source {
            file: file.into(),
            target: real.clone(),
            fingerprint: fingerprint(&text),
            bindings,
        });
    }
    for node in document.nodes() {
        match node.name().value() {
            "include" => {
                let (path, optional) = crate::compositor::niri_include(node).ok_or_else(|| {
                    anyhow::anyhow!(mandara_i18n::t(
                        "Computed shortcuts must be edited in their source configuration."
                    ))
                })?;
                let path = environment.resolve(file, std::ffi::OsStr::new(path))?;
                if path.try_exists()? || !optional {
                    walk(&path, scan, visited, bytes, count, visits, environment)?;
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
                        let value = setting.get(0).and_then(kdl::KdlValue::as_string)
                            .and_then(modifier_name).ok_or_else(|| anyhow::anyhow!(mandara_i18n::t("Some compositor shortcuts cannot be resolved safely. Edit them in their source configuration first.")))?;
                        *modifier = Some(value);
                    }
                }
            }
            _ => {}
        }
    }
    visited.remove(&real);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modifier_names_and_expansions_are_bounded_before_copying() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("config.kdl");
        std::fs::write(
            &root,
            format!(
                "input {{ mod-key \"{}\"; }}\nbinds {{ {}Q {{ quit; }}; }}\n",
                "m".repeat(128 * 1024),
                "Mod+".repeat(8192)
            ),
        )
        .unwrap();
        assert!(
            !scan(
                std::slice::from_ref(&root),
                &IncludeEnvironment::captured([])
            )
            .errors
            .is_empty()
        );
        let chord = format!(
            "{}Q",
            "Mod+".repeat(super::super::super::MAX_FIELD_BYTES / 4 - 1)
        );
        std::fs::write(
            &root,
            format!("input {{ mod-key \"Super\"; }}\nbinds {{ {chord} {{ quit; }}; }}\n"),
        )
        .unwrap();
        assert!(
            !scan(
                std::slice::from_ref(&root),
                &IncludeEnvironment::captured([])
            )
            .errors
            .is_empty()
        );
        for (name, expected) in [
            ("Control", "ctrl+q"),
            ("Win", "super+q"),
            ("ISO_Level3_Shift", "mod5+q"),
            ("ISO_Level5_Shift", "hyper+q"),
        ] {
            std::fs::write(
                &root,
                format!("input {{ mod-key \"{name}\"; }}\nbinds {{ Mod+Q {{ quit; }}; }}\n"),
            )
            .unwrap();
            let result = scan(
                std::slice::from_ref(&root),
                &IncludeEnvironment::captured([]),
            );
            assert!(result.errors.is_empty(), "{:?}", result.errors);
            assert_eq!(
                result.sources[0].bindings[0].chord.as_deref(),
                Some(expected)
            );
        }
    }

    #[test]
    fn repeated_includes_reapply_settings_and_recursive_includes_fail() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("config.kdl");
        std::fs::write(
            directory.path().join("mods.kdl"),
            "input { mod-key \"Super\"; }\n",
        )
        .unwrap();
        std::fs::write(&root, "include \"mods.kdl\"; input { mod-key \"Alt\"; }; include \"mods.kdl\"; binds { Mod+Q { close-window; }; }\n").unwrap();
        let result = scan(std::slice::from_ref(&root), &IncludeEnvironment::default());
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(result.sources.len(), 2);
        assert_eq!(
            result.sources[0].bindings[0].chord.as_deref(),
            Some("super+q")
        );
        std::fs::write(
            directory.path().join("mods.kdl"),
            "include \"config.kdl\"\n",
        )
        .unwrap();
        let result = scan(&[root], &IncludeEnvironment::default());
        assert!(result.errors[0].1.contains(mandara_i18n::t(
            "The shortcut configuration contains an include cycle."
        )));
    }
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
        let mut scan = scan(&[root], &IncludeEnvironment::default());
        assert!(scan.errors.is_empty(), "{:?}", scan.errors);
        let row = |scan: &Scan| scan.sources[1].bindings[0].chord.clone();
        assert_eq!(row(&scan).as_deref(), Some("alt+q"));
        resolve_mod(&mut scan, true);
        assert_eq!(row(&scan).as_deref(), Some("super+q"));
        scan.niri.as_mut().unwrap().nested = Some("Mod5");
        resolve_mod(&mut scan, true);
        assert_eq!(row(&scan).as_deref(), Some("mod5+q"));
        assert!(
            scan.sources
                .iter()
                .all(|source| source.unchanged().unwrap())
        );
    }
}
