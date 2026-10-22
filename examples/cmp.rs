//! TEMPORARY paired deck comparison: deck A vs deck B in a rotating seat against three
//! league decks, same seeds. `cmp <data dir> <a.txt> <b.txt> <games> [--threads N]
//! [--start S] [--off flags]` (`--off`: switches turned off for every seat).
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
    let threads: usize = opt("--threads").and_then(|s| s.parse().ok()).unwrap_or(4);
    let start: u64 = opt("--start").and_then(|s| s.parse().ok()).unwrap_or(5000);
    let off = exp::parse(&opt("--off").unwrap_or_default());
    let mut args: Vec<&String> = Vec::new();
    let mut skip = false;
    for a in all.iter() {
        if skip {
            skip = false;
            continue;
        }
        if ["--threads", "--start", "--off"].contains(&a.as_str()) {
            skip = true;
            continue;
        }
        if !a.starts_with("--") {
            args.push(a);
        }
    }
    let dir = std::path::PathBuf::from(args[1]);
    let games: u64 = args[4].parse().unwrap();
    let mut db = CardDb::load(&dir.join("cards.json")).unwrap();
    db.attach_ratings(&dir);
    let mut rules = Rules::default();
    rules.data_dir = Some(dir.clone());
    let load = |p: &str| Prepared::new(&Deck::load(std::path::Path::new(p), &db).expect("deck"), &db);
    let da = load(args[2]);
    let dbb = load(args[3]);
    let gauntlet = playtest::gauntlet(&db, &rules, 2026);
    let n = gauntlet.len();
    let run_arm = |me: &Prepared| -> Vec<bool> {
        let chunk = games.div_ceil(threads as u64);
        let mut outs = Vec::new();
        std::thread::scope(|s| {
            let hs: Vec<_> = (0..threads as u64)
                .map(|t| {
                    let rules = &rules;
                    let gauntlet = &gauntlet;
                    s.spawn(move || {
                        exp::set_off([off; 8]);
                        let mut v = Vec::new();
                        for k in t * chunk..((t + 1) * chunk).min(games) {
                            let mut rng = Rng::new(start * 7919 + k);
                            let mut picks: Vec<usize> = (1..=n).collect();
                            rng.shuffle(&mut picks);
                            let seat = k as usize % 4;
                            let mut table: Vec<&Prepared> =
                                picks[..3].iter().map(|&d| &gauntlet[d - 1]).collect();
                            table.insert(seat, me);
                            let mut g =
                                playtest::state::Game::new(&table, rules, Rng::new(start + k));
                            v.push(playtest::play(&mut g) == seat);
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
    let a = run_arm(&da);
    let b = run_arm(&dbb);
    let ng = a.len() as f64;
    let d: Vec<f64> = a
        .iter()
        .zip(b.iter())
        .map(|(x, y)| (*y as i32 - *x as i32) as f64)
        .collect();
    let mean = d.iter().sum::<f64>() / ng;
    let var = d.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (ng - 1.0);
    let se = (var / ng).sqrt();
    let name = |p: &str| {
        std::path::Path::new(p)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string()
    };
    println!(
        "{} vs {} off={} games={} | A {:.2}% B {:.2}% | B-A {:+.2} +- {:.2} pp (z {:.1})",
        name(args[2]),
        name(args[3]),
        opt("--off").unwrap_or_default(),
        games,
        100.0 * a.iter().filter(|x| **x).count() as f64 / ng,
        100.0 * b.iter().filter(|x| **x).count() as f64 / ng,
        100.0 * mean,
        100.0 * se,
        mean / se.max(1e-9)
    );
}
