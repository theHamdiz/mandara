//! Keyboard movement within a bounded template review list.

use crate::tokens::SPACE_24_PX;
use gpui::{Keystroke, ScrollHandle, point, px};

fn offset(key: &str, current: f32, maximum: f32, viewport: f32) -> Option<f32> {
    let next = match key {
        "up" => current + SPACE_24_PX,
        "down" => current - SPACE_24_PX,
        "pageup" => current + viewport,
        "pagedown" => current - viewport,
        "home" => 0.0,
        "end" => -maximum,
        _ => return None,
    };
    Some(next.clamp(-maximum, 0.0))
}

pub(super) fn key(handle: &ScrollHandle, key: &Keystroke) -> bool {
    if key.modifiers.control || key.modifiers.alt || key.modifiers.platform {
        return false;
    }
    let current = handle.offset();
    let Some(y) = offset(
        &key.key,
        current.y.into(),
        handle.max_offset().y.into(),
        handle.bounds().size.height.into(),
    ) else {
        return false;
    };
    handle.set_offset(point(current.x, px(y)));
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keyboard_scrolling_reaches_every_part_and_stays_inside_the_list() {
        let (maximum, viewport) = (900.0, 320.0);
        let mut y = 0.0;
        for _ in 0..4 {
            y = offset("pagedown", y, maximum, viewport).unwrap();
        }
        assert_eq!(y, -maximum);
        assert_eq!(offset("down", y, maximum, viewport), Some(y));
        assert_eq!(offset("up", y, maximum, viewport), Some(y + SPACE_24_PX));
        assert_eq!(offset("home", y, maximum, viewport), Some(0.0));
        assert_eq!(offset("end", 0.0, maximum, viewport), Some(-maximum));
        for _ in 0..4 {
            y = offset("pageup", y, maximum, viewport).unwrap();
        }
        assert_eq!(y, 0.0);
        assert_eq!(offset("up", y, maximum, viewport), Some(0.0));
        assert_eq!(offset("escape", y, maximum, viewport), None);
        assert_eq!(offset("pagedown", 0.0, 0.0, 0.0), Some(0.0));
    }
}
