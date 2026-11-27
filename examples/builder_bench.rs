//! The deck builder's benchmark: a fixed panel of commanders (the real table's, Thanos and
//! strong builds across archetypes), each auto-built from 2 realistic pools
//! (`table::realistic_pool`), playing the same games against the real table decks and the
//! peer field. With variants (`analysis::BUILD_VARIANT` bit flags), each is built again
//! with the variant and compared deck for deck with the standard build on the same games:
//! the mean paired difference over the panel, in win-rate points.
//!
//! `cargo run --release --example builder_bench -- <data dir> <games> [variant]...`
//! (`BENCH_SEED=<n>`: other pools and games, to confirm a result on fresh ones.)
use msh_deckbuilder::analysis::BUILD_VARIANT;
use msh_deckbuilder::db::CardDb;
use msh_deckbuilder::deck::Deck;
use msh_deckbuilder::playtest::Prepared;
use msh_deckbuilder::playtest::lab::play_all;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const PANEL: &[&str] = &[
    "Thanos, the Mad Titan",
    "Tatyova, Benthic Druid",
    "Smaug, Wicked Worm",
    "Nick Fury, Agent of S.H.I.E.L.D.",
    "The Sentry, Golden Guardian",
    "Kwia Vigorbloom",
    "Zetalpa, Primal Dawn",
    "Storm, Windrider",
    "Elenda, Saint of Dusk",
    "Dwalin, Weaponmaster",
    "Black Widow, Double Agent",
    "Bard the Bowman",
    "The Lord of the Eagles",
    "Thor Odinson",
    "Vraska, Soul of Stone",
    "Spider-Woman, Secret Agent",
    "King T'Challa",
    "Black Panther, Vanguard",
    "Doctor Doom",
    "Captain Marvel, Earth's Protector",
    "Kykar, Zephyr Awakener",
    "Ghalta the Immovable",
];

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(&a[0]);
    let games: u32 = a[1].parse().expect("games");
    let variants: Vec<u32> = a[2..].iter().map(|v| v.parse().expect("variant")).collect();
    let mut db = CardDb::load(&dir.join("cards.json")).expect("cards.json");
    db.attach_ratings(&dir);
    let rules = msh_deckbuilder::league::app_rules(&dir, None);
    let table = msh_deckbuilder::table::Table::load(&dir);
    let mut gauntlet = msh_deckbuilder::table::gauntlet(&table, &db);
    gauntlet.extend(msh_deckbuilder::table::peer_field(&db, &rules));
    // `BENCH_SEED=<n>`: other pools and other games (to confirm a result on fresh ones).
    let bench_seed: u64 = std::env::var("BENCH_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let build = |variant: u32| -> Vec<Deck> {
        BUILD_VARIANT.store(variant, Ordering::Relaxed);
        let mut out = Vec::new();
        for name in PANEL {
            let Some(cmd) = db.get(name) else {
                eprintln!("{} not in the card database", name);
                continue;
            };
            for s in 0..2u64 {
                let seed = msh_deckbuilder::table::name_seed(&cmd.name)
                    ^ (s * 0x51ED)
                    ^ bench_seed.wrapping_mul(0x9E3779B97F4A7C15);
                let pool = msh_deckbuilder::table::realistic_pool(cmd, &db, 66, 14, seed);
                let (mut d, _, _) =
                    msh_deckbuilder::archetypes::autobuild(cmd, &db, &pool, &rules, None, None);
                d.name = format!("{} #{}", cmd.name, s + 1);
                out.push(d);
            }
        }
        BUILD_VARIANT.store(0, Ordering::Relaxed);
        out
    };
    let base = build(0);
    let mut all: Vec<Deck> = base.clone();
    for &v in &variants {
        all.extend(build(v));
    }
    let preps: Vec<Prepared> = all.iter().map(|d| Prepared::new(d, &db)).collect();
    let refs: Vec<&Prepared> = preps.iter().collect();
    let (done, cancel) = (AtomicUsize::new(0), AtomicBool::new(false));
    let o = play_all(
        &refs,
        &gauntlet,
        &rules,
        games,
        0xBE7C4 ^ bench_seed,
        &done,
        &cancel,
    );
    let n = base.len();
    let mean = (0..n).map(|i| o.win_rate(i)).sum::<f32>() / n as f32;
    println!(
        "panel: {} decks, {} games each against the table and the peer field: mean win rate {:.2}%",
        n,
        games,
        mean * 100.0
    );
    for (k, &v) in variants.iter().enumerate() {
        let off = n * (k + 1);
        let mut changed = 0;
        let (mut sum, mut var) = (0.0f64, 0.0f64);
        let mut worst = (f32::MAX, String::new());
        let mut best = (f32::MIN, String::new());
        for i in 0..n {
            let same = base[i].cards == all[off + i].cards;
            if !same {
                changed += 1;
            }
            let d = o.diff(off + i, i);
            sum += d.gain as f64;
            var += (d.stderr as f64).powi(2);
            if d.gain < worst.0 {
                worst = (d.gain, base[i].name.clone());
            }
            if d.gain > best.0 {
                best = (d.gain, base[i].name.clone());
            }
        }
        println!(
            "variant {}: {:+.2} ± {:.2} win-rate pts mean over the panel ({} of {} decks changed; best {:+.1} {}, worst {:+.1} {})",
            v,
            sum / n as f64 * 100.0,
            var.sqrt() / n as f64 * 100.0,
            changed,
            n,
            best.0 * 100.0,
            best.1,
            worst.0 * 100.0,
            worst.1
        );
    }
}
