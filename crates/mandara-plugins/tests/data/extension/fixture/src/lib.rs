//! One `extension-plugin` component whose `mode` setting picks what the
//! host's extension tests see. See ../README.md.

use std::sync::Mutex;

wit_bindgen::generate!({ world: "plugin", path: "../../../../wit" });

use mandara::plugin::broker;
use mandara::plugin::storage;
use mandara::plugin::host;
use mandara::plugin::types::Tone;
use mandara::plugin::ui;

/// What happened, newest last; the view's first text node shows it.
static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
static SEARCH: Mutex<Option<(String, u64, bool)>> = Mutex::new(None);
static PROJECT: Mutex<Option<u64>> = Mutex::new(None);
static IMAGE: Mutex<u64> = Mutex::new(0);

fn log(line: String) {
    LOG.lock().unwrap().push(line);
}

fn mode() -> String {
    host::setting("mode").unwrap_or_default().trim_matches('"').to_string()
}

fn style() -> ui::Style {
    ui::Style { width: 0.0, height: 0.0, min_width: 0.0, min_height: 0.0, padding: 0.0, gap: 0.0, radius: 0.0, size: 0.0, grow: false, bold: false, align: ui::Align::Auto, tone: Tone::Normal }
}

fn node(kind: ui::Kind, id: &str, text: &str) -> ui::Node {
    ui::Node { id: id.into(), kind, text: text.into(), hint: String::new(), action: id.into(), value: 0.0, min: 0.0, max: 1.0, step: 0.0, index: 0, children: Vec::new(), style: style(), options: Vec::new(), image: 0, marks: Vec::new(), disabled: false }
}

fn submit(capability: &str, operation: &str, payload: &str) -> Result<u64, String> {
    broker::submit(&broker::Request { capability: capability.into(), operation: operation.into(), payload: payload.into() })
}

struct Fixture;

impl Guest for Fixture {
    fn negotiate(host: HostInfo) -> Vec<String> {
        log(format!("api {}", host.api));
        match mode().as_str() {
            "future" => vec!["ui.layout".into(), "ui.webview".into()],
            _ => vec!["ui.layout".into(), "ui.input".into(), "ui.list".into(), "launcher".into(), "broker".into()],
        }
    }

    fn init() -> Result<(), String> {
        Ok(())
    }

    fn refresh_ms() -> u32 {
        0
    }

    fn render(_theme: Palette, context: ui::Context) -> Option<ui::View> {
        if mode() == "huge" {
            let mut nodes: Vec<ui::Node> = (0..5000).map(|i| node(ui::Kind::Text, "", &format!("row {i}"))).collect();
            nodes[0].children = (1..5000).collect();
            nodes[0].kind = ui::Kind::Column;
            return Some(ui::View { nodes, root: 0, focus: String::new() });
        }
        if context.surface == ui::Surface::Bar {
            return Some(ui::View { nodes: vec![node(ui::Kind::Text, "", "Fixture")], root: 0, focus: String::new() });
        }
        if mode() == "live" {
            let mut root = node(ui::Kind::Column, "root", "");
            root.children = vec![6, 1, 2, 3, 4, 5];
            let mut nodes = vec![root];
            for action in ["status", "projects", "render", "stop"] {
                nodes.push(node(ui::Kind::Button, action, action));
            }
            nodes.push(node(ui::Kind::Text, "", &LOG.lock().unwrap().join("\n")));
            let mut image = node(ui::Kind::Image, "thumbnail", "");
            image.image = *IMAGE.lock().unwrap();
            nodes.push(image);
            return Some(ui::View { nodes, root: 0, focus: "status".into() });
        }
        if mode() == "sneaky" {
            // Rendering isn't a user gesture: this must be refused.
            log(format!("render-clip {:?}", submit("clipboard.read", "text", "{}")));
        }
        let mut root = node(ui::Kind::Column, "root", "");
        root.children = vec![1, 2, 3, 4];
        let status = node(ui::Kind::Text, "", &LOG.lock().unwrap().join("\n"));
        let mut field = node(ui::Kind::Input, "field", "typed");
        field.hint = "Type here".into();
        let toggle = node(ui::Kind::Toggle, "toggle", "Switch");
        let mut list = node(ui::Kind::List, "list", "");
        list.index = 500;
        list.children = vec![5];
        let row = node(ui::Kind::Text, "", "row 0");
        Some(ui::View { nodes: vec![root, status, field, toggle, list, row], root: 0, focus: "field".into() })
    }

    fn on_event(event: ui::Event) {
        match event.value {
            ui::Value::Replies(replies) => {
                for r in replies {
                    let _ = storage::set("last-id", &r.id.to_string());
                    {
                        let mut search = SEARCH.lock().unwrap();
                        if let Some((_, id, ready)) = search.as_mut() {
                            if *id == r.id {
                                if r.outcome.is_ok() { *ready = true; } else { *search = None; }
                            }
                        }
                    }
                    match r.outcome {
                        Ok(body) => {
                            let _ = storage::set("last", &body);
                            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
                                if let Some(project) = value["items"][0]["project"].as_u64() { *PROJECT.lock().unwrap() = Some(project); }
                                if let Some(image) = value["image"].as_u64() { *IMAGE.lock().unwrap() = image; }
                            }
                            log(format!("reply {} ok {body}", r.id));
                        }
                        Err(e) => {
                            let _ = storage::set("last", &serde_json::json!({"error":e}).to_string());
                            log(format!("reply {} err {e}", r.id));
                        }
                    }
                }
                return;
            }
            ui::Value::Text(t) => return log(format!("text {t}")),
            ui::Value::Launch(id) => return log(format!("launch {id}")),
            ui::Value::Command(words) => {
                if words.len() == 4 && words[0] == "request" {
                    log(format!("request {:?}", submit(&words[1], &words[2], &words[3])));
                }
                return log(format!("command {}", words.join(" ")));
            }
            _ => {}
        }
        match event.action.as_str() {
            "status" | "stop" => log(format!("live {:?}", submit("wallpaper.live", &event.action, "{}"))),
            "projects" => log(format!("projects {:?}", submit("files.folders", "projects", r#"{"folder":0}"#))),
            "render" => {
                let project = *PROJECT.lock().unwrap();
                log(format!("live {:?}", project.map(|project| submit("wallpaper.live", "render", &serde_json::json!({"project":project}).to_string()))));
            }
            "media" => log(format!("media {:?}", submit("media.read", "status", "{}"))),
            "subscribe" => log(format!("subscribe {:?}", submit("media.read", "subscribe", ""))),
            "undeclared" => log(format!("undeclared {:?}", submit("wallpaper.set", "apply", "{\"image\": 1}"))),
            "unknown" => log(format!("unknown {:?}", submit("media.read", "erase", "{}"))),
            "asset" => log(format!("asset {:?} {:?}", broker::asset_image("moon.png"), broker::asset_image("../plugin.toml"))),
            "granted" => log(format!("granted {:?}", broker::granted())),
            "clip" => log(format!("clip {:?}", submit("clipboard.read", "text", "{}"))),
            "store" => log(format!("store {:?} {:?}", storage::set("greeting", "salam"), storage::set("", "x"))),
            "recall" => log(format!("recall {:?} {:?}", storage::get("greeting"), storage::keys())),
            "flood" => {
                let accepted = (0..100).filter(|_| submit("media.control", "next", "{}").is_ok()).count();
                log(format!("flood {accepted}"));
            }
            other => log(format!("event {other}")),
        }
    }

    fn launcher_query(_provider: String, query: String, _limit: u32) -> Vec<ui::Hit> {
        if mode() == "async" {
            let mut search = SEARCH.lock().unwrap();
            if search.as_ref().is_none_or(|(previous, _, _)| previous != &query) {
                *search = submit("files.folders", "folders", "{}").ok().map(|id| (query.clone(), id, false));
            }
            if !search.as_ref().is_some_and(|(_, _, ready)| *ready) { return Vec::new(); }
            let _ = storage::set("launcher-ready", &query);
            return (0..3).map(|i| ui::Hit { id: format!("async-{i}"), title: format!("{query} result {i}"), detail: "Asynchronous reply received".into(), icon: "folder".into(), image: 0, keywords: String::new() }).collect();
        }
        if mode() == "spin" {
            loop {
                std::hint::spin_loop();
            }
        }
        (0..100).map(|i| ui::Hit { id: format!("hit-{i}"), title: format!("{query} {}", "x".repeat(1000)), detail: String::new(), icon: "moon".into(), image: 0, keywords: String::new() }).collect()
    }

    fn on_call(caller: String, operation: String, payload: String) -> Result<String, String> {
        match operation.as_str() {
            "ping" => Ok(format!("{{\"pong\": \"{caller}\", \"payload\": {payload}}}")),
            _ => Err(format!("no operation {operation}")),
        }
    }
}

export!(Fixture);
