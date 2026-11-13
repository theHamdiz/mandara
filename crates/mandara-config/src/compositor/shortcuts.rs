//! Refresh only managed bindings while preserving display and appearance bytes.

use std::{ops::Range, path::Path};

use anyhow::{Result, ensure};
use mandara_i18n::t;

use super::{ManagedConfig, Target};

fn region(target: Target, lua: bool, text: &str) -> Option<Range<usize>> {
    let mut offset = 0;
    let mut begin = None;
    let mut legacy_end = None;
    for raw in text.split_inclusive('\n') {
        let line = raw.trim();
        let comment = line.starts_with('#') || line.starts_with("//") || line.starts_with("--");
        if begin.is_none()
            && ((comment && line.ends_with(" Mandara keybinds"))
                || (target == Target::Niri && line == "binds {")
                || (target == Target::Hyprland && !lua && line.starts_with("bind = "))
                || (target == Target::Hyprland && lua && line.starts_with("pcall(hl.bind,")))
        {
            begin = Some(offset);
        } else if let Some(begin) = begin {
            if comment && line.ends_with(" End Mandara keybinds") {
                return Some(begin..offset + raw.len());
            }
            // Pre-marker releases put includes after the binding tail. Umbriel
            // also puts layer/window rules after its [keybinds] table.
            if (comment && line.contains(" Mandara Shell "))
                || (target == Target::Umbriel
                    && (line.starts_with("# Shell surfaces") || line.starts_with('[')))
                    && line != "[keybinds]"
            {
                legacy_end.get_or_insert(offset);
            }
        }
        offset += raw.len();
    }
    begin.map(|start| start..legacy_end.unwrap_or(text.len()))
}

pub fn replace(
    target: Target,
    file: &Path,
    previous: &str,
    managed: &ManagedConfig<'_>,
) -> Result<String> {
    ensure!(
        crate::compositor_theme::supported(target),
        "{}",
        t("This compositor manages shortcuts in its own settings.")
    );
    ensure!(
        previous
            .lines()
            .next()
            .is_some_and(|line| line.contains(super::HEADER)),
        "{}",
        t("The managed compositor file has changed. Run Config doctor before editing shortcuts.")
    );
    let lua = file.extension().is_some_and(|extension| extension == "lua");
    let fresh = match target {
        Target::Hyprland if lua => super::emit_hyprland_lua(managed),
        Target::Hyprland => super::emit_hyprland_conf(managed),
        _ => super::emit(target, managed),
    };
    let next = region(target, lua, &fresh.text).expect("every managed emitter marks its bindings");
    let old = region(target, lua, previous).unwrap_or(previous.len()..previous.len());
    let mut result = String::with_capacity(previous.len() - old.len() + next.len());
    result.push_str(&previous[..old.start]);
    if old.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }
    result.push_str(&fresh.text[next]);
    result.push_str(&previous[old.end..]);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::{BindAction, CompositorConfig, Keybind};

    #[test]
    fn explicit_end_markers_take_precedence_over_comments_and_command_text() {
        let text = "# Mandara keybinds\nbind=SUPER,Q,spawn,notify-send ' Mandara Shell ready'\n# Mandara Shell note\nbind=SUPER,W,spawn,foot\n# End Mandara keybinds\n# keep\n";
        let span = region(Target::Mango, false, text).unwrap();
        assert_eq!(&text[span.end..], "# keep\n");
        assert!(text[span].contains("spawn,foot"));
        let legacy = text.replace("# End Mandara keybinds\n", "");
        let span = region(Target::Mango, false, &legacy).unwrap();
        assert!(legacy[span].contains("notify-send"));
    }

    #[test]
    fn swapping_bindings_keeps_every_other_byte_and_attached_fragment() {
        for (target, file) in [
            (Target::Mango, "mango.conf"),
            (Target::Niri, "niri.kdl"),
            (Target::Hyprland, "hyprland.conf"),
            (Target::Hyprland, "hyprland.lua"),
            (Target::Umbriel, "umbriel.toml"),
            (Target::Sway, "sway.conf"),
        ] {
            let config = CompositorConfig {
                keybinds: vec![Keybind::new(
                    &["super"],
                    "Q",
                    BindAction::Spawn("notify-send ' Mandara Shell ready'".into()),
                )],
                ..Default::default()
            };
            let mut managed = ManagedConfig {
                config: &config,
                borders: None,
                existing_chords: Default::default(),
                mandara_bin: "/usr/bin/mandara".into(),
                version: None,
                user_autostarts: true,
            };
            let file = Path::new(file);
            let lua = file.extension().unwrap() == "lua";
            let emitted = if target == Target::Hyprland {
                if lua {
                    super::super::emit_hyprland_lua(&managed)
                } else {
                    super::super::emit_hyprland_conf(&managed)
                }
            } else {
                super::super::emit(target, &managed)
            };
            let suffix = if target == Target::Niri {
                "// Mandara Shell saved displays\ninclude \"saved.kdl\"\n// End Mandara Shell saved displays\n"
            } else {
                "# Mandara Shell saved displays\n# unchanged fragment\n# End Mandara Shell saved displays\n"
            };
            let original = format!("{}{suffix}", emitted.text);
            let range = region(target, lua, &original).unwrap();
            managed
                .existing_chords
                .insert(super::super::target_chord(target, &config.keybinds[0]));
            let updated = replace(target, file, &original, &managed).unwrap();
            let updated_range = region(target, lua, &updated).unwrap();
            assert_eq!(
                &original[..range.start],
                &updated[..updated_range.start],
                "{target:?}"
            );
            assert_eq!(
                &original[range.end..],
                &updated[updated_range.end..],
                "{target:?}"
            );
            assert!(
                !updated[updated_range.clone()].lines().any(|line| {
                    let line = line.trim();
                    !line.starts_with('#')
                        && !line.starts_with("//")
                        && !line.starts_with("--")
                        && line.contains("notify-send")
                }),
                "{target:?}: {updated}"
            );
            assert!(updated.ends_with(suffix));
            managed.existing_chords.clear();
            assert_eq!(replace(target, file, &updated, &managed).unwrap(), original);
            let legacy = original
                .lines()
                .filter(|line| {
                    !line.contains("End Mandara keybinds")
                        && (!matches!(target, Target::Niri | Target::Hyprland)
                            || !line.ends_with(" Mandara keybinds"))
                })
                .map(|line| format!("{line}\n"))
                .collect::<String>();
            let refreshed = replace(target, file, &legacy, &managed).unwrap();
            assert!(refreshed.ends_with(suffix), "{target:?}");
            if target == Target::Umbriel {
                assert!(refreshed.contains("# Shell surfaces"));
            }
        }
    }
}
