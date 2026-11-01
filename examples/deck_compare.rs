//! Plays several saved decks on the same games (same opponents, seats and shuffles) and
//! prints each one's win rate and its paired difference from the first:
//! `cargo run --release --example deck_compare -- <data dir> <games> [--focus K] [--gseed N]
//! [--seed N] <deck file>...`
//! `--focus K`: every opponent weighs our seat K times as a target (like `lab focus`, K = 2.5).
//! `--gseed N`: the simulated season the opponents come from (default 2026); `--seed N`: the
//! games' seed (default 20261029).
//!
//! A deck with the first one's commanders and size is played as the first deck with its
//! different cards swapped into the same library slots, so each game deals it the same
//! cards in the same places and only the swapped cards change; a deck loaded on its own
//! (the slots shift) shuffles differently, which roughly quadruples the noise of the
//! difference. Decks that can't be swapped that way are loaded on their own (marked).
use msh_deckbuilder::db::CardDb;
use msh_deckbuilder::deck::Deck;
use msh_deckbuilder::playtest::compile::compile;
use msh_deckbuilder::playtest::{self, Prepared};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize};

/// `deck` as `base` with swaps (same commanders and size), or None.
fn swapped(base: &Deck, base_prep: &Prepared, deck: &Deck, db: &CardDb) -> Option<Prepared> {
    if base.commanders() != deck.commanders() {
        return None;
    }
    let canon = |d: &Deck| -> Option<BTreeMap<String, i64>> {
        let mut m = BTreeMap::new();
        for (n, &k) in &d.cards {
            *m.entry(db.get(n)?.name.clone()).or_insert(0) += k as i64;
        }
        Some(m)
    };
    let (a, b) = (canon(base)?, canon(deck)?);
    let mut out: Vec<&String> = Vec::new();
    let mut add: Vec<&String> = Vec::new();
    for n in a.keys().chain(b.keys()) {
        let d = b.get(n).copied().unwrap_or(0) - a.get(n).copied().unwrap_or(0);
        if d < 0 && !out.contains(&n) {
            out.extend(std::iter::repeat_n(n, (-d) as usize));
        } else if d > 0 && !add.contains(&n) {
            add.extend(std::iter::repeat_n(n, d as usize));
        }
    }
    if out.len() != add.len() {
        return None;
    }
    let swaps: Vec<(u16, _)> = out
        .iter()
        .zip(&add)
        .map(|(o, n)| Some((base_prep.card_index(o)?, compile(db.get(n)?))))
        .collect::<Option<_>>()?;
    Some(base_prep.with_swaps(&swaps))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(&args[1]);
    let games: u32 = args[2].parse().expect("games");
    let mut rules = msh_deckbuilder::league::app_rules(&dir, None);
    let (mut gseed, mut seed) = (2026u64, 20261029u64);
    let mut files = Vec::new();
    let mut k = 3;
    while k < args.len() {
        match args[k].as_str() {
            "--focus" => rules.focus_us = args[k + 1].parse().expect("focus"),
            "--gseed" => gseed = args[k + 1].parse().expect("gseed"),
            "--seed" => seed = args[k + 1].parse().expect("seed"),
            _ => {
                files.push(args[k].clone());
                k += 1;
                continue;
            }
        }
        k += 2;
    }
    let mut db = CardDb::load(&dir.join("cards.json")).unwrap();
    db.attach_ratings(&dir);
    let loaded: Vec<Deck> = files
        .iter()
        .map(|f| Deck::load(std::path::Path::new(f), &db).unwrap())
        .collect();
    let base = Prepared::new(&loaded[0], &db);
    let mut paired = vec![true];
    let mut decks = vec![base.with_swaps(&[])];
    for d in &loaded[1..] {
        match swapped(&loaded[0], &base, d, &db) {
            Some(p) => decks.push(p),
            None => {
                paired.push(false);
                decks.push(Prepared::new(d, &db));
                continue;
            }
        }
        paired.push(true);
    }
    let gauntlet = playtest::gauntlet(&db, &rules, gseed);
    let refs: Vec<&Prepared> = decks.iter().collect();
    let o = playtest::lab::play_all(
        &refs,
        &gauntlet,
        &rules,
        games,
        seed,
        &AtomicUsize::new(0),
        &AtomicBool::new(false),
    );
    println!(
        "{} games each, same games for every deck, focus {}, season {}, seed {}",
        games, rules.focus_us, gseed, seed
    );
    for (i, f) in files.iter().enumerate() {
        let d = o.diff(i, 0);
        println!(
            "{:>6.2}%  {:+.2} ± {:.2} vs the first  {}{}",
            o.win_rate(i) * 100.0,
            d.gain * 100.0,
            d.stderr * 100.0,
            f,
            if paired[i] {
                ""
            } else {
                "  (loaded on its own: not slot-paired)"
            }
        );
    }
}
