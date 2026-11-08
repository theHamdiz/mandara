//! The search field's prefix help, one source for every launcher look.
//!
//! Classic, Grid and Beacon all show the same list of what a query can
//! start with: `/` slash commands, `=` arithmetic, the AI Shell Agent's
//! `!` while it is switched on, then each provider that is on and has a
//! prefix (`:` emoji, `@` windows…), with the prefix you configured.
//! Built-in syntax and the agent come before the providers, so they stay
//! visible when the field ellipsizes the end. A field wide enough for all
//! of it also leads with what plain text searches ([`Form::Full`]); a
//! narrower one (any look at its default size, or larger type) starts
//! with the prefixes ([`Form::Compact`]).
//!
//! Both forms are composed once per prefix-settings revision and language
//! ([`PrefixHelp`]), never per frame; a frame only compares the field's
//! width with the cached length. The launcher bumps the revision from its
//! config observer when the provider settings or the agent switch change
//! while it is open.

use gpui::SharedString;
use mandara_config::workflow::LauncherProviders;
use mandara_i18n::{Lang, t, tf, tk};

/// Between two entries of the help.
const SEPARATOR: &str = " · ";
/// Average advance of the help's characters, in ems: UI type averages
/// about half an em, a little more keeps the lead for fields that surely
/// fit it (a wrong guess only ellipsizes the end).
const EM_PER_CHAR: f32 = 0.55;

/// With or without the lead.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Form {
    /// Leads with what plain text searches.
    Full,
    /// The prefixes only.
    Compact,
}

/// Each provider's entry, in [`LauncherProviders::all`] order.
fn provider_entry(id: &str) -> Option<&'static str> {
    Some(match id {
        "emoji" => tk("“{prefix}” emoji"),
        "windows" => tk("“{prefix}” windows"),
        "clipboard" => tk("“{prefix}” clipboard"),
        "wallpapers" => tk("“{prefix}” wallpapers"),
        "files" => tk("“{prefix}” files"),
        "actions" => tk("“{prefix}” actions"),
        _ => return None,
    })
}

/// The help text for these settings, in the current language.
pub(super) fn compose(providers: &LauncherProviders, agent: bool, form: Form) -> String {
    let mut out = String::new();
    let mut push = |part: &str| {
        if !out.is_empty() {
            out.push_str(SEPARATOR);
        }
        out.push_str(part);
    };
    if form == Form::Full {
        push(t("Apps, settings, commands"));
    }
    push(t("“/” slash commands"));
    push(t("“=” calculate"));
    if agent {
        push(&tf("“{prefix}” ask the AI agent", &[("prefix", &mandara_services::command_agent::PREFIX)]));
    }
    for (id, p) in providers.all() {
        if p.enabled
            && !p.prefix.is_empty()
            && let Some(entry) = provider_entry(id)
        {
            push(&tf(entry, &[("prefix", &p.prefix)]));
        }
    }
    out
}

/// Whether text of `chars` characters at `font_px` fits a field `width`
/// px wide (`None`: not laid out yet).
fn fits(chars: usize, font_px: f32, width: Option<f32>) -> bool {
    width.is_some_and(|w| chars as f32 * font_px * EM_PER_CHAR <= w)
}

/// Both forms for one revision and language.
struct Composed {
    revision: u64,
    lang: Lang,
    full: SharedString,
    /// `full`'s length in characters (its fit is checked per frame).
    full_chars: usize,
    compact: SharedString,
}

/// The composed help, cached by prefix-settings revision and language.
#[derive(Default)]
pub(super) struct PrefixHelp {
    /// Bumped whenever the providers or the agent switch change.
    revision: u64,
    cached: Option<Composed>,
}

impl PrefixHelp {
    /// The settings it is composed from changed: the next [`Self::get`]
    /// composes again.
    pub(super) fn invalidate(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// The help for a field `width` px wide (as laid out last frame) at
    /// `font_px`, in the current language. Composes only when the revision
    /// or the language differ from the cached ones.
    pub(super) fn get(&mut self, providers: &LauncherProviders, agent: bool, width: Option<f32>, font_px: f32) -> SharedString {
        let lang = mandara_i18n::language();
        let stale = self.cached.as_ref().is_none_or(|c| (c.revision, c.lang) != (self.revision, lang));
        if stale {
            let full = compose(providers, agent, Form::Full);
            let full_chars = full.chars().count();
            self.cached = Some(Composed { revision: self.revision, lang, full: full.into(), full_chars, compact: compose(providers, agent, Form::Compact).into() });
        }
        let c = self.cached.as_ref().expect("composed above");
        if fits(c.full_chars, font_px, width) { c.full.clone() } else { c.compact.clone() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefixes(providers: &LauncherProviders) -> Vec<&str> {
        providers.all().into_iter().filter(|(_, p)| p.enabled && !p.prefix.is_empty()).map(|(_, p)| p.prefix.as_str()).collect()
    }

    /// Every enabled prefix is listed, in both forms; the agent's `!` only
    /// while it is on.
    #[test]
    fn the_help_lists_every_enabled_prefix_and_the_agent_only_when_on() {
        let mut providers = LauncherProviders::default();
        for form in [Form::Full, Form::Compact] {
            for agent in [false, true] {
                let text = compose(&providers, agent, form);
                for prefix in prefixes(&providers).into_iter().chain(["/", "="]) {
                    assert!(text.contains(&format!("“{prefix}”")), "{form:?} {agent}: {prefix} missing from {text:?}");
                }
                assert_eq!(text.contains("“!”"), agent, "{form:?}: {text:?}");
                assert_eq!(text.starts_with("Apps, settings, commands"), form == Form::Full, "{text:?}");
            }
        }
        // A switched-off provider and an emptied prefix leave the list; a
        // changed prefix shows as configured.
        providers.clipboard.enabled = false;
        providers.files.prefix.clear();
        providers.windows.prefix = "w:".into();
        let text = compose(&providers, true, Form::Full);
        assert!(!text.contains("clipboard") && !text.contains("files"), "{text:?}");
        assert!(text.contains("“w:” windows") && !text.contains("“@”"), "{text:?}");
        // Built-in syntax and the agent lead the providers (so they stay
        // visible when the field ellipsizes).
        let agent = text.find("“!”").unwrap();
        assert!(text.find("“=”").unwrap() < agent && agent < text.find("“:”").unwrap(), "{text:?}");
    }

    /// Every look shows the same prefixes, the agent's included while it
    /// is on: the lead only joins them when the whole list fits the field.
    #[test]
    fn every_look_shows_the_same_prefixes_and_the_lead_only_when_it_fits() {
        use mandara_config::workflow::LauncherLook;
        let providers = LauncherProviders::default();
        let mut help = PrefixHelp::default();
        let full_chars = compose(&providers, true, Form::Full).chars().count();
        // Each look's default field: Classic and Grid at their width and
        // body type, Beacon at its bar width and large type; then a field
        // with room for everything.
        let look = LauncherLook::default();
        for (name, width, font) in [("classic", look.width, 14.0), ("beacon", look.beacon_width, 20.0), ("roomy", full_chars as f32 * 14.0, 14.0)] {
            let text = help.get(&providers, true, Some(width), font);
            for prefix in prefixes(&providers).into_iter().chain(["/", "=", "!"]) {
                assert!(text.contains(&format!("“{prefix}”")), "{name}: {prefix} missing from {text:?}");
            }
            assert_eq!(text.starts_with("Apps, settings, commands"), name == "roomy", "{name}: {text:?}");
        }
        assert!(!help.get(&providers, true, None, 14.0).starts_with("Apps"), "not laid out yet: the prefixes first");
        // Few prefixes fit with the lead at the default width.
        let mut few = LauncherProviders::default();
        for p in [&mut few.emoji, &mut few.windows, &mut few.clipboard, &mut few.wallpapers, &mut few.files, &mut few.actions] {
            p.enabled = false;
        }
        help.invalidate();
        assert!(help.get(&few, false, Some(look.width), 14.0).starts_with("Apps, settings, commands"));
    }

    /// Both renderers draw their empty field from this module (Beacon's
    /// modes name what they search instead), so no look keeps a
    /// placeholder of its own.
    #[test]
    fn every_renderer_takes_its_placeholder_from_the_shared_help() {
        let classic = include_str!("../overlays.rs");
        let looks = include_str!("launcher_looks.rs");
        assert_eq!(classic.matches("self.query_field(").count() + looks.matches("self.query_field(").count(), 2, "one search field per renderer");
        assert!(classic.contains("let hint = self.prefix_help(text::BODY + 1.0);"));
        assert!(looks.contains("None => self.prefix_help(BAR_TEXT),"));
    }

    /// Composed once per revision and language; an edit composes it again.
    #[test]
    fn the_help_is_cached_until_the_settings_change() {
        let mut help = PrefixHelp::default();
        let mut providers = LauncherProviders::default();
        let first = help.get(&providers, false, Some(400.0), 14.0);
        // An unannounced edit is not seen: nothing is composed per frame.
        providers.emoji.enabled = false;
        let again = help.get(&providers, false, Some(400.0), 14.0);
        assert!(std::ptr::eq(first.as_ref(), again.as_ref()), "the cached text is shared, not composed again");
        help.invalidate();
        let edited = help.get(&providers, true, Some(400.0), 14.0);
        assert!(!edited.contains("emoji") && edited.contains("“!”"), "{edited:?}");
    }
}
