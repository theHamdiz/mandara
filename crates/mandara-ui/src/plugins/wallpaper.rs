//! Wallpaper plugin: a bar button that opens the picker, plus documented
//! commands to list, set, randomise and pick wallpapers.

use gpui::{IntoElement, ParentElement, Styled, div};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PluginInstance, PluginManifest, PluginRegistry, Zone};
use serde_json::json;

use super::fn_view;
use crate::icons::icon;
use crate::state::{config, shell};
use crate::wallpaper::{apply, directory, list};

pub const ID: &str = "wallpaper";

/// Bar glyph size.
const BAR_ICON_PX: f32 = 15.0;
/// Default `wallpaper render-preview` size in pixels.
const PREVIEW_SIZE: (u32, u32) = (2560, 1440);

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, mandara_i18n::tk("Wallpaper"), mandara_i18n::tk("Wallpaper picker and desktop background"),Some(Zone::Right)), |window, cx, _| {
        let bar = fn_view(None, |palette, _, _| div().flex().items_center().child(icon("image", crate::style::bar_icon(BAR_ICON_PX), palette.on_surface)).into_any_element(), window, cx);
        PluginInstance {
            bar: Some(bar),
            on_click: Some(std::rc::Rc::new(|_, cx| {
                let open = !shell(cx).read(cx).wallpaper_open;
                shell(cx).update(cx, |s, cx| s.set_wallpaper_picker(open, cx));
            })),
            ..PluginInstance::empty()
        }
    });
    r.command(
        ID,
        CommandSpec::new("wallpaper picker", mandara_i18n::tk("Open, close or toggle the wallpaper picker"), |a, cx| {
            if !config(cx).read(cx).get().wallpaper.picker {
                return Err("the wallpaper picker is switched off (Mandara Studio → Wallpaper)".into());
            }
            let open = match a.str(0)? {
                "open" => true,
                "close" => false,
                _ => !shell(cx).read(cx).wallpaper_open,
            };
            shell(cx).update(cx, |s, cx| s.set_wallpaper_picker(open, cx));
            Ok(json!({ "open": open }))
        })
        .arg(ArgSpec::required("action", ArgKind::Enum, mandara_i18n::tk("what to do")).choices(&["open", "close", "toggle"]))
        .example("wallpaper picker toggle"),
    );
    r.command(
        ID,
        CommandSpec::new("wallpaper list", mandara_i18n::tk("Images and videos in the wallpaper folder (optionally filtered)"), |a, cx| {
            let q = a.rest(0).to_lowercase();
            let items: Vec<String> = list(&directory(cx))
                .into_iter()
                .filter(|p| q.is_empty() || p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().contains(&q)))
                .map(|p| p.display().to_string())
                .collect();
            let videos = items.iter().filter(|p| mandara_media::is_video(std::path::Path::new(p))).count();
            Ok(json!({ "directory": directory(cx), "count": items.len(), "videos": videos, "items": items }))
        })
        .arg(ArgSpec::optional("filter", ArgKind::String, mandara_i18n::tk("substring of the file name"))),
    );
    r.command(
        ID,
        CommandSpec::new("wallpaper current", mandara_i18n::tk("The wallpaper Mandara is drawing"), |_, cx| {
            let w = &config(cx).read(cx).get().wallpaper;
            let video = w.path.as_deref().is_some_and(mandara_media::is_video);
            Ok(json!({ "path": w.path, "video": video, "drawing": w.enabled, "match_colors": w.match_colors }))
        }),
    );
    r.command(
        ID,
        CommandSpec::new("wallpaper set", mandara_i18n::tk("Set the wallpaper from an image or video path"), |a, cx| {
            let path = mandara_config::paths::expand_tilde(&a.rest(0));
            if !path.is_file() {
                return Err(format!("no such image: {}", path.display()));
            }
            apply(path.clone(), cx);
            Ok(json!({ "path": path }))
        })
        .arg(ArgSpec::required("path", ArgKind::String, mandara_i18n::tk("image, or AV1 video (.webm, .mkv, .ivf)")))
        .example("wallpaper set ~/Pictures/Wallpapers/dunes.jpg")
        .example("wallpaper set ~/Pictures/Wallpapers/tide.webm"),
    );
    r.command(
        ID,
        CommandSpec::new("wallpaper random", mandara_i18n::tk("Pick a random image or video from the wallpaper folder"), |_, cx| {
            let all = list(&directory(cx));
            if all.is_empty() {
                return Err(format!("no images in {}", directory(cx).display()));
            }
            let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos() as usize).unwrap_or(0);
            let path = all[n % all.len()].clone();
            apply(path.clone(), cx);
            Ok(json!({ "path": path }))
        }),
    );
    r.command(
        ID,
        CommandSpec::new_output("wallpaper depth-mask", mandara_i18n::tk("Show or regenerate the depth mask of the current wallpaper"), |a, cx| {
            let w = config(cx).read(cx).get().wallpaper.clone();
            let path = w.path.clone().ok_or("no wallpaper is set")?;
            let masks = crate::depth::masks(cx);
            match a.str(0)? {
                "regenerate" => {
                    if w.depth_mask == mandara_config::DepthMask::Off {
                        return Err("the depth mask is off (Mandara Studio → Wallpaper)".into());
                    }
                    let (tx, rx) = async_channel::bounded(1);
                    masks.update(cx, |m, cx| m.regenerate(&path, w.depth_mask, Some(tx), cx));
                    Ok(Output::Pending(rx))
                }
                _ => {
                    let status = masks.read(cx).known(&path, w.depth_mask);
                    Ok(Output::Value(match status {
                        Some(crate::depth::MaskStatus::Ready(r)) => json!({ "wallpaper": path, "mask": r.path, "source": r.source, "detail": r.detail }),
                        Some(crate::depth::MaskStatus::Unavailable(why)) => json!({ "wallpaper": path, "mask": null, "reason": why }),
                        Some(crate::depth::MaskStatus::Pending) => json!({ "wallpaper": path, "mask": null, "reason": "generating" }),
                        None => json!({ "wallpaper": path, "mask": null, "reason": if w.desktop_clock { "not requested yet" } else { "desktop clock is off" }, "custom_path": crate::depth::user_mask_path(&path) }),
                    }))
                }
            }
        })
        .arg(ArgSpec::required("action", ArgKind::Enum, mandara_i18n::tk("what to do")).choices(&["status", "regenerate"]))
        .example("wallpaper depth-mask regenerate"),
    );
    r.command(
        ID,
        CommandSpec::new_output("wallpaper render-preview", mandara_i18n::tk("Render the desktop canvas (wallpaper, clock, subject) to a PNG"), |a, cx| {
            let cfg = config(cx).read(cx).snapshot();
            let wallpaper = match a.get(2) {
                Some(p) => mandara_config::paths::expand_tilde(p),
                None => cfg.wallpaper.path.clone().ok_or("no wallpaper is set")?,
            };
            if !wallpaper.is_file() {
                return Err(format!("no such image: {}", wallpaper.display()));
            }
            let out = crate::desktop::preview_path(a.str(0)?);
            let size = match a.get(1) {
                Some(s) => crate::desktop::parse_size(s).ok_or("size must look like 2560x1440")?,
                None => PREVIEW_SIZE,
            };
            let now = chrono::Local::now().naive_local();
            let input = crate::desktop::PreviewInput {
                wallpaper,
                out,
                size,
                // `3840x2160@1.5`: physical size and output scale.
                scale: a.get(1).map(crate::desktop::parse_scale).unwrap_or(1.0),
                cfg: cfg.wallpaper.clone(),
                lines: crate::desktop::clock_lines(&now, cfg.wallpaper.clock_style, cfg.general.clock_24h),
                date: cfg.wallpaper.show_date.then(|| crate::desktop::date_line(&cfg, cfg.wallpaper.show_weather, cx)),
                fonts: crate::fonts::fonts(cx),
                primary: mandara_core::theme_entity(cx).read(cx).sample(std::time::Instant::now()).0.primary,
                widgets: crate::backdrop::widgets::WidgetScene {
                    items: cfg.desktop.widgets.painted().cloned().collect(),
                    on_surface: mandara_core::theme_entity(cx).read(cx).target().on_surface,
                    weather: crate::hub::hub(cx).read(cx).weather.weather.clone().map(|w| (crate::plugins::weather::condition_icon(w.condition, w.is_day), cfg.units().format_degrees(w.temp))),
                    units: cfg.units(),
                    h24: cfg.general.clock_24h,
                    rtl: cfg.general.locale.is_arabic(),
                    ..Default::default()
                },
            };
            let (tx, rx) = async_channel::bounded(1);
            std::thread::Builder::new()
                .name("mandara-preview".into())
                .spawn(move || {
                    let _ = tx.send_blocking(crate::desktop::render_preview(input).map(|v| v.to_string()));
                })
                .map_err(|e| e.to_string())?;
            Ok(Output::Pending(rx))
        })
        .arg(ArgSpec::required("path", ArgKind::String, mandara_i18n::tk("PNG file to write")))
        .arg(ArgSpec::optional("size", ArgKind::String, mandara_i18n::tk("WIDTHxHEIGHT in pixels (default 2560x1440)")))
        .arg(ArgSpec::optional("wallpaper", ArgKind::String, mandara_i18n::tk("another image to preview (default: the current wallpaper)")))
        .example("wallpaper render-preview /tmp/desktop.png 1920x1080"),
    );
}
