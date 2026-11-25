//! A tiny StatusNotifierItem with a dbusmenu, for testing Mandara's tray:
//! a generated gradient pixmap icon, a tooltip, and a menu with a plain
//! entry, a checkmark, a radio group in a submenu, a disabled entry, icons,
//! a shortcut and Quit.
//!
//! `cargo run -p mandara-services --example tray_item`
//!
//! Left-click (Activate) bumps a counter in the title, middle-click
//! (SecondaryActivate) toggles NeedsAttention, scrolling prints the delta.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};
use zbus::{Connection, interface};

const MENU_PATH: &str = "/MenuBar";

#[derive(Default)]
struct Shared {
    activations: u32,
    attention: bool,
    notifications: bool,
    mode: i32,
    revision: u32,
}

type Pixmaps = Vec<(i32, i32, Vec<u8>)>;

/// An ARGB32 disc with a teal → violet diagonal gradient.
fn pixmap(side: i32) -> Pixmaps {
    let mut data = Vec::with_capacity((side * side * 4) as usize);
    let c = side as f32 / 2.0;
    for y in 0..side {
        for x in 0..side {
            let d = ((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c).powi(2)).sqrt();
            let a = ((c - d) * 1.5).clamp(0.0, 1.0);
            let t = (x + y) as f32 / (2 * side) as f32;
            let (r, g, b) = (40.0 + 140.0 * t, 190.0 - 90.0 * t, 200.0 + 40.0 * t);
            data.extend_from_slice(&[(a * 255.0) as u8, r as u8, g as u8, b as u8]);
        }
    }
    vec![(side, side, data)]
}

struct Item(Arc<Mutex<Shared>>);

#[interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    fn activate(&self, x: i32, y: i32) {
        let n = {
            let mut s = self.0.lock();
            s.activations += 1;
            s.activations
        };
        println!("Activate({x}, {y}) #{n}");
    }

    async fn secondary_activate(&self, x: i32, y: i32, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        let attention = {
            let mut s = self.0.lock();
            s.attention = !s.attention;
            s.attention
        };
        println!("SecondaryActivate({x}, {y}) → attention {attention}");
        let _ = Item::new_status(&emitter, if attention { "NeedsAttention" } else { "Active" }).await;
    }

    fn scroll(&self, delta: i32, orientation: &str) {
        println!("Scroll({delta}, {orientation})");
    }

    fn context_menu(&self, x: i32, y: i32) {
        println!("ContextMenu({x}, {y})");
    }

    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }
    #[zbus(property)]
    fn id(&self) -> &str {
        "mandara-tray-demo"
    }
    #[zbus(property)]
    fn title(&self) -> String {
        "Mandara Demo".into()
    }
    #[zbus(property)]
    fn status(&self) -> &str {
        if self.0.lock().attention { "NeedsAttention" } else { "Active" }
    }
    #[zbus(property)]
    fn icon_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn icon_theme_path(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn icon_pixmap(&self) -> Pixmaps {
        pixmap(48)
    }
    #[zbus(property)]
    fn attention_icon_name(&self) -> &str {
        "dialog-warning"
    }
    #[zbus(property)]
    fn tool_tip(&self) -> (String, Pixmaps, String, String) {
        let n = self.0.lock().activations;
        (String::new(), Vec::new(), "Mandara Demo".into(), format!("Activated {n} times"))
    }
    #[zbus(property)]
    fn menu(&self) -> ObjectPath<'_> {
        ObjectPath::from_static_str_unchecked(MENU_PATH)
    }
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }

    #[zbus(signal)]
    async fn new_status(emitter: &SignalEmitter<'_>, status: &str) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn new_title(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn new_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn new_tool_tip(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

struct Menu(Arc<Mutex<Shared>>);

type Props = HashMap<String, OwnedValue>;

fn props(pairs: &[(&str, Value<'_>)]) -> Props {
    pairs.iter().map(|(k, v)| (k.to_string(), OwnedValue::try_from(v.try_clone().unwrap()).unwrap())).collect()
}

fn node(id: i32, p: Props, children: Vec<OwnedValue>) -> OwnedValue {
    OwnedValue::try_from(Value::from((id, p, children))).unwrap()
}

impl Menu {
    fn tree(&self) -> (i32, Props, Vec<OwnedValue>) {
        let s = self.0.lock();
        let radio = |id: i32, label: &str| node(id, props(&[("label", label.into()), ("toggle-type", "radio".into()), ("toggle-state", (if s.mode == id { 1 } else { 0 }).into())]), vec![]);
        let modes = vec![radio(21, "Balanced"), radio(22, "Performance"), radio(23, "Power saver")];
        let shortcut = Value::from(vec![vec!["Control".to_string(), "q".to_string()]]);
        let children = vec![
            node(1, props(&[("label", "_Open Dashboard".into()), ("icon-name", "go-home".into())]), vec![]),
            node(2, props(&[("type", "separator".into())]), vec![]),
            node(3, props(&[("label", "Notifications".into()), ("toggle-type", "checkmark".into()), ("toggle-state", (if s.notifications { 1 } else { 0 }).into())]), vec![]),
            node(20, props(&[("label", "Power mode".into()), ("children-display", "submenu".into())]), modes),
            node(4, props(&[("label", "Version 1.0".into()), ("enabled", false.into())]), vec![]),
            node(5, props(&[("label", "Hidden entry".into()), ("visible", false.into())]), vec![]),
            node(6, props(&[("type", "separator".into())]), vec![]),
            node(9, props(&[("label", "Quit".into()), ("icon-name", "application-exit".into()), ("shortcut", shortcut)]), vec![]),
        ];
        (0, props(&[("children-display", "submenu".into())]), children)
    }
}

#[interface(name = "com.canonical.dbusmenu")]
impl Menu {
    fn get_layout(&self, parent_id: i32, _depth: i32, _names: Vec<String>) -> (u32, (i32, Props, Vec<OwnedValue>)) {
        let rev = self.0.lock().revision;
        println!("GetLayout({parent_id})");
        (rev, self.tree())
    }

    fn get_group_properties(&self, _ids: Vec<i32>, _names: Vec<String>) -> Vec<(i32, Props)> {
        Vec::new()
    }

    fn about_to_show(&self, id: i32) -> bool {
        println!("AboutToShow({id})");
        false
    }

    async fn event(&self, id: i32, event_id: &str, _data: OwnedValue, _timestamp: u32, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        println!("Event({id}, {event_id})");
        if event_id != "clicked" {
            return;
        }
        let rev = {
            let mut s = self.0.lock();
            match id {
                3 => s.notifications = !s.notifications,
                21..=23 => s.mode = id,
                9 => std::process::exit(0),
                _ => {}
            }
            s.revision += 1;
            s.revision
        };
        let _ = Menu::layout_updated(&emitter, rev, 0).await;
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }
    #[zbus(property)]
    fn status(&self) -> &str {
        "normal"
    }

    #[zbus(signal)]
    async fn layout_updated(emitter: &SignalEmitter<'_>, revision: u32, parent: i32) -> zbus::Result<()>;
}

fn main() -> zbus::Result<()> {
    zbus::block_on(async {
        let shared = Arc::new(Mutex::new(Shared { notifications: true, mode: 21, ..Default::default() }));
        let name = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
        let conn: Connection = zbus::connection::Builder::session()?
            .name(name.as_str())?
            .serve_at("/StatusNotifierItem", Item(shared.clone()))?
            .serve_at(MENU_PATH, Menu(shared))?
            .build()
            .await?;
        let watcher = zbus::Proxy::new(&conn, "org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher", "org.kde.StatusNotifierWatcher").await?;
        match watcher.call::<_, _, ()>("RegisterStatusNotifierItem", &(name.as_str(),)).await {
            Ok(()) => println!("registered as {name}"),
            Err(e) => println!("no watcher yet ({e}); waiting to be adopted as {name}"),
        }
        std::future::pending::<()>().await;
        Ok(())
    })
}
