//! The reader: a stored, sanitized article drawn with the shell's own
//! text elements (the notes renderer for text, native images when they
//! are allowed). No web engine, no scripts, no remote requests unless
//! article pictures are switched on.

use gpui::{AnyElement, FontWeight, ImageSource, InteractiveElement, IntoElement, ObjectFit, ParentElement, SharedString, StatefulInteractiveElement, Styled, StyledImage as _, Window, div, img, px};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf};
use mandara_services::feeds::{Article, Block, Entry, Rich, html};
use mandara_theme::{Color, MandaraPalette};

use super::super::extras::caption;
use super::super::panel_fx::{WASH_SOFT, pill_button, readable, wash};
use super::images::{Fit, Shared};
use super::{Pictures, library::FeedRow};
use crate::bidi::Bidi as _;
use crate::icons::icon;
use crate::kit::{text, tokens};
use crate::markdown;

/// The reading column: text width, title size and spacing.
const TITLE_PX: f32 = 22.0;
const TITLE_LINE: f32 = 1.3;
const META_GAP_PX: f32 = 6.0;
const PART_GAP_PX: f32 = 14.0;
const IMAGE_RADIUS_PX: f32 = 12.0;
/// Article pictures are at most this tall (logical px).
const IMAGE_MAX_H_PX: f32 = 420.0;
const HERO_H_PX: f32 = 220.0;
const PLACEHOLDER_H_PX: f32 = 34.0;
const PLACEHOLDER_ICON_PX: f32 = 13.0;
/// Line height of a summary shown instead of the article.
const SUMMARY_LINE: f32 = 1.6;

/// One piece of a reader document.
pub enum Part {
    Text(markdown::Doc),
    Image { src: String, alt: String },
}

/// An article converted for drawing once, when it opens.
pub struct Doc {
    pub parts: Vec<Part>,
    pub images: usize,
}

fn inline(rich: &Rich) -> markdown::Inline {
    let spans = rich.marks.iter().map(|mark| markdown::Span {
        range: mark.start as usize..mark.end as usize,
        style: markdown::Style { bold: mark.style & html::BOLD != 0, italic: mark.style & html::ITALIC != 0, strike: mark.style & html::STRIKE != 0, code: mark.style & html::CODE != 0 },
        link: mark.link.clone(),
    }).filter(|span| rich.text.is_char_boundary(span.range.start) && rich.text.is_char_boundary(span.range.end)).collect();
    markdown::Inline { text: rich.text.clone(), spans }
}

impl Doc {
    /// Group the article's blocks into text runs (quotes and list items
    /// merged) and images.
    pub fn from_article(article: &Article) -> Self {
        let mut parts = Vec::new();
        let mut blocks: Vec<markdown::Block> = Vec::new();
        let mut images = 0;
        let flush = |blocks: &mut Vec<markdown::Block>, parts: &mut Vec<Part>| if !blocks.is_empty() { parts.push(Part::Text(markdown::Doc { blocks: std::mem::take(blocks) })); };
        for block in &article.blocks {
            match block {
                Block::Heading { level, text } => blocks.push(markdown::Block::Heading { level: (*level).clamp(2, 4) - 1, text: inline(text) }),
                Block::Paragraph(text) => blocks.push(markdown::Block::Paragraph(inline(text))),
                Block::Quote(text) => match blocks.last_mut() {
                    Some(markdown::Block::Quote(inner)) => inner.push(markdown::Block::Paragraph(inline(text))),
                    _ => blocks.push(markdown::Block::Quote(vec![markdown::Block::Paragraph(inline(text))])),
                },
                Block::Item { number, text, .. } => {
                    let item = markdown::Item { task: None, blocks: vec![markdown::Block::Paragraph(inline(text))] };
                    match blocks.last_mut() {
                        Some(markdown::Block::List { start, items }) if start.is_some() == number.is_some() => items.push(item),
                        _ => blocks.push(markdown::Block::List { start: number.map(u64::from), items: vec![item] }),
                    }
                }
                Block::Code(code) => blocks.push(markdown::Block::Code { lang: String::new(), text: code.clone() }),
                Block::Rule => blocks.push(markdown::Block::Rule),
                Block::Image { src, alt } => {
                    flush(&mut blocks, &mut parts);
                    images += 1;
                    parts.push(Part::Image { src: src.clone(), alt: alt.clone() });
                }
            }
        }
        flush(&mut blocks, &mut parts);
        Self { parts, images }
    }
}

/// Everything the reader body needs.
pub struct Body<'a> {
    pub row: &'a FeedRow,
    pub entry: &'a Entry,
    pub doc: Option<&'a Doc>,
    pub loading: bool,
    pub width: f32,
    pub pictures: Pictures,
    pub accent: Color,
    pub when: String,
}

/// The article: source line, title, author, hero picture and content.
pub fn body(b: Body<'_>, images: &Shared, mono: SharedString, palette: &MandaraPalette, window: &mut Window) -> AnyElement {
    let scale = window.scale_factor();
    let physical = |logical: f32| (logical * scale).round().max(1.0) as u32;
    let mut column = div().w_full().flex().flex_col().gap(px(PART_GAP_PX));
    let mut meta = div().row().items_center().gap(px(META_GAP_PX)).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla())
        .child(div().text_color(b.accent.hsla()).font_weight(FontWeight::SEMIBOLD).child(b.row.name().to_owned()))
        .child("·").child(div().font_features(crate::fonts::tabular_nums()).child(b.when));
    if let Some(author) = &b.entry.author { meta = meta.child("·").child(div().truncate().child(author.clone())); }
    column = column.child(meta)
        .child(div().text_size(px(TITLE_PX)).line_height(gpui::relative(TITLE_LINE)).font_weight(FontWeight::BOLD).child(crate::bidi::paragraph(b.entry.title.clone())));
    let has_images = b.doc.is_some_and(|doc| doc.images > 0);
    // The feed's own thumbnail heads the article when the content has no pictures of its own.
    if b.pictures != Pictures::Off && (!has_images || b.pictures == Pictures::Thumbnails) && let Some(src) = &b.entry.image {
        let image = images.borrow_mut().get(src, physical(b.width), physical(HERO_H_PX), Fit::Fill);
        if let Some(image) = image {
            column = column.child(img(ImageSource::Render(image)).w(px(b.width)).h(px(HERO_H_PX)).rounded(px(crate::style::radius(IMAGE_RADIUS_PX))).object_fit(ObjectFit::Cover));
        } else if !images.borrow().failed(src, physical(b.width), physical(HERO_H_PX), Fit::Fill) {
            column = column.child(div().w(px(b.width)).h(px(HERO_H_PX)).rounded(px(crate::style::radius(IMAGE_RADIUS_PX))).bg(wash(WASH_SOFT, palette).hsla()));
        }
    }
    let rcx = markdown::RenderCx { id: SharedString::from(format!("feed-article-{}", b.entry.id.len())), on_toggle: None, mono };
    match b.doc {
        Some(doc) if !doc.parts.is_empty() => {
            for (index, part) in doc.parts.iter().enumerate() {
                match part {
                    Part::Text(text_doc) => {
                        let rcx = markdown::RenderCx { id: SharedString::from(format!("{}-{index}", rcx.id)), on_toggle: None, mono: rcx.mono.clone() };
                        column = column.child(div().w(px(b.width)).child(markdown::render(text_doc, palette, &rcx)));
                    }
                    Part::Image { src, alt } if b.pictures == Pictures::All => {
                        let (w, h) = (physical(b.width), physical(IMAGE_MAX_H_PX));
                        let image = images.borrow_mut().get(src, w, h, Fit::Contain);
                        column = column.child(match image {
                            Some(image) => {
                                let size = image.size(0);
                                let (lw, lh) = (size.width.0 as f32 / scale, size.height.0 as f32 / scale);
                                img(ImageSource::Render(image)).w(px(lw.min(b.width))).h(px(lh)).rounded(px(crate::style::radius(IMAGE_RADIUS_PX))).object_fit(ObjectFit::Contain).into_any_element()
                            }
                            None if images.borrow().failed(src, w, h, Fit::Contain) => placeholder(alt, b.width, palette),
                            None => div().w(px(b.width)).h(px(HERO_H_PX)).rounded(px(crate::style::radius(IMAGE_RADIUS_PX))).bg(wash(WASH_SOFT, palette).hsla()).into_any_element(),
                        });
                    }
                    Part::Image { alt, .. } => column = column.child(placeholder(alt, b.width, palette)),
                }
            }
        }
        _ if b.loading => column = column.child(caption(t("Opening the article…"), palette)),
        _ => {
            if !b.entry.summary.is_empty() {
                column = column.child(div().text_size(px(text::BODY)).line_height(gpui::relative(SUMMARY_LINE)).child(crate::bidi::paragraph(b.entry.summary.clone())));
            }
            column = column.child(caption(t("The full article is on its website."), palette));
        }
    }
    let link = b.entry.link.clone();
    column = column.child(div().row().pt(px(tokens::SM)).child(pill_button("feeds-read-online", Some("external"), t("Read on the website"), readable(palette.primary, palette), palette, move |_, _| {
        if let Err(error) = super::open_in_browser(&link) { super::hub().borrow().flash.show(false, error); }
    }).focusable().tab_index(0)));
    column.into_any_element()
}


/// Where an article picture would be: its description, when pictures are off.
fn placeholder(alt: &str, width: f32, palette: &MandaraPalette) -> AnyElement {
    div().w(px(width)).h(px(PLACEHOLDER_H_PX)).px(px(tokens::MD)).rounded(px(crate::style::radius(IMAGE_RADIUS_PX))).bg(wash(WASH_SOFT, palette).hsla())
        .row().items_center().gap(px(tokens::SM)).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla())
        .child(icon("image", PLACEHOLDER_ICON_PX, palette.on_surface_variant))
        .child(div().truncate().child(if alt.is_empty() { t("Picture").to_string() } else { tf("Picture: {alt}", &[("alt", &alt)]) }))
        .into_any_element()
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn articles_group_into_text_runs_lists_quotes_and_images() {
        let article = html::sanitize("<h1>Big</h1><p>One <b>two</b></p><blockquote><p>q1</p><p>q2</p></blockquote><ul><li>a</li><li>b</li></ul><ol><li>c</li></ol><img src=\"https://example.org/x.png\" alt=\"x\"><p>after</p>", Some("https://example.org/"));
        let doc = Doc::from_article(&article);
        assert_eq!(doc.images, 1);
        assert_eq!(doc.parts.len(), 3);
        let Part::Text(first) = &doc.parts[0] else { panic!() };
        assert!(matches!(&first.blocks[0], markdown::Block::Heading { level: 1, .. }), "an h1 inside the article sits below its title");
        let markdown::Block::Paragraph(p) = &first.blocks[1] else { panic!() };
        assert!(p.spans.iter().any(|s| s.style.bold && &p.text[s.range.clone()] == "two"));
        assert!(matches!(&first.blocks[2], markdown::Block::Quote(inner) if inner.len() == 2));
        assert!(matches!(&first.blocks[3], markdown::Block::List { start: None, items } if items.len() == 2));
        assert!(matches!(&first.blocks[4], markdown::Block::List { start: Some(1), items } if items.len() == 1));
        assert!(matches!(&doc.parts[1], Part::Image { src, .. } if src == "https://example.org/x.png"));
    }
}
