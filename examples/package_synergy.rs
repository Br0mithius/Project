//! Are some cards worth more together than apart? Paired playtests of a deck with parts of a
//! package replaced by neutral stand-ins (a colourless vanilla N/N for {N}, N = the card's
//! mana value), all on the same games:
//!
//! `cargo run --release --example package_synergy -- <data dir> <games> [--focus K]
//! [--seed N] [--opp <deck file>...] --deck <deck file> --pkg <card>...`
//!
//! prints each card's value alone (the rest of the package out), its value in the full
//! package (leave-one-out), the package's value, and the interaction: package value minus
//! the sum of the cards' values alone, with its standard error from the paired games.
//!
//! `... --deck <deck file> --pkg <card>... --with <card>...`
//!
//! instead plays the two groups in a 2x2 (both, each alone, neither): is the first group
//! worth more when the second is in the deck (payoffs and their enablers)?
//!
//! `... --deck <deck file> --tribe <Subtype> [--payoffs <card>...] [--steps 0,4,8,12]`
//!
//! sweeps how many of the deck's other creatures of that type keep the type (the rest play
//! as the same card without it, so only the tribal membership changes), with the payoffs
//! in and with them replaced by stand-ins: the payoffs' value at each tribe count, and how
//! much the tribe count is worth with and without them.
//!
//! Opponents: the simulated league (season 2026, like `deck_compare`), or the decks given
//! with `--opp` (a fixed table, like `pod_compare`).
use msh_deckbuilder::analysis::Rng;
use msh_deckbuilder::card::Card;
use msh_deckbuilder::db::CardDb;
use msh_deckbuilder::deck::Deck;
use msh_deckbuilder::playtest::compile::{SimCard, compile};
use msh_deckbuilder::playtest::lab::{Measure, play_all};
use msh_deckbuilder::playtest::simvalues::vanilla;
use msh_deckbuilder::playtest::{self, Prepared};
use std::sync::atomic::{AtomicBool, AtomicUsize};

fn pts(m: &Measure) -> String {
    format!("{:+6.2} ± {:.2}", m.gain * 100.0, m.stderr * 100.0)
}

/// The card without creature type `sub` (renamed, so the deck sees a different card).
fn untyped(c: &Card, sub: &str) -> Card {
    let mut out = c.clone();
    out.roles_cache = Default::default();
    out.text_cache = Default::default();
    out.parsed_cache = Default::default();
    out.name = format!("{} (no {})", c.name, sub);
    out.oracle_text = c.oracle_text.replace(&c.name, &out.name);
    let (head, tail) = match c.type_line.split_once('—') {
        Some((h, t)) => (h.trim().to_string(), t.to_string()),
        None => (c.type_line.clone(), String::new()),
    };
    let rest: Vec<&str> = tail
        .split_whitespace()
        .filter(|w| !w.eq_ignore_ascii_case(sub))
        .collect();
    out.type_line = if rest.is_empty() {
        head
    } else {
        format!("{} — {}", head, rest.join(" "))
    };
    out
}

fn stand_in(c: &Card) -> SimCard {
    compile(&vanilla(c.mana_value().max(1)))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(&args[1]);
    let games: u32 = args[2].parse().expect("games");
    let mut rules = msh_deckbuilder::league::app_rules(&dir, None);
    let mut seed = 20261029u64;
    let (mut deck_file, mut tribe) = (String::new(), String::new());
    let (mut opp, mut pkg, mut payoffs) = (Vec::new(), Vec::new(), Vec::new());
    let mut with: Vec<String> = Vec::new();
    let mut steps: Vec<usize> = Vec::new();
    let mut list: Option<&str> = None;
    let mut k = 3;
    while k < args.len() {
        let a = args[k].as_str();
        let next = || args.get(k + 1).cloned().unwrap_or_default();
        match a {
            "--focus" => rules.focus_us = next().parse().expect("focus"),
            "--seed" => seed = next().parse().expect("seed"),
            "--deck" => deck_file = next(),
            "--tribe" => tribe = next(),
            "--steps" => {
                steps = next()
                    .split(',')
                    .map(|s| s.trim().parse().expect("steps"))
                    .collect()
            }
            "--opp" | "--pkg" | "--payoffs" | "--with" => {
                list = Some(match a {
                    "--opp" => "opp",
                    "--pkg" => "pkg",
                    "--with" => "with",
                    _ => "payoffs",
                });
                k += 1;
                continue;
            }
            _ => {
                match list {
                    Some("opp") => opp.push(a.to_string()),
                    Some("pkg") => pkg.push(a.to_string()),
                    Some("payoffs") => payoffs.push(a.to_string()),
                    Some("with") => with.push(a.to_string()),
                    _ => panic!("unexpected argument {}", a),
                }
                k += 1;
                continue;
            }
        }
        list = None;
        k += 2;
    }
    let mut db = CardDb::load(&dir.join("cards.json")).unwrap();
    db.attach_ratings(&dir);
    let deck = Deck::load(std::path::Path::new(&deck_file), &db).unwrap();
    let base = Prepared::new(&deck, &db);
    let gauntlet: Vec<Prepared> = if opp.is_empty() {
        playtest::gauntlet(&db, &rules, 2026)
    } else {
        opp.iter()
            .map(|f| Prepared::new(&Deck::load(std::path::Path::new(f), &db).unwrap(), &db))
            .collect()
    };
    let card = |n: &str| -> &Card { db.get(n).unwrap_or_else(|| panic!("unknown card {}", n)) };
    let index = |n: &str| -> u16 {
        base.card_index(&card(n).name)
            .unwrap_or_else(|| panic!("{} is not in the deck", n))
    };
    let run = |decks: &[Prepared]| {
        let refs: Vec<&Prepared> = decks.iter().collect();
        play_all(
            &refs,
            &gauntlet,
            &rules,
            games,
            seed,
            &AtomicUsize::new(0),
            &AtomicBool::new(false),
        )
    };
    println!(
        "{} games, {} opponents, focus {}, seed {}, deck {}",
        games,
        if opp.is_empty() {
            "simulated league".to_string()
        } else {
            format!("{} fixed", opp.len())
        },
        rules.focus_us,
        seed,
        deck_file
    );

    if !pkg.is_empty() && !with.is_empty() {
        let group = |names: &[String]| -> Vec<(u16, SimCard)> {
            names
                .iter()
                .map(|n| (index(n), stand_in(card(n))))
                .collect()
        };
        let (a, b) = (group(&pkg), group(&with));
        let both: Vec<(u16, SimCard)> = a.iter().chain(b.iter()).cloned().collect();
        // 0 both groups, 1 without the first, 2 without the second, 3 without either.
        let decks = vec![
            base.with_swaps(&[]),
            base.with_swaps(&a),
            base.with_swaps(&b),
            base.with_swaps(&both),
        ];
        let o = run(&decks);
        for (i, label) in ["both", "second only", "first only", "neither"]
            .iter()
            .enumerate()
        {
            println!("{:>12}  {:.2}%", label, o.win_rate(i) * 100.0);
        }
        println!(
            "first group's value with the second    {}",
            pts(&o.diff(0, 1))
        );
        println!(
            "first group's value without the second {}",
            pts(&o.diff(2, 3))
        );
        println!(
            "second group's value with the first    {}",
            pts(&o.diff(0, 2))
        );
        println!(
            "second group's value without the first {}",
            pts(&o.diff(1, 3))
        );
        println!(
            "interaction (both - each - neither)    {}",
            pts(&o.contrast(&[(0, 1.0), (1, -1.0), (2, -1.0), (3, 1.0)]))
        );
    } else if !pkg.is_empty() {
        let n = pkg.len();
        let out = |skip: &[usize]| -> Prepared {
            let swaps: Vec<(u16, SimCard)> = (0..n)
                .filter(|i| skip.contains(i))
                .map(|i| (index(&pkg[i]), stand_in(card(&pkg[i]))))
                .collect();
            base.with_swaps(&swaps)
        };
        let all: Vec<usize> = (0..n).collect();
        // 0 full, 1 none, 2.. alone_i, then leave-one-out_i.
        let mut decks = vec![base.with_swaps(&[]), out(&all)];
        for i in 0..n {
            let others: Vec<usize> = all.iter().copied().filter(|&j| j != i).collect();
            decks.push(out(&others));
        }
        for i in 0..n {
            decks.push(out(&[i]));
        }
        let o = run(&decks);
        println!(
            "full deck {:.2}%  package out {:.2}%",
            o.win_rate(0) * 100.0,
            o.win_rate(1) * 100.0
        );
        println!("{:>16}  {:>16}  card", "alone", "in the package");
        for i in 0..n {
            println!(
                "{}  {}  {}",
                pts(&o.diff(2 + i, 1)),
                pts(&o.diff(0, 2 + n + i)),
                pkg[i]
            );
        }
        let whole = o.diff(0, 1);
        let mut terms = vec![(0usize, 1.0f64), (1, -1.0)];
        for i in 0..n {
            terms.push((2 + i, -1.0));
            terms.push((1, 1.0));
        }
        let alone_sum = o.contrast(
            &(0..n)
                .flat_map(|i| [(2 + i, 1.0), (1usize, -1.0)])
                .collect::<Vec<_>>(),
        );
        let loo_sum = o.contrast(
            &(0..n)
                .flat_map(|i| [(0usize, 1.0), (2 + n + i, -1.0)])
                .collect::<Vec<_>>(),
        );
        println!(
            "package {}   sum alone {}   sum in package {}",
            pts(&whole),
            pts(&alone_sum),
            pts(&loo_sum)
        );
        println!(
            "interaction (package - sum alone) {}",
            pts(&o.contrast(&terms))
        );
    }

    if !tribe.is_empty() {
        let sub = tribe.as_str();
        let pay_idx: Vec<u16> = payoffs.iter().map(|p| index(p)).collect();
        // The deck's other creatures of the type, in a fixed shuffled order.
        let mut members: Vec<u16> = Vec::new();
        for i in 0..base.library_len() {
            let idx = base.library_card(i);
            let c = card(&base.cards[idx as usize].name);
            if c.is_creature()
                && c.subtypes().iter().any(|s| s.eq_ignore_ascii_case(sub))
                && !pay_idx.contains(&idx)
                && !members.contains(&idx)
            {
                members.push(idx);
            }
        }
        Rng::new(seed ^ 0x7121BE).shuffle(&mut members);
        if steps.is_empty() {
            steps = (0..=4).map(|q| q * members.len() / 4).collect();
        }
        steps.retain(|&s| s <= members.len());
        println!(
            "{} other {}s in the deck (payoffs kept as they are: {})",
            members.len(),
            sub,
            payoffs.join(", ")
        );
        let variant = |keep: usize, with_payoffs: bool| -> Prepared {
            let mut swaps: Vec<(u16, SimCard)> = members[keep..]
                .iter()
                .map(|&i| {
                    (
                        i,
                        compile(&untyped(card(&base.cards[i as usize].name), sub)),
                    )
                })
                .collect();
            if !with_payoffs {
                for (p, &i) in payoffs.iter().zip(&pay_idx) {
                    swaps.push((i, stand_in(card(p))));
                }
            }
            base.with_swaps(&swaps)
        };
        let mut decks = Vec::new();
        for &s in &steps {
            decks.push(variant(s, true));
            decks.push(variant(s, false));
        }
        let o = run(&decks);
        println!(
            "{:>5}  {:>8}  {:>8}  {:>16}  {:>16}  {:>16}  {:>16}",
            "typed",
            "with",
            "without",
            "payoffs' value",
            "vs fewest",
            "tribe w/ payoffs",
            "tribe w/o"
        );
        for (s_i, &s) in steps.iter().enumerate() {
            let (w, wo) = (2 * s_i, 2 * s_i + 1);
            let inter = o.contrast(&[(w, 1.0), (wo, -1.0), (0, -1.0), (1, 1.0)]);
            println!(
                "{:>5}  {:>7.2}%  {:>7.2}%  {}  {}  {}  {}",
                s,
                o.win_rate(w) * 100.0,
                o.win_rate(wo) * 100.0,
                pts(&o.diff(w, wo)),
                pts(&inter),
                pts(&o.diff(w, 0)),
                pts(&o.diff(wo, 1)),
            );
        }
    }
}
