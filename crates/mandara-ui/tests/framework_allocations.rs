//! Font defaults share feature data while custom font edits stay independent.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::hint::black_box;
use std::sync::Arc;

use gpui::FontFeatures;

#[path = "support/allocations.rs"]
mod allocations;
use allocations::allocations;

/// Enough repeated construction to distinguish sharing from allocator reuse.
const CONSTRUCTIONS: usize = 64;

fn hash(features: &FontFeatures) -> u64 {
    let mut state = DefaultHasher::new();
    features.hash(&mut state);
    state.finish()
}

#[test]
fn font_and_text_style_feature_defaults_share_without_allocating_per_instance() {
    let first = FontFeatures::default();
    let second = FontFeatures::default();
    assert!(Arc::ptr_eq(&first.0, &second.0));
    assert!(Arc::ptr_eq(&first.0, &gpui::TextStyle::default().font_features.0));
    assert!(first.tag_value_list().is_empty());
    assert_eq!(first.is_calt_enabled(), None);
    let old = allocations(|| {
        for _ in 0..CONSTRUCTIONS {
            black_box(FontFeatures(Arc::new(Vec::new())));
        }
    });
    assert_eq!(old.0, CONSTRUCTIONS, "the former default must remain an allocating positive control");
    let current = allocations(|| {
        for _ in 0..CONSTRUCTIONS {
            black_box(FontFeatures::default());
            black_box(gpui::font("Inter"));
            black_box(gpui::TextStyle::default());
        }
    });
    assert_eq!(current, (0, 0));
    eprintln!("empty font-feature defaults: old={old:?}, shared={current:?} over {CONSTRUCTIONS} constructions");
}

#[test]
fn shared_empty_features_preserve_wire_hash_and_copy_on_write_behavior() {
    let original = FontFeatures::default();
    let mut custom = original.clone();
    Arc::make_mut(&mut custom.0).push(("tnum".into(), 1));
    assert!(original.tag_value_list().is_empty());
    assert_eq!(custom.tag_value_list(), &[("tnum".to_owned(), 1)]);
    assert!(!Arc::ptr_eq(&original.0, &custom.0));
    let decoded: FontFeatures = serde_json::from_str("{}").unwrap();
    assert!(Arc::ptr_eq(&original.0, &decoded.0));
    assert_eq!(hash(&original), hash(&FontFeatures(Arc::new(Vec::new()))));
    assert_eq!(serde_json::to_string(&original).unwrap(), "{}");
    let decoded: FontFeatures = serde_json::from_str(r#"{"calt":false,"tnum":1}"#).unwrap();
    assert_eq!(decoded.is_calt_enabled(), Some(false));
    assert_eq!(serde_json::to_string(&decoded).unwrap(), r#"{"calt":0,"tnum":1}"#);
}

#[test]
fn repeated_tabular_figures_share_their_exact_features_and_custom_edits_stay_local() {
    let first = mandara_ui::fonts::tabular_nums();
    assert_eq!(first.tag_value_list(), &[("tnum".to_owned(), 1)]);
    let old = allocations(|| {
        for _ in 0..CONSTRUCTIONS { black_box(FontFeatures(Arc::new(vec![("tnum".into(), 1)]))); }
    });
    assert!(old.0 >= CONSTRUCTIONS);
    let current = allocations(|| {
        for _ in 0..CONSTRUCTIONS { black_box(mandara_ui::fonts::tabular_nums()); }
    });
    assert_eq!(current, (0, 0));
    let mut edited = mandara_ui::fonts::tabular_nums();
    assert!(Arc::ptr_eq(&first.0, &edited.0));
    Arc::make_mut(&mut edited.0).push(("calt".into(), 0));
    assert_eq!(first.is_calt_enabled(), None);
    assert_eq!(edited.is_calt_enabled(), Some(false));
    assert_eq!(first, mandara_ui::fonts::tabular_nums());
    eprintln!("tabular font features: old={old:?}, shared={current:?} over {CONSTRUCTIONS} constructions");
}

#[test]
fn desktop_widget_keys_borrow_strings_and_cover_every_pixel_setting() {
    use mandara_config::desktop_widgets::{DesktopWidget,Depth,Anchor};
    use mandara_ui::backdrop::widgets::content_key;
    let widget = DesktopWidget { id:"clock-with-a-long-header".into(),..Default::default() };
    let old = allocations(|| { for _ in 0..CONSTRUCTIONS {
        let mut copy = widget.clone();copy.place.x = 0.0;copy.place.y = 0.0;copy.place.displays.clear();copy.place.depth = Depth::Behind;
        black_box(format!("{copy:?}"));
    } });
    let current = allocations(|| { for _ in 0..CONSTRUCTIONS { black_box(content_key(&widget)); } });
    assert!(old.0 >= CONSTRUCTIONS);
    assert_eq!(current,(0,0));
    let key = content_key(&widget);
    let mut moved = widget.clone();
    moved.place.x = 0.5;moved.place.y = -0.3;moved.place.displays.push("DP-2".into());moved.place.depth = Depth::Front;
    assert_eq!(content_key(&moved),key);
    moved.place.anchor = Anchor::TopLeft;assert_ne!(content_key(&moved),key);
    for field in ["scale","opacity","rotation","flip_x","flip_y"] {
        let mut value = serde_json::to_value(&widget).unwrap();
        value["place"][field] = match field { "flip_x"|"flip_y" => true.into(),"rotation" => 45.into(),_ => 0.5.into() };
        let changed:DesktopWidget = serde_json::from_value(value).unwrap();
        assert_ne!(content_key(&changed),key,"{field}");
    }
    let mut changed = widget.clone();changed.kind.look_mut().unwrap().header = "New header".into();
    assert_ne!(content_key(&changed),key);
    eprintln!("widget keys over {CONSTRUCTIONS} frames: cloned={old:?}, borrowed={current:?}");
}

#[test]
fn widget_font_selection_borrows_the_scene_and_custom_family_without_allocating() {
    use mandara_config::desktop_widgets::{FontRole, Look};
    use mandara_ui::backdrop::paint::{FontNames, WidgetFonts};
    use mandara_ui::backdrop::widgets::draw::family;
    let fonts = FontNames::new("Interface", "Display", "Arabic", "Monospace");
    let look = Look { font:FontRole::Custom,font_family:"Personal font".into(),..Default::default() };
    let old = allocations(|| { for _ in 0..CONSTRUCTIONS { black_box(fonts.clone()); } });
    assert!(old.0 >= CONSTRUCTIONS);
    let current = allocations(|| { for _ in 0..CONSTRUCTIONS {
        let selected = fonts.for_look(Some(&look));
        assert_eq!(family(&selected,FontRole::Mono),"Personal font");
        black_box(selected);
    } });
    assert_eq!(current,(0,0));
    assert!(std::mem::size_of::<WidgetFonts<'_>>() < std::mem::size_of::<FontNames>());
    eprintln!("widget fonts over {CONSTRUCTIONS} paints: cloned={old:?}, borrowed={current:?}; owned={} bytes, selection={} bytes",std::mem::size_of::<FontNames>(),std::mem::size_of::<WidgetFonts<'_>>());
}

#[test]
fn unchanged_widget_traversal_allocates_only_its_returned_layer_order() {
    use mandara_config::desktop_widgets::{DesktopWidget,WidgetKind,Decoration,DecorShape,Insets};
    use mandara_core::shm_canvas::{Planes,Place,Rect};
    use mandara_ui::backdrop::{paint::FontNames,widgets::{self,WidgetScene,OutWidgets,OutputCx,Shared}};
    struct CachedPlanes { paints:usize }
    impl Planes for CachedPlanes {
        fn paint(&mut self,_:u32,_:usize,_:Place,_:Option<Rect>,_:bool,_:bool,_:impl FnOnce(&mut tiny_skia::PixmapMut<'_>))->anyhow::Result<bool> { self.paints += 1;Ok(true) }
        fn hide(&mut self,_:u32,_:usize) {}
        fn remove_plane(&mut self,_:u32,_:usize) {}
        fn has_plane(&self,_:u32,_:usize)->bool { false }
        fn frame_ready(&self,_:u32)->bool { true }
        fn move_plane(&mut self,_:u32,_:usize,_:Rect)->bool { true }
    }
    let widget = DesktopWidget { id:"disc".into(),kind:WidgetKind::Decoration(Decoration { shape:DecorShape::Disc,..Default::default() }),..Default::default() };
    let scene = WidgetScene { items:vec![widget],visible:vec!["DP-1".into()],..Default::default() };
    let fonts = FontNames::default();let names = ["DP-1"];
    let output = OutputCx { id:1,names:&names,logical:(1920.0,1080.0),physical:(1920,1080),scale120:120,tone:None,primary:mandara_theme::Color::hex("#0088ff"),fonts:&fonts,depth:false,subject:None,safe:Insets::default() };
    let mut planes = CachedPlanes { paints:0 };let mut state = OutWidgets::default();
    let mut text = mandara_ui::lock::paint::TextPainter::new();let shared = Shared::default();let now = chrono::Local::now();
    widgets::step_static(&mut planes,&mut state,&output,&scene,&shared,&mut text,now,0).unwrap();
    let paints = planes.paints;
    let former_traversal = allocations(|| { for _ in 0..CONSTRUCTIONS {
        let shown:Vec<_> = scene.items.iter().enumerate().filter(|(_,widget)|widget.enabled).collect();
        black_box(shown.iter().map(|(index,_)|*index).collect::<Vec<_>>());
        black_box(shown);
    } });
    let current = allocations(|| { for _ in 0..CONSTRUCTIONS {
        black_box(widgets::step_static(&mut planes,&mut state,&output,&scene,&shared,&mut text,now,0).unwrap());
    } });
    assert_eq!(planes.paints,paints,"unchanged widgets must not repaint");
    assert!(former_traversal.0 >= CONSTRUCTIONS * 2);
    assert!(current.0 <= CONSTRUCTIONS,"only the returned layer list may allocate: {current:?}");
    eprintln!("widget traversal over {CONSTRUCTIONS} cache hits: former temporary lists={former_traversal:?}, complete current pass={current:?}");
}
