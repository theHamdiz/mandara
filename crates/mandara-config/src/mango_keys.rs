//! How MangoWM decides two key binds are the same key, so Mandara never
//! writes a bind MangoWM reports as a conflict.
//!
//! MangoWM (docs/bindings/keys.md, src/config/binding_parse.c and
//! conflict_check.c, 0.18.4): a plain `bind` resolves its key name to a
//! keycode, looking through the layouts set with `xkb_rules_layout` in
//! order and falling back to the `us` layout; `binds` (the `s` flag) keeps
//! the keysym. Two binds conflict when their modifiers match and they land
//! on the same keycode (or keysym), in the same `key_mode` or when either
//! is in `common`. So `XF86AudioPause` and `XF86AudioPlay`, which share one
//! key, conflict.
//!
//! Each key name is mapped to a canonical name: the first keysym of the
//! keycode it resolves to. Binds on one physical key then compare equal,
//! and the chord stays a real keysym name for Studio and takeovers.

use std::cell::RefCell;
use std::rc::Rc;

use xkbcommon::xkb;

/// The layout MangoWM falls back to (its `xkb_fallback_rules`).
const FALLBACK_LAYOUT: &str = "us";

/// Keyboard-name and keycode checks need libxkbcommon even without a GUI.
pub fn is_available() -> bool {
    xkb::is_available()
}
/// One spelling per keysym, lowercased: every compositor matches binds by
/// keysym (MangoWM's `binds`, Niri, Hyprland), and xkb names some keysyms
/// twice (`Prior` and `Page_Up`), so the names must be compared as keysyms.
/// Anything that isn't a keysym name (`mouse:272`, `WheelScrollDown`,
/// `code:24`) stays as written.
pub fn canonical_keysym(name: &str) -> String {
    let name = name.trim();
    let sym = xkb::keysym_from_name(name, xkb::KEYSYM_CASE_INSENSITIVE);
    if sym.raw() == xkb::keysyms::KEY_NoSymbol { name.to_ascii_lowercase() } else { xkb::keysym_get_name(sym).to_ascii_lowercase() }
}

/// The prefix of a raw keycode key (`code:24`).
const CODE_PREFIX: &str = "code:";
/// MangoWM's default key mode, and the one whose binds apply in every mode.
pub const DEFAULT_MODE: &str = "default";
pub const COMMON_MODE: &str = "common";
/// The config key that switches the key mode for the binds below it.
pub const KEY_MODE_KEY: &str = "key_mode";
/// The bind flag that keeps the keysym instead of resolving a keycode.
const KEYSYM_FLAG: char = 's';
/// Bind flags MangoWM knows (`s`, `l`, `r`, `p`, `c`).
const BIND_FLAGS: &str = "slrpc";

/// MangoWM's `xkb_rules_*` options.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct XkbRules {
    pub rules: String,
    pub model: String,
    pub layout: String,
    pub variant: String,
    pub options: String,
}

impl XkbRules {
    /// Reads one `key=value` config line; true when it was an xkb option.
    pub fn take_line(&mut self, key: &str, value: &str) -> bool {
        let slot = match key {
            "xkb_rules_rules" => &mut self.rules,
            "xkb_rules_model" => &mut self.model,
            "xkb_rules_layout" => &mut self.layout,
            "xkb_rules_variant" => &mut self.variant,
            "xkb_rules_options" => &mut self.options,
            _ => return false,
        };
        *slot = value.trim().to_string();
        true
    }
}

/// Resolves MangoWM key names to the key they bind.
pub struct MangoKeys {
    configured: Option<xkb::Keymap>,
    fallback: Option<xkb::Keymap>,
}

impl MangoKeys {
    pub fn new(rules: &XkbRules) -> Self {
        let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let options = (!rules.options.is_empty()).then(|| rules.options.clone());
        let configured = xkb::Keymap::new_from_names(&ctx, &rules.rules, &rules.model, &rules.layout, &rules.variant, options, xkb::KEYMAP_COMPILE_NO_FLAGS);
        let fallback = xkb::Keymap::new_from_names(&ctx, "", "", FALLBACK_LAYOUT, "", None, xkb::KEYMAP_COMPILE_NO_FLAGS);
        Self { configured, fallback }
    }

    /// The `us` layout alone, MangoWM's own default.
    pub fn us() -> Self {
        Self::new(&XkbRules::default())
    }

    /// The canonical name of `key` as a plain `bind` (or, with
    /// `keysym_only`, a `binds`) sees it: lowercased, and for keycode binds
    /// the first keysym of the key it lands on.
    pub fn canonical(&self, key: &str, keysym_only: bool) -> String {
        let key = key.trim();
        let lower = key.to_ascii_lowercase();
        if let Some(code) = lower.strip_prefix(CODE_PREFIX) {
            return code.trim().parse::<u32>().ok().and_then(|c| self.name_of(xkb::Keycode::new(c))).unwrap_or(lower);
        }
        if keysym_only {
            return canonical_keysym(key);
        }
        let sym = xkb::keysym_from_name(key, xkb::KEYSYM_CASE_INSENSITIVE);
        if sym.raw() == xkb::keysyms::KEY_NoSymbol {
            return lower;
        }
        self.keycode_for(sym).and_then(|code| self.name_of(code)).unwrap_or(lower)
    }

    /// The first keycode producing `sym`: the configured layouts in order,
    /// then `us` (MangoWM's `find_keycodes_for_keysym`).
    fn keycode_for(&self, sym: xkb::Keysym) -> Option<xkb::Keycode> {
        [self.configured.as_ref(), self.fallback.as_ref()].into_iter().flatten().find_map(|map| (0..map.num_layouts()).find_map(|layout| first_keycode(map, layout, sym)))
    }

    /// The keysym a keycode gives on the first layout's first level.
    fn name_of(&self, code: xkb::Keycode) -> Option<String> {
        let map = self.configured.as_ref().or(self.fallback.as_ref())?;
        let sym = map.key_get_syms_by_level(code, 0, 0).first().copied()?;
        Some(xkb::keysym_get_name(sym).to_ascii_lowercase())
    }
}

fn first_keycode(map: &xkb::Keymap, layout: xkb::LayoutIndex, sym: xkb::Keysym) -> Option<xkb::Keycode> {
    (map.min_keycode().raw()..=map.max_keycode().raw()).map(xkb::Keycode::new).find(|&code| (0..map.num_levels_for_key(code, layout)).any(|level| map.key_get_syms_by_level(code, layout, level).contains(&sym)))
}

/// Runs `f` with the resolver for the user's own MangoWM layouts. The
/// options are re-read each time (they're a few small files) and the
/// keymaps are compiled again only when they changed; keymaps aren't
/// thread-safe, so each thread keeps its own.
pub fn with_user_keys<R>(f: impl FnOnce(&MangoKeys) -> R) -> R {
    thread_local! {
        static KEYS: RefCell<Option<(XkbRules, Rc<MangoKeys>)>> = const { RefCell::new(None) };
    }
    let rules = user_rules();
    let keys = KEYS.with_borrow_mut(|cache| match cache {
        Some((cached, keys)) if *cached == rules => keys.clone(),
        _ => {
            let keys = Rc::new(MangoKeys::new(&rules));
            *cache = Some((rules, keys.clone()));
            keys
        }
    });
    f(&keys)
}

/// The `xkb_rules_*` options in the user's MangoWM config tree (the last
/// value wins, as when MangoWM reads it).
fn user_rules() -> XkbRules {
    let mut rules = XkbRules::default();
    for file in crate::conflicts::config_files(crate::compositor::Target::Mango) {
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        for line in text.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
            if let Some((key, value)) = line.split_once('=') {
                rules.take_line(key.trim(), value);
            }
        }
    }
    rules
}

/// A `bind…=` line's flags (`bindl` → `l`), or `None` when the line isn't a
/// key bind (`mousebind`, `axisbind`, an unknown flag…).
pub fn bind_flags(key: &str) -> Option<&str> {
    key.strip_prefix("bind").filter(|f| f.chars().all(|c| BIND_FLAGS.contains(c)))
}

/// Whether a bind's flags keep the keysym (`s`).
pub fn keysym_only(flags: &str) -> bool {
    flags.contains(KEYSYM_FLAG)
}

/// The modifier a `code:N` modifier stands for (MangoWM's `parse_mod`):
/// both Supers, Ctrls, Shifts and Alts.
const MODIFIER_CODES: &[(u32, &str)] = &[(133, "super"), (134, "super"), (37, "ctrl"), (105, "ctrl"), (50, "shift"), (62, "shift"), (64, "alt"), (108, "alt")];

/// `code:64` → `alt`, for modifiers written as keycodes.
pub fn modifier_code(token: &str) -> Option<&'static str> {
    let code: u32 = token.strip_prefix(CODE_PREFIX)?.trim().parse().ok()?;
    MODIFIER_CODES.iter().find(|(c, _)| *c == code).map(|&(_, m)| m)
}

/// Whether binds in key modes `a` and `b` can conflict.
pub fn modes_meet(a: &str, b: &str) -> bool {
    a == b || a == COMMON_MODE || b == COMMON_MODE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_and_pause_are_one_key() {
        let keys = MangoKeys::us();
        assert_eq!(keys.canonical("XF86AudioPause", false), keys.canonical("XF86AudioPlay", false));
        assert_ne!(keys.canonical("XF86AudioPause", true), keys.canonical("XF86AudioPlay", true), "binds keeps the keysym");
    }

    #[test]
    fn names_and_codes_meet_on_one_key() {
        let keys = MangoKeys::us();
        assert_eq!(keys.canonical("Q", false), "q");
        assert_eq!(keys.canonical("code:24", false), "q");
        assert_eq!(keys.canonical("exclam", false), keys.canonical("1", false), "shifted symbols share their key");
        assert_eq!(keys.canonical("Return", false), "return");
        assert_ne!(keys.canonical("XF86AudioNext", false), keys.canonical("XF86AudioPrev", false));
    }

    #[test]
    fn keysym_aliases_are_one_key() {
        assert_eq!(canonical_keysym("Prior"), canonical_keysym("Page_Up"));
        assert_eq!(canonical_keysym("Q"), "q");
        assert_eq!(canonical_keysym("mouse:272"), "mouse:272");
        assert_ne!(canonical_keysym("XF86AudioPlay"), canonical_keysym("XF86AudioPause"), "two keysyms, as Niri and Hyprland see them");
    }

    #[test]
    fn unknown_names_stay_themselves() {
        assert_eq!(MangoKeys::us().canonical("NotAKey", false), "notakey");
    }

    #[test]
    fn configured_layouts_come_first() {
        // On a French layout `a` sits where `q` is on `us`.
        let fr = MangoKeys::new(&XkbRules { layout: "fr".into(), ..Default::default() });
        assert_eq!(fr.canonical("a", false), "a");
        assert_eq!(fr.canonical("code:24", false), "a");
    }

    #[test]
    fn flags_and_modes() {
        assert_eq!(bind_flags("bindl"), Some("l"));
        assert_eq!(bind_flags("bind"), Some(""));
        assert_eq!(bind_flags("bindsl"), Some("sl"));
        assert_eq!(bind_flags("mousebind"), None);
        assert_eq!(bind_flags("bindx"), None);
        assert!(keysym_only("sl"));
        assert!(modes_meet("default", "common") && modes_meet("resize", "resize") && !modes_meet("default", "resize"));
    }
}
