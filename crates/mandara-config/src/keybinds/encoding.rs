//! Field encodings keep edited values inside their original syntax node.

use anyhow::{Result, ensure};

#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Encoding {
    Plain,
    Word,
    Identifier,
    Csv,
    CsvTail,
    ShellWord,
    TomlString,
    KdlName,
    KdlString,
    KdlValue,
    XmlAttribute,
    XmlText,
    LuaLiteral,
    LuaScalar,
    LuaPath,
    RonValue,
    KConfig,
}

impl Encoding {
    pub fn encode(self, value: &str) -> Result<String> {
        ensure!(
            !value.contains('\0'),
            "{}",
            mandara_i18n::t("A shortcut value cannot contain a null character.")
        );
        Ok(match self {
            Self::Plain | Self::Word | Self::Csv | Self::CsvTail => {
                ensure!(
                    !value.contains(['\n', '\r', '#']),
                    "{}",
                    mandara_i18n::t("Keep this shortcut value on one line without a comment.")
                );
                if self == Self::Word {
                    ensure!(
                        !value.is_empty() && !value.chars().any(char::is_whitespace),
                        "{}",
                        mandara_i18n::t("This shortcut value must be one word.")
                    );
                }
                if self == Self::Csv {
                    ensure!(
                        !value.contains(','),
                        "{}",
                        mandara_i18n::t("This shortcut field cannot contain a comma.")
                    );
                }
                if self == Self::CsvTail && !value.trim().is_empty() {
                    format!(",{value}")
                } else {
                    value.to_owned()
                }
            }
            Self::ShellWord => crate::compositor::shell_quote(value),
            Self::Identifier => {
                ensure!(
                    !value.is_empty()
                        && value.chars().enumerate().all(|(at, ch)| ch == '_'
                            || ch.is_ascii_alphabetic()
                            || (at > 0 && ch.is_ascii_digit())),
                    "{}",
                    mandara_i18n::t(
                        "Enter a native action name using letters, digits or underscores."
                    )
                );
                value.to_owned()
            }
            Self::TomlString => toml_edit::Value::from(value).to_string(),
            Self::KdlName => kdl::KdlIdentifier::from(value).to_string(),
            Self::KdlString => {
                let mut entry = kdl::KdlEntry::new(kdl::KdlValue::String(value.to_owned()));
                entry.ensure_v1();
                entry.to_string().trim().to_owned()
            }
            Self::KdlValue => {
                let document = kdl::KdlDocument::parse_v1(&format!("value {value}\n"))?;
                ensure!(
                    document.nodes().len() == 1
                        && document.nodes()[0].entries().len() == 1
                        && document.nodes()[0].entries()[0].name().is_none()
                        && document.nodes()[0].children().is_none(),
                    "{}",
                    mandara_i18n::t("Enter one native value for this shortcut field.")
                );
                value.to_owned()
            }
            Self::XmlAttribute | Self::XmlText => value
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
                .replace('\'', "&apos;"),
            Self::LuaLiteral => {
                super::formats::lua::validate_value(value, true)?;
                value.to_owned()
            }
            Self::LuaScalar => {
                super::formats::lua::validate_value(value, false)?;
                value.to_owned()
            }
            Self::LuaPath => {
                for part in value.split('.') {
                    Self::Identifier.encode(part.trim())?;
                }
                value.to_owned()
            }
            Self::RonValue => {
                ron::value::RawValue::from_ron(value)?;
                value.to_owned()
            }
            Self::KConfig => {
                ensure!(
                    !value.contains(['\n', '\r', '\t']),
                    "{}",
                    mandara_i18n::t("Enter one shortcut alternative without a line break or tab.")
                );
                // KConfig's QStringList escaping is inside the INI escaping.
                value
                    .replace('\\', "\\\\")
                    .replace(',', "\\,")
                    .replace('\\', "\\\\")
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_cannot_inject_an_extra_binding_or_xml_element() {
        assert!(
            Encoding::Plain
                .encode("close\nbind=SUPER,Q,spawn,evil")
                .is_err()
        );
        assert!(Encoding::Word.encode("Super Q").is_err());
        assert!(Encoding::KdlValue.encode("1; evil").is_err());
        assert_eq!(
            Encoding::XmlAttribute.encode("\"/><evil>&").unwrap(),
            "&quot;/&gt;&lt;evil&gt;&amp;"
        );
        let quoted = Encoding::TomlString.encode("a\"\nb").unwrap();
        let parsed: toml::Value = toml::from_str(&format!("field={quoted}")).unwrap();
        assert_eq!(parsed["field"].as_str(), Some("a\"\nb"));
    }
}
