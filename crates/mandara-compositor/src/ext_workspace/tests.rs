use super::*;

#[test]
fn constructing_a_driver_never_connects_and_only_serves_its_compositors() {
    let display = Some(std::ffi::OsStr::new("/nonexistent/wayland-fixture"));
    for kind in [CompositorKind::Labwc, CompositorKind::Cosmic] {
        let driver = ExtWorkspaceDriver::for_display(kind, display).unwrap();
        assert_eq!(driver.kind(), kind);
        assert!(driver.control.lock().unwrap().is_none(), "no thread before first use");
        assert!(ExtWorkspaceDriver::for_display(kind, None).is_none());
        assert!(ExtWorkspaceDriver::for_display(kind, Some(std::ffi::OsStr::new(""))).is_none());
    }
    assert!(ExtWorkspaceDriver::for_display(CompositorKind::Sway, display).is_none());
    assert_eq!(ExtWorkspaceDriver::for_display(CompositorKind::Cosmic, display).unwrap().thread_name(), "cosmic-workspaces");
}

#[test]
fn unsupported_requests_fail_at_once_without_a_thread() {
    let driver = ExtWorkspaceDriver::for_display(CompositorKind::Labwc, Some(std::ffi::OsStr::new("/nonexistent/wayland-fixture"))).unwrap();
    assert!(driver.cycle_layout("DP-1").is_err());
    assert!(driver.dispatch_raw("overview", &[]).is_err());
    assert!(driver.switch_keyboard_layout().is_err());
    assert!(driver.reload_config().is_ok());
    let config = mandara_config::compositor::CompositorConfig::default();
    assert!(driver.sync_managed_config(&config, None).is_err(), "no drop-in for labwc");
    // labwc's state is in: its windows can't be moved.
    driver.shared.live().ready = true;
    assert!(driver.move_client_to(None, 2).is_err());
    assert!(driver.toggle_client_tag(Some(4), 2).is_err());
    assert!(driver.control.lock().unwrap().is_none(), "nothing was queued");
}

#[test]
fn the_reconnect_pause_ends_early_when_the_driver_is_gone() {
    let (reader, writer) = std::io::pipe().unwrap();
    let start = std::time::Instant::now();
    assert!(pause(&reader, Duration::from_millis(20)), "a plain timeout keeps going");
    assert!(start.elapsed() >= Duration::from_millis(20));
    drop(writer);
    let start = std::time::Instant::now();
    assert!(!pause(&reader, Duration::from_secs(30)));
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_wake_byte_does_not_end_the_pause() {
    let (reader, mut writer) = std::io::pipe().unwrap();
    writer.write_all(&[1]).unwrap();
    assert!(pause(&reader, Duration::from_millis(20)));
}

/// Runs against a real labwc or cosmic-comp started in an isolated,
/// headless session (its own `XDG_RUNTIME_DIR` with the marker file),
/// never the desktop's own: `MANDARA_EXT_WORKSPACE_TEST=labwc|cosmic`.
#[test]
#[ignore = "requires an isolated headless labwc or cosmic-comp and its marker"]
fn real_compositor_lists_switches_and_tracks_windows() {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap();
    assert!(runtime.starts_with("/tmp") || runtime.starts_with("/var/tmp"), "not the desktop's runtime directory");
    assert_eq!(std::fs::read_to_string(runtime.join(".mandara-ext-workspace-test")).unwrap().trim(), "mandara isolated ext-workspace test");
    let kind = match std::env::var("MANDARA_EXT_WORKSPACE_TEST").as_deref() {
        Ok("cosmic") => CompositorKind::Cosmic,
        _ => CompositorKind::Labwc,
    };
    let driver = ExtWorkspaceDriver::from_env(kind).unwrap();
    let nav = driver.get_navigation_state().unwrap();
    eprintln!("navigation: {nav:#?}");
    assert!(!nav.is_empty(), "an output with workspaces");
    let events = driver.subscribe_events();
    let (monitor, model) = nav[0].clone();
    let NavigationModel::Workspaces { workspaces } = &model else { panic!("workspaces expected") };
    assert!(workspaces.len() >= 2, "the fixture config has several workspaces: {workspaces:?}");
    let target = workspaces.iter().find(|w| !w.active).unwrap().id;
    driver.select_primary(&monitor, target).unwrap();
    // Events arrive on the compositor's schedule: wait for them, bounded.
    let settled = |done: &dyn Fn() -> bool| {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !done() && std::time::Instant::now() < deadline {
            while events.try_recv().is_ok() {}
            std::thread::sleep(Duration::from_millis(20));
        }
        done()
    };
    assert!(settled(&|| driver.get_navigation_state().unwrap().iter().any(|(m, n)| *m == monitor && n.contains_active(&[target]))), "workspace {target} became active on {monitor}");
    if let Ok(app) = std::env::var("MANDARA_EXT_WORKSPACE_WINDOW") {
        let placed = || driver.get_all_clients().unwrap().iter().any(|c| c.app_id == app && !c.monitor.is_empty() && (kind == CompositorKind::Labwc || !c.tags.is_empty()));
        assert!(settled(&placed), "the fixture window is listed with its output (and, on COSMIC, its workspace): {:#?}", driver.get_all_clients());
        if kind == CompositorKind::Cosmic {
            let window = driver.get_all_clients().unwrap().into_iter().find(|c| c.app_id == app).unwrap();
            let home = window.tags[0];
            let other = workspaces.iter().map(|w| w.id).find(|id| *id != home).unwrap();
            // The workspace object at that position now: COSMIC renumbers
            // its dynamic workspaces once the window's old one empties.
            let key = |live: &Live| {
                let output = live.state.outputs.iter().find(|(_, o)| o.name == window.monitor).map(|(id, _)| *id).unwrap();
                live.state.workspaces_on(output)[usize::try_from(other).unwrap() - 1].0
            };
            let target = key(&driver.shared.live());
            // Moved to another workspace through COSMIC's manager.
            driver.move_client_to(Some(window.id), other).unwrap();
            let id = u32::try_from(window.id).unwrap();
            assert!(settled(&|| driver.shared.live().state.windows.get(&id).is_some_and(|w| w.workspaces == [target])), "the window moved to workspace {other}");
        }
    }
    eprintln!("clients: {:#?}", driver.get_all_clients().unwrap());
    eprintln!("monitors: {:#?}", driver.get_monitors().unwrap());
}
