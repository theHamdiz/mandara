//! Dev tool: presses a key chord through `zwp_virtual_keyboard_v1`, e.g.
//! `cargo run -p mandara-services --example vkey -- ctrl+shift+w`, to test
//! compositor keybinds end to end. An optional second argument keeps the
//! modifiers held that many milliseconds after the key tap
//! (`vkey alt+tab 2000` = hold Alt, tap Tab, release Alt 2 s later).

fn main() -> Result<(), String> {
    let spec = std::env::args().nth(1).ok_or("usage: vkey MOD+MOD+KEY [HOLD_MS]")?;
    let hold_ms: u64 = std::env::args().nth(2).map(|s| s.parse().map_err(|e| format!("HOLD_MS: {e}"))).transpose()?.unwrap_or(0);
    let parts: Vec<&str> = spec.split('+').collect();
    let (key, mods) = parts.split_last().ok_or("empty chord")?;
    let code = mandara_services::clipboard::keyboard::key_code(key).ok_or(format!("unknown key {key:?}"))?;
    mandara_services::clipboard::keyboard::chord_held(mods, code, std::time::Duration::from_millis(hold_ms))
}
