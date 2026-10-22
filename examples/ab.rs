//! TEMPORARY paired A/B runner: our deck (a deck file) in a rotating seat vs three league
//! decks. `ab <data dir> <deck.txt> <games> <flags> [--base F] [--threads N] [--start S]
//! [--global]`. Arm A: our seat plays with the `--base` switches on (everything else off).
//! Arm B: also `flags` on. Opponents have every switch off (with --global: like our seat).
use msh_deckbuilder::analysis::Rng;
use msh_deckbuilder::db::*;
use msh_deckbuilder::deck::Deck;
use msh_deckbuilder::playtest::exp;
use msh_deckbuilder::playtest::{self, Prepared};

fn main() {
    let all: Vec<String> = std::env::args().collect();
    let opt = |name: &str| {
        all.iter()
            .position(|a| a == name)
            .and_then(|i| all.get(i + 1))
            .cloned()
    };
    let threads: usize = opt("--threads").and_then(|s| s.parse().ok()).unwrap_or(3);
    let start: u64 = opt("--start").and_then(|s| s.parse().ok()).unwrap_or(5000);
    let base = exp::parse(&opt("--base").unwrap_or_default());
    let global = all.iter().any(|a| a == "--global");
    let mut args: Vec<&String> = Vec::new();
    let mut skip = false;
    for a in all.iter() {
        if skip {
            skip = false;
            continue;
        }
        if ["--threads", "--start", "--base"].contains(&a.as_str()) {
            skip = true;
            continue;
        }
        if !a.starts_with("--") {
            args.push(a);
        }
    }
    let dir = std::path::PathBuf::from(args[1]);
    let games: u64 = args[3].parse().unwrap();
    let flags = exp::parse(args.get(4).map(|s| s.as_str()).unwrap_or(""));
    let mut db = CardDb::load(&dir.join("cards.json")).unwrap();
    db.attach_ratings(&dir);
    let mut rules = Rules::default();
    rules.data_dir = Some(dir.clone());
    let deck = Deck::load(std::path::Path::new(args[2]), &db).expect("deck file");
    let me = Prepared::new(&deck, &db);
    let gauntlet = playtest::gauntlet(&db, &rules, 2026);
    let n = gauntlet.len();
    let run_arm = |b: bool| -> Vec<(bool, u32)> {
        let chunk = games.div_ceil(threads as u64);
        let mut outs = Vec::new();
        std::thread::scope(|s| {
            let hs: Vec<_> = (0..threads as u64)
                .map(|t| {
                    let rules = &rules;
                    let me = &me;
                    let gauntlet = &gauntlet;
                    s.spawn(move || {
                        let mut v = Vec::new();
                        for k in t * chunk..((t + 1) * chunk).min(games) {
                            let mut rng = Rng::new(start * 7919 + k);
                            let mut picks: Vec<usize> = (1..=n).collect();
                            rng.shuffle(&mut picks);
                            let seat = k as usize % 4;
                            let mut table: Vec<&Prepared> =
                                picks[..3].iter().map(|&d| &gauntlet[d - 1]).collect();
                            table.insert(seat, me);
                            let on = if b { flags | base } else { base };
                            let everything = exp::parse("all");
                            let mut off = [everything; 8];
                            for (s, o) in off.iter_mut().enumerate() {
                                if global || s == seat {
                                    *o = everything & !on;
                                }
                            }
                            exp::set_off(off);
                            let mut g =
                                playtest::state::Game::new(&table, rules, Rng::new(start + k));
                            let w = playtest::play(&mut g);
                            v.push((w == seat, g.round));
                        }
                        v
                    })
                })
                .collect();
            for h in hs {
                outs.extend(h.join().unwrap());
            }
        });
        outs
    };
    let a = run_arm(false);
    let bb = run_arm(true);
    let wa = a.iter().filter(|x| x.0).count() as f64;
    let wb = bb.iter().filter(|x| x.0).count() as f64;
    let ng = a.len() as f64;
    let d: Vec<f64> = a
        .iter()
        .zip(bb.iter())
        .map(|(x, y)| (y.0 as i32 - x.0 as i32) as f64)
        .collect();
    let mean = d.iter().sum::<f64>() / ng;
    let var = d.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (ng - 1.0);
    let se = (var / ng).sqrt();
    let ra = a.iter().map(|x| x.1 as f64).sum::<f64>() / ng;
    let rb = bb.iter().map(|x| x.1 as f64).sum::<f64>() / ng;
    println!(
        "flags={} base={} deck={} games={}{} | A {:.2}% B {:.2}% | diff {:+.2} +- {:.2} pp (z {:.1}) | rounds {:.2}/{:.2}",
        args.get(4).map(|s| s.as_str()).unwrap_or(""),
        opt("--base").unwrap_or_default(),
        std::path::Path::new(args[2]).file_name().unwrap().to_string_lossy(),
        games,
        if global { " global" } else { "" },
        100.0 * wa / ng,
        100.0 * wb / ng,
        100.0 * mean,
        100.0 * se,
        mean / se.max(1e-9),
        ra,
        rb
    );
}
