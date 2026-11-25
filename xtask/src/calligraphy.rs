//! `cargo xtask art --calligraphy`: the Kufic words behind the main menu's
//! app and tool tiles, as vector paths.
//!
//! Each word is shaped with rustybuzz in Reem Kufi (a Fatimid Kufic, OFL
//! 1.1) and its glyph outlines are written out as one SVG path, so the
//! shell draws real calligraphy without shipping or depending on the font.
//! The output, `crates/mandara-ui/assets/calligraphy/words.rs`, is
//! committed (a build input, like the plugin art) next to the font's
//! licence. Regenerating needs the font: `MANDARA_CALLIGRAPHY_FONT`
//! overrides the default path. `art --check` validates the committed table
//! without it.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use rustybuzz::ttf_parser::{GlyphId, OutlineBuilder};

use crate::art::Report;
use crate::{Result, root};

/// Where the table and the licence go, relative to the repository root.
pub const OUT_DIR: &str = "crates/mandara-ui/assets/calligraphy";
/// The generated table `mandara_ui::plugins::main_menu::ornament` includes.
pub const TABLE_FILE: &str = "words.rs";
/// The font's licence, shipped next to the outlines drawn from it.
pub const LICENCE_FILE: &str = "OFL.txt";
/// Reem Kufi Regular as Arch's `ttf-reem-kufi` installs it.
const DEFAULT_FONT: &str = "/usr/share/fonts/TTF/ReemKufi-Regular.ttf";
/// Its licence from the same package.
const DEFAULT_LICENCE: &str = "/usr/share/licenses/ttf-reem-kufi/OFL.txt";
/// The licence must name the font it covers.
const LICENCE_MARK: &str = "Reem Kufi";

/// (key, word): the tile each word belongs to and the Arabic it writes.
/// Words, not labels: a name in the spirit of what the tile does.
pub const WORDS: &[(&str, &str)] = &[
    // "Set off!": the launcher.
    ("launcher", "انطلق"),
    // "Backgrounds".
    ("wallpapers", "خلفيات"),
    // "Side".
    ("sidebar", "جانب"),
    // El-Mandara, the Alexandrian district the shell is named after.
    ("studio", "المندرة"),
    // "A snapshot".
    ("screenshot", "لقطة"),
    // "The keeper" (the clipboard).
    ("clipboard", "حافظة"),
    // "Editing".
    ("edit", "تحرير"),
];

/// A path command in font units, y down, before normalising.
enum Seg {
    M(f32, f32),
    L(f32, f32),
    Q(f32, f32, f32, f32),
    C(f32, f32, f32, f32, f32, f32),
    Z,
}

/// Collects one glyph's outline, placed at the pen.
struct Collect<'a> {
    out: &'a mut Vec<Seg>,
    dx: f32,
    dy: f32,
}

impl Collect<'_> {
    fn at(&self, x: f32, y: f32) -> (f32, f32) {
        (self.dx + x, -(self.dy + y))
    }
}

impl OutlineBuilder for Collect<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.at(x, y);
        self.out.push(Seg::M(x, y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.at(x, y);
        self.out.push(Seg::L(x, y));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (a, b) = self.at(x1, y1);
        let (x, y) = self.at(x, y);
        self.out.push(Seg::Q(a, b, x, y));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (a, b) = self.at(x1, y1);
        let (c, d) = self.at(x2, y2);
        let (x, y) = self.at(x, y);
        self.out.push(Seg::C(a, b, c, d, x, y));
    }
    fn close(&mut self) {
        self.out.push(Seg::Z);
    }
}

/// One shaped word: its size in font units and its path, origin at the
/// top-left of its outline's box.
pub struct Word {
    pub width: f32,
    pub height: f32,
    pub path: String,
}

/// Shapes `text` right to left in `font` and outlines it.
pub fn outline(font: &[u8], text: &str) -> Result<Word> {
    let face = rustybuzz::Face::from_slice(font, 0).ok_or("not a font")?;
    let mut buffer = rustybuzz::UnicodeBuffer::new();
    buffer.push_str(text);
    buffer.set_direction(rustybuzz::Direction::RightToLeft);
    let shaped = rustybuzz::shape(&face, &[], buffer);
    let mut segs = Vec::new();
    // Glyphs come in visual order, left to right.
    let mut pen = 0.0;
    for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
        let glyph = GlyphId(u16::try_from(info.glyph_id).map_err(|_| "glyph id out of range")?);
        let mut c = Collect { out: &mut segs, dx: pen + pos.x_offset as f32, dy: pos.y_offset as f32 };
        face.outline_glyph(glyph, &mut c);
        pen += pos.x_advance as f32;
    }
    let mut points = segs.iter().flat_map(|s| match *s {
        Seg::M(x, y) | Seg::L(x, y) => vec![(x, y)],
        Seg::Q(a, b, x, y) => vec![(a, b), (x, y)],
        Seg::C(a, b, c, d, x, y) => vec![(a, b), (c, d), (x, y)],
        Seg::Z => Vec::new(),
    });
    let first = points.next().ok_or_else(|| format!("{text}: draws nothing"))?;
    let (mut x0, mut y0, mut x1, mut y1) = (first.0, first.1, first.0, first.1);
    for (x, y) in points {
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    // Whole font units: a tenth of a pixel at the sizes the menu draws.
    let n = |v: f32, o: f32| (v - o).round() as i32;
    let mut path = String::new();
    for s in &segs {
        let _ = match *s {
            Seg::M(x, y) => write!(path, "M{} {}", n(x, x0), n(y, y0)),
            Seg::L(x, y) => write!(path, "L{} {}", n(x, x0), n(y, y0)),
            Seg::Q(a, b, x, y) => write!(path, "Q{} {} {} {}", n(a, x0), n(b, y0), n(x, x0), n(y, y0)),
            Seg::C(a, b, c, d, x, y) => write!(path, "C{} {} {} {} {} {}", n(a, x0), n(b, y0), n(c, x0), n(d, y0), n(x, x0), n(y, y0)),
            Seg::Z => write!(path, "Z"),
        };
    }
    Ok(Word { width: (x1 - x0).round(), height: (y1 - y0).round(), path })
}

fn out_dir() -> PathBuf {
    root().join(OUT_DIR)
}

/// Writes the table and copies the licence.
pub fn generate() -> Result<()> {
    let font_path = std::env::var("MANDARA_CALLIGRAPHY_FONT").unwrap_or_else(|_| DEFAULT_FONT.to_string());
    let font = std::fs::read(&font_path).map_err(|e| format!("{font_path}: {e} (set MANDARA_CALLIGRAPHY_FONT to Reem Kufi Regular)"))?;
    let licence_path = std::env::var("MANDARA_CALLIGRAPHY_LICENCE").unwrap_or_else(|_| DEFAULT_LICENCE.to_string());
    let licence = std::fs::read_to_string(&licence_path).map_err(|e| format!("{licence_path}: {e}"))?;
    if !licence.contains(LICENCE_MARK) {
        return Err(format!("{licence_path} is not Reem Kufi's licence"));
    }
    let mut table = String::from("// Generated by `cargo xtask art --calligraphy` from Reem Kufi (OFL 1.1, see OFL.txt). Do not edit.\n// (key, word, width, height, path): font units, origin at the outline's top-left.\n&[\n");
    for (key, text) in WORDS {
        let w = outline(&font, text)?;
        let _ = writeln!(table, "    (\"{key}\", \"{text}\", {:.1}, {:.1}, \"{}\"),", w.width, w.height, w.path);
    }
    table.push_str("]\n");
    let dir = out_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    write(&dir.join(TABLE_FILE), &table)?;
    write(&dir.join(LICENCE_FILE), &licence)?;
    println!("calligraphy: {} words, {} bytes", WORDS.len(), table.len());
    Ok(())
}

fn write(path: &Path, text: &str) -> Result<()> {
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The committed table holds every word and the licence is next to it.
pub fn check(r: &mut Report) {
    let dir = out_dir();
    let table = std::fs::read_to_string(dir.join(TABLE_FILE)).unwrap_or_default();
    for (key, text) in WORDS {
        r.ensure(table.contains(&format!("(\"{key}\", \"{text}\"")), || format!("{OUT_DIR}/{TABLE_FILE} lacks {key} — run `cargo xtask art --calligraphy`"));
    }
    let licence = std::fs::read_to_string(dir.join(LICENCE_FILE)).unwrap_or_default();
    r.ensure(licence.contains(LICENCE_MARK) && licence.contains("SIL OPEN FONT LICENSE"), || format!("{OUT_DIR}/{LICENCE_FILE} must be Reem Kufi's OFL"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_committed_words_are_complete() {
        let mut r = Report::default();
        check(&mut r);
        assert!(r.0.is_empty(), "{}", r.0.join("\n"));
    }

    /// Where the font is installed, the committed table is what it makes.
    #[test]
    fn the_table_matches_the_font_when_installed() {
        let Ok(font) = std::fs::read(std::env::var("MANDARA_CALLIGRAPHY_FONT").unwrap_or_else(|_| DEFAULT_FONT.to_string())) else { return };
        let table = std::fs::read_to_string(out_dir().join(TABLE_FILE)).unwrap();
        for (key, text) in WORDS {
            let w = outline(&font, text).unwrap();
            assert!(w.width > 0.0 && w.height > 0.0, "{key}");
            assert!(table.contains(&w.path), "{key}: stale — run `cargo xtask art --calligraphy`");
        }
    }
}
