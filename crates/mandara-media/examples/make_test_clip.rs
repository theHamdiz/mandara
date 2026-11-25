//! Generates a synthetic AV1 clip for testing the login screen's video
//! backgrounds without downloading anything:
//!
//! ```sh
//! cargo run -p mandara-media --example make_test_clip -- OUT.webm [WxH] [frames] [fps] [detail]
//! ```
//!
//! `.webm` / `.mkv` paths are muxed as WebM, anything else as IVF.

mod clipgen;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(out) = args.first() else {
        eprintln!("usage: make_test_clip OUT.(webm|ivf) [WIDTHxHEIGHT=1280x720] [FRAMES=72] [FPS=24] [DETAIL=2]");
        std::process::exit(2);
    };
    let (width, height) = args
        .get(1)
        .and_then(|s| s.split_once('x'))
        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        .unwrap_or((1280, 720));
    let frames = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(72);
    let fps = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(24);
    let detail = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(2);
    let path = std::path::PathBuf::from(out);
    let webm = path.extension().is_some_and(|e| e == "webm" || e == "mkv");
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        let _ = std::fs::create_dir_all(dir);
    }
    let spec = clipgen::ClipSpec { width, height, fps, frames, detail, webm, speed: 10 };
    let t = std::time::Instant::now();
    match clipgen::write_clip(&path, &spec) {
        Ok(()) => println!("wrote {} ({width}x{height}, {frames} frames @ {fps} fps) in {:.1}s", path.display(), t.elapsed().as_secs_f32()),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
