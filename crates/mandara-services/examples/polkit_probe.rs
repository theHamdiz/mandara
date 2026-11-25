//! Registers Mandara's polkit agent for this session, prints the result and
//! exits (polkitd drops the agent with the connection). Run twice at once to
//! see the "another agent" path:
//! `cargo run -p mandara-services --example polkit_probe`.

fn main() {
    let agent = mandara_services::polkit::spawn_polkit_agent(true);
    match agent.events.recv_blocking() {
        Ok(event) => println!("{event:?}"),
        Err(_) => println!("agent thread ended"),
    }
    std::thread::sleep(std::time::Duration::from_millis(1500));
}
