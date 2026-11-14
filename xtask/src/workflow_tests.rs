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

/// Build artifacts must come from completed producer jobs and pass the
/// downloader's digest check before the publish job signs their contents.
fn release_artifact_handoff(workflow: &serde_yaml_ng::Value) -> Result<(), String> {
    let jobs = workflow["jobs"].as_mapping().ok_or("workflow has no jobs")?;
    let publish = &workflow["jobs"]["publish"];
    if !publish["if"].is_null() && !matches!(publish["if"].as_str(), Some("success()" | "${{ success() }}")) {
        return Err("publication must require successful artifact producers".into());
    }
    let needs = publish["needs"].as_sequence().ok_or("publish has no producer dependencies")?;
    for (id, job) in jobs {
        let produces = job["steps"].as_sequence().ok_or("job has no steps")?.iter().any(|step| {
            step["uses"].as_str().is_some_and(|name| name.starts_with("actions/upload-artifact@"))
                && step["with"]["name"].as_str().is_some_and(|name| name.starts_with("dist-"))
        });
        if produces && !needs.contains(id) { return Err("publish does not wait for an artifact producer".into()); }
    }
    let downloads: Vec<_> = publish["steps"].as_sequence().ok_or("publish has no steps")?.iter().filter(|step| {
        step["uses"].as_str().is_some_and(|name| name.starts_with("actions/download-artifact@"))
    }).collect();
    if downloads.len() != 1 { return Err("publish requires one distribution download".into()); }
    let download = downloads[0];
    if !download["if"].is_null() {
        return Err("distribution downloads must not be conditional".into());
    }
    if download["uses"].as_str() != Some("actions/download-artifact@9000827ccba6bdab643e8b6fd33ac0654aef8333") {
        return Err("distribution downloads must use the reviewed immutable action".into());
    }
    let input = &download["with"];
    let enabled = |value: &serde_yaml_ng::Value| value.as_bool() == Some(true) || value.as_str() == Some("true");
    if input["pattern"].as_str() != Some("dist-*") || input["path"].as_str() != Some("dist") || !enabled(&input["merge-multiple"]) {
        return Err("publish must merge every distribution artifact into dist".into());
    }
    if !input["digest-mismatch"].is_null() && input["digest-mismatch"].as_str() != Some("error") {
        return Err("artifact digest mismatches must fail publication".into());
    }
    let disabled = |value: &serde_yaml_ng::Value| value.is_null() || value.as_bool() == Some(false) || value.as_str() == Some("false");
    if !disabled(&input["skip-decompress"]) || !disabled(&download["continue-on-error"]) {
        return Err("publication must receive extracted, verified artifacts".into());
    }
    for key in ["name", "artifact-ids", "repository", "run-id", "github-token"] {
        if !input[key].is_null() { return Err("distribution artifacts must come from this workflow run".into()); }
    }
    Ok(())
}

/// The production handoff stays pinned, waits for all architectures and fails
/// closed when its input selection or integrity policy is weakened.
#[test]
fn release_artifact_downloads_keep_producer_and_integrity_contracts() {
    let source = std::fs::read_to_string(crate::root().join(".github/workflows/release.yml")).unwrap();
    let workflow: serde_yaml_ng::Value = serde_yaml_ng::from_str(&source).unwrap();
    release_artifact_handoff(&workflow).unwrap();
    for (key, value) in [
        ("pattern", "other-*"), ("path", "elsewhere"), ("merge-multiple", "false"),
        ("digest-mismatch", "warn"), ("digest-mismatch", "ignore"),
        ("skip-decompress", "true"), ("name", "dist-one-target"), ("run-id", "123"),
    ] {
        let mut invalid = workflow.clone();
        let step = invalid["jobs"]["publish"]["steps"].as_sequence_mut().unwrap().iter_mut().find(|step| {
            step["uses"].as_str().is_some_and(|name| name.starts_with("actions/download-artifact@"))
        }).unwrap();
        step["with"][key] = serde_yaml_ng::Value::String(value.into());
        assert!(release_artifact_handoff(&invalid).is_err(), "{key}={value}");
    }
    let mut old = workflow.clone();
    let step = old["jobs"]["publish"]["steps"].as_sequence_mut().unwrap().iter_mut().find(|step| {
        step["uses"].as_str().is_some_and(|name| name.starts_with("actions/download-artifact@"))
    }).unwrap();
    step["uses"] = serde_yaml_ng::Value::String("actions/download-artifact@v4".into());
    assert!(release_artifact_handoff(&old).is_err());
    let mut early = workflow.clone();
    early["jobs"]["publish"]["needs"].as_sequence_mut().unwrap().retain(|job| job.as_str() != Some("build-musl"));
    assert!(release_artifact_handoff(&early).is_err());

    for (scope, key, value) in [
        ("step", "if", "${{ inputs.rebuild }}"),
        ("with", "skip-decompress", "${{ true }}"),
        ("step", "continue-on-error", "${{ true }}"),
    ] {
        let mut invalid = workflow.clone();
        let download = invalid["jobs"]["publish"]["steps"].as_sequence_mut().unwrap().iter_mut().find(|step| {
            step["uses"].as_str().is_some_and(|name| name.starts_with("actions/download-artifact@"))
        }).unwrap();
        if scope == "with" { download["with"][key] = serde_yaml_ng::Value::String(value.into()); }
        else { download[key] = serde_yaml_ng::Value::String(value.into()); }
        assert!(release_artifact_handoff(&invalid).is_err(), "{key}={value}");
    }
    for condition in ["always()", "${{ always() }}", "failure()", "${{ !cancelled() }}", "needs.build.result == 'success' || needs.build-musl.result == 'skipped'"] {
        let mut invalid = workflow.clone();
        invalid["jobs"]["publish"]["if"] = serde_yaml_ng::Value::String(condition.into());
        assert!(release_artifact_handoff(&invalid).is_err(), "{condition}");
    }
    for condition in ["success()", "${{ success() }}"] {
        let mut safe = workflow.clone();
        safe["jobs"]["publish"]["if"] = serde_yaml_ng::Value::String(condition.into());
        release_artifact_handoff(&safe).unwrap();
    }
    let mut safe = workflow;
    let download = safe["jobs"]["publish"]["steps"].as_sequence_mut().unwrap().iter_mut().find(|step| {
        step["uses"].as_str().is_some_and(|name| name.starts_with("actions/download-artifact@"))
    }).unwrap();
    download["with"]["skip-decompress"] = serde_yaml_ng::Value::Bool(false);
    download["continue-on-error"] = serde_yaml_ng::Value::String("false".into());
    release_artifact_handoff(&safe).unwrap();
}

#[test]
fn nix_runner_memory_is_prepared_before_compilation_without_reducing_release_optimization() {
    let workflow = std::fs::read_to_string(crate::root().join(".github/workflows/ci.yml")).unwrap();
    let workflow: serde_yaml_ng::Value = serde_yaml_ng::from_str(&workflow).unwrap();
    let steps = workflow["jobs"]["nix"]["steps"].as_sequence().unwrap();
    let memory = steps.iter().position(|step| step["run"].as_str().is_some_and(|run| run.contains("sudo swapon /swapfile-mandara-nix"))).unwrap();
    let setup = steps[memory]["run"].as_str().unwrap();
    for operation in ["fallocate -l 8G", "chmod 600", "mkswap", "swapon"] { assert!(setup.contains(operation)); }
    for (index, step) in steps.iter().enumerate() {
        let run = step["run"].as_str().unwrap_or_default();
        if run.contains("nix build") || run.contains("nix flake check") {
            assert!(memory < index, "swap must exist before the evaluator starts builders");
            assert!(run.contains("--max-jobs 1 --cores 2"), "bound both derivations and compiler jobs");
        }
    }
    let manifest: toml::Value = toml::from_str(&std::fs::read_to_string(crate::root().join("Cargo.toml")).unwrap()).unwrap();
    assert_eq!(manifest["profile"]["release"]["lto"].as_str(), Some("fat"));
    assert_eq!(manifest["profile"]["release"]["codegen-units"].as_integer(), Some(1));
}

#[test]
fn nix_builds_cover_stable_unstable_and_the_previous_reported_release() {
    let workflow = std::fs::read_to_string(crate::root().join(".github/workflows/ci.yml")).unwrap();
    let workflow: serde_yaml_ng::Value = serde_yaml_ng::from_str(&workflow).unwrap();
    let job = &workflow["jobs"]["nix"];
    let channels = job["strategy"]["matrix"]["include"].as_sequence().unwrap();
    for channel in ["nixos-26.05", "nixos-unstable", "nixos-25.11"] {
        assert!(
            channels
                .iter()
                .any(|value| value["channel"].as_str() == Some(channel))
        );
    }
    for step in job["steps"].as_sequence().unwrap() {
        if let Some(run) = step["run"].as_str()
            && (run.contains("nix build") || run.contains("nix flake check"))
        {
            assert!(run.contains("--override-input nixpkgs https://channels.nixos.org/${{ matrix.channel }}/nixexprs.${{ matrix.archive }}"));
            assert!(run.contains("--no-write-lock-file"));
        }
    }
    let flake = std::fs::read_to_string(crate::root().join("flake.nix")).unwrap();
    assert!(flake.contains(
        "inputs.nixpkgs.url = \"https://channels.nixos.org/nixos-26.05/nixexprs.tar.zst\""
    ));
}
