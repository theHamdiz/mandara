//! A small builder for view arenas.

use crate::mandara::plugin::types::Tone;
use crate::mandara::plugin::ui;

/// A view under construction.
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

/// `s` with Eastern Arabic digits when `eastern`.
pub fn digits(s: &str, eastern: bool) -> String {
    const EASTERN: [char; 10] = ['٠', '١', '٢', '٣', '٤', '٥', '٦', '٧', '٨', '٩'];
    if !eastern {
        return s.to_string();
    }
    s.chars().map(|c| c.to_digit(10).map_or(c, |d| EASTERN[d as usize])).collect()
}
