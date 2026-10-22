//! Plays a saved deck against many differently-seeded simulated leagues to shake out
//! simulator crashes: `cargo run --release --example sim_fuzz -- <data dir> <deck file> [seeds] [games] [--more-sessions]`.
//! The first pass plays the league the app uses for that data folder (rules.json or the upcoming
//! session, with its imported pool and pins); `--more-sessions` adds pure simulations of
//! Sessions 3 and 5 (other deck rules).
use msh_deckbuilder::analysis::Rng;
use msh_deckbuilder::db::*;
use msh_deckbuilder::deck::Deck;
use msh_deckbuilder::playtest::{self, Prepared};

fn main() {
    let all: Vec<String> = std::env::args().collect();
    let more = all.iter().any(|a| a == "--more-sessions");
    let args: Vec<&String> = all.iter().filter(|a| !a.starts_with("--")).collect();
    let dir = std::path::PathBuf::from(args[1]);
    let seeds: u64 = args.get(3).and_then(|g| g.parse().ok()).unwrap_or(20);
    let games: u64 = args.get(4).and_then(|g| g.parse().ok()).unwrap_or(500);
    let db = CardDb::load(&dir.join("cards.json")).unwrap();
    let deck = Deck::load(std::path::Path::new(args[2]), &db).unwrap();
    let me = Prepared::new(&deck, &db);
    let app = msh_deckbuilder::league::app_rules(&dir, None);
    let mut leagues = vec![app];
    if more {
        for session in [3u32, 5] {
            // Pure simulations: the imported pool belongs to the app's session only.
            let mut r = Rules::default();
            let _ = msh_deckbuilder::league::apply_session(&mut r, session);
            leagues.push(r);
        }
    }
    for rules in leagues {
        let session = rules.session;
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
