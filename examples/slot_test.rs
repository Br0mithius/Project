//! Paired test of cards in one slot: the deck as it is against the deck with `slot` swapped
//! for each candidate, on the same games. Against the peer field and the data folder's next
//! table, weighted as the optimiser does.
//!
//! `cargo run --release --example slot_test -- <data dir> <deck file> <slot card> <games>
//! <candidate>...`
use msh_deckbuilder::db::CardDb;
use msh_deckbuilder::deck::Deck;
use msh_deckbuilder::lab::Progress;
use msh_deckbuilder::optimize::{DEFAULT_TABLE_WEIGHT, Opponents, compare};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(&a[0]);
    let mut db = CardDb::load(&dir.join("cards.json")).expect("cards.json");
    db.sim_values = msh_deckbuilder::playtest::simvalues::SimValues::load(&dir);
    let deck = Deck::load(std::path::Path::new(&a[1]), &db).expect("deck");
    let slot = &a[2];
    let games: u32 = a[3].parse().expect("games");
    let cands = &a[4..];
    let rules = msh_deckbuilder::league::app_rules(&dir, None);
    let table = msh_deckbuilder::table::Table::load(&dir);
    let opp = Opponents {
        table: msh_deckbuilder::table::gauntlet(&table, &db),
        league: msh_deckbuilder::table::peer_field(&db, &rules),
        table_weight: DEFAULT_TABLE_WEIGHT,
    };
    assert!(deck.count_of(slot) > 0, "{} is not in the deck", slot);
    let decks: Vec<Deck> = cands
        .iter()
        .map(|c| {
            assert!(db.get(c).is_some(), "unknown card {}", c);
            let mut d = deck.clone();
            d.remove(slot, 1);
            d.add(c, 1);
            d
        })
        .collect();
    let r = compare(&deck, &decks, &db, &opp, &rules, games, 77_123, &Progress::new());
    println!("in place of {} ({} games each; points of win rate, + = better than {}):", slot, games, slot);
    for (c, g) in cands.iter().zip(&r) {
        println!("{:<34} {:+.2} ± {:.2}", c, g.score.gain, g.score.stderr);
    }
}
