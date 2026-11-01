//! The proportions Mandara Login and the lock screen share, so both look
//! like one design: logical px on a 1080 px tall screen, scaled by the
//! screen's height (`height / REFERENCE_H`, clamped to [`SCALE_RANGE`]).
//! Mandara Login's default layout and sizes, and the lock screen's layout,
//! both read these.

/// The screen height the values below are designed for.
pub const REFERENCE_H: f32 = 1080.0;
/// Smallest and largest scale applied to them.
pub const SCALE_RANGE: (f32, f32) = (0.6, 2.0);

/// Date line, top edge from the top of the screen.
pub const DATE_TOP: f32 = 78.0;
pub const DATE_SIZE: f32 = 24.0;
pub const DATE_WEIGHT: u16 = 600;
/// Clock, top edge from the top of the screen.
pub const CLOCK_TOP: f32 = 108.0;
pub const CLOCK_SIZE: f32 = 150.0;
pub const CLOCK_WEIGHT: u16 = 700;

/// The user card (avatar over name), centred this far below the middle.
pub const CARD_CENTER: f32 = 40.0;
pub const AVATAR: f32 = 108.0;
/// Between the avatar and the name.
pub const NAME_GAP: f32 = 12.0;
pub const NAME_SIZE: f32 = 20.0;
/// Height the name line takes in the card.
pub const NAME_LINE: f32 = 26.0;
/// The password field, centred this far below the middle.
pub const FIELD_CENTER: f32 = 166.0;
pub const FIELD_W: f32 = 340.0;
pub const FIELD_H: f32 = 48.0;
/// The user list (several accounts), centred this far below the middle.
pub const USERS_CENTER: f32 = 262.0;

/// The scale for a screen `height` logical px tall.
pub fn scale(height: f32) -> f32 {
    (height / REFERENCE_H).clamp(SCALE_RANGE.0, SCALE_RANGE.1)
}

/// Height of the user card (avatar, gap, name).
pub fn card_height(avatar: bool) -> f32 {
    if avatar { AVATAR + NAME_GAP + NAME_LINE } else { NAME_LINE }
}

/// Centre of the avatar below the middle of the screen (unscaled).
pub fn avatar_center() -> f32 {
    CARD_CENTER - card_height(true) / 2.0 + AVATAR / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_clears_the_clock_and_the_field_clears_the_card() {
        let clock_bottom = CLOCK_TOP + CLOCK_SIZE * 1.1;
        let card_top = REFERENCE_H / 2.0 + CARD_CENTER - card_height(true) / 2.0;
        let card_bottom = card_top + card_height(true);
        let field_top = REFERENCE_H / 2.0 + FIELD_CENTER - FIELD_H / 2.0;
        assert!(card_top > clock_bottom + 120.0, "room between the clock and the card");
        assert!(field_top >= card_bottom + 12.0, "field below the name");
        assert!((scale(864.0) - 0.8).abs() < 1e-6 && scale(100.0) == SCALE_RANGE.0);
    }
}
