//! Keep search editing, result navigation and unrelated fields distinct.

#[derive(Debug, PartialEq)]
pub(super) enum Action { Ignore, Results, Move(isize), Activate }

pub(super) fn action(key: &str, panel: bool, search: bool, unmodified: bool) -> Action {
    if !unmodified { return Action::Ignore; }
    if search && key == "tab" { return Action::Results; }
    if !panel { return Action::Ignore; }
    match key {
        "up" => Action::Move(-1),
        "down" => Action::Move(1),
        "enter" => Action::Activate,
        _ => Action::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_can_return_to_results_without_activating_a_copy() {
        assert_eq!(action("tab", false, true, true), Action::Results);
        for key in ["up", "down", "enter"] {
            assert_eq!(action(key, false, true, true), Action::Ignore);
        }
        assert_eq!(action("down", true, false, true), Action::Move(1));
        assert_eq!(action("enter", true, false, true), Action::Activate);
    }

    #[test]
    fn naming_fields_and_modified_keys_keep_their_own_input() {
        for key in ["tab", "up", "down", "enter"] {
            assert_eq!(action(key, false, false, true), Action::Ignore);
            assert_eq!(action(key, true, false, false), Action::Ignore);
        }
    }
}
