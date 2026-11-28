//! Measures every spell of a deck and every owned spell outside it (each played in the
//! deck), as the app's "measure" does (`simvalues::PoolJob`): paired games against the
//! real table and the peer field (half each). Rows are added to
//! `<out dir>/sim_card_values.tsv`.
//!
//! `cargo run --release --example measure_pool -- <data dir> <deck file> <games> <out dir>`
use msh_deckbuilder::db::{CardDb, Collection};
use msh_deckbuilder::deck::Deck;
use msh_deckbuilder::playtest::simvalues::{PoolJob, SimValues};
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
    let job = PoolJob::new(&deck, &db, &coll, &rules, games, false);
    eprintln!("{} pool cards, {} deck-games", job.extra.len(), job.size());
    let ms = job.run(&rules, &AtomicUsize::new(0), &AtomicBool::new(false));
    println!("{} measurements", ms.len());
    let mut table = SimValues::load_own(&out);
    table.add(ms);
    table.save(&out).expect("save");
}
