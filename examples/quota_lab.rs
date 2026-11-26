//! Lets playtests set a control plan's minimums: builds the commander's deck under many
//! settings (the plan's minimum locks, answers, card advantage and ramp; the archetype's
//! creature, draw and ramp targets; a quality floor for the slots), and plays every
//! distinct deck against the default build on the same games - the peer field and the
//! data folder's next table, weighted as the optimiser does. The leaders are measured
//! again on fresh games.
//!
//! `cargo run --release --example quota_lab -- <data dir> "<commander>" [games] [session]`
use msh_deckbuilder::cohesion::set_quota_counts;
use msh_deckbuilder::db::{CardDb, Collection};
use msh_deckbuilder::deck::Deck;
use msh_deckbuilder::lab::Progress;
use msh_deckbuilder::optimize::{DEFAULT_TABLE_WEIGHT, Opponents, compare};

#[derive(Clone, Copy, Debug, PartialEq)]
struct Setting {
    jobs: [u32; 4],
    floor: bool,
    creatures: u32,
    draw: u32,
    ramp: u32,
}

impl Setting {
    fn label(&self) -> String {
        format!(
            "locks {} answers {} cards {} ramp {} | creatures {} draw {} ramp-role {}{}",
            self.jobs[0],
            self.jobs[1],
            self.jobs[2],
            self.jobs[3],
            self.creatures,
            self.draw,
            self.ramp,
            if self.floor { " | floor" } else { "" }
        )
    }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(&a[0]);
    let cmd_name = &a[1];
    let games: u32 = a.get(2).and_then(|g| g.parse().ok()).unwrap_or(3000);
    let mut db = CardDb::load(&dir.join("cards.json")).expect("cards.json");
    db.sim_values = msh_deckbuilder::playtest::simvalues::SimValues::load(&dir);
    let coll = Collection::load(&dir.join("collection.txt")).expect("collection");
    let rules = msh_deckbuilder::league::app_rules(&dir, a.get(3).and_then(|s| s.parse().ok()));
    let cmd = db.get(cmd_name).expect("commander").clone();
    let arch = msh_deckbuilder::archetypes::for_commander(&cmd.name).expect("archetype");
    let t0 = msh_deckbuilder::archetypes::targets(arch);
    let base = Setting {
        jobs: [5, 9, 7, 8],
        floor: false,
        creatures: t0.creatures,
        draw: t0.draw,
        ramp: t0.ramp,
    };
    let mut settings = vec![base];
    #[allow(unused_mut)]
    let mut push = |s: Setting| {
        if !settings.contains(&s) {
            settings.push(s);
        }
    };
    for k in 0..4 {
        for v in [0, 3, 5, 7, 9, 12] {
            let mut s = base;
            s.jobs[k] = v;
            push(s);
        }
    }
    for v in [0, 12] {
        push(Setting {
            creatures: v,
            ..base
        });
    }
    for v in [0, 6] {
        push(Setting { draw: v, ..base });
    }
    for v in [0, 6] {
        push(Setting { ramp: v, ..base });
    }
    push(Setting {
        floor: true,
        ..base
    });
    // Only a lock minimum: the plan's identity kept, the rest left to the cards' scores.
    for locks in [3, 5] {
        push(Setting {
            jobs: [locks, 0, 0, 0],
            floor: false,
            creatures: 0,
            draw: 0,
            ramp: 0,
        });
    }
    // No minimums at all: the cards' scores and the upgrade pass decide.
    push(Setting {
        jobs: [0; 4],
        floor: false,
        creatures: 0,
        draw: 0,
        ramp: 0,
    });
    push(Setting {
        floor: true,
        creatures: 0,
        draw: 0,
        ramp: 0,
        ..base
    });
    // QUOTA_SETS="5,0,0,8;5,0,7,8": test only these job minimums (no creature, draw or ramp
    // targets) against the default.
    if let Ok(sets) = std::env::var("QUOTA_SETS") {
        settings.truncate(1);
        for set in sets.split(';') {
            // A fifth field "f": with the quality floor.
            let floor = set.split(',').any(|x| x.trim() == "f");
            let v: Vec<u32> = set.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            if v.len() == 4 {
                settings.push(Setting {
                    jobs: [v[0], v[1], v[2], v[3]],
                    floor,
                    creatures: 0,
                    draw: 0,
                    ramp: 0,
                });
            }
        }
    }
    let build = |s: &Setting| -> Deck {
        set_quota_counts(Some(s.jobs), s.floor);
        let t = msh_deckbuilder::archetypes::Targets {
            creatures: s.creatures,
            draw: s.draw,
            ramp: s.ramp,
            ..t0
        };
        let (mut d, _) = msh_deckbuilder::archetypes::build(arch, &cmd, &db, &coll, &rules, &t);
        d.commander = Some(cmd.name.clone());
        d
    };
    let canon = |d: &Deck| -> Vec<(String, u32)> {
        let mut v: Vec<(String, u32)> = d.cards.iter().map(|(k, n)| (k.clone(), *n)).collect();
        v.sort();
        v
    };
    let base_deck = build(&base);
    let mut seen = vec![canon(&base_deck)];
    let mut cands: Vec<(Setting, Deck)> = Vec::new();
    for s in &settings[1..] {
        let d = build(s);
        if d.total_count(&rules) != base_deck.total_count(&rules) {
            println!("skipped (short deck): {}", s.label());
            continue;
        }
        let c = canon(&d);
        if seen.contains(&c) {
            continue;
        }
        seen.push(c);
        cands.push((*s, d));
    }
    set_quota_counts(None, false);
    let table = msh_deckbuilder::table::Table::load(&dir);
    let opp = Opponents {
        table: msh_deckbuilder::table::gauntlet(&table, &db),
        league: msh_deckbuilder::table::peer_field(&db, &rules),
        table_weight: DEFAULT_TABLE_WEIGHT,
    };
    println!(
        "{} settings, {} distinct decks besides the default; {} games each",
        settings.len(),
        cands.len(),
        games
    );
    let decks: Vec<Deck> = cands.iter().map(|c| c.1.clone()).collect();
    let g = compare(
        &base_deck,
        &decks,
        &db,
        &opp,
        &rules,
        games,
        0x9_07A1,
        &Progress::new(),
    );
    let mut order: Vec<usize> = (0..g.len()).collect();
    order.sort_by(|&x, &y| g[y].score.gain.partial_cmp(&g[x].score.gain).unwrap());
    println!("\nSCREEN (vs the default build {}):", base.label());
    for &i in &order {
        println!(
            "  {:+5.1} ± {:.1}  {}",
            g[i].score.gain * 100.0,
            g[i].score.stderr * 100.0,
            cands[i].0.label()
        );
    }
    let top: Vec<usize> = order.iter().copied().take(4).collect();
    let decks: Vec<Deck> = top.iter().map(|&i| cands[i].1.clone()).collect();
    let g2 = compare(
        &base_deck,
        &decks,
        &db,
        &opp,
        &rules,
        games * 3,
        0x9_07A2,
        &Progress::new(),
    );
    println!("\nLEADERS ON FRESH GAMES ({} games):", games * 3);
    for (k, &i) in top.iter().enumerate() {
        println!(
            "  {:+5.1} ± {:.1}  (table {:+.1}, peers {:+.1})  {}",
            g2[k].score.gain * 100.0,
            g2[k].score.stderr * 100.0,
            g2[k].table.map_or(0.0, |m| m.gain * 100.0),
            g2[k].league.map_or(0.0, |m| m.gain * 100.0),
            cands[i].0.label()
        );
        let _ = std::fs::write(
            dir.join(format!("quota_lead_{}.txt", k + 1)),
            cands[i].1.to_text(),
        );
    }
    let _ = std::fs::write(dir.join("quota_base.txt"), base_deck.to_text());
}
