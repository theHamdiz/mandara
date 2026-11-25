//! Replace declaration values while retaining the stylesheet's source text.

use anyhow::{Result, ensure};
use cssparser::{
    AtRuleParser, BasicParseErrorKind, CowRcStr, DeclarationParser, ParseError, ParseErrorKind,
    Parser, ParserState, QualifiedRuleParser, RuleBodyItemParser, RuleBodyParser, StyleSheetParser,
    Token, parse_important,
};

struct Editor<'a> {
    source: &'a str,
    property: &'a str,
    replacement: &'a str,
    result: String,
    copied: usize,
    changed: bool,
    too_deep: bool,
}

fn drain(input: &mut Parser<'_>) -> Result<(), ParseError<()>> {
    while let Ok(token) = input.next_including_whitespace_and_comments() {
        if token.is_parse_error() {
            return Err(ParseError::unexpected_token());
        }
    }
    Ok(())
}

impl Editor<'_> {
    fn body(&mut self, input: &mut Parser<'_>) -> Result<(), ParseError<()>> {
        // CSS recovery leaves unrelated invalid rules in their original text.
        let mut rules = RuleBodyParser::new(input, self);
        while let Some(result) = rules.next() {
            if matches!(
                result,
                Err((
                    ParseError {
                        kind: ParseErrorKind::Basic(BasicParseErrorKind::TooManyNestedBlocks)
                    },
                    _,
                    _
                ))
            ) {
                rules.parser.too_deep = true;
                return Err(ParseError::custom(()));
            }
        }
        Ok(())
    }
}

impl<'i> DeclarationParser<'i> for Editor<'_> {
    type Declaration = ();
    type Error = ();

    fn parse_value(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i>,
        _: &ParserState,
    ) -> Result<(), ParseError<()>> {
        let start = input.position().byte_index();
        let target = name.as_ref() == self.property;
        let mut priority = None;
        loop {
            let state = input.state();
            let Ok(token) = input.next_including_whitespace_and_comments() else {
                break;
            };
            if token.is_parse_error()
                || (matches!(token, Token::CurlyBracketBlock) && !name.starts_with("--"))
            {
                return Err(ParseError::unexpected_token());
            }
            if target && matches!(token, Token::Delim('!')) {
                input.reset(&state);
                if input
                    .try_parse(|input| {
                        parse_important(input)?;
                        input.expect_exhausted()
                    })
                    .is_ok()
                {
                    priority = Some(state.position().byte_index());
                    drain(input)?;
                    break;
                }
                input.next_including_whitespace_and_comments()?;
            }
        }
        if target {
            let end = input.position().byte_index();
            let value = &self.source[start..end];
            let leading = value.len() - value.trim_start().len();
            let trailing = value.len() - value.trim_end().len();
            self.result
                .push_str(&self.source[self.copied..start + leading]);
            self.result.push_str(self.replacement);
            if let Some(priority) = priority {
                self.result.push(' ');
                self.result.push_str(self.source[priority..end].trim());
            }
            self.result.push_str(&value[value.len() - trailing..]);
            self.copied = end;
            self.changed = true;
        }
        Ok(())
    }
}

impl<'i> QualifiedRuleParser<'i> for Editor<'_> {
    type Prelude = ();
    type QualifiedRule = ();
    type Error = ();
    fn parse_prelude(&mut self, input: &mut Parser<'i>) -> Result<(), ParseError<()>> {
        drain(input)
    }
    fn parse_block(
        &mut self,
        _: (),
        _: &ParserState,
        input: &mut Parser<'i>,
    ) -> Result<(), ParseError<()>> {
        self.body(input)
    }
}

impl<'i> AtRuleParser<'i> for Editor<'_> {
    type Prelude = ();
    type AtRule = ();
    type Error = ();
    fn parse_prelude(
        &mut self,
        _: CowRcStr<'i>,
        input: &mut Parser<'i>,
    ) -> Result<(), ParseError<()>> {
        drain(input)
    }
    fn parse_block(
        &mut self,
        _: (),
        _: &ParserState,
        input: &mut Parser<'i>,
    ) -> Result<(), ParseError<()>> {
        self.body(input)
    }
}

impl<'i> RuleBodyItemParser<'i, (), ()> for Editor<'_> {
    fn parse_declarations(&self) -> bool {
        true
    }
    fn parse_qualified(&self) -> bool {
        true
    }
}

pub(super) fn replace(source: &str, property: &str, replacement: &str) -> Result<String> {
    let mut editor = Editor {
        source,
        property,
        replacement,
        result: String::with_capacity(source.len()),
        copied: 0,
        changed: false,
        too_deep: false,
    };
    // The parser also bounds nested blocks before calling our rule callbacks.
    for _ in StyleSheetParser::new(&mut Parser::new(source), &mut editor) {}
    ensure!(
        !editor.too_deep,
        "{}",
        mandara_i18n::t("Browser CSS exceeds the nesting limit")
    );
    ensure!(editor.changed, "browser CSS has no font size setting");
    editor.result.push_str(&source[editor.copied..]);
    Ok(editor.result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rule_traversal_is_bounded() {
        const EXCESSIVE_NESTING: usize = 1024;
        let source = format!(
            "{}:root{{--size:12px}}{}",
            "@media all{".repeat(EXCESSIVE_NESTING),
            "}".repeat(EXCESSIVE_NESTING)
        );
        assert!(replace(&source, "--size", "18px").is_err());
    }
}
