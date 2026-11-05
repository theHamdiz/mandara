//! Immutable checkouts protect the sources used for tests and release artifacts.

#[test]
fn checkout_steps_share_one_immutable_release() {
    let mut pins = Vec::new();
    for file in ["ci.yml", "release.yml"] {
        let workflow = std::fs::read_to_string(crate::root().join(".github/workflows").join(file)).unwrap();
        for line in workflow.lines() {
            if let Some((_, version)) = line.split_once("uses: actions/checkout@") {
                let pin = version.split_whitespace().next().unwrap();
                assert_eq!(pin.len(), 40, "{file}: checkout must use a full commit SHA");
                assert!(pin.bytes().all(|b| b.is_ascii_hexdigit()));
                pins.push(pin.to_string());
            }
        }
    }
    assert!(!pins.is_empty());
    assert!(pins.iter().all(|pin| pin == &pins[0]));
}

fn no_token(environment: &serde_yaml_ng::Value) -> bool {
    ["GH_TOKEN", "GITHUB_TOKEN"].iter().all(|key| environment.get(*key).is_none())
}

fn credentials_are_scoped(workflow: &serde_yaml_ng::Value) -> Result<(), String> {
    if !no_token(&workflow["env"]) { return Err("workflow environment exposes a git token".into()); }
    let jobs = workflow["jobs"].as_mapping().ok_or("workflow has no jobs")?;
    let version = &workflow["jobs"]["version"];
    if !no_token(&version["env"]) { return Err("version environment exposes a git token".into()); }
    for job in jobs.values() {
        let steps = job["steps"].as_sequence().ok_or("job has no steps")?;
        for step in steps {
            if step["uses"].as_str().is_some_and(|name| name.starts_with("actions/checkout@")) {
                let input = &step["with"]["persist-credentials"];
                if input.as_bool() != Some(false) && input.as_str() != Some("false") {
                    return Err("checkout must explicitly disable credential persistence".into());
                }
            }
            let run = step["run"].as_str().unwrap_or_default();
            if job == version && run.contains("cargo ") && !no_token(&step["env"]) {
                return Err("version build command exposes a git token".into());
            }
            if run.contains(" push ") {
                if !run.contains("credential.helper=!gh auth git-credential") { return Err("push requires command-scoped credentials".into()); }
                if run.contains("${{ github.ref_name }}") { return Err("branch names must enter the shell through an environment variable".into()); }
            }
        }
    }
    Ok(())
}

#[test]
fn release_build_commands_do_not_inherit_write_credentials() {
    let workflow = std::fs::read_to_string(crate::root().join(".github/workflows/release.yml")).unwrap();
    let workflow: serde_yaml_ng::Value = serde_yaml_ng::from_str(&workflow).unwrap();
    credentials_are_scoped(&workflow).unwrap();
}

#[test]
fn workflow_credentials_checks_reject_comments_and_inherited_tokens() {
    let source = r#"
jobs:
  version:
    steps:
      - uses: actions/checkout@fixture
        with:
          persist-credentials: false
      - run: cargo xtask bump --commit --tag
      - env:
          GH_TOKEN: fixture-token
          REF_NAME: fixture-branch
        run: git -c 'credential.helper=!gh auth git-credential' push origin "HEAD:${REF_NAME}"
"#;
    let parse = |source: &str| serde_yaml_ng::from_str(source).unwrap();
    assert!(credentials_are_scoped(&parse(source)).is_ok());
    for input in ["true # persist-credentials: false", "'true'", "null", "'${{ inputs.persist }}'"] {
        assert!(credentials_are_scoped(&parse(&source.replace("persist-credentials: false", &format!("persist-credentials: {input}")))).is_err(), "{input}");
    }
    for source in [format!("env:\n  GH_TOKEN: fixture-token\n{source}"), source.replace("  version:\n", "  version:\n    env:\n      GH_TOKEN: fixture-token\n"), source.replace("      - run: cargo", "      - env:\n          GH_TOKEN: fixture-token\n        run: cargo")] {
        assert!(credentials_are_scoped(&parse(&source)).is_err());
    }
}
