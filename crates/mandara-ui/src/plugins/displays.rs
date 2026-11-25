//! `mandara msg display …`: list outputs, change one property with an
//! automatic revert unless kept, saved profiles (applied on hotplug when
//! enabled), persistence to the compositor drop-ins, and external-monitor
//! brightness over DDC/CI.

use gpui::{App, Global};
use mandara_config::outputs::{DisplayProfile, OutputConfig, Transform, VrrMode};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PluginRegistry};
use mandara_services::{ddc, outputs};
use serde_json::json;

use crate::state::{config, update_config};

const OWNER: &str = "displays";
/// Properties `display set` changes.
const PROPERTIES: &[&str] = &["mode", "scale", "transform", "position", "vrr", "enabled"];
/// Hertz per millihertz.
const MHZ_PER_HZ: f64 = 1000.0;

/// Parses `WIDTHxHEIGHT[@HZ]`.
fn parse_mode(s: &str) -> Option<(u32, u32, Option<u32>)> {
    let (size, hz) = match s.split_once('@') {
        Some((size, hz)) => (size, Some(hz.trim_end_matches("Hz").trim_end_matches("hz").parse::<f64>().ok()
                    .filter(|rate| {
                        rate.is_finite() && *rate > 0.0 && *rate <= i32::MAX as f64 / MHZ_PER_HZ
                    })?)),
        None => (s, None),
    };
    let (w, h) = size.split_once('x')?;
    let width = w.trim().parse::<u32>().ok()
        .filter(|value| *value > 0 && *value <= i32::MAX as u32)?;
    let height = h.trim().parse::<u32>().ok()
        .filter(|value| *value > 0 && *value <= i32::MAX as u32)?;
    Some((width, height, hz.map(|hz| (hz * MHZ_PER_HZ).round() as u32)))
}

fn parse_bool(s: &str) -> Option<bool> {
    match s {
        "on" | "true" | "1" | "yes" => Some(true),
        "off" | "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

/// Applies one property to `base` (the head's present state).
pub fn set_property(base: &mut OutputConfig, property: &str, value: &str) -> Result<(), String> {
    let bad = || {
        tf("Invalid value {value} for {property}", &[("value", &value), ("property", &property)])
    };
    match property {
        "mode" => {
            let (w, h, r) = parse_mode(value).ok_or_else(bad)?;
            (base.width, base.height, base.refresh_mhz) = (Some(w), Some(h), r);
        }
        "scale" => {
            base.scale = Some(value.parse::<f32>().ok().filter(|s| {
                        s.is_finite()
                            && (mandara_config::outputs::MIN_SCALE
                                ..=mandara_config::outputs::MAX_SCALE)
                                .contains(s)
                    }).ok_or_else(bad)?)
        }
        "transform" => base.transform = Some(Transform::parse(value).ok_or_else(bad)?),
        "position" => {
            let (x, y) = value.split_once([',', 'x']).ok_or_else(bad)?;
            (base.x, base.y) = (Some(x.trim().parse().map_err(|_| bad())?), Some(y.trim().parse().map_err(|_| bad())?));
        }
        "vrr" => {
            base.vrr = Some(match value {
                "fullscreen" => VrrMode::Fullscreen,
                v => {
                    if parse_bool(v).ok_or_else(bad)? {
                        VrrMode::On
                    } else {
                        VrrMode::Off
                    }
                }
            })
        }
        "enabled" => base.enabled = parse_bool(value).ok_or_else(bad)?,
        _ => return Err(bad()),
    }
    Ok(())
}

/// Queues one hardware operation and returns its JSON reply asynchronously.
fn pending(f: impl FnOnce() -> Result<serde_json::Value, String> + Send + 'static) -> Output {
    let (tx, rx) = async_channel::bounded(1);
    mandara_services::runtime::spawn("display-command", async move {
        let _ = tx.send(outputs::jobs::run(f).await.map(|v| v.to_string()))
            .await;
    });
    Output::Pending(rx)
}

fn head_json(h: &outputs::Head) -> serde_json::Value {
    let (lw, lh) = h.logical_size();
    json!({
        "name": h.name,
        "description": h.description,
        "make": h.make,
        "model": h.model,
        "serial": h.serial,
        "enabled": h.enabled,
        "mode": h.current().map(|m| format!("{}x{}@{:.3}", m.width, m.height, m.refresh_mhz as f64 / MHZ_PER_HZ)),
        "modes": h.modes.iter().filter(|m| m.width > 0).map(|m| format!("{}x{}@{:.3}", m.width, m.height, m.refresh_mhz as f64 / MHZ_PER_HZ)).collect::<Vec<_>>(),
        "position": [h.x, h.y],
        "logical_size": [lw.round(), lh.round()],
        "scale": h.scale,
        "transform": h.transform.name(),
        "adaptive_sync": h.adaptive_sync,
        "physical_mm": [h.physical_mm.0, h.physical_mm.1],
    })
}

/// Saves the current arrangement as a profile named `name` (replacing one
/// with that name).
pub fn save_profile(name: &str, heads: &[outputs::Head], cx: &mut App) {
    let profile = DisplayProfile::new(name.to_string(), heads.iter().map(outputs::Head::to_config).collect());
    update_config(cx, move |c| {
        c.displays.profiles.retain(|p| p.name != profile.name);
        c.displays.profiles.push(profile);
    });
    ensure_hotplug(cx);
}

/// Writes the current arrangement to the compositor drop-in (survives
/// restarts; the compositor sync regenerates the file).
pub fn persist(heads: &[outputs::Head], cx: &mut App) -> Option<async_channel::Receiver<Result<(), String>>> {
    update_config(cx, |c| {
        mandara_config::outputs::remember(
            &mut c.compositor.outputs,
            heads.iter().map(outputs::Head::to_config),
        )
    });
    queue_output_preferences(cx, true)
}

/// Keep the tested arrangement and save the live state, including changes
/// made through profiles. Never persist an unapplied Studio draft.
pub fn keep(cx: &mut App) -> gpui::Task<Result<bool, String>> {
    cx.spawn(async move |cx| {
        let Some(heads) = outputs::jobs::run(outputs::confirm_and_snapshot).await? else {
            return Ok(false);
        };
        if let Some(saved) = cx.update(|cx| persist(&heads, cx)) {
            saved.recv().await.map_err(|error| error.to_string())??;
        }
        Ok(true)
    })
}

// ── Brightness keys on external monitors ────────────────────────────────────

/// Last brightness (`0..=1`) set or read over DDC/CI.
#[derive(Default)]
struct DdcLevel(Option<f32>);
impl Global for DdcLevel {}

/// Percent per whole.
const PERCENT: f32 = 100.0;

/// The lowest level brightness keys may reach (`displays.brightness.min_percent`).
pub fn min_level(cx: &App) -> f32 {
    config(cx).read(cx).get().displays.brightness.floor_percent() as f32 / PERCENT
}

/// The last known external-monitor brightness (desktops without a backlight).
pub fn ddc_level(cx: &App) -> Option<f32> {
    cx.try_global::<DdcLevel>().and_then(|l| l.0)
}

/// Sets every DDC/CI monitor to `level` when the brightness keys should
/// drive them (`sync_all`, or a machine without a backlight). `None` when
/// they shouldn't.
pub fn sync_external(level: f32, has_backlight: bool, cx: &mut App) -> Option<Output> {
    let b = config(cx).read(cx).get().displays.brightness.clone();
    if !b.ddc || (has_backlight && !b.sync_all) {
        return None;
    }
    cx.default_global::<DdcLevel>().0 = Some(level);
    let percent = (level * PERCENT) as f64;
    Some(pending(move || {
        if ddc::access() != ddc::Access::Ready {
            return Err(t("No access to /dev/i2c-* (Studio → Displays explains how)").into());
        }
        let mut set = serde_json::Map::new();
        for m in ddc::monitors() {
            set.insert(m.connector.clone(), json!(ddc::set_percent(&m.connector, percent, b.floor_percent())?));
        }
        Ok(json!({ "brightness": set }))
    }))
}

/// Reads the external monitors' brightness once in the background so the
/// first brightness key on a desktop starts from the real level.
fn prime_ddc_level(cx: &mut App) {
    let cfg = config(cx).read(cx).snapshot();
    if !cfg.displays.brightness.ddc || !mandara_services::backlight::devices().is_empty() {
        return;
    }
    cx.spawn(async move |cx| {
        let level = mandara_services::runtime::unblock(|| { (ddc::access() == ddc::Access::Ready).then(ddc::monitors).and_then(|m| m.first().map(|m| m.percent() as f32 / PERCENT)) })
            .await;
        cx.update(|cx| cx.default_global::<DdcLevel>().0 = level);
    })
    .detach();
}

// ── Profile auto-apply on hotplug ───────────────────────────────────────────

#[derive(Default)]
struct Hotplug(Option<outputs::Watch>);
impl Global for Hotplug {}

/// Starts (or stops) the hotplug watcher to match `displays.auto_profiles`
/// and whether any profile exists.
pub fn ensure_hotplug(cx: &mut App) {
    let cfg = config(cx).read(cx).snapshot();
    let want = cfg.displays.auto_profiles && !cfg.displays.profiles.is_empty();
    let running = cx.try_global::<Hotplug>().is_some_and(|h| h.0.as_ref().is_some_and(outputs::Watch::is_alive));
    if want == running {
        return;
    }
    if !want {
        drop(cx.default_global::<Hotplug>().0.take());
        return;
    }
    let (tx, rx) = async_channel::bounded::<Vec<outputs::Head>>(1);
    let watch = match outputs::watch(move |heads| {
        let _ = tx.force_send(heads);
    }) {
        Ok(watch) => watch,
        Err(error) => {
            tracing::warn!("display hotplug watcher: {error}");
            return;
        }
    };
    let stop = watch.stopped();
    cx.default_global::<Hotplug>().0 = Some(watch);
    cx.spawn(async move |cx| {
        while let Ok(mut heads) = rx.recv().await {
            while let Ok(newer) = rx.try_recv() {
                heads = newer;
            }
            let profile = cx.update(|cx| {
                let cfg = config(cx).read(cx).snapshot();
                cfg.displays.auto_profiles.then(|| {
                        cfg.displays.matching_profile(&outputs::identities(&heads)).cloned()
                    }).flatten()
            });
            if let Some(profile) = profile {
                tracing::info!("displays: applying profile {:?}", profile.name);
                let cancellation = stop.clone();
                let expected = outputs::current_revision();
                let preferences = profile.outputs.clone();
                let result = outputs::jobs::run(move || {
                    outputs::apply_if_active(&profile.outputs, expected, &cancellation)
                })
                .await;
                match result {
                    Ok(Some(applied))
                        if !stop.load(std::sync::atomic::Ordering::Acquire)
                            && outputs::current_revision() == applied.revision =>
                    {
                        cx.update(|cx| {
                            let mut saved = preferences;
                            mandara_config::outputs::remember(
                                &mut saved,
                                applied.heads.iter().map(outputs::Head::to_config),
                            );
                            update_config(cx, |config| {
                                mandara_config::outputs::remember(
                                    &mut config.compositor.outputs,
                                    saved,
                                )
                            });
                        });
                    }
                    Ok(_) => {}
                    Err(error) => tracing::warn!("applying automatic display profile: {error}"),
                }
            }
        }
    })
    .detach();
}

/// Starts the hotplug watcher when needed and follows config changes.
pub fn init(cx: &mut App) {
    ensure_hotplug(cx);
    persist_output_preferences(cx);
    prime_ddc_level(cx);
    let cfg = config(cx).read(cx).snapshot();
    let output_revision = outputs::current_revision();
    // The hotplug watcher handles an enabled profile, including its first snapshot.
    if !cfg.compositor.outputs.is_empty() {
        cx.spawn(async move |_| {
            mandara_services::runtime::unblock(move || {
                if outputs::supported()
                    && let Err(err) = outputs::restore(
                        &cfg.compositor.outputs,
                        cfg.displays.auto_profiles.then_some(&cfg.displays),
                        output_revision,
                    )
                {
                    tracing::warn!("displays: restoring the saved arrangement: {err}");
                }
            })
            .await;
        })
        .detach();
    }
    let store = config(cx);
    // Turning "Control external monitors" on live reads the monitors'
    // level again, so the next brightness key starts from it (it was only
    // read at startup, leaving desktops with "no backlight" until a restart).
    let mut ddc_on = store.read(cx).get().displays.brightness.ddc;
    cx.observe(&store, move |store, cx| {
        ensure_hotplug(cx);
        persist_output_preferences(cx);
        let now = store.read(cx).get().displays.brightness.ddc;
        if now && !ddc_on {
            prime_ddc_level(cx);
        }
        ddc_on = now;
    })
    .detach();
}

/// One active writer and one newest snapshot; no resident blocking thread.
struct OutputPreferences(mandara_services::compositor_config::persistence::Writer);
impl Global for OutputPreferences {}

fn persist_output_preferences(cx: &mut App) {
    queue_output_preferences(cx, false);
}

fn queue_output_preferences(cx: &mut App, wait: bool) -> Option<async_channel::Receiver<Result<(), String>>> {
    use mandara_config::compositor_theme;
    let backend = crate::hub::hub(cx).read(cx).compositor.clone()?;
    let target = backend.target();
    if !compositor_theme::supported(target) { return None; }
    if cx.try_global::<OutputPreferences>().is_none() {
        cx.set_global(OutputPreferences(mandara_services::compositor_config::persistence::Writer::new(target)));
    }
    cx.global::<OutputPreferences>().0.submit(&config(cx).read(cx).get().compositor.outputs, wait)
}

pub fn register(r: &mut PluginRegistry) {
    r.command(
        OWNER,
        CommandSpec::new_output("display list", tk("Connected displays with modes, position, scale and rotation (wlr-output-management)"), |_, _| {
            Ok(pending(|| {
            let heads = outputs::heads()?;
            Ok(json!({ "outputs": heads.iter().map(head_json).collect::<Vec<_>>(), "heads":heads, "revert_pending_secs": outputs::revert_remaining().map(|d| d.as_secs()) }))
        }))
        }),
    );
    r.command(OWNER,CommandSpec::new_output("display test",tk("Test a display arrangement from JSON, keeping unrelated changes made since the draft"),|arguments,cx| {
        let request: outputs::TestRequest = serde_json::from_str(&arguments.rest(0)).map_err(|error|error.to_string())?;
        let seconds = config(cx).read(cx).get().displays.revert_secs();
        Ok(pending(move || {
            let seconds = match request.before {
                Some(before) => outputs::apply_edits_with_revert(&before,&request.outputs,seconds)?,
                None => outputs::apply_with_revert(&request.outputs,seconds)?,
            };
            Ok(json!({"reverts_in_secs":seconds}))
        }))
    }).arg(ArgSpec::required("json",ArgKind::String,tk("display draft as JSON"))));
    r.command(
        OWNER,
        CommandSpec::new_output("display set", tk("Change one display property; it reverts after the countdown unless `display confirm` keeps it"), |a, cx| {
            let (name, property, value) = (a.str(0)?.to_string(), a.str(1)?.to_string(), a.str(2)?.to_string());
            let secs = config(cx).read(cx).get().displays.revert_secs();
            Ok(pending(move || {
                let heads = outputs::heads()?;
                let head = heads.iter().find(|h| h.name == name).ok_or_else(|| tf("No display named {name}", &[("name", &name)]))?;
                let mut change = head.to_config();
                set_property(&mut change, &property, &value)?;
                let secs = outputs::apply_with_revert(&[change], secs)?;
                Ok(json!({ "applied": true, "output": name, "property": property, "value": value, "reverts_in_secs": secs, "keep": "mandara msg display confirm" }))
            }))
        })
        .arg(ArgSpec::required("output", ArgKind::String, tk("connector, e.g. DP-1")))
        .arg(ArgSpec::required("property", ArgKind::Enum, tk("what to change")).choices(PROPERTIES))
        .arg(ArgSpec::required("value", ArgKind::String, tk("e.g. 2560x1440@144, 1.5, 90, 1920,0, on")))
        .example("display set DP-1 mode 2560x1440@144"),
    );
    r.command(OWNER, CommandSpec::new_output("display confirm", tk("Keep the display change being tested"), |_, cx| {
                let task = keep(cx);
                let (tx, rx) = async_channel::bounded(1);
                cx.spawn(async move |_| {
                    let result = task.await.map(|kept| json!({ "kept": kept }).to_string());
                    let _ = tx.send(result).await;
                })
                .detach();
                Ok(Output::Pending(rx)) }),
    );
    r.command(
        OWNER,
        CommandSpec::new_output("display revert", tk("Undo the display change being tested right away"), |_, _| {
                Ok(pending(|| {
                    outputs::revert_now().map(|r| json!({ "reverted": r }))
                }))
            },
        ),
    );
    r.command(
        OWNER,
        CommandSpec::new_output("display persist", tk("Write the current display arrangement to the compositor drop-in so it survives restarts"), |_, cx| {
            let (tx,rx) = async_channel::bounded(1);
            cx.spawn(async move |cx| {
                let result = async {
                    let heads = outputs::jobs::run(outputs::heads).await?;
                    if let Some(saved) = cx.update(|cx| persist(&heads, cx)) {
                        saved.recv().await.map_err(|error| error.to_string())??;
                    }
                    Ok(json!({ "persisted": heads.iter().map(|head| &head.name).collect::<Vec<_>>() }).to_string())
                }.await;
                let _ = tx.send(result).await;
            }).detach();
            Ok(Output::Pending(rx))
        }),
    );
    r.command(
        OWNER,
        CommandSpec::new("display unpersist", tk("Stop writing display settings to the compositor drop-in (your own config decides again)"), |_, cx| {
            update_config(cx, |c| c.compositor.outputs.clear());
            Ok(json!({ "persisted": [] }))
        }),
    );
    r.command(
        OWNER,
        CommandSpec::new_output("display profile list", tk("Saved display profiles and which one matches the connected monitors"), |_, cx| {
            let cfg = config(cx).read(cx).snapshot();
            Ok(pending(move || {
                let connected = outputs::identities(&outputs::heads()?);
            let list: Vec<_> = cfg.displays.profiles.iter().map(|p| json!({ "name": p.name, "monitors": p.identities(), "matches": p.matches(&connected) })).collect();
            Ok(json!({ "profiles": list, "auto_apply": cfg.displays.auto_profiles }))
        }))
        }),
    );
    r.command(
        OWNER,
        CommandSpec::new_output("display profile save", tk("Save the current arrangement as a profile (applied automatically when the same monitors connect)"), |a, cx| {
            let name = a.rest(0);
            if name.trim().is_empty() {
                return Err(t("Give the profile a name").into());
            }
            let name = name.trim().to_string();
            let (tx,rx) = async_channel::bounded(1);
            cx.spawn(async move |cx| {
                let result = outputs::jobs::run(outputs::heads).await.map(|heads| {
                    cx.update(|cx| save_profile(&name, &heads, cx));
                    json!({ "saved": name, "monitors": outputs::identities(&heads) }).to_string()
                });
                let _ = tx.send(result).await;
            }).detach();
            Ok(Output::Pending(rx))
        })
        .arg(ArgSpec::required("name", ArgKind::String, tk("profile name")))
        .example("display profile save Desk"),
    );
    r.command(
        OWNER,
        CommandSpec::new_output("display profile apply", tk("Apply a saved display profile"), |a, cx| {
            let name = a.rest(0);
            let profile = config(cx).read(cx).get().displays.profiles.iter().find(|p| p.name == name.trim()).cloned().ok_or_else(|| tf("No profile named {name}", &[("name", &name)]))?;
                let (sender, receiver) = async_channel::bounded(1);
                cx.spawn(async move |cx| {
                    let preferences = profile.outputs.clone();
                    let result =
                        outputs::jobs::run(move || outputs::apply_and_snapshot(&profile.outputs))
                            .await
                            .map(|applied| {
                                if outputs::current_revision() == applied.revision {
                                    cx.update(|cx| {
                                        let mut saved = preferences;
                                        mandara_config::outputs::remember(
                                            &mut saved,
                                            applied.heads.iter().map(outputs::Head::to_config),
                                        );
                                        update_config(cx, |config| {
                                            mandara_config::outputs::remember(
                                                &mut config.compositor.outputs,
                                                saved,
                                            )
                                        });
                                    });
                                }
                                json!({ "applied": profile.name }).to_string()
                            });
                    let _ = sender.send(result).await;
                })
                .detach();
                Ok(Output::Pending(receiver))
        })
        .arg(ArgSpec::required("name", ArgKind::String, tk("profile name"))),
    );
    r.command(
        OWNER,
        CommandSpec::new("display profile delete", tk("Delete a saved display profile"), |a, cx| {
            let name = a.rest(0).trim().to_string();
            let existed = config(cx).read(cx).get().displays.profiles.iter().any(|p| p.name == name);
            update_config(cx, |c| c.displays.profiles.retain(|p| p.name != name));
            Ok(json!({ "deleted": existed }))
        })
        .arg(ArgSpec::required("name", ArgKind::String, tk("profile name"))),
    );
    r.command(
        OWNER,
        CommandSpec::new_output("display brightness status", tk("External monitors answering DDC/CI and their brightness (JSON text)"), |_, _| {
            Ok(pending(|| {
                let access = ddc::access();
                let monitors = if access == ddc::Access::Ready { ddc::monitors() } else { Vec::new() };
                Ok(json!({ "access": access, "monitors": monitors.iter().map(|m| json!({ "output": m.connector, "percent": m.percent(), "bus": m.bus })).collect::<Vec<_>>() }))
            }))
        }),
    );
    r.command(
        OWNER,
        CommandSpec::new_output("display brightness", tk("Set an external monitor's brightness over DDC/CI (or `all`)"), |a, cx| {
            let target = a.str(0)?.to_string();
            let percent = a.number(1)?;
            let cfg = config(cx).read(cx).snapshot();
            if !cfg.displays.brightness.ddc {
                return Err(t("External monitor brightness is off (Studio → Displays)").into());
            }
            let min = cfg.displays.brightness.floor_percent();
            // `all` moves the level the brightness keys continue from.
            if target == "all" && percent.is_finite() {
                cx.default_global::<DdcLevel>().0 = Some((ddc::floored_percent(percent, min) as f32) / PERCENT);
            }
            Ok(pending(move || {
                if let ddc::Access::NoPermission { group } = ddc::access() {
                    return Err(tf("No access to /dev/i2c-*: add yourself to the {group} group and log in again", &[("group", &group.unwrap_or_else(|| "i2c".into()))]));
                }
                let names: Vec<String> = if target == "all" { ddc::monitors().into_iter().map(|m| m.connector).collect() } else { vec![target] };
                let mut set = serde_json::Map::new();
                for n in names {
                    set.insert(n.clone(), json!(ddc::set_percent(&n, percent, min)?));
                }
                Ok(json!({ "brightness": set }))
            }))
        })
        .arg(ArgSpec::required("output", ArgKind::String, tk("connector or all")))
        .arg(ArgSpec::required("percent", ArgKind::Number, "1-100"))
        .example("display brightness all 60"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_property_values() {
        let mut o = OutputConfig::default();
        set_property(&mut o, "mode", "2560x1440@143.912").unwrap();
        assert_eq!((o.width, o.height, o.refresh_mhz), (Some(2560), Some(1440), Some(143_912)));
        set_property(&mut o, "mode", "1920x1080").unwrap();
        assert_eq!(o.refresh_mhz, None);
        set_property(&mut o, "position", "1920,-200").unwrap();
        assert_eq!((o.x, o.y), (Some(1920), Some(-200)));
        set_property(&mut o, "transform", "flipped-90").unwrap();
        assert_eq!(o.transform, Some(Transform::Flipped90));
        set_property(&mut o, "vrr", "fullscreen").unwrap();
        assert_eq!(o.vrr, Some(VrrMode::Fullscreen));
        set_property(&mut o, "enabled", "off").unwrap();
        assert!(!o.enabled);
        for (p, v) in [("scale", "-1"), ("scale", "inf"),
            ("scale", "NaN"),
            ("mode", "0x1080"),
            ("mode", "1920x1080@NaN"),
            ("mode", "big"), ("position", "1"), ("vrr", "maybe"), ("nope", "1")] {
            assert!(set_property(&mut o, p, v).is_err(), "{p}={v}");
        }
        assert!(PROPERTIES.iter().all(|p| *p != "nope"));
    }
}
