//! Builds and inspects decks for the set archetypes (`archetypes::ARCHETYPES`), for paired
//! playtests with `deck_compare` / `pod_compare` (decks with the same commander and size are
//! slot-paired there):
//!
//! `cargo run --release --example archetype_lab -- build <data dir> <out dir> [query]`
//! builds, for each archetype matching `query` (a name part, set code or colours) and its
//! first commander, singleton 60-card decks from the data dir's collection (give it every
//! card once for the full-database experiment): `<slug>.auto.txt` (the plain auto-build,
//! `analysis::autobuild`), `<slug>.arch.txt` (16 archetype cards first, else the builder) and
//! `<slug>.rules.txt` (the archetype's targets, `archetypes::targets`).
//!
//! `... sweep <data dir> <out dir> [query]` writes deck-building variants per archetype
//! around a base (`<slug>/00_base.txt`, the package of 16 on the builder's defaults): land
//! count, package size, removal / draw / ramp / creature / counterspell minimums, 6+ drop cap.
//!
//! `... fit <data dir> <deck file>...` prints each deck's best-fitting archetypes.
//!
//! `... dump-gauntlet <data dir> <out dir> [season seed]` saves the simulated league's
//! decks (for `pod_compare --opp`, a fixed table that doesn't move with the builder).
use msh_deckbuilder::analysis;
use msh_deckbuilder::archetypes::{self, SetArchetype, Targets};
use msh_deckbuilder::db::{CardDb, Collection};
use msh_deckbuilder::deck::Deck;
use std::path::{Path, PathBuf};

fn slug(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .split('_')
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

fn arch_slug(a: &SetArchetype) -> String {
    slug(&format!("{} {} {}", a.set, a.colors, a.name))
}

fn describe(arch: &'static SetArchetype, deck: &Deck, db: &CardDb) -> String {
    let st = analysis::stats(deck, db);
    let fit = match archetypes::deck_report(arch, deck, db) {
        Some(r) => format!(
            "{:.0}% ({} fit, {:.0}e {:.0}p)",
            r.closeness * 100.0,
            r.cards.len(),
            r.enablers,
            r.payoffs
        ),
        None => "colours don't fit".into(),
    };
    format!(
        "{} L{} C{} avg{:.2}",
        fit, st.lands, st.creatures, st.avg_mv
    )
}

/// The sweep's variants: (label, targets).
fn variants() -> Vec<(String, Targets)> {
    let base = Targets {
        package: 16,
        ..Targets::PLAIN
    };
    let mut v = vec![("00_base".to_string(), base)];
    for l in [23, 24, 25, 27, 28] {
        v.push((
            format!("lands{}", l),
            Targets {
                lands: Some(l),
                ..base
            },
        ));
    }
    for p in [0, 8, 24] {
        v.push((format!("pkg{:02}", p), Targets { package: p, ..base }));
    }
    for r in [6, 9, 12] {
        v.push((format!("removal{:02}", r), Targets { removal: r, ..base }));
    }
    for d in [5, 8, 11] {
        v.push((format!("draw{:02}", d), Targets { draw: d, ..base }));
    }
    for r in [4, 7, 10] {
        v.push((format!("ramp{:02}", r), Targets { ramp: r, ..base }));
    }
    for c in [12, 18, 24] {
        v.push((
            format!("creatures{:02}", c),
            Targets {
                creatures: c,
                ..base
            },
        ));
    }
    for c in [2, 4] {
        v.push((
            format!("counters{}", c),
            Targets {
                counters: c,
                ..base
            },
        ));
    }
    for t in [0, 2, 4] {
        v.push((
            format!("topend{}", t),
            Targets {
                topend: Some(t),
                ..base
            },
        ));
    }
    v
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args[1].as_str();
    let dir = PathBuf::from(&args[2]);
    let mut db = CardDb::load(&dir.join("cards.json")).unwrap();
    db.attach_ratings(&dir);
    // Singleton 60 (the league from session 6 on).
    let rules = msh_deckbuilder::league::app_rules(&dir, Some(6));
    let coll = || Collection::load(&dir.join("collection.txt")).unwrap();
    match mode {
        "dump-gauntlet" => {
            let out = PathBuf::from(&args[3]);
            let seed: u64 = args.get(4).map(|s| s.parse().unwrap()).unwrap_or(2026);
            let rules = msh_deckbuilder::league::app_rules(&dir, None);
            for (i, d) in msh_deckbuilder::opponents::decks(&db, &rules, seed)
                .iter()
                .enumerate()
            {
                let p = out.join(format!("opp{:02}.txt", i));
                d.save(&p).unwrap();
                println!(
                    "{}  {}",
                    p.display(),
                    d.commander.clone().unwrap_or_default()
                );
            }
        }
        "build" => {
            let out = PathBuf::from(&args[3]);
            let query = args.get(4).cloned().unwrap_or_default();
            let coll = coll();
            for arch in archetypes::find(&query).into_iter().filter(|a| !a.sparse) {
                let Some(cmd) = arch.commanders.iter().find_map(|n| db.get(n)) else {
                    continue;
                };
                let s = arch_slug(arch);
                let (mut auto, _) = analysis::autobuild(cmd, &db, &coll, &rules, None, None);
                auto.name = format!("auto {}", cmd.name);
                let pkg = Targets {
                    package: 16,
                    ..Targets::PLAIN
                };
                let ab = archetypes::build(arch, cmd, &db, &coll, &rules, &pkg);
                let rb =
                    archetypes::build(arch, cmd, &db, &coll, &rules, &archetypes::targets(arch));
                auto.save(&out.join(format!("{}.auto.txt", s))).unwrap();
                ab.save(&out.join(format!("{}.arch.txt", s))).unwrap();
                rb.save(&out.join(format!("{}.rules.txt", s))).unwrap();
                println!(
                    "{:<4}{:<4}{:<24} {:<32}\n    auto  {}\n    arch  {}\n    rules {}",
                    arch.set,
                    arch.colors,
                    arch.name,
                    cmd.name,
                    describe(arch, &auto, &db),
                    describe(arch, &ab, &db),
                    describe(arch, &rb, &db)
                );
            }
        }
        "sweep" => {
            let out = PathBuf::from(&args[3]);
            let query = args.get(4).cloned().unwrap_or_default();
            let coll = coll();
            for arch in archetypes::find(&query).into_iter().filter(|a| !a.sparse) {
                let Some(cmd) = arch.commanders.iter().find_map(|n| db.get(n)) else {
                    continue;
                };
                let sub = out.join(arch_slug(arch));
                for (label, t) in variants() {
                    let d = archetypes::build(arch, cmd, &db, &coll, &rules, &t);
                    d.save(&sub.join(format!("{}.txt", label))).unwrap();
                    println!("{} {} {}", sub.display(), label, describe(arch, &d, &db));
                }
            }
        }
        "fit" => {
            for f in &args[3..] {
                let deck = Deck::load(Path::new(f), &db).unwrap();
                let mut rs: Vec<_> = archetypes::ARCHETYPES
                    .iter()
                    .filter_map(|a| archetypes::deck_report(a, &deck, &db))
                    .collect();
                rs.sort_by(|a, b| b.closeness.partial_cmp(&a.closeness).unwrap());
                println!("{}", f);
                for r in rs.iter().take(5) {
                    println!(
                        "  {:>3.0}%  {} {} {} ({} cards: {})",
                        r.closeness * 100.0,
                        r.arch.set,
                        r.arch.colors,
                        r.arch.name,
                        r.cards.len(),
                        r.cards
                            .iter()
                            .take(8)
                            .map(|(c, _)| c.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
        }
        _ => panic!("modes: build, sweep, fit, dump-gauntlet"),
    }
}
