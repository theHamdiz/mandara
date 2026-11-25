//! Update a CSS custom property without replacing its containing rule.

mod syntax;

/// The font-size custom property exposed by the browser extension.
const PROPERTY: &str = "--pywalfox-font-size";

pub(super) fn resize(source: &str, size: i64) -> anyhow::Result<String> {
    syntax::replace(source, PROPERTY, &format!("{size}px"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn minified_and_multiline_rules_keep_selectors_braces_and_other_properties() {
        for (source, expected) in [
            (
                ":root{--pywalfox-font-size:12px;color:red}",
                ":root{--pywalfox-font-size:18px;color:red}",
            ),
            (
                ":root {\n  --pywalfox-font-size: 12px !important;\n  color: red;\n}\n",
                ":root {\n  --pywalfox-font-size: 18px !important;\n  color: red;\n}\n",
            ),
            (
                "/* --pywalfox-font-size:4px; */ a{content:'--pywalfox-font-size:2px;';--pywalfox-font-size:12px}",
                "/* --pywalfox-font-size:4px; */ a{content:'--pywalfox-font-size:2px;';--pywalfox-font-size:18px}",
            ),
        ] {
            assert_eq!(resize(source, 18).unwrap(), expected);
        }
        assert!(resize("/* --pywalfox-font-size:12px; */", 18).is_err());
    }

    #[test]
    fn value_comments_strings_and_nested_tokens_do_not_end_the_declaration() {
        for (source, expected) in [
            (
                ":root{--pywalfox-font-size:12px /* size; keep } */;color:red}",
                ":root{--pywalfox-font-size:18px;color:red}",
            ),
            (
                ":root{--pywalfox-font-size:'a;}' ;color:red}",
                ":root{--pywalfox-font-size:18px ;color:red}",
            ),
            (
                ":root{--pywalfox-font-size:var(--size, 'a; b}');color:red}",
                ":root{--pywalfox-font-size:18px;color:red}",
            ),
            (
                ":root{--pywalfox-font-size/* hint */:12px ! /* priority */ IMPORTANT; color:red}",
                ":root{--pywalfox-font-size/* hint */:18px ! /* priority */ IMPORTANT; color:red}",
            ),
            (
                ":root{--pywalfox-font-size:12px /* !important */;color:red}",
                ":root{--pywalfox-font-size:18px;color:red}",
            ),
            (
                ":root{--pywalfox-font-size:12px\\;;color:red}",
                ":root{--pywalfox-font-size:18px;color:red}",
            ),
        ] {
            assert_eq!(resize(source, 18).unwrap(), expected);
        }
    }
    #[test]
    fn strings_urls_custom_values_and_nested_rules_are_separate_from_declarations() {
        let source = r#"@media all{:root{--other:{--pywalfox-font-size:42px};background:url(data:text/plain;--pywalfox-font-size:42px);content:"--pywalfox-font-size:42px";--pywalfox-font-size:12px;div:hover{--pywalfox-font-size:14px}}}"#;
        let expected = r#"@media all{:root{--other:{--pywalfox-font-size:42px};background:url(data:text/plain;--pywalfox-font-size:42px);content:"--pywalfox-font-size:42px";--pywalfox-font-size:18px;div:hover{--pywalfox-font-size:18px}}}"#;
        assert_eq!(resize(source, 18).unwrap(), expected);
    }
}
