//! A diverse, realistic peer field: every commander auto-built from a pool a league
//! player could own - every common and uncommon, plus a random sample of `R` rares and
//! `M` mythics in its colours (seeded by the commander's name) - ranked by playing in our
//! seat against the real table decks (`table.json`, the players at the next table), and
//! the strongest taken with at most two per archetype and one per commander.
//!
//! `cargo run --release --example peer_field_real -- <data dir> <games> <out file>
//! [--rares R] [--mythics M] [--decks N] [--profile]`
//! `--profile`: only build every deck and print the average rarity counts (to calibrate
//! R and M against the real decks), no games.
use msh_deckbuilder::card::Card;
use msh_deckbuilder::db::{CardDb, Collection};
use msh_deckbuilder::deck::Deck;
use msh_deckbuilder::playtest::Prepared;
use msh_deckbuilder::playtest::lab::play_all;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize};

fn hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
}

fn shuffle<T>(v: &mut [T], mut seed: u64) {
    for i in (1..v.len()).rev() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        v.swap(i, (seed % (i as u64 + 1)) as usize);
    }
}

fn arg(a: &[String], name: &str, def: usize) -> usize {
    a.iter()
        .position(|x| x == name)
        .and_then(|i| a.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(def)
}

fn rarity_counts(d: &Deck, db: &CardDb) -> (u32, u32, u32) {
    let (mut m, mut r, mut e) = (0, 0, 0);
    for (n, k) in &d.cards {
        if let Some(c) = db.get(n) {
            if c.is_basic() {
                continue;
            }
            match c.rarity.as_str() {
                "mythic" => m += k,
                "rare" => r += k,
                _ => {}
            }
            if c.type_line.contains("Equipment") {
                e += k;
            }
        }
    }
    (m, r, e)
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(&a[0]);
    let games: u32 = a[1].parse().expect("games");
    let out = &a[2];
    let (rares, mythics, n_decks) = (
        arg(&a, "--rares", 24),
        arg(&a, "--mythics", 8),
        arg(&a, "--decks", 12),
    );
    let profile = a.iter().any(|x| x == "--profile");
    let mut db = CardDb::load(&dir.join("cards.json")).expect("cards.json");
    db.sim_values = msh_deckbuilder::playtest::simvalues::SimValues::load(&dir);
    let rules = msh_deckbuilder::league::app_rules(&dir, None);
    let mut built: Vec<(Deck, String)> = Vec::new();
    for cmd in db.cards.iter().filter(|c| c.can_be_commander()) {
        let id = cmd.identity();
        let fits = |c: &&Card| c.identity().is_subset_of(id) && c.name != cmd.name;
        let mut counts: BTreeMap<String, u32> = BTreeMap::new();
        let mut rare: Vec<&Card> = Vec::new();
        let mut mythic: Vec<&Card> = Vec::new();
        for c in db.cards.iter().filter(fits) {
            match c.rarity.as_str() {
                "rare" => rare.push(c),
                "mythic" => mythic.push(c),
                _ => {
                    counts.insert(c.name.clone(), 1);
                }
            }
        }
        let seed = hash(&cmd.name) | 1;
        shuffle(&mut rare, seed);
        shuffle(&mut mythic, seed.rotate_left(17) | 1);
        for c in rare.iter().take(rares).chain(mythic.iter().take(mythics)) {
            counts.insert(c.name.clone(), 1);
        }
        counts.insert(cmd.name.clone(), 1);
        let pool = Collection::new(std::path::PathBuf::new(), counts);
        let (mut d, _, arch) =
            msh_deckbuilder::archetypes::autobuild(cmd, &db, &pool, &rules, None, None);
        if d.main_count() < d.main_target(&rules) {
            continue;
        }
        d.name = format!("peer {}", cmd.name);
        built.push((
            d,
            arch.map_or("general".to_string(), |x| x.name.to_string()),
        ));
    }
    let n = built.len().max(1) as f32;
    let (sm, sr, se) = built.iter().fold((0, 0, 0), |acc, (d, _)| {
        let (m, r, e) = rarity_counts(d, &db);
        (acc.0 + m, acc.1 + r, acc.2 + e)
    });
    eprintln!(
        "{} decks built (rares {} mythics {} sampled): average {:.1} mythics, {:.1} rares, {:.1} Equipment",
        built.len(),
        rares,
        mythics,
        sm as f32 / n,
        sr as f32 / n,
        se as f32 / n
    );
    if profile {
        return;
    }
    let table = msh_deckbuilder::table::Table::load(&dir);
    let gauntlet = msh_deckbuilder::table::gauntlet(&table, &db);
    assert!(!gauntlet.is_empty(), "no table decks to rank against");
    let prepared: Vec<Prepared> = built.iter().map(|(d, _)| Prepared::new(d, &db)).collect();
    let refs: Vec<&Prepared> = prepared.iter().collect();
    let o = play_all(
        &refs,
        &gauntlet,
        &rules,
        games,
        0x9EE3,
        &AtomicUsize::new(0),
        &AtomicBool::new(false),
    );
    let mut order: Vec<usize> = (0..built.len()).collect();
    order.sort_by(|&x, &y| o.win_rate(y).partial_cmp(&o.win_rate(x)).unwrap());
    let mut per_arch: BTreeMap<String, usize> = BTreeMap::new();
    let mut chosen: Vec<usize> = Vec::new();
    for &i in &order {
        let k = per_arch.entry(built[i].1.clone()).or_insert(0);
        if *k >= 2 {
            continue;
        }
        *k += 1;
        chosen.push(i);
        if chosen.len() >= n_decks {
            break;
        }
    }
    let mut text = format!(
        "# The peer field: realistic league decks. Each commander auto-built from every common\n\
         # and uncommon plus {} rares and {} mythics sampled in its colours, ranked against the\n\
         # real table decks ({} games each), the strongest taken with at most two per archetype\n\
         # (examples/peer_field_real.rs).\n",
        rares, mythics, games
    );
    for &i in &chosen {
        let (d, arch) = &built[i];
        let (m, r, e) = rarity_counts(d, &db);
        println!(
            "{:5.1}%  {:<34} {:<22} mythic {:2} rare {:2} equipment {}",
            o.win_rate(i) * 100.0,
            d.commander.clone().unwrap_or_default(),
            arch,
            m,
            r,
            e
        );
        text += "===\n";
        text += &d.to_text();
    }
    std::fs::write(out, text).expect("write");
}
