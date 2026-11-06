use std::hint::black_box;
use std::mem::{align_of, size_of};
use std::time::{Duration, Instant};

use super::*;
use crate::{ThemeEngine, ThemeSource, catalog};

#[path = "../../../mandara-core/src/input_mask/tests/allocations.rs"]
mod allocations;

// Keep the previous representation for wire and layout comparisons. The token
// macro ensures new palette roles are included in both representations.
macro_rules! legacy_palette {
    ($($field:ident),*) => {
        #[derive(Clone, Serialize, Deserialize)]
        struct LegacyPalette {
            name: String,
            is_dark: bool,
            $($field: Color,)*
        }

        impl From<&MandaraPalette> for LegacyPalette {
            fn from(palette: &MandaraPalette) -> Self {
                Self { name: palette.name.to_string(), is_dark: palette.is_dark, $($field: palette.$field,)* }
            }
        }
    };
}
palette_tokens!(legacy_palette);

fn palette(id: &str) -> MandaraPalette {
    catalog::builtin().iter().find(|theme| theme.id == id).unwrap().palette(true)
}

#[test]
fn shared_names_keep_the_existing_json_and_toml_wire_format() {
    let mut palette = palette(catalog::DEFAULT_THEME);
    for name in ["Mandara Corniche", "ألواني الخاصة", "", "A custom name with a long authored description"] {
        palette.name = name.into();
        let previous = LegacyPalette::from(&palette);
        let json = serde_json::to_string(&previous).unwrap();
        assert_eq!(serde_json::to_string(&palette).unwrap(), json);
        assert_eq!(serde_json::from_str::<MandaraPalette>(&json).unwrap(), palette);
        let toml = toml::to_string(&previous).unwrap();
        assert_eq!(toml::to_string(&palette).unwrap(), toml);
        assert_eq!(toml::from_str::<MandaraPalette>(&toml).unwrap(), palette);
    }
}

#[test]
fn clones_and_transition_samples_share_their_source_names() {
    let left = palette(catalog::DEFAULT_THEME);
    let right = palette("montaza-twilight");
    assert!(Arc::ptr_eq(&left.name, &left.clone().name));
    for (time, source) in [(0.0, &left), (0.49, &left), (0.5, &right), (1.0, &right)] {
        let blended = left.lerp(&right, time);
        assert!(Arc::ptr_eq(&blended.name, &source.name));
        assert_eq!(blended.is_dark, source.is_dark);
    }
    let adjusted = left.adjusted(&PaletteAdjustments { hue_shift: 25.0, ..Default::default() });
    assert!(Arc::ptr_eq(&left.name, &adjusted.name));
}

#[test]
fn warmed_palette_copies_and_crossfades_do_not_allocate() {
    let left = palette(catalog::DEFAULT_THEME);
    let right = palette("montaza-twilight");
    let previous = LegacyPalette::from(&left);
    let (_, previous_count) = allocations::measure(|| {
        for _ in 0..64 {
            black_box(previous.clone());
        }
    });
    let start = Instant::now();
    let mut engine = ThemeEngine::new(ThemeSource::default(), true, PaletteAdjustments::default());
    engine.apply_resolved(
        ThemeSource::Preset { preset: "montaza-twilight".into() },
        true,
        crate::engine::ResolvedTheme { palette: right.clone(), seeds: Vec::new() },
        start,
    );
    let (_, count) = allocations::measure(|| {
        for index in 0..64 {
            black_box(left.clone());
            black_box(left.lerp(&right, index as f32 / 63.0));
            black_box(engine.sample(start + Duration::from_millis(index)));
            black_box(engine.sample(start + Duration::from_secs(1)));
        }
    });
    eprintln!("palette work: 64 legacy copies {previous_count:?}; 64 shared copies/interpolations/active samples/settled samples {count:?}");
    assert_eq!(count, allocations::Count::default(), "palette work allocated: {count:?}");
}

#[test]
fn palette_layout_is_recorded_against_the_previous_field_types() {
    eprintln!(
        "palette layout: previous={} align={}, shared={} align={}; String={}, Arc<str>={}",
        size_of::<LegacyPalette>(), align_of::<LegacyPalette>(), size_of::<MandaraPalette>(), align_of::<MandaraPalette>(), size_of::<String>(), size_of::<Arc<str>>(),
    );
    assert!(size_of::<MandaraPalette>() <= size_of::<LegacyPalette>(), "shared ownership must not inflate every palette");
    assert_eq!(align_of::<MandaraPalette>(), align_of::<LegacyPalette>());
}

#[test]
fn retained_snapshots_keep_names_alive_without_mutating_authored_metadata() {
    let theme = &catalog::builtin()[0];
    let authored = theme.name.clone();
    let snapshot = {
        let palette = theme.palette(true);
        palette.clone()
    };
    assert_eq!(snapshot.name.as_ref(), authored);
    assert_eq!(theme.name, authored);
    let mut renamed = snapshot.clone();
    renamed.name = "Personal palette".into();
    assert_eq!(snapshot.name.as_ref(), authored);
    assert!(!Arc::ptr_eq(&renamed.name, &snapshot.name));
}
