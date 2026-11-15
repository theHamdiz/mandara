//! One component, many behaviors: the `mode` setting (its default comes
//! from the plugin.toml each test writes) picks what the compliance check
//! sees. See ../README.md.

wit_bindgen::generate!({ world: "plugin", path: "../../../../wit" });

use mandara::plugin::host;
use mandara::plugin::types::Tone;
use mandara::plugin::ui;

/// `ok` (a labelled bar item), `empty` (an empty label), `trap` (init
/// panics), `spin` (rendering never returns) or `eager` (init queues a
/// command).
fn mode() -> String {
    host::setting("mode").unwrap_or_default().trim_matches('"').to_string()
}

fn label(text: &str) -> View {
    let style = ui::Style { width: 0.0, height: 0.0, min_width: 0.0, min_height: 0.0, padding: 0.0, gap: 0.0, radius: 0.0, size: 0.0, grow: false, bold: false, align: ui::Align::Auto, tone: Tone::Normal };
    let node = ui::Node { id: String::new(), kind: ui::Kind::Text, text: text.into(), hint: String::new(), action: String::new(), value: 0.0, min: 0.0, max: 0.0, step: 0.0, index: 0, children: Vec::new(), style, options: Vec::new(), image: 0, marks: Vec::new(), disabled: false };
    View { nodes: vec![node], root: 0, focus: String::new() }
}

struct Plugin;

impl Guest for Plugin {
    fn negotiate(_host: HostInfo) -> Vec<String> {
        vec!["ui.layout".into()]
    }

    fn init() -> Result<(), String> {
        match mode().as_str() {
            "trap" => panic!("fixture: trap on init"),
            "eager" => host::command("shell restart"),
            _ => Ok(()),
        }
    }

    fn render(_theme: Palette, context: Context) -> Option<View> {
        if context.surface != ui::Surface::Bar {
            return None;
        }
        match mode().as_str() {
            "empty" => Some(label(" ")),
            "spin" => loop {
                std::hint::spin_loop();
            },
            _ => Some(label("Fixture")),
        }
    }

    fn on_event(_event: Event) {}

    fn refresh_ms() -> u32 {
        0
    }

    fn launcher_query(_provider: String, _query: String, _limit: u32) -> Vec<Hit> {
        Vec::new()
    }

    fn on_call(_caller: String, _operation: String, _payload: String) -> Result<String, String> {
        Err("no operations".into())
    }
}

export!(Plugin);
