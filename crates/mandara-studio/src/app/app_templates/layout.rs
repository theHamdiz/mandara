//! Shared reading order and spacing for catalog and review controls.

use crate::tokens::SPACE_8_PX;
use gpui::{Div, Styled as _, div, px};
use mandara_ui::bidi::{self, Bidi as _};

pub(super) fn controls_row() -> Div {
    div().row().gap(px(SPACE_8_PX))
}

pub(super) fn paging_icons() -> [&'static str; 2] {
    [
        bidi::dir_icon("chevron-left"),
        bidi::dir_icon("chevron-right"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::FlexDirection;
    use mandara_i18n::{Lang, scoped};

    #[test]
    fn control_layout_and_paging_follow_each_interfaces_reading_direction() {
        for (language, direction, icons) in [
            (
                Lang::En,
                FlexDirection::Row,
                ["chevron-left", "chevron-right"],
            ),
            (
                Lang::Ar,
                FlexDirection::RowReverse,
                ["chevron-right", "chevron-left"],
            ),
            (
                Lang::Fa,
                FlexDirection::RowReverse,
                ["chevron-right", "chevron-left"],
            ),
            (
                Lang::Ur,
                FlexDirection::RowReverse,
                ["chevron-right", "chevron-left"],
            ),
        ] {
            let _language = scoped(Some(language));
            let mut row = controls_row();
            assert_eq!(row.style().flex_direction, Some(direction));
            assert_eq!(paging_icons(), icons);
        }
    }
}
