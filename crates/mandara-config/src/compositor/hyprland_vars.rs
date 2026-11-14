//! Hyprlang substitutes every matching variable, including after a backslash.

use anyhow::{Result, ensure};

/// Hyprlang rejects a configuration after 100 rounds of variable substitution.
const MAX_ROUNDS: usize = 100;

pub(super) fn expand(input: &str, variables: &[(String, String)]) -> Result<String> {
    let invalid = || mandara_i18n::t("The shortcut configuration is too large to edit.");
    ensure!(input.len() <= crate::keybinds::MAX_FILE_BYTES, invalid());
    let mut output = input.to_owned();
    for _ in 0..MAX_ROUNDS {
        let mut changed = false;
        for (name, value) in variables {
            let count = output.matches(name).count();
            if count == 0 {
                continue;
            }
            let bytes = output
                .len()
                .checked_sub(
                    count
                        .checked_mul(name.len())
                        .ok_or_else(|| anyhow::anyhow!(invalid()))?,
                )
                .and_then(|kept| {
                    count
                        .checked_mul(value.len())
                        .and_then(|added| kept.checked_add(added))
                })
                .filter(|bytes| *bytes <= crate::keybinds::MAX_FILE_BYTES)
                .ok_or_else(|| anyhow::anyhow!(invalid()))?;
            let mut next = String::with_capacity(bytes);
            let mut start = 0;
            for (at, _) in output.match_indices(name) {
                next.push_str(&output[start..at]);
                next.push_str(value);
                start = at + name.len();
            }
            next.push_str(&output[start..]);
            output = next;
            changed = true;
        }
        if !changed {
            return Ok(output);
        }
    }
    anyhow::bail!(invalid())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backslashes_do_not_escape_variables_and_expansion_remains_bounded() {
        let variables = [("$arg".into(), "hello".into())];
        assert_eq!(
            expand(r"notify-send \$arg $$arg", &variables).unwrap(),
            r"notify-send \hello $hello"
        );
        assert!(expand("$arg", &[("$arg".into(), "$arg".into())]).is_err());
        assert!(
            expand(
                "$arg$arg",
                &[("$arg".into(), "x".repeat(crate::keybinds::MAX_FILE_BYTES))]
            )
            .is_err()
        );
    }
}
