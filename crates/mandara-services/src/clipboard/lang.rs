//! Is this text a code snippet, and in which language? A cheap, pure
//! scoring pass: each language has a table of telltale signals (a line
//! starting with `fn `, `console.log(`, `:=` …) with weights, a few formats
//! are checked structurally (JSON parses, SQL statements, markup tags,
//! YAML / TOML shapes), and generic structure (semicolons, braces,
//! operators, indentation) decides whether a weak match is code at all.
//! Prose (sentences, few symbols) needs a decisive signal to count as code.

/// A telltale signal, matched against each line (case-sensitive).
#[derive(Copy, Clone)]
enum P {
    /// The trimmed line starts with it.
    Start(&'static str),
    /// Anywhere in a line.
    Has(&'static str),
    /// The trimmed line ends with it.
    End(&'static str),
    /// The trimmed line is exactly it.
    Line(&'static str),
    /// The trimmed line starts with the first and also contains the second.
    StartHas(&'static str, &'static str),
}

/// Signal weights: a hint, a likely marker, a strong marker, and one that
/// settles the language on its own.
const HINT: u8 = 1;
const LIKELY: u8 = 2;
const STRONG: u8 = 3;
const DECISIVE: u8 = 5;

/// A language score at or above this is code, prose or not.
const SURE: u32 = DECISIVE as u32;
/// A score at or above this is code when the text isn't prose (or has
/// code structure).
const PROBABLE: u32 = STRONG as u32;
/// Generic structure alone (no language) at or above this is code.
const STRUCTURE_ONLY: u32 = 4;
/// Prose: at least this many words and fewer symbols than this per 100
/// characters.
const PROSE_MIN_WORDS: usize = 6;
const PROSE_SYMBOLS_PER_100: f32 = 1.5;
/// Lines that must look like YAML / TOML for the whole to count.
const YAML_MIN_LINES: usize = 3;
/// Share of lines that must fit a structural format.
const SHAPE_SHARE: f32 = 0.8;
/// Share of lines ending in `;`, `{`, `}` or `,` that reads as code.
const TERMINATOR_SHARE: f32 = 0.3;
/// Share of indented lines that reads as code (with at least 3 lines).
const INDENT_SHARE: f32 = 0.3;

struct Lang {
    id: &'static str,
    signals: &'static [(P, u8)],
}

use P::{End, Has, Line, Start, StartHas};

const LANGS: &[Lang] = &[
    Lang {
        id: "rust",
        signals: &[
            (Start("fn "), STRONG),
            (Has("pub fn "), STRONG),
            (Has("let mut "), STRONG),
            (Start("impl"), LIKELY),
            (Start("#["), STRONG),
            (Has("println!("), DECISIVE),
            (Has("format!("), DECISIVE),
            (Has("vec!["), DECISIVE),
            (Has(".unwrap()"), STRONG),
            (Has("&mut "), LIKELY),
            (Has("&self"), STRONG),
            (Start("pub struct "), STRONG),
            (Start("pub enum "), STRONG),
            (Start("use std::"), DECISIVE),
            (StartHas("use ", "::"), LIKELY),
            (Has("Option<"), LIKELY),
            (Has("Result<"), LIKELY),
            (Has("&str"), LIKELY),
            (Has("-> "), HINT),
            (Start("let "), HINT),
            (Start("mod "), HINT),
        ],
    },
    Lang {
        id: "python",
        signals: &[
            (StartHas("def ", "):"), DECISIVE),
            (StartHas("def ", ") ->"), DECISIVE),
            (StartHas("class ", ":"), LIKELY),
            (StartHas("from ", " import "), STRONG),
            (Start("import "), HINT),
            (Has("elif "), STRONG),
            (Has("self."), LIKELY),
            (Has("__init__"), STRONG),
            (Has("__name__"), STRONG),
            (Has("print("), HINT),
            (Has("lambda "), LIKELY),
            (Start("@"), HINT),
            (End(":"), HINT),
            (Has(" is None"), STRONG),
            (Has(" is not None"), STRONG),
        ],
    },
    Lang {
        id: "ruby",
        signals: &[(Line("end"), LIKELY), (Start("puts "), STRONG), (Start("require '"), STRONG), (Start("require \""), STRONG), (Has(".each do"), DECISIVE), (Has(" do |"), STRONG), (Has("attr_accessor"), DECISIVE), (Start("def "), HINT)],
    },
    Lang {
        id: "javascript",
        signals: &[
            (Has("console.log("), DECISIVE),
            (Start("const "), LIKELY),
            (Start("var "), LIKELY),
            (Start("let "), HINT),
            (Has("function("), LIKELY),
            (StartHas("function ", "("), STRONG),
            (Has("=> "), LIKELY),
            (Has("=> {"), STRONG),
            (Has("require("), STRONG),
            (Start("export "), LIKELY),
            (StartHas("import ", " from "), STRONG),
            (Has("document."), LIKELY),
            (Has("==="), STRONG),
            (Has("!=="), STRONG),
            (Has("await "), HINT),
            (Has("async "), HINT),
        ],
    },
    Lang {
        id: "typescript",
        signals: &[
            (Has(": string"), STRONG),
            (Has(": number"), STRONG),
            (Has(": boolean"), STRONG),
            (Start("interface "), STRONG),
            (Start("export interface "), DECISIVE),
            (StartHas("type ", " = "), LIKELY),
            (Has(" as const"), STRONG),
            (Has("readonly "), LIKELY),
            (Start("const "), HINT),
            (Has("=> "), HINT),
            (StartHas("import ", " from "), LIKELY),
        ],
    },
    Lang {
        id: "go",
        signals: &[(Start("func "), STRONG), (Start("package "), DECISIVE), (Has(" := "), STRONG), (Has("fmt."), STRONG), (Has("err != nil"), DECISIVE), (Start("import ("), DECISIVE), (Has("go func"), STRONG), (Has("chan "), HINT)],
    },
    Lang {
        id: "c",
        signals: &[(Start("#include"), DECISIVE), (Has("int main("), DECISIVE), (Has("printf("), STRONG), (Has("malloc("), STRONG), (Has("sizeof("), LIKELY), (Start("#define "), DECISIVE), (Has("NULL"), HINT), (Start("void "), LIKELY), (Start("int "), HINT)],
    },
    Lang {
        id: "cpp",
        signals: &[(Has("std::"), DECISIVE), (Has("#include <iostream>"), DECISIVE), (Has("cout <<"), DECISIVE), (Start("template<"), DECISIVE), (Start("template <"), DECISIVE), (Has("nullptr"), DECISIVE), (Start("namespace "), LIKELY), (Start("#include"), STRONG), (Has("auto "), HINT)],
    },
    Lang {
        id: "java",
        signals: &[(Has("public static void main"), DECISIVE), (Has("System.out."), DECISIVE), (Start("import java"), DECISIVE), (Start("@Override"), DECISIVE), (Has("public class "), STRONG), (Start("private "), LIKELY), (Start("public "), HINT), (Has("new "), HINT)],
    },
    Lang {
        id: "kotlin",
        signals: &[(Start("fun "), DECISIVE), (Start("val "), LIKELY), (Start("data class "), DECISIVE), (Has("?.let"), DECISIVE), (Start("var "), HINT)],
    },
    Lang {
        id: "csharp",
        signals: &[(Start("using System"), DECISIVE), (Has("Console.WriteLine"), DECISIVE), (Has(" { get; set; }"), DECISIVE), (Start("namespace "), LIKELY), (Has("public class "), LIKELY)],
    },
    Lang { id: "php", signals: &[(Has("<?php"), DECISIVE), (Has("$this->"), DECISIVE), (Start("echo "), HINT), (Has("function "), HINT)] },
    Lang {
        id: "shell",
        signals: &[
            (Start("$ "), STRONG),
            (Start("sudo "), STRONG),
            (Line("fi"), STRONG),
            (Line("done"), STRONG),
            (Line("esac"), DECISIVE),
            (End("; then"), DECISIVE),
            (End("; do"), STRONG),
            (Has("2>&1"), DECISIVE),
            (Has("> /dev/null"), DECISIVE),
            (Has("$("), LIKELY),
            (Has("${"), LIKELY),
            (StartHas("export ", "="), LIKELY),
            (Start("echo "), LIKELY),
            (Has(" | "), HINT),
            (Has(" && "), HINT),
        ],
    },
    Lang { id: "powershell", signals: &[(Has("Write-Host"), DECISIVE), (Has("$env:"), DECISIVE), (Start("Get-"), STRONG), (Start("Set-"), LIKELY)] },
    Lang {
        id: "dockerfile",
        signals: &[(Start("FROM "), STRONG), (Start("RUN "), STRONG), (Start("WORKDIR "), DECISIVE), (Start("ENTRYPOINT "), DECISIVE), (Start("CMD ["), DECISIVE), (Start("EXPOSE "), STRONG), (Start("COPY "), LIKELY)],
    },
    Lang { id: "lua", signals: &[(Start("local function "), DECISIVE), (Start("local "), LIKELY), (Has(" ~= "), STRONG), (Line("end"), HINT)] },
    Lang { id: "nix", signals: &[(Has("mkDerivation"), DECISIVE), (Has("with pkgs;"), DECISIVE), (Has("pkgs."), STRONG), (Start("inherit "), STRONG)] },
    Lang { id: "diff", signals: &[(Start("diff --git"), DECISIVE), (Start("@@ "), STRONG), (Start("+++ "), STRONG), (Start("--- "), HINT)] },
];

/// Command-line tools whose invocation (with a flag, a path or a pipe)
/// reads as a shell snippet.
const COMMANDS: &[&str] = &[
    "sudo", "cd", "ls", "cp", "mv", "rm", "mkdir", "cat", "grep", "rg", "find", "sed", "awk", "curl", "wget", "git", "cargo", "npm", "npx", "pnpm", "yarn", "pip", "python", "python3", "docker",
    "podman", "kubectl", "systemctl", "journalctl", "pacman", "paru", "yay", "apt", "apt-get", "dnf", "zypper", "flatpak", "chmod", "chown", "ssh", "scp", "rsync", "tar", "make", "go", "rustup", "brew",
    "export", "source", "echo", "mandara", "niri", "hyprctl", "mmsg", "wl-copy", "wl-paste", "ffmpeg", "nix", "nix-shell",
];

/// HTML element names (markup with one of these is HTML, otherwise XML).
const HTML_TAGS: &[&str] = &[
    "html", "head", "body", "div", "span", "p", "a", "img", "ul", "ol", "li", "table", "tr", "td", "th", "form", "input", "button", "script", "style", "link", "meta", "section", "article", "nav",
    "header", "footer", "main", "h1", "h2", "h3", "h4", "h5", "h6", "br", "hr", "svg", "label", "select", "option", "textarea", "iframe", "video", "audio", "canvas", "template", "pre", "code", "em", "strong",
];

/// Per-line view of the scanned text.
struct Lines<'a> {
    all: Vec<&'a str>,
}

impl<'a> Lines<'a> {
    fn new(text: &'a str) -> Self {
        Self { all: text.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect() }
    }

    fn any(&self, p: P) -> bool {
        self.all.iter().any(|raw| {
            let l = raw.trim_start();
            match p {
                Start(s) => l.starts_with(s),
                Has(s) => l.contains(s),
                End(s) => l.ends_with(s),
                Line(s) => l == s,
                StartHas(a, b) => l.starts_with(a) && l[a.len()..].contains(b),
            }
        })
    }

    fn share(&self, f: impl Fn(&str) -> bool) -> f32 {
        if self.all.is_empty() {
            return 0.0;
        }
        self.all.iter().filter(|l| f(l)).count() as f32 / self.all.len() as f32
    }
}

/// A detected snippet: its language (`""` when only the structure says
/// code) and whether the evidence settles it (a format that parses, a
/// shebang, a command line or a decisive signal) rather than just leans.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Detected {
    pub lang: &'static str,
    pub sure: bool,
}

/// Code, with its best-guess language, or `None` for anything else.
pub fn detect(text: &str) -> Option<Detected> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let sure = |lang| Some(Detected { lang, sure: true });
    if let Some(lang) = structural(t) {
        return sure(lang);
    }
    let lines = Lines::new(t);
    if let Some(lang) = shebang(&lines) {
        return sure(lang);
    }
    let (mut best, mut best_score) = ("", 0u32);
    for lang in LANGS {
        let score: u32 = lang.signals.iter().filter(|(p, _)| lines.any(*p)).map(|(_, w)| u32::from(*w)).sum();
        if score > best_score {
            (best, best_score) = (lang.id, score);
        }
    }
    if command_line(&lines) && best_score < SURE {
        (best, best_score) = ("shell", SURE);
    }
    // TypeScript wins over JavaScript only with its own markers.
    if best == "javascript" {
        let ts = &LANGS.iter().find(|l| l.id == "typescript").expect("listed").signals;
        if ts.iter().any(|(p, w)| *w >= STRONG && lines.any(*p)) {
            best = "typescript";
        }
    }
    if best_score >= SURE {
        return sure(best);
    }
    let structure = structure(&lines, t);
    let prose = is_prose(t);
    if best_score >= PROBABLE && (structure > 0 || !prose) {
        return Some(Detected { lang: best, sure: false });
    }
    (!prose && structure >= STRUCTURE_ONLY && lines.all.len() > 1).then_some(Detected { lang: if best_score >= u32::from(LIKELY) { best } else { "" }, sure: false })
}

/// `#!/usr/bin/env python3` and friends.
fn shebang(lines: &Lines) -> Option<&'static str> {
    let first = lines.all.first()?.trim();
    let rest = first.strip_prefix("#!")?;
    let interp = rest.split_whitespace().last().unwrap_or(rest).rsplit('/').next().unwrap_or("");
    Some(match interp.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.') {
        "python" => "python",
        "node" | "deno" | "bun" => "javascript",
        "ruby" => "ruby",
        "perl" => "perl",
        "lua" => "lua",
        "php" => "php",
        "fish" => "fish",
        _ => "shell",
    })
}

/// A single command line: a known tool followed by a flag, path, pipe,
/// redirect or variable (`git commit -m "x"`, `cd ~/src`, `ls | wc -l`).
fn command_line(lines: &Lines) -> bool {
    lines.all.iter().take(1).any(|l| {
        let l = l.trim().trim_start_matches("$ ");
        let mut words = l.split_whitespace();
        let Some(first) = words.next() else { return false };
        if !COMMANDS.contains(&first) || l.ends_with(['.', '?', '!']) && !l.ends_with("..") {
            return false;
        }
        words.any(|w| w.starts_with('-') || w.contains('/') || w.starts_with('~') || w.starts_with('$') || matches!(w, "|" | "&&" | ">" | ">>" | "2>&1"))
    })
}

/// Formats recognized by shape: JSON, SQL, markup, YAML, TOML.
fn structural(t: &str) -> Option<&'static str> {
    if json(t) {
        return Some("json");
    }
    if sql(t) {
        return Some("sql");
    }
    if let Some(markup) = markup(t) {
        return Some(markup);
    }
    let lines = Lines::new(t);
    if toml(&lines) {
        return Some("toml");
    }
    if yaml(&lines) {
        return Some("yaml");
    }
    None
}

fn json(t: &str) -> bool {
    let wrapped = (t.starts_with('{') && t.ends_with('}')) || (t.starts_with('[') && t.ends_with(']'));
    if !wrapped {
        // A long document cut by the scan window: an object or array of
        // objects with a quoted first key.
        return t.starts_with("{\"") || t.starts_with("[{\"") || t.starts_with("{\n  \"");
    }
    serde_json::from_str::<serde::de::IgnoredAny>(t).is_ok() && t.len() > 2 && (t.contains(':') || t.contains(','))
}

fn sql(t: &str) -> bool {
    const LEADS: &[&str] = &["SELECT", "INSERT", "UPDATE", "DELETE", "CREATE", "ALTER", "DROP", "WITH"];
    const FOLLOWS: &[&str] = &["FROM", "INTO", "SET", "TABLE", "INDEX", "VIEW", "WHERE", "VALUES", "AS"];
    let mut words = t.split_whitespace();
    let Some(first) = words.next() else { return false };
    if !LEADS.iter().any(|k| first.eq_ignore_ascii_case(k)) {
        return false;
    }
    let follows = t.split(|c: char| !c.is_ascii_alphabetic()).any(|w| FOLLOWS.iter().any(|k| w.eq_ignore_ascii_case(k)));
    // Lower-case "select the best one from the list" is a sentence; SQL
    // shouts its keywords or carries symbols.
    let shouted = LEADS.contains(&first);
    follows && (shouted || t.contains(['*', ';', '=', '(']))
}

fn markup(t: &str) -> Option<&'static str> {
    if !(t.starts_with('<') && t.ends_with('>')) {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    if lower.starts_with("<!doctype html") {
        return Some("html");
    }
    if lower.starts_with("<?xml") {
        return Some("xml");
    }
    let name: String = t[1..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == ':').collect();
    if name.is_empty() || !name.starts_with(|c: char| c.is_ascii_alphabetic()) || !(t.contains("</") || t.contains("/>")) {
        return None;
    }
    Some(if HTML_TAGS.contains(&name.to_ascii_lowercase().as_str()) { "html" } else { "xml" })
}

/// `[section]` headers and `key = value` lines.
fn toml(lines: &Lines) -> bool {
    let section = |l: &str| {
        let l = l.trim();
        l.len() > 2 && l.starts_with('[') && l.ends_with(']') && l.trim_matches(['[', ']']).chars().all(|c| c.is_ascii_alphanumeric() || "._-\"' ".contains(c)) && !l.contains(' ')
    };
    let pair = |l: &str| l.split_once(" = ").is_some_and(|(k, v)| !k.trim().is_empty() && k.trim().chars().all(|c| c.is_ascii_alphanumeric() || "._-\"".contains(c)) && !v.trim().is_empty());
    let comment = |l: &str| l.trim_start().starts_with('#');
    lines.all.iter().any(|l| section(l)) && lines.all.iter().any(|l| pair(l)) && lines.share(|l| section(l) || pair(l) || comment(l)) >= SHAPE_SHARE
}

/// `key: value` maps with nesting or lists (flat `Name: Ann` notes stay text).
fn yaml(lines: &Lines) -> bool {
    if lines.all.len() < YAML_MIN_LINES || lines.all.iter().any(|l| l.contains(['{', '}', ';'])) {
        return false;
    }
    let key = |l: &str| {
        let l = l.trim_start().trim_start_matches("- ");
        l.split_once(':').is_some_and(|(k, v)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c)) && (v.is_empty() || v.starts_with(' ')))
    };
    let item = |l: &str| l.trim_start().starts_with("- ");
    let nested = lines.all.iter().skip(1).any(|l| l.starts_with([' ', '\t']) && (key(l) || item(l)));
    let header = lines.all.first().is_some_and(|l| *l == "---");
    (nested || header) && lines.share(|l| key(l) || item(l) || l == "---" || l.trim_start().starts_with('#')) >= SHAPE_SHARE
}

/// Language-agnostic code structure: line terminators, balanced braces,
/// operators, indentation.
fn structure(lines: &Lines, t: &str) -> u32 {
    const OPERATORS: &[&str] = &["==", "!=", "&&", "||", "+=", "-=", "->", "=>", "::", "();", "){", ") {", "[]", "//", "/*"];
    let mut score = OPERATORS.iter().filter(|op| t.contains(*op)).count() as u32;
    if lines.share(|l| l.trim_end().ends_with([';', '{', '}', ','])) >= TERMINATOR_SHARE {
        score += 2;
    }
    if t.contains('{') && t.contains('}') {
        score += 1;
    }
    if lines.all.len() >= 3 && lines.share(|l| l.starts_with([' ', '\t'])) >= INDENT_SHARE {
        score += 1;
    }
    score
}

/// Sentences: enough words and hardly any code symbols.
fn is_prose(t: &str) -> bool {
    let words = t.split_whitespace().filter(|w| w.chars().any(char::is_alphabetic)).count();
    let chars = t.chars().count().max(1);
    let symbols = t.chars().filter(|c| "{}[]();=<>$|&\\".contains(*c)).count();
    words >= PROSE_MIN_WORDS && (symbols as f32 * 100.0 / chars as f32) < PROSE_SYMBOLS_PER_100
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages() {
        let cases: &[(&str, &str)] = &[
            ("fn main() {\n    println!(\"hi\");\n}", "rust"),
            ("let mut v = vec![1, 2];\nv.push(3);", "rust"),
            ("use std::collections::HashMap;", "rust"),
            ("#[derive(Debug)]\npub struct Point { x: f32, y: f32 }", "rust"),
            ("def greet(name):\n    print(f\"hi {name}\")", "python"),
            ("from pathlib import Path\nimport os\n\nif __name__ == \"__main__\":\n    main()", "python"),
            ("class Foo:\n    def __init__(self):\n        self.x = 1", "python"),
            ("console.log('hello')", "javascript"),
            ("const add = (a, b) => {\n  return a + b;\n};", "javascript"),
            ("import React from 'react';\nexport default function App() {\n  return null;\n}", "javascript"),
            ("export interface User {\n  name: string;\n  age: number;\n}", "typescript"),
            ("const n: number = 3;\nlet s: string = 'x';", "typescript"),
            ("package main\n\nimport \"fmt\"\n\nfunc main() {\n\tfmt.Println(\"hi\")\n}", "go"),
            ("if err != nil {\n\treturn err\n}", "go"),
            ("#include <stdio.h>\nint main(void) {\n  printf(\"hi\\n\");\n  return 0;\n}", "c"),
            ("#include <iostream>\nint main() { std::cout << \"hi\"; }", "cpp"),
            ("public class Hello {\n  public static void main(String[] args) {\n    System.out.println(\"hi\");\n  }\n}", "java"),
            ("fun main() {\n    val x = 1\n}", "kotlin"),
            ("using System;\nConsole.WriteLine(\"hi\");", "csharp"),
            ("<?php echo $name; ?>", "php"),
            ("#!/bin/bash\nset -e\necho done", "shell"),
            ("#!/usr/bin/env python3\nprint('x')", "python"),
            ("if [ -f x ]; then\n  rm x\nfi", "shell"),
            ("sudo pacman -Syu", "shell"),
            ("git commit -m \"fix: things\"", "shell"),
            ("cd ~/src/mandara && cargo build --release", "shell"),
            ("$ ls -la", "shell"),
            ("ls | wc -l", "shell"),
            ("Write-Host \"hi\"", "powershell"),
            ("FROM rust:1.80\nWORKDIR /app\nRUN cargo build", "dockerfile"),
            ("local function f(x)\n  return x\nend", "lua"),
            ("{ pkgs, ... }:\npkgs.mkDerivation { name = \"x\"; }", "nix"),
            ("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@", "diff"),
            ("{\"name\": \"mandara\", \"version\": 1}", "json"),
            ("[1, 2, 3]", "json"),
            ("SELECT * FROM users WHERE id = 1;", "sql"),
            ("select name from users where id = 1;", "sql"),
            ("INSERT INTO t (a) VALUES (1)", "sql"),
            ("<div class=\"x\"><p>Hi</p></div>", "html"),
            ("<!DOCTYPE html>\n<html></html>", "html"),
            ("<?xml version=\"1.0\"?>\n<note></note>", "xml"),
            ("<config><item key=\"a\"/></config>", "xml"),
            ("[package]\nname = \"mandara\"\nversion = \"0.4.0\"", "toml"),
            ("server:\n  host: localhost\n  port: 8080\nlogging:\n  level: debug", "yaml"),
            ("---\nname: x\nversion: 1", "yaml"),
        ];
        for (text, want) in cases {
            assert_eq!(detect(text).map(|d| d.lang), Some(*want), "{text:?}");
        }
    }

    #[test]
    fn prose_and_near_misses_are_not_code() {
        let cases = [
            "Let me know when you are free tomorrow.",
            "import this",
            "I think (maybe) we should go; it's late.",
            "Select the best option from the list below.",
            "Name: Ann\nPhone: 555 0100\nCity: Alexandria",
            "The function returns a value, and then the caller uses it.",
            "hello world",
            "x = 5",
            "Meeting at 5 => bring snacks",
            "git is a version control system.",
            "[citation needed]",
            "<3 see you soon >",
            "a < b and c > d",
            "Do this first; then do that.",
            "TODO: buy milk",
            "end",
            "{}",
        ];
        for text in cases {
            assert_eq!(detect(text), None, "{text:?}");
        }
    }

    #[test]
    fn structure_without_a_language_is_still_code() {
        assert_eq!(detect("x += 1;\ny -= 2;\nif (x == y) { z(); }"), Some(Detected { lang: "", sure: false }));
    }
}
