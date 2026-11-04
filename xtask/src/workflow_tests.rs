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
