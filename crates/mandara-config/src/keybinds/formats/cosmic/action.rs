//! Validate changed actions against COSMIC's published shortcut protocol.
//! https://github.com/pop-os/cosmic-settings-daemon/blob/master/config/src/shortcuts/action.rs

use std::fmt;

use serde::{
    Deserialize, Deserializer,
    de::{EnumAccess, Error as _, VariantAccess as _, Visitor},
};

const UNIT_ACTIONS: &[&str] = &[
    "Close",
    "Debug",
    "Disable",
    "LastWorkspace",
    "Maximize",
    "Fullscreen",
    "MigrateWorkspaceToNextOutput",
    "MigrateWorkspaceToPreviousOutput",
    "Minimize",
    "MoveToLastWorkspace",
    "MoveToNextOutput",
    "MoveToNextWorkspace",
    "MoveToPreviousOutput",
    "MoveToPreviousWorkspace",
    "NextOutput",
    "NextWorkspace",
    "PreviousOutput",
    "PreviousWorkspace",
    "SendToLastWorkspace",
    "SendToNextOutput",
    "SendToNextWorkspace",
    "SendToPreviousOutput",
    "SendToPreviousWorkspace",
    "SwapWindow",
    "Terminate",
    "ToggleOrientation",
    "ToggleStacking",
    "ToggleSticky",
    "ToggleTiling",
    "ToggleWindowFloating",
    "ZoomIn",
    "ZoomOut",
];
const DIRECTIONAL_ACTIONS: &[&str] = &[
    "MigrateWorkspaceToOutput",
    "Move",
    "MoveToOutput",
    "SendToOutput",
    "SwitchOutput",
];

#[derive(Deserialize)]
enum Direction {
    Left,
    Right,
    Up,
    Down,
}
#[derive(Deserialize)]
enum FocusDirection {
    Left,
    Right,
    Up,
    Down,
    In,
    Out,
}
#[derive(Deserialize)]
enum Orientation {
    Horizontal,
    Vertical,
}
#[derive(Deserialize)]
enum ResizeDirection {
    Inwards,
    Outwards,
}
#[derive(Deserialize)]
enum System {
    AppLibrary,
    BrightnessDown,
    BrightnessUp,
    DisplayToggle,
    HomeFolder,
    InputSourceSwitch,
    KeyboardBrightnessDown,
    KeyboardBrightnessUp,
    Launcher,
    LockScreen,
    LogOut,
    Mute,
    MuteMic,
    PlayPause,
    PlayNext,
    PlayPrev,
    PowerOff,
    ScreenReader,
    Screenshot,
    Suspend,
    Terminal,
    TouchpadToggle,
    VolumeLower,
    VolumeRaise,
    WebBrowser,
    WindowSwitcher,
    WindowSwitcherPrevious,
    WorkspaceOverview,
}

struct Action;
struct ActionName(String);
impl<'de> Deserialize<'de> for ActionName {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        struct NameVisitor;
        impl Visitor<'_> for NameVisitor {
            type Value = ActionName;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a native action identifier")
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<ActionName, E> {
                Ok(ActionName(value.into()))
            }
        }
        de.deserialize_identifier(NameVisitor)
    }
}
impl<'de> Deserialize<'de> for Action {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        struct ActionVisitor;
        impl<'de> Visitor<'de> for ActionVisitor {
            type Value = Action;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a COSMIC shortcut action")
            }
            fn visit_enum<A: EnumAccess<'de>>(self, access: A) -> Result<Action, A::Error> {
                let (ActionName(name), variant) = access.variant::<ActionName>()?;
                match name.as_str() {
                    name if UNIT_ACTIONS.contains(&name) => variant.unit_variant()?,
                    name if DIRECTIONAL_ACTIONS.contains(&name) => {
                        let _: Direction = variant.newtype_variant()?;
                    }
                    "Focus" => {
                        let _: FocusDirection = variant.newtype_variant()?;
                    }
                    "Orientation" => {
                        let _: Orientation = variant.newtype_variant()?;
                    }
                    "Resizing" => {
                        let _: ResizeDirection = variant.newtype_variant()?;
                    }
                    "System" => {
                        let _: System = variant.newtype_variant()?;
                    }
                    "Spawn" => {
                        let _: String = variant.newtype_variant()?;
                    }
                    "MoveToWorkspace" | "SendToWorkspace" | "Workspace" => {
                        let _: u8 = variant.newtype_variant()?;
                    }
                    _ => {
                        return Err(A::Error::custom(mandara_i18n::tf(
                            "Unknown COSMIC shortcut action: {action}",
                            &[("action", &name)],
                        )));
                    }
                }
                Ok(Action)
            }
        }
        de.deserialize_enum("Action", &[], ActionVisitor)
    }
}

pub(super) fn validate(value: &str) -> anyhow::Result<()> {
    ron::from_str::<Action>(value)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_published_action_family_and_legacy_unit_action_is_checked() {
        for action in UNIT_ACTIONS {
            validate(action).unwrap();
        }
        for action in DIRECTIONAL_ACTIONS {
            for direction in ["Left", "Right", "Up", "Down"] {
                validate(&format!("{action}({direction})")).unwrap();
            }
        }
        for action in [
            "Focus(In)",
            "Focus(Out)",
            "Orientation(Horizontal)",
            "Resizing(Inwards)",
            "System(Terminal)",
            "Spawn(\"foot\")",
            "Workspace(1)",
            "MoveToWorkspace(255)",
            "SendToWorkspace(0)",
        ] {
            validate(action).unwrap();
        }
    }

    #[test]
    fn misspelled_actions_wrong_payloads_and_out_of_range_workspaces_are_rejected() {
        for action in [
            "Clsoe",
            "Close(1)",
            "Move(In)",
            "Focus(Around)",
            "Workspace(256)",
            "System(Unknown)",
            "Spawn(4)",
            "Spawn(\"a\", \"b\")",
        ] {
            assert!(validate(action).is_err(), "{action}");
        }
    }
}
