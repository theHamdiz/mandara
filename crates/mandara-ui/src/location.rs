//! The shared location (`[location]`): explicit locate / search / set
//! commands. Network lookups happen only when asked for (a Studio button or
//! `mandara msg location …`); GeoClue mode also refreshes once at startup
//! (GeoClue enforces its own permission prompt).

use gpui::App;
use mandara_config::desktop::LocationMode;
use mandara_i18n::tk;
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PluginRegistry};
use mandara_services::location::{self, Place};
use serde_json::{Value, json};

use crate::state::{config, update_config};

/// Owner id of the location commands.
const OWNER: &str = "core";
/// Pending-reply channel size.
const REPLY_QUEUE: usize = 1;

/// Which backend a locate request uses.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Backend {
    Geoclue,
    Ip,
}

fn place_json(p: &Place) -> Value {
    json!({ "latitude": p.latitude, "longitude": p.longitude, "name": p.name, "detail": p.detail })
}

/// Stores `place` as the shared location (coarsened to ~1 km) and turns
/// sharing on.
fn store(place: &Place, cx: &mut App) {
    let place = place.clone();
    update_config(cx, move |c| {
        c.location.latitude = location::coarse(place.latitude);
        c.location.longitude = location::coarse(place.longitude);
        if !place.name.is_empty() {
            c.location.name = place.name.clone();
        }
        c.location.shared = true;
    });
}

/// Runs `job` off the UI thread, stores its place, and replies with it.
fn run(job: impl FnOnce() -> Result<Place, String> + Send + 'static, cx: &mut App) -> Output {
    let (tx, rx) = async_channel::bounded(REPLY_QUEUE);
    let task = cx.background_executor().spawn(async move { job() });
    cx.spawn(async move |cx| {
        let result = task.await;
        if let Ok(place) = &result {
            let place = place.clone();
            cx.update(|cx| store(&place, cx));
        }
        let _ = tx.send(result.map(|p| place_json(&p).to_string())).await;
    })
    .detach();
    Output::Pending(rx)
}

fn locate(backend: Backend, cx: &mut App) -> Output {
    match backend {
        Backend::Ip => run(location::locate_by_ip, cx),
        Backend::Geoclue => run(|| mandara_services::runtime::block_on(location::locate_geoclue()), cx),
    }
}

/// Refreshes a GeoClue-mode shared location once at startup.
pub fn init(cx: &mut App) {
    let cfg = config(cx).read(cx).snapshot();
    if cfg.location.shared && cfg.location.mode == LocationMode::Geoclue {
        let _ = locate(Backend::Geoclue, cx);
    }
}

pub fn register(r: &mut PluginRegistry) {
    r.command(
        OWNER,
        CommandSpec::new("location", tk("The shared location used by weather and prayer times"), |_, cx| {
            let l = config(cx).read(cx).get().location.clone();
            Ok(json!({ "shared": l.shared, "mode": l.mode, "latitude": l.latitude, "longitude": l.longitude, "name": l.name }))
        })
        .example("location"),
    );
    r.command(
        OWNER,
        CommandSpec::new_output("location locate", tk("Find your location now (GeoClue, or one IP lookup) and share it with weather and prayer times"), |a, cx| {
            let backend = match a.get(0) {
                Some("ip") => Backend::Ip,
                Some("geoclue") => Backend::Geoclue,
                _ => match config(cx).read(cx).get().location.mode {
                    LocationMode::Ip => Backend::Ip,
                    // Manual mode still uses the privacy-respecting service.
                    LocationMode::Geoclue | LocationMode::Manual => Backend::Geoclue,
                },
            };
            Ok(locate(backend, cx))
        })
        .arg(ArgSpec::optional("method", ArgKind::Enum, tk("geoclue or ip (default: the configured mode)")).choices(&["geoclue", "ip"]))
        .example("location locate")
        .example("location locate ip"),
    );
    r.command(
        OWNER,
        CommandSpec::new_output("location search", tk("Search places by name (Open-Meteo geocoding)"), |a, cx| {
            let query = a.rest(0);
            let lang = mandara_i18n::language().code().to_string();
            let (tx, rx) = async_channel::bounded(REPLY_QUEUE);
            let task = cx.background_executor().spawn(async move { location::search(&query, &lang) });
            cx.spawn(async move |_| {
                let result = task.await.map(|places| Value::Array(places.iter().map(place_json).collect()).to_string());
                let _ = tx.send(result).await;
            })
            .detach();
            Ok(Output::Pending(rx))
        })
        .arg(ArgSpec::required("query", ArgKind::String, tk("City or place name")))
        .example("location search Alexandria"),
    );
    r.command(
        OWNER,
        CommandSpec::new_output("location geocode", tk("Look up the coordinates of the configured place name and use the best match"), |_, cx| {
            let name = config(cx).read(cx).get().location.name.clone();
            let lang = mandara_i18n::language().code().to_string();
            Ok(run(
                move || {
                    let first = location::search(&name, &lang)?.into_iter().next().ok_or_else(|| mandara_i18n::tf("No place found for “{name}”", &[("name", &name)]))?;
                    // Keep the name as typed.
                    Ok(Place { name: name.clone(), ..first })
                },
                cx,
            ))
        })
        .example("location geocode"),
    );
    r.command(
        OWNER,
        CommandSpec::new("location set", tk("Set the shared location by coordinates"), |a, cx| {
            let (lat, lon) = (a.number(0)?, a.number(1)?);
            if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
                return Err(mandara_i18n::t("Latitude must be within ±90° and longitude within ±180°").into());
            }
            let name = a.rest(2);
            store(&Place { latitude: lat, longitude: lon, name, detail: String::new() }, cx);
            Ok(json!({ "latitude": location::coarse(lat), "longitude": location::coarse(lon) }))
        })
        .arg(ArgSpec::required("latitude", ArgKind::Number, tk("Degrees north")))
        .arg(ArgSpec::required("longitude", ArgKind::Number, tk("Degrees east")))
        .arg(ArgSpec::optional("name", ArgKind::String, tk("Place name")))
        .example("location set 31.28 30.02 El-Mandara"),
    );
}
