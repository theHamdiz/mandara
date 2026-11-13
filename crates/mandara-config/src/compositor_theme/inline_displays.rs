//! Saved displays have one owner when their independent fragment is enabled.

use crate::compositor::Target;
use anyhow::Result;
use std::path::Path;

pub(super) fn remove(target: Target, body: &str, fragment: &Path) -> Result<String> {
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
    // Upgrade generated files written before the explicit end marker existed.
    // Only the older Niri-specific list can contain additional output settings.
    if target == Target::Niri {
        use std::io::Read as _;
        const MAX_FRAGMENT_BYTES: u64 = 8 * 1024 * 1024;
        let mut fragment_body = String::new();
        std::fs::File::open(fragment)?
            .take(MAX_FRAGMENT_BYTES + 1)
            .read_to_string(&mut fragment_body)?;
        anyhow::ensure!(
            fragment_body.len() as u64 <= MAX_FRAGMENT_BYTES,
            "display fragment is too large"
        );
        let outputs = kdl::KdlDocument::parse_v1(&fragment_body)?;
        let names: Vec<_> = outputs
            .nodes()
            .iter()
            .filter(|node| node.name().value() == "output")
            .filter_map(|node| node.get(0)?.as_string())
            .collect();
        let mut document = kdl::KdlDocument::parse_v1(body)?;
        document.nodes_mut().retain(|node| {
            node.name().value() != "output"
                || !node
                    .get(0)
                    .and_then(|value| value.as_string())
                    .is_some_and(|name| names.contains(&name))
        });
        document.ensure_v1();
        return Ok(document.to_string());
    }
    if target == Target::Umbriel {
        let mut document: toml_edit::DocumentMut = body.parse()?;
        document.remove("output");
        return Ok(document.to_string());
    }
    Ok(body
        .lines()
        .filter(|line| {
            let line = line.trim_start();
            !match target {
                Target::Mango => line.starts_with("monitor_rule="),
                Target::Hyprland => {
                    line.starts_with("monitor = ") || line.starts_with("pcall(hl.monitor,")
                }
                Target::Sway => line.starts_with("output "),
                _ => false,
            }
        })
        .fold(String::with_capacity(body.len()), |mut body, line| {
            body.push_str(line);
            body.push('\n');
            body
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outputs::{OutputConfig, emit_outputs};
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
            let stripped = remove(target, &body, file.path()).unwrap();
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
            let stripped = remove(target, &legacy, file.path()).unwrap();
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
