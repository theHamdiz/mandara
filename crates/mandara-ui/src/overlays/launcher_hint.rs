//! The search field's prefix help, one source for every launcher look.
//!
//! Classic, Grid and Beacon all show the same list of what a query can
//! start with: `/` slash commands, `=` arithmetic, the AI Shell Agent's
//! `!` while it is switched on, then each provider that is on and has a
//! prefix (`:` emoji, `@` windows…), with the prefix you configured. The
//! wide looks lead with what plain text searches; Beacon's larger type
//! drops that lead ([`Form::Compact`]) so the prefixes come first, and the
//! field ellipsizes whatever still doesn't fit. Built-in syntax and the
//! agent come before the providers, so they stay visible in a narrow field.
//!
//! The text is composed once per prefix-settings revision, language and
//! form ([`PrefixHelp`]), never per frame. The launcher bumps the revision
//! from its config observer when the provider settings or the agent switch
//! change while it is open.

use gpui::SharedString;
use mandara_config::workflow::{LauncherProviders, LauncherStyle};
use mandara_i18n::{Lang, t, tf, tk};

/// Between two entries of the help.
const SEPARATOR: &str = " · ";

/// Which looks get which form of the help.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Form {
    /// Leads with what plain text searches (Classic and Grid).
    Full,
    /// The prefixes only (Beacon's large search bar).
    Compact,
}

impl Form {
    /// The form a launcher style shows (Classic covers the list and the
    /// grid layouts).
    pub(super) fn of(style: LauncherStyle) -> Self {
        match style {
            LauncherStyle::Classic => Form::Full,
            LauncherStyle::Beacon => Form::Compact,
        }
    }
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

/// The composed help, cached by prefix-settings revision, language and
/// form.
#[derive(Default)]
pub(super) struct PrefixHelp {
    /// Bumped whenever the providers or the agent switch change.
    revision: u64,
    cached: Option<(u64, Lang, Form, SharedString)>,
}

impl PrefixHelp {
    /// The settings it is composed from changed: the next [`Self::get`]
    /// composes again.
    pub(super) fn invalidate(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// The help in `form` for the current language, composed only when the
    /// revision, language or form differ from the cached one.
    pub(super) fn get(&mut self, providers: &LauncherProviders, agent: bool, form: Form) -> SharedString {
        let lang = mandara_i18n::language();
        if let Some((rev, l, f, text)) = &self.cached
            && (*rev, *l, *f) == (self.revision, lang, form)
        {
            return text.clone();
        }
        let text = SharedString::from(compose(providers, agent, form));
        self.cached = Some((self.revision, lang, form, text.clone()));
        text
    }

    #[cfg(test)]
    pub(super) fn composed(&self) -> Option<&SharedString> {
        self.cached.as_ref().map(|(.., text)| text)
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

    /// Every ready-made look (Classic, Grid, Beacon) shows the same
    /// prefixes, the agent's included while it is on.
    #[test]
    fn every_look_shows_the_same_prefixes() {
        let providers = LauncherProviders::default();
        for look in &mandara_config::workflow::LAUNCHER_LOOKS {
            let text = compose(&providers, true, Form::of(look.style));
            for prefix in prefixes(&providers).into_iter().chain(["/", "=", "!"]) {
                assert!(text.contains(&format!("“{prefix}”")), "{}: {prefix} missing from {text:?}", look.id);
            }
            assert!(!compose(&providers, false, Form::of(look.style)).contains("“!”"), "{}", look.id);
        }
    }

    /// Both renderers draw their empty field from this module (Beacon's
    /// modes name what they search instead), so no look keeps a
    /// placeholder of its own.
    #[test]
    fn every_renderer_takes_its_placeholder_from_the_shared_help() {
        let classic = include_str!("../overlays.rs");
        let looks = include_str!("launcher_looks.rs");
        assert_eq!(classic.matches("self.query_field(").count() + looks.matches("self.query_field(").count(), 2, "one search field per renderer");
        assert!(classic.contains("let hint = self.prefix_help(launcher_hint::Form::of(look.style));"));
        assert!(looks.contains("None => self.prefix_help(super::launcher_hint::Form::of(LauncherStyle::Beacon)),"));
    }

    /// Composed once per revision, language and form; an edit composes it
    /// again.
    #[test]
    fn the_help_is_cached_until_the_settings_or_form_change() {
        let mut help = PrefixHelp::default();
        let mut providers = LauncherProviders::default();
        let first = help.get(&providers, false, Form::Full);
        // An unannounced edit is not seen: nothing is composed per frame.
        providers.emoji.enabled = false;
        let again = help.get(&providers, false, Form::Full);
        assert!(std::ptr::eq(first.as_ref(), again.as_ref()), "the cached text is shared, not composed again");
        help.invalidate();
        let edited = help.get(&providers, true, Form::Full);
        assert!(!edited.contains("emoji") && edited.contains("“!”"), "{edited:?}");
        let compact = help.get(&providers, true, Form::Compact);
        assert!(!compact.starts_with("Apps"), "{compact:?}");
        assert_eq!(help.composed(), Some(&compact));
    }
}
