//! Prints the log of a simulated game, or counts what happens over many:
//! `cargo run --release --example sim_log -- <data dir> [seed] [games]`.
use msh_deckbuilder::analysis::Rng;
use msh_deckbuilder::playtest::{self, Prepared};
use msh_deckbuilder::{db::*, ranking};
use std::collections::HashMap;

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("data dir"));
    let seed: u64 = std::env::args()
        .nth(2)
        .and_then(|g| g.parse().ok())
        .unwrap_or(1);
    let games: u64 = std::env::args()
        .nth(3)
        .and_then(|g| g.parse().ok())
        .unwrap_or(1);
    let db = CardDb::load(&dir.join("cards.json")).unwrap();
    let coll = Collection::load(&dir.join("collection.txt")).unwrap();
    let rules = Rules::default();
    let gauntlet = playtest::gauntlet(&db, &rules, 2026);
    let opts = ranking::candidates(&db, &coll, &rules);
    let (name, piper) = &opts[0];
    let cmd = db.get(name).unwrap();
    let e = ranking::evaluate(cmd, *piper, &db, &coll, &rules, &ranking::TestBed::none());
    let me = Prepared::new(&e.deck, &db);
    let mut counts: HashMap<String, u32> = HashMap::new();
    let mut rounds = 0;
    for k in 0..games {
        let n = gauntlet.len();
        let table: Vec<&Prepared> = vec![
            &me,
            &gauntlet[k as usize % n],
            &gauntlet[(k as usize + 1) % n],
            &gauntlet[(k as usize + 2) % n],
        ];
        let mut g = playtest::state::Game::new(&table, &rules, Rng::new(seed + k));
        g.log = Some(Vec::new());
        let winner = playtest::play(&mut g);
        rounds += g.round;
        let log = g.log.take().unwrap_or_default();
        if games == 1 {
            for line in &log {
                println!("{}", line);
            }
            println!(
                "winner: seat {} after {} rounds; life {:?}",
                if winner == playtest::NO_WINNER {
                    "none (a draw)".to_string()
                } else {
                    winner.to_string()
                },
                g.round,
                g.players.iter().map(|p| p.life).collect::<Vec<_>>()
            );
        }
        for line in log {
            // Lines start with the round ("R5 "): not part of the kind of event.
            let line = match line.split_once(' ') {
                Some((r, rest)) if r.starts_with('R') => rest.trim_start().to_string(),
                _ => line,
            };
            // Group by kind of event (strip card-specific detail for AI decisions).
            let key: String = if line.starts_with("AI ") {
                line.split(':')
                    .next()
                    .unwrap_or(&line)
                    .replace(|c: char| c.is_ascii_digit(), "#")
            } else {
                line.chars().take(110).collect()
            };
            *counts.entry(key).or_default() += 1;
        }
    }
    if games > 1 {
        let mut v: Vec<_> = counts.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        for (line, n) in v.iter().take(40) {
            println!("{:>6.1}/game  {}", *n as f64 / games as f64, line);
        }
        println!("{:.1} rounds per game", rounds as f64 / games as f64);
    }
}
