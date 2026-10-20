//! Times the parts of a commander ranking: `cargo run --release --example rank_timing -- <data dir>`.
use msh_deckbuilder::{db::*, playtest, ranking};
use std::time::Instant;

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("data dir"));
    let db = CardDb::load(&dir.join("cards.json")).unwrap();
    let coll = Collection::load(&dir.join("collection.txt")).unwrap();
    let rules = Rules::default();
    let t = Instant::now();
    let g = playtest::gauntlet(&db, &rules, 2026);
    println!("gauntlet: {} decks in {:?}", g.len(), t.elapsed());
    let opts = ranking::candidates(&db, &coll, &rules);
    println!("{} options", opts.len());
    let t = Instant::now();
    let (name, piper) = &opts[0];
    let cmd = db.get(name).unwrap();
    let e = ranking::evaluate(cmd, *piper, &db, &coll, &rules, &ranking::TestBed::none());
    println!("one evaluate without playtest: {:?}", t.elapsed());
    let t = Instant::now();
    let wr = playtest::win_rate(&e.deck, &g, &db, &rules, 160, 7);
    println!("160 games: {:?} (win rate {:.2})", t.elapsed(), wr);
    for seed in 1..=5 {
        print!(
            "{:.2} ",
            playtest::win_rate(&e.deck, &g, &db, &rules, 160, seed)
        );
    }
    println!(" <- same deck, different seeds");
    // Sanity: a pile of basics should almost never win; a test deck vs. its peers ~25%.
    let mut lands = msh_deckbuilder::deck::Deck::new("lands");
    lands.commander = e.deck.commander.clone();
    lands.add("Mountain", 30);
    lands.add("Swamp", 29);
    println!(
        "all basics: {:.3}",
        playtest::win_rate(&lands, &g, &db, &rules, 1000, 3)
    );
    let peers = playtest::gauntlet(&db, &rules, 99);
    let auto = msh_deckbuilder::analysis::autobuild(cmd, &db, &coll, &rules, None, None).0;
    println!(
        "{} vs gauntlet: {:.3}",
        name,
        playtest::win_rate(&auto, &peers, &db, &rules, 1000, 3)
    );
}
