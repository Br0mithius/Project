//! Plays several saved decks on the same games (same opponents, seats and shuffles) and
//! prints each one's win rate and its paired difference from the first:
//! `cargo run --release --example deck_compare -- <data dir> <games> [--focus K] <deck file>...`
//! `--focus K`: every opponent weighs our seat K times as a target (like `lab focus`, K = 2.5).
use msh_deckbuilder::db::CardDb;
use msh_deckbuilder::deck::Deck;
use msh_deckbuilder::playtest::{self, Prepared};
use std::sync::atomic::{AtomicBool, AtomicUsize};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(&args[1]);
    let games: u32 = args[2].parse().expect("games");
    let mut rules = msh_deckbuilder::league::app_rules(&dir, None);
    let mut files = Vec::new();
    let mut k = 3;
    while k < args.len() {
        if args[k] == "--focus" {
            rules.focus_us = args[k + 1].parse().expect("focus");
            k += 2;
        } else {
            files.push(args[k].clone());
            k += 1;
        }
    }
    let mut db = CardDb::load(&dir.join("cards.json")).unwrap();
    db.attach_ratings(&dir);
    let decks: Vec<Prepared> = files
        .iter()
        .map(|f| Prepared::new(&Deck::load(std::path::Path::new(f), &db).unwrap(), &db))
        .collect();
    let gauntlet = playtest::gauntlet(&db, &rules, 2026);
    let refs: Vec<&Prepared> = decks.iter().collect();
    let o = playtest::lab::play_all(
        &refs,
        &gauntlet,
        &rules,
        games,
        20261029,
        &AtomicUsize::new(0),
        &AtomicBool::new(false),
    );
    println!(
        "{} games each, same games for every deck, focus {}",
        games, rules.focus_us
    );
    for (i, f) in files.iter().enumerate() {
        let d = o.diff(i, 0);
        println!(
            "{:>6.2}%  {:+.2} ± {:.2} vs the first  {}",
            o.win_rate(i) * 100.0,
            d.gain * 100.0,
            d.stderr * 100.0,
            f
        );
    }
}
