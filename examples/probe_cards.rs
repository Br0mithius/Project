//! Fires every ability of each card on a standard board and lists what the simulator
//! actually does, to check against the rules text:
//! `cargo run --release --example probe_cards -- <data dir> <file with one card name per line>`
use msh_deckbuilder::db::CardDb;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(&args[1]);
    let db = CardDb::load(&dir.join("cards.json")).unwrap();
    let names = std::fs::read_to_string(&args[2]).unwrap();
    for name in names.lines().map(str::trim).filter(|l| !l.is_empty()) {
        match db.get(name) {
            Some(c) => println!("### {}\n", msh_deckbuilder::playtest::probe::probe(c)),
            None => println!("### {} - NOT IN DATABASE\n", name),
        }
    }
}
