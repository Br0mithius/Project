//! Times the game simulator: `cargo run --release --example sim_bench -- <data dir> [games]`.
use msh_deckbuilder::{db::*, playtest, ranking};
use std::time::Instant;

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("data dir"));
    let games: u32 = std::env::args()
        .nth(2)
        .and_then(|g| g.parse().ok())
        .unwrap_or(200);
    let db = CardDb::load(&dir.join("cards.json")).unwrap();
    let coll = Collection::load(&dir.join("collection.txt")).unwrap();
    let rules = Rules::default();
    let g = playtest::gauntlet(&db, &rules, 2026);
    let opts = ranking::candidates(&db, &coll, &rules);
    let (name, piper) = &opts[0];
    let cmd = db.get(name).unwrap();
    let e = ranking::evaluate(cmd, *piper, &db, &coll, &rules, &ranking::TestBed::none());
    let t = Instant::now();
    let wr = playtest::win_rate(&e.deck, &g, &db, &rules, games, 7);
    let ms = t.elapsed().as_secs_f64() * 1000.0 / games as f64;
    println!(
        "{} games: {:.2} ms/game, win rate {:.3} ({})",
        games,
        ms,
        wr,
        e.label()
    );
}
