//! Small builders for view arenas: [`Tree`] for extension views and
//! [`Legacy`] for the older exports (the bar item, desktop widget and the
//! panel older hosts ask for).

use crate::mandara::plugin::types::{Kind, Node, Tone};
use crate::mandara::plugin::ui;
use crate::moon::{self, Hemisphere, Moon};
use crate::{View, WidgetContext, host};

/// An extension view under construction.
#[derive(Default)]
pub struct Tree {
    nodes: Vec<ui::Node>,
}

fn style() -> ui::Style {
    ui::Style { width: 0.0, height: 0.0, min_width: 0.0, min_height: 0.0, padding: 0.0, gap: 0.0, radius: 0.0, size: 0.0, grow: false, bold: false, align: ui::Align::Auto, tone: Tone::Normal }
}

impl Tree {
    pub fn add(&mut self, kind: ui::Kind, text: &str) -> u32 {
        self.nodes.push(ui::Node { id: String::new(), kind, text: text.into(), hint: String::new(), action: String::new(), value: 0.0, min: 0.0, max: 0.0, step: 0.0, index: 0, children: Vec::new(), style: style(), options: Vec::new(), image: 0, marks: Vec::new(), disabled: false });
        (self.nodes.len() - 1) as u32
    }

    pub fn node(&mut self, i: u32) -> &mut ui::Node {
        &mut self.nodes[i as usize]
    }

    /// Text at `size` px (0 = the default size).
    pub fn text(&mut self, text: &str, size: f32) -> u32 {
        let i = self.add(ui::Kind::Text, text);
        self.node(i).style.size = size;
        i
    }

    pub fn tone(&mut self, i: u32, tone: Tone) {
        self.node(i).style.tone = tone;
    }

    pub fn bold(&mut self, i: u32) {
        self.node(i).style.bold = true;
    }

    pub fn grow(&mut self, i: u32) {
        self.node(i).style.grow = true;
    }

    pub fn align(&mut self, i: u32, align: ui::Align) {
        self.node(i).style.align = align;
    }

    pub fn children(&mut self, parent: u32, kids: &[u32]) -> u32 {
        self.node(parent).children = kids.to_vec();
        parent
    }

    pub fn row(&mut self, gap: f32, kids: &[u32]) -> u32 {
        let i = self.add(ui::Kind::Row, "");
        self.node(i).style.gap = gap;
        self.children(i, kids)
    }

    pub fn column(&mut self, gap: f32, kids: &[u32]) -> u32 {
        let i = self.add(ui::Kind::Column, "");
        self.node(i).style.gap = gap;
        self.children(i, kids)
    }

    /// An inset grouped section of `rows`, separated by hairlines.
    pub fn group(&mut self, rows: &[u32]) -> u32 {
        let i = self.add(ui::Kind::Group, "");
        self.children(i, rows)
    }

    pub fn view(self, root: u32, focus: &str) -> ui::View {
        ui::View { nodes: self.nodes, root, focus: focus.into() }
    }
}

/// A view for the older exports.
#[derive(Default)]
pub struct Legacy {
    nodes: Vec<Node>,
}

/// Sizes and spacing of the older views (logical px).
const HERO_MOON_PX: f32 = 56.0;
const TITLE_PX: f32 = 16.0;
const GAP: f32 = 6.0;
const STACK_GAP: f32 = 4.0;

impl Legacy {
    pub fn add(&mut self, kind: Kind, text: &str) -> u32 {
        self.nodes.push(Node { kind, text: text.into(), value: 0.0, children: Vec::new(), action: String::new(), tone: Tone::Normal, size: 0.0, gap: GAP, bold: false, grow: false });
        (self.nodes.len() - 1) as u32
    }

    pub fn node(&mut self, i: u32) -> &mut Node {
        &mut self.nodes[i as usize]
    }

    pub fn children(&mut self, parent: u32, kids: &[u32]) -> u32 {
        self.node(parent).children = kids.to_vec();
        parent
    }

    pub fn view(self, root: u32) -> Option<View> {
        Some(View { nodes: self.nodes, root })
    }

    /// Tonight's moon with its name and how lit it is.
    fn moon(t: &mut Self, m: &Moon, hemi: Hemisphere, lit: &str) -> u32 {
        let col = t.add(Kind::Column, "");
        t.node(col).gap = STACK_GAP;
        let glyph = t.add(Kind::Text, moon::glyph(m.phase_index(), hemi));
        t.node(glyph).size = HERO_MOON_PX;
        let name = t.add(Kind::Text, &host::translate(m.name()));
        t.node(name).size = TITLE_PX;
        t.node(name).bold = true;
        let detail = t.add(Kind::Text, lit);
        t.node(detail).tone = Tone::Muted;
        t.children(col, &[glyph, name, detail])
    }

    /// The panel and card older hosts draw.
    pub fn summary(m: &Moon, hemi: Hemisphere) -> Option<View> {
        let mut t = Self::default();
        let lit = host::translate("{percent}% illuminated").replace("{percent}", &format!("{:.0}", m.illumination() * 100.0));
        let root = Self::moon(&mut t, m, hemi, &lit);
        t.view(root)
    }

    /// The desktop widget, with the digits the person chose for it.
    pub fn widget(m: &Moon, hemi: Hemisphere, context: &WidgetContext) -> Option<View> {
        let language = if context.language.is_empty() { host::locale() } else { context.language.clone() };
        let eastern = context.numerals == "eastern-arabic" || (context.numerals == "auto" && language == "ar");
        let lit = host::translate("{percent}% illuminated").replace("{percent}", &digits(&format!("{:.0}", m.illumination() * 100.0), eastern));
        let mut t = Self::default();
        let root = Self::moon(&mut t, m, hemi, &lit);
        t.view(root)
    }
}

/// `s` with Eastern Arabic digits when `eastern`.
fn digits(s: &str, eastern: bool) -> String {
    const EASTERN: [char; 10] = ['٠', '١', '٢', '٣', '٤', '٥', '٦', '٧', '٨', '٩'];
    if !eastern {
        return s.to_string();
    }
    s.chars().map(|c| c.to_digit(10).map_or(c, |d| EASTERN[d as usize])).collect()
}
