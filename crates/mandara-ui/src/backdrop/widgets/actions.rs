//! What a click on a painted desktop widget does.
//!
//! The wallpaper canvas takes the pointer; a left click (or a scroll step)
//! on a widget's clickable area is posted to the shell as an [`Action`],
//! which [`crate::desktop_actions`] runs. Every action also has a text form
//! (`toggle:wifi`, `todo:42`, `media:next`) so `mandara msg desktop
//! widgets press <widget> <action>` does exactly what the click does.

use std::fmt;

/// A widget action.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Check or uncheck a to-do.
    Todo(u64),
    /// Open the to-do panel to add one.
    TodoAdd,
    /// Open the notes panel (on a note, by title).
    Note(String),
    /// Start or pause the focus timer; skip the phase; reset it.
    Pomodoro,
    PomodoroSkip,
    PomodoroReset,
    /// Launch an application (desktop id).
    Launch(String),
    /// Open a folder, file or web address.
    Open(String),
    /// Flip a quick toggle (`wifi`, `bluetooth`, `dnd`, `night-light`,
    /// `keep-awake`, `dark`, `mute`).
    Toggle(String),
    /// Copy text to the clipboard (a palette swatch's colour).
    Copy(String),
    /// `play-pause`, `next` or `previous`.
    Media(String),
    /// Open the launcher.
    Search,
    /// Set `volume` or `brightness` to a level (0–1).
    Level(String, f32),
    /// Nudge `volume` or `brightness` up (+1) or down (−1) a step.
    Step(String, i32),
    /// The widget's own "next" (another photo, another quote) or a poke
    /// (the desk pet).
    Next,
    /// Open a plugin's panel.
    Panel(String),
    /// Fetch the widget's feed again now (rates, GitHub, AI usage, games).
    Refresh,
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Action::Todo(id) => write!(f, "todo:{id}"),
            Action::TodoAdd => f.write_str("todo-add"),
            Action::Note(t) if t.is_empty() => f.write_str("note"),
            Action::Note(t) => write!(f, "note:{t}"),
            Action::Pomodoro => f.write_str("pomodoro"),
            Action::PomodoroSkip => f.write_str("pomodoro-skip"),
            Action::PomodoroReset => f.write_str("pomodoro-reset"),
            Action::Launch(id) => write!(f, "launch:{id}"),
            Action::Open(p) => write!(f, "open:{p}"),
            Action::Toggle(t) => write!(f, "toggle:{t}"),
            Action::Copy(t) => write!(f, "copy:{t}"),
            Action::Media(v) => write!(f, "media:{v}"),
            Action::Search => f.write_str("search"),
            Action::Level(w, v) => write!(f, "level:{w}:{v}"),
            Action::Step(w, d) => write!(f, "step:{w}:{d}"),
            Action::Next => f.write_str("next"),
            Action::Panel(p) => write!(f, "panel:{p}"),
            Action::Refresh => f.write_str("refresh"),
        }
    }
}

impl std::str::FromStr for Action {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let (verb, arg) = s.split_once(':').map_or((s, ""), |(v, a)| (v, a));
        let need = |a: &str| if a.is_empty() { Err(format!("{verb} needs an argument ({verb}:…)")) } else { Ok(a.to_string()) };
        Ok(match verb {
            "todo" => Action::Todo(arg.parse().map_err(|_| format!("not a to-do id: {arg:?}"))?),
            "todo-add" => Action::TodoAdd,
            "note" => Action::Note(arg.to_string()),
            "pomodoro" => Action::Pomodoro,
            "pomodoro-skip" => Action::PomodoroSkip,
            "pomodoro-reset" => Action::PomodoroReset,
            "launch" => Action::Launch(need(arg)?),
            "open" => Action::Open(need(arg)?),
            "toggle" => Action::Toggle(need(arg)?),
            "copy" => Action::Copy(need(arg)?),
            "media" => Action::Media(need(arg)?),
            "search" => Action::Search,
            "level" | "step" => {
                let (what, v) = arg.split_once(':').ok_or_else(|| format!("{verb}:<volume|brightness>:<value>"))?;
                if verb == "level" {
                    Action::Level(what.to_string(), v.parse::<f32>().map_err(|e| e.to_string())?.clamp(0.0, 1.0))
                } else {
                    Action::Step(what.to_string(), v.parse::<i32>().map_err(|e| e.to_string())?.signum())
                }
            }
            "next" => Action::Next,
            "panel" => Action::Panel(need(arg)?),
            "refresh" => Action::Refresh,
            other => return Err(format!("unknown action {other:?}")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_round_trip_through_text() {
        let all = [
            Action::Todo(42),
            Action::TodoAdd,
            Action::Note(String::new()),
            Action::Note("Ideas".into()),
            Action::Pomodoro,
            Action::PomodoroSkip,
            Action::PomodoroReset,
            Action::Launch("firefox.desktop".into()),
            Action::Open("https://example.org/a:b".into()),
            Action::Toggle("wifi".into()),
            Action::Copy("#1a2b3c".into()),
            Action::Media("next".into()),
            Action::Search,
            Action::Level("volume".into(), 0.5),
            Action::Step("brightness".into(), -1),
            Action::Next,
            Action::Panel("prayer".into()),
            Action::Refresh,
        ];
        for a in all {
            assert_eq!(a.to_string().parse::<Action>(), Ok(a.clone()), "{a}");
        }
        assert!("toggle".parse::<Action>().is_err() && "dance".parse::<Action>().is_err());
    }
}
