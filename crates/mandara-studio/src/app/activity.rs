//! Page visibility shared by Studio's background samplers.

use crate::registry::Section;

/// Home and Performance share one process sampler across navigation.
pub(super) const PROCESS_PAGES: [Section; 2] = [Section::Home, Section::Performance];

/// Search results and hidden windows do not consume a page's live readings.
pub(super) fn visible_page(current: Section, pages: &[Section], searching: bool, visible: bool) -> bool {
    visible && !searching && pages.contains(&current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_sampling_survives_switching_between_its_two_pages() {
        for page in [Section::Home, Section::Performance, Section::Home] {
            assert!(visible_page(page, &PROCESS_PAGES, false, true));
        }
        assert!(!visible_page(Section::Themes, &PROCESS_PAGES, false, true));
        assert!(!visible_page(Section::Home, &PROCESS_PAGES, true, true));
        assert!(!visible_page(Section::Performance, &PROCESS_PAGES, false, false));
    }

    #[test]
    fn diagnostics_run_only_on_the_visible_performance_page() {
        let pages = [Section::Performance];
        assert!(visible_page(Section::Performance, &pages, false, true));
        assert!(!visible_page(Section::Home, &pages, false, true));
        assert!(!visible_page(Section::Performance, &pages, true, true));
        assert!(!visible_page(Section::Performance, &pages, false, false));
    }
}
