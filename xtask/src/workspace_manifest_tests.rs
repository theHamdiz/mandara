//! Catch unused inherited package fields before newer Cargo warns about them.

use std::{collections::BTreeSet, path::Path};

fn unused<'a>(workspace: &'a toml::Value, members: &[toml::Value]) -> BTreeSet<&'a str> {
    workspace["workspace"]["package"]
        .as_table()
        .unwrap()
        .keys()
        .filter(|key| {
            !members.iter().any(|member| {
                member
                    .get("package")
                    .and_then(|package| package.get(*key))
                    .and_then(|value| value.get("workspace"))
                    .and_then(toml::Value::as_bool)
                    == Some(true)
            })
        })
        .map(String::as_str)
        .collect()
}

#[test]
fn workspace_package_fields_are_inherited_and_versions_stay_with_their_components() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut workspace: toml::Value =
        toml::from_str(&std::fs::read_to_string(root.join("Cargo.toml")).unwrap()).unwrap();
    let members: Vec<toml::Value> = workspace["workspace"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|member| {
            toml::from_str(
                &std::fs::read_to_string(root.join(member.as_str().unwrap()).join("Cargo.toml"))
                    .unwrap(),
            )
            .unwrap()
        })
        .collect();
    assert!(unused(&workspace, &members).is_empty());
    assert!(
        members
            .iter()
            .all(|member| member["package"]["version"].as_str().is_some())
    );
    workspace["workspace"]["package"]
        .as_table_mut()
        .unwrap()
        .insert("version".into(), "1.0.0".into());
    assert_eq!(unused(&workspace, &members), BTreeSet::from(["version"]));
}
