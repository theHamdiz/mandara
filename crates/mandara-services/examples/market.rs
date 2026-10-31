//! Dev tool: lists marketplace plugins. `cargo run -p mandara-services --example market [query]`
fn main() {
    let q = std::env::args().nth(1).unwrap_or_default();
    match mandara_services::marketplace::search(&q) {
        Ok(found) => {
            println!("{} of {} plugin(s) for {:?}", found.items.len(), found.total, q);
            for l in found.items.iter().take(5) {
                println!("  {} ({} stars) — {}", l.full_name, l.stargazers_count, l.description.clone().unwrap_or_default());
            }
        }
        Err(e) => eprintln!("error: {e}"),
    }
}
