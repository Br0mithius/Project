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
    variants.extend(a[3..].iter().map(|v| v.parse::<u32>().expect("variant")));
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
        if *v == 0 {
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
    }
    std::process::exit(if pass { 0 } else { 1 });
}
