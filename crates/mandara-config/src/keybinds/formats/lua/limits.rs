//! Bound recursive parser work before building or visiting a Lua AST.

use anyhow::{Result, bail, ensure};
use full_moon::{
    LuaVersion,
    tokenizer::{Lexer, LexerResult, Symbol, TokenType},
};
use mandara_i18n::t;

/// Four thousand literal binds fit without accepting unbounded token storage.
const MAX_TOKENS: usize = 131_072;
/// Recursive expressions and blocks must fit a short-lived worker's stack.
const MAX_DEPTH: usize = 64;
/// Operators can form recursive ASTs without any brackets. Complex programs
/// stay with the user's text editor; literal binding catalogs need none.
const MAX_OPERATORS: usize = 128;

pub(super) fn check(source: &str) -> Result<()> {
    let mut lexer = Lexer::new(source, LuaVersion::lua54());
    let mut tokens = 0;
    let mut depth = 0usize;
    let mut operators = 0;
    while let Some(next) = lexer.consume() {
        let token = match next {
            LexerResult::Ok(token) => token,
            LexerResult::Fatal(errors) | LexerResult::Recovered(_, errors) => bail!(
                "{}: {}",
                t("The Lua configuration has a tokenization error."),
                errors.first().map(ToString::to_string).unwrap_or_default()
            ),
        };
        tokens += 1;
        if let TokenType::Symbol { symbol } = token.token_type() {
            match symbol {
                Symbol::Function
                | Symbol::If
                | Symbol::Do
                | Symbol::Repeat
                | Symbol::LeftParen
                | Symbol::LeftBrace
                | Symbol::LeftBracket => depth += 1,
                Symbol::End
                | Symbol::Until
                | Symbol::RightParen
                | Symbol::RightBrace
                | Symbol::RightBracket => depth = depth.saturating_sub(1),
                Symbol::And
                | Symbol::Or
                | Symbol::Not
                | Symbol::Caret
                | Symbol::TwoDots
                | Symbol::TwoEqual
                | Symbol::GreaterThan
                | Symbol::GreaterThanEqual
                | Symbol::DoubleGreaterThan
                | Symbol::Hash
                | Symbol::LessThan
                | Symbol::LessThanEqual
                | Symbol::DoubleLessThan
                | Symbol::Minus
                | Symbol::Percent
                | Symbol::Plus
                | Symbol::Slash
                | Symbol::DoubleSlash
                | Symbol::Star
                | Symbol::Tilde
                | Symbol::TildeEqual
                | Symbol::Ampersand
                | Symbol::Pipe => operators += 1,
                _ => {}
            }
        }
        ensure!(
            tokens <= MAX_TOKENS && depth <= MAX_DEPTH && operators <= MAX_OPERATORS,
            "{}",
            t(
                "This Lua configuration is too complex to inspect safely. Edit its shortcuts in your text editor."
            )
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brackets_in_strings_and_comments_do_not_count_but_recursive_programs_do() {
        assert!(
            check(&format!(
                "-- {}\nhl.bind('Super+Q', hl.dsp.exec_cmd({:?}))",
                "(".repeat(1000),
                "(".repeat(1000)
            ))
            .is_ok()
        );
        assert!(
            check(&format!(
                "return {}1{}",
                "(".repeat(MAX_DEPTH + 1),
                ")".repeat(MAX_DEPTH + 1)
            ))
            .is_err()
        );
        assert!(check(&format!("return {}true", "not ".repeat(MAX_OPERATORS + 1))).is_err());
        assert!(check(&format!("return 1{}", "+1".repeat(MAX_OPERATORS + 1))).is_err());
        assert!(check(&"hl.bind('SUPER + Q', hl.dsp.close())\n".repeat(4000)).is_ok());
    }
}
