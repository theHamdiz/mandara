//! End to end through the wallpaper generator plugin's own layers: command
//! words → request → job runner → files on disk, then the `list` /
//! `remix` / prune paths and the picker's listing, all against a temporary
//! wallpaper folder. Sizes stay tiny so the suite runs in seconds.

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use mandara_config::wallgen::{WallgenKind, WallgenSettings};
use mandara_ui::plugins::wallpaper_generator::{self as wg, Inputs, cli, jobs, model};
use mandara_wallgen::{PaletteSpec, Recipe, Style};

/// Longest a tiny job may take before the test gives up.
const JOB_TIMEOUT: Duration = Duration::from_secs(120);

fn inputs(wallpapers: &Path) -> Inputs {
    let mut settings = WallgenSettings::default();
    // A recipe of our own, so "the current recipe" is recognisable.
    model::store_recipe(&mut settings, &Recipe::random(Some(Style::Corniche), 11));
    Inputs {
        settings,
        theme_dark: true,
        theme_palette: PaletteSpec::Theme(vec!["#0b1020".into(), "#4fd1c5".into(), "#f6e05e".into()]),
        wallpaper_dir: wallpapers.to_path_buf(),
        display: Some((1920, 1080)),
        base: None,
        seed: 1234,
    }
}

fn words(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_string).collect()
}

/// Parses `line` as `wallgen generate`'s words, runs it through the job
/// runner and waits for the result.
fn generate(runner: &jobs::JobRunner, inputs: &Inputs, line: &str, origin: jobs::Origin) -> jobs::Finished {
    let g = cli::parse_generate(&words(line)).unwrap_or_else(|e| panic!("{line}: {e}"));
    let request = wg::build_request(inputs, &g, origin).unwrap();
    let (tx, rx) = mpsc::channel();
    runner.start(request, move |f| tx.send(f).unwrap()).unwrap();
    rx.recv_timeout(JOB_TIMEOUT).expect("the job finished")
}

#[test]
fn a_still_is_written_with_its_sidecar_size_and_format() {
    let dir = tempfile::tempdir().unwrap();
    let i = inputs(dir.path());
    let runner = jobs::JobRunner::default();
    let f = generate(&runner, &i, "--size 96x54 --format png --no-apply", jobs::Origin::Manual);
    let done = f.result.expect("generated");
    assert!(done.file.path.starts_with(dir.path().join("Generated")), "{}", done.file.path.display());
    assert_eq!(done.file.path.extension().unwrap(), "png");
    assert!(done.file.recipe_path.exists(), "the recipe sidecar is written");
    assert_eq!((done.file.width, done.file.height), (96, 54));
    let img = image::open(&done.file.path).unwrap();
    assert_eq!((img.width(), img.height()), (96, 54), "--size is respected");
    assert_eq!(done.file.bytes, std::fs::metadata(&done.file.path).unwrap().len());
    assert!(!f.request.apply, "--no-apply");
    // The sidecar carries the exact recipe: the settings' current one.
    let recipe = mandara_wallgen::recipe_for(&done.file.path).unwrap();
    assert_eq!(recipe, model::recipe(&i.settings, true));
    // JSON for the command reply.
    let json = done.json();
    assert_eq!(json["kind"], "image");
    assert_eq!(json["width"], 96);
    assert!(json["seconds"].as_f64().unwrap() >= 0.0);
    // The status snapshot remembers the last job.
    let status = runner.status().json();
    assert!(status["running"].is_null());
    assert_eq!(status["last"]["ok"], true);
    assert_eq!(status["last"]["id"], f.id);
}

#[test]
fn every_format_and_flag_reaches_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let i = inputs(dir.path());
    let runner = jobs::JobRunner::default();
    for (format, ext) in [("webp", "webp"), ("png", "png"), ("jpeg", "jpg")] {
        let f = generate(&runner, &i, &format!("aurora --size 64x36 --format {format} --palette nord --seed 0x2a --light"), jobs::Origin::Manual);
        let done = f.result.unwrap();
        assert_eq!(done.file.path.extension().unwrap(), ext);
        let decoded = image::open(&done.file.path).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (64, 36));
        let r = mandara_wallgen::recipe_for(&done.file.path).unwrap();
        assert_eq!((r.style, r.seed, r.light, r.palette.clone()), (Style::Aurora, 42, true, PaletteSpec::Named("nord".into())));
    }
    // Lossless (the default) writes whichever exact file is smaller, named by
    // its real format.
    let f = generate(&runner, &i, "aurora --size 64x36 --format lossless --seed 0x2a", jobs::Origin::Manual);
    let done = f.result.unwrap();
    let ext = done.file.path.extension().unwrap().to_string_lossy().into_owned();
    assert!(ext == "webp" || ext == "png", "{ext}");
    let fresh = generate(&runner, &i, "aurora --size 64x36 --seed 0x2a", jobs::Origin::Manual).result.unwrap();
    assert_eq!(fresh.file.bytes, done.file.bytes, "lossless is the default");
    assert_eq!(image::open(&done.file.path).unwrap().to_rgb8(), image::open(&fresh.file.path).unwrap().to_rgb8());
    // The live theme as a palette.
    let f = generate(&runner, &i, "random --size 48x27 --palette theme", jobs::Origin::Manual);
    assert_eq!(mandara_wallgen::recipe_for(&f.result.unwrap().file.path).unwrap().palette, i.theme_palette);
    // Size classes resolve like the setting (the display is 16:9 here).
    let g = cli::parse_generate(&words("--size display")).unwrap();
    let jobs::Output::Still(o) = wg::build_request(&i, &g, jobs::Origin::Manual).unwrap().output else { panic!() };
    assert_eq!(o.resolution.size(), (3840, 2160));
}

#[test]
fn a_tiny_video_loop_encodes_through_the_runner() {
    let dir = tempfile::tempdir().unwrap();
    let i = inputs(dir.path());
    let runner = jobs::JobRunner::default();
    let f = generate(&runner, &i, "waves --video --seconds 1 --fps 3 --size 64x36", jobs::Origin::Manual);
    let done = f.result.expect("loop written");
    assert!(done.video);
    assert_eq!(done.file.path.extension().unwrap(), "webm");
    assert!(done.file.bytes > 0 && done.file.recipe_path.exists());
    assert_eq!((done.file.width, done.file.height), (64, 36));
    // The picker shows it as a video wallpaper, flagged as generated.
    let listed = mandara_ui::wallpaper::list(dir.path());
    assert!(listed.contains(&done.file.path));
    assert!(mandara_media::is_video(&done.file.path));
    assert!(model::generated_among(&listed).contains(&done.file.path));
    // It decodes back into its frames.
    let frame = mandara_media::first_frame(&done.file.path).expect("first frame");
    assert_eq!((frame.width(), frame.height()), (64, 36));
    let clip = mandara_media::Demuxer::open(&done.file.path).unwrap().info();
    assert_eq!((clip.width, clip.height), (64, 36));
}

#[test]
fn cancel_stops_a_loop_and_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let i = inputs(dir.path());
    let runner = jobs::JobRunner::default();
    let g = cli::parse_generate(&words("dunes --video --seconds 60 --fps 60 --size 640x360")).unwrap();
    let request = wg::build_request(&i, &g, jobs::Origin::Manual).unwrap();
    let (tx, rx) = mpsc::channel();
    runner.start(request.clone(), move |f| tx.send(f).unwrap()).unwrap();
    assert!(runner.busy());
    assert!(runner.start(request, |_| {}).is_err(), "one job at a time");
    assert!(runner.status().running.is_some());
    assert!(runner.cancel());
    let f = rx.recv_timeout(JOB_TIMEOUT).unwrap();
    assert!(f.result.is_err(), "cancelled");
    assert!(!runner.busy());
    assert!(!runner.cancel(), "nothing left to cancel");
    let left: Vec<_> = std::fs::read_dir(dir.path().join("Generated")).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    assert!(left.iter().all(|p| !mandara_media::is_video(p)), "no partial loop: {left:?}");
}

#[test]
fn list_remix_and_prune_find_the_recipes() {
    let dir = tempfile::tempdir().unwrap();
    let mut i = inputs(dir.path());
    let runner = jobs::JobRunner::default();
    let first = generate(&runner, &i, "glass --size 48x27", jobs::Origin::Manual).result.unwrap();
    std::thread::sleep(Duration::from_millis(20));
    let second = generate(&runner, &i, "bokeh --size 48x27", jobs::Origin::Manual).result.unwrap();
    // `wallgen list`: newest first, with style and seed.
    let out = i.settings.output_dir(dir.path());
    let list = wg::list_json(&out, 10);
    assert_eq!(list["count"], 2);
    assert_eq!(list["items"][0]["style"], "bokeh");
    assert_eq!(list["items"][1]["path"], first.file.path.display().to_string());
    // `wallgen remix`: the file's recipe becomes the current one.
    let recipe = wg::remix_recipe(&second.file.path).unwrap();
    model::store_recipe(&mut i.settings, &recipe);
    assert_eq!(model::recipe(&i.settings, true), recipe);
    assert!(wg::remix_recipe(&dir.path().join("nope.png")).is_err());
    // A scheduled run keeps only the newest N (its own file included).
    i.settings.schedule.keep = 2;
    i.settings.schedule.kind = WallgenKind::Still;
    i.settings.size = mandara_config::wallgen::WallgenSize::Custom;
    i.settings.custom_width = 48;
    i.settings.custom_height = 27;
    std::thread::sleep(Duration::from_millis(20));
    let g = wg::scheduled(&i.settings);
    let request = wg::build_request(&i, &g, jobs::Origin::Schedule).unwrap();
    assert_eq!(request.keep, Some(2));
    let (tx, rx) = mpsc::channel();
    runner.start(request, move |f| tx.send(f).unwrap()).unwrap();
    let done = rx.recv_timeout(JOB_TIMEOUT).unwrap().result.unwrap();
    assert_eq!(done.pruned, vec![first.file.path.clone()], "the oldest goes");
    assert!(!first.file.path.exists() && !first.file.recipe_path.exists());
    assert_eq!(wg::list_json(&out, 10)["count"], 2);
    // Styles and palettes describe the engine.
    assert_eq!(wg::styles_json()["styles"].as_array().unwrap().len(), Style::ALL.len());
    assert!(!wg::palettes_json()["palettes"].as_array().unwrap().is_empty());
}
