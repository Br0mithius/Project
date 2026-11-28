//! Lists every card with a "whenever ... cast(s) a spell that targets ..." trigger and the
//! target filter the engine compiled for it. A filter of 0 means the trigger fires on any
//! spell of the right type (the Storm, Windrider bug): `cast_target_scan <data dir>`.
use msh_deckbuilder::db::CardDb;
use msh_deckbuilder::playtest::compile::*;

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("data dir"));
    let db = CardDb::load(&dir.join("cards.json")).unwrap();
    let mut bad = 0;
    for c in &db.cards {
        let text = c.oracle_text.to_lowercase();
        if !text.contains("spell that targets") {
            continue;
        }
        let sc = compile(c);
        let filters: Vec<u8> = sc
            .abilities
            .iter()
            .filter_map(|a| match &a.trig {
                Trig::Event(Ev::Cast, f) => Some(f.targets),
                _ => None,
            })
            .collect();
        let ok = !filters.is_empty() && filters.iter().any(|&t| t != 0);
        if !ok {
            bad += 1;
        }
        println!(
            "{} {:<32} filter bits {:?} (1 own creature, 2 opposing creature, 4 opponent, 8 artifact/land, 16 or Equipment)",
            if ok { "ok " } else { "BAD" },
            c.name,
            filters
        );
    }
    println!("{} without a target filter", bad);
}
