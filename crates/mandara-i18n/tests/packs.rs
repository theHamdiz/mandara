//! Language packs end to end, in a process of their own (it points the data
//! dirs at a scratch folder and turns off the debug build's source
//! fallback, so it sees exactly what a release build sees):
//!
//! - a language without a pack falls back to English and asks for it once;
//! - a pack installed for the running version is mapped, swapped in by
//!   `reload` and announced to listeners;
//! - packs for another version, damaged packs and packs of another language
//!   are ignored;
//! - lookups are at least as fast as the old parsed `HashMap` catalog
//!   (`lookup_speed`, run with `--release -- --ignored --nocapture`).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mandara_i18n::{Catalog, Lang, packs, t_in, tr_in, untranslate, with_language};

fn source(lang: Lang) -> String {
    std::fs::read_to_string(packs::source_catalog(lang)).unwrap()
}

#[test]
fn packs_are_found_validated_and_swapped_in() {
    let home = tempfile::tempdir().unwrap();
    // SAFETY: this is the only test here that reads the environment (the
    // other one is a timing loop over bytes in memory), so no other thread
    // reads it while it changes.
    unsafe {
        std::env::set_var("XDG_DATA_HOME", home.path());
        std::env::set_var("XDG_DATA_DIRS", home.path().join("system"));
    }
    packs::disable_source_fallback();
    let version = packs::running_version();
    let missing: Arc<Mutex<Vec<Lang>>> = Arc::default();
    let seen = missing.clone();
    packs::on_missing(move |lang| seen.lock().unwrap().push(lang));
    let loaded = Arc::new(AtomicUsize::new(0));
    let count = loaded.clone();
    packs::on_loaded(move |_| {
        count.fetch_add(1, Ordering::SeqCst);
    });

    // No pack: English, and the hook runs once however often it's asked.
    assert_eq!(t_in(Lang::Ar, "Settings"), "Settings");
    assert_eq!(tr_in(Lang::Ar, "Settings"), "Settings");
    assert!(!packs::is_available(Lang::Ar));
    assert_eq!(*missing.lock().unwrap(), vec![Lang::Ar]);
    assert!(!packs::reload(Lang::Ar), "nothing to load yet");
    assert_eq!(loaded.load(Ordering::SeqCst), 0);

    // A pack for another version, a damaged one and another language's are
    // all ignored.
    let dir = packs::user_dir(version);
    let other = packs::user_dir("0.0.1");
    let ar = mandara_i18n::compile(Lang::Ar, version, &source(Lang::Ar)).unwrap();
    packs::install(Lang::Ar, "0.0.1", &mandara_i18n::compile(Lang::Ar, "0.0.1", &source(Lang::Ar)).unwrap(), &other).unwrap();
    assert!(packs::install(Lang::Ar, version, &ar[..ar.len() - 1], &dir).is_err(), "install refuses a truncated pack");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("ar.mlang"), &ar[..ar.len() / 2]).unwrap();
    assert!(!packs::reload(Lang::Ar), "a truncated pack is ignored");
    std::fs::write(dir.join("ar.mlang"), mandara_i18n::compile(Lang::Fa, version, &source(Lang::Fa)).unwrap()).unwrap();
    assert!(!packs::reload(Lang::Ar), "Persian under Arabic's name is ignored");
    assert_eq!(t_in(Lang::Ar, "Settings"), "Settings");

    // The right pack: mapped, swapped in, announced.
    let path = packs::install(Lang::Ar, version, &ar, &dir).unwrap();
    assert!(packs::reload(Lang::Ar));
    assert_eq!(loaded.load(Ordering::SeqCst), 1);
    assert_eq!(packs::loaded_from(Lang::Ar), Some(path.clone()));
    let before = t_in(Lang::Ar, "Settings");
    assert_eq!(before, "الإعدادات");
    // Several keys may share a translation; any of them translates back.
    with_language(Some(Lang::Ar), || assert_eq!(tr_in(Lang::Ar, &untranslate("الإعدادات")), "الإعدادات"));

    // Replacing the file under a live mapping is a rename: strings handed
    // out before stay readable, and a reload serves the new file.
    packs::install(Lang::Ar, version, &ar, &dir).unwrap();
    assert!(packs::reload(Lang::Ar));
    assert_eq!(before, "الإعدادات");

    // Pruning keeps the versions asked for.
    let removed = packs::prune(&[version]);
    assert_eq!(removed, vec![other]);
    assert!(path.exists());

    // A system folder is searched too.
    let system = home.path().join("system").join(packs::DIR).join(version);
    packs::install(Lang::De, version, &mandara_i18n::compile(Lang::De, version, &source(Lang::De)).unwrap(), &system).unwrap();
    assert_eq!(t_in(Lang::De, "Settings"), "Einstellungen");
    assert_eq!(*missing.lock().unwrap(), vec![Lang::Ar], "German was found, not missing");
}

/// `t()` on a pack against the old parsed catalog (a `HashMap<Box<str>,
/// Box<str>>`), over every Arabic key in a shuffled order, best of several
/// rounds. Release numbers are what count:
/// `cargo test --release -p mandara-i18n --test packs -- --ignored --nocapture`.
#[test]
#[ignore = "a timing loop; run in release with --ignored --nocapture"]
fn lookup_speed() {
    const ROUNDS: usize = 20;
    const HOT_KEYS: usize = 200;
    const HOT_REPEAT: usize = 50;
    let text = source(Lang::Ar);
    let catalog = Catalog::parse(&text).unwrap();
    let bytes = mandara_i18n::pack::build(Lang::Ar, "bench", catalog.iter()).unwrap();
    let pack = mandara_i18n::pack::Pack::parse(&bytes, "bench", Lang::Ar).unwrap();
    // Keys of their own, as the code's string literals are: looking a map's
    // own key up would compare it with itself, already in cache.
    let owned: Vec<String> = catalog.keys().map(str::to_string).collect();
    let mut keys: Vec<&str> = owned.iter().map(String::as_str).collect();
    // A fixed shuffle (xorshift), so neither side benefits from order.
    let mut s = 0x9e37_79b9_7f4a_7c15u64;
    for i in (1..keys.len()).rev() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        keys.swap(i, (s % (i as u64 + 1)) as usize);
    }
    let time = |f: &dyn Fn(&str) -> Option<usize>| {
        (0..ROUNDS)
            .map(|_| {
                let start = std::time::Instant::now();
                let mut sum = 0usize;
                for k in &keys {
                    sum = sum.wrapping_add(f(std::hint::black_box(k)).unwrap_or(0));
                }
                std::hint::black_box(sum);
                start.elapsed().as_nanos() as f64 / keys.len() as f64
            })
            .fold(f64::INFINITY, f64::min)
    };
    let map_ns = time(&|k| catalog.get(k).map(str::len));
    let pack_ns = time(&|k| pack.get(k).map(str::len));
    // A view's worth of strings looked up again and again (a render loop).
    let hot: Vec<&str> = keys.iter().copied().take(HOT_KEYS).collect();
    let time_hot = |f: &dyn Fn(&str) -> Option<usize>| {
        (0..ROUNDS)
            .map(|_| {
                let start = std::time::Instant::now();
                let mut sum = 0usize;
                for _ in 0..HOT_REPEAT {
                    for k in &hot {
                        sum = sum.wrapping_add(f(std::hint::black_box(k)).unwrap_or(0));
                    }
                }
                std::hint::black_box(sum);
                start.elapsed().as_nanos() as f64 / (hot.len() * HOT_REPEAT) as f64
            })
            .fold(f64::INFINITY, f64::min)
    };
    println!("hot ({HOT_KEYS} keys): HashMap {:.1} ns, pack {:.1} ns", time_hot(&|k| catalog.get(k).map(str::len)), time_hot(&|k| pack.get(k).map(str::len)));
    let hash_ns = time(&|k| Some(mandara_i18n::pack::hash(k) as usize));
    let sip = std::collections::hash_map::RandomState::new();
    let sip_ns = time(&|k| Some(std::hash::BuildHasher::hash_one(&sip, k) as usize));
    println!("hash only: pack {hash_ns:.1} ns, HashMap's SipHash {sip_ns:.1} ns");
    let parse = std::time::Instant::now();
    let _ = std::hint::black_box(Catalog::parse(&text).unwrap());
    let parse_ms = parse.elapsed().as_secs_f64() * 1e3;
    let check = std::time::Instant::now();
    let _ = std::hint::black_box(mandara_i18n::pack::Pack::parse(&bytes, "bench", Lang::Ar).unwrap());
    let check_ms = check.elapsed().as_secs_f64() * 1e3;
    println!("{} keys: HashMap {map_ns:.1} ns/lookup, pack {pack_ns:.1} ns/lookup; load: TOML parse {parse_ms:.2} ms, pack validation {check_ms:.3} ms ({} bytes)", keys.len(), bytes.len());
}
