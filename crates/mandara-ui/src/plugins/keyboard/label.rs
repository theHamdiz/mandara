//! Keyboard badge labels are resolved only after the reported or custom name changes.

use gpui::SharedString;

/// Letters kept from an unknown layout's name.
const CODE_LEN: usize = 2;

/// "English (US)" → "EN", "Arabic" → "AR"; else the first two letters.
pub fn short_code(layout: &str) -> String {
    const NAMES: &[(&str, &str)] = &[
        ("english", "EN"),
        ("arabic", "AR"),
        ("french", "FR"),
        ("german", "DE"),
        ("spanish", "ES"),
        ("russian", "RU"),
        ("turkish", "TR"),
        ("japanese", "JA"),
        ("chinese", "ZH"),
        ("hindi", "HI"),
        ("persian", "FA"),
        ("hebrew", "HE"),
        ("italian", "IT"),
        ("portuguese", "PT"),
        ("dutch", "NL"),
        ("polish", "PL"),
        ("korean", "KO"),
        ("urdu", "UR"),
        ("greek", "EL"),
        ("ukrainian", "UK"),
    ];
    let lower = layout.to_lowercase();
    NAMES
        .iter()
        .find(|(name, _)| lower.starts_with(name))
        .map(|(_, code)| code.to_string())
        .unwrap_or_else(|| {
            layout
                .chars()
                .filter(|c| c.is_alphabetic())
                .take(CODE_LEN)
                .collect::<String>()
                .to_uppercase()
        })
}

#[derive(Default)]
pub(super) struct LayoutLabel {
    layout: String,
    custom: Option<SharedString>,
    shown: SharedString,
}

impl LayoutLabel {
    /// Compare borrowed inputs before deriving or copying their label.
    pub fn label(&mut self, layout: &str, custom: Option<&str>) -> SharedString {
        if self.layout != layout || self.custom.as_deref() != custom {
            self.layout.clear();
            self.layout.push_str(layout);
            self.custom = custom.map(|label| SharedString::from(label.to_owned()));
            self.shown = self
                .custom
                .clone()
                .unwrap_or_else(|| short_code(layout).into());
        }
        self.shown.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A config label edit changes a warm badge even when the hardware layout stays put.
    #[test]
    fn label_cache_tracks_layout_override_and_removal() {
        let mut labels = LayoutLabel::default();
        assert_eq!(labels.label("English (US)", None).as_ref(), "EN");
        assert_eq!(labels.label("English (US)", Some("ع")).as_ref(), "ع");
        let authored = "تخطيط لوحة المفاتيح العربية المخصص";
        let custom = labels.label("English (US)", Some(authored));
        assert_eq!(custom.as_ref(), authored);
        assert_eq!(
            custom.as_ref().as_ptr(),
            labels.label("English (US)", Some(authored)).as_ref().as_ptr()
        );
        assert_eq!(labels.label("English (US)", Some("ENG")).as_ref(), "ENG");
        assert_eq!(labels.label("English (US)", None).as_ref(), "EN");
        assert_eq!(labels.label("Arabic (Egypt)", None).as_ref(), "AR");
        assert_eq!(labels.label("Esperanto", None).as_ref(), "ES");
    }
}
