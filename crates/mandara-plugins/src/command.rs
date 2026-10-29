//! Structured, self-documenting commands.
//!
//! Every action the shell can perform is a [`CommandSpec`]: a namespaced
//! path (`media play-pause`, `audio volume set`), typed arguments, a
//! description, examples, and a handler returning JSON. The catalog is
//! exported by `mandara commands --json`, so agents can discover and call
//! everything without scraping help text.

use std::rc::Rc;

use gpui::App;
use mandara_i18n::{t, tf};
use serde::Serialize;
use serde_json::Value;

/// Argument kinds, used for validation and documentation.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArgKind {
    String,
    Integer,
    Number,
    Bool,
    /// One of a fixed set of values (listed in `choices`).
    Enum,
}

#[derive(Clone, Debug, Serialize)]
pub struct ArgSpec {
    pub name: &'static str,
    pub kind: ArgKind,
    pub required: bool,
    pub description: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<&'static str>,
}

impl ArgSpec {
    pub fn required(name: &'static str, kind: ArgKind, description: &'static str) -> Self {
        Self { name, kind, required: true, description, choices: Vec::new() }
    }

    pub fn optional(name: &'static str, kind: ArgKind, description: &'static str) -> Self {
        Self { name, kind, required: false, description, choices: Vec::new() }
    }

    pub fn choices(mut self, choices: &[&'static str]) -> Self {
        self.kind = ArgKind::Enum;
        self.choices = choices.to_vec();
        self
    }

    /// Validates a raw string argument against this spec.
    fn validate(&self, raw: &str) -> Result<(), String> {
        let ok = match self.kind {
            ArgKind::String => true,
            ArgKind::Integer => raw.parse::<i64>().is_ok(),
            ArgKind::Number => raw.parse::<f64>().is_ok(),
            ArgKind::Bool => matches!(raw, "true" | "false" | "on" | "off" | "1" | "0"),
            ArgKind::Enum => self.choices.contains(&raw),
        };
        if ok {
            Ok(())
        } else if self.kind == ArgKind::Enum {
            Err(tf("Argument <{name}> must be one of: {choices}", &[("name", &self.name), ("choices", &self.choices.join(", "))]))
        } else {
            Err(tf(
                "Argument <{name}> must be {kind}, got {value}",
                &[("name", &self.name), ("kind", &format!("{:?}", self.kind)), ("value", &format!("{raw:?}"))],
            ))
        }
    }
}

/// Parsed, validated arguments handed to a handler.
#[derive(Clone, Debug, Default)]
pub struct Args(pub Vec<String>);

impl Args {
    pub fn get(&self, i: usize) -> Option<&str> {
        self.0.get(i).map(String::as_str)
    }

    pub fn str(&self, i: usize) -> Result<&str, String> {
        self.get(i).ok_or_else(|| tf("Missing argument #{position}", &[("position", &(i + 1))]))
    }

    pub fn int(&self, i: usize) -> Result<i64, String> {
        self.str(i)?.parse().map_err(|_| tf("Argument #{position} is not an integer", &[("position", &(i + 1))]))
    }

    pub fn number(&self, i: usize) -> Result<f64, String> {
        self.str(i)?.parse().map_err(|_| tf("Argument #{position} is not a number", &[("position", &(i + 1))]))
    }

    pub fn bool(&self, i: usize) -> Result<bool, String> {
        match self.str(i)? {
            "true" | "on" | "1" => Ok(true),
            "false" | "off" | "0" => Ok(false),
            other => Err(tf("Argument #{position} is not a boolean: {value}", &[("position", &(i + 1)), ("value", &format!("{other:?}"))])),
        }
    }

    /// Joins arguments from `i` onward (for free-text tails like SSIDs).
    pub fn rest(&self, i: usize) -> String {
        self.0.get(i..).map(|s| s.join(" ")).unwrap_or_default()
    }
}

/// What a handler produces: an immediate JSON value, or a pending result a
/// service actor will deliver (awaited asynchronously by the IPC layer so
/// the UI thread never blocks).
pub enum Output {
    Value(Value),
    Pending(async_channel::Receiver<Result<String, String>>),
}

impl From<Value> for Output {
    fn from(v: Value) -> Self {
        Output::Value(v)
    }
}

impl From<async_channel::Receiver<Result<String, String>>> for Output {
    fn from(rx: async_channel::Receiver<Result<String, String>>) -> Self {
        Output::Pending(rx)
    }
}

impl Output {
    /// Resolves a pending output (await on any executor).
    pub async fn resolve(self) -> Result<Value, String> {
        match self {
            Output::Value(v) => Ok(v),
            Output::Pending(rx) => match rx.recv().await {
                // A worker may answer with a JSON document (an object or a
                // list); anything else is a plain message.
                Ok(result) => result.map(|text| match text.trim_start().starts_with(['{', '[']) {
                    true => serde_json::from_str(&text).unwrap_or(Value::from(text)),
                    false => Value::from(text),
                }),
                Err(_) => Err(t("The service did not reply").into()),
            },
        }
    }
}

pub type Handler = Rc<dyn Fn(&Args, &mut App) -> Result<Output, String>>;

/// One documented command.
#[derive(Clone, Serialize)]
pub struct CommandSpec {
    /// Space-separated path, e.g. `media play-pause`.
    pub path: &'static str,
    pub summary: &'static str,
    pub args: Vec<ArgSpec>,
    /// Example invocations (without the `mandara msg` prefix).
    pub examples: Vec<&'static str>,
    /// Plugin that contributed the command (`core` for built-in shell verbs).
    pub owner: String,
    /// Runs even while the owning plugin is off (e.g. `dock`, which turns
    /// the dock plugin itself on). Other commands of an off plugin answer
    /// with the [`CommandGate`]'s explanation instead of running.
    #[serde(skip)]
    pub works_when_off: bool,
    #[serde(skip)]
    pub handler: Handler,
}

impl CommandSpec {
    /// A command with an immediate JSON result.
    pub fn new(
        path: &'static str,
        summary: &'static str,
        handler: impl Fn(&Args, &mut App) -> Result<Value, String> + 'static,
    ) -> Self {
        Self::new_output(path, summary, move |args, cx| handler(args, cx).map(Output::Value))
    }

    /// A command that may hand back a pending service result.
    pub fn new_output(
        path: &'static str,
        summary: &'static str,
        handler: impl Fn(&Args, &mut App) -> Result<Output, String> + 'static,
    ) -> Self {
        Self { path, summary, args: Vec::new(), examples: Vec::new(), owner: "core".into(), works_when_off: false, handler: Rc::new(handler) }
    }

    /// Keeps the command available while its plugin is off.
    pub fn works_when_off(mut self) -> Self {
        self.works_when_off = true;
        self
    }

    pub fn arg(mut self, arg: ArgSpec) -> Self {
        self.args.push(arg);
        self
    }

    pub fn example(mut self, example: &'static str) -> Self {
        self.examples.push(example);
        self
    }

    fn words(&self) -> Vec<&'static str> {
        self.path.split(' ').collect()
    }

    /// One-line usage, e.g. `audio volume set <percent>`.
    pub fn usage(&self) -> String {
        let mut s = self.path.to_string();
        for a in &self.args {
            if a.required {
                s.push_str(&format!(" <{}>", a.name));
            } else {
                s.push_str(&format!(" [{}]", a.name));
            }
        }
        s
    }
}

/// Says why a command's owning plugin can't run it right now (it is off or
/// removed), or `None` to let it run. Installed by the shell as a global;
/// without one every command runs.
#[derive(Clone)]
pub struct CommandGate(pub Rc<dyn Fn(&str, &App) -> Option<String>>);

impl gpui::Global for CommandGate {}

/// All commands known to the shell.
#[derive(Default, Clone)]
pub struct CommandRegistry {
    commands: Vec<CommandSpec>,
}

/// Result of resolving a command line.
pub struct Resolved<'a> {
    pub spec: &'a CommandSpec,
    pub args: Args,
}

impl CommandRegistry {
    pub fn register(&mut self, owner: &str, mut spec: CommandSpec) {
        spec.owner = owner.to_string();
        self.commands.retain(|c| c.path != spec.path);
        self.commands.push(spec);
        self.commands.sort_by(|a, b| a.path.cmp(b.path));
    }

    pub fn all(&self) -> &[CommandSpec] {
        &self.commands
    }

    /// Finds the longest command path that prefixes `words`, then validates
    /// the remaining words as arguments.
    pub fn resolve(&self, words: &[String]) -> Result<Resolved<'_>, String> {
        let spec = self
            .commands
            .iter()
            .filter(|c| {
                let path = c.words();
                path.len() <= words.len() && path.iter().zip(words).all(|(p, w)| p == w)
            })
            .max_by_key(|c| c.words().len())
            .ok_or_else(|| {
                let prefix = words.first().map(String::as_str).unwrap_or("");
                let near: Vec<String> =
                    self.commands.iter().filter(|c| c.path.starts_with(prefix)).map(|c| c.usage()).collect();
                let command = format!("{:?}", words.join(" "));
                if near.is_empty() || prefix.is_empty() {
                    tf("Unknown command {command}; run `mandara commands` for the catalog", &[("command", &command)])
                } else {
                    tf("Unknown command {command}; did you mean:\n  {suggestions}", &[("command", &command), ("suggestions", &near.join("\n  "))])
                }
            })?;
        let rest: Vec<String> = words[spec.words().len()..].to_vec();
        let required = spec.args.iter().filter(|a| a.required).count();
        if rest.len() < required {
            return Err(tf("Not enough arguments; usage: {usage}", &[("usage", &spec.usage())]));
        }
        // The last argument may swallow extra words only if it is a string.
        if rest.len() > spec.args.len() && spec.args.last().is_none_or(|a| a.kind != ArgKind::String) {
            return Err(tf("Too many arguments; usage: {usage}", &[("usage", &spec.usage())]));
        }
        for (arg, raw) in spec.args.iter().zip(&rest) {
            arg.validate(raw)?;
        }
        Ok(Resolved { spec, args: Args(rest) })
    }

    /// Resolves and runs a command line.
    pub fn execute(&self, words: &[String], cx: &mut App) -> Result<Output, String> {
        let Resolved { spec, args } = self.resolve(words)?;
        if !spec.works_when_off
            && let Some(gate) = cx.try_global::<CommandGate>().cloned()
            && let Some(why) = (gate.0)(&spec.owner, cx)
        {
            return Err(why);
        }
        let handler = spec.handler.clone();
        let name = spec.path;
        crate::guard(&format!("command `{name}`"), || handler(&args, cx))
            .unwrap_or_else(|| Err(tf("`{command}` hit an internal error (logged); the shell kept running", &[("command", &name)])))
    }

    /// The machine-readable catalog.
    pub fn catalog_json(&self) -> Value {
        serde_json::to_value(&self.commands).unwrap_or(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> CommandRegistry {
        let mut r = CommandRegistry::default();
        r.register("core", CommandSpec::new("ping", "Liveness check", |_, _| Ok(Value::from("pong"))));
        r.register(
            "audio",
            CommandSpec::new("audio volume set", "Set volume", |a, _| Ok(Value::from(a.int(0)?)))
                .arg(ArgSpec::required("percent", ArgKind::Integer, "0-150")),
        );
        r.register("audio", CommandSpec::new("audio volume", "Get volume", |_, _| Ok(Value::from(50))));
        r.register(
            "network",
            CommandSpec::new("network wifi connect", "Connect", |a, _| Ok(Value::from(a.rest(0))))
                .arg(ArgSpec::required("ssid", ArgKind::String, "network name")),
        );
        r.register(
            "core",
            CommandSpec::new("bar layout", "Set layout", |a, _| Ok(Value::from(a.str(0)?.to_string())))
                .arg(ArgSpec::required("layout", ArgKind::String, "").choices(&["edge", "pills"])),
        );
        r
    }

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn resolves_longest_path_and_validates() {
        let r = registry();
        assert_eq!(r.resolve(&words("audio volume set 40")).unwrap().spec.path, "audio volume set");
        assert_eq!(r.resolve(&words("audio volume")).unwrap().spec.path, "audio volume");
        let err = |line: &str| r.resolve(&words(line)).err().unwrap();
        assert!(r.resolve(&words("audio volume set loud")).is_err());
        assert!(err("audio volume set").contains("usage"));
        assert!(err("bar layout island").contains("one of"));
        assert!(err("audio volume set 1 2").contains("Too many"));
    }

    #[test]
    fn string_tail_swallows_spaces() {
        let r = registry();
        let res = r.resolve(&words("network wifi connect Home Net 5G")).unwrap();
        assert_eq!(res.args.rest(0), "Home Net 5G");
    }

    #[test]
    fn unknown_commands_suggest_neighbours() {
        let r = registry();
        let err = r.resolve(&words("audio mute")).err().unwrap();
        assert!(err.contains("audio volume set <percent>"), "{err}");
    }

    #[test]
    fn catalog_is_json_documented() {
        let catalog = registry().catalog_json();
        let arr = catalog.as_array().unwrap();
        let set = arr.iter().find(|c| c["path"] == "audio volume set").unwrap();
        assert_eq!(set["owner"], "audio");
        assert_eq!(set["args"][0]["name"], "percent");
        assert_eq!(set["args"][0]["kind"], "integer");
    }
}
