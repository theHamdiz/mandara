//! Opt-in checks against private Niri and Hyprland sessions configured with us,ara.

use std::path::Path;
use std::time::{Duration, Instant};
use mandara_compositor::{CompositorBackend, CompositorEvent};

/// A stopped compositor or missing layout must fail rather than hang the test.
const WAIT: Duration = Duration::from_secs(10);
/// Event subscriptions are nonblocking; this yield is only for the native test.
const POLL: Duration = Duration::from_millis(10);

fn next_layout(rx: &async_channel::Receiver<CompositorEvent>) -> String {
    let deadline = Instant::now() + WAIT;
    loop {
        match rx.try_recv() {
            Ok(CompositorEvent::KeyboardLayout(layout)) => return layout,
            Ok(_) => {},
            Err(async_channel::TryRecvError::Closed) => panic!("compositor event stream closed"),
            Err(async_channel::TryRecvError::Empty) => {},
        }
        assert!(Instant::now() < deadline, "no keyboard-layout event arrived");
        std::thread::sleep(POLL);
    }
}

#[test]
#[ignore = "requires a private compositor with us,ara and MANDARA_KEYBOARD_TEST_RUNTIME"]
fn native_layout_switches_reach_the_shell_without_device_permissions() {
    let root = std::env::var("MANDARA_KEYBOARD_TEST_RUNTIME").expect("explicit private runtime required");
    let root = Path::new(&root);
    assert!(root.starts_with("/tmp") || root.starts_with("/var/tmp"));
    assert_eq!(std::fs::read_to_string(root.join(".mandara-keyboard-test")).unwrap(), "private keyboard layout test\n");
    assert_eq!(std::env::var_os("XDG_RUNTIME_DIR").as_deref(), Some(root.as_os_str()));
    assert_eq!(std::env::var("DBUS_SYSTEM_BUS_ADDRESS").unwrap(), "unix:path=/nonexistent");
    let driver: Box<dyn CompositorBackend> = match std::env::var("MANDARA_KEYBOARD_TEST_COMPOSITOR").unwrap().as_str() {
        "niri" => {
            let socket = std::env::var("NIRI_SOCKET").unwrap();
            assert!(Path::new(&socket).starts_with(root));
            Box::new(mandara_compositor::niri::NiriDriver::from_env().unwrap())
        },
        "hyprland" => {
            let signature = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").unwrap();
            assert!(!signature.is_empty() && !signature.contains('/') && signature != "..");
            Box::new(mandara_compositor::hyprland::HyprlandDriver::from_env().unwrap())
        },
        other => panic!("unsupported native fixture {other}"),
    };
    let events = driver.subscribe_events();
    assert_eq!(next_layout(&events), "English (US)");
    driver.switch_keyboard_layout().unwrap();
    assert_eq!(next_layout(&events), "Arabic");
    driver.switch_keyboard_layout().unwrap();
    assert_eq!(next_layout(&events), "English (US)");
}
