//! `[desktop.menu]`: the desktop's right-click menu.
//!
//! The menu is a ring of icon buttons around the click by default
//! ([`MenuStyle::Ring`]), or the classic glass list. Its items keep their
//! order and on/off state here; the built-in actions are always in the
//! list (a missing one is put back at the end, switched on), so an action
//! added in a later version shows up in a menu someone already arranged.
//! Custom items run a program with its arguments directly, never through
//! a shell ([`MenuItem::argv`]).
//!
//! Every field has a serde default: a file without `[desktop.menu]` opens
//! the ring with every item, as before only the list did.

use serde::{Deserialize, Serialize};

use crate::surfaces::Span;

/// Bounds of the menu's numeric settings.
pub mod ranges {
    use crate::surfaces::Span;

    /// Distance from the ring's centre to its items' centres, logical px.
    pub const RADIUS: Span = Span::new(80.0, 160.0, 2.0);
    /// Diameter of an item's button, logical px.
    pub const ITEM_SIZE: Span = Span::new(36.0, 56.0, 1.0);
    /// Opacity of the ring's glass.
    pub const OPACITY: Span = Span::new(0.6, 1.0, 0.01);
    /// The menu's own animation speed (0 = instant).
    pub const SPEED: Span = Span::new(0.0, 3.0, 0.05);
    /// Most custom items.
    pub const MAX_CUSTOM: usize = 8;
    /// Longest custom label, characters.
    pub const MAX_LABEL_CHARS: usize = 40;
    /// Most arguments of a custom command (the program included), and the
    /// longest one in bytes.
    pub const MAX_ARGS: usize = 32;
    pub const MAX_ARG_BYTES: usize = 4096;
}

/// Default values.
pub mod defaults {
    pub const RADIUS: f32 = 100.0;
    pub const ITEM_SIZE: f32 = 44.0;
    pub const OPACITY: f32 = 0.96;
    pub const SPEED: f32 = 1.0;
}

/// How the menu looks.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MenuStyle {
    /// Icon buttons on a ring around the click.
    #[default]
    Ring,
    /// The glass list of rows.
    Classic,
}

/// Where the ring names its items.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MenuLabels {
    /// The centre names the item under the pointer.
    #[default]
    Hub,
    /// A label beside the item under the pointer, outside the ring.
    Around,
    /// Every item has its name under its icon (the ring grows to fit).
    Always,
}

/// How the menu is used with the mouse.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MenuGesture {
    /// Right-click opens it; click an item.
    Click,
    /// Hold the right button, move toward an item and let go.
    Drag,
    /// Either: letting go on an item picks it, letting go where you pressed
    /// keeps the menu open.
    #[default]
    Both,
}

/// When "Add widget" opens its outer ring.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MenuSubmenu {
    /// As soon as the pointer is on it.
    #[default]
    Hover,
    /// On a click (or Enter).
    Click,
}

/// What paces the menu's animation.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MenuAnimation {
    /// Studio → Motion (speed, feel, reduced motion).
    #[default]
    Follow,
    /// [`DesktopMenuConfig::speed`] (reduced motion still applies).
    Own,
}

/// One item of the menu.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MenuItemId {
    EditDesktop,
    AddWidget,
    ToggleWidgets,
    ChangeWallpaper,
    NextWallpaper,
    Themes,
    Displays,
    Terminal,
    Screenshot,
    Studio,
    /// A program of your own ([`MenuItem::label`], `icon`, `command`).
    Custom,
    /// An id from a newer version: kept out of the menu.
    #[serde(other)]
    Unknown,
}

impl MenuItemId {
    /// The built-in items in their default order.
    pub const BUILTIN: [MenuItemId; 10] = [
        MenuItemId::EditDesktop,
        MenuItemId::AddWidget,
        MenuItemId::ToggleWidgets,
        MenuItemId::ChangeWallpaper,
        MenuItemId::NextWallpaper,
        MenuItemId::Themes,
        MenuItemId::Displays,
        MenuItemId::Terminal,
        MenuItemId::Screenshot,
        MenuItemId::Studio,
    ];

    pub fn is_builtin(self) -> bool {
        Self::BUILTIN.contains(&self)
    }
}

/// An item's place in the menu: on or off, and for a custom item what it
/// shows and runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MenuItem {
    pub id: MenuItemId,
    pub enabled: bool,
    /// Custom items: the name shown.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// Custom items: a bundled icon by name.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub icon: String,
    /// Custom items: the program and its arguments, run directly (no
    /// shell), in the home folder.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
}

impl Default for MenuItem {
    fn default() -> Self {
        Self { id: MenuItemId::Unknown, enabled: true, label: String::new(), icon: String::new(), command: Vec::new() }
    }
}

/// Why a custom item can't be saved.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CustomError {
    NoLabel,
    LongLabel,
    NoCommand,
    TooManyArgs,
    LongArg,
    /// A NUL byte, which no program argument can hold.
    Nul,
    /// An opening quote without its closing one.
    Unclosed,
}

impl MenuItem {
    pub fn builtin(id: MenuItemId) -> Self {
        Self { id, ..Self::default() }
    }

    /// A custom item, validated: `label`, an icon name and the command
    /// line `line` split into arguments ([`split_argv`]).
    pub fn custom(label: &str, icon: &str, line: &str) -> Result<Self, CustomError> {
        let item = Self { id: MenuItemId::Custom, enabled: true, label: label.trim().to_string(), icon: icon.trim().to_string(), command: split_argv(line)? };
        item.check()?;
        Ok(item)
    }

    /// A custom item's problem, if any (built-ins are always fine).
    pub fn check(&self) -> Result<(), CustomError> {
        if self.id != MenuItemId::Custom {
            return Ok(());
        }
        let label = self.label.trim();
        if label.is_empty() {
            return Err(CustomError::NoLabel);
        }
        if label.chars().count() > ranges::MAX_LABEL_CHARS {
            return Err(CustomError::LongLabel);
        }
        if self.command.first().is_none_or(|p| p.trim().is_empty()) {
            return Err(CustomError::NoCommand);
        }
        if self.command.len() > ranges::MAX_ARGS {
            return Err(CustomError::TooManyArgs);
        }
        if self.command.iter().any(|a| a.len() > ranges::MAX_ARG_BYTES) {
            return Err(CustomError::LongArg);
        }
        if self.command.iter().any(|a| a.contains('\0')) || self.label.contains('\0') {
            return Err(CustomError::Nul);
        }
        Ok(())
    }

    /// A valid custom item's program and arguments (`None` for built-ins
    /// and invalid ones). Run them with `Command::new(program).args(rest)`:
    /// never join them into a shell line.
    pub fn argv(&self) -> Option<(&str, &[String])> {
        self.check().ok()?;
        let (program, rest) = self.command.split_first()?;
        Some((program.as_str(), rest))
    }

    /// The command as one editable line, quoting what needs it (the
    /// inverse of [`split_argv`]).
    pub fn command_line(&self) -> String {
        let mut out = String::new();
        for (i, arg) in self.command.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            if !arg.is_empty() && !arg.chars().any(|c| c.is_whitespace() || matches!(c, '\'' | '"' | '\\')) {
                out.push_str(arg);
            } else {
                out.push('\'');
                for c in arg.chars() {
                    if c == '\'' {
                        out.push_str("'\\''");
                    } else {
                        out.push(c);
                    }
                }
                out.push('\'');
            }
        }
        out
    }
}

/// Splits a command line into arguments: spaces separate them, '…' keeps
/// text as it is, "…" too (with \" and \\ inside), a backslash outside
/// quotes keeps the next character. Nothing else is special: no variables,
/// globs, pipes or redirections, as no shell ever sees the line.
pub fn split_argv(line: &str) -> Result<Vec<String>, CustomError> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut has = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                has = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => cur.push(c),
                        None => return Err(CustomError::Unclosed),
                    }
                }
            }
            '"' => {
                has = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\')) => cur.push(c),
                            Some(c) => {
                                cur.push('\\');
                                cur.push(c);
                            }
                            None => return Err(CustomError::Unclosed),
                        },
                        Some(c) => cur.push(c),
                        None => return Err(CustomError::Unclosed),
                    }
                }
            }
            '\\' => {
                has = true;
                if let Some(c) = chars.next() {
                    cur.push(c);
                }
            }
            c if c.is_whitespace() => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => {
                has = true;
                cur.push(c);
            }
        }
    }
    if has {
        out.push(cur);
    }
    Ok(out)
}

/// `[desktop.menu]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopMenuConfig {
    pub style: MenuStyle,
    /// The items in menu order (see [`DesktopMenuConfig::items`]).
    pub items: Vec<MenuItem>,
    /// Ring: distance from the centre to the items, logical px.
    pub radius: f32,
    /// Ring: an item's button diameter, logical px.
    pub item_size: f32,
    pub labels: MenuLabels,
    pub gesture: MenuGesture,
    pub submenu: MenuSubmenu,
    /// Ring: opacity of the glass.
    pub opacity: f32,
    pub animation: MenuAnimation,
    /// With [`MenuAnimation::Own`]: the menu's playback speed (0 = instant).
    pub speed: f32,
}

impl Default for DesktopMenuConfig {
    fn default() -> Self {
        Self {
            style: MenuStyle::Ring,
            items: MenuItemId::BUILTIN.into_iter().map(MenuItem::builtin).collect(),
            radius: defaults::RADIUS,
            item_size: defaults::ITEM_SIZE,
            labels: MenuLabels::default(),
            gesture: MenuGesture::default(),
            submenu: MenuSubmenu::default(),
            opacity: defaults::OPACITY,
            animation: MenuAnimation::default(),
            speed: defaults::SPEED,
        }
    }
}

impl DesktopMenuConfig {
    /// The items in menu order: unknown ids, repeated built-ins and broken
    /// custom items left out, missing built-ins put back at the end
    /// (switched on), at most [`ranges::MAX_CUSTOM`] custom items. Switched
    /// off ones are included: check [`MenuItem::enabled`].
    pub fn items(&self) -> Vec<MenuItem> {
        let mut out: Vec<MenuItem> = Vec::with_capacity(self.items.len().max(MenuItemId::BUILTIN.len()));
        let mut custom = 0;
        for item in &self.items {
            match item.id {
                MenuItemId::Unknown => {}
                MenuItemId::Custom => {
                    if custom < ranges::MAX_CUSTOM && item.check().is_ok() {
                        custom += 1;
                        out.push(item.clone());
                    }
                }
                id if out.iter().any(|o| o.id == id) => {}
                _ => out.push(item.clone()),
            }
        }
        for id in MenuItemId::BUILTIN {
            if !out.iter().any(|o| o.id == id) {
                out.push(MenuItem::builtin(id));
            }
        }
        out
    }

    /// Writes `items` back in the normalized form ([`Self::items`]).
    pub fn normalize(&mut self) {
        self.items = self.items();
    }

    /// Moves item `at` (an index into [`Self::items`]) one place earlier
    /// (`up`) or later; returns whether it moved.
    pub fn move_item(&mut self, at: usize, up: bool) -> bool {
        self.normalize();
        let to = if up { at.checked_sub(1) } else { Some(at + 1).filter(|t| *t < self.items.len()) };
        match to {
            Some(to) if at < self.items.len() => {
                self.items.swap(at, to);
                true
            }
            _ => false,
        }
    }

    /// Switches item `at` on or off.
    pub fn set_enabled(&mut self, at: usize, on: bool) {
        self.normalize();
        if let Some(item) = self.items.get_mut(at) {
            item.enabled = on;
        }
    }

    /// Adds a custom item at the end; `false` when the list is full.
    pub fn add_custom(&mut self, item: MenuItem) -> bool {
        self.normalize();
        if item.id != MenuItemId::Custom || item.check().is_err() || self.custom_count() >= ranges::MAX_CUSTOM {
            return false;
        }
        self.items.push(item);
        true
    }

    /// Removes item `at` if it is a custom one.
    pub fn remove_custom(&mut self, at: usize) -> bool {
        self.normalize();
        if self.items.get(at).is_some_and(|i| i.id == MenuItemId::Custom) {
            self.items.remove(at);
            true
        } else {
            false
        }
    }

    pub fn custom_count(&self) -> usize {
        self.items.iter().filter(|i| i.id == MenuItemId::Custom).count()
    }

    pub fn radius(&self) -> f32 {
        ranges::RADIUS.clamp(self.radius, defaults::RADIUS)
    }

    pub fn item_size(&self) -> f32 {
        ranges::ITEM_SIZE.clamp(self.item_size, defaults::ITEM_SIZE)
    }

    pub fn opacity(&self) -> f32 {
        ranges::OPACITY.clamp(self.opacity, defaults::OPACITY)
    }

    /// The menu's playback speed against the shell's: `follow` (Studio →
    /// Motion's speed for popups) or its own.
    pub fn speed(&self, follow: f32) -> f32 {
        match self.animation {
            MenuAnimation::Follow => follow,
            MenuAnimation::Own => Span::clamp(ranges::SPEED, self.speed, defaults::SPEED),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ShellConfig;

    #[test]
    fn a_config_without_a_menu_section_opens_the_ring_with_every_item() {
        let cfg: ShellConfig = toml::from_str("[desktop.widgets]\nhidden = true\n").unwrap();
        assert!(cfg.desktop.widgets.hidden, "the rest of [desktop] still loads");
        let m = &cfg.desktop.menu;
        assert_eq!(*m, DesktopMenuConfig::default());
        assert_eq!(m.style, MenuStyle::Ring);
        let ids: Vec<MenuItemId> = m.items().iter().map(|i| i.id).collect();
        assert_eq!(ids, MenuItemId::BUILTIN);
        assert!(m.items().iter().all(|i| i.enabled));
        // And the default round-trips through the file format unchanged.
        let text = toml::to_string(&cfg).unwrap();
        let back: ShellConfig = toml::from_str(&text).unwrap();
        assert_eq!(back.desktop.menu, cfg.desktop.menu);
    }

    #[test]
    fn a_partial_menu_section_keeps_the_other_defaults() {
        let cfg: ShellConfig = toml::from_str("[desktop.menu]\nstyle = \"classic\"\nradius = 9999\n").unwrap();
        let m = &cfg.desktop.menu;
        assert_eq!(m.style, MenuStyle::Classic);
        assert_eq!(m.radius(), ranges::RADIUS.max, "hand-edited values are clamped where used");
        assert_eq!(m.item_size, defaults::ITEM_SIZE);
        assert_eq!(m.gesture, MenuGesture::Both);
        assert_eq!(m.items().len(), MenuItemId::BUILTIN.len());
    }

    #[test]
    fn items_keep_their_order_and_missing_ones_come_back() {
        let toml = r#"
            [[desktop.menu.items]]
            id = "studio"
            [[desktop.menu.items]]
            id = "terminal"
            enabled = false
            [[desktop.menu.items]]
            id = "teleport"
            [[desktop.menu.items]]
            id = "studio"
            [[desktop.menu.items]]
            id = "custom"
            label = "Files"
            icon = "folder"
            command = ["nautilus", "--new-window"]
            [[desktop.menu.items]]
            id = "custom"
            label = ""
            command = ["broken"]
        "#;
        let cfg: ShellConfig = toml::from_str(toml).unwrap();
        let items = cfg.desktop.menu.items();
        assert_eq!(items[0].id, MenuItemId::Studio);
        assert_eq!((items[1].id, items[1].enabled), (MenuItemId::Terminal, false));
        assert_eq!(items[2].id, MenuItemId::Custom, "unknown and repeated ids are skipped");
        assert_eq!(items[2].argv(), Some(("nautilus", &["--new-window".to_string()][..])));
        assert_eq!(items.iter().filter(|i| i.id == MenuItemId::Custom).count(), 1, "the broken custom item is left out");
        for id in MenuItemId::BUILTIN {
            assert_eq!(items.iter().filter(|i| i.id == id).count(), 1, "{id:?} exactly once");
        }
    }

    #[test]
    fn moving_toggling_and_removing_items() {
        let mut m = DesktopMenuConfig::default();
        assert!(!m.move_item(0, true), "the first can't go up");
        assert!(m.move_item(0, false));
        assert_eq!(m.items[1].id, MenuItemId::EditDesktop);
        assert!(!m.move_item(m.items.len() - 1, false));
        m.set_enabled(0, false);
        assert!(!m.items[0].enabled);
        assert!(!m.remove_custom(0), "built-ins stay");
        assert!(m.add_custom(MenuItem::custom("Files", "folder", "nautilus").unwrap()));
        let at = m.items.len() - 1;
        assert!(m.remove_custom(at));
        for i in 0..ranges::MAX_CUSTOM {
            assert!(m.add_custom(MenuItem::custom(&format!("App {i}"), "box", "true").unwrap()));
        }
        assert!(!m.add_custom(MenuItem::custom("One more", "box", "true").unwrap()), "the list is full");
    }

    #[test]
    fn custom_commands_are_argv_never_a_shell_line() {
        assert_eq!(split_argv("  foot  -e   htop ").unwrap(), ["foot", "-e", "htop"]);
        assert_eq!(split_argv(r#"code "My Projects/app" 'a b' c\ d"#).unwrap(), ["code", "My Projects/app", "a b", "c d"]);
        assert_eq!(split_argv(r#"echo "say \"hi\"" ''"#).unwrap(), ["echo", "say \"hi\"", ""]);
        // Shell syntax is just text: no pipes, variables or globs.
        assert_eq!(split_argv("ls | rm -rf $HOME *").unwrap(), ["ls", "|", "rm", "-rf", "$HOME", "*"]);
        assert_eq!(split_argv("vim 'unclosed"), Err(CustomError::Unclosed));
        assert_eq!(split_argv("").unwrap(), Vec::<String>::new());
        assert_eq!(MenuItem::custom("Files", "folder", "   "), Err(CustomError::NoCommand));
        assert_eq!(MenuItem::custom(" ", "folder", "nautilus"), Err(CustomError::NoLabel));
        assert_eq!(MenuItem::custom(&"x".repeat(ranges::MAX_LABEL_CHARS + 1), "folder", "nautilus"), Err(CustomError::LongLabel));
        assert_eq!(MenuItem::custom("Files", "folder", &"a ".repeat(ranges::MAX_ARGS + 1)), Err(CustomError::TooManyArgs));
        let item = MenuItem { id: MenuItemId::Custom, label: "Nul".into(), command: vec!["a\0b".into()], ..MenuItem::default() };
        assert_eq!(item.check(), Err(CustomError::Nul));
        assert_eq!(item.argv(), None);
        // The editable line reads back to the same arguments.
        for line in ["foot -e htop", r#"code "My Projects/app""#, r#"echo "it's" \\"#, "x ''"] {
            let item = MenuItem::custom("T", "box", line).unwrap();
            assert_eq!(split_argv(&item.command_line()).unwrap(), item.command, "{line}");
        }
    }

    #[test]
    fn the_menu_speed_follows_motion_or_its_own() {
        let mut m = DesktopMenuConfig::default();
        assert_eq!(m.speed(1.7), 1.7);
        m.animation = MenuAnimation::Own;
        m.speed = 0.0;
        assert_eq!(m.speed(1.7), 0.0, "0 is instant");
        m.speed = f32::NAN;
        assert_eq!(m.speed(1.7), defaults::SPEED);
    }
}
