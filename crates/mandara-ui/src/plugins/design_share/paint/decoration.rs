//! Prepare fixed ornaments once; strip clipping cannot reshape their curves.

use mandara_i18n::t;
use mandara_theme::Color;
use tiny_skia::Transform;

use super::super::document::{Document, Layout, Ornament};
use super::{Decoration, Ground, Raster};
use crate::lock::paint as lp;
use crate::plugins::jannah::art;

/// Include every antialiased edge outside the vector's stroke bounds.
const EDGE_ROOM: f32 = 2.0;

/// Parses a bundled ornament SVG with its black ink replaced by `color`.
fn tinted(source: Option<String>, color: Color) -> Result<resvg::usvg::Tree, String> {
    let [r, g, b, a] = color.to_rgba8();
    let source = source
        .ok_or_else(|| t("The image design could not be prepared").to_string())?
        .replace("black", &format!("#{r:02x}{g:02x}{b:02x}{a:02x}"));
    resvg::usvg::Tree::from_str(&source, &resvg::usvg::Options::default())
        .map_err(|error| error.to_string())
}

impl Decoration {
    /// Renders the export's fixed ornaments once, at their final scale, so
    /// strips only blit them.
    pub(in crate::plugins::design_share) fn new(
        doc: &Document,
        layout: &Layout,
    ) -> Result<Self, String> {
        let width = layout.width as f32;
        let scale = doc.scale;
        let lattice = (doc.lattice_alpha > 0.0)
            .then(|| {
                tinted(
                    art::asset("jannah-lattice"),
                    doc.accent.with_alpha(doc.lattice_alpha),
                )
            })
            .transpose()?;
        let lattice_transform =
            Transform::from_scale(width / art::LATTICE_VIEW, width / art::LATTICE_VIEW);
        let (head, transform, glow_y) = match doc.ornament {
            Ornament::Plain => (None, Transform::identity(), None),
            Ornament::Crown { inset, alpha, .. } => {
                let inset = inset * scale;
                let k = (width - inset * 2.0) / art::CROWN_VIEW.0;
                (
                    Some(tinted(
                        art::asset("jannah-crown"),
                        doc.accent.with_alpha(alpha),
                    )?),
                    Transform::from_row(k, 0.0, 0.0, k, inset, inset),
                    Some(
                        inset
                            + art::CROWN_VIEW.1 * k * crate::plugins::jannah::panel::DAWN_TOP_SHARE,
                    ),
                )
            }
            Ornament::Manuscript {
                width_share,
                top,
                dawn_top,
            } => {
                let drawn = width * width_share;
                let k = drawn / crate::plugins::hadith::art::HEADPIECE_VIEW.0;
                (
                    Some(tinted(
                        crate::plugins::hadith::art::asset("hadith-headpiece"),
                        doc.accent,
                    )?),
                    Transform::from_row(k, 0.0, 0.0, k, (width - drawn) / 2.0, top * scale),
                    Some(dawn_top * scale),
                )
            }
        };
        let bounds = |tree: &resvg::usvg::Tree, transform| {
            tree.root()
                .abs_layer_bounding_box()
                .transform(transform)
                .map(|bounds| bounds.bottom() + EDGE_ROOM)
        };
        let dawn = crate::plugins::jannah::panel::DAWN_GLOW;
        let bottom = lattice
            .as_ref()
            .and_then(|tree| bounds(tree, lattice_transform))
            .into_iter()
            .chain(head.as_ref().and_then(|tree| bounds(tree, transform)))
            .chain(glow_y.map(|y| y + dawn.1 * scale + EDGE_ROOM))
            .fold(0.0, f32::max)
            .ceil()
            .min(layout.height as f32) as u32;
        let top = if bottom > 0 {
            let mut image = tiny_skia::Pixmap::new(layout.width, bottom)
                .ok_or_else(|| t("The image design could not be prepared").to_string())?;
            if let Some(tree) = lattice {
                resvg::render(&tree, lattice_transform, &mut image.as_mut());
            }
            if let Some(y) = glow_y {
                lp::glow(
                    &mut image.as_mut(),
                    width / 2.0,
                    y,
                    dawn.1 * scale,
                    lp::skia(doc.accent, dawn.0),
                );
            }
            if let Some(tree) = head {
                resvg::render(&tree, transform, &mut image.as_mut());
            }
            Some(image)
        } else {
            None
        };
        Ok(Self {
            top,
            ground: Ground::new(doc, layout)?,
            rim: rim(doc, layout)?,
        })
    }
}

/// Only perimeter pixels are retained, independent of the card's interior area.
fn rim(doc: &Document, layout: &Layout) -> Result<[Option<Raster>; 4], String> {
    let (w, h) = (layout.width, layout.height);
    let radius = (doc.radius * doc.scale)
        .min(w as f32 / 2.0)
        .min(h as f32 / 2.0);
    let corner = (radius + doc.scale / 2.0 + EDGE_ROOM).ceil() as u32;
    let top = corner.min(h);
    let bottom = corner.min(h - top);
    let middle = h - top - bottom;
    let edge = (doc.scale / 2.0 + EDGE_ROOM).ceil() as u32;
    let left = edge.min(w);
    let right = edge.min(w - left);
    let regions = [
        (0, 0, w, top),
        (0, h - bottom, w, bottom),
        (0, top, left, middle),
        (w - right, top, right, middle),
    ];
    let path = lp::rounded_rect(0.0, 0.0, w as f32, h as f32, radius)
        .ok_or_else(|| t("The image design could not be prepared").to_string())?;
    let paint = lp::linear(
        (0.0, 0.0),
        (w as f32, h as f32),
        lp::skia(doc.palette.specular_rim_light, 1.0),
        lp::skia(doc.palette.specular_rim_shadow, 0.8),
    );
    let mut rasters = [None, None, None, None];
    for (slot, (x, y, width, height)) in rasters.iter_mut().zip(regions) {
        if width == 0 || height == 0 {
            continue;
        }
        let mut pixels = tiny_skia::Pixmap::new(width, height)
            .ok_or_else(|| t("The image design could not be prepared").to_string())?;
        pixels.as_mut().stroke_path(
            &path,
            &paint,
            &tiny_skia::Stroke {
                width: doc.scale,
                ..tiny_skia::Stroke::default()
            },
            Transform::from_translate(-(x as f32), -(y as f32)),
            None,
        );
        *slot = Some(Raster {
            pixels,
            origin: (x as i32, y as i32),
        });
    }
    Ok(rasters)
}
