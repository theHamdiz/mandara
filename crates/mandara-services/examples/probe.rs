//! Prints the first state snapshot of every service as JSON.
//! `cargo run -p mandara-services --example probe`

fn main() {
    let net = mandara_services::spawn_network_service();
    let bt = mandara_services::spawn_bluetooth_service();
    let audio = mandara_services::spawn_audio_service();
    let media = mandara_services::spawn_media_service();
    let n = net.updates.recv_blocking().unwrap();
    println!("network: {}", serde_json::to_string(&serde_json::json!({
        "connectivity": n.connectivity, "primary": n.primary, "wifi_enabled": n.wifi_enabled,
        "strength": n.strength, "access_points": n.access_points.len(),
    })).unwrap());
    let b = bt.updates.recv_blocking().unwrap();
    println!("bluetooth: powered={} devices={} connected={}", b.powered, b.devices.len(), b.connected_count());
    println!("audio: {}", serde_json::to_string(&audio.updates.recv_blocking().unwrap()).unwrap());
    match media.updates.recv_blocking().unwrap() {
        Some(m) => println!("media: {} — {} [{:?}] via {}", m.title, m.artist, m.status, m.identity),
        None => println!("media: no players"),
    }
}
