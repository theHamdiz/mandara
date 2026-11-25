//! Dev tool: prints the live spectrum of what's playing for a few seconds.
//! `cargo run -p mandara-services --example spectrum`
fn main() {
    let Some(s) = mandara_services::spectrum::subscribe() else {
        eprintln!("no sound server");
        return;
    };
    for _ in 0..12 {
        std::thread::sleep(std::time::Duration::from_millis(250));
        let bars: String = s.bands().iter().map(|v| [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'][(v * 8.0).round().clamp(0.0, 8.0) as usize]).collect();
        println!("{} |{bars}|", if s.is_active() { "♪" } else { "·" });
    }
}
