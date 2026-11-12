//! Studio → Desktop → Context menu: the right-click menu's look and
//! behaviour (`[desktop.menu]`). Its items and their order have their own
//! editor on the tab (`app/desktop_page.rs`).

use mandara_config::desktop_menu::ranges;
use mandara_i18n::tk;

use super::{Control::Choice, Section::Desktop, Setting, s, span};

/// The settings' group on the tab (and in search results).
pub const CONTEXT_MENU: &str = tk("Context menu");

const STYLE: &[(&str, &str)] = &[("ring", tk("Ring")), ("classic", tk("Classic list"))];
const LABELS: &[(&str, &str)] = &[("hub", tk("In the middle")), ("around", tk("Beside the ring")), ("always", tk("Always"))];
const GESTURE: &[(&str, &str)] = &[("both", tk("Click or drag")), ("click", tk("Click")), ("drag", tk("Press, drag and release"))];
const SUBMENU: &[(&str, &str)] = &[("hover", tk("When pointed at")), ("click", tk("When clicked"))];
const ANIMATION: &[(&str, &str)] = &[("follow", tk("Follow Motion")), ("own", tk("Its own speed"))];

pub(super) fn entries() -> Vec<Setting> {
    vec![
        s(Desktop, CONTEXT_MENU, "/desktop/menu/style", tk("Style"), tk("A ring of icons around the pointer, or the classic list"), Choice(STYLE)),
        s(Desktop, CONTEXT_MENU, "/desktop/menu/labels", tk("Labels"), tk("Where the ring names its items"), Choice(LABELS)),
        s(Desktop, CONTEXT_MENU, "/desktop/menu/gesture", tk("Open with"), tk("Hold the right button, move toward an item and let go to pick it in one motion"), Choice(GESTURE)),
        s(Desktop, CONTEXT_MENU, "/desktop/menu/submenu", tk("Add widget opens"), tk("When the outer ring of widgets appears"), Choice(SUBMENU)),
        s(Desktop, CONTEXT_MENU, "/desktop/menu/radius", tk("Ring size"), tk("Distance from the middle to the items; it grows on its own when many items need room"), span(ranges::RADIUS, "px")),
        s(Desktop, CONTEXT_MENU, "/desktop/menu/item_size", tk("Item size"), tk("Size of each item's button"), span(ranges::ITEM_SIZE, "px")),
        s(Desktop, CONTEXT_MENU, "/desktop/menu/opacity", tk("Glass opacity"), tk("Lower it to let the desktop show through the ring"), span(ranges::OPACITY, "")),
        s(Desktop, CONTEXT_MENU, "/desktop/menu/animation", tk("Animation"), tk("Follow Studio → Motion, or give the menu a speed of its own"), Choice(ANIMATION)),
        s(Desktop, CONTEXT_MENU, "/desktop/menu/speed", tk("Animation speed"), tk("Playback speed of the menu; zero opens and closes it at once"), span(ranges::SPEED, "×")),
    ]
}

/// Why a context-menu setting doesn't apply under `cfg` right now.
pub(super) fn locked(path: &str, cfg: &mandara_config::ShellConfig) -> Option<&'static str> {
    use mandara_config::desktop_menu::{MenuAnimation, MenuStyle};
    let m = &cfg.desktop.menu;
    let rest = path.strip_prefix("/desktop/menu/")?;
    match rest {
        "style" => None,
        _ if m.style == MenuStyle::Classic => Some(tk("For the Ring style")),
        "speed" if m.animation == MenuAnimation::Follow => Some(tk("Follows Studio → Motion")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_path_exists_and_ring_settings_lock_for_the_list() {
        let mut cfg = mandara_config::ShellConfig::default();
        let json = serde_json::to_value(&cfg).unwrap();
        for e in entries() {
            assert!(json.pointer(e.path).is_some(), "{} is in the config", e.path);
            assert_eq!(e.group, CONTEXT_MENU);
        }
        assert_eq!(locked("/desktop/menu/radius", &cfg), None);
        assert_eq!(locked("/desktop/menu/speed", &cfg), Some(tk("Follows Studio → Motion")));
        cfg.desktop.menu.animation = mandara_config::desktop_menu::MenuAnimation::Own;
        assert_eq!(locked("/desktop/menu/speed", &cfg), None);
        cfg.desktop.menu.style = mandara_config::desktop_menu::MenuStyle::Classic;
        assert_eq!(locked("/desktop/menu/radius", &cfg), Some(tk("For the Ring style")));
        assert_eq!(locked("/desktop/menu/style", &cfg), None, "the style itself never locks");
        assert_eq!(locked("/bar/height", &cfg), None);
    }
}
