//! How the simulator handles named cards: `card_cov <data dir> "<card>" ...`.
use msh_deckbuilder::db::CardDb;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let db = CardDb::load(&std::path::Path::new(&a[0]).join("cards.json")).unwrap();
    for n in &a[1..] {
        match db.get(n) {
            Some(c) => println!(
                "{}: {:?}",
                c.name,
                msh_deckbuilder::playtest::coverage::card_coverage(c)
            ),
            None => println!("{}: not found", n),
        }
    }
}
