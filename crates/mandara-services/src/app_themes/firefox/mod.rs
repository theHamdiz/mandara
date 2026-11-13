//! Browser theme integration through the Pywalfox extension's native protocol.

#[cfg(any(target_os = "linux", test))]
mod css;
#[cfg(target_os = "linux")]
mod host;
#[cfg(target_os = "linux")]
mod watch;
#[cfg(any(target_os = "linux", test))]
mod wire;

use super::paths::Locations;
use anyhow::Result;
use mandara_theme::templates::Ledger;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};

pub const EXTENSION_ID: &str = "pywalfox@frewacom.org";
const DESCRIPTION: &str = "Mandara Shell Firefox theme host";
/// The browser protocol uses sixteen ordered slots; unused slots may be empty.
const COLOR_KEYS: [&str;16] = ["color0","color1","color2","color3","color4","color5","color6","color7","color8","color9","color10","color11","color12","color13","color14","color15"];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Settings {
    colors: PathBuf,
    mode: String,
    styles: PathBuf,
    #[serde(default)]
    owner: String,
    #[serde(skip)]
    _lease: Option<std::sync::Arc<std::fs::File>>,
}

pub(super) fn settings_path(locations: &Locations) -> PathBuf {
    locations
        .config
        .join("mandara/generated/firefox-theme.json")
}

pub fn launched(arguments: impl IntoIterator<Item = String>) -> bool {
    arguments
        .into_iter()
        .take(2)
        .any(|argument| argument == EXTENSION_ID)
}

pub fn host() -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        host::run(&Locations::current()?)
    }
    #[cfg(not(target_os = "linux"))]
    {
        anyhow::bail!(
            "the browser native host requires Linux; an existing Pywalfox host can still receive generated colors"
        )
    }
}

#[cfg(any(target_os = "linux", test))]
pub(super) fn settings(locations: &Locations) -> Result<Settings> {
    let mut settings: Settings =
        serde_json::from_str(&super::apply::read_text(&settings_path(locations))?)?;
    if let Some(root) = settings.styles.parent().and_then(Path::parent)
        && let Ok(file) = std::fs::File::open(root.join(".lease"))
    {
        file.lock_shared()?;
        settings._lease = Some(std::sync::Arc::new(file));
    }
    Ok(settings)
}

pub(super) fn colors(settings: &Settings) -> Result<serde_json::Value> {
    let root: serde_json::Value =
        serde_json::from_str(&super::apply::read_text(&settings.colors)?)?;
    let palette = root["colors"].as_object().ok_or_else(||anyhow::anyhow!("browser theme has no palette"))?;
    let numbered = COLOR_KEYS.map(|key|palette.get(key).and_then(serde_json::Value::as_str));
    let colors:Vec<&str> = if numbered.iter().all(Option::is_some) {
        numbered.into_iter().flatten().collect()
    } else {
        palette.values().filter_map(serde_json::Value::as_str).take(COLOR_KEYS.len()).collect()
    };
    anyhow::ensure!(colors.len() == COLOR_KEYS.len(),"browser theme needs sixteen color slots");
    let wallpaper = root["wallpaper"].as_str().ok_or_else(||anyhow::anyhow!("browser theme has no wallpaper path"))?;
    Ok(
        json!({"action":"action:colors","success":true,"data":{"colors":colors,"wallpaper":wallpaper}}),
    )
}

fn shared_palette(owner: &str, output: &Path, locations: &Locations, ledger: &mut Ledger, approved: bool) -> Result<Vec<PathBuf>> {
    let standard = locations.cache.join("wal/colors.json");
    let same = mandara_theme::templates::destination::slot(&standard)? == mandara_theme::templates::destination::slot(output)?;
    anyhow::ensure!(approved || same, mandara_i18n::t("Review and approve the shared browser palette first. Other Pywal apps will read these colors too."));
    if same { return Ok(Vec::new()); }
    ledger.put_file(owner, &standard, &super::apply::read_text(output)?)?;
    Ok(vec![standard])
}

pub fn apply(
    owner: &str,
    output: &Path,
    package: &Path,
    mode: &str,
    locations: &Locations,
    ledger: &mut Ledger,
    shared_palette_approved: bool,
) -> Result<Vec<PathBuf>> {
    let settings = Settings {
        colors: output.into(),
        mode: mode.into(),
        styles: package.join(".mandara/firefox"),
        owner: owner.into(),
        _lease: None,
    };
    colors(&settings)?;
    let path = settings_path(locations);
    let manifest = locations
        .home
        .join(".mozilla/native-messaging-hosts/pywalfox.json");
    let foreign = std::fs::symlink_metadata(&manifest).is_ok()
        && super::apply::read_text(&manifest)
            .ok()
            .and_then(|bytes| serde_json::from_str::<serde_json::Value>(&bytes).ok())
            .is_none_or(|value| value["description"] != DESCRIPTION);
    anyhow::ensure!(
        foreign || cfg!(target_os = "linux"),
        "the built-in browser host requires Linux; install an external Pywalfox host on this platform"
    );
    let mut written = if foreign { shared_palette(owner, output, locations, ledger, shared_palette_approved)? } else { Vec::new() };
    ledger.put_file(owner, &path, &serde_json::to_string(&settings)?)?;
    written.push(path);
    if !foreign {
        let body = json!({"name":"pywalfox","description":DESCRIPTION,"path":mandara_config::paths::self_exe(),"type":"stdio","allowed_extensions":[EXTENSION_ID]});
        ledger.put_file(owner, &manifest, &serde_json::to_string_pretty(&body)?)?;
        written.push(manifest);
    }
    // An existing host remains the browser's choice. Its established socket
    // receives updates without replacing the manifest or starting another host.
    if foreign {
        notify_legacy(mode, &std::env::temp_dir().join(format!("pywalfox_socket_{}", rustix::process::getuid().as_raw())))?;
    }
    Ok(written)
}

fn notify_legacy(mode: &str, path: &Path) -> Result<()> {
    let socket = std::os::unix::net::UnixDatagram::unbound()?;
    socket.set_write_timeout(Some(super::process::TIMEOUT))?;
    match socket.connect(path) {
        Ok(()) => {
            socket.send(format!("theme:mode:{mode}").as_bytes())?;
            socket.send(b"action:update")?;
        }
        Err(error) if matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused) => {},
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

pub(super) fn undo(owner: &str, locations: &Locations) -> Result<Vec<PathBuf>> {
    #[cfg(any(target_os = "linux", test))]
    if settings_path(locations).exists() && settings(locations)?.owner == owner {
        return css::undo(locations);
    }
    #[cfg(not(any(target_os = "linux", test)))]
    let _ = (owner, locations);
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_browser_updates_use_the_private_socket_and_report_unexpected_failures() {
        let root = tempfile::tempdir().unwrap(); let path = root.path().join("socket");
        let socket = std::os::unix::net::UnixDatagram::bind(&path).unwrap();
        socket.set_read_timeout(Some(super::super::process::TIMEOUT)).unwrap();
        notify_legacy("light", &path).unwrap();
        let mut bytes = [0; 64];
        let count = socket.recv(&mut bytes).unwrap(); assert_eq!(&bytes[..count], b"theme:mode:light");
        let count = socket.recv(&mut bytes).unwrap(); assert_eq!(&bytes[..count], b"action:update");
        notify_legacy("dark", &root.path().join("not-running")).unwrap();
        assert!(notify_legacy("dark", &root.path().join("x".repeat(256))).is_err());
    }

    #[test]
    fn shared_browser_palettes_require_review_and_restore_the_original() {
        let root = tempfile::tempdir().unwrap();
        let locations = Locations { home: root.path().into(), config: root.path().join("config"), data: root.path().join("data"), cache: root.path().join("cache"), state: root.path().join("state") };
        let standard = locations.cache.join("wal/colors.json");
        std::fs::create_dir_all(standard.parent().unwrap()).unwrap();
        std::fs::write(&standard, "personal palette").unwrap();
        let output = root.path().join("browser.json"); std::fs::write(&output, "reviewed palette").unwrap();
        let mut ledger = Ledger::default();
        assert!(shared_palette("browser", &output, &locations, &mut ledger, false).is_err());
        assert_eq!(std::fs::read_to_string(&standard).unwrap(), "personal palette");
        assert!(ledger.is_empty());
        assert_eq!(shared_palette("browser", &output, &locations, &mut ledger, true).unwrap().as_slice(), std::slice::from_ref(&standard));
        assert_eq!(std::fs::read_to_string(&standard).unwrap(), "reviewed palette");
        assert!(ledger.take_back_checked("browser").errors.is_empty());
        assert_eq!(std::fs::read_to_string(standard).unwrap(), "personal palette");
    }

    #[test]
    fn browser_launch_arguments_are_checked_before_normal_cli_parsing() {
        assert!(launched(["manifest.json".into(), EXTENSION_ID.into()]));
        assert!(launched([EXTENSION_ID.into()]));
        assert!(!launched([
            "msg".into(),
            "theme".into(),
            EXTENSION_ID.into()
        ]));
    }

    #[test]
    fn missing_palette_entries_are_reported_instead_of_sending_partial_colors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("colors.json");
        std::fs::write(&path, "{}").unwrap();
        let settings = Settings {
            colors: path,
            mode: "dark".into(),
            styles: dir.path().into(),
            owner: "browser".into(),
            _lease: None,
        };
        assert!(colors(&settings).is_err());
    }

    #[test]
    fn sparse_browser_palettes_keep_empty_slots_and_numeric_order() {
        let dir = tempfile::tempdir().unwrap();let path = dir.path().join("colors.json");
        let palette:serde_json::Map<_,_> = COLOR_KEYS.iter().enumerate().map(|(index,key)|((*key).into(),if index % 2 == 0 { json!("#123456") } else { json!("") })).collect();
        std::fs::write(&path,json!({"wallpaper":"wallpaper.webp","colors":palette}).to_string()).unwrap();
        let settings = Settings { colors:path,mode:"dark".into(),styles:dir.path().into(),owner:"browser".into(),_lease:None };
        let result = colors(&settings).unwrap();
        assert_eq!(result["data"]["colors"].as_array().unwrap().len(),COLOR_KEYS.len());
        for index in 0..COLOR_KEYS.len() { assert_eq!(result["data"]["colors"][index],if index % 2 == 0 { "#123456" } else { "" }); }
    }

    #[test]
    fn named_palette_slots_accept_browser_color_strings_without_inventing_values() {
        let dir = tempfile::tempdir().unwrap();let path = dir.path().join("colors.json");
        let palette:serde_json::Map<_,_> = (0..COLOR_KEYS.len()).map(|index|(format!("slot{index:02}"),json!("transparent"))).collect();
        std::fs::write(&path,json!({"wallpaper":"wallpaper.webp","colors":palette}).to_string()).unwrap();
        let settings = Settings { colors:path,mode:"dark".into(),styles:dir.path().into(),owner:"browser".into(),_lease:None };
        let result = colors(&settings).unwrap();
        assert!(result["data"]["colors"].as_array().unwrap().iter().all(|color|color == "transparent"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn applying_a_browser_template_installs_an_owned_host_and_disable_restores_files() {
        let root = tempfile::tempdir().unwrap();
        let locations = Locations {
            home: root.path().into(),
            config: root.path().join("config"),
            data: root.path().join("data"),
            cache: root.path().join("cache"),
            state: root.path().join("state"),
        };
        let output = root.path().join("colors.json");
        let palette = mandara_theme::engine::default_palette(true);
        std::fs::write(
            &output,
            mandara_theme::templates::pywal_json(&palette, "wallpaper.webp"),
        )
        .unwrap();
        let mut ledger = Ledger::default();
        let files = apply(
            "browser",
            &output,
            root.path(),
            "dark",
            &locations,
            &mut ledger,
            false,
        )
        .unwrap();
        assert_eq!(files.len(), 2);
        let settings = settings(&locations).unwrap();
        assert_eq!(settings.mode, "dark");
        assert_eq!(
            colors(&settings).unwrap()["data"]["colors"]
                .as_array()
                .unwrap()
                .len(),
            16
        );
        let manifest = locations
            .home
            .join(".mozilla/native-messaging-hosts/pywalfox.json");
        assert!(
            std::fs::read_to_string(&manifest)
                .unwrap()
                .contains(DESCRIPTION)
        );
        ledger.take_back("browser");
        assert!(!manifest.exists());
        assert!(!settings_path(&locations).exists());
    }
}
