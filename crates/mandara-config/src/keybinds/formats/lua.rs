//! Parse Lua without executing a user's configuration or imported modules.

use std::ops::Range;

use anyhow::{Result, ensure};
use full_moon::{
    ast::{Call, Expression, FunctionArgs, FunctionCall, Index, LastStmt, Prefix, Suffix},
    visitors::Visitor,
};
use mandara_i18n::tk;

use crate::keybinds::{Binding, DYNAMIC, Encoding, ShortcutFormat, field, line, normalize};

pub(super) struct Lua;

fn parse_ast(source: &str) -> Result<full_moon::ast::Ast> {
    full_moon::parse(source).map_err(|errors| {
        anyhow::anyhow!(
            "{}",
            errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        )
    })
}

pub(crate) fn validate_value(value: &str, string: bool) -> Result<()> {
    let ast = parse_ast(&format!("return {value}"))?;
    let Some(LastStmt::Return(result)) = ast.nodes().last_stmt() else {
        anyhow::bail!("expected a Lua string");
    };
    ensure!(
        ast.nodes().stmts().next().is_none()
            && result.returns().iter().count() == 1
            && result.returns().iter().next().is_some_and(|expression| {
                if string { matches!(expression, Expression::String(_)) }
                else { matches!(expression, Expression::Number(_) | Expression::Symbol(_)) || (value.trim_start().starts_with('-') && matches!(expression, Expression::UnaryOperator { expression, .. } if matches!(expression.as_ref(), Expression::Number(_)))) }
            }),
        "{}",
        mandara_i18n::t("Enter one literal Lua value for this shortcut field.")
    );
    Ok(())
}

impl ShortcutFormat for Lua {
    fn parse(&self, source: &str) -> Result<Vec<Binding>> {
        let ast = parse_ast(source)?;
        let mut visitor = Bindings {
            source,
            rows: Vec::new(),
            loops: 0,
        };
        visitor.visit_ast(&ast);
        Ok(visitor.rows)
    }
}

struct Bindings<'a> {
    source: &'a str,
    rows: Vec<Binding>,
    loops: usize,
}

fn span(node: &impl full_moon::node::Node) -> Option<Range<usize>> {
    Some(node.start_position()?.bytes()..node.end_position()?.bytes())
}

fn call<'a>(
    node: &'a FunctionCall,
    names: &[&str],
) -> Option<&'a full_moon::ast::punctuated::Punctuated<Expression>> {
    let Prefix::Name(prefix) = node.prefix() else {
        return None;
    };
    if prefix.token().to_string() != *names.first()? {
        return None;
    }
    let mut suffixes = node.suffixes();
    for name in &names[1..] {
        let Suffix::Index(Index::Dot { name: actual, .. }) = suffixes.next()? else {
            return None;
        };
        if actual.token().to_string() != *name {
            return None;
        }
    }
    let Suffix::Call(Call::AnonymousCall(args)) = suffixes.next()? else {
        return None;
    };
    if suffixes.next().is_some() {
        return None;
    }
    let FunctionArgs::Parentheses { arguments, .. } = args.as_ref() else {
        return None;
    };
    Some(arguments)
}

impl Visitor for Bindings<'_> {
    fn visit_numeric_for(&mut self, _: &full_moon::ast::NumericFor) {
        self.loops += 1;
    }
    fn visit_numeric_for_end(&mut self, _: &full_moon::ast::NumericFor) {
        self.loops -= 1;
    }
    fn visit_generic_for(&mut self, _: &full_moon::ast::GenericFor) {
        self.loops += 1;
    }
    fn visit_generic_for_end(&mut self, _: &full_moon::ast::GenericFor) {
        self.loops -= 1;
    }
    fn visit_while(&mut self, _: &full_moon::ast::While) {
        self.loops += 1;
    }
    fn visit_while_end(&mut self, _: &full_moon::ast::While) {
        self.loops -= 1;
    }

    fn visit_function_call(&mut self, node: &FunctionCall) {
        let Some(arguments) = call(node, &["hl", "bind"]) else {
            return;
        };
        let Some(first) = arguments.iter().next() else {
            return;
        };
        let Some(range) = span(first) else {
            return;
        };
        let raw = &self.source[range.clone()];
        let literal = matches!(first, Expression::String(_));
        let mut row = Binding {
            line: line(self.source, range.start),
            scope: "Lua".into(),
            chord: literal
                .then(|| raw.trim_matches(['\'', '"']))
                .filter(|v| !v.contains('\\'))
                .and_then(normalize),
            fields: vec![field(tk("Shortcut"), raw, range, Encoding::LuaLiteral)],
            read_only: (!literal || self.loops > 0).then_some(DYNAMIC),
        };
        if let Some(Expression::FunctionCall(action)) = arguments.iter().nth(1) {
            // A dispatcher name and each literal argument are independently editable.
            let suffixes: Vec<_> = action.suffixes().collect();
            if let [
                Suffix::Index(Index::Dot {
                    name: namespace, ..
                }),
                Suffix::Index(Index::Dot { name, .. }),
                Suffix::Call(Call::AnonymousCall(args)),
            ] = suffixes.as_slice()
                && matches!(action.prefix(), Prefix::Name(prefix) if prefix.token().to_string() == "hl")
                && namespace.token().to_string() == "dsp"
                && let FunctionArgs::Parentheses { arguments, .. } = args.as_ref()
                && let Some(range) = span(name)
            {
                row.fields.push(field(
                    tk("Action"),
                    &self.source[range.clone()],
                    range,
                    Encoding::Identifier,
                ));
                for argument in arguments.iter() {
                    if let Some(range) = span(argument) {
                        let literal = matches!(argument, Expression::String(_));
                        row.fields.push(field(
                            tk("Argument"),
                            &self.source[range.clone()],
                            range,
                            if literal {
                                Encoding::LuaLiteral
                            } else {
                                Encoding::LuaScalar
                            },
                        ));
                        if !literal
                            && !matches!(argument, Expression::Number(_) | Expression::Symbol(_))
                        {
                            row.read_only = Some(DYNAMIC);
                        }
                    }
                }
            } else {
                row.read_only = Some(DYNAMIC);
            }
        } else {
            row.read_only = Some(DYNAMIC);
        }
        if let Some(Expression::TableConstructor(options)) = arguments.iter().nth(2) {
            for option in options.fields().iter() {
                if let full_moon::ast::Field::NameKey { key, value, .. } = option
                    && let Some(range) = span(value.as_ref())
                {
                    let string = matches!(value.as_ref(), Expression::String(_));
                    if string
                        || matches!(
                            value.as_ref(),
                            Expression::Number(_) | Expression::Symbol(_)
                        )
                    {
                        row.fields.push(
                            field(
                                tk("Option"),
                                &self.source[range.clone()],
                                range,
                                if string {
                                    Encoding::LuaLiteral
                                } else {
                                    Encoding::LuaScalar
                                },
                            )
                            .named(key.token().to_string()),
                        );
                    } else {
                        row.read_only = Some(DYNAMIC);
                    }
                } else {
                    row.read_only = Some(DYNAMIC);
                }
            }
        } else if arguments.iter().nth(2).is_some() {
            row.read_only = Some(DYNAMIC);
        }
        self.rows.push(row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiline_lua_binds_are_read_without_running_any_lua() {
        let text = "-- keep\nos.execute('never run this')\nhl.bind(\n 'SUPER + T',\n hl.dsp.exec_cmd('foot --title=hello')\n)\nfor _, key in ipairs(keys) do hl.bind(key, hl.dsp.close()) end\n";
        let binds = Lua.parse(text).unwrap();
        assert_eq!(binds.len(), 2);
        assert!(binds[0].read_only.is_none());
        assert_eq!(binds[1].read_only, Some(DYNAMIC));
        let mut values: Vec<_> = binds[0].fields.iter().map(|f| f.value.clone()).collect();
        values[2] = "'kitty'".into();
        let next = crate::keybinds::patch(text, &binds[0], &values).unwrap();
        assert_eq!(next, text.replace("'foot --title=hello'", "'kitty'"));
        assert!(Lua.parse(&next).is_ok());
    }

    #[test]
    fn lua_fields_cannot_turn_into_calls_or_extra_statements() {
        assert!(validate_value("'hello'", true).is_ok());
        assert!(validate_value("true", false).is_ok());
        assert!(validate_value("load('bad')", false).is_err());
        assert!(validate_value("os.execute('bad')", true).is_err());
        assert!(validate_value("'a'; os.execute('bad')", true).is_err());
    }
}
