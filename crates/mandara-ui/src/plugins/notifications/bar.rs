//! The Notifications bar item (`[plugins.settings.notifications]
//! bar_style`): the bell with its unread badge, or a chip with the bell,
//! the latest notification (app and summary, ellipsized at `bar_width`)
//! and a count: the unread ones in the accent, otherwise how many are kept.
//!
//! Event-driven only: it redraws when the notification list changes, and
//! its text is re-made only when the latest notification's app or summary
//! does.

use std::cell::RefCell;
use std::sync::Arc;

use gpui::{AnyElement, App, FontWeight, IntoElement, ParentElement, Styled, div, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_config::bar_display::{BarDisplay, NOTIFICATIONS_BAR_WIDTH, NOTIFICATIONS_BAR_WIDTH_RANGE, NotificationsBar};
use mandara_i18n::Lang;
use mandara_services::{Notification, NotificationsState};
use mandara_theme::MandaraPalette;

use super::super::bar_chip::{self, GLYPH_GAP_PX, TextSlot};
use super::super::panel_fx::{WASH_STRONG, wash};
use super::super::settings_cache::{ConfigSettings, SettingsCache};
use crate::bar::badge::{self, Badge};
use crate::bidi::Bidi as _;
use crate::icons::icon;

/// The app name's share of the chip's text room (the summary gets the
/// rest and ellipsizes first).
const APP_SHARE: f32 = 0.4;
/// Gap between the app name and the summary.
const APP_GAP_PX: f32 = 4.0;

#[derive(Copy, Clone, Debug, PartialEq)]
struct BarLook {
    style: NotificationsBar,
    width: f32,
}

impl ConfigSettings for BarLook {
    type Scope = ();

    fn resolve(config: &ShellConfig, _: Lang, _: ()) -> Self {
        let p = &config.plugins;
        let w = p.setting(crate::plugins::notifications::ID, "bar_width", NOTIFICATIONS_BAR_WIDTH).clamp(NOTIFICATIONS_BAR_WIDTH_RANGE.0, NOTIFICATIONS_BAR_WIDTH_RANGE.1);
        Self { style: NotificationsBar::parse(p.setting_str(crate::plugins::notifications::ID, "bar_style")), width: w as f32 }
    }
}

thread_local! {
    static LOOK: RefCell<SettingsCache<BarLook>> = const { RefCell::new(SettingsCache::new()) };
}

fn look_in(config: &Arc<ShellConfig>) -> BarLook {
    *LOOK.with(|l| l.borrow_mut().get(config, mandara_i18n::language(), ()))
}

/// The latest notification's text, kept between renders.
#[derive(Default)]
pub(super) struct Latest {
    app: TextSlot,
    summary: TextSlot,
}

/// Its first line: the summary, or the body when the summary is empty.
fn headline(n: &Notification) -> &str {
    let text = if n.summary.trim().is_empty() { &n.body } else { &n.summary };
    text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default()
}

/// The count the chip shows: the unread ones (accented), else how many
/// are kept (`(count, unread)`).
fn count(st: &NotificationsState) -> (usize, bool) {
    match st.unread() {
        0 => (st.items.len(), false),
        n => (n, true),
    }
}

/// The bell (or the moon while quiet), tinted while something is unread.
fn bell(st: &NotificationsState, unread: usize, size: f32, palette: &MandaraPalette) -> impl IntoElement {
    icon(if st.quiet() { "moon" } else { "bell" }, size, if unread > 0 { palette.primary } else { palette.on_surface })
}

pub(super) fn render(latest: &RefCell<Latest>, palette: &MandaraPalette, cx: &App) -> AnyElement {
    let hub = crate::hub::hub(cx);
    let st = &hub.read(cx).notifications;
    let unread = st.unread();
    let size = crate::style::bar_icon(super::BAR_ICON_PX);
    let look = look_in(&crate::state::config(cx).read(cx).snapshot());
    // A side bar has no room for text: the classic bell there.
    if look.style == NotificationsBar::Icon || crate::bar::vertical(cx) {
        return badge::counted(bell(st, unread, size, palette), size, unread, Badge { fill: palette.primary, ink: palette.on_primary, glow: None }, super::ID, palette, cx);
    }
    let Some(newest) = st.items.first() else {
        return bell(st, unread, size, palette).into_any_element();
    };
    let (n, accented) = count(st);
    let (app, summary) = {
        let mut l = latest.borrow_mut();
        (l.app.get(newest.app_name.trim()), l.summary.get(headline(newest)))
    };
    let ink = if accented { palette.on_surface } else { palette.on_surface_variant };
    let badge = if accented { Badge { fill: palette.primary, ink: palette.on_primary, glow: None } } else { Badge { fill: wash(WASH_STRONG, palette), ink: palette.on_surface_variant, glow: None } };
    bar_chip::text_row(GLYPH_GAP_PX)
        .child(bell(st, unread, size, palette))
        .child(
            div()
                .row()
                .items_center()
                .min_w(px(0.0))
                .max_w(px(look.width))
                .gap(px(APP_GAP_PX))
                .when(!app.is_empty(), |el| el.child(bar_chip::label(app, look.width * APP_SHARE, palette.on_surface_variant).flex_shrink_0().font_weight(FontWeight::MEDIUM)))
                .child(bar_chip::label(summary, look.width, ink)),
        )
        .children(badge::inline(n, badge))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: u32, summary: &str, body: &str, read: bool) -> Notification {
        Notification { id, app_name: "Files".into(), summary: summary.into(), body: body.into(), read, ..Default::default() }
    }

    #[test]
    fn the_headline_is_the_first_line_with_text() {
        assert_eq!(headline(&note(1, "Copied\nthree files", "", false)), "Copied");
        assert_eq!(headline(&note(1, "  ", "\n Done ", false)), "Done");
        assert_eq!(headline(&note(1, "", "", false)), "");
    }

    #[test]
    fn the_count_is_unread_first_then_everything_kept() {
        let mut st = NotificationsState::default();
        assert_eq!(count(&st), (0, false));
        st.items = vec![note(2, "b", "", true), note(1, "a", "", true)];
        assert_eq!(count(&st), (2, false));
        st.items[0].read = false;
        assert_eq!(count(&st), (1, true));
    }

    #[test]
    fn the_default_is_the_classic_bell_and_widths_are_clamped() {
        let l = look_in(&Arc::new(ShellConfig::default()));
        assert_eq!(l, BarLook { style: NotificationsBar::Icon, width: NOTIFICATIONS_BAR_WIDTH as f32 });
        let mut c = ShellConfig::default();
        c.plugins.settings.insert(crate::plugins::notifications::ID.into(), toml::from_str("bar_style = \"latest\"\nbar_width = 9999").unwrap());
        let l = look_in(&Arc::new(c));
        assert_eq!(l, BarLook { style: NotificationsBar::Latest, width: NOTIFICATIONS_BAR_WIDTH_RANGE.1 as f32 });
    }

    #[test]
    fn unchanged_text_is_not_made_again() {
        let mut latest = Latest::default();
        // Long enough to live on the heap (short ones are stored inline).
        let n = note(1, "Download finished: holiday-photos-2026-full-resolution.tar", "", false);
        let a = latest.summary.get(headline(&n));
        let b = latest.summary.get(headline(&n));
        assert!(std::ptr::eq(a.as_ref().as_ptr(), b.as_ref().as_ptr()));
    }
}
