//! Native probes get a private home, display and dead system/session buses.

use anyhow::{Context as _, Result, ensure};
use mandara_config::{
    doctor::{self, CheckEnv, CheckerStatus},
    keybinds::{Binding, Encoding},
};
use mandara_i18n::t;
use std::{
    os::unix::{
        fs::{FileTypeExt as _, PermissionsExt as _},
        process::CommandExt as _,
    },
    path::Path,
    process::{Child, Command, Stdio},
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

/// Poll only while waiting for a short-lived native probe to create its socket.
const SOCKET_POLL: Duration = Duration::from_millis(20);

struct Probe(tempfile::TempDir);
impl Probe {
    fn new() -> Result<Self> {
        let root = tempfile::Builder::new()
            .prefix("mdr-keys-")
            .tempdir_in("/tmp")?;
        for directory in ["config", "runtime", "data", "cache", "state"] {
            std::fs::create_dir(root.path().join(directory))?;
        }
        std::fs::set_permissions(
            root.path().join("runtime"),
            std::fs::Permissions::from_mode(0o700),
        )?;
        Ok(Self(root))
    }
    fn path(&self) -> &Path {
        self.0.path()
    }
    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.path())
            .env("XDG_RUNTIME_DIR", self.path().join("runtime"))
            .env("XDG_CONFIG_HOME", self.path().join("config"))
            .env("XDG_DATA_HOME", self.path().join("data"))
            .env("XDG_CACHE_HOME", self.path().join("cache"))
            .env("XDG_STATE_HOME", self.path().join("state"))
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                "unix:path=/nonexistent/mandara-shortcut-probe",
            )
            .env(
                "DBUS_SYSTEM_BUS_ADDRESS",
                "unix:path=/nonexistent/mandara-shortcut-probe",
            )
            .env("WLR_BACKENDS", "headless")
            .env("WLR_RENDERER", "pixman")
            .env("WLR_LIBINPUT_NO_DEVICES", "1")
            .env("LANG", "C.UTF-8");
        command
    }
    fn run(&self, command: Command) -> Result<(String, String)> {
        crate::app_themes::process::native(command, &AtomicBool::new(false), doctor::CHECK_TIMEOUT)
    }
}

struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        crate::hooks::kill_group(self.0.id(), libc::SIGKILL);
        let _ = self.0.wait();
    }
}

fn with_river<T>(mode: &str, action: impl FnOnce(&Probe, &Path) -> Result<T>) -> Result<T> {
    let probe = Probe::new()?;
    let mut server = probe.command("river");
    let mut running = Running(
        server
            .args(["-c", "true"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()?,
    );
    let runtime = probe.path().join("runtime");
    let deadline = Instant::now() + doctor::CHECK_TIMEOUT;
    let display = loop {
        let found = std::fs::read_dir(&runtime)?.find_map(|entry| {
            let entry = entry.ok()?;
            (entry.file_name().to_string_lossy().starts_with("wayland-")
                && entry.file_type().ok()?.is_socket())
            .then(|| entry.path())
        });
        if let Some(display) = found {
            break display;
        }
        ensure!(
            running.0.try_wait()?.is_none() && Instant::now() < deadline,
            "{}",
            t("The private compositor shortcut checker could not start.")
        );
        std::thread::sleep(SOCKET_POLL);
    };
    let mut declare = probe.command("riverctl");
    declare
        .env("WAYLAND_DISPLAY", &display)
        .args(["declare-mode", mode]);
    probe.run(declare)?;
    action(&probe, &display)
}

pub(super) fn river(row: &Binding) -> Result<()> {
    with_river(&row.scope, |probe, display| {
        let mut map = probe.command("riverctl");
        map.env("WAYLAND_DISPLAY", &display)
            .args(super::river::words(row, false)?);
        let (output, diagnostic) = probe.run(map)?;
        ensure!(
            output.trim().is_empty() && diagnostic.trim().is_empty(),
            "{}: {output}{diagnostic}",
            t("The compositor rejected these shortcuts.")
        );
        Ok(())
    })
}

fn lua_sample(row: &Binding) -> Result<String> {
    ensure!(
        row.read_only.is_none() && row.fields.len() >= 2,
        "{}",
        t("Computed shortcuts must be edited in their source configuration.")
    );
    let fields = &row.fields;
    let chord = fields[0].encoding.encode(&fields[0].value)?;
    let action = fields[1].encoding.encode(&fields[1].value)?;
    let mut arguments = Vec::new();
    let mut options = Vec::new();
    for field in &fields[2..] {
        let value = field.encoding.encode(&field.value)?;
        if let Some(name) = &field.name {
            options.push(format!("{} = {value}", Encoding::Identifier.encode(name)?));
        } else {
            arguments.push(value);
        }
    }
    Ok(format!(
        "hl.bind({chord}, hl.dsp.{action}({}), {{ {} }})\n",
        arguments.join(", "),
        options.join(", ")
    ))
}

fn hyprland_sample(session: &super::Session, row: &Binding) -> Result<(&'static str, String)> {
    if row
        .fields
        .first()
        .is_some_and(|field| field.encoding == Encoding::LuaLiteral)
    {
        return Ok(("lua", lua_sample(row)?));
    }
    ensure!(
        row.read_only.is_none() && row.fields.len() >= 5,
        "{}",
        t("Computed shortcuts must be edited in their source configuration.")
    );
    let mut body = String::new();
    // Copy only literal variable declarations, never source/exec directives.
    // They retain the same include traversal order as the scanned bindings.
    for source in &session.scan.sources {
        let text = crate::app_themes::apply::read_text(&source.file)?;
        for line in text.lines() {
            let line = line.trim();
            if let Some((name, _)) = line.split_once('=')
                && let Some(name) = name.trim().strip_prefix('$')
                && !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            {
                body.push_str(line);
                body.push('\n');
            }
        }
    }
    if row.scope != "default" {
        body.push_str("submap = ");
        body.push_str(&Encoding::Word.encode(&row.scope)?);
        body.push('\n');
    }
    body.push_str(&Encoding::Word.encode(&row.fields[0].value)?);
    body.push_str(" = ");
    for (index, field) in row.fields[1..].iter().enumerate() {
        if index != 0 {
            body.push_str(", ");
        }
        body.push_str(&field.encoding.encode(&field.value)?);
    }
    body.push('\n');
    Ok(("conf", body))
}

pub(super) fn hyprland(session: &super::Session, row: &Binding) -> Result<()> {
    let probe = Probe::new()?;
    let (extension, body) = hyprland_sample(session, row)?;
    let path = probe.path().join(format!("config/hyprland.{extension}"));
    std::fs::write(&path, body)?;
    let report = doctor::check_with(
        mandara_config::compositor::Target::Hyprland,
        &path,
        &CheckEnv {
            home: Some(probe.path().into()),
            ..Default::default()
        },
    );
    ensure!(
        report.status == CheckerStatus::Ran && report.problems() == 0,
        "{}: {}",
        t("The compositor rejected these shortcuts."),
        report.raw
    );
    Ok(())
}

pub(super) fn labwc(body: &str) -> Result<()> {
    let probe = Probe::new()?;
    // Only the keyboard section reaches the private compositor. Startup hooks,
    // window rules and external entities never run during shortcut validation.
    let keyboard = mandara_config::keybinds::labwc_keyboard(body)?;
    std::fs::write(probe.path().join("config/rc.xml"), keyboard)?;
    let mut command = probe.command("labwc");
    command
        .arg("-C")
        .arg(probe.path().join("config"))
        .args(["-S", "true", "-V"]);
    let (_, diagnostic) = probe.run(command)?;
    ensure!(
        !diagnostic.contains("[ERROR]") && !diagnostic.contains("[WARN]"),
        "{}: {diagnostic}",
        t("The compositor rejected these shortcuts.")
    );
    Ok(())
}

pub(super) fn labwc_reload() -> Result<()> {
    ensure!(
        mandara_compositor::detect() == Some(mandara_compositor::CompositorKind::Labwc),
        "{}",
        t("The running compositor changed. Refresh the list and try again.")
    );
    let pid = mandara_compositor::display_server_pid()
        .context(t("The running compositor could not be identified safely."))?;
    let mut command = Command::new("labwc");
    command
        .arg("--reconfigure")
        .env("LABWC_PID", pid.to_string());
    crate::app_themes::process::native(command, &AtomicBool::new(false), doctor::CHECK_TIMEOUT)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::compositor::Target;

    #[test]
    #[ignore = "requires River and riverctl"]
    fn river_layout_pinned_save_and_undo_run_in_a_private_server() {
        let rows = mandara_config::keybinds::parse(Target::River, Path::new("init"), "riverctl map -release -layout 2 normal Super Q close\nriverctl map -release -layout 2 normal Super W close\n").unwrap();
        with_river("normal", |probe, display| {
            let run = |words: &[String]| {
                let mut command = probe.command("riverctl");
                command.env("WAYLAND_DISPLAY", display).args(words);
                let (output, diagnostic) = probe.run(command)?;
                ensure!(
                    output.trim().is_empty() && diagnostic.trim().is_empty(),
                    "{output}{diagnostic}"
                );
                Ok(())
            };
            run(&super::super::river::words(&rows[0], false)?)?;
            let edit = super::super::river::Edit::default();
            edit.apply_with(&rows[0], &rows[1], &run)?;
            edit.restore_with(&rows[0], &rows[1], &run)?;
            run(&super::super::river::words(&rows[0], true)?)?;
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn lua_probe_copies_literals_without_loading_imports_or_startup_code() {
        let source = "os.execute('touch /never-run')\nrequire('personal')\nhl.bind('SUPER + Q', hl.dsp.exec_cmd('foot'), { repeatable = true })\n";
        let rows =
            mandara_config::keybinds::parse(Target::Hyprland, Path::new("hyprland.lua"), source)
                .unwrap();
        assert_eq!(
            lua_sample(&rows[0]).unwrap(),
            "hl.bind('SUPER + Q', hl.dsp.exec_cmd('foot'), { repeatable = true })\n"
        );
    }

    #[test]
    fn legacy_hyprland_probe_copies_bindings_and_variables_without_sources_or_startup() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("custom.conf");
        let source = "$mainMod = SUPER\nsource = dangerous.lua\nexec-once = touch /never-run\nsubmap = resize\nbind = $mainMod, Q, exec, foot\n";
        std::fs::write(&file, source).unwrap();
        let rows = mandara_config::keybinds::parse(Target::Hyprland, &file, source).unwrap();
        let session = super::super::Session {
            environment: Default::default(),
            primary_mod_is_alt: false,
            target: Target::Hyprland,
            root: file.clone(),
            scan: mandara_config::keybinds::Scan {
                sources: vec![mandara_config::keybinds::Source {
                    file: file.clone(),
                    target: file,
                    fingerprint: 0,
                    bindings: Vec::new(),
                }],
                ..Default::default()
            },
        };
        let (extension, body) = hyprland_sample(&session, &rows[0]).unwrap();
        assert_eq!(extension, "conf");
        assert_eq!(
            body,
            "$mainMod = SUPER\nsubmap = resize\nbind = $mainMod, Q, exec, foot\n"
        );
    }
}
