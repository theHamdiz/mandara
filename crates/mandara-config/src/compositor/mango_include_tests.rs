use super::*;

#[test]
fn connections_follow_native_root_paths_and_both_source_formats() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("config.conf");
    let generated = directory.path().join("generated.conf");
    let child = directory.path().join("nested/child.toml");
    std::fs::create_dir(child.parent().unwrap()).unwrap();
    std::fs::write(&root, "source=./nested/child.toml\n").unwrap();
    std::fs::write(&child, "[global]\nsource_optional=['./generated.conf']\n").unwrap();
    assert!(connected(&root, &generated).unwrap());
    // Optional references remain installed while their generated file is being repaired.
    assert!(!generated.exists());
    std::fs::write(&child, "[global]\nsource_optional=['./missing.conf']\n").unwrap();
    assert!(!connected(&root, &generated).unwrap());
}

#[test]
fn config_variables_are_ordered_and_never_borrow_process_variables() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("config.conf");
    let child = directory.path().join("variables.toml");
    let generated = directory.path().join("generated.conf");
    std::fs::write(
        &root,
        format!(
            "var=HOME,{}\nsource=./variables.toml\nsource_optional=${{NEXT}}\n",
            directory.path().display()
        ),
    )
    .unwrap();
    std::fs::write(&child, "[var]\nNEXT='$HOME/generated.conf'\n").unwrap();
    assert!(connected(&root, &generated).unwrap());
    std::fs::write(&root, "source_optional=$HOME/generated.conf\n").unwrap();
    let mut walk = Walk::new(&root, Some(&generated), None).unwrap();
    walk.environment =
        IncludeEnvironment::captured([("HOME".into(), directory.path().as_os_str().to_owned())]);
    walk.file(&root).unwrap();
    assert!(
        !walk.found,
        "MangoWM does not implicitly define a HOME config variable"
    );
    std::fs::write(
        &root,
        format!(
            "source_optional=$HOME/generated.conf\nvar=HOME,{}\n",
            directory.path().display()
        ),
    )
    .unwrap();
    assert!(
        !connected(&root, &generated).unwrap(),
        "a declaration cannot affect an earlier include"
    );
    std::fs::write(
        &root,
        format!(
            "var=NAME,HOME\nvar=$NAME,{}\nsource_optional=$HOME/generated.conf\n",
            directory.path().display()
        ),
    )
    .unwrap();
    assert!(connected(&root, &generated).unwrap());
}

#[test]
fn native_variable_expansion_preserves_unknown_names_without_recursive_expansion() {
    let mut variables = IncludeEnvironment::captured([]);
    variables.define("A", "$B".into()).unwrap();
    variables.define("B", "later".into()).unwrap();
    assert_eq!(
        variables.expand_config("$A/${B}/${}/$UNKNOWN").unwrap(),
        "$B/later/${}/$UNKNOWN"
    );
}

#[test]
fn toml_variables_keep_scalar_spelling_and_native_byte_limits() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("config.toml");
    std::fs::write(&root, "").unwrap();
    let mut walk = Walk::new(&root, None, None).unwrap();
    let scalar: toml_edit::DocumentMut = "UID = 1_000 # comment\n".parse().unwrap();
    walk.define_toml("UID", scalar["UID"].as_value().unwrap())
        .unwrap();
    assert_eq!(&*walk.variables.value("UID").unwrap(), OsStr::new("1_000"));
    let value = "a".repeat(TOML_VARIABLE_BYTES - 2) + "é";
    walk.define_toml("LONG", &toml_edit::Value::from(value))
        .unwrap();
    let stored = walk.variables.value("LONG").unwrap();
    assert_eq!(stored.len(), TOML_VARIABLE_BYTES - 1);
    assert_eq!(stored.as_bytes().last(), Some(&0xc3));
    let long = "/".to_owned() + &"a".repeat(PATH_BYTES);
    assert!(validate_generated_path(Path::new(&long)).is_err());
    assert_eq!(
        walk.resolve(&root, OsStr::new(&long))
            .unwrap()
            .as_os_str()
            .len(),
        PATH_BYTES - 1
    );
}

#[test]
fn cycles_and_include_budgets_fail_closed() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("config.conf");
    let child = directory.path().join("child.conf");
    let goal = directory.path().join("generated.conf");
    std::fs::write(&root, "source=./child.conf\n").unwrap();
    std::fs::write(&child, "source=./config.conf\n").unwrap();
    assert!(connected(&root, &goal).is_err());
    std::fs::write(&root, "#".repeat(crate::keybinds::MAX_FILE_BYTES + 1)).unwrap();
    assert!(connected(&root, &goal).is_err());
    let mut text = String::new();
    for index in 0..=crate::keybinds::MAX_FILES {
        text.push_str(&format!("source_optional=./missing-{index}.conf\n"));
    }
    std::fs::write(&root, text).unwrap();
    assert!(connected(&root, &goal).is_err());
}

#[test]
fn relocation_checks_nested_optional_targets_and_preserves_absolute_targets() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("system/config.conf");
    let destination = directory.path().join("user/config.conf");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    let child = directory.path().join("child.conf");
    let text = format!("source={}\n", child.display());
    std::fs::write(&source, &text).unwrap();
    std::fs::write(&child, "source_optional=./missing.conf\n").unwrap();
    assert!(preserve_targets(&source, &destination, &text).is_err());
    std::fs::write(&child, "border_radius=6\n").unwrap();
    preserve_targets(&source, &destination, &text).unwrap();
    // Same-file aliases preserve the include target even across different directories.
    let real = directory.path().join("real.conf");
    std::fs::write(&real, "border_radius=9\n").unwrap();
    for parent in [source.parent().unwrap(), destination.parent().unwrap()] {
        std::os::unix::fs::symlink(&real, parent.join("shared.conf")).unwrap();
    }
    preserve_targets(&source, &destination, "source_optional=./shared.conf\n").unwrap();
}
