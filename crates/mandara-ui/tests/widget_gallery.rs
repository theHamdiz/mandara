//! Gallery factories and painters agree on the ten additional widget styles.

use mandara_config::desktop_widgets::{Anchor, DesktopWidget, plugin_ids};
use mandara_plugins::PluginRegistry;

fn collection() -> Vec<(String, DesktopWidget)> {
    let mut registry = PluginRegistry::default();
    mandara_ui::desktop_plugins::register(&mut registry);
    [(plugin_ids::CLOCKS, "word"), (plugin_ids::CLOCKS, "split-flap"), (plugin_ids::CLOCKS, "nixie"), (plugin_ids::CLOCKS, "binary"),
        (plugin_ids::CALENDAR, "ticket"), (plugin_ids::CALENDAR, "perpetual"), (plugin_ids::CALENDAR, "moon-calendar"),
        (plugin_ids::WEATHER, "ribbon"), (plugin_ids::WEATHER, "thermometer"), (plugin_ids::WEATHER, "humidity-rose")]
        .into_iter().map(|(plugin, id)| {
            let mut widgets = registry.make_desktop_widgets(&format!("{plugin}/{id}")).unwrap();
            assert_eq!(widgets.len(), 1, "{plugin}/{id}");
            (id.into(), widgets.remove(0))
        }).collect()
}

#[test]
fn new_styles_are_distinct_and_eligible_for_public_screens() {
    let widgets = collection();
    let distinct: std::collections::BTreeSet<_> = widgets.iter().map(|(_, widget)| serde_json::to_string(&widget.kind).unwrap()).collect();
    assert_eq!(distinct.len(), 10);
    for (_, widget) in widgets {
        assert!(mandara_config::preauth::copy(&widget).is_some());
    }
}

#[test]
#[ignore = "writes preview artifacts for a manual visual check"]
fn export_widget_previews() {
    let dir = std::path::PathBuf::from(std::env::var_os("MANDARA_WIDGET_ARTIFACTS").expect("set MANDARA_WIDGET_ARTIFACTS to a scratch directory"));
    std::fs::create_dir_all(&dir).unwrap();
    let fonts = mandara_ui::backdrop::paint::FontNames::new("Inter", "Inter", "Cairo", "JetBrains Mono");
    let mut text = mandara_ui::lock::paint::TextPainter::new();
    let palette = mandara_theme::engine::default_palette(true);
    let mut placed = Vec::new();
    for (i, (id, mut widget)) in collection().into_iter().enumerate() {
        let preview = mandara_ui::backdrop::widgets::thumbnail(&widget, (600, 400), &fonts, palette.primary, &mut text).unwrap();
        assert!(preview.pixels().any(|p| p[3] != 0), "{id} has no visible ink");
        preview.save(dir.join(format!("{id}.png"))).unwrap();
        let (column, row, columns) = if i < 4 { (i, 0, 4) } else if i < 7 { (i - 4, 1, 3) } else { (i - 7, 2, 3) };
        widget.id = format!("demo-{id}");
        widget.place.anchor = Anchor::TopLeft;
        widget.place.x = 0.04 + column as f32 / columns as f32;
        widget.place.y = 0.08 + row as f32 * 0.31;
        widget.place.scale = 0.65;
        placed.push(widget);
    }
    std::fs::write(dir.join("widgets.json"), serde_json::to_vec(&placed).unwrap()).unwrap();
}
