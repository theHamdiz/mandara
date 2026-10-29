//! One component, many behaviors: the `mode` setting (its default comes
//! from the plugin.toml each test writes) picks what the compliance check
//! sees. See ../README.md.

wit_bindgen::generate!({ world: "plugin", path: "../../../../wit" });

use mandara::plugin::host;
use mandara::plugin::types::{Kind, Node, Tone};

/// `ok` (a labelled bar item), `empty` (an empty label), `trap` (init
/// panics), `spin` (the bar never returns) or `eager` (init queues a
/// command).
fn mode() -> String {
    host::setting("mode").unwrap_or_default().trim_matches('"').to_string()
}

fn label(text: &str) -> View {
    let node = Node { kind: Kind::Text, text: text.into(), value: 0.0, children: Vec::new(), action: String::new(), tone: Tone::Normal, size: 0.0, gap: 0.0, bold: false, grow: false };
    View { nodes: vec![node], root: 0 }
}

struct Plugin;

impl Guest for Plugin {
    fn init() -> Result<(), String> {
        match mode().as_str() {
            "trap" => panic!("fixture: trap on init"),
            "eager" => host::command("shell restart"),
            _ => Ok(()),
        }
    }

    fn bar(_theme: Palette) -> Option<View> {
        match mode().as_str() {
            "empty" => Some(label(" ")),
            "spin" => loop {
                std::hint::spin_loop();
            },
            _ => Some(label("Fixture")),
        }
    }

    fn panel(_theme: Palette) -> Option<View> {
        None
    }

    fn sidebar(_theme: Palette) -> Option<View> {
        None
    }

    fn on_event(_action: String) {}

    fn refresh_ms() -> u32 {
        0
    }
}

export!(Plugin);
