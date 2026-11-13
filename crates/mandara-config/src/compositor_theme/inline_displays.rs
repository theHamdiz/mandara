//! Saved displays have one owner when their independent fragment is enabled.

use crate::compositor::Target;
use anyhow::Result;
use std::path::Path;

pub(super) fn remove(
    target: Target,
    body: &str,
    fragment: &Path,
    previous: Option<&str>,
) -> Result<String> {
    let heading = "Displays (Studio → Displays)";
    let ending = "End Displays (Studio → Displays)";
    let Some(start) = body
        .lines()
        .position(|line| line.ends_with(heading) && !line.contains(ending))
    else {
        return Ok(body.into());
    };
    if let Some(end) = body.lines().position(|line| line.ends_with(ending)) {
        return Ok(body
            .lines()
            .enumerate()
            .filter(|(index, _)| *index < start || *index > end)
            .fold(String::with_capacity(body.len()), |mut body, (_, line)| {
                body.push_str(line);
                body.push('\n');
                body
            }));
    }
    // Older generated files lack an end marker. Leave all preceding text and
    // unrelated connectors intact; only migrate selectors in the saved fragment.
    use std::io::Read as _;
    const MAX_FRAGMENT_BYTES: u64 = 8 * 1024 * 1024;
    let file = std::fs::File::open(fragment)?;
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "display fragment is not a regular file"
    );
    let mut fragment_body = String::new();
    file.take(MAX_FRAGMENT_BYTES + 1)
        .read_to_string(&mut fragment_body)?;
    anyhow::ensure!(
        fragment_body.len() as u64 <= MAX_FRAGMENT_BYTES,
        "display fragment is too large"
    );
    let offset: usize = body.split_inclusive('\n').take(start).map(str::len).sum();
    let (prefix, suffix) = body.split_at(offset);
    let mut names = selectors(target, &fragment_body);
    if let Some(previous) = previous {
        names.extend(selectors(target, previous));
    }
    names.sort();
    names.dedup();
    let mut result = String::with_capacity(body.len());
    result.push_str(prefix);
    if target == Target::Niri {
        let mut document = kdl::KdlDocument::parse_v1(suffix)?;
        document.nodes_mut().retain(|node| {
            node.name().value() != "output"
                || !node
                    .get(0)
                    .and_then(|value| value.as_string())
                    .is_some_and(|name| names.iter().any(|owned| owned == name))
        });
        document.ensure_v1();
        result.push_str(&document.to_string());
    } else if target == Target::Umbriel {
        let mut document: toml_edit::DocumentMut = suffix.parse()?;
        if let Some(outputs) = document
            .get_mut("output")
            .and_then(toml_edit::Item::as_table_like_mut)
        {
            for name in names {
                outputs.remove(&name);
            }
            if outputs.is_empty() {
                document.remove("output");
            }
        }
        result.push_str(&document.to_string());
    } else {
        for line in suffix.lines() {
            let matches = selectors(target, line)
                .iter()
                .any(|name| names.contains(name));
            // Wallpaper and other output rules were never emitted by Displays.
            let user_sway_rule = target == Target::Sway
                && line.split_whitespace().skip(2).any(|word| {
                    matches!(
                        word,
                        "bg" | "background" | "color_profile" | "max_render_time"
                    )
                });
            if matches && !user_sway_rule {
                continue;
            }
            result.push_str(line);
            result.push('\n');
        }
    }
    Ok(result)
}

fn selectors(target: Target, body: &str) -> Vec<String> {
    if target == Target::Hyprland
        && body
            .lines()
            .any(|line| line.trim_start().starts_with("pcall(hl.monitor,"))
    {
        body.lines()
            .filter_map(|line| {
                let value = line
                    .trim_start()
                    .strip_prefix("pcall(hl.monitor,")?
                    .split_once("output = ")?
                    .1;
                // Generated Lua uses JSON-compatible quoted connector names.
                serde_json::Deserializer::from_str(value)
                    .into_iter::<String>()
                    .next()?
                    .ok()
            })
            .collect()
    } else {
        crate::outputs::parse_output_rules(target, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outputs::{OutputConfig, emit_outputs};
    #[test]
    fn previous_ownership_migrates_old_connectors_after_a_new_fragment_replaces_them() {
        let root = tempfile::tempdir().unwrap();
        for (target, lua) in [
            (Target::Mango, false),
            (Target::Niri, false),
            (Target::Hyprland, false),
            (Target::Hyprland, true),
            (Target::Umbriel, false),
            (Target::Sway, false),
        ] {
            let old = emit_outputs(
                target,
                &[OutputConfig {
                    name: "DP-1".into(),
                    enabled: false,
                    ..Default::default()
                }],
                lua,
            );
            let legacy = old
                .lines()
                .filter(|line| !line.contains("End Displays"))
                .fold(String::new(), |mut body, line| {
                    body.push_str(line);
                    body.push('\n');
                    body
                });
            let next = emit_outputs(
                target,
                &[OutputConfig {
                    name: "DP-2".into(),
                    scale: Some(1.25),
                    ..Default::default()
                }],
                lua,
            );
            let file = root.path().join("fragment");
            std::fs::write(&file, next).unwrap();
            let migrated = remove(target, &legacy, &file, Some(&old)).unwrap();
            assert!(!migrated.contains("DP-1"), "{target:?}: {migrated}");
        }
    }

    #[test]
    fn legacy_migration_preserves_rules_before_the_heading_and_unrelated_outputs() {
        let root = tempfile::tempdir().unwrap();
        for (target, lua, before, unrelated) in [
            (
                Target::Mango,
                false,
                "monitor_rule=name:^DP-1$,scale:2\n",
                "monitor_rule=name:^HDMI-A-1$,scale:2\n",
            ),
            (
                Target::Hyprland,
                false,
                "monitor = DP-1, preferred, auto, 2\n",
                "monitor = HDMI-A-1, preferred, auto, 2\n",
            ),
            (
                Target::Hyprland,
                true,
                "pcall(hl.monitor, { output = \"DP-1\", scale = 2 })\n",
                "pcall(hl.monitor, { output = \"HDMI-A-1\", scale = 2 })\n",
            ),
            (
                Target::Sway,
                false,
                "output DP-1 scale 2\n",
                "output * bg /tmp/image fill\noutput HDMI-A-1 scale 2\noutput DP-1 bg /tmp/image fill\n",
            ),
            (
                Target::Niri,
                false,
                "output \"DP-1\" { scale 2; }\n",
                "output \"HDMI-A-1\" { scale 2; }\n",
            ),
            (
                Target::Umbriel,
                false,
                "# keep this prefix\n",
                "[output.\"HDMI-A-1\"]\nscale = 2\n",
            ),
        ] {
            let output = OutputConfig {
                name: "DP-1".into(),
                scale: Some(1.25),
                ..Default::default()
            };
            let fragment = emit_outputs(target, &[output], lua);
            let file = root.path().join("fragment");
            std::fs::write(&file, &fragment).unwrap();
            let legacy = fragment
                .lines()
                .filter(|line| !line.contains("End Displays"))
                .fold(String::new(), |mut body, line| {
                    body.push_str(line);
                    body.push('\n');
                    body
                });
            let result =
                remove(target, &format!("{before}{legacy}{unrelated}"), &file, None).unwrap();
            assert!(result.starts_with(before), "{target:?}: {result}");
            assert!(result.contains(unrelated.trim()), "{target:?}: {result}");
            assert!(
                !result[before.len()..].contains("1.25"),
                "{target:?}: {result}"
            );
        }
    }

    #[test]
    fn fragments_replace_inline_settings_and_preserve_other_generated_settings() {
        let output = OutputConfig {
            name: "DP-1".into(),
            scale: Some(1.25),
            ..Default::default()
        };
        for (target, lua, unrelated) in [
            (Target::Mango, false, "borderpx=3\n"),
            (Target::Niri, false, "output \"eDP-1\" { scale 2; }\n"),
            (Target::Hyprland, false, "general { border_size = 3 }\n"),
            (Target::Hyprland, true, "hl.general({ border_size = 3 })\n"),
            (Target::Umbriel, false, "[general]\nborder_width = 3\n"),
            (Target::Sway, false, "set $mod Mod4\n"),
        ] {
            let fragment = emit_outputs(target, std::slice::from_ref(&output), lua);
            let file = tempfile::NamedTempFile::new().unwrap();
            std::fs::write(file.path(), &fragment).unwrap();
            let body = format!("{fragment}\n{unrelated}");
            let stripped = remove(target, &body, file.path(), None).unwrap();
            assert!(
                !stripped.contains("output \"DP-1\" ")
                    && !stripped.contains("name:^DP-1$")
                    && !stripped.contains("output DP-1"),
                "{target:?}: {stripped}"
            );
            assert!(
                stripped.contains(unrelated.trim()),
                "{target:?}: {stripped}"
            );
            let legacy = body
                .lines()
                .filter(|line| !line.contains("End Displays"))
                .collect::<Vec<_>>()
                .join("\n");
            let stripped = remove(target, &legacy, file.path(), None).unwrap();
            assert!(
                !stripped.contains("\"DP-1\"")
                    && !stripped.contains("name:^DP-1$")
                    && !stripped.contains("output DP-1"),
                "{target:?}: {stripped}"
            );
            assert!(
                stripped.contains(unrelated.trim()),
                "{target:?}: {stripped}"
            );
        }
    }
}
