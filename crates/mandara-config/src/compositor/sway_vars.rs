//! Sway substitutions stay inside the source budget before allocating output.

use anyhow::{Result, ensure};
use std::borrow::Cow;

pub(super) fn expand<'a>(input: &'a str, variables: &[(String, String)]) -> Result<Cow<'a, str>> {
    let mut output = Cow::Borrowed(input);
    ensure!(
        output.len() <= crate::keybinds::MAX_FILE_BYTES,
        mandara_i18n::t("The shortcut configuration is too large to edit.")
    );
    for (name, value) in variables {
        let count = output.matches(name).count();
        if count == 0 {
            continue;
        }
        let size = count
            .checked_mul(name.len())
            .and_then(|removed| output.len().checked_sub(removed))
            .and_then(|kept| {
                count
                    .checked_mul(value.len())
                    .and_then(|added| kept.checked_add(added))
            })
            .filter(|&size| size <= crate::keybinds::MAX_FILE_BYTES)
            .ok_or_else(|| {
                anyhow::anyhow!(mandara_i18n::t("The shortcut configuration is too large to edit."))
            })?;
        let mut next = String::with_capacity(size);
        let mut start = 0;
        for (at, _) in output.match_indices(name) {
            next.push_str(&output[start..at]);
            next.push_str(value);
            start = at + name.len();
        }
        next.push_str(&output[start..]);
        output = Cow::Owned(next);
    }
    Ok(output)
}

pub(super) fn check_assignment(
    variables: &[(String, String)],
    name: &str,
    value: &str,
) -> Result<()> {
    let replacing = variables.iter().any(|(key, _)| key == name);
    ensure!(
        replacing || variables.len() < crate::keybinds::MAX_BINDINGS,
        mandara_i18n::t("The shortcut configuration is too large to edit.")
    );
    let mut bytes = name
        .len()
        .checked_add(value.len())
        .ok_or_else(|| anyhow::anyhow!(mandara_i18n::t("The shortcut configuration is too large to edit.")))?;
    for (key, value) in variables.iter().filter(|(key, _)| key != name) {
        bytes = bytes
            .checked_add(key.len())
            .and_then(|bytes| bytes.checked_add(value.len()))
            .filter(|&bytes| bytes <= crate::keybinds::MAX_TOTAL_BYTES)
            .ok_or_else(|| anyhow::anyhow!(mandara_i18n::t("The shortcut configuration is too large to edit.")))?;
    }
    ensure!(
        bytes <= crate::keybinds::MAX_TOTAL_BYTES,
        mandara_i18n::t("The shortcut configuration is too large to edit.")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oversized_substitution_is_rejected_before_constructing_output() {
        let variables = [("$a".into(), "x".repeat(crate::keybinds::MAX_FILE_BYTES))];
        assert!(expand("$a$a", &variables).is_err());
        assert_eq!(
            expand("$a", &variables).unwrap().len(),
            crate::keybinds::MAX_FILE_BYTES
        );
        assert!(matches!(
            expand("untouched", &variables).unwrap(),
            Cow::Borrowed(_)
        ));
    }
    #[test]
    fn stored_variables_are_bounded_and_replacements_reuse_their_budget() {
        let variables: Vec<_> = (0..crate::keybinds::MAX_TOTAL_BYTES
            / crate::keybinds::MAX_FILE_BYTES)
            .map(|i| {
                (
                    format!("${i}"),
                    "x".repeat(crate::keybinds::MAX_FILE_BYTES - 16),
                )
            })
            .collect();
        assert!(
            check_assignment(
                &variables,
                "$new",
                &"y".repeat(crate::keybinds::MAX_FILE_BYTES)
            )
            .is_err()
        );
        assert!(check_assignment(&variables, "$0", "replacement").is_ok());
    }
}
