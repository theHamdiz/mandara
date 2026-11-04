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

#[test]
fn release_build_commands_do_not_inherit_write_credentials() {
    let workflow = std::fs::read_to_string(crate::root().join(".github/workflows/release.yml")).unwrap();
    for checkout in workflow.split("uses: actions/checkout@").skip(1) {
        let with = checkout.split("\n      - ").next().unwrap();
        assert!(with.contains("persist-credentials: false"));
    }
    let result = workflow.split("      - name: Result\n").nth(1).unwrap().split("\n  build:").next().unwrap();
    assert!(!result.contains("        env:"));
    for line in result.lines().filter(|line| line.contains("cargo ")) {
        assert!(!line.contains("GH_TOKEN"));
    }
    for line in workflow.lines().filter(|line| !line.trim_start().starts_with('#') && line.contains(" push ")) {
        assert!(line.contains("credential.helper=!gh auth git-credential"), "push must use command-scoped credentials: {line}");
    }
}
