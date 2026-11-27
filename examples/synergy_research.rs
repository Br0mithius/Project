//! Research on archetypes and synergy in this league, on paired playtests:
//!
//! `strength`: every archetype's commanders (up to 4), each auto-built for the archetype
//! (`archetypes::autobuild_as`) from 2 realistic pools (`table::realistic_pool`, as the
//! peer field), all playing the same games against the real table decks and the peer
//! field together. Prints each archetype's mean win rate over its decks (with the spread
//! between them) and its best deck, and saves every deck to `<out dir>`.
//!
//! `synergy`: for each archetype's best deck from `strength`, its 5 most on-theme spells
//! (key cards first) as a package: each card's value alone, the package's value, and the
//! interaction (package minus the sum alone; positive = the cards are worth more
//! together), against the same opponents.
//!
//! `cargo run --release --example synergy_research -- <data dir> <out dir> strength <games>`
//! `cargo run --release --example synergy_research -- <data dir> <out dir> synergy <games> [archetype part]`
//! `cargo run --release --example synergy_research -- <data dir> <out dir> deck <games> <deck file> <card|card|...>...`
//! (`deck`: each package's cards in that deck, alone and together.)
//! `cargo run --release --example synergy_research -- <data dir> <out dir> copies <games> <deck file> <card>...`
//! (`copies`: the value of each copy of a card the deck plays more than once.)
use msh_deckbuilder::archetypes::ARCHETYPES;
use msh_deckbuilder::db::CardDb;
use msh_deckbuilder::deck::Deck;
use msh_deckbuilder::playtest::Prepared;
use msh_deckbuilder::playtest::lab::play_all;
use std::sync::atomic::{AtomicBool, AtomicUsize};

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(&a[0]);
    let out = std::path::PathBuf::from(&a[1]);
    let mode = a[2].as_str();
    let games: u32 = a[3].parse().expect("games");
    std::fs::create_dir_all(&out).unwrap();
    let mut db = CardDb::load(&dir.join("cards.json")).expect("cards.json");
    db.sim_values = msh_deckbuilder::playtest::simvalues::SimValues::load(&dir);
    let rules = msh_deckbuilder::league::app_rules(&dir, None);
    let table = msh_deckbuilder::table::Table::load(&dir);
    // One mixed set of opponents: the real table decks and the peer field.
    let mut gauntlet = msh_deckbuilder::table::gauntlet(&table, &db);
    gauntlet.extend(msh_deckbuilder::table::peer_field(&db, &rules));
    let (done, cancel) = (AtomicUsize::new(0), AtomicBool::new(false));
    match mode {
        "strength" => {
            let mut decks: Vec<(usize, Deck)> = Vec::new();
            for (k, arch) in ARCHETYPES.iter().enumerate() {
                for name in arch.commanders.iter().take(4) {
                    let Some(cmd) = db.get(name) else { continue };
                    if !cmd.can_be_commander() {
                        continue;
                    }
                    for s in 0..2u64 {
                        let seed = msh_deckbuilder::table::name_seed(&cmd.name) ^ (s * 0x9E37);
                        let pool = msh_deckbuilder::table::realistic_pool(cmd, &db, 66, 14, seed);
                        let (mut d, _) = msh_deckbuilder::archetypes::autobuild_as(
                            arch, cmd, &db, &pool, &rules,
                        );
                        if d.main_count() < d.main_target(&rules) {
                            continue;
                        }
                        d.name = format!("{} - {} #{}", arch.name, cmd.name, s + 1);
                        decks.push((k, d));
                    }
                }
            }
            eprintln!("{} decks for {} archetypes", decks.len(), ARCHETYPES.len());
            let preps: Vec<Prepared> = decks.iter().map(|(_, d)| Prepared::new(d, &db)).collect();
            let refs: Vec<&Prepared> = preps.iter().collect();
            let o = play_all(&refs, &gauntlet, &rules, games, 0x5EED, &done, &cancel);
            let mut rows = Vec::new();
            for (k, arch) in ARCHETYPES.iter().enumerate() {
                let idx: Vec<usize> = (0..decks.len()).filter(|&i| decks[i].0 == k).collect();
                if idx.is_empty() {
                    continue;
                }
                let rates: Vec<f32> = idx.iter().map(|&i| o.win_rate(i) * 100.0).collect();
                let mean = rates.iter().sum::<f32>() / rates.len() as f32;
                let sd = (rates.iter().map(|r| (r - mean).powi(2)).sum::<f32>()
                    / rates.len().max(2).saturating_sub(1) as f32)
                    .sqrt();
                let best = *idx
                    .iter()
                    .max_by(|&&x, &&y| o.win_rate(x).partial_cmp(&o.win_rate(y)).unwrap())
                    .unwrap();
                let path = out.join(format!("{}.best.txt", slug(arch.name)));
                std::fs::write(&path, decks[best].1.to_text()).unwrap();
                rows.push((
                    mean,
                    sd,
                    rates.len(),
                    arch.name,
                    o.win_rate(best) * 100.0,
                    decks[best].1.commander.clone().unwrap_or_default(),
                ));
            }
            rows.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap());
            println!(
                "archetype strength: {} games per deck against the table and the peer field (25% = even)",
                games
            );
            println!(
                "{:<34} {:>6} {:>6} {:>5}  best deck",
                "archetype", "mean%", "sd", "decks"
            );
            for (mean, sd, n, name, best, cmd) in rows {
                println!(
                    "{:<34} {:>6.1} {:>6.1} {:>5}  {:.1}% {}",
                    name, mean, sd, n, best, cmd
                );
            }
        }
        "synergy" => {
            let filter = a.get(4).map(|s| s.to_lowercase());
            println!(
                "package synergy: {} games per deck variant against the table and the peer field",
                games
            );
            for arch in ARCHETYPES {
                if filter
                    .as_ref()
                    .is_some_and(|f| !arch.name.to_lowercase().contains(f))
                {
                    continue;
                }
                let path = out.join(format!("{}.best.txt", slug(arch.name)));
                let Ok(deck) = Deck::load(&path, &db) else {
                    continue;
                };
                // The deck's most on-theme spells: key cards, then the most themes matched.
                // Ranked by how well each fits (key cards first), payoffs before enablers;
                // ties by name only last (an integer score made ties of most cards, and the
                // alphabet picked them).
                let mut themed: Vec<(bool, f32, f32, String)> = deck
                    .cards
                    .keys()
                    .filter_map(|n| db.get(n))
                    .filter(|c| !c.is_land() && Some(&c.name) != deck.commander.as_ref())
                    .filter_map(|c| {
                        let f = arch.fit(c);
                        f.fits()
                            .then(|| (f.key, f.score(), f.payoff, c.name.clone()))
                    })
                    .collect();
                themed.sort_by(|x, y| {
                    y.0.cmp(&x.0)
                        .then(y.1.partial_cmp(&x.1).unwrap())
                        .then(y.2.partial_cmp(&x.2).unwrap())
                        .then(x.3.cmp(&y.3))
                });
                let pkg_names: Vec<String> = themed.iter().take(5).map(|x| x.3.clone()).collect();
                if pkg_names.len() < 3 {
                    println!(
                        "{}: only {} on-theme spells, skipped",
                        arch.name,
                        pkg_names.len()
                    );
                    continue;
                }
                let r = match msh_deckbuilder::playtest::synergy::package_synergy(
                    &deck, &pkg_names, &db, &rules, &gauntlet, games, 0xA11CE, &done, &cancel,
                ) {
                    Ok(Some(r)) => r,
                    Ok(None) => return,
                    Err(e) => {
                        println!("{}: {}", arch.name, e);
                        continue;
                    }
                };
                println!(
                    "{:<34} package {:+.2} ± {:.2}  sum alone {:+.2} ± {:.2}  interaction {:+.2} ± {:.2}  [{}]",
                    arch.name,
                    r.package.gain * 100.0,
                    r.package.stderr * 100.0,
                    r.sum_alone.gain * 100.0,
                    r.sum_alone.stderr * 100.0,
                    r.interaction.gain * 100.0,
                    r.interaction.stderr * 100.0,
                    pkg_names.join(", ")
                );
            }
        }
        "deck" => {
            // `deck <games> <deck file> <card|card|...>...`: each package in that deck.
            let deck = Deck::load(std::path::Path::new(&a[4]), &db).expect("deck file");
            println!(
                "packages in {}: {} games per variant against the table and the peer field",
                a[4], games
            );
            for spec in &a[5..] {
                let names: Vec<String> = spec.split('|').map(|x| x.trim().to_string()).collect();
                match msh_deckbuilder::playtest::synergy::package_synergy(
                    &deck, &names, &db, &rules, &gauntlet, games, 0xD3C, &done, &cancel,
                ) {
                    Ok(Some(r)) => {
                        println!(
                            "package {:+.2} ± {:.2}  sum alone {:+.2} ± {:.2}  interaction {:+.2} ± {:.2}  [{}]",
                            r.package.gain * 100.0,
                            r.package.stderr * 100.0,
                            r.sum_alone.gain * 100.0,
                            r.sum_alone.stderr * 100.0,
                            r.interaction.gain * 100.0,
                            r.interaction.stderr * 100.0,
                            names.join(", ")
                        );
                        for (c, (al, inp)) in r.cards.iter().zip(r.alone.iter().zip(&r.in_package))
                        {
                            println!(
                                "    {:<34} alone {:+.2} ± {:.2}   in the package {:+.2} ± {:.2}",
                                c,
                                al.gain * 100.0,
                                al.stderr * 100.0,
                                inp.gain * 100.0,
                                inp.stderr * 100.0
                            );
                        }
                    }
                    Ok(None) => return,
                    Err(e) => println!("{}: {}", spec, e),
                }
            }
        }
        "copies" => {
            // `copies <games> <deck file> <card>...`: what each copy adds - the deck with
            // k of its copies replaced by neutral stand-ins, k = 0..n, on the same games.
            let deck = Deck::load(std::path::Path::new(&a[4]), &db).expect("deck file");
            let base = Prepared::new(&deck, &db);
            println!(
                "copies in {}: {} games per variant against the table and the peer field",
                a[4], games
            );
            for name in &a[5..] {
                let n = deck.count_of(name) as usize;
                let (Some(idx), Some(card)) = (base.card_index(name), db.get(name)) else {
                    println!("{}: not in the deck", name);
                    continue;
                };
                let stand = msh_deckbuilder::playtest::synergy::stand_in(card);
                // variants[k]: k copies replaced (0 = the deck as it is).
                let variants: Vec<Prepared> = (0..=n)
                    .map(|k| base.with_swaps(&vec![(idx, stand.clone()); k]))
                    .collect();
                let refs: Vec<&Prepared> = variants.iter().collect();
                let o = play_all(&refs, &gauntlet, &rules, games, 0xC0B1E5, &done, &cancel);
                println!(
                    "{} ({} copies): win rate with all {:.2}%",
                    name,
                    n,
                    o.win_rate(0) * 100.0
                );
                for m in 1..=n {
                    // The m-th copy: n-m+1 copies kept against n-m.
                    let d = o.diff(n - m, n - m + 1);
                    println!(
                        "    copy {}: {:+.2} ± {:.2} win-rate pts",
                        m,
                        d.gain * 100.0,
                        d.stderr * 100.0
                    );
                }
            }
        }
        _ => panic!("mode: strength, synergy, deck or copies"),
    }
}
