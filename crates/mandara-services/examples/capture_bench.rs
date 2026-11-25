//! Dev tool: times screencopy grabs and YUV conversion separately.
use std::time::Instant;

fn main() -> Result<(), String> {
    let mut src = mandara_services::screencopy::FrameSource::new("", None).map_err(|e| format!("{e:#}"))?;
    let (mut grab, mut conv) = (0.0, 0.0);
    for _ in 0..20 {
        let t = Instant::now();
        let f = src.next_frame().map_err(|e| format!("{e:#}"))?;
        grab += t.elapsed().as_secs_f64();
        let t = Instant::now();
        let _ = mandara_services::recording::to_yuv420(f.data, f.width, f.height, f.stride, f.bgr, f.y_invert, 1920, 1080);
        conv += t.elapsed().as_secs_f64();
    }
    println!("grab {:.1} ms, convert {:.1} ms per frame", grab / 20.0 * 1000.0, conv / 20.0 * 1000.0);
    Ok(())
}
