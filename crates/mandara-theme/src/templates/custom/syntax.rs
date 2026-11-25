//! Normalize the two app-template block forms and filter argument spellings.

use anyhow::{Result, bail};

fn parts(text: &str, delimiter: char) -> Vec<&str> {
    let mut start = 0;
    let mut quote = None;
    let mut escape = false;
    let mut depth = 0u32;
    let mut parts = Vec::new();
    for (at, ch) in text.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if let Some(q) = quote {
            if ch == '\\' {
                escape = true;
            } else if ch == q {
                quote = None;
            }
        } else {
            match ch {
                '\'' | '"' => quote = Some(ch),
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth = depth.saturating_sub(1),
                c if c == delimiter && depth == 0 => {
                    parts.push(&text[start..at]);
                    start = at + ch.len_utf8();
                }
                _ => {}
            }
        }
    }
    parts.push(&text[start..]);
    parts
}

fn expression(text: &str) -> String {
    let mut pieces = parts(text, '|').into_iter();
    let mut out = pieces.next().unwrap_or_default().trim().to_string();
    for filter in pieces {
        out.push_str(" | ");
        let filter = filter.trim();
        let end = filter
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(filter.len());
        let (name, rest) = filter.split_at(end);
        out.push_str(name);
        let rest = rest.trim().trim_start_matches(':').trim();
        if rest.is_empty() || rest.starts_with('(') {
            out.push_str(rest);
        } else {
            out.push('(');
            out.push_str(rest);
            out.push(')');
        }
    }
    zero_based_indexes(out)
}

fn zero_based_indexes(source: String) -> String {
    const INDEX: &str = "loop.index";
    if !source.contains(INDEX) {
        return source;
    }
    let mut out = String::with_capacity(source.len());
    let mut quote = None;
    let mut escaped = false;
    let mut at = 0;
    while at < source.len() {
        let ch = source[at..]
            .chars()
            .next()
            .expect("at a character boundary");
        if escaped {
            escaped = false;
        } else if let Some(current) = quote {
            if ch == '\\' {
                escaped = true;
            } else if ch == current {
                quote = None;
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
        } else if source[at..].starts_with(INDEX)
            && source[..at]
                .chars()
                .next_back()
                .is_none_or(|ch| !ch.is_alphanumeric() && ch != '_' && ch != '.')
            && source[at + INDEX.len()..]
                .chars()
                .next()
                .is_none_or(|ch| !ch.is_alphanumeric() && ch != '_')
        {
            out.push_str("loop.index0");
            at += INDEX.len();
            continue;
        }
        out.push(ch);
        at += ch.len_utf8();
    }
    out
}

fn block(text: &str) -> String {
    let text = text.trim().replace("{{", "").replace("}}", "");
    if let Some(condition) = text.strip_prefix("if ") {
        return match condition.trim().strip_prefix("not ") {
            Some(condition) => format!("if not truthy({})", expression(condition)),
            None => format!("if truthy({})", expression(condition)),
        };
    }
    if let Some((binding, iterable)) = text.strip_prefix("for ").and_then(|s| s.split_once(" in "))
    {
        let iterable = iterable.trim();
        let iterable = match iterable.split_once("..") {
            Some((a, b)) if a.trim().parse::<i64>().is_ok() && b.trim().parse::<i64>().is_ok() => {
                format!("range({}, {})", a.trim(), b.trim())
            }
            _ => expression(iterable),
        };
        return format!(
            "for {binding} in {iterable}{}",
            if binding.contains(',') {
                " | items"
            } else {
                ""
            }
        );
    }
    text
}

fn end(text: &str, delimiter: &str) -> Option<usize> {
    let mut quote = None;
    let mut escaped = false;
    for (at, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if let Some(q) = quote {
            if ch == '\\' {
                escaped = true;
            } else if ch == q {
                quote = None;
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
        } else if text[at..].starts_with(delimiter) {
            return Some(at);
        }
    }
    None
}

pub fn normalize(source: &str) -> Result<String> {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some((at, open, close)) = [("{{", "}}"), ("<*", "*>"), ("{%", "%}")]
        .into_iter()
        .filter_map(|(open, close)| rest.find(open).map(|at| (at, open, close)))
        .min_by_key(|(at, _, _)| *at)
    {
        out.push_str(&rest[..at]);
        // Triple braces are folding markers in editor configuration files.
        if rest[at..].starts_with("{{{") {
            out.push_str("{{ '{{{' }}");
            rest = &rest[at + 3..];
            continue;
        }
        let body = &rest[at + open.len()..];
        let Some(stop) = end(body, close) else {
            bail!("unterminated app-template expression");
        };
        if open == "{{" {
            out.push_str("{{ ");
            out.push_str(&expression(&body[..stop]));
            out.push_str(" }}");
        } else {
            out.push_str("<* ");
            out.push_str(&block(&body[..stop]));
            out.push_str(" *>");
        }
        rest = &body[stop + close.len()..];
    }
    out.push_str(rest);
    let mut trimmed = String::with_capacity(out.len());
    for line in out.split_inclusive('\n') {
        let bare = line.trim();
        if bare.starts_with("<*") && bare.ends_with("*>") && bare.matches("<*").count() == 1 {
            trimmed.push_str(bare);
        } else {
            trimmed.push_str(line);
        }
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_delimiters_and_pipes_stay_inside_filter_arguments() {
        assert_eq!(
            normalize("{{ mode | replace: '}}|', 'night' | lower_case }}").unwrap(),
            "{{ mode | replace('}}|', 'night') | lower_case }}"
        );
        assert_eq!(
            normalize("x<* if {{ mode }} *>\ny<* endif *>\n").unwrap(),
            "x<* if truthy(mode) *>\ny<* endif *>\n"
        );
        assert!(normalize("{{ missing").is_err());
    }
}
