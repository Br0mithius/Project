//! Times building the playtest opponents (the ranking's TestBed) at a few sessions, and
//! prints one opponent's simulated season:
//! `cargo run --release --example testbed_timing -- <data dir> [player number] [session]`.
//! The first pass simulates; the second reads the cache in `<data dir>`.
use msh_deckbuilder::{db::*, league, opponents, ranking};
use std::time::Instant;

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("data dir"));
    let who: usize = std::env::args()
        .nth(2)
        .and_then(|a| a.parse().ok())
        .unwrap_or(1);
    let session: u32 = std::env::args()
        .nth(3)
        .and_then(|a| a.parse().ok())
        .unwrap_or(league::upcoming_session().number);
    let db = CardDb::load(&dir.join("cards.json")).unwrap();
    for pass in ["simulated", "cached"] {
        if pass == "simulated" {
            opponents::refresh(&dir);
        }
        for s in [1, 2, 5, 7, 13] {
            let mut rules = Rules::default();
            rules.data_dir = Some(dir.clone());
            league::apply_session(&mut rules, s).unwrap();
            let t = Instant::now();
            let bed = ranking::TestBed::new(&db, &rules);
            println!(
                "{} session {}: TestBed {} decks in {:?}",
                pass,
                s,
                bed.gauntlet.len(),
                t.elapsed()
            );
        }
    }
    let mut rules = Rules::default();
    rules.data_dir = Some(dir.clone());
    league::apply_session(&mut rules, session).unwrap();
    let s = opponents::season(&db, &rules, ranking::GAUNTLET_SEEDS[0]);
    let one = msh_deckbuilder::season::Season {
        opponents: vec![s.opponents[who.saturating_sub(1)].clone()],
        ..(*s).clone()
    };
    print!("{}", opponents::report(&db, &one, true));
}
