//! Shared widget effects for animated raster content, with retained scratch.

use mandara_config::desktop_widgets::Effect;
use mandara_theme::Color;
use tiny_skia::{Pixmap, PixmapMut, PixmapPaint, Transform};

use super::{
    BLUR_PASSES, EFFECT_BLUR_PX, GLOW_ALPHA, SHADOW_ALPHA, SHADOW_DROP_PX, Style, box_blur_with,
};

#[derive(Default)]
pub struct RasterEffects {
    source: Option<Pixmap>,
    mask: Option<Pixmap>,
    line: Vec<[u32; 4]>,
}

impl RasterEffects {
    pub(in crate::backdrop::widgets) fn cached_bytes(&self) -> usize {
        self.source.as_ref().map_or(0, |image| image.data().len())
            + self.mask.as_ref().map_or(0, |image| image.data().len())
            + self.line.capacity() * std::mem::size_of::<[u32; 4]>()
    }
    pub fn paint(
        &mut self,
        target: &mut PixmapMut<'_>,
        style: &Style,
        paint: impl FnOnce(&mut PixmapMut<'_>),
    ) {
        if style.effect == Effect::None && style.drop.is_none() {
            self.source = None;
            self.mask = None;
            self.line = Vec::new();
            paint(target);
            return;
        }
        let size = (target.width(), target.height());
        if self
            .source
            .as_ref()
            .is_none_or(|image| (image.width(), image.height()) != size)
        {
            self.source = Pixmap::new(size.0, size.1);
            self.mask = Pixmap::new(size.0, size.1);
        }
        let (Some(source), Some(mask)) = (&mut self.source, &mut self.mask) else {
            return;
        };
        source.fill(tiny_skia::Color::TRANSPARENT);
        paint(&mut source.as_mut());
        let drop = style.drop.map(|drop| {
            (
                style.inks.drop,
                drop.opacity,
                drop.offset,
                drop.blur / BLUR_PASSES as f32,
            )
        });
        let effect = match style.effect {
            Effect::None => None,
            Effect::Shadow => Some((
                style.inks.shadow,
                SHADOW_ALPHA * style.strength,
                (0.0, SHADOW_DROP_PX * style.k),
                EFFECT_BLUR_PX * style.k,
            )),
            Effect::Glow => Some((
                style.inks.accent,
                GLOW_ALPHA * style.strength,
                (0.0, 0.0),
                EFFECT_BLUR_PX * style.k,
            )),
        };
        for (color, opacity, offset, blur) in [drop, effect].into_iter().flatten() {
            tint(mask, source, color, opacity.clamp(0.0, 1.0));
            if blur > 0.0 {
                box_blur_with(
                    &mut mask.as_mut(),
                    blur.round().max(1.0) as usize,
                    &mut self.line,
                );
            }
            target.draw_pixmap(
                0,
                0,
                mask.as_ref(),
                &PixmapPaint::default(),
                Transform::from_translate(offset.0, offset.1),
                None,
            );
        }
        target.draw_pixmap(
            0,
            0,
            source.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }
}

fn tint(target: &mut Pixmap, source: &Pixmap, color: Color, opacity: f32) {
    let color = color.to_rgba8();
    for (input, output) in source
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .zip(target.data_mut().as_chunks_mut::<4>().0.iter_mut())
    {
        let alpha = (f32::from(input[3]) * opacity * f32::from(color[3]) / 255.0).round() as u8;
        for channel in 0..3 {
            output[channel] = (u16::from(color[channel]) * u16::from(alpha) / 255) as u8;
        }
        output[3] = alpha;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tinted_masks_preserve_premultiplied_alpha_and_reuse_storage() {
        let mut source = Pixmap::new(10, 10).unwrap();
        source.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 128));
        let mut target = Pixmap::new(10, 10).unwrap();
        tint(&mut target, &source, Color::hex("#ff8000"), 0.5);
        assert_eq!(&target.data()[..4], &[64, 32, 0, 64]);
        let mut line = Vec::new();
        box_blur_with(&mut target.as_mut(), 2, &mut line);
        let allocation = line.as_ptr();
        box_blur_with(&mut target.as_mut(), 2, &mut line);
        assert_eq!(line.as_ptr(), allocation);
        assert!(
            target
                .data()
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[..3].iter().all(|channel| *channel <= pixel[3]))
        );
    }
}
