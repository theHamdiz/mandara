//! Keep native GPU builds free of browser-only shader translation.

use std::collections::BTreeSet;

fn strings(value: &toml::Value) -> BTreeSet<&str> {
    value.as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect()
}

#[test]
fn native_renderer_selects_vulkan_and_gl_without_browser_backends() {
    let workspace: toml::Value = toml::from_str(include_str!("../../../Cargo.toml")).unwrap();
    let renderer: toml::Value = toml::from_str(include_str!("../../../vendor/gpui_wgpu/Cargo.toml")).unwrap();
    let dependency = &workspace["workspace"]["dependencies"]["gpui_wgpu"];
    assert_eq!(dependency["default-features"].as_bool(), Some(false));
    assert_eq!(strings(&dependency["features"]), BTreeSet::from(["vulkan", "gles"]));
    assert_eq!(strings(&renderer["features"]["vulkan"]), BTreeSet::from(["wgpu/vulkan"]));
    assert_eq!(strings(&renderer["features"]["gles"]), BTreeSet::from(["wgpu/gles"]));
    let native = &renderer["target"]["cfg(any(target_os = \"linux\", target_os = \"freebsd\"))"]["dependencies"]["wgpu"];
    assert_eq!(native["default-features"].as_bool(), Some(false));
    assert_eq!(strings(&native["features"]), BTreeSet::from(["std", "wgsl"]));

    let browser = &renderer["target"]["cfg(target_family = \"wasm\")"]["dependencies"]["wgpu"];
    let browser_features = strings(&browser["features"]);
    assert!(browser_features.contains("webgpu") && browser_features.contains("webgl"));
}
