//! Dev tool: records the screen for N seconds (default 3) and prints stats.
//! `cargo run -p mandara-services --example record -- 3 /tmp/out.webm`

fn main() -> Result<(), String> {
    let secs: f64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(3.0);
    let path = std::env::args().nth(2).map(Into::into).unwrap_or_else(mandara_services::recording::default_path);
    let rec = mandara_services::recording::start(mandara_services::recording::RecordOptions {
        output: String::new(),
        region: None,
        fps: 30,
        max_width: std::env::args().nth(3).and_then(|s| s.parse().ok()).unwrap_or(1920),
        path,
    })?;
    std::thread::sleep(std::time::Duration::from_secs_f64(secs));
    let r = rec.stop()?;
    println!("{} — {}×{} {:.2}s, {} frames ({} dropped), {} KiB", r.path.display(), r.width, r.height, r.duration.as_secs_f64(), r.frames, r.dropped, r.bytes / 1024);
    Ok(())
}
