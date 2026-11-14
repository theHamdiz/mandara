//! A presentation palette can differ from the desktop palette it previews.

#[derive(Clone)]
pub(super) struct ThemeLayers<T> {
    pub shell: T,
    pub presentation: Option<T>,
}

impl<T> ThemeLayers<T> {
    pub fn frame(&self) -> &T {
        self.presentation.as_ref().unwrap_or(&self.shell)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_desktop_theme_change_does_not_replace_the_presentation_palette() {
        let mut layers = ThemeLayers {
            shell: "coast",
            presentation: None,
        };
        assert_eq!(*layers.frame(), "coast");
        layers.presentation = Some("paper");
        layers.shell = "night";
        assert_eq!(*layers.frame(), "paper");
        assert_eq!(layers.shell, "night");
        layers.presentation = None;
        assert_eq!(*layers.frame(), "night");
    }

    #[test]
    fn clearing_presentation_releases_its_owner() {
        let owner = std::rc::Rc::new(());
        let weak = std::rc::Rc::downgrade(&owner);
        let mut layers = ThemeLayers {
            shell: std::rc::Rc::new(()),
            presentation: Some(owner),
        };
        layers.presentation = None;
        assert!(weak.upgrade().is_none());
    }
}
