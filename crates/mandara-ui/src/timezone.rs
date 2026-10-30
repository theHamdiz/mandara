//! The system clock's time zone and network time (`time …` commands),
//! through systemd-timedated ([`mandara_services::timedate`]); changes ask
//! for permission through polkit. Studio → Date, time & location shows the
//! same, and warns when the clock's time zone doesn't match the shared
//! location's ([`location_timezone`]).

use gpui::App;
use mandara_i18n::{t, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PluginRegistry};
use mandara_services::timedate;
use serde_json::{Value, json};

use crate::state::config;

/// Owner id of the time commands.
const OWNER: &str = "core";
/// Pending-reply channel size.
const REPLY_QUEUE: usize = 1;

/// The shared location's time zone: the forecast's when the weather is for
/// that place (computed from its coordinates), else the one the last place
/// search or IP lookup found. Empty when unknown.
pub fn location_timezone(cx: &App) -> String {
    let cfg = config(cx).read(cx).get();
    let forecast_here = crate::plugins::weather::own_location(cfg, crate::plugins::weather::ID).is_none() && crate::plugins::weather::own_location(cfg, crate::plugins::prayer::ID).is_none();
    let forecast = crate::hub::try_hub(cx).and_then(|h| h.read(cx).weather.weather.as_ref().map(|w| w.timezone.clone())).filter(|z| forecast_here && timedate::is_zone(z));
    forecast.unwrap_or_else(|| cfg.location.timezone.clone())
}

/// Runs `job` on the service runtime and replies with its JSON.
fn pending(job: impl std::future::Future<Output = Result<Value, String>> + Send + 'static) -> Output {
    let (tx, rx) = async_channel::bounded(REPLY_QUEUE);
    mandara_services::runtime::spawn("timedate", async move {
        let _ = tx.send(job.await.map(|v| v.to_string())).await;
    });
    Output::Pending(rx)
}

pub fn register(r: &mut PluginRegistry) {
    r.command(
        OWNER,
        CommandSpec::new_output("time", tk("The clock's time zone, network time, and the shared location's time zone"), |_, cx| {
            let location = location_timezone(cx);
            Ok(pending(async move {
                let s = timedate::status().await?;
                let now = chrono::Utc::now();
                let offset = timedate::offset_seconds(&s.timezone, now).map(timedate::offset_label);
                let mismatch = timedate::mismatch(&s.timezone, &location, now).map(str::to_string);
                Ok(json!({ "timezone": s.timezone, "offset": offset, "ntp": s.ntp, "can_ntp": s.can_ntp, "synchronized": s.synced, "location_timezone": location, "suggested_timezone": mismatch }))
            }))
        })
        .example("time"),
    );
    r.command(
        OWNER,
        CommandSpec::new_output("time zone", tk("Set the system time zone (asks for your password)"), |a, _| {
            let zone = a.str(0)?.to_string();
            if !timedate::is_zone(&zone) {
                return Err(mandara_i18n::tf("“{zone}” isn't a time zone", &[("zone", &zone)]));
            }
            Ok(pending(async move {
                timedate::set_timezone(&zone).await?;
                Ok(json!({ "timezone": zone }))
            }))
        })
        .arg(ArgSpec::required("zone", ArgKind::String, tk("IANA name, e.g. Africa/Cairo")))
        .example("time zone Africa/Cairo"),
    );
    r.command(
        OWNER,
        CommandSpec::new_output("time zones", tk("List time zones, optionally only those containing a word"), |a, _| {
            let filter = a.rest(0).to_lowercase();
            Ok(pending(async move {
                let zones = timedate::zones().await?;
                Ok(Value::from(zones.into_iter().filter(|z| filter.is_empty() || z.to_lowercase().contains(&filter)).collect::<Vec<_>>()))
            }))
        })
        .arg(ArgSpec::optional("filter", ArgKind::String, tk("part of a name, e.g. cairo")))
        .example("time zones europe"),
    );
    r.command(
        OWNER,
        CommandSpec::new_output("time auto", tk("Set the clock from internet time servers (asks for your password)"), |a, _| {
            let state = a.str(0)?.to_string();
            Ok(pending(async move {
                let on = match state.as_str() {
                    "toggle" => !timedate::status().await?.ntp,
                    other => matches!(other, "on" | "true" | "1"),
                };
                timedate::set_ntp(on).await.map_err(|e| if e.contains("NoNTPSupport") { t("No network time service is installed (for example systemd-timesyncd)").to_string() } else { e })?;
                Ok(json!({ "ntp": on }))
            }))
        })
        .arg(ArgSpec::required("state", ArgKind::Enum, tk("on / off / toggle")).choices(&["on", "off", "toggle"]))
        .example("time auto on"),
    );
}
