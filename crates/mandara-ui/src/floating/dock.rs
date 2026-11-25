//! The bar's panel slot and its pin, as a pure state machine.
//!
//! One panel at a time opens out of the bar. Normally it is transient: an
//! outside click, a focus change, the pointer leaving a hover-opened panel
//! or an overlay opening (launcher, power menu, wallpaper picker) closes
//! it. A **pinned** panel ignores all of those; only its chip, Esc while it
//! has the keyboard, `panel close`, unpinning-then-dismissing or its plugin
//! closing it on purpose close it.
//!
//! When another panel needs the bar's slot while one is pinned, the pinned
//! one is not closed: it pops out into a floating card ([`Effect::Float`])
//! and the new panel takes the slot. A plugin whose panel already floats is
//! never opened in the slot as well: opening it raises its card, toggling
//! it closes the card.

/// What the shell must do besides changing the slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    /// This pinned panel was displaced: float it.
    Float(String),
    /// The panel already floats: bring its card to the front.
    Raise(String),
    /// The panel floats and was toggled: close its card.
    CloseFloating(String),
}

/// The bar's panel slot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PanelDock {
    /// The plugin whose panel is open in the slot.
    pub open: Option<String>,
    /// That panel stays open until closed on purpose.
    pub pinned: bool,
}

impl PanelDock {
    /// Opens `id` in the slot (`floating`: its panel is a floating card).
    pub fn open(&mut self, id: &str, floating: bool) -> Effect {
        if floating {
            return Effect::Raise(id.into());
        }
        if self.open.as_deref() == Some(id) {
            return Effect::None;
        }
        let displaced = if self.pinned { self.open.take() } else { None };
        self.open = Some(id.into());
        self.pinned = false;
        displaced.map_or(Effect::None, Effect::Float)
    }

    /// The chip / `panel toggle`: closes the open panel (pinned or not) or
    /// the floating card, opens anything else.
    pub fn toggle(&mut self, id: &str, floating: bool) -> Effect {
        if floating {
            return Effect::CloseFloating(id.into());
        }
        if self.open.as_deref() == Some(id) {
            self.close();
            return Effect::None;
        }
        self.open(id, false)
    }

    /// Closes the slot on purpose (Esc, `panel close`, the plugin itself).
    pub fn close(&mut self) {
        self.open = None;
        self.pinned = false;
    }

    /// A dismissal (outside click, focus moved, pointer left, an overlay
    /// opened): closes an unpinned panel. Returns whether it closed.
    pub fn dismiss(&mut self) -> bool {
        if self.pinned || self.open.is_none() {
            return false;
        }
        self.open = None;
        true
    }

    /// Opens `id` in the slot pinned (a pinned panel already there is
    /// displaced and floats).
    pub fn pin(&mut self, id: &str) -> Effect {
        let effect = if self.open.as_deref() == Some(id) { Effect::None } else { self.open(id, false) };
        self.pinned = true;
        effect
    }

    /// Unpins `id` if it is the pinned panel; it stays open until the next
    /// dismissal. Returns whether it was pinned.
    pub fn unpin(&mut self, id: &str) -> bool {
        let was = self.pinned && self.open.as_deref() == Some(id);
        if was {
            self.pinned = false;
        }
        was
    }

    /// Whether `id` is the pinned panel.
    pub fn is_pinned(&self, id: &str) -> bool {
        self.pinned && self.open.as_deref() == Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(id: &str) -> PanelDock {
        PanelDock { open: Some(id.into()), pinned: false }
    }

    #[test]
    fn dismissals_close_only_unpinned_panels() {
        let mut d = open("media");
        assert!(d.dismiss(), "an outside click closes a normal panel");
        assert_eq!(d.open, None);
        let mut d = open("media");
        assert_eq!(d.pin("media"), Effect::None);
        for _ in 0..3 {
            assert!(!d.dismiss(), "outside clicks, focus changes and the launcher leave a pinned panel open");
        }
        assert!(d.is_pinned("media"));
        assert!(!PanelDock::default().dismiss(), "nothing open, nothing to dismiss");
    }

    #[test]
    fn the_chip_and_esc_close_a_pinned_panel() {
        let mut d = PanelDock::default();
        d.pin("media");
        assert_eq!(d.toggle("media", false), Effect::None);
        assert_eq!(d, PanelDock::default(), "its chip closes and unpins it");
        d.pin("media");
        d.close();
        assert_eq!(d, PanelDock::default(), "Esc / panel close closes it");
    }

    #[test]
    fn unpinning_makes_it_dismissable_again() {
        let mut d = PanelDock::default();
        d.pin("weather");
        assert!(!d.unpin("media"), "only the pinned panel unpins");
        assert!(d.unpin("weather"));
        assert_eq!(d.open.as_deref(), Some("weather"), "unpinning keeps it open");
        assert!(d.dismiss());
    }

    #[test]
    fn a_displaced_pinned_panel_floats() {
        let mut d = PanelDock::default();
        d.pin("media");
        assert_eq!(d.open("weather", false), Effect::Float("media".into()));
        assert_eq!(d, open("weather"), "the newcomer takes the slot, unpinned");
        assert_eq!(d.open("clock", false), Effect::None, "an unpinned panel is simply replaced");
        d.pin("clock");
        assert_eq!(d.pin("network"), Effect::Float("clock".into()), "pinning another one floats the first");
        assert!(d.is_pinned("network"));
        assert_eq!(d.open("network", false), Effect::None, "reopening the pinned panel keeps the pin");
        assert!(d.is_pinned("network"));
    }

    #[test]
    fn floating_panels_never_open_twice() {
        let mut d = PanelDock::default();
        d.pin("media");
        assert_eq!(d.open("weather", true), Effect::Raise("weather".into()));
        assert_eq!(d.toggle("weather", true), Effect::CloseFloating("weather".into()));
        assert!(d.is_pinned("media"), "the pinned slot is untouched");
    }
}
