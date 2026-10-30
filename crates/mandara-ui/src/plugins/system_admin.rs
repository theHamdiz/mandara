//! Documented commands for system integration (Studio → Network &
//! Bluetooth, Input, Applications, Profiles & backup), so every one of
//! those settings is scriptable with `mandara msg …`:
//!
//! * `network vpn|hotspot|saved|ipv4|devices …` — NetworkManager profiles
//!   (`mandara_services::nm_admin`), run on worker threads.
//! * `bluetooth pair|trust|rename|forget|adapter|auto-connect …`.
//! * `apps defaults|default set|autostart …|preferred` — `mimeapps.list`
//!   and XDG autostart, edited non-destructively (`mandara_config::apps`).
//! * `input cursor-themes|layouts` — what Studio → Input offers.
//! * `cursor list|install|apply|remove` — Studio → Themes → Cursor (the
//!   opt-in `cursors` plugin's commands).
//! * `profile list|save|apply|delete`, `config export|import|reset`.

use gpui::{App, AsyncApp};
use mandara_config::apps::{self, AutostartNew};
use mandara_config::profiles::{self, ExportOptions, ProfileScope};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PluginRegistry};
use mandara_services::BluetoothCommand;
use mandara_services::nm_admin::{self, DbusNm, Ipv4Settings, NmBackend as _};
use serde_json::{Value, json};

use crate::hub::hub;
use crate::state::{config, update_config};

const NETWORK: &str = super::network::ID;
const BLUETOOTH: &str = "bluetooth";
const CORE: &str = "core";
/// The cursor commands belong to the opt-in cursor themes plugin (refused
/// while it is off).
const CURSORS: &str = super::cursors::ID;
/// Matches in `input layouts <query>`.
const LAYOUT_MATCHES_MAX: usize = 40;

/// Runs NetworkManager work on a thread; the command resolves to its
/// JSON (a string result stays a plain message).
fn nm_job(f: impl FnOnce(&DbusNm) -> anyhow::Result<Value> + Send + 'static) -> Output {
    Output::Pending(nm_job_rx(f))
}

fn nm_job_rx(f: impl FnOnce(&DbusNm) -> anyhow::Result<Value> + Send + 'static) -> async_channel::Receiver<Result<String, String>> {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::Builder::new()
        .name("mandara-nm".into())
        .spawn(move || {
            let result = DbusNm::connect().and_then(|nm| f(&nm)).map(|v| match v {
                Value::String(s) => s,
                other => other.to_string(),
            });
            let _ = tx.send_blocking(result.map_err(|e| format!("{e:#}")));
        })
        .ok();
    rx
}

/// Like [`nm_job`], then `after(result)` on the UI thread (config updates).
fn nm_job_then(cx: &mut App, f: impl FnOnce(&DbusNm) -> anyhow::Result<Value> + Send + 'static, after: impl FnOnce(&str, &mut App) + 'static) -> Output {
    let rx = nm_job_rx(f);
    let (tx, out) = async_channel::bounded(1);
    cx.spawn(async move |cx: &mut AsyncApp| {
        let result = rx.recv().await.unwrap_or_else(|_| Err(t("The service did not reply").into()));
        if let Ok(text) = &result {
            let text = text.clone();
            cx.update(|cx| after(&text, cx));
        }
        let _ = tx.send(result).await;
    })
    .detach();
    Output::Pending(out)
}

fn bt_request(cx: &mut App, command: BluetoothCommand) -> Result<Output, String> {
    let service = hub(cx).read(cx).bluetooth_service.clone().ok_or("bluetooth service unavailable")?;
    Ok(Output::Pending(service.request(command)))
}

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

fn stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

/// Wires `[bluetooth]` into the BlueZ actor and posts connection
/// notifications (Bluetooth devices, VPNs) when enabled.
pub fn init(cx: &mut App) {
    init_bluetooth(cx);
    init_notifications(cx);
    init_openers(cx);
    init_autostart(cx);
}

/// After the computer wakes up: "Reconnect trusted devices" also covers
/// sleep (adapters that stay powered across suspend never see the
/// power-on that triggers the reconnect otherwise).
pub fn after_resume(cx: &mut App) {
    if !config(cx).read(cx).get().bluetooth.auto_connect {
        return;
    }
    if let Some(s) = hub(cx).read(cx).bluetooth_service.clone() {
        s.send(BluetoothCommand::ConnectTrusted);
    }
}

/// Marks that this login's autostart entries were started (shell restarts
/// must not start them again). In `$XDG_RUNTIME_DIR`, cleared at logout.
const AUTOSTART_MARKER: &str = "autostart-started";
/// The systemd target that runs XDG autostart entries in systemd-managed
/// sessions (uwsm, GNOME, KDE).
const SYSTEMD_AUTOSTART_TARGET: &str = "xdg-desktop-autostart.target";

/// Whether systemd already starts the XDG autostart entries this session
/// (read-only `systemctl --user is-active`).
pub fn systemd_runs_autostart() -> bool {
    std::process::Command::new("systemctl").args(["--user", "is-active", "--quiet", SYSTEMD_AUTOSTART_TARGET]).status().is_ok_and(|s| s.success())
}

/// Studio → Applications → "Mandara starts these at login": runs the
/// enabled autostart entries once per login, unless systemd does already.
fn init_autostart(cx: &mut App) {
    let cfg = config(cx).read(cx).snapshot();
    if !cfg.apps.run_autostart {
        return;
    }
    let marker = dirs::runtime_dir().unwrap_or_else(mandara_config::paths::cache_dir).join("mandara").join(AUTOSTART_MARKER);
    if marker.exists() {
        return; // a shell restart, not a new login
    }
    if let Some(dir) = marker.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&marker, std::env::var("XDG_SESSION_ID").unwrap_or_default());
    let launch = cfg.launcher.launch.clone();
    let _ = std::thread::Builder::new().name("mandara-autostart".into()).spawn(move || {
        if systemd_runs_autostart() {
            tracing::info!("autostart: systemd's {SYSTEMD_AUTOSTART_TARGET} runs the entries; skipping");
            return;
        }
        let entries = mandara_config::apps::autostart_entries();
        for e in mandara_config::apps::autostart_to_run(&entries, &mandara_config::apps::current_desktops(), crate::apps::on_path) {
            let argv = crate::apps::launch_argv(&e.exec, false, &launch);
            let Some((program, args)) = argv.split_first() else { continue };
            use std::os::unix::process::CommandExt as _;
            match mandara_services::reaper::spawn(std::process::Command::new(program).args(args).process_group(0)) {
                Ok(_) => tracing::info!("autostart: started {}", e.id),
                Err(err) => tracing::warn!("autostart {}: {err}", e.id),
            }
        }
    });
}

/// Keeps the browser / file manager Mandara opens links and folders with in
/// step with `[apps]` (the shell, and Studio for the links it opens).
pub fn init_openers(cx: &mut App) {
    let apply = |cx: &App| {
        let a = config(cx).read(cx).get().apps.clone();
        let resolve = |v: &str| apps::resolve_command(v).unwrap_or_default();
        mandara_services::session::set_preferred_openers(&resolve(&a.browser), &resolve(&a.file_manager));
        crate::apps::set_app_id_aliases(a.id_map.clone());
        a
    };
    let last = std::cell::RefCell::new(apply(cx));
    let store = config(cx);
    cx.observe(&store, move |_, cx| {
        if config(cx).read(cx).get().apps != *last.borrow() {
            *last.borrow_mut() = apply(cx);
        }
    })
    .detach();
}

/// Names that appeared in / vanished from `now` compared with `before`.
fn diff_names(before: &[String], now: &[String]) -> (Vec<String>, Vec<String>) {
    let up = now.iter().filter(|n| !before.contains(n)).cloned().collect();
    let down = before.iter().filter(|n| !now.contains(n)).cloned().collect();
    (up, down)
}

fn post(cx: &mut App, app: &str, icon: &str, summary: String) {
    super::notifications::send(cx, mandara_services::NotificationCommand::Post { app: app.into(), icon: icon.into(), summary, body: String::new() });
}

/// A connection change worth a notification.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Announce {
    Bluetooth { name: String, connected: bool },
    Vpn { name: String, connected: bool },
}

/// Connected Bluetooth devices (address, shown name) and VPNs, each with
/// its own baseline: the first report of a service (at startup, or when it
/// comes back) is what's already there, never "just connected". Devices
/// are told apart by address, so renaming one isn't a disconnect plus a
/// connect.
#[derive(Default)]
struct ConnectionWatch {
    bluetooth: Option<Vec<(String, String)>>,
    vpns: Option<Vec<String>>,
}

impl ConnectionWatch {
    /// `None` for a service that isn't available (no adapter, no
    /// NetworkManager): its baseline resets silently.
    fn update(&mut self, bluetooth: Option<Vec<(String, String)>>, vpns: Option<Vec<String>>) -> Vec<Announce> {
        let mut out = Vec::new();
        if let (Some(before), Some(now)) = (std::mem::replace(&mut self.bluetooth, bluetooth.clone()), bluetooth) {
            let addr = |l: &[(String, String)]| l.iter().map(|(a, _)| a.clone()).collect::<Vec<_>>();
            let (up, down) = diff_names(&addr(&before), &addr(&now));
            let name = |list: &[(String, String)], a: &str| list.iter().find(|(x, _)| x == a).map(|(_, n)| n.clone()).unwrap_or_default();
            out.extend(up.iter().map(|a| Announce::Bluetooth { name: name(&now, a), connected: true }));
            out.extend(down.iter().map(|a| Announce::Bluetooth { name: name(&before, a), connected: false }));
        }
        if let (Some(before), Some(now)) = (std::mem::replace(&mut self.vpns, vpns.clone()), vpns) {
            let (up, down) = diff_names(&before, &now);
            out.extend(up.into_iter().map(|name| Announce::Vpn { name, connected: true }));
            out.extend(down.into_iter().map(|name| Announce::Vpn { name, connected: false }));
        }
        out
    }
}

fn init_notifications(cx: &mut App) {
    let snapshot = |cx: &App| {
        let h = hub(cx).read(cx);
        let bt = h.bluetooth.available.then(|| h.bluetooth.devices.iter().filter(|d| d.connected).map(|d| (d.address.clone(), d.name.clone())).collect());
        let vpns = h.network.available.then(|| h.network.vpns.clone());
        (bt, vpns)
    };
    let watch = std::cell::RefCell::new(ConnectionWatch::default());
    let hub_entity = hub(cx);
    cx.observe(&hub_entity, move |_, cx| {
        let (bt, vpns) = snapshot(cx);
        let announcements = watch.borrow_mut().update(bt, vpns);
        if announcements.is_empty() {
            return;
        }
        let (cfg_bt, cfg_net) = {
            let c = config(cx).read(cx).get();
            (c.bluetooth.connect_notifications, c.network.vpn_notifications)
        };
        for a in announcements {
            match a {
                Announce::Bluetooth { name, connected: true } if cfg_bt => post(cx, &t("Bluetooth"), "bluetooth", tf("{device} connected", &[("device", &name)])),
                Announce::Bluetooth { name, connected: false } if cfg_bt => post(cx, &t("Bluetooth"), "bluetooth-off", tf("{device} disconnected", &[("device", &name)])),
                Announce::Vpn { name, connected: true } if cfg_net => post(cx, &t("VPN"), "shield", tf("VPN {name} connected", &[("name", &name)])),
                Announce::Vpn { name, connected: false } if cfg_net => post(cx, &t("VPN"), "shield", tf("VPN {name} disconnected", &[("name", &name)])),
                _ => {}
            }
        }
    })
    .detach();
}

/// Applies the pieces of `[bluetooth]` the actor needs, and keeps them in
/// sync with the config.
fn init_bluetooth(cx: &mut App) {
    let push = |cx: &mut App| {
        let b = config(cx).read(cx).get().bluetooth.clone();
        if let Some(s) = hub(cx).read(cx).bluetooth_service.clone() {
            s.send(BluetoothCommand::SelectAdapter(b.adapter.clone()));
            s.send(BluetoothCommand::SetAutoConnect(b.auto_connect));
        }
        b
    };
    let last = std::cell::RefCell::new(push(cx));
    let store = config(cx);
    cx.observe(&store, move |_, cx| {
        let now = config(cx).read(cx).get().bluetooth.clone();
        if *last.borrow() != now {
            *last.borrow_mut() = push(cx);
        }
    })
    .detach();
}

pub fn register(r: &mut PluginRegistry) {
    register_network(r);
    register_bluetooth(r);
    register_apps(r);
    register_profiles(r);
}

fn register_network(r: &mut PluginRegistry) {
    r.command(NETWORK, CommandSpec::new_output("network vpn list", tk("Saved VPNs and which are connected"), |_, _| Ok(nm_job(|nm| Ok(json!(nm_admin::vpns(nm)?))))));
    r.command(
        NETWORK,
        CommandSpec::new_output("network vpn up", tk("Connect a VPN (the last one used when no name is given)"), |a, cx| {
            let name = match a.get(0) {
                Some(_) => a.rest(0),
                None => config(cx).read(cx).get().network.last_vpn.clone().ok_or_else(|| t("Name a VPN: no VPN was used yet").to_string())?,
            };
            Ok(nm_job_then(cx, move |nm| Ok(json!(nm_admin::vpn_set(nm, &name, true)?)), |name, cx| {
                let name = name.to_string();
                update_config(cx, |c| c.network.last_vpn = Some(name));
            }))
        })
        .arg(ArgSpec::optional("name", ArgKind::String, tk("VPN name or UUID")))
        .example("network vpn up Office"),
    );
    r.command(
        NETWORK,
        CommandSpec::new_output("network vpn down", tk("Disconnect a VPN (every connected VPN when no name is given)"), |a, _| {
            let name = a.get(0).map(|_| a.rest(0));
            Ok(nm_job(move |nm| match name {
                Some(n) => Ok(json!(nm_admin::vpn_set(nm, &n, false)?)),
                None => {
                    let mut down = Vec::new();
                    for v in nm_admin::vpns(nm)?.into_iter().filter(|v| v.active || v.connecting) {
                        down.push(nm_admin::vpn_set(nm, &v.uuid, false)?);
                    }
                    Ok(json!(down))
                }
            }))
        })
        .arg(ArgSpec::optional("name", ArgKind::String, tk("VPN name or UUID"))),
    );
    r.command(
        NETWORK,
        CommandSpec::new_output("network vpn import", tk("Import a WireGuard (.conf) or OpenVPN (.ovpn) profile"), |a, _| {
            let file = mandara_config::paths::expand_tilde(a.str(0)?);
            let name = a.get(1).map(|_| a.rest(1));
            Ok(nm_job(move |nm| Ok(json!(nm_admin::import_vpn(nm, &file, name.as_deref(), None)?))))
        })
        .arg(ArgSpec::required("file", ArgKind::String, tk("path to the profile file")))
        .arg(ArgSpec::optional("name", ArgKind::String, tk("connection name (default: the file name)")))
        .example("network vpn import ~/Downloads/office.conf Office"),
    );
    r.command(
        NETWORK,
        CommandSpec::new_output("network vpn remove", tk("Delete a VPN profile"), |a, _| {
            let name = a.rest(0);
            Ok(nm_job(move |nm| Ok(json!(nm_admin::vpn_remove(nm, &name)?))))
        })
        .arg(ArgSpec::required("name", ArgKind::String, tk("VPN name or UUID"))),
    );
    r.command(NETWORK, CommandSpec::new_output("network hotspot status", tk("Whether the hotspot runs, and its network name"), |_, _| Ok(nm_job(|nm| Ok(json!(nm_admin::hotspot_status(nm, false)?))))));
    r.command(
        NETWORK,
        CommandSpec::new_output("network hotspot start", tk("Share this computer's connection over Wi-Fi"), |a, cx| {
            let net = config(cx).read(cx).get().network.clone();
            let password = a.get(0).map(str::to_string);
            let (ssid, band) = (net.hotspot_name(), net.hotspot_band.nm());
            Ok(nm_job(move |nm| {
                let info = nm_admin::hotspot_start(nm, &ssid, password.as_deref(), band)?;
                Ok(json!({ "ssid": info.ssid, "active": info.active, "qr": mandara_config::connectivity::wifi_qr_payload(&info.ssid, info.password.as_deref(), false) }))
            }))
        })
        .arg(ArgSpec::optional("password", ArgKind::String, tk("8–63 characters; default: the saved or a generated one"))),
    );
    r.command(NETWORK, CommandSpec::new_output("network hotspot stop", tk("Stop the hotspot"), |_, _| Ok(nm_job(|nm| Ok(json!(if nm_admin::hotspot_stop(nm)? { "hotspot stopped" } else { "the hotspot wasn't running" }))))));
    r.command(
        NETWORK,
        CommandSpec::new_output("network hotspot share", tk("The hotspot's name, password and Wi-Fi QR payload"), |_, _| {
            Ok(nm_job(|nm| {
                let info = nm_admin::hotspot_status(nm, true)?.ok_or_else(|| anyhow::anyhow!(t("No hotspot has been set up yet")))?;
                Ok(json!({ "ssid": info.ssid, "password": info.password, "qr": mandara_config::connectivity::wifi_qr_payload(&info.ssid, info.password.as_deref(), false) }))
            }))
        }),
    );
    r.command(NETWORK, CommandSpec::new_output("network saved list", tk("Saved Wi-Fi networks with autoconnect and priority"), |_, _| Ok(nm_job(|nm| Ok(json!(nm_admin::saved_wifi(nm)?))))));
    r.command(
        NETWORK,
        CommandSpec::new_output("network saved forget", tk("Forget a saved network"), |a, _| {
            let name = a.rest(0);
            Ok(nm_job(move |nm| Ok(json!(nm_admin::forget(nm, &name)?))))
        })
        .arg(ArgSpec::required("name", ArgKind::String, tk("connection name or UUID"))),
    );
    r.command(
        NETWORK,
        CommandSpec::new_output("network saved autoconnect", tk("Join a saved network automatically, or not"), |a, _| {
            let on = a.bool(0)?;
            let name = a.rest(1);
            Ok(nm_job(move |nm| Ok(json!(nm_admin::set_autoconnect(nm, &name, on)?))))
        })
        .arg(ArgSpec::required("state", ArgKind::Bool, tk("on / off")))
        .arg(ArgSpec::required("name", ArgKind::String, tk("connection name or UUID")))
        .example("network saved autoconnect off Cafe"),
    );
    r.command(
        NETWORK,
        CommandSpec::new_output("network saved priority", tk("Autoconnect priority (higher wins when several are in range)"), |a, _| {
            let p = a.int(0)? as i32;
            let name = a.rest(1);
            Ok(nm_job(move |nm| Ok(json!(nm_admin::set_priority(nm, &name, p)?))))
        })
        .arg(ArgSpec::required("priority", ArgKind::Integer, tk("-999 to 999")))
        .arg(ArgSpec::required("name", ArgKind::String, tk("connection name or UUID"))),
    );
    r.command(
        NETWORK,
        CommandSpec::new_output("network wifi hidden", tk("Connect to a hidden network"), |a, _| {
            let ssid = a.str(0)?.to_string();
            let password = a.get(1).map(str::to_string);
            Ok(nm_job(move |nm| Ok(json!(nm_admin::connect_hidden(nm, &ssid, password.as_deref())?))))
        })
        .arg(ArgSpec::required("ssid", ArgKind::String, tk("network name (quote if it has spaces)")))
        .arg(ArgSpec::optional("password", ArgKind::String, tk("WPA passphrase"))),
    );
    r.command(NETWORK, CommandSpec::new_output("network devices", tk("Network devices with addresses, gateway, DNS and MAC"), |_, _| Ok(nm_job(|nm| Ok(json!(nm.devices()?))))));
    r.command(
        NETWORK,
        CommandSpec::new_output("network ipv4 show", tk("A connection's IPv4 method, addresses, gateway and DNS"), |a, _| {
            let name = a.rest(0);
            Ok(nm_job(move |nm| Ok(json!(nm_admin::ipv4(nm, &name)?))))
        })
        .arg(ArgSpec::required("name", ArgKind::String, tk("connection name or UUID"))),
    );
    r.command(
        NETWORK,
        CommandSpec::new_output("network ipv4 set", tk("Set DHCP or a static address; DNS servers comma-separated (applies on reconnect)"), |a, _| {
            let name = a.str(0)?.to_string();
            let method = a.str(1)?.to_string();
            let split = |s: Option<&str>| s.map(|s| s.split(',').map(str::trim).filter(|x| !x.is_empty() && *x != "-").map(String::from).collect::<Vec<_>>()).unwrap_or_default();
            let v4 = Ipv4Settings {
                method,
                addresses: split(a.get(2)),
                gateway: a.get(3).filter(|g| *g != "-").map(String::from),
                dns: split(a.get(4)),
                ignore_auto_dns: a.get(4).is_some_and(|d| d != "-"),
            };
            Ok(nm_job(move |nm| Ok(json!(nm_admin::set_ipv4(nm, &name, &v4)?))))
        })
        .arg(ArgSpec::required("name", ArgKind::String, tk("connection name or UUID")))
        .arg(ArgSpec::required("method", ArgKind::Enum, tk("auto (DHCP) or manual")).choices(&["auto", "manual"]))
        .arg(ArgSpec::optional("address", ArgKind::String, tk("e.g. 192.168.1.20/24 (- for none)")))
        .arg(ArgSpec::optional("gateway", ArgKind::String, tk("e.g. 192.168.1.1 (- for none)")))
        .arg(ArgSpec::optional("dns", ArgKind::String, tk("e.g. 1.1.1.1,9.9.9.9")))
        .example("network ipv4 set Home manual 192.168.1.20/24 192.168.1.1 1.1.1.1"),
    );
}

fn register_bluetooth(r: &mut PluginRegistry) {
    r.command(
        BLUETOOTH,
        CommandSpec::new_output("bluetooth pair", tk("Pair and trust a discovered device (prompts appear in Mandara's dialog)"), |a, cx| bt_request(cx, BluetoothCommand::Pair(a.rest(0))))
            .arg(ArgSpec::required("device", ArgKind::String, tk("device name or MAC address"))),
    );
    r.command(
        BLUETOOTH,
        CommandSpec::new_output("bluetooth trust", tk("Trust a device (it may connect by itself and is auto-connected)"), |a, cx| bt_request(cx, BluetoothCommand::SetTrusted(a.rest(1), a.bool(0)?)))
            .arg(ArgSpec::required("state", ArgKind::Bool, tk("on / off")))
            .arg(ArgSpec::required("device", ArgKind::String, tk("device name or MAC address"))),
    );
    r.command(
        BLUETOOTH,
        CommandSpec::new_output("bluetooth rename", tk("Rename a device"), |a, cx| bt_request(cx, BluetoothCommand::Rename(a.str(0)?.to_string(), a.rest(1))))
            .arg(ArgSpec::required("device", ArgKind::String, tk("device name or MAC address (quote names with spaces)")))
            .arg(ArgSpec::required("name", ArgKind::String, tk("the new name")))
            .example("bluetooth rename AA:BB:CC:DD:EE:FF Living room speaker"),
    );
    r.command(
        BLUETOOTH,
        CommandSpec::new_output("bluetooth forget", tk("Remove a paired device"), |a, cx| bt_request(cx, BluetoothCommand::Forget(a.rest(0))))
            .arg(ArgSpec::required("device", ArgKind::String, tk("device name or MAC address"))),
    );
    r.command(BLUETOOTH, CommandSpec::new("bluetooth adapters", tk("Bluetooth adapters and which one Mandara uses"), |_, cx| Ok(super::json(&hub(cx).read(cx).bluetooth.adapters))));
    r.command(
        BLUETOOTH,
        CommandSpec::new("bluetooth adapter", tk("Use another adapter (auto = the first)"), |a, cx| {
            let name = a.str(0)?.to_string();
            let pick = (name != "auto").then_some(name);
            update_config(cx, |c| c.bluetooth.adapter = pick.clone());
            Ok(json!({ "adapter": pick.unwrap_or_else(|| "auto".into()) }))
        })
        .arg(ArgSpec::required("adapter", ArgKind::String, tk("hci0, hci1… or auto"))),
    );
    r.command(
        BLUETOOTH,
        CommandSpec::new("bluetooth auto-connect", tk("Reconnect trusted devices when Bluetooth turns on"), |a, cx| {
            let on = a.bool(0)?;
            update_config(cx, |c| c.bluetooth.auto_connect = on);
            Ok(json!({ "auto_connect": on }))
        })
        .arg(ArgSpec::required("state", ArgKind::Bool, tk("on / off"))),
    );
    r.command(BLUETOOTH, CommandSpec::new_output("bluetooth connect-trusted", tk("Connect every trusted device in range"), |_, cx| bt_request(cx, BluetoothCommand::ConnectTrusted)));
}

fn register_apps(r: &mut PluginRegistry) {
    r.command(
        CORE,
        CommandSpec::new("apps defaults", tk("Default apps per category (from mimeapps.list)"), |_, _| {
            let file = apps::mimeapps_file();
            Ok(Value::Array(
                apps::APP_CATEGORIES.iter().map(|c| json!({ "category": c.id, "label": mandara_i18n::tr(c.label), "default": apps::default_for(&file, c.mime_types[0]) })).collect(),
            ))
        }),
    );
    let ids: Vec<&'static str> = apps::APP_CATEGORIES.iter().map(|c| c.id).collect();
    r.command(
        CORE,
        CommandSpec::new("apps default set", tk("Make an app the default for a category (edits only those keys of mimeapps.list)"), |a, _| {
            let cat = apps::category(a.str(0)?).ok_or_else(|| t("Unknown category").to_string())?;
            let mut id = a.str(1)?.trim().to_string();
            if !id.ends_with(".desktop") {
                id.push_str(".desktop");
            }
            if apps::find_desktop(&id).is_none() {
                return Err(tf("No installed app {id}", &[("id", &id)]));
            }
            apps::set_default(cat, &id).map_err(err)?;
            Ok(json!({ "category": cat.id, "default": id }))
        })
        .arg(ArgSpec::required("category", ArgKind::Enum, tk("which kind of file or link")).choices(&ids))
        .arg(ArgSpec::required("app", ArgKind::String, tk("desktop id, e.g. firefox or org.gnome.Nautilus")))
        .example("apps default set browser firefox"),
    );
    r.command(CORE, CommandSpec::new("apps autostart list", tk("Apps started when you log in"), |_, _| Ok(super::json(apps::autostart_entries()))));
    r.command(
        CORE,
        CommandSpec::new("apps autostart enable", tk("Start an app at login again"), |a, _| {
            apps::set_autostart_enabled(&desktop_id(&a.rest(0)), true).map_err(err)?;
            Ok(json!("enabled"))
        })
        .arg(ArgSpec::required("entry", ArgKind::String, tk("autostart entry id"))),
    );
    r.command(
        CORE,
        CommandSpec::new("apps autostart disable", tk("Stop an app from starting at login (Hidden=true, nothing is deleted)"), |a, _| {
            apps::set_autostart_enabled(&desktop_id(&a.rest(0)), false).map_err(err)?;
            Ok(json!("disabled"))
        })
        .arg(ArgSpec::required("entry", ArgKind::String, tk("autostart entry id"))),
    );
    r.command(
        CORE,
        CommandSpec::new("apps autostart add", tk("Start an installed app (desktop id) or a command at login"), |a, _| {
            let what = a.rest(0);
            let id = desktop_id(&what);
            let new = if !what.contains(' ') && apps::find_desktop(&id).is_some() { AutostartNew::App(id) } else { AutostartNew::Command { name: String::new(), command: what } };
            Ok(json!({ "added": apps::add_autostart(&new).map_err(err)? }))
        })
        .arg(ArgSpec::required("app", ArgKind::String, tk("desktop id or command line")))
        .example("apps autostart add org.keepassxc.KeePassXC"),
    );
    r.command(
        CORE,
        CommandSpec::new("apps autostart remove", tk("Remove an autostart entry Mandara added"), |a, _| {
            apps::remove_autostart(&desktop_id(&a.rest(0))).map_err(err)?;
            Ok(json!("removed"))
        })
        .arg(ArgSpec::required("entry", ArgKind::String, tk("autostart entry id"))),
    );
    r.command(
        CORE,
        CommandSpec::new("apps preferred", tk("The terminal, file manager and browser Mandara launches"), |_, cx| {
            let a = config(cx).read(cx).get().apps.clone();
            Ok(json!({ "terminal": a.terminal_command(), "file_manager": a.file_manager_command(), "browser": a.browser_command() }))
        }),
    );
    register_uninstall(r);
    register_cursor(r);
    r.command(CORE, CommandSpec::new("input cursor-themes", tk("Installed cursor themes"), |_, _| Ok(json!(mandara_config::input::cursor_themes()))));
    r.command(
        CORE,
        CommandSpec::new("input layouts", tk("Keyboard layouts (xkb) matching a search"), |a, _| {
            let q = a.rest(0).to_lowercase();
            let cat = mandara_config::input::xkb_catalog();
            let list: Vec<Value> = cat
                .layouts
                .iter()
                .filter(|l| q.is_empty() || l.name.to_lowercase().contains(&q) || l.description.to_lowercase().contains(&q))
                .take(LAYOUT_MATCHES_MAX)
                .map(|l| json!({ "layout": l.name, "description": l.description, "variants": l.variants.len() }))
                .collect();
            Ok(Value::Array(list))
        })
        .arg(ArgSpec::optional("query", ArgKind::String, tk("name or language, e.g. arabic"))),
    );
}

/// Theme names `cursor apply` takes for "back to your compositor's cursor".
const CURSOR_UNSET: [&str; 3] = ["none", "default", "your-config"];

/// The XCursor theme names a `cursor remove` spec covers: one variant
/// (`bibata/ice`, `Bibata-Modern-Ice`) or every variant of a catalog theme
/// (`bibata`).
fn cursor_targets(spec: &str) -> Vec<&'static str> {
    use mandara_services::cursors::catalog;
    match (catalog::find(spec), spec.contains('/')) {
        (Some(theme), false) => theme.variants.iter().map(|v| v.theme).collect(),
        _ => catalog::resolve(spec).map(|(_, v)| vec![v.theme]).unwrap_or_default(),
    }
}

/// Makes `theme` the cursor (and `~/.icons/default` follow, when that file
/// is Mandara's); the compositor drop-in and live reload follow the config.
fn apply_cursor(cx: &mut App, theme: Option<String>, size: Option<u32>) -> Value {
    let next = theme.clone();
    update_config(cx, move |c| {
        c.compositor.input.cursor.theme = next;
        if size.is_some() {
            c.compositor.input.cursor.size = size;
        }
    });
    let x11 = mandara_config::input::sync_default_cursor(theme.as_deref()).unwrap_or(false);
    json!({ "theme": theme, "size": config(cx).read(cx).get().compositor.input.cursor.size, "x11_default_follows": x11 })
}

/// `cursor list|install|apply|remove`: the Studio → Themes → Cursor gallery.
fn register_cursor(r: &mut PluginRegistry) {
    use mandara_services::cursors::{catalog, install};
    r.command(
        CURSORS,
        CommandSpec::new("cursor list", tk("The cursor theme catalog (variants, licence, installed, active) and other installed themes"), |_, cx| {
            let installed = mandara_config::input::cursor_themes();
            let ours: Vec<String> = install::installed(&install::Env::current()).into_iter().map(|i| i.theme).collect();
            let active = config(cx).read(cx).get().compositor.input.cursor.theme.clone();
            let catalog: Vec<Value> = catalog::CATALOG
                .iter()
                .map(|t| {
                    let variants: Vec<Value> = t
                        .variants
                        .iter()
                        .map(|v| json!({ "id": format!("{}/{}", t.id, v.id), "theme": v.theme, "installed": installed.iter().any(|n| n == v.theme), "by_mandara": ours.iter().any(|n| n == v.theme), "active": active.as_deref() == Some(v.theme), "download_bytes": v.asset.size, "url": v.asset.url, "sha256": v.asset.sha256 }))
                        .collect();
                    json!({ "id": t.id, "name": t.name, "author": t.author, "license": t.license, "homepage": t.homepage, "stars": t.stars, "variants": variants })
                })
                .collect();
            let others: Vec<&String> = installed.iter().filter(|n| catalog::by_theme_name(n).is_none()).collect();
            Ok(json!({ "active": active, "catalog": catalog, "other_installed": others }))
        }),
    );
    r.command(
        CURSORS,
        CommandSpec::new_output("cursor install", tk("Download a catalog cursor theme from its GitHub release, verify its SHA-256 and install it to ~/.local/share/icons"), |a, _| {
            let spec = a.get(0).ok_or("expected a theme id, e.g. bibata/ice (see `cursor list`)")?.to_string();
            let (theme, variant) = catalog::resolve(&spec).ok_or_else(|| format!("unknown cursor theme {spec:?} (see `cursor list`)"))?;
            let (tx, rx) = async_channel::bounded(1);
            let _ = std::thread::Builder::new().name("mandara-cursor".into()).spawn(move || {
                let result = install::install(theme.id, variant, &install::Env::current(), &|_| {}, &std::sync::atomic::AtomicBool::new(false)).map(|i| serde_json::to_string_pretty(&json!({ "installed": i.theme, "dir": i.dir, "sha256": i.sha256 })).unwrap_or_default());
                let _ = tx.send_blocking(result);
            });
            Ok(Output::Pending(rx))
        })
        .arg(ArgSpec::required("id", ArgKind::String, tk("catalog id[/variant] or XCursor theme name")))
        .example("cursor install bibata/ice"),
    );
    r.command(
        CURSORS,
        CommandSpec::new("cursor apply", tk("Make an installed cursor theme (and optionally a size) the cursor, live; `none` returns to your compositor's own"), |a, cx| {
            let spec = a.get(0).ok_or("expected a theme (see `cursor list`)")?;
            let size = match a.get(1) {
                None => None,
                Some(_) => {
                    let (lo, hi) = mandara_config::input::limits::CURSOR_SIZE;
                    let n = a.int(1)?;
                    Some(u32::try_from(n).ok().filter(|n| (lo..=hi).contains(n)).ok_or_else(|| format!("size must be {lo}–{hi}"))?)
                }
            };
            if CURSOR_UNSET.contains(&spec.to_lowercase().as_str()) {
                return Ok(apply_cursor(cx, None, size));
            }
            let name = catalog::resolve(spec).map(|(_, v)| v.theme.to_string()).unwrap_or_else(|| spec.to_string());
            if !mandara_config::input::cursor_themes().contains(&name) {
                return Err(format!("{name} isn't installed (run `mandara msg cursor install {spec}`)"));
            }
            Ok(apply_cursor(cx, Some(name), size))
        })
        .arg(ArgSpec::required("theme", ArgKind::String, tk("catalog id[/variant], XCursor theme name, or none")))
        .arg(ArgSpec::optional("size", ArgKind::Integer, tk("cursor size in px, e.g. 24, 32, 48")))
        .example("cursor apply bibata/ice 32"),
    );
    r.command(
        CURSORS,
        CommandSpec::new("cursor remove", tk("Remove a cursor theme Mandara installed (never one from a package or your own folders)"), |a, cx| {
            let spec = a.get(0).ok_or("expected a theme id (see `cursor list`)")?;
            let env = install::Env::current();
            let ours: Vec<String> = install::installed(&env).into_iter().map(|i| i.theme).collect();
            let targets: Vec<&str> = cursor_targets(spec).into_iter().filter(|t| ours.iter().any(|o| o == t)).collect();
            let targets = if targets.is_empty() { vec![spec] } else { targets };
            let active = config(cx).read(cx).get().compositor.input.cursor.theme.clone();
            let mut removed = Vec::new();
            for theme in targets {
                install::remove(theme, &env)?;
                if active.as_deref() == Some(theme) {
                    apply_cursor(cx, None, None);
                }
                removed.push(theme.to_string());
            }
            Ok(json!({ "removed": removed }))
        })
        .arg(ArgSpec::required("id", ArgKind::String, tk("catalog id[/variant] or XCursor theme name")))
        .example("cursor remove bibata"),
    );
}

/// `--purge-data`, `--dry-run`, `--confirm-default` after `apps uninstall <id>`.
const UNINSTALL_FLAGS: [&str; 3] = ["--purge-data", "--dry-run", "--confirm-default"];

/// Runs `f` on a worker thread (owner queries and removals block).
fn uninstall_job(f: impl FnOnce() -> Result<Value, String> + Send + 'static) -> Output {
    let (tx, rx) = async_channel::bounded(1);
    let _ = std::thread::Builder::new().name("mandara-uninstall".into()).spawn(move || {
        let _ = tx.send_blocking(f().map(|v| serde_json::to_string_pretty(&v).unwrap_or_default()));
    });
    Output::Pending(rx)
}

fn register_uninstall(r: &mut PluginRegistry) {
    use mandara_services::app_uninstall::{self as un, AppRef, Choices, SystemRunner};
    r.command(
        CORE,
        CommandSpec::new_output("apps installed", tk("Installed apps with where each came from (package manager, Flatpak, Snap, AppImage, Mandara, local); --by-mandara lists what Mandara's installer installed, with every file it wrote"), |a, _| {
            let by_mandara = match a.get(0) {
                None => false,
                Some("--by-mandara") => true,
                Some(other) => return Err(tf("Unknown option {option}; expected {options}", &[("option", &other), ("options", &"--by-mandara")])),
            };
            Ok(uninstall_job(move || {
                let env = un::Env::current();
                if by_mandara {
                    return Ok(json!(mandara_services::app_install::registry::installed(&env.data_home)));
                }
                Ok(Value::Array(un::installed(&env, &SystemRunner).into_iter().filter(|a| !a.no_display).map(|a| json!({ "id": a.app.id, "name": a.app.name, "source": a.source, "path": a.app.path })).collect()))
            }))
        })
        .arg(ArgSpec::optional("options", ArgKind::String, tk("--by-mandara")))
        .example("apps installed --by-mandara"),
    );
    r.command(
        CORE,
        CommandSpec::new_output("apps owner", tk("Who installed an app, the exact removal command and its leftovers (changes nothing)"), |a, cx| {
            let id = un::desktop_id(&a.rest(0));
            let apps = config(cx).read(cx).get().apps.clone();
            Ok(uninstall_job(move || {
                let app = AppRef::find(&id).ok_or_else(|| tf("No installed app {id}", &[("id", &id)]))?;
                let plan = un::plan(&app, &un::env_for(&apps), &SystemRunner);
                Ok(json!(plan))
            }))
        })
        .arg(ArgSpec::required("app", ArgKind::String, tk("desktop id, e.g. firefox or org.gimp.GIMP")))
        .example("apps owner firefox"),
    );
    r.command(
        CORE,
        CommandSpec::new_output("apps uninstall", tk("Uninstall an app with its own package manager (system packages ask polkit); leftovers go to the Trash only with --purge-data"), |a, cx| {
            let id = un::desktop_id(a.str(0)?);
            let flags: Vec<String> = a.0.iter().skip(1).cloned().collect();
            if let Some(bad) = flags.iter().find(|f| !UNINSTALL_FLAGS.contains(&f.as_str())) {
                return Err(tf("Unknown option {option}; expected {options}", &[("option", bad), ("options", &UNINSTALL_FLAGS.join(", "))]));
            }
            let has = |f: &str| flags.iter().any(|x| x == f);
            let (purge, dry_run, confirm) = (has(UNINSTALL_FLAGS[0]), has(UNINSTALL_FLAGS[1]), has(UNINSTALL_FLAGS[2]));
            let apps = config(cx).read(cx).get().apps.clone();
            Ok(uninstall_job(move || {
                let app = AppRef::find(&id).ok_or_else(|| tf("No installed app {id}", &[("id", &id)]))?;
                let env = un::env_for(&apps);
                let plan = un::plan(&app, &env, &SystemRunner);
                let choices = Choices { purge: if purge { plan.leftovers.iter().map(|l| l.path.clone()).collect() } else { Vec::new() }, confirm_cautions: confirm, dry_run };
                let outcome = un::execute(&plan, &choices, &env, &SystemRunner)?;
                Ok(json!({ "app": plan.app.id, "source": plan.owner.source, "package": plan.owner.package, "also_removes": plan.also_removes, "outcome": outcome }))
            }))
        })
        .arg(ArgSpec::required("app", ArgKind::String, tk("desktop id")))
        .arg(ArgSpec::optional("options", ArgKind::String, tk("--purge-data (leftovers to the Trash), --dry-run, --confirm-default (allow removing a default app)")))
        .example("apps uninstall org.gimp.GIMP --dry-run")
        .example("apps uninstall obsidian --purge-data"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_notices_need_a_baseline_per_service() {
        let dev = |a: &str, n: &str| (a.to_string(), n.to_string());
        let mut w = ConnectionWatch::default();
        // Startup: Bluetooth reports first, NetworkManager later. Neither
        // announces what was already connected.
        assert!(w.update(Some(vec![dev("AA", "Buds")]), None).is_empty());
        assert!(w.update(Some(vec![dev("AA", "Buds")]), Some(vec!["Work".into()])).is_empty(), "an active VPN at startup is not news");
        // A rename keeps the address: no disconnect / connect pair.
        assert!(w.update(Some(vec![dev("AA", "My buds")]), Some(vec!["Work".into()])).is_empty());
        let got = w.update(Some(vec![dev("BB", "Mouse")]), Some(vec![]));
        assert_eq!(
            got,
            [
                Announce::Bluetooth { name: "Mouse".into(), connected: true },
                Announce::Bluetooth { name: "My buds".into(), connected: false },
                Announce::Vpn { name: "Work".into(), connected: false },
            ]
        );
        // Bluetooth goes away (adapter switch / off) and back: silent rebaseline.
        assert!(w.update(None, Some(vec![])).is_empty());
        assert!(w.update(Some(vec![dev("CC", "Keyboard")]), Some(vec![])).is_empty());
        assert_eq!(w.update(Some(vec![]), Some(vec!["Home".into()])), [Announce::Bluetooth { name: "Keyboard".into(), connected: false }, Announce::Vpn { name: "Home".into(), connected: true }]);
    }

    #[test]
    fn name_diffs() {
        let (up, down) = diff_names(&["Buds".into(), "Mouse".into()], &["Mouse".into(), "Keyboard".into()]);
        assert_eq!((up, down), (vec!["Keyboard".to_string()], vec!["Buds".to_string()]));
        assert_eq!(desktop_id("firefox"), "firefox.desktop");
        assert_eq!(desktop_id("a.desktop"), "a.desktop");
    }
}

fn desktop_id(s: &str) -> String {
    let s = s.trim();
    if s.ends_with(".desktop") { s.to_string() } else { format!("{s}.desktop") }
}

fn register_profiles(r: &mut PluginRegistry) {
    r.command(CORE, CommandSpec::new("profile list", tk("Saved settings profiles"), |_, _| Ok(Value::Array(profiles::list().iter().map(|p| json!({ "name": p.name, "scope": p.scope, "saved": p.saved, "description": p.description })).collect()))));
    r.command(
        CORE,
        CommandSpec::new("profile save", tk("Save the current settings as a profile (look = appearance only)"), |a, cx| {
            let scope = match a.str(0)? {
                "full" => ProfileScope::Full,
                _ => ProfileScope::Look,
            };
            let name = a.rest(1);
            let cfg = config(cx).read(cx).snapshot();
            let saved = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
            let profile = profiles::snapshot(&cfg, &name, "", scope, &saved).map_err(err)?;
            let path = profiles::save(&profile).map_err(err)?;
            Ok(json!({ "saved": profile.name, "file": path }))
        })
        .arg(ArgSpec::required("scope", ArgKind::Enum, tk("look or full")).choices(&["look", "full"]))
        .arg(ArgSpec::required("name", ArgKind::String, tk("profile name")))
        .example("profile save look Night"),
    );
    r.command(
        CORE,
        CommandSpec::new("profile apply", tk("Switch to a saved profile"), |a, cx| {
            let name = a.rest(0);
            let profile = profiles::find(&name).ok_or_else(|| tf("No profile named \"{name}\"", &[("name", &name)]))?;
            let cfg = config(cx).read(cx).snapshot();
            let next = profiles::apply(&cfg, &profile).map_err(err)?;
            update_config(cx, |c| *c = next);
            Ok(json!({ "applied": profile.name }))
        })
        .arg(ArgSpec::required("name", ArgKind::String, tk("profile name"))),
    );
    r.command(
        CORE,
        CommandSpec::new("profile delete", tk("Delete a saved profile"), |a, _| {
            profiles::delete(&a.rest(0)).map_err(err)?;
            Ok(json!("deleted"))
        })
        .arg(ArgSpec::required("name", ArgKind::String, tk("profile name"))),
    );
    r.command(
        CORE,
        CommandSpec::new("config export", tk("Write the settings to a file (changes = only values that differ from the defaults)"), |a, cx| {
            let path = mandara_config::paths::expand_tilde(a.str(0)?);
            let changes_only = a.get(1) == Some("changes");
            let redact = a.get(2) == Some("redact");
            let cfg = config(cx).read(cx).snapshot();
            let (text, sensitive) = profiles::export_text(&cfg, ExportOptions { changes_only, redact }).map_err(err)?;
            if path.exists() {
                return Err(tf("{file} already exists", &[("file", &path.display())]));
            }
            mandara_config::write_atomic(&path, text.as_bytes()).map_err(err)?;
            Ok(json!({ "file": path, "personal_values": sensitive }))
        })
        .arg(ArgSpec::required("file", ArgKind::String, tk("where to write (never overwritten)")))
        .arg(ArgSpec::optional("what", ArgKind::Enum, tk("changes or full (default)")).choices(&["full", "changes"]))
        .arg(ArgSpec::optional("privacy", ArgKind::Enum, tk("redact replaces locations, keys and accounts")).choices(&["keep", "redact"]))
        .example("config export ~/mandara-settings.toml changes"),
    );
    r.command(
        CORE,
        CommandSpec::new("config import", tk("Load settings from an exported file (the current ones are backed up first)"), |a, cx| {
            let path = mandara_config::paths::expand_tilde(a.str(0)?);
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let next = profiles::import_text(&text).map_err(err)?;
            let backup = profiles::backup_config_dir(&stamp(), "before-import").map_err(err)?;
            update_config(cx, |c| *c = next);
            Ok(json!({ "imported": path, "backup": backup }))
        })
        .arg(ArgSpec::required("file", ArgKind::String, tk("an exported settings file"))),
    );
    r.command(
        CORE,
        CommandSpec::new("config reset", tk("Put one settings section back to its defaults"), |a, cx| {
            let section = a.str(0)?.to_string();
            let cfg = config(cx).read(cx).snapshot();
            let next = profiles::reset_section(&cfg, &section).map_err(err)?;
            update_config(cx, |c| *c = next);
            Ok(json!({ "reset": section }))
        })
        .arg(ArgSpec::required("section", ArgKind::String, tk("top-level section, e.g. bar, launcher, compositor")))
        .example("config reset bar"),
    );
}
