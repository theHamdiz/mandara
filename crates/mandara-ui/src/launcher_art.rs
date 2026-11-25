//! Colour launcher artwork, embedded as SVG and decoded only when drawn.

use std::path::Path;

/// A shipped alternative to the phoenix launcher button.
pub struct LauncherIcon {
    /// Stable configuration value, separate from system icon-theme names.
    pub spec: &'static str,
    /// Descriptive file name shown in the chooser.
    pub name: &'static str,
    /// Embedded resource key understood by the sized icon decoder.
    pub path: &'static str,
    bytes: &'static [u8],
}

macro_rules! icon {
    ($name:literal) => {
        LauncherIcon {
            spec: concat!("mandara:", $name),
            name: $name,
            path: concat!("bundled-launcher/", $name, ".svg"),
            bytes: include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/launcher-icons/", $name, ".svg")),
        }
    };
}

/// The chooser's alternatives; reading this table does not decode any image.
pub const ICONS: &[LauncherIcon] = &[
    icon!("amethyst-medallion"),
    icon!("violet-woven-diamond"),
    icon!("violet-clover"),
    icon!("golden-sunwheel"),
    icon!("scarlet-quatrefoil"),
    icon!("leaf-pinwheel"),
    icon!("teal-hourglass"),
    icon!("coral-knot"),
    icon!("blue-whorl"),
    icon!("golden-rosette"),
    icon!("pink-rosette"),
    icon!("turquoise-weave"),
    icon!("lilac-lattice"),
];

/// Sized resource key for the existing phoenix artwork.
pub const PHOENIX_PATH: &str = "bundled-launcher/phoenix.png";

/// Resolves a configuration value without allocating or consulting the disk.
pub fn find(spec: &str) -> Option<&'static LauncherIcon> {
    ICONS.iter().find(|icon| icon.spec == spec)
}

/// Bytes for a sized embedded resource; ordinary paths keep the file loader.
pub(crate) fn asset(path: &Path) -> Option<&'static [u8]> {
    let path = path.to_str()?;
    if path == PHOENIX_PATH {
        return Some(include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/brand/mandara-256.png")));
    }
    ICONS.iter().find(|icon| icon.path == path).map(|icon| icon.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alternatives_are_distinct_self_contained_drawings() {
        for (i, icon) in ICONS.iter().enumerate() {
            assert!(!icon.name.to_ascii_lowercase().contains("pik"));
            assert_eq!(find(icon.spec).unwrap().path, icon.path);
            assert_eq!(asset(Path::new(icon.path)), Some(icon.bytes));
            for previous in &ICONS[..i] {
                assert_ne!(icon.spec, previous.spec);
                assert_ne!(icon.bytes, previous.bytes, "duplicate artwork: {}", icon.name);
            }
            let tree = usvg::Tree::from_data(icon.bytes, &usvg::Options::default()).unwrap();
            assert!(tree.root().has_children(), "{} is empty", icon.name);
            assert!(!std::str::from_utf8(icon.bytes).unwrap().contains("href="), "{} references another file", icon.name);
        }
        assert!(find("mandara:missing").is_none());
        assert!(asset(Path::new("/tmp/phoenix.png")).is_none());
    }
}
