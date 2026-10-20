//! Lists what the simulator's opponent decks contain by card type:
//! `cargo run --release --example sim_decks -- <data dir>`.
use msh_deckbuilder::db::*;
use msh_deckbuilder::playtest::{self, compile::*};

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("data dir"));
    let db = CardDb::load(&dir.join("cards.json")).unwrap();
    let rules = Rules::default();
    for (k, d) in playtest::gauntlet(&db, &rules, 2026).iter().enumerate() {
        let n = |f: &dyn Fn(&SimCard) -> bool| d.cards.iter().filter(|c| f(c)).count();
        let instants: Vec<&str> = d
            .cards
            .iter()
            .filter(|c| c.is(T_INSTANT) || c.kw & FLASH != 0)
            .map(|c| c.name.as_str())
            .collect();
        println!(
            "deck {}: {} distinct cards, {} creatures, {} lands; instant speed: {}",
            k,
            d.cards.len(),
            n(&|c| c.is(T_CREATURE)),
            n(&|c| c.is(T_LAND)),
            instants.join(", ")
        );
    }
}
