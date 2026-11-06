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
