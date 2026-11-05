//! Mandara patch: preserve fractional geometry for SDF curves.

use super::{PaintQuad, Window};

impl Window {
    /// Paint an SDF quad without moving its bounds onto the device-pixel grid.
    ///
    /// Use this for dots along a curve: snapping each center independently makes
    /// the curve uneven. Layout backgrounds keep using `paint_quad`. Opacity,
    /// content clipping and stroke-width snapping follow the normal quad painter.
    /// Like `paint_quad`, this is only valid during the paint phase.
    pub fn paint_subpixel_quad(&mut self, quad: PaintQuad) {
        let bounds = quad.bounds.scale(self.scale_factor());
        self.paint_quad_with_bounds(quad, bounds);
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        AppContext as _, Bounds, Context, Corners, IntoElement, ParentElement as _, Quad, Render,
        ScaledPixels, Styled as _, TestAppContext, Window, canvas, div, fill, point, px, red, size,
    };
    use std::{cell::RefCell, rc::Rc};

    struct Probe {
        quads: Rc<RefCell<Vec<(Quad, Quad)>>>,
    }

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let recorded = self.quads.clone();
            div()
                .size(px(64.))
                .rounded(px(8.))
                .overflow_hidden()
                .opacity(0.5)
                .child(
                    canvas(
                        |_, _, _| {},
                        move |_, _, window, _| {
                            let bounds = Bounds {
                                origin: point(px(10.25), px(20.75)),
                                size: size(px(12.), px(12.)),
                            };
                            window.paint_quad(fill(bounds, red()).corner_radii(px(6.)));
                            let snapped = *window.next_frame.scene.quads.last().unwrap();
                            window.paint_subpixel_quad(fill(bounds, red()).corner_radii(px(6.)));
                            let precise = *window.next_frame.scene.quads.last().unwrap();
                            recorded.borrow_mut().push((snapped, precise));
                        },
                    )
                    .size_full(),
                )
        }
    }

    #[crate::test]
    fn fractional_sdf_bounds_keep_the_legacy_clip_and_opacity(cx: &mut TestAppContext) {
        for scale in [1., 1.25, 1.5, 2.] {
            let recorded = Rc::new(RefCell::new(Vec::new()));
            let capture = recorded.clone();
            let window = cx.add_window(move |_, _| Probe { quads: capture });
            cx.simulate_window_scale_factor_change(window.into(), scale);
            cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
                .unwrap();
            let &(snapped, precise) = recorded.borrow().last().unwrap();
            let logical = Bounds {
                origin: point(px(10.25), px(20.75)),
                size: size(px(12.), px(12.)),
            };
            assert_eq!(precise.bounds, logical.scale(scale));
            assert_ne!(precise.bounds, snapped.bounds);
            assert_eq!(precise.content_mask, snapped.content_mask);
            assert!(precise.content_mask.is_rounded());
            assert_eq!(precise.corner_radii, Corners::all(ScaledPixels(6. * scale)));
            assert_eq!(precise.corner_radii, snapped.corner_radii);
            assert_eq!(precise.border_widths, snapped.border_widths);
            assert_eq!(precise.background, snapped.background);
            assert_eq!(precise.border_color, snapped.border_color);
        }
    }
}
