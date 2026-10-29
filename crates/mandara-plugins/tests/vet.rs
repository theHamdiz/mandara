//! The compliance check on real components: the fixture (one component
//! whose `mode` setting picks its behaviour, see tests/data/vet/README.md)
//! for each class, and the moon-phase example when it is built.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mandara_plugins::vet::{verdict_json, vet};
use mandara_services::marketplace::Class;

/// Built-in ids a community plugin may not pose as (a sample).
const RESERVED: &[&str] = &["weather", "clock"];
/// What "well under a second" means for one check (a debug build compiles
/// the component without a cache).
const MAX_VET: Duration = Duration::from_secs(5);

fn data() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/vet")
}

/// A plugin folder with the fixture component and `manifest`.
fn folder(name: &str, manifest: &str, wasm: &Path) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mandara-vet-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("plugin.toml"), manifest).unwrap();
    std::fs::copy(wasm, dir.join("plugin.wasm")).unwrap();
    dir
}

fn fixture(mode: &str) -> PathBuf {
    let manifest = format!("id = \"fixture-{mode}\"\nname = \"Fixture {mode}\"\ndescription = \"A test plugin\"\n\n[[settings]]\nkey = \"mode\"\ntype = \"text\"\ndefault = \"{mode}\"\n");
    folder(mode, &manifest, &data().join("fixture.wasm"))
}

fn class(dir: &Path) -> Option<Class> {
    let started = Instant::now();
    let verdict = vet(dir, RESERVED, None);
    assert!(started.elapsed() < MAX_VET, "vet took {:?}", started.elapsed());
    let _ = std::fs::remove_dir_all(dir);
    verdict.err().map(|f| {
        assert!(!f.reason.is_empty());
        f.class
    })
}

#[test]
fn each_class_is_caught() {
    // SAFETY: set before anything reads it; this binary's only test that
    // touches the environment. The compile cache goes to a scratch folder.
    let cache = std::env::temp_dir().join(format!("mandara-vet-cache-{}", std::process::id()));
    unsafe { std::env::set_var("XDG_CACHE_HOME", &cache) };
    assert_eq!(class(&fixture("ok")), None);
    assert_eq!(class(&fixture("empty")), Some(Class::Placeholder));
    assert_eq!(class(&fixture("trap")), Some(Class::Broken));
    assert_eq!(class(&fixture("spin")), Some(Class::Broken), "a render past its time budget");
    assert_eq!(class(&fixture("eager")), Some(Class::Malicious), "a command queued without a user event");

    let wasm = data().join("fixture.wasm");
    // Manifest problems, a missing component and a core module are broken.
    assert_eq!(class(&folder("bad-id", "id = \"Bad Id\"\nname = \"x\"\n", &wasm)), Some(Class::Broken));
    let missing = folder("missing", "id = \"missing\"\nname = \"Missing\"\n", &wasm);
    std::fs::remove_file(missing.join("plugin.wasm")).unwrap();
    assert_eq!(class(&missing), Some(Class::Broken));
    let core = folder("core", "id = \"core-module\"\nname = \"Core\"\n", &wasm);
    std::fs::write(core.join("plugin.wasm"), b"\0asm\x01\0\0\0").unwrap();
    assert_eq!(class(&core), Some(Class::Broken));
    // Posing as a built-in, the template and a republished example.
    assert_eq!(class(&folder("pose", "id = \"weather\"\nname = \"Weather\"\n", &wasm)), Some(Class::Malicious));
    assert_eq!(class(&folder("template", "id = \"my-plugin\"\nname = \"My plugin\"\n", &wasm)), Some(Class::Placeholder));
    let example = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/examples/moon-phase/plugin.toml")).unwrap();
    assert_eq!(class(&folder("copy", &example, &wasm)), Some(Class::Placeholder));

    let json = verdict_json(&Err(mandara_services::marketplace::Finding::new(Class::Broken, "x")));
    assert_eq!((json["ok"].as_bool(), json["class"].as_str()), (Some(false), Some("broken")));
    assert_eq!(verdict_json(&Ok(())), serde_json::json!({ "ok": true }));

    // The real example passes once it's an author's own plugin.
    let built = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/examples/moon-phase/target/wasm32-wasip2/release/moon_phase.wasm");
    if built.is_file() {
        let own = example.replace("id = \"moon-phase\"", "id = \"lunar\"").replace("name = \"Moon phase\"", "name = \"Lunar\"");
        assert_eq!(class(&folder("moon", &own, &built)), None);
    } else {
        eprintln!("skipped the moon-phase check: {} not built", built.display());
    }
    let _ = std::fs::remove_dir_all(cache);
}
