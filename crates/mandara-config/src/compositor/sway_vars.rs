//! Sway substitutions stay inside the source budget before allocating output.

use anyhow::{Result, ensure};
use std::borrow::Cow;

pub(super) fn expand<'a>(input: &'a str, variables: &[(String, String)]) -> Result<Cow<'a, str>> {
    ensure!(
        input.len() <= crate::keybinds::MAX_FILE_BYTES,
        mandara_i18n::t("The shortcut configuration is too large to edit.")
    );
    let mut bytes = 0_usize;
    let changed = substitute(input, variables, |part| {
        bytes = bytes
            .checked_add(part.len())
            .filter(|size| *size <= crate::keybinds::MAX_FILE_BYTES)
            .ok_or_else(|| {
                anyhow::anyhow!(mandara_i18n::t(
                    "The shortcut configuration is too large to edit."
                ))
            })?;
        Ok(())
    })?;
    if !changed {
        return Ok(Cow::Borrowed(input));
    }
    let mut output = String::with_capacity(bytes);
    substitute(input, variables, |part| {
        output.push_str(part);
        Ok(())
    })?;
    Ok(Cow::Owned(output))
}

fn substitute(
    input: &str,
    variables: &[(String, String)],
    mut append: impl FnMut(&str) -> Result<()>,
) -> Result<bool> {
    let (mut at, mut start, mut changed) = (0, 0, false);
    let bytes = input.as_bytes();
    while let Some(offset) = input[at..].find('$') {
        at += offset;
        // Sway leaves a singly escaped dollar for the next parsing layer.
        if at > 0 && bytes[at - 1] == b'\\' && (at == 1 || bytes[at - 2] != b'\\') {
            at += 1;
            continue;
        }
        let replacement = if bytes.get(at + 1) == Some(&b'$') {
            Some(("$", at + 2))
        } else {
            variables.iter().find_map(|(name, value)| {
                input[at..]
                    .starts_with(name)
                    .then_some((value.as_str(), at + name.len()))
            })
        };
        let Some((value, end)) = replacement else {
            at += 1;
            continue;
        };
        append(&input[start..at])?;
        append(value)?;
        start = end;
        at = end;
        changed = true;
    }
    append(&input[start..])?;
    Ok(changed)
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
    let mut bytes = name.len().checked_add(value.len()).ok_or_else(|| {
        anyhow::anyhow!(mandara_i18n::t(
            "The shortcut configuration is too large to edit."
        ))
    })?;
    for (key, value) in variables.iter().filter(|(key, _)| key != name) {
        bytes = bytes
            .checked_add(key.len())
            .and_then(|bytes| bytes.checked_add(value.len()))
            .filter(|&bytes| bytes <= crate::keybinds::MAX_TOTAL_BYTES)
            .ok_or_else(|| {
                anyhow::anyhow!(mandara_i18n::t(
                    "The shortcut configuration is too large to edit."
                ))
            })?;
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
    fn escaped_dollars_remain_available_to_the_next_parsing_layer() {
        let variables = [
            ("$KEYS".into(), "WRONG".into()),
            ("$OTHER".into(), "value".into()),
        ];
        assert_eq!(expand("$$KEYS", &variables).unwrap(), "$KEYS");
        assert_eq!(expand(r"\$KEYS", &variables).unwrap(), r"\$KEYS");
        assert_eq!(expand(r"\\$KEYS", &variables).unwrap(), r"\\WRONG");
        assert_eq!(
            expand("$KEYS $$OTHER $OTHER", &variables).unwrap(),
            "WRONG $OTHER value"
        );
        assert_eq!(
            expand(
                "$a",
                &[("$a".into(), "$b".into()), ("$b".into(), "value".into())]
            )
            .unwrap(),
            "$b"
        );
    }
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
