//! API keys are edited separately from config-backed plugin settings.
//! The shared masked field is cleared after a write and on dismissal.

use gpui::{AnyElement, ClickEvent, Context, IntoElement, ParentElement, StatefulInteractiveElement, Styled, div, px};
use mandara_i18n::t;
use mandara_core::ToGpui;
use mandara_services::credentials::{Access, Key, Keyring, Secret, Store as _};
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;

use super::{StudioView, compositor_page::button, form};
use crate::tokens::*;

const INPUT: &str = "plugin-api-key";

#[derive(Default)]
pub(super) struct Editor {
    key: Option<Key>,
    generation: u64,
    working: bool,
    saved: Option<bool>,
    note: Option<(bool, String)>,
}

impl Editor {
    /// Invalidate UI replies on dismissal; in-flight keyring writes still finish.
    pub(super) fn close(&mut self) {
        self.key = None;
        self.generation = self.generation.wrapping_add(1);
        self.working = false;
        self.saved = None;
        self.note = None;
    }

    /// Ignore a closing editor; a newly selected key invalidates earlier jobs.
    fn select(&mut self, key: Key, open: bool) -> Option<bool> {
        if !open { return None; }
        if self.key.as_ref() == Some(&key) { return Some(false); }
        self.key = Some(key);
        self.generation = self.generation.wrapping_add(1);
        self.working = false;
        self.saved = None;
        self.note = None;
        Some(true)
    }

    /// Allow one keyring operation and tag it with the current editor generation.
    fn begin(&mut self) -> Option<(u64, Key)> {
        if self.working { return None; }
        let key = self.key.clone()?;
        self.generation = self.generation.wrapping_add(1);
        self.working = true;
        self.note = None;
        Some((self.generation, key))
    }

    /// Publish a keyring result only if its editor generation is still current.
    fn finish(&mut self, generation: u64, result: Result<bool, String>) {
        if generation != self.generation { return; }
        self.working = false;
        match result {
            Ok(saved) => { self.saved = Some(saved); self.note = None; }
            Err(error) => self.note = Some((true, error)),
        }
    }
}

pub(super) fn clear_input(cx: &mut gpui::App) { form::clear(INPUT, cx); }

impl StudioView {
    /// Check for a saved key quietly, without prompting just because settings opened.
    fn check_plugin_key(&mut self, cx: &mut Context<Self>) {
        let Some((generation, key)) = self.credentials.begin() else { return };
        form::job(cx, move || Keyring.read(&key, Access::Quiet).map(|secret| secret.is_some()), move |this, result, _| this.credentials.finish(generation, result));
    }

    /// Clear the input immediately and save its validated secret outside the UI thread.
    fn save_plugin_key(&mut self, cx: &mut Context<Self>) {
        if self.credentials.working { return; }
        let secret = Secret::new(form::value(INPUT, cx));
        clear_input(cx);
        let secret = match secret {
            Ok(secret) => secret,
            Err(error) => { self.credentials.note = Some((true, error)); cx.notify(); return; }
        };
        let Some((generation, key)) = self.credentials.begin() else { return };
        form::job(cx, move || Keyring.save(&key, &secret).map(|()| true), move |this, result, _| this.credentials.finish(generation, result));
        cx.notify();
    }

    /// Remove the selected server’s key and clear any pasted input.
    fn remove_plugin_key(&mut self, cx: &mut Context<Self>) {
        let Some((generation, key)) = self.credentials.begin() else { return };
        clear_input(cx);
        form::job(cx, move || Keyring.remove(&key).map(|()| false), move |this, result, _| this.credentials.finish(generation, result));
        cx.notify();
    }

    /// Render only while open; reopening rechecks the selected server’s keyring entry.
    pub(super) fn plugin_key_editor(&mut self, key: Key, open: bool, p: &MandaraPalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.credentials.select(key, open)? {
            clear_input(cx);
            self.check_plugin_key(cx);
        }
        let field = form::input(INPUT, t("Paste an API key"), true, cx);
        let label = if self.credentials.working { t("Opening the keyring…") }
            else if self.credentials.saved == Some(true) { t("An API key is saved for this server") }
            else { t("No API key saved for this server") };
        let mut card = div().row().flex_col().gap(px(SPACE_12_PX))
            .child(div().text_size(px(TEXT_14_PX)).child(t("API key")))
            .child(div().text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla())
                .child(mandara_ui::bidi::paragraph(t("Saved in your desktop keyring for this server, separate from Mandara's configuration."))))
            .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(label))
            .child(field);
        if !self.credentials.working {
            card = card.child(div().row().gap(px(SPACE_8_PX))
                .child(button("plugin-key-save", "key", t("Save key"), true, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.save_plugin_key(cx))))
                .child(button("plugin-key-remove", "close", t("Remove key"), false, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.remove_plugin_key(cx)))));
        }
        if let Some((error, note)) = &self.credentials.note { card = card.child(form::note(note, *error, p)); }
        Some(card.into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reopening_rechecks_the_key_and_discards_a_closed_editors_reply() {
        let key = Key { provider: mandara_services::credentials::Provider::HomeAssistant, server: "https://home.example".into() };
        let mut editor = Editor::default();
        assert_eq!(editor.select(key.clone(), true), Some(true));
        let old = editor.begin().unwrap().0;
        editor.close();
        assert_eq!(editor.select(key.clone(), false), None);
        assert!(editor.begin().is_none());
        editor.finish(old, Err("closed reply".into()));
        assert!(editor.note.is_none());
        assert_eq!(editor.select(key, true), Some(true));
        let current = editor.begin().unwrap().0;
        editor.finish(old, Ok(true));
        assert!(editor.working);
        assert!(editor.saved.is_none());
        editor.finish(current, Ok(false));
        assert_eq!(editor.saved, Some(false));
    }
    use mandara_services::credentials::Provider;

    #[test]
    fn a_key_result_cannot_cross_servers_or_override_a_later_operation() {
        let mut editor = Editor::default();
        let first = Key { provider: Provider::HomeAssistant, server: "https://first.example".into() };
        assert_eq!(editor.select(first.clone(), true), Some(true));
        let (old, _) = editor.begin().unwrap();
        assert!(editor.begin().is_none());
        assert_eq!(editor.select(Key { server: "https://second.example".into(), ..first }, true), Some(true));
        let (new, _) = editor.begin().unwrap();
        editor.finish(old, Ok(true));
        assert_eq!(editor.saved, None);
        assert!(editor.working);
        editor.finish(new, Ok(false));
        assert_eq!(editor.saved, Some(false));
        assert!(!editor.working);
    }
}
