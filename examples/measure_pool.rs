//! Measures every spell of a deck and every owned spell outside it (each played in the
//! deck) on paired games against the real table (30%) and the peer field (70%) - precise values for the cards a pool's
//! auto-build chooses between. Rows are added to `<out dir>/sim_card_values.tsv`.
//!
//! `cargo run --release --example measure_pool -- <data dir> <deck file> <games> <out dir> [label]`
use msh_deckbuilder::card::Card;
use msh_deckbuilder::db::{CardDb, Collection};
use msh_deckbuilder::deck::Deck;
use msh_deckbuilder::playtest::Prepared;
use msh_deckbuilder::playtest::simvalues::{self, SimValues};
use std::sync::atomic::{AtomicBool, AtomicUsize};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(&a[0]);
    let games: u32 = a[2].parse().expect("games");
    let out = std::path::PathBuf::from(&a[3]);
    std::fs::create_dir_all(&out).unwrap();
    let mut db = CardDb::load(&dir.join("cards.json")).expect("cards.json");
    db.attach_ratings(&dir);
    let rules = msh_deckbuilder::league::app_rules(&dir, None);
    let deck = Deck::load(std::path::Path::new(&a[1]), &db).expect("deck");
    let coll = Collection::load(&dir.join("collection.txt")).expect("collection");
    let id = deck.identity(&db).expect("identity");
    let extra: Vec<&Card> = db
        .cards
        .iter()
        .filter(|c| {
            !c.is_land()
                && coll.owned(&c.name) > 0
                && deck.count_of(&c.name) == 0
                && deck.commander.as_deref() != Some(c.name.as_str())
                && c.identity().is_subset_of(id)
        })
        .collect();
    let cmd = deck.commander.clone().unwrap_or_default();
    let label = a.get(4).cloned().unwrap_or_else(|| "pool".into());
    let name = format!("{} ({}, {} games)", cmd, label, games);
    eprintln!(
        "{} deck spells and {} pool cards",
        deck.cards.len(),
        extra.len()
    );
    // The optimiser's objective: the real table 30%, the peer field 70% (each measured on
    // its own games, combined per card).
    let w = msh_deckbuilder::optimize::DEFAULT_TABLE_WEIGHT;
    let table = msh_deckbuilder::table::Table::load(&dir);
    let p = Prepared::new(&deck, &db);
    let (done, cancel) = (AtomicUsize::new(0), AtomicBool::new(false));
    let run = |g: &[Prepared], seed: u64| {
        simvalues::measure(&name, &p, &extra, g, &rules, games, seed, &done, &cancel)
    };
    let peer = run(&msh_deckbuilder::table::peer_field(&db, &rules), 0x9001);
    let tab = msh_deckbuilder::table::gauntlet(&table, &db);
    let ms: Vec<simvalues::Measurement> = if tab.is_empty() {
        peer
    } else {
        let t = run(&tab, 0x9002);
        peer.into_iter()
            .map(|mut m| {
                if let Some(x) = t.iter().find(|x| x.card == m.card) {
                    m.value = w * x.value + (1.0 - w) * m.value;
                    m.se = ((w * x.se).powi(2) + ((1.0 - w) * m.se).powi(2)).sqrt();
                    m.games += x.games;
                }
                m
            })
            .collect()
    };
    println!("{} measurements", ms.len());
    let mut table = SimValues::load_own(&out);
    table.add(ms);
    table.save(&out).expect("save");
}
