//! Software counterparts of the login presets. Authentication and password
//! painting stay in the common locker; only composition and trim differ.

use super::{
    L, LockScreen,
    paint::{self, Font},
};
use mandara_config::desktop::LockDesign;
use mandara_core::session_lock::SurfaceInfo;
use std::time::Instant;
use tiny_skia::PixmapMut;

/// Below 3:2 the paired compositions stack to keep credentials readable.
const SPLIT_ASPECT: f32 = 1.5;
/// Shared inset and panel height at the reference resolution.
const INSET: f32 = 120.0;
const PANEL_HEIGHT: f32 = 270.0;
const PANEL_PAD: f32 = 32.0;
/// The editorial baseline, matching Meridian's login screen.
const HORIZON: f32 = 0.618;
/// Hairline grid pitch and opacity; decoration is static, with no timer.
const GRID: f32 = 120.0;
const GRID_ALPHA: f32 = 0.028;
/// The halo diameter relative to the login clock size.
const HALO_RATIO: f32 = 4.0;
/// Thin text used below the preset clocks, in reference pixels.
const DATE_SIZE: f32 = 15.0;

#[derive(Copy, Clone)]
pub(super) struct Layout {
    pub cx: f32,
    pub top: f32,
    pub name_x: f32,
    pub field_y: f32,
    pub field_width: f32,
}

pub(super) fn layout(
    design: LockDesign,
    w: f32,
    h: f32,
    unit: f32,
    rtl: bool,
    avatar: bool,
) -> Option<Layout> {
    let tokens = design.tokens()?;
    let wide = w >= h * SPLIT_ASPECT;
    let side = if rtl { -1.0 } else { 1.0 };
    let half = tokens.card_width * unit / 2.0;
    let offset = (w / 2.0 - INSET * unit - half).max(0.0);
    let cx = w / 2.0
        + match design {
            LockDesign::Ghostline if wide => -offset * side,
            LockDesign::ChromeHalo if wide => offset * side,
            _ => 0.0,
        };
    let avatar_size = if avatar { tokens.avatar } else { 0.0 };
    let top = h / 2.0 - avatar_size * unit / 2.0;
    let field_width = ((tokens.card_width - PANEL_PAD * 2.0) * unit)
        .min(w - PANEL_PAD * unit * 2.0)
        .max(unit);
    if design == LockDesign::Meridian {
        let baseline = h * HORIZON;
        let field_width = field_width.min(w / 2.0);
        Some(Layout {
            cx: if wide {
                w / 2.0 + offset * side
            } else {
                w / 2.0
            },
            name_x: if wide {
                w / 2.0 - offset * side
            } else {
                w / 2.0
            },
            top: baseline - (if wide { tokens.name } else { tokens.name * 2.0 }) * unit,
            field_y: baseline - tokens.field_height * unit,
            field_width,
        })
    } else {
        Some(Layout {
            cx,
            top,
            name_x: cx,
            field_y: h / 2.0 + (INSET - tokens.avatar + avatar_size) * unit,
            field_width,
        })
    }
}

impl LockScreen {
    pub(super) fn draw_design(
        &mut self,
        info: &SurfaceInfo,
        pm: &mut PixmapMut<'_>,
        now: Instant,
        (lift, alpha, presence): (f32, f32, f32),
    ) {
        let design = self.look.cfg.design;
        let tokens = design.tokens().expect("preset design");
        let p = self.palette.clone();
        let (w, h) = (pm.width() as f32, pm.height() as f32);
        let unit = L::screen_scale(info.width as f32, info.height as f32) * info.scale;
        let avatar = self.look.cfg.show_avatar && design != LockDesign::Meridian;
        let layout = layout(design, w, h, unit, self.rtl, avatar).expect("preset layout");
        let wide = w >= h * SPLIT_ASPECT;
        let clock_x = if wide && matches!(design, LockDesign::Ghostline | LockDesign::ChromeHalo) {
            w - layout.cx
        } else {
            w / 2.0
        };
        let clock_y = if design == LockDesign::ChromeHalo && !wide {
            tokens.clock * unit * HALO_RATIO / 2.0
        } else if wide && matches!(design, LockDesign::Ghostline | LockDesign::ChromeHalo) {
            h / 2.0 - tokens.clock * unit / 2.0
        } else {
            L::CLOCK_TOP * unit
        };
        let font = Font {
            family: &self.fonts.display,
            size: tokens.clock * unit,
            weight: tokens.clock_weight,
            tabular: true,
            tracking: -0.01,
        };
        let (time, ampm) = self.time_strings();
        let time = if let Some(ampm) = ampm {
            format!("{time} {ampm}")
        } else {
            time
        };
        let (_, line_h) = self.text.measure(&time, &font);
        let stacked = design == LockDesign::Ghostline && wide && !self.look.custom_time;
        let clock_y = if stacked {
            clock_y - line_h / 2.0
        } else {
            clock_y
        };
        let clock_height = if stacked { line_h * 2.0 } else { line_h };
        match design {
            LockDesign::Ghostline => {
                let mut x = GRID * unit;
                while x < w {
                    paint::fill(
                        pm,
                        &paint::rounded_rect(x, 0.0, unit, h, 0.0),
                        &paint::solid(p.on_surface, GRID_ALPHA * alpha),
                    );
                    x += GRID * unit;
                }
                let mut y = GRID * unit;
                while y < h {
                    paint::fill(
                        pm,
                        &paint::rounded_rect(0.0, y, w, unit, 0.0),
                        &paint::solid(p.on_surface, GRID_ALPHA * alpha),
                    );
                    y += GRID * unit;
                }
                let ruler_x = clock_x + tokens.clock * unit;
                for tick in 0..=24 {
                    let y = clock_y + tick as f32 * clock_height / 24.0;
                    let width = if tick % 6 == 0 {
                        PANEL_PAD
                    } else {
                        PANEL_PAD / 2.0
                    } * unit;
                    paint::fill(
                        pm,
                        &paint::rounded_rect(ruler_x, y, width, unit, 0.0),
                        &paint::solid(p.primary, alpha * 0.5),
                    );
                }
            }
            LockDesign::ChromeHalo => {
                let radius = tokens.clock * unit * HALO_RATIO / 2.0;
                let cy = clock_y + line_h / 2.0;
                let ring = paint::circle(clock_x, cy, radius);
                let rim = paint::linear(
                    (clock_x - radius, cy - radius),
                    (clock_x + radius, cy + radius),
                    paint::skia(p.primary, alpha),
                    paint::skia(p.tertiary, alpha),
                );
                paint::stroke(pm, &ring, &rim, unit);
                paint::stroke(
                    pm,
                    &paint::circle(clock_x, cy, radius + PANEL_PAD * unit / 2.0),
                    &paint::solid(p.secondary, alpha * 0.25),
                    unit,
                );
            }
            LockDesign::Meridian => {
                paint::fill(
                    pm,
                    &paint::rounded_rect(0.0, h * HORIZON, w, unit, 0.0),
                    &paint::solid(p.on_surface, alpha * 0.24),
                );
            }
            _ => {}
        }
        if design != LockDesign::Meridian {
            if stacked && let Some((hours, minutes)) = time.split_once(':') {
                self.text.draw_centered(
                    pm,
                    hours,
                    &font,
                    clock_x,
                    clock_y + lift,
                    p.on_surface,
                    alpha,
                );
                self.text.draw_centered(
                    pm,
                    minutes,
                    &font,
                    clock_x,
                    clock_y + line_h + lift,
                    p.primary,
                    alpha,
                );
            } else {
                self.text.draw_centered(
                    pm,
                    &time,
                    &font,
                    clock_x,
                    clock_y + lift,
                    p.on_surface,
                    alpha,
                );
            }
            self.draw_date(
                pm,
                clock_x,
                clock_y + clock_height + DATE_SIZE * unit + lift,
                alpha,
                unit,
            );
            self.draw_info(
                pm,
                clock_x,
                clock_y + clock_height + DATE_SIZE * unit * 3.0 + lift,
                alpha,
                unit,
            );
            if info.focused {
                let pw = tokens.card_width * unit;
                let top = layout.top - PANEL_PAD * unit + lift * 0.6;
                let ph = (PANEL_HEIGHT - if avatar { 0.0 } else { tokens.avatar }) * unit;
                let rect = paint::rounded_rect(
                    layout.cx - pw / 2.0,
                    top,
                    pw,
                    ph,
                    tokens.card_radius * unit,
                );
                paint::fill(pm, &rect, &paint::solid(p.surface_container, alpha * 0.96));
                paint::stroke(pm, &rect, &paint::solid(p.on_surface, alpha * 0.18), unit);
                if design == LockDesign::ChromeHalo {
                    let pad = PANEL_PAD * unit / 4.0;
                    paint::stroke(
                        pm,
                        &paint::rounded_rect(
                            layout.cx - pw / 2.0 - pad,
                            top - pad,
                            pw + pad * 2.0,
                            ph + pad * 2.0,
                            tokens.card_radius * unit + pad,
                        ),
                        &paint::solid(p.primary, alpha * 0.25),
                        unit,
                    );
                }
            }
        } else {
            let vast = Font {
                size: tokens.clock * unit * HALO_RATIO,
                ..font
            };
            self.text.draw_centered(
                pm,
                &time,
                &vast,
                w / 2.0,
                h * HORIZON,
                p.on_surface,
                alpha * 0.075,
            );
            self.draw_date(pm, w / 2.0, PANEL_PAD * unit, alpha, unit);
            self.draw_info(
                pm,
                w / 2.0,
                (PANEL_PAD + DATE_SIZE * 2.0) * unit,
                alpha,
                unit,
            );
        }
        if info.focused {
            self.draw_lower(
                info,
                pm,
                now,
                (w, h, layout.cx, lift, alpha, presence, info.scale, unit),
            );
        } else if self.demo {
            paint::fade(pm, presence);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hiding_the_avatar_closes_its_space_above_the_password() {
        for design in [
            LockDesign::MidnightCard,
            LockDesign::Ghostline,
            LockDesign::ChromeHalo,
        ] {
            let shown = layout(design, 1920.0, 1080.0, 1.0, false, true).unwrap();
            let hidden = layout(design, 1920.0, 1080.0, 1.0, false, false).unwrap();
            assert!(hidden.field_y < shown.field_y);
            assert_eq!(shown.cx, hidden.cx);
            assert_eq!(shown.field_width, hidden.field_width);
        }
    }

    #[test]
    fn credentials_fit_landscape_portrait_and_rtl() {
        for design in LockDesign::ALL {
            for (w, h) in [(1920.0, 1080.0), (800.0, 1280.0), (640.0, 480.0)] {
                let unit = L::screen_scale(w, h);
                if let Some(left) = layout(design, w, h, unit, false, true) {
                    let right = layout(design, w, h, unit, true, true).unwrap();
                    assert!((left.cx + right.cx - w).abs() < 0.01);
                    assert!(left.cx - left.field_width / 2.0 >= 0.0);
                    assert!(left.cx + left.field_width / 2.0 <= w);
                    assert!(left.top >= 0.0 && left.field_y >= 0.0);
                    if design == LockDesign::Meridian && w >= h * SPLIT_ASPECT {
                        assert!((left.name_x - left.cx).abs() > left.field_width);
                    } else {
                        assert!(left.field_y > left.top);
                    }
                    assert!(left.field_y + design.tokens().unwrap().field_height * unit < h);
                }
            }
        }
    }
}
