//! Measures pure-Rust AV1 decode (+ BGRA conversion) throughput:
//!
//! ```sh
//! cargo run --release -p mandara-media --example decode_bench -- CLIP.webm [frames]
//! ```
//!
//! Prints full / lite decoding and full / half-resolution conversion fps.

use mandara_media::av1::DecoderSettings;
use mandara_media::bench;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("usage: decode_bench CLIP [FRAMES=240] [THREADS=0 (all cores)] [quick]");
        std::process::exit(2);
    };
    let frames = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(240);
    let threads = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let only_first = args.get(3).is_some_and(|s| s == "quick");
    let path = std::path::Path::new(path);
    let modes: &[(bool, bool)] = if only_first { &[(false, false)] } else { &[(false, false), (false, true), (true, false), (true, true)] };
    for &(lite, half) in modes {
        match bench(path, DecoderSettings { threads, lite, ..DecoderSettings::default() }, half, frames) {
            Ok(r) => println!(
                "{}x{} {:<5} {:<5} {:>4} frames: decode {:>6.1} fps · decode+convert {:>6.1} fps sequential, ~{:>6.1} fps pipelined (convert {:.2} ms/frame)",
                r.width,
                r.height,
                if lite { "lite" } else { "full" },
                if half { "half" } else { "1:1" },
                r.frames,
                r.decode_fps(),
                r.total_fps(),
                r.pipelined_fps(),
                r.convert.as_secs_f64() * 1000.0 / r.frames.max(1) as f64,
            ),
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    }
}
