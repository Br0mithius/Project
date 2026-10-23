//! Calibrates the deck builder's card scores against the game simulator: what each card is
//! worth in simulated games, next to what `analysis::score_card` says.
//!
//! `cargo run --release --example card_calibration -- <mode> ...`
//!
//! * `list <data dir> <deck>`: the cards to measure, one `kind:name` per line - the deck's
//!   spells (`deck:`) and every other owned spell inside the commander's identity (`pool:`).
//! * `score <data dir> <deck> <names file> <out.tsv>`: the builder's score for each listed
//!   card - a deck card scored against the deck without one copy (like `analysis::weakest`),
//!   a pool card against the whole deck (like `analysis::suggestions`).
//! * `measure <data dir> <deck> <games> <seed> <slot> <names file> <out.tsv>`: paired games
//!   against the playtest gauntlet. The neutral stand-in is a colourless vanilla N/N for
//!   {N} (N = the card's mana value, 1..7), never a land:
//!   - a deck card: the deck, minus the deck with that card replaced by the stand-in;
//!   - a pool card: the deck with `slot` (one of its spells) replaced by the card, minus
//!     the deck with `slot` replaced by the stand-in.
//!
//!   `vs_vanilla` is that difference. `abs` puts every card on one scale - over the 3-mana
//!   stand-in in `slot` - by adding how much the card's own stand-in is worth over the
//!   3-mana one, so cards of different costs compare the way a deck builder must compare
//!   them. Win-rate points (1 = one percentage point), with standard errors.
use msh_deckbuilder::analysis::{self, resolved};
use msh_deckbuilder::card::Card;
use msh_deckbuilder::db::*;
use msh_deckbuilder::deck::Deck;
use msh_deckbuilder::playtest::Prepared;
use msh_deckbuilder::playtest::compile::compile;
use msh_deckbuilder::playtest::lab::play_all;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize};

fn setup(dir: &str) -> (CardDb, Collection, Rules) {
    let dir = PathBuf::from(dir);
    let mut db = CardDb::load(&dir.join("cards.json")).unwrap();
    db.attach_ratings(&dir);
    let coll = Collection::load(&dir.join("collection.txt")).unwrap();
    let rules = msh_deckbuilder::league::app_rules(&dir, None);
    (db, coll, rules)
}

/// A colourless vanilla N/N for {N}.
fn vanilla(mv: u32) -> Card {
    let mv = mv.clamp(1, 7);
    Card {
        name: format!("Vanilla {}/{} for {{{}}}", mv, mv, mv),
        mana_cost: format!("{{{}}}", mv),
        cmc: mv as f32,
        type_line: "Artifact Creature — Construct".into(),
        power: Some(mv.to_string()),
        toughness: Some(mv.to_string()),
        rarity: "common".into(),
        confidence: "verified".into(),
        ..Default::default()
    }
}

fn names(path: &str) -> Vec<(String, String)> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let (k, n) = l.split_once(':').expect("kind:name");
            (k.to_string(), n.to_string())
        })
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("list") => {
            let (db, coll, rules) = setup(&args[2]);
            let deck = Deck::load(&PathBuf::from(&args[3]), &db).unwrap();
            let identity = deck.identity(&db);
            let mut seen = std::collections::HashSet::new();
            for (c, _) in resolved(&deck, &db) {
                if !c.is_land() && seen.insert(c.name.clone()) {
                    println!("deck:{}", c.name);
                }
            }
            for c in db.cards.iter().filter(|c| !c.is_land()) {
                if seen.contains(&c.name)
                    || deck.commanders().contains(&&c.name)
                    || analysis::available(c, &deck, &coll, &rules) == 0
                    || identity.is_some_and(|id| !c.identity().is_subset_of(id))
                {
                    continue;
                }
                seen.insert(c.name.clone());
                println!("pool:{}", c.name);
            }
        }
        Some("score") => {
            let (db, _coll, rules) = setup(&args[2]);
            let deck = Deck::load(&PathBuf::from(&args[3]), &db).unwrap();
            let mut out =
                String::from("kind\tname\tscore\tmv\ttype\twhy\tbody\tkeywords\tabilities\n");
            for (kind, name) in names(&args[4]) {
                let c = db.get(&name).unwrap_or_else(|| panic!("{}", name));
                let d = if kind == "deck" {
                    let mut d = deck.clone();
                    d.remove(&c.name, 1);
                    d
                } else {
                    deck.clone()
                };
                let (s, why) = analysis::score_card(c, &d, &db, &rules);
                let v = if c.is_creature() {
                    msh_deckbuilder::value::creature_value(c)
                } else {
                    Default::default()
                };
                out += &format!(
                    "{}\t{}\t{:.3}\t{}\t{}\t{}\t{:.2}\t{:.2}\t{:.2}\n",
                    kind,
                    c.name,
                    s,
                    c.mana_value(),
                    c.main_type(),
                    why.join("; "),
                    v.body,
                    v.keywords,
                    v.abilities
                );
            }
            std::fs::write(&args[5], out).unwrap();
        }
        Some("measure") => {
            let (db, _coll, rules) = setup(&args[2]);
            let deck = Deck::load(&PathBuf::from(&args[3]), &db).unwrap();
            let games: u32 = args[4].parse().unwrap();
            let seed: u64 = args[5].parse().unwrap();
            let slot_name = &args[6];
            let list = names(&args[7]);
            let bed = msh_deckbuilder::ranking::TestBed::new(&db, &rules);
            let prep = Prepared::new(&deck, &db);
            let slot = prep.card_index(slot_name).expect("slot card in the deck");
            // Variants 1..=7: the slot replaced by the N-mana stand-in.
            let mut variants: Vec<Prepared> = (1..=7)
                .map(|mv| prep.with_swaps(&[(slot, compile(&vanilla(mv)))]))
                .collect();
            for (kind, name) in &list {
                let card = db.get(name).unwrap_or_else(|| panic!("{}", name));
                let v = if kind == "deck" {
                    let i = prep.card_index(&card.name).expect("deck card");
                    prep.with_swaps(&[(i, compile(&vanilla(card.mana_value())))])
                } else {
                    prep.with_swaps(&[(slot, compile(card))])
                };
                variants.push(v);
            }
            let mut all: Vec<&Prepared> = vec![&prep];
            all.extend(variants.iter());
            let (done, cancel) = (AtomicUsize::new(0), AtomicBool::new(false));
            let t = std::time::Instant::now();
            let o = play_all(&all, &bed.gauntlet, &rules, games, seed, &done, &cancel);
            let van = |mv: u32| mv.clamp(1, 7) as usize; // index of the N-mana stand-in
            let mut out = format!(
                "# games={} seed={} slot={} base_win={:.4} secs={:.0} opponents={}\n\
                 kind\tname\tmv\tvs_vanilla\tse\tabs\tabs_se\n",
                games,
                seed,
                slot_name,
                o.win_rate(0),
                t.elapsed().as_secs_f32(),
                bed.gauntlet.len()
            );
            for mv in 1..=7u32 {
                let m = o.diff(van(mv), van(3));
                out += &format!(
                    "vanilla\tVanilla {}/{}\t{}\t0\t0\t{:.3}\t{:.3}\n",
                    mv,
                    mv,
                    mv,
                    m.gain * 100.0,
                    m.stderr * 100.0
                );
            }
            for (i, (kind, name)) in list.iter().enumerate() {
                let card = db.get(name).unwrap();
                let mv = card.mana_value();
                let k = 8 + i;
                let vv = if kind == "deck" {
                    o.diff(0, k)
                } else {
                    o.diff(k, van(mv))
                };
                let shift = o.diff(van(mv), van(3));
                let (abs, abs_se) = if kind == "deck" {
                    (
                        vv.gain + shift.gain,
                        (vv.stderr.powi(2) + shift.stderr.powi(2)).sqrt(),
                    )
                } else {
                    let m = o.diff(k, van(3));
                    (m.gain, m.stderr)
                };
                out += &format!(
                    "{}\t{}\t{}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\n",
                    kind,
                    card.name,
                    mv,
                    vv.gain * 100.0,
                    vv.stderr * 100.0,
                    abs * 100.0,
                    abs_se * 100.0
                );
            }
            std::fs::write(&args[8], out).unwrap();
        }
        // colors <data dir> <commander>: pool quality of every colour set the commander
        // allows, and the set `best_colors` picks.
        Some("colors") => {
            let (db, coll, rules) = setup(&args[2]);
            let cmd = db.get(&args[3]).expect("commander");
            let id = cmd.identity();
            for mask in 1u8..32 {
                let s = msh_deckbuilder::card::ColorSet(mask);
                if s.is_subset_of(id) && cmd.cost_colors().is_subset_of(s) {
                    let q = analysis::pool_quality_for(cmd, s, &db, &coll, &rules);
                    println!("{:6} {:.3}", s.to_string(), q);
                }
            }
            println!(
                "best_colors: {}",
                analysis::best_colors(cmd, &db, &coll, &rules)
            );
        }
        // rank <data dir> <games>: ranks every commander option, then checks each weight set
        // against an independent playtest of the same decks (opponents from other simulated
        // pools, other shuffles): rank correlation of the ranking total with that win rate.
        Some("rank") => {
            use msh_deckbuilder::ranking::{self, Weights};
            let (db, coll, rules) = setup(&args[2]);
            let games: u32 = args[3].parse().unwrap();
            let done = AtomicUsize::new(0);
            let mut evals = ranking::rank_all(&db, &coll, &rules, &Weights::default(), &done);
            let other: Vec<Prepared> = [777u64, 888]
                .into_iter()
                .flat_map(|seed| msh_deckbuilder::playtest::gauntlet(&db, &rules, seed))
                .collect();
            // One option per thread at a time.
            let next = AtomicUsize::new(0);
            let truth_m = std::sync::Mutex::new(vec![0.0f32; evals.len()]);
            std::thread::scope(|s| {
                for _ in 0..std::thread::available_parallelism().map_or(2, |n| n.get()) {
                    s.spawn(|| {
                        loop {
                            let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let Some(e) = evals.get(i) else { break };
                            let w = msh_deckbuilder::playtest::win_rate(
                                &e.deck, &other, &db, &rules, games, 11,
                            );
                            truth_m.lock().unwrap()[i] = w;
                        }
                    });
                }
            });
            let truth = truth_m.into_inner().unwrap();
            let keyed: std::collections::HashMap<String, f32> = evals
                .iter()
                .zip(&truth)
                .map(|(e, t)| (format!("{}{:?}", e.commander, e.piper), *t))
                .collect();
            // The defaults before this calibration: playtest 1.5, mana 0.6, curve 0.5.
            let old = Weights {
                cards: 1.0,
                curve: 0.5,
                creatures: 0.5,
                interaction: 0.6,
                mana: 0.6,
                commander: 0.8,
                playtest: 1.5,
                edhrec: 0.4,
            };
            let sets: Vec<(&str, Weights)> = vec![
                ("old defaults", old),
                ("default", Weights::default()),
                (
                    "old, playtest 3",
                    Weights {
                        playtest: 3.0,
                        ..old
                    },
                ),
                ("old, mana 0.2", Weights { mana: 0.2, ..old }),
                (
                    "old, playtest 2.5, mana 0.3",
                    Weights {
                        playtest: 2.5,
                        mana: 0.3,
                        ..old
                    },
                ),
                (
                    "playtest only",
                    Weights {
                        cards: 0.0,
                        curve: 0.0,
                        creatures: 0.0,
                        interaction: 0.0,
                        mana: 0.0,
                        commander: 0.0,
                        playtest: 1.0,
                        edhrec: 0.0,
                    },
                ),
            ];
            let rho = |a: &[f32], b: &[f32]| {
                let r = |v: &[f32]| {
                    let mut idx: Vec<usize> = (0..v.len()).collect();
                    idx.sort_by(|&x, &y| v[x].partial_cmp(&v[y]).unwrap());
                    let mut out = vec![0.0f32; v.len()];
                    for (k, i) in idx.into_iter().enumerate() {
                        out[i] = k as f32;
                    }
                    out
                };
                let (ra, rb) = (r(a), r(b));
                let n = ra.len() as f32;
                let (ma, mb) = (ra.iter().sum::<f32>() / n, rb.iter().sum::<f32>() / n);
                let cov: f32 = ra.iter().zip(&rb).map(|(x, y)| (x - ma) * (y - mb)).sum();
                let sa: f32 = ra.iter().map(|x| (x - ma).powi(2)).sum::<f32>().sqrt();
                let sb: f32 = rb.iter().map(|x| (x - mb).powi(2)).sum::<f32>().sqrt();
                cov / (sa * sb).max(1e-6)
            };
            println!(
                "{} options; independent playtest {} games each",
                evals.len(),
                games
            );
            for p in 0..ranking::PARTS {
                let v: Vec<f32> = evals.iter().map(|e| e.parts.values()[p]).collect();
                let t: Vec<f32> = evals
                    .iter()
                    .map(|e| keyed[&format!("{}{:?}", e.commander, e.piper)])
                    .collect();
                let pairs: Vec<(f32, f32)> = v
                    .iter()
                    .zip(&t)
                    .filter(|(x, _)| x.is_finite())
                    .map(|(x, y)| (*x, *y))
                    .collect();
                let (x, y): (Vec<f32>, Vec<f32>) = pairs.into_iter().unzip();
                if x.len() >= 3 {
                    println!(
                        "  part {:<12} vs independent playtest: {:+.2}",
                        ranking::Parts::LABELS[p],
                        rho(&x, &y)
                    );
                }
            }
            for (label, w) in sets {
                ranking::finalize(&mut evals, &w);
                let tot: Vec<f32> = evals.iter().map(|e| e.total).collect();
                let t: Vec<f32> = evals
                    .iter()
                    .map(|e| keyed[&format!("{}{:?}", e.commander, e.piper)])
                    .collect();
                let top = &evals[0];
                println!(
                    "  {:<34} total vs independent playtest {:+.2}; #1 {} {} ({:.0}%)",
                    label,
                    rho(&tot, &t),
                    top.commander,
                    top.plays,
                    100.0 * keyed[&format!("{}{:?}", top.commander, top.piper)]
                );
            }
        }
        _ => eprintln!("usage: card_calibration list|score|measure ... (see the file header)"),
    }
}
