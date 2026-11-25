//! Read-only display diagnostics: outputs (wlr-output-management), gamma
//! support, DDC/CI monitors. `--gamma-test` additionally warms every screen
//! to 5500 K for two seconds and restores it.
//!
//! `cargo run -p mandara-services --example display_probe [-- --gamma-test]`

use std::time::Duration;

use mandara_services::night_light::GammaTarget;
use mandara_services::{ddc, gamma, outputs};

/// A mild test temperature and how long it shows.
const TEST_KELVIN: u32 = 5500;
const TEST_HOLD: Duration = Duration::from_secs(2);
/// Fade in/out of the test.
const TEST_FADE: Duration = Duration::from_millis(300);
/// Time for the gamma thread to bind outputs and report sizes.
const SETTLE: Duration = Duration::from_millis(400);

fn main() {
    match outputs::heads() {
        Ok(heads) => {
            for h in &heads {
                let m = h.current().map(|m| format!("{}x{}@{:.3}", m.width, m.height, m.refresh_mhz as f64 / 1000.0)).unwrap_or_default();
                println!("{} [{}] enabled={} mode={m} pos={},{} scale={} transform={} vrr={:?} modes={} id={:?}", h.name, h.description, h.enabled, h.x, h.y, h.scale, h.transform.name(), h.adaptive_sync, h.modes.len(), h.identity());
            }
        }
        Err(e) => println!("output management: {e}"),
    }
    println!("gamma control: {:?}", gamma::probe());
    let access = ddc::access();
    println!("ddc access: {access:?}");
    if access == ddc::Access::Ready {
        for m in ddc::monitors() {
            println!("ddc {} on /dev/i2c-{}: {}/{} ({}%)", m.connector, m.bus, m.brightness, m.max, m.percent());
        }
    }
    if std::env::args().any(|a| a == "--gamma-test") {
        let g = gamma::Gamma::spawn();
        std::thread::sleep(SETTLE);
        g.set(GammaTarget { temperature: TEST_KELVIN, ..GammaTarget::NEUTRAL }, TEST_FADE);
        std::thread::sleep(TEST_HOLD);
        println!("gamma status while tinted: {:?}", g.status());
        g.set(GammaTarget::NEUTRAL, TEST_FADE);
        std::thread::sleep(TEST_FADE + SETTLE);
        println!("gamma status after restore: {:?}", g.status());
    }
}
