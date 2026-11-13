//! Compile an include component once; matching never recurses or allocates.

#[derive(Clone, Copy)]
enum Token {
    Literal(char),
    Any,
    Star,
}

pub(super) struct Pattern(Vec<Token>);

impl Pattern {
    pub fn new(pattern: &str) -> Self {
        let mut tokens = Vec::new();
        let mut chars = pattern.chars();
        while let Some(ch) = chars.next() {
            tokens.push(match ch {
                '\\' => Token::Literal(chars.next().unwrap_or(ch)),
                '?' => Token::Any,
                '*' => {
                    if matches!(tokens.last(), Some(Token::Star)) {
                        continue;
                    }
                    Token::Star
                }
                ch => Token::Literal(ch),
            });
        }
        Self(tokens)
    }

    pub fn matches(&self, name: &str) -> bool {
        if name.starts_with('.') && !matches!(self.0.first(), Some(Token::Literal('.'))) {
            return false;
        }
        let (mut token, mut byte, mut star) = (0, 0, None);
        while byte < name.len() {
            let ch = name[byte..].chars().next().expect("a valid UTF-8 boundary");
            match self.0.get(token) {
                Some(Token::Any) => {}
                Some(Token::Literal(expected)) if *expected == ch => {}
                Some(Token::Star) => {
                    token += 1;
                    star = Some((token, byte));
                    continue;
                }
                _ => {
                    let Some((resume, matched)) = &mut star else {
                        return false;
                    };
                    let Some(next) = name[*matched..].chars().next() else {
                        return false;
                    };
                    *matched += next.len_utf8();
                    token = *resume;
                    byte = *matched;
                    continue;
                }
            }
            token += 1;
            byte += ch.len_utf8();
        }
        self.0[token..]
            .iter()
            .all(|token| matches!(token, Token::Star))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_escapes_unicode_hidden_files_and_backtracking_match_shell_components() {
        for (pattern, name, expected) in [
            ("*.conf", "a.conf", true),
            ("*.conf", "a.kdl", false),
            ("a?c", "a界c", true),
            ("a\\*c", "a*c", true),
            ("*", ".hidden", false),
            ("\\.*", ".hidden", true),
            ("a*bc?d", "a界bcbczd", true),
            ("a*bc?d", "a界bcbcd", false),
            ("*", "", true),
            ("?", "", false),
        ] {
            assert_eq!(
                Pattern::new(pattern).matches(name),
                expected,
                "{pattern}: {name}"
            );
        }
    }

    #[test]
    fn adversarial_stars_do_not_create_exponential_recursion() {
        let pattern = format!("{}b", "*a".repeat(512));
        assert!(!Pattern::new(&pattern).matches(&"a".repeat(1024)));
    }
}
