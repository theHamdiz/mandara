//! Line-level edits of the few config files Boot look changes, each
//! undoable on its own (Restore works even when you edited the file
//! since):
//!
//! * `/etc/default/grub` — only the keys a theme needs (`GRUB_THEME`,
//!   `GRUB_GFXMODE`, `GRUB_TERMINAL_OUTPUT`, `GRUB_BACKGROUND`,
//!   `GRUB_TERMINAL`), each changed line under a `# mandara-boot-look`
//!   marker, retired lines commented as `# mandara-boot-look: <line>`;
//!   or, where `grub-mkconfig` reads `/etc/default/grub.d`, a drop-in of
//!   Mandara's own ([`grub_dropin`]) and the main file untouched.
//! * Limine's config — global options in a marked block before the first
//!   entry; options it replaces are retired the same way. Entries are
//!   never touched (they belong to your distribution's entry tool).
//! * rEFInd's `refind.conf` — one `include themes/…` line in a marked
//!   block; other theme includes are retired.

use super::MARK;

/// The `KEY=value` assignments of a shell-style file (`/etc/default/grub`,
/// `/etc/os-release`, `/etc/default/limine`), last one winning. Values
/// are unquoted; nothing is evaluated.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShellVars {
    vars: Vec<(String, String)>,
}

impl ShellVars {
    pub fn parse(text: &str) -> Self {
        let mut out = Self::default();
        for line in text.lines() {
            if let Some((key, raw)) = assignment(line) {
                out.set(key, unquote(raw));
            }
        }
        out
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    fn set(&mut self, key: &str, value: String) {
        self.remove(key);
        self.vars.push((key.to_string(), value));
    }

    pub fn remove(&mut self, key: &str) {
        self.vars.retain(|(k, _)| k != key);
    }

    /// `other`'s assignments override these.
    pub fn overlay(&mut self, other: &ShellVars) {
        for (k, v) in &other.vars {
            self.set(k, v.clone());
        }
    }
}

/// `(key, raw value)` of an uncommented `KEY=value` / `export KEY=value`.
fn assignment(line: &str) -> Option<(&str, &str)> {
    let l = line.trim_start();
    let l = l.strip_prefix("export ").map(str::trim_start).unwrap_or(l);
    let (key, raw) = l.split_once('=')?;
    let ok = !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !key.starts_with(|c: char| c.is_ascii_digit());
    ok.then_some((key, raw.trim_end()))
}

fn unquote(raw: &str) -> String {
    let r = raw.trim();
    // A trailing `# comment` after a quoted value.
    for q in ['"', '\''] {
        if let Some(rest) = r.strip_prefix(q) && let Some(end) = rest.find(q) {
            return rest[..end].to_string();
        }
    }
    r.split(" #").next().unwrap_or(r).trim().to_string()
}

/// Whether `v` is safe inside double quotes in a sourced shell file (no
/// expansion, no quotes, no newlines).
pub fn shell_safe(v: &str) -> bool {
    v.len() <= 512 && v.chars().all(|c| c.is_ascii_alphanumeric() || " /._,:+-@".contains(c))
}

fn marker() -> String {
    format!("# {MARK}")
}

fn retired_prefix() -> String {
    format!("# {MARK}: ")
}

/// A key's previous raw value (`None`: it wasn't set).
pub type Previous = Vec<(String, Option<String>)>;

/// Sets (`Some`) or retires (`None`) `keys` in `/etc/default/grub` text.
/// Returns the new text and each key's previous raw value, for
/// [`grub_restore`].
pub fn grub_set(text: &str, keys: &[(String, Option<String>)]) -> Result<(String, Previous), String> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut previous = Vec::new();
    for (key, value) in keys {
        if let Some(v) = value && !shell_safe(v) {
            return Err(format!("{key}: value {v:?} has characters Mandara doesn't write into a shell file"));
        }
        let at: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| assignment(l).is_some_and(|(k, _)| k == key)).map(|(i, _)| i).collect();
        previous.push((key.clone(), at.last().and_then(|i| assignment(&lines[*i]).map(|(_, raw)| raw.to_string()))));
        match value {
            Some(v) => {
                let line = format!("{key}=\"{v}\"");
                match at.last() {
                    Some(&i) if lines.get(i.wrapping_sub(1)).is_some_and(|m| *m == marker()) => lines[i] = line,
                    Some(&i) => {
                        lines[i] = line;
                        lines.insert(i, marker());
                    }
                    None => {
                        lines.push(marker());
                        lines.push(line);
                    }
                }
            }
            None => {
                for i in at {
                    lines[i] = format!("{}{}", retired_prefix(), lines[i]);
                }
            }
        }
    }
    Ok((join(lines, text), previous))
}

/// Undoes [`grub_set`] key by key: retired lines come back, Mandara's
/// lines go, and each key gets its previous value again.
pub fn grub_restore(text: &str, previous: &Previous) -> String {
    let mut lines: Vec<String> = text.lines().map(|l| l.strip_prefix(&retired_prefix()).map(str::to_string).unwrap_or_else(|| l.to_string())).collect();
    for (key, raw) in previous {
        let is_key = |l: &str| assignment(l).is_some_and(|(k, _)| k == key);
        // Mandara's own line for the key: the one under a marker.
        let ours = (1..lines.len()).rev().find(|&i| is_key(&lines[i]) && lines[i - 1] == marker());
        match (ours, raw) {
            // Changed in place: its old value goes back where it was.
            (Some(i), Some(raw)) => {
                lines[i] = format!("{key}={raw}");
                lines.remove(i - 1);
            }
            (Some(i), None) => {
                lines.remove(i);
                lines.remove(i - 1);
            }
            (None, Some(raw)) => {
                if !lines.iter().any(|l| is_key(l)) {
                    lines.push(format!("{key}={raw}"));
                }
            }
            (None, None) => {}
        }
    }
    lines.retain(|l| *l != marker());
    join(lines, text)
}

/// The GRUB drop-in Mandara owns (`/etc/default/grub.d/99-mandara-boot-look.cfg`):
/// `None` values clear the key (an empty value is how `grub-mkconfig`
/// reads "unset").
pub fn grub_dropin(keys: &[(String, Option<String>)]) -> Result<String, String> {
    let mut out = format!("# {MARK}: written by Mandara Boot look (Studio → Themes → Boot).\n# Restore removes this file; it overrides /etc/default/grub for these keys only.\n");
    for (key, value) in keys {
        let v = value.as_deref().unwrap_or("");
        if !shell_safe(v) {
            return Err(format!("{key}: value {v:?} has characters Mandara doesn't write into a shell file"));
        }
        out.push_str(&format!("{key}=\"{v}\"\n"));
    }
    Ok(out)
}

fn join(lines: Vec<String>, original: &str) -> String {
    let mut s = lines.join("\n");
    if original.ends_with('\n') || original.is_empty() {
        s.push('\n');
    }
    s
}

fn block_open() -> String {
    format!("# >>> {MARK} (Studio → Themes → Boot; Restore removes this block)")
}

fn block_close() -> String {
    format!("# <<< {MARK}")
}

/// Removes Mandara's block and brings retired lines back.
fn unmark(text: &str) -> Vec<String> {
    let (open, close) = (block_open(), block_close());
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line == open || line.starts_with(&format!("# >>> {MARK}")) {
            inside = true;
        } else if line == close {
            inside = false;
        } else if !inside {
            out.push(line.strip_prefix(&retired_prefix()).unwrap_or(line).to_string());
        }
    }
    out
}

/// A Limine option name, aliases folded (`_color` → `_colour`).
fn limine_key(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace("_color", "_colour").replace("randomize", "randomise")
}

/// Sets Limine global `options` (name, value): existing global lines for
/// them are retired and a marked block goes right before the first entry.
pub fn limine_set(text: &str, options: &[(String, String)]) -> Result<String, String> {
    for (k, v) in options {
        if k.is_empty() || !k.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
            return Err(format!("{k:?} isn't a Limine option name"));
        }
        if v.contains(['\n', '\r']) || v.chars().any(char::is_control) || v.len() > 1024 {
            return Err(format!("{k}: the value can't hold control characters"));
        }
    }
    let mut lines = unmark(text);
    let first_entry = lines.iter().position(|l| l.trim_start().starts_with('/')).unwrap_or(lines.len());
    let wanted: Vec<String> = options.iter().map(|(k, _)| limine_key(k)).collect();
    for line in lines.iter_mut().take(first_entry) {
        let t = line.trim_start();
        if t.starts_with('#') {
            continue;
        }
        if let Some((name, _)) = t.split_once(':') && wanted.contains(&limine_key(name)) {
            *line = format!("{}{line}", retired_prefix());
        }
    }
    let mut block = vec![block_open()];
    block.extend(options.iter().map(|(k, v)| format!("{k}: {v}")));
    block.push(block_close());
    // Right after the other global options; blank lines before the
    // entries stay where they are.
    let mut at = first_entry;
    while at > 0 && lines[at - 1].trim().is_empty() {
        at -= 1;
    }
    lines.splice(at..at, block);
    Ok(join(lines, text))
}

/// Undoes [`limine_set`].
pub fn limine_restore(text: &str) -> String {
    join(unmark(text), text)
}

/// Points rEFInd at `include` (`themes/<name>/theme.conf`), retiring other
/// theme includes.
pub fn refind_set(text: &str, include: &str) -> Result<String, String> {
    if !include.starts_with("themes/") || include.contains("..") || !include.chars().all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c)) {
        return Err(format!("{include:?} isn't a theme include Mandara writes"));
    }
    let mut lines = unmark(text);
    for line in &mut lines {
        let t = line.trim_start();
        if t.starts_with("include") && t["include".len()..].trim_start().starts_with("themes/") {
            *line = format!("{}{line}", retired_prefix());
        }
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    lines.push(String::new());
    lines.push(block_open());
    lines.push(format!("include {include}"));
    lines.push(block_close());
    let mut s = lines.join("\n");
    s.push('\n');
    Ok(s)
}

/// Undoes [`refind_set`].
pub fn refind_restore(text: &str) -> String {
    let mut lines = unmark(text);
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    let mut s = lines.join("\n");
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARCH_GRUB: &str = "# GRUB boot loader configuration\n\nGRUB_DEFAULT=0\nGRUB_TIMEOUT=5\nGRUB_DISTRIBUTOR=\"Arch\"\nGRUB_CMDLINE_LINUX_DEFAULT=\"loglevel=3 quiet\"\n\n# Uncomment to use basic console\n#GRUB_TERMINAL_INPUT=console\nGRUB_TERMINAL_OUTPUT=console\n\nGRUB_GFXMODE=auto\n#GRUB_THEME=\"/path/to/gfxtheme\"\n";

    fn keys(pairs: &[(&str, Option<&str>)]) -> Vec<(String, Option<String>)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.map(str::to_string))).collect()
    }

    #[test]
    fn shell_vars_read_like_the_shell_would() {
        let v = ShellVars::parse("A=1\nexport B=\"two words\" # note\nC='x'\n#D=no\nKERNEL_CMDLINE[default]+=quiet\nA=3\n");
        assert_eq!(v.get("A").as_deref(), Some("3"));
        assert_eq!(v.get("B").as_deref(), Some("two words"));
        assert_eq!(v.get("C").as_deref(), Some("x"));
        assert_eq!(v.get("D"), None);
        assert_eq!(v.get("KERNEL_CMDLINE"), None);
    }

    #[test]
    fn grub_keys_change_and_restore_exactly() {
        let set = keys(&[("GRUB_THEME", Some("/boot/grub/themes/mandara-vimix/theme.txt")), ("GRUB_GFXMODE", Some("1920x1080,auto")), ("GRUB_TERMINAL_OUTPUT", Some("gfxterm")), ("GRUB_BACKGROUND", None)]);
        let (out, prev) = grub_set(ARCH_GRUB, &set).unwrap();
        let v = ShellVars::parse(&out);
        assert_eq!(v.get("GRUB_THEME").as_deref(), Some("/boot/grub/themes/mandara-vimix/theme.txt"));
        assert_eq!(v.get("GRUB_GFXMODE").as_deref(), Some("1920x1080,auto"));
        assert_eq!(v.get("GRUB_TERMINAL_OUTPUT").as_deref(), Some("gfxterm"));
        assert_eq!(v.get("GRUB_DEFAULT").as_deref(), Some("0"), "other keys untouched");
        assert!(out.contains("GRUB_CMDLINE_LINUX_DEFAULT=\"loglevel=3 quiet\""), "the kernel command line is never touched");
        assert_eq!(prev, keys(&[("GRUB_THEME", None), ("GRUB_GFXMODE", Some("auto")), ("GRUB_TERMINAL_OUTPUT", Some("console")), ("GRUB_BACKGROUND", None)]));
        assert_eq!(grub_restore(&out, &prev), ARCH_GRUB);
        // Applying twice keeps one marker per key and restores to the first state.
        let (again, prev2) = grub_set(&out, &keys(&[("GRUB_THEME", Some("/boot/grub/themes/mandara-tela/theme.txt"))])).unwrap();
        assert_eq!(again.matches(&marker()).count(), out.matches(&marker()).count());
        assert_eq!(prev2[0].1.as_deref(), Some("\"/boot/grub/themes/mandara-vimix/theme.txt\""));
    }

    #[test]
    fn grub_retires_and_brings_back_a_console_terminal() {
        let text = "GRUB_TIMEOUT=5\nGRUB_TERMINAL=console\nGRUB_BACKGROUND=\"/usr/share/images/desktop-base/desktop-grub.png\"\n";
        let (out, prev) = grub_set(text, &keys(&[("GRUB_TERMINAL", None), ("GRUB_BACKGROUND", None), ("GRUB_THEME", Some("/boot/grub/themes/mandara-image/theme.txt"))])).unwrap();
        let v = ShellVars::parse(&out);
        assert_eq!(v.get("GRUB_TERMINAL"), None);
        assert_eq!(v.get("GRUB_BACKGROUND"), None);
        assert!(out.contains("# mandara-boot-look: GRUB_TERMINAL=console"));
        assert_eq!(grub_restore(&out, &prev), text);
    }

    #[test]
    fn grub_values_never_carry_shell() {
        for bad in ["/x\"; rm -rf /", "$(id)", "`id`", "a\nb", "a\\b"] {
            assert!(grub_set("", &keys(&[("GRUB_THEME", Some(bad))])).is_err(), "{bad}");
            assert!(grub_dropin(&keys(&[("GRUB_THEME", Some(bad))])).is_err(), "{bad}");
        }
        let d = grub_dropin(&keys(&[("GRUB_THEME", Some("/boot/grub/themes/mandara-x/theme.txt")), ("GRUB_BACKGROUND", None)])).unwrap();
        let v = ShellVars::parse(&d);
        assert_eq!(v.get("GRUB_THEME").as_deref(), Some("/boot/grub/themes/mandara-x/theme.txt"));
        assert_eq!(v.get("GRUB_BACKGROUND").as_deref(), Some(""));
    }

    const LIMINE: &str = "timeout: 5\nwallpaper: boot():/old.png\ninterface_branding_color: 2\n\n/+CachyOS\ncomment: machine-id=abc\n  //linux-cachyos\n    protocol: linux\n    kernel_path: boot():/vmlinuz-linux-cachyos\n    wallpaper: boot():/entry-local.png\n";

    #[test]
    fn limine_global_options_round_trip_and_leave_entries_alone() {
        let opts = vec![("wallpaper".to_string(), "boot():/mandara-boot-look/wallpaper.png".to_string()), ("interface_branding_colour".to_string(), "7cd5c4".to_string()), ("backdrop".to_string(), "0b1026".to_string())];
        let out = limine_set(LIMINE, &opts).unwrap();
        let entry_at = out.lines().position(|l| l.starts_with('/')).unwrap();
        let block_at = out.lines().position(|l| l.starts_with("# >>>")).unwrap();
        assert!(block_at < entry_at, "global options come before the first entry");
        assert!(out.contains("# mandara-boot-look: wallpaper: boot():/old.png"));
        assert!(out.contains("# mandara-boot-look: interface_branding_color: 2"), "the US spelling is the same option");
        assert!(out.contains("    wallpaper: boot():/entry-local.png"), "entry-local lines stay");
        assert!(out.contains("timeout: 5"));
        assert_eq!(limine_restore(&out), LIMINE);
        // Re-applying with fewer options brings the others back.
        let again = limine_set(&out, &opts[1..]).unwrap();
        assert!(again.contains("\nwallpaper: boot():/old.png\n"));
        assert_eq!(limine_restore(&again), LIMINE);
        assert!(limine_set(LIMINE, &[("wallpaper".into(), "x\n/evil entry".into())]).is_err());
        assert!(limine_set(LIMINE, &[("Bad Key".into(), "1".into())]).is_err());
        // A config with no entries yet.
        let bare = limine_set("timeout: 3\n", &opts[2..]).unwrap();
        assert_eq!(limine_restore(&bare), "timeout: 3\n");
    }

    #[test]
    fn refind_include_round_trips() {
        let text = "timeout 20\nuse_graphics_for linux\ninclude themes/old/theme.conf\n";
        let out = refind_set(text, "themes/rEFInd-minimal/theme.conf").unwrap();
        assert!(out.contains("# mandara-boot-look: include themes/old/theme.conf"));
        assert!(out.trim_end().ends_with("# <<< mandara-boot-look"));
        assert_eq!(refind_restore(&out), text);
        assert!(refind_set(text, "../../etc/x").is_err());
        assert!(refind_set(text, "themes/a b/theme.conf").is_err());
    }
}
