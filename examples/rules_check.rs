//! Parses a rules.json and prints the error, if any, and the rules read:
//! `cargo run --release --example rules_check -- <rules.json>`
fn main() {
    let p = std::env::args().nth(1).expect("rules.json");
    let text = std::fs::read_to_string(&p).expect("read");
    match serde_json::from_str::<msh_deckbuilder::db::Rules>(&text) {
        Ok(r) => println!(
            "ok: deck_size {} singleton {} session {}",
            r.deck_size, r.singleton, r.session
        ),
        Err(e) => println!("parse error: {}", e),
    }
}
