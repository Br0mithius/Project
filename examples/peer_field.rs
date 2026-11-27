//! Ranks every commander's strong build (`table::strong_build`) against a reference deck:
//! each plays in our seat against three copies of the reference. 25% = as strong as it.
//! `cargo run --release --example peer_field -- <data dir> <reference deck> [games] [out dir]`
//! With an out dir, the top builds are saved there as deck files.
use msh_deckbuilder::db::CardDb;
use msh_deckbuilder::deck::Deck;
use msh_deckbuilder::playtest::Prepared;
use msh_deckbuilder::playtest::lab::play_all;
use std::sync::atomic::{AtomicBool, AtomicUsize};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(&a[0]);
    let mut db = CardDb::load(&dir.join("cards.json")).expect("cards.json");
    // The measured card values and ratings, as the app loads them.
    db.attach_ratings(&dir);
    let rules = msh_deckbuilder::league::app_rules(&dir, None);
    let reference = Deck::load(std::path::Path::new(&a[1]), &db).expect("reference deck");
    let games: u32 = a.get(2).and_then(|g| g.parse().ok()).unwrap_or(300);
    let mut decks: Vec<Deck> = Vec::new();
    for c in db.cards.iter().filter(|c| c.can_be_commander()) {
        if let Ok(d) = msh_deckbuilder::table::strong_build(&c.name, &db, &rules) {
            if d.main_count() >= d.main_target(&rules) {
                decks.push(d);
            }
        }
    }
    eprintln!("{} commanders built", decks.len());
    let prepared: Vec<Prepared> = decks.iter().map(|d| Prepared::new(d, &db)).collect();
    let refs: Vec<&Prepared> = prepared.iter().collect();
    let gauntlet = vec![Prepared::new(&reference, &db)];
    let o = play_all(
        &refs,
        &gauntlet,
        &rules,
        games,
        0x9EE2,
        &AtomicUsize::new(0),
        &AtomicBool::new(false),
    );
    let mut order: Vec<usize> = (0..decks.len()).collect();
    order.sort_by(|&x, &y| o.win_rate(y).partial_cmp(&o.win_rate(x)).unwrap());
    for (rank, &i) in order.iter().enumerate().take(40) {
        println!(
            "{:5.1}%  {}",
            o.win_rate(i) * 100.0,
            decks[i].commander.clone().unwrap_or_default()
        );
        if let Some(out) = a.get(3) {
            if rank < 12 {
                let _ = std::fs::create_dir_all(out);
                let path = std::path::Path::new(out).join(format!("peer{:02}.txt", rank + 1));
                let _ = std::fs::write(path, decks[i].to_text());
            }
        }
    }
}
