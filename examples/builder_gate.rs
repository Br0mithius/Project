//! The gate a deck-builder change must pass before it becomes the default:
//!
//! 1. the golden deck: auto-build from the data folder's pool (the golden deck's commander)
//!    against the playtest-tested best deck, on the standard referee - cards in common and
//!    the paired win-rate gap, which must be within `MAX_GAP` points;
//! 2. (run `builder_bench` for the panel: no worse on average.)
//!
//! `cargo run --release --example builder_gate -- <data dir> <golden deck file> <games> [variant]...`
//! Each variant (`analysis::BUILD_VARIANT` bits; 0 = the standard builder, always run) is
//! built and judged the same way.
use msh_deckbuilder::analysis::BUILD_VARIANT;
use msh_deckbuilder::db::{CardDb, Collection};
use msh_deckbuilder::deck::Deck;
use msh_deckbuilder::lab::Progress;
use msh_deckbuilder::optimize::{Opponents, compare};
use std::sync::atomic::Ordering;

/// How far behind the tested deck the auto-build may be (win-rate points).
const MAX_GAP: f32 = 0.5;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(&a[0]);
    let games: u32 = a[2].parse().expect("games");
    let mut variants: Vec<u32> = vec![0];
    variants.extend(a[3..].iter().filter_map(|v| v.parse::<u32>().ok()));
    let mut db = CardDb::load(&dir.join("cards.json")).expect("cards.json");
    db.attach_ratings(&dir);
    let rules = msh_deckbuilder::league::app_rules(&dir, None);
    let coll = Collection::load(&dir.join("collection.txt")).expect("collection");
    let golden = Deck::load(std::path::Path::new(&a[1]), &db).expect("golden deck");
    let cmd = db
        .get(golden.commander.as_deref().expect("commander"))
        .expect("commander card")
        .clone();
    let referee = Opponents::standard(&dir, &db, &rules);
    let builds: Vec<Deck> = variants
        .iter()
        .map(|&v| {
            BUILD_VARIANT.store(v, Ordering::Relaxed);
            let (d, _, _) =
                msh_deckbuilder::archetypes::autobuild(&cmd, &db, &coll, &rules, None, None);
            BUILD_VARIANT.store(0, Ordering::Relaxed);
            d
        })
        .collect();
    // `--refine` (as the app's auto-build does): the standard draft after a quick playtest
    // search, judged as variant 9999.
    let mut builds = builds;
    if a.iter().any(|x| x == "--refine") {
        let t0 = std::time::Instant::now();
        let nominated = msh_deckbuilder::optimize::value_refine(
            &builds[0],
            &db,
            &coll,
            &rules,
            &referee,
            std::env::var("MSH_REFINE_GAMES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(6000),
            &Progress::new(),
        )
        .filter(|r| !r.steps.is_empty());
        if let Some(r) = &nominated {
            for n in &r.notes {
                eprintln!("  nominated: {}", n);
            }
        }
        let rep = if nominated.is_some() {
            nominated
        } else {
            msh_deckbuilder::optimize::optimize(
                &cmd,
                &builds[0],
                &db,
                &coll,
                &rules,
                &referee,
                msh_deckbuilder::lab::Effort::REFINE,
                2,
                &Progress::new(),
            )
        };
        let refined = match rep {
            Some(r) if r.final_gain.is_some_and(|g| g.score.z() >= 2.0) => r.best,
            _ => builds[0].clone(),
        };
        eprintln!("refine stage took {:.0} s", t0.elapsed().as_secs_f32());
        builds.push(refined);
        variants.push(9999);
    }
    let gains = compare(
        &golden,
        &builds,
        &db,
        &referee,
        &rules,
        games,
        0x6A7E,
        &Progress::new(),
    );
    let size: u32 = golden.cards.values().sum();
    let mut pass = true;
    for ((v, d), g) in variants.iter().zip(&builds).zip(&gains) {
        let common: u32 = golden
            .cards
            .iter()
            .map(|(n, k)| (*k).min(d.count_of(n)))
            .sum();
        let (gap, se) = (g.score.gain * 100.0, g.score.stderr * 100.0);
        let ok = gap >= -MAX_GAP;
        if *v == 0 || *v == 9999 {
            pass = ok;
        }
        println!(
            "variant {}: {} of {} cards in common, gap to the tested deck {:+.2} ± {:.2} pts -> {}",
            v,
            common,
            size,
            gap,
            se,
            if ok { "PASS" } else { "FAIL" }
        );
        let missing: Vec<String> = golden
            .cards
            .iter()
            .filter(|(n, k)| d.count_of(n) < **k)
            .map(|(n, k)| format!("{} {}", k - d.count_of(n), n))
            .collect();
        let extra: Vec<String> = d
            .cards
            .iter()
            .filter(|(n, k)| golden.count_of(n) < **k)
            .map(|(n, k)| format!("{} {}", k - golden.count_of(n), n))
            .collect();
        println!("    tested deck has, build lacks: {}", missing.join(", "));
        println!("    build has instead:            {}", extra.join(", "));
        // Why: each differing card's builder score (in the build without it) and its
        // measured value.
        let names = golden
            .cards
            .iter()
            .filter(|(n, k)| d.count_of(n) < **k)
            .chain(d.cards.iter().filter(|(n, k)| golden.count_of(n) < **k))
            .map(|(n, _)| n.clone());
        for n in names {
            let Some(c) = db.get(&n).filter(|c| !c.is_land()) else {
                continue;
            };
            let mut without = d.clone();
            without.remove(&n, 1);
            let (score, why) = msh_deckbuilder::analysis::score_card(c, &without, &db, &rules);
            let p = db.sim_values.power(&c.name);
            println!(
                "      {:<30} score {:5.2}  measured {}  | {}",
                n,
                score,
                p.map_or("-".to_string(), |p| format!(
                    "{:+.2} ± {:.2}",
                    p.value, p.se
                )),
                why.iter().take(4).cloned().collect::<Vec<_>>().join("; ")
            );
        }
    }
    std::process::exit(if pass { 0 } else { 1 });
}
