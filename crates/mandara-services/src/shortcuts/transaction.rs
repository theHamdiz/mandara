//! Keep native fields, managed bindings and live reload in the same undo boundary.

use anyhow::{Result, ensure};
use mandara_config::{
    compositor::{self, CompositorConfig, ManagedConfig},
    doctor::theme::{self, BatchValidator, FileEdit, LiveCheck},
    keybinds::{self, PreparedEdit},
};
use mandara_i18n::t;
use mandara_theme::templates::WriteLock;
use std::path::Path;

use super::{Preferences, Session};

pub(super) trait Backend {
    fn validate(&self, path: &Path, body: &str) -> Result<()>;
    fn check(&self) -> LiveCheck;
    fn apply(&self) -> Result<()>;
    fn restore(&self) -> Result<()>;
    fn version(&self) -> Option<(u32, u32)>;
}

pub(super) struct DocumentGuard<'a> {
    pub session: &'a Session,
    pub destination: &'a Path,
    pub backend: &'a dyn Backend,
    pub initial: &'a Preferences,
    pub preferences: &'a dyn Fn() -> Result<Preferences>,
}

impl DocumentGuard<'_> {
    pub fn fresh(&self) -> Result<()> {
        ensure!(
            (self.preferences)()? == *self.initial,
            "{}",
            t("Mandara's shortcuts changed while saving. Refresh the list and try again.")
        );
        for source in &self.session.scan.sources {
            if source.target != self.destination {
                ensure!(
                    source.unchanged()?,
                    "{}",
                    t(
                        "The compositor shortcuts changed while they were being checked. Refresh the list and try again."
                    )
                );
            }
        }
        Ok(())
    }
}

impl BatchValidator for DocumentGuard<'_> {
    fn validate(&self, path: &Path, body: &str) -> Result<()> {
        self.backend.validate(path, body)
    }
    fn before(&self) -> LiveCheck {
        self.backend.check()
    }
    fn after(&self) -> LiveCheck {
        self.backend.check()
    }
    fn apply_live(&self) -> Result<()> {
        self.fresh()?;
        self.backend.apply()
    }
    fn restore_live(&self) -> Result<()> {
        self.backend.restore()
    }
}

pub(super) fn apply(
    session: &Session,
    edit: &PreparedEdit,
    initial: &Preferences,
    preferences: &dyn Fn() -> Result<Preferences>,
    backend: &dyn Backend,
) -> Result<()> {
    let generated = companion(session)?.unwrap_or_default();
    let companion = (!generated.as_os_str().is_empty()).then_some(generated.as_path());
    let (native, generated_lock) = WriteLock::pair(&edit.id.file, companion)?;
    let dialect = generated_lock
        .as_ref()
        .map_or(generated.as_path(), |lock| lock.target());
    ensure!(
        native.target() == edit.id.target,
        "{}",
        t(
            "The compositor shortcuts changed while they were being checked. Refresh the list and try again."
        )
    );
    let previous = companion.map(keybinds::read_source).transpose()?;
    let mut config = CompositorConfig {
        autostart: false,
        keybinds: initial.keybinds.clone(),
        ..Default::default()
    };
    // With management off, changing a native shortcut must not enable another
    // shell shortcut that was absent from the existing generated file.
    if !initial.manage
        && let Some(previous) = &previous
    {
        let enabled = compositor::parse_chords(session.target, previous);
        config
            .keybinds
            .retain(|bind| enabled.contains(&compositor::target_chord(session.target, bind)));
    }
    let mut managed = ManagedConfig {
        config: &config,
        borders: None,
        existing_chords: Default::default(),
        mandara_bin: mandara_compositor::backend::mandara_binary(),
        version: backend.version(),
        user_autostarts: true,
    };
    managed.existing_chords.extend(
        config
            .keybinds
            .iter()
            .map(|bind| compositor::target_chord(session.target, bind)),
    );
    let bridge = previous
        .as_deref()
        .map(|body| compositor::shortcuts::replace(session.target, dialect, body, &managed))
        .transpose()?;
    let guard = DocumentGuard {
        session,
        destination: &edit.id.target,
        backend,
        initial,
        preferences,
    };
    guard.fresh()?;
    let native_edit = FileEdit {
        lock: &native,
        before: &edit.original,
        after: edit.body(),
        intermediate: None,
    };
    let mut edits = vec![native_edit];
    if let (Some(lock), Some(before), Some(bridge)) = (&generated_lock, &previous, &bridge) {
        edits.push(FileEdit {
            lock,
            before,
            after: bridge,
            intermediate: Some(bridge),
        });
    }
    let finalize = |index: usize| {
        if index == 0 {
            return Ok(None);
        }
        guard.fresh()?;
        let mut current =
            keybinds::scan_roots_in(session.target, [session.root.clone()], &session.environment);
        if let Some(modifiers) = &session.scan.niri {
            keybinds::resolve_niri_mod(&mut current, modifiers.nested_session);
        }
        ensure!(
            current.errors.is_empty(),
            "{}",
            t("Resolve the shortcut configuration errors before saving.")
        );
        let mut managed = managed.clone();
        managed.existing_chords = if session.target == compositor::Target::Niri {
            Default::default()
        } else {
            compositor::existing_chords_checked_in(
                session.target,
                &session.root,
                &session.environment,
            )?
        };
        // TOML and self-managed dialects also expose literal alternatives through
        // the field parser; unresolved expressions remain with the native checker.
        managed.existing_chords.extend(
            current
                .sources
                .iter()
                .flat_map(|source| &source.bindings)
                .filter_map(|row| row.chord.clone()),
        );
        previous
            .as_deref()
            .map(|body| compositor::shortcuts::replace(session.target, dialect, body, &managed))
            .transpose()
    };
    crate::outputs::reload_preserving(&[], true, crate::outputs::current_revision(), || {
        theme::guarded_batch_finalizing(
            &edits,
            &guard,
            companion.map(|_| &finalize as &dyn Fn(usize) -> Result<Option<String>>),
        )
        .map_err(|error| format!("{error:#}"))
    })
    .map_err(anyhow::Error::msg)
}

fn companion(session: &Session) -> Result<Option<std::path::PathBuf>> {
    if !mandara_config::compositor_theme::supported(session.target) {
        return Ok(None);
    }
    let expected = session.target.generated_file();
    let mut found = None;
    for alias in &session.scan.generated {
        let file = std::fs::canonicalize(alias)?;
        let managed = |path: &Path| {
            let name = path.file_name();
            compositor::is_generated(path)
                && (name == expected.file_name()
                    || session.target == compositor::Target::Hyprland
                        && name
                            .is_some_and(|name| name == "hyprland.lua" || name == "hyprland.conf"))
        };
        if managed(alias) || managed(&file) {
            ensure!(
                session
                    .scan
                    .sources
                    .iter()
                    .all(|source| source.target != file),
                "{}",
                t("Resolve the shortcut configuration errors before saving.")
            );
            ensure!(
                found.as_ref().is_none_or(|(_, previous)| *previous == file),
                "{}",
                t(
                    "More than one managed shortcut file is included. Run Config doctor before editing shortcuts."
                )
            );
            // Lock the include's original path, so retargeting any symlink in
            // that path invalidates the transaction as well as its destination.
            if found.is_none() {
                found = Some((alias.clone(), file));
            }
        }
    }
    Ok(found.map(|(alias, _)| alias))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::compositor::Target;
    use std::{fs, os::unix::fs::symlink};

    struct UnusedBackend;
    impl Backend for UnusedBackend {
        fn validate(&self, _: &Path, _: &str) -> Result<()> {
            panic!("validation must follow bounded reads")
        }
        fn check(&self) -> LiveCheck {
            panic!("no live check before bounded reads")
        }
        fn apply(&self) -> Result<()> {
            panic!("no live write before bounded reads")
        }
        fn restore(&self) -> Result<()> {
            panic!("no live write before bounded reads")
        }
        fn version(&self) -> Option<(u32, u32)> {
            panic!("version lookup must follow bounded reads")
        }
    }

    #[test]
    fn logical_companion_aliases_keep_their_role_without_claiming_native_sources() {
        let directory = tempfile::tempdir().unwrap();
        let generated = directory.path().join("mandara/generated");
        fs::create_dir_all(&generated).unwrap();
        let file = directory.path().join("outside.conf");
        fs::write(&file, "").unwrap();
        let alias = generated.join("hyprland.conf");
        symlink(&file, &alias).unwrap();
        let mut session = Session {
            target: Target::Hyprland,
            root: directory.path().join("config.conf"),
            scan: Default::default(),
            environment: Default::default(),
            primary_mod_is_alt: false,
        };
        session.scan.generated.push(alias.clone());
        assert_eq!(companion(&session).unwrap(), Some(alias));
        session.scan.sources.push(keybinds::Source {
            file: file.clone(),
            target: file,
            fingerprint: 0,
            bindings: Vec::new(),
        });
        assert!(companion(&session).is_err());
        let native = directory.path().join("hyprland.conf");
        fs::write(&native, "").unwrap();
        let colors = generated.join("app-theme-hyprland.conf");
        symlink(&native, &colors).unwrap();
        session.scan.generated = vec![colors];
        assert!(companion(&session).unwrap().is_none());
    }

    #[test]
    fn oversized_and_fifo_companions_fail_before_native_or_live_mutation() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("config.conf");
        let original = "bind=SUPER,Q,killclient\n";
        fs::write(&file, original).unwrap();
        let directory = root.path().join("mandara/generated");
        fs::create_dir_all(&directory).unwrap();
        let companion = directory.join("mango.conf");
        let mut session = Session {
            target: Target::Mango,
            root: file.clone(),
            scan: keybinds::scan_from(Target::Mango, &file),
            environment: Default::default(),
            primary_mod_is_alt: false,
        };
        session.scan.generated.push(companion.clone());
        let source = &session.scan.sources[0];
        let mut values: Vec<_> = source.bindings[0]
            .fields
            .iter()
            .map(|field| field.value.clone())
            .collect();
        values[1] = "W".into();
        let edit =
            keybinds::prepare(Target::Mango, &source.binding_id(0).unwrap(), &values).unwrap();
        let initial = Preferences {
            target: Target::Mango,
            manage: false,
            keybinds: Vec::new(),
        };
        fs::write(&companion, vec![b'x'; keybinds::MAX_FILE_BYTES + 1]).unwrap();
        assert!(
            apply(
                &session,
                &edit,
                &initial,
                &|| Ok(initial.clone()),
                &UnusedBackend
            )
            .is_err()
        );
        assert_eq!(fs::read_to_string(&file).unwrap(), original);
        fs::remove_file(&companion).unwrap();
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            &companion,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )
        .unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            send.send(
                apply(
                    &session,
                    &edit,
                    &initial,
                    &|| Ok(initial.clone()),
                    &UnusedBackend,
                )
                .is_err(),
            )
            .unwrap();
        });
        assert!(
            receive
                .recv_timeout(mandara_config::doctor::CHECK_TIMEOUT)
                .unwrap()
        );
        worker.join().unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), original);
    }

    #[test]
    fn the_companion_is_the_actual_included_file_including_aliases_and_lua() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("mandara/generated");
        fs::create_dir_all(&directory).unwrap();
        let file = directory.join("hyprland.lua");
        let colors = directory.join("app-theme-hyprland.lua");
        let alias = root.path().join("settings");
        fs::write(&file, "").unwrap();
        fs::write(&colors, "").unwrap();
        symlink(&file, &alias).unwrap();
        let mut session = Session {
            environment: Default::default(),
            primary_mod_is_alt: false,
            target: Target::Hyprland,
            root: root.path().join("custom.lua"),
            scan: Default::default(),
        };
        session.scan.generated = vec![colors.clone(), alias.clone(), file.clone()];
        assert_eq!(companion(&session).unwrap(), Some(alias.clone()));
        let selected = companion(&session).unwrap().unwrap();
        let lock = WriteLock::new(&selected).unwrap();
        fs::remove_file(&alias).unwrap();
        symlink(&colors, &alias).unwrap();
        assert!(lock.check_target().is_err());
        drop(lock);
        let other = directory.join("hyprland.conf");
        fs::write(&other, "").unwrap();
        session.scan.generated.push(other);
        assert!(companion(&session).is_err());
        session.scan.generated = vec![alias];
        session.target = Target::Cosmic;
        assert!(companion(&session).unwrap().is_none());
    }
}
