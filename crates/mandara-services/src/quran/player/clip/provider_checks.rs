//! Explicit network checks of the small first chapter, without a sound output.

use std::path::{Path, PathBuf};

use crate::quran::audio::{
    AudioSource, AyahIndex, FetchCache, Interval, Observer, Prepared, Recitations, Request,
};

use super::Clip;

const MARKER: &str = "mandara-quran-provider-check-v1\n";

fn private_cache(
    opted_in: bool,
    root: &Path,
    home: &Path,
    cache: &Path,
) -> Result<PathBuf, String> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    if !opted_in {
        return Err("set MANDARA_QURAN_PROVIDER_NETWORK=1 to allow provider downloads".into());
    }
    let metadata = std::fs::symlink_metadata(root).map_err(|e| e.to_string())?;
    // SAFETY: geteuid has no pointer arguments or side effects.
    let uid = unsafe { libc::geteuid() };
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != uid
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err("the provider fixture root must be a private owned directory".into());
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let parent = root.parent().ok_or("missing fixture parent")?;
    if ![Path::new("/tmp"), Path::new("/var/tmp")].contains(&parent)
        || !root.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .starts_with("mandara-quran-provider-")
        })
        || std::fs::read_to_string(root.join(".mandara-quran-provider-check"))
            .ok()
            .as_deref()
            != Some(MARKER)
        || home.canonicalize().ok().as_ref() != Some(&root.join("home"))
        || cache.canonicalize().ok().as_ref() != Some(&root.join("cache"))
    {
        return Err(
            "the provider fixture needs its marker, private HOME and private XDG cache".into(),
        );
    }
    Ok(root.join("cache/mandara/quran"))
}

struct Live;
impl Observer for Live {
    fn current(&self) -> bool {
        true
    }
    fn progress(&self, _: u64, _: u64) {}
}

fn consume(clip: &mut Clip) -> (u64, f32) {
    let mut count = 0;
    let mut peak = 0f32;
    while let Some(frame) = clip.next().expect("provider MP3 decodes") {
        assert!(frame.iter().all(|sample| sample.is_finite()));
        peak = frame
            .iter()
            .fold(peak, |peak, sample| peak.max(sample.abs()));
        count += 1;
    }
    assert!(
        count > 0 && peak > 0.0,
        "the provider returned no audible samples"
    );
    (count, peak)
}

/// A fresh ranged decoder must match the same samples decoded sequentially.
fn check_range(prepared: &Prepared) {
    let interval = prepared.interval.expect("a timed chapter source");
    let sequential = Prepared {
        path: prepared.path.clone(),
        interval: Some(Interval {
            start_time: 0,
            end_time: interval.end_time,
        }),
    };
    let mut reference = Clip::open(&sequential, 0, &Live).unwrap();
    let mut ranged = Clip::open(prepared, 0, &Live).unwrap();
    assert_eq!(ranged.rate, reference.rate);
    let start = interval.start_time * u64::from(reference.rate) / 1000;
    for _ in 0..start {
        assert!(reference.next().unwrap().is_some());
    }
    let mut count = 0;
    while let Some(actual) = ranged.next().unwrap() {
        let expected = reference
            .next()
            .unwrap()
            .expect("the sequential recording contains this ayah");
        assert!(
            actual
                .iter()
                .zip(expected)
                .all(|(actual, expected)| (actual - expected).abs() < 0.0001),
            "seek differs at sample {count}: {actual:?} versus {expected:?}"
        );
        count += 1;
    }
    let end = interval.end_time * u64::from(reference.rate) / 1000;
    assert_eq!(count, end - start);
    assert!(reference.next().unwrap().is_none());
}

#[test]
fn provider_network_guard_requires_opt_in_and_a_private_cache() {
    use std::os::unix::fs::PermissionsExt as _;
    let root = tempfile::Builder::new()
        .prefix("mandara-quran-provider-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let home = root.path().join("home");
    let cache = root.path().join("cache");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&cache).unwrap();
    std::fs::write(root.path().join(".mandara-quran-provider-check"), MARKER).unwrap();
    assert!(private_cache(false, root.path(), &home, &cache).is_err());
    assert!(private_cache(true, root.path(), &home, &home).is_err());
    assert_eq!(
        private_cache(true, root.path(), &home, &cache).unwrap(),
        root.path()
            .canonicalize()
            .unwrap()
            .join("cache/mandara/quran")
    );
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(private_cache(true, root.path(), &home, &cache).is_err());
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::remove_file(root.path().join(".mandara-quran-provider-check")).unwrap();
    assert!(private_cache(true, root.path(), &home, &cache).is_err());
}

/// Uses the real source adapter and decoder; it never creates AudioOutput or a player.
#[test]
#[ignore = "downloads only chapter 1 from three providers; requires an explicit marked private fixture"]
fn actual_reciters_prepare_and_decode_first_chapter_without_audio() {
    let env_path = |name: &str| {
        PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("missing {name}")))
    };
    let root = env_path("MANDARA_QURAN_PROVIDER_ROOT");
    let cache = private_cache(
        std::env::var("MANDARA_QURAN_PROVIDER_NETWORK").as_deref() == Ok("1"),
        &root,
        &env_path("HOME"),
        &env_path("XDG_CACHE_HOME"),
    )
    .unwrap();
    assert_eq!(
        crate::quran::store::cache_dir(),
        cache,
        "the real source must use the guarded private cache"
    );
    let quran = serde_json::from_value(serde_json::json!({"format":1,"edition":"quran-uthmani","bismillah":"","surahs":[{"number":1,"name":"","transliteration":"","meaning":"","revelation":"makki","ayahs":7,"first":1}],"ayahs":[],"juz_starts":[]})).unwrap();
    let index = AyahIndex::new(&quran);
    for voice in ["ar.yasseraldossari", "ar.abdulsamad", "mp3quran.258"] {
        let mut cache = FetchCache::default();
        let mut chapter = None;
        for global in if voice == "mp3quran.258" {
            1..=7
        } else {
            1..=2
        } {
            let prepared = Recitations
                .prepare(
                    &Request {
                        voice,
                        global,
                        index: Some(&index),
                        observer: &Live,
                    },
                    &mut cache,
                )
                .unwrap_or_else(|error| panic!("{voice} ayah {global}: {error}"));
            let bytes = std::fs::metadata(&prepared.path).unwrap().len();
            assert!(bytes > 1024);
            let mut clip = Clip::open(&prepared, 0, &Live).unwrap();
            let (frames, peak) = consume(&mut clip);
            eprintln!(
                "provider={voice} ayah={global} bytes={bytes} rate={} frames={frames} peak={peak:.4} interval={:?}",
                clip.rate, prepared.interval
            );
            if prepared.interval.is_some() {
                if let Some(path) = &chapter {
                    assert_eq!(
                        path, &prepared.path,
                        "all ayahs reuse the same chapter file"
                    );
                }
                chapter = Some(prepared.path.clone());
                if global == 2 || global == 7 {
                    check_range(&prepared);
                }
            } else {
                assert!(prepared.path.ends_with(format!("{global}.mp3")));
            }
        }
    }
}
