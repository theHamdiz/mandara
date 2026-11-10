//! What a sidebar card was in the middle of when the sidebar let it go
//! ([`crate::sidebar_tabs::RELEASE_AFTER`] after it closed): a half-typed
//! task, the open note and its caret, a search. The card saves it as its
//! views are released, and the card built at the next opening takes it
//! back, so the user finds the card as they left it while the views
//! themselves, their elements and caches, are gone.
//!
//! Drafts stay in memory, one per card and output: a shell restart starts
//! clean (saved tasks and notes are in their own stores). A card with
//! nothing typed keeps nothing and allocates nothing.

use gpui::{AnyView, App, Global};

use crate::text_field::{TextField, floor_boundary};

/// The longest draft text kept, in bytes (cut at a character boundary).
pub const MAX_TEXT_BYTES: usize = 4096;

/// A text field's typed text, its caret and whether it was still
/// uncommitted.
#[derive(Debug, PartialEq, Eq)]
pub struct TextDraft {
    text: Box<str>,
    caret: u32,
    dirty: bool,
}

impl TextDraft {
    /// `text` (cut to [`MAX_TEXT_BYTES`]) with its caret, or `None` when
    /// it is empty. The text's spare capacity is given back.
    pub fn new(mut text: String, caret: usize, dirty: bool) -> Option<Self> {
        if text.is_empty() {
            return None;
        }
        if text.len() > MAX_TEXT_BYTES {
            text.truncate(floor_boundary(&text, MAX_TEXT_BYTES));
        }
        let caret = floor_boundary(&text, caret) as u32;
        Some(Self { text: text.into_boxed_str(), caret, dirty })
    }

    /// Takes the text out of `field`, which is going away.
    pub fn take(field: &mut TextField) -> Option<Self> {
        let (caret, dirty) = (field.caret(), field.is_dirty());
        Self::new(std::mem::take(&mut field.value), caret, dirty)
    }

    /// Puts the text back into a newly built `field`.
    pub fn restore(self, field: &mut TextField) {
        field.restore(self.text.into_string(), self.caret as usize, self.dirty);
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn caret(&self) -> usize {
        self.caret as usize
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }
}

/// One kind of card's drafts, by output (an app global per kind, made by
/// the first draft kept).
pub struct Drafts<T> {
    slots: Vec<(Box<str>, T)>,
}

impl<T> Default for Drafts<T> {
    fn default() -> Self {
        Self { slots: Vec::new() }
    }
}

impl<T: 'static> Global for Drafts<T> {}

impl<T> Drafts<T> {
    /// Keeps `draft` for the card on `monitor`, replacing an older one.
    pub fn keep(&mut self, monitor: &str, draft: T) {
        match self.slots.iter_mut().find(|(m, _)| &**m == monitor) {
            Some((_, slot)) => *slot = draft,
            None => {
                // One slot per output, not the four a first push reserves.
                self.slots.reserve_exact(1);
                self.slots.push((monitor.into(), draft));
            }
        }
    }

    /// Takes the card on `monitor`'s draft; the last one taken frees the
    /// list's room.
    pub fn take(&mut self, monitor: &str) -> Option<T> {
        let at = self.slots.iter().position(|(m, _)| &**m == monitor)?;
        let (_, draft) = self.slots.swap_remove(at);
        if self.slots.is_empty() {
            self.slots = Vec::new();
        }
        Some(draft)
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

/// Keeps `draft` for the card on `monitor`; `None` keeps nothing (and
/// touches no global).
pub fn keep<T: 'static>(monitor: &str, draft: Option<T>, cx: &mut App) {
    if let Some(draft) = draft {
        cx.default_global::<Drafts<T>>().keep(monitor, draft);
        tracing::debug!(monitor, kind = std::any::type_name::<T>(), "kept a sidebar card's draft");
    }
}

/// Takes the card on `monitor`'s draft, if one was kept.
pub fn take<T: 'static>(monitor: &str, cx: &mut App) -> Option<T> {
    if !cx.has_global::<Drafts<T>>() {
        return None;
    }
    let draft = cx.global_mut::<Drafts<T>>().take(monitor);
    if draft.is_some() {
        tracing::debug!(monitor, kind = std::any::type_name::<T>(), "a rebuilt sidebar card took its draft back");
    }
    draft
}

/// Runs `save` once, when the card `view` is released, while whatever
/// `save` holds (the card's fields) is still alive to read.
pub fn on_release(view: &AnyView, cx: &mut App, save: impl FnOnce(&mut App) + 'static) {
    match view.clone().downcast::<crate::plugins::FnView>() {
        Ok(view) => cx.observe_release(&view, move |_, cx| save(cx)).detach(),
        Err(_) => tracing::warn!("card drafts: the card isn't a function view, so its draft isn't kept"),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::sidebar_tabs::CardPool;

    #[test]
    fn an_empty_field_keeps_nothing() {
        assert_eq!(TextDraft::new(String::new(), 0, true), None);
        let empty: Drafts<TextDraft> = Drafts::default();
        assert_eq!(empty.slots.capacity(), 0, "no draft, no allocation");
    }

    #[test]
    fn a_draft_keeps_text_caret_and_whether_it_was_committed() {
        let d = TextDraft::new("Buy bread".into(), 3, true).unwrap();
        assert_eq!((d.text(), d.caret(), d.is_dirty()), ("Buy bread", 3, true));
        let d = TextDraft::new("مرحبا".into(), 3, false).unwrap();
        assert_eq!(d.caret(), 2, "a caret inside a letter moves to its start");
        assert!(!d.is_dirty());
        let d = TextDraft::new("ab".into(), 99, true).unwrap();
        assert_eq!(d.caret(), 2, "a caret past the end stays at the end");
    }

    #[test]
    fn a_long_draft_is_capped_at_a_character_boundary() {
        let text = "é".repeat(MAX_TEXT_BYTES);
        let d = TextDraft::new(text, usize::MAX, true).unwrap();
        assert_eq!(d.text().len(), MAX_TEXT_BYTES);
        assert_eq!(d.caret(), MAX_TEXT_BYTES);
        let odd = format!("a{}", "é".repeat(MAX_TEXT_BYTES));
        let d = TextDraft::new(odd, 0, true).unwrap();
        assert_eq!(d.text().len(), MAX_TEXT_BYTES - 1, "never splits a letter");
    }

    #[test]
    fn a_draft_holds_no_spare_room() {
        let mut typed = String::with_capacity(64);
        typed.push_str("Call the plumber");
        let d = TextDraft::new(typed, 4, true).unwrap();
        assert_eq!(d.text.len(), "Call the plumber".len(), "boxed to its length");
        assert_eq!(std::mem::size_of::<TextDraft>(), 24);
    }

    #[test]
    fn drafts_are_kept_per_output_and_taken_once() {
        let mut drafts: Drafts<&str> = Drafts::default();
        drafts.keep("DP-1", "left");
        assert_eq!(drafts.slots.capacity(), 1, "room for one output's draft only");
        drafts.keep("HDMI-A-1", "right");
        drafts.keep("DP-1", "left again");
        assert_eq!(drafts.len(), 2, "one draft per card and output");
        assert_eq!(drafts.take("DP-1"), Some("left again"));
        assert_eq!(drafts.take("DP-1"), None, "a rebuilt card takes its draft");
        assert_eq!(drafts.take("HDMI-A-1"), Some("right"));
        assert!(drafts.is_empty());
        assert_eq!(drafts.slots.capacity(), 0, "the last draft taken frees the list");
    }

    /// A card that saves its field when the pool lets it go, as a card's
    /// release hook does when its view is released.
    struct Card {
        field: RefCell<String>,
        store: Rc<RefCell<Drafts<TextDraft>>>,
    }

    impl Drop for Card {
        fn drop(&mut self) {
            let text = std::mem::take(&mut *self.field.borrow_mut());
            let caret = text.len();
            if let Some(d) = TextDraft::new(text, caret, true) {
                self.store.borrow_mut().keep("DP-1", d);
            }
        }
    }

    fn build(store: &Rc<RefCell<Drafts<TextDraft>>>) -> Rc<Card> {
        let text = store.borrow_mut().take("DP-1").map(|d| d.text().to_owned()).unwrap_or_default();
        Rc::new(Card { field: RefCell::new(text), store: store.clone() })
    }

    #[test]
    fn a_released_card_gives_its_draft_to_the_next_build() {
        let store: Rc<RefCell<Drafts<TextDraft>>> = Rc::default();
        let mut pool: CardPool<Rc<Card>> = CardPool::default();
        pool.set_layout(vec![("main".into(), vec!["todo".into()])], |_| Some(1));
        let card = pool.ensure("main", |_| Some(1), |_| Some(build(&store)))[0].clone();
        card.field.borrow_mut().push_str("Water the plants");
        drop(card);
        assert!(store.borrow().is_empty(), "nothing is kept while the card lives");
        pool.release();
        assert_eq!(store.borrow().len(), 1, "the release kept the draft");
        let card = pool.ensure("main", |_| Some(1), |_| Some(build(&store)))[0].clone();
        assert_eq!(*card.field.borrow(), "Water the plants");
        assert!(store.borrow().is_empty(), "the rebuilt card holds the only copy");
        // Submitted: the field is empty, so the next release keeps nothing.
        card.field.borrow_mut().clear();
        drop(card);
        pool.release();
        assert!(store.borrow().is_empty());
        let card = pool.ensure("main", |_| Some(1), |_| Some(build(&store)))[0].clone();
        assert_eq!(*card.field.borrow(), "", "a submitted draft is gone after a rebuild");
    }
}
