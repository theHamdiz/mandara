//! Prints file sizes per style and format, with and without the dither.
use mandara_wallgen::*;
fn main() {
    let styles: Vec<Style> = std::env::args().skip(1).filter_map(|a| Style::from_id(&a)).collect();
    let styles = if styles.is_empty() { Style::ALL.to_vec() } else { styles };
    let (w, h) = std::env::var("WG_SIZE").ok().and_then(|s| s.split_once('x').map(|(a, b)| (a.parse().unwrap(), b.parse().unwrap()))).unwrap_or((3840, 2160));
    let png = std::env::var_os("WG_PNG").is_some();
    for s in styles {
        for d in [0.0, 100.0] {
            let mut r = Recipe::new(s, 1);
            r.set_param("dither", d);
            let img = render(&r, w, h);
            let mut line = format!("{:<10} dither {d:>3}:", s.id());
            let formats: Vec<Format> = if png { vec![Format::WebpLossless, Format::Png] } else { vec![Format::WebpLossless] };
            for f in formats {
                let t = std::time::Instant::now();
                let n = encode(&img, f).unwrap().len();
                line += &format!("  {} {:>6} KiB ({} ms)", f.id(), n / 1024, t.elapsed().as_millis());
            }
            println!("{line}");
        }
    }
}
