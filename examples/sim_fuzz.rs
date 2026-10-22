//! Plays a saved deck against many differently-seeded simulated leagues to shake out
//! simulator crashes: `cargo run --release --example sim_fuzz -- <data dir> <deck file> [seeds] [games]`.
use msh_deckbuilder::analysis::Rng;
use msh_deckbuilder::db::*;
use msh_deckbuilder::deck::Deck;
use msh_deckbuilder::playtest::{self, Prepared};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(&args[1]);
    let seeds: u64 = args.get(3).and_then(|g| g.parse().ok()).unwrap_or(20);
    let games: u64 = args.get(4).and_then(|g| g.parse().ok()).unwrap_or(500);
    let db = CardDb::load(&dir.join("cards.json")).unwrap();
    let deck = Deck::load(std::path::Path::new(&args[2]), &db).unwrap();
    let me = Prepared::new(&deck, &db);
    for session in [2u32, 3, 5] {
        let rules = Rules {
            session,
            ..Rules::default()
        };
        for s in 0..seeds {
            let gauntlet = playtest::gauntlet(&db, &rules, 7000 + s);
            let n = gauntlet.len();
            for k in 0..games {
                let mut table: Vec<&Prepared> =
                    (0..3).map(|i| &gauntlet[(k as usize + i) % n]).collect();
                table.insert(k as usize % 4, &me);
                let seed = s * 100_000 + k;
                eprintln!("session {} league seed {} game {}", session, 7000 + s, seed);
                let mut g = playtest::state::Game::new(&table, &rules, Rng::new(seed));
                playtest::play(&mut g);
            }
        }
        println!("session {} done", session);
    }
}
