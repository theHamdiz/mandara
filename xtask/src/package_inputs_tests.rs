//! Distribution manifests consume the same versioned packs without publishing staging files.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use mandara_i18n::{Lang, pack::Pack};
use serde_yaml_ng::Value as Yaml;

/// Locate actual packaging commands and the following distribution-artifact upload.
fn package_steps(workflow: &Yaml) -> Result<(&str, &Yaml), String> {
    for job in workflow["jobs"].as_mapping().ok_or("missing jobs")?.values() {
        let steps = job["steps"].as_sequence().ok_or("missing steps")?;
        for (index, step) in steps.iter().enumerate() {
            let Some(run) = step["run"].as_str() else { continue };
            let commands: Vec<_> = run.lines().map(str::trim).collect();
            if !commands.iter().any(|line| line.starts_with("cargo deb ")) || !commands.iter().any(|line| line.starts_with("cargo generate-rpm ")) {
                continue;
            }
            let upload = steps[index + 1..].iter().find(|step| {
                step["uses"].as_str().is_some_and(|value| value.starts_with("actions/upload-artifact@"))
                    && step["with"]["name"].as_str().is_some_and(|value| value.starts_with("dist-"))
            }).ok_or("packages have no subsequent artifact upload")?;
            return Ok((run, upload));
        }
    }
    Err("no distribution packaging step".into())
}

/// Resolve the producer's version variable without executing workflow shell commands.
fn stage_directory(workflow: &Yaml, version: &str) -> Result<PathBuf, String> {
    let (run, _) = package_steps(workflow)?;
    let commands: Vec<_> = run.lines().map(str::trim).collect();
    let producer = commands.iter().position(|line| line.starts_with("cargo ") && line.contains("-- langpacks ")).ok_or("no pack producer")?;
    let first_package = commands.iter().position(|line| line.starts_with("cargo deb ") || line.starts_with("cargo generate-rpm ")).ok_or("no packager")?;
    if producer >= first_package { return Err("language packs must exist before packaging".into()); }
    let args: Vec<_> = commands[producer].split_whitespace().map(|word| word.trim_matches('"').to_string()).collect();
    let variable = crate::opt(&args, "--version").ok_or("pack producer has no version")?;
    let directory = crate::opt(&args, "--dir").ok_or("pack producer has no directory")?;
    let directory = PathBuf::from(directory.replace(&variable, version));
    if directory.components().any(|part| !matches!(part, Component::Normal(_))) { return Err("staging must be workspace relative".into()); }
    if !directory.starts_with("dist") || !directory.components().any(|part| part.as_os_str().to_string_lossy().starts_with('.')) {
        return Err("package inputs need a hidden distribution staging directory".into());
    }
    if directory.file_name().is_none_or(|name| name != version) { return Err("staging must retain the binary version".into()); }
    Ok(directory)
}

/// Expand the manifests' two-level version/pack glob against real fixture files.
fn resolved_packs(workspace: &Path, source: &str, destination: &str) -> Result<BTreeMap<PathBuf, Vec<u8>>, String> {
    let (prefix, remainder) = source.split_once('*').ok_or("language source has no version glob")?;
    if remainder != "/*.mlang" { return Err("language source must preserve version and pack components".into()); }
    let base = workspace.join("crates/mandara-app").join(prefix).canonicalize().map_err(|error| error.to_string())?;
    let dist = workspace.join("dist").canonicalize().map_err(|error| error.to_string())?;
    if !base.starts_with(dist) { return Err("package inputs must be separate from Cargo build products".into()); }
    let destination = Path::new(destination.trim_start_matches('/'));
    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(&base).map_err(|error| error.to_string())? {
        let version = entry.map_err(|error| error.to_string())?.path();
        if !version.is_dir() { continue; }
        for entry in std::fs::read_dir(version).map_err(|error| error.to_string())? {
            let pack = entry.map_err(|error| error.to_string())?.path();
            if !pack.is_file() || pack.extension().is_none_or(|extension| extension != "mlang") { continue; }
            let relative = pack.strip_prefix(&base).map_err(|error| error.to_string())?;
            files.insert(destination.join(relative), std::fs::read(&pack).map_err(|error| error.to_string())?);
        }
    }
    Ok(files)
}

/// Check both backend payload paths, pack validity and the effective hidden-upload policy.
fn package_contract(workspace: &Path, manifest: &toml::Value, workflow: &Yaml, version: &str) -> Result<(), String> {
    let stage = stage_directory(workflow, version)?;
    let (_, upload) = package_steps(workflow)?;
    let hidden = &upload["with"]["include-hidden-files"];
    if !(hidden.is_null() || hidden.as_bool() == Some(false) || hidden.as_str() == Some("false")) {
        return Err("package staging must stay outside the uploaded artifact".into());
    }
    let metadata = &manifest["package"]["metadata"];
    let deb = metadata["deb"]["assets"].as_array().ok_or("missing deb assets")?.iter().find(|asset| asset.get(0).and_then(toml::Value::as_str).is_some_and(|source| source.ends_with(".mlang"))).ok_or("missing deb languages")?;
    let rpm = metadata["generate-rpm"]["assets"].as_array().ok_or("missing rpm assets")?.iter().find(|asset| asset["source"].as_str().is_some_and(|source| source.ends_with(".mlang"))).ok_or("missing rpm languages")?;
    let deb = resolved_packs(workspace, deb[0].as_str().ok_or("invalid deb source")?, deb[1].as_str().ok_or("invalid deb destination")?)?;
    let rpm = resolved_packs(workspace, rpm["source"].as_str().ok_or("invalid rpm source")?, rpm["dest"].as_str().ok_or("invalid rpm destination")?)?;
    if deb != rpm { return Err("package backends disagree on language payloads".into()); }
    let codes: Vec<_> = Lang::ALL.into_iter().filter(|language| *language != Lang::En).collect();
    if deb.len() != codes.len() { return Err("a package is missing language packs".into()); }
    for language in codes {
        let file = format!("{}.mlang", language.code());
        let destination = Path::new("usr/share/mandara/locales").join(version).join(&file);
        let bytes = deb.get(&destination).ok_or("pack destination lost its version")?;
        let produced = std::fs::read(workspace.join(&stage).join(file)).map_err(|error| error.to_string())?;
        if *bytes != produced { return Err("package input differs from the generated pack".into()); }
        Pack::parse(bytes, version, language).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// The real producer and both manifests agree; old, flattened or exposed inputs fail.
#[test]
fn distribution_language_inputs_keep_versioned_payloads_out_of_uploads() {
    let root = crate::root();
    let manifest: toml::Value = toml::from_str(&std::fs::read_to_string(root.join("crates/mandara-app/Cargo.toml")).unwrap()).unwrap();
    let source = std::fs::read_to_string(root.join(".github/workflows/release.yml")).unwrap();
    let workflow: Yaml = serde_yaml_ng::from_str(&source).unwrap();
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("crates/mandara-app")).unwrap();
    let version = "1.2.3";
    let stage = stage_directory(&workflow, version).unwrap();
    crate::langpacks::write_dir(&temp.path().join(&stage), version).unwrap();
    package_contract(temp.path(), &manifest, &workflow, version).unwrap();

    let old_stage = temp.path().join("target/langpacks").join(version);
    std::fs::create_dir_all(&old_stage).unwrap();
    for entry in std::fs::read_dir(temp.path().join(&stage)).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), old_stage.join(entry.file_name())).unwrap();
    }
    let mut old_manifest = manifest.clone();
    for asset in old_manifest["package"]["metadata"]["deb"]["assets"].as_array_mut().unwrap() {
        if asset[0].as_str().is_some_and(|source| source.ends_with(".mlang")) {
            asset[0] = toml::Value::String("../../target/langpacks/*/*.mlang".into());
        }
    }
    for asset in old_manifest["package"]["metadata"]["generate-rpm"]["assets"].as_array_mut().unwrap() {
        if asset["source"].as_str().is_some_and(|source| source.ends_with(".mlang")) {
            asset["source"] = toml::Value::String("../../target/langpacks/*/*.mlang".into());
        }
    }
    assert!(package_contract(temp.path(), &old_manifest, &workflow, version).is_err());
    let mut exposed = workflow.clone();
    let mut late = workflow.clone();
    for job in exposed["jobs"].as_mapping_mut().unwrap().values_mut() {
        for step in job["steps"].as_sequence_mut().unwrap() {
            if step["uses"].as_str().is_some_and(|value| value.starts_with("actions/upload-artifact@")) {
                step["with"]["include-hidden-files"] = Yaml::Bool(true);
            }
        }
    }
    assert!(package_contract(temp.path(), &manifest, &exposed, version).is_err());
    for job in late["jobs"].as_mapping_mut().unwrap().values_mut() {
        for step in job["steps"].as_sequence_mut().unwrap() {
            if let Some(run) = step["run"].as_str().filter(|run| run.lines().any(|line| line.trim_start().starts_with("cargo deb "))) {
                step["run"] = Yaml::String(format!("cargo deb -p mandara-app --no-build\n{run}"));
            }
        }
    }
    assert!(package_contract(temp.path(), &manifest, &late, version).is_err());
    let misplaced = temp.path().join(&stage).join("ar.mlang");
    let flat = temp.path().join(stage.parent().unwrap()).join("ar.mlang");
    std::fs::rename(&misplaced, &flat).unwrap();
    assert!(package_contract(temp.path(), &manifest, &workflow, version).is_err());
    std::fs::rename(flat, misplaced).unwrap();
    package_contract(temp.path(), &manifest, &workflow, version).unwrap();
}
