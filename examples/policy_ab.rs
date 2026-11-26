//! Measures experimental AI behaviours (`ai::exp` flags): the deck played with each set of
//! flags against itself with none, on the same games (same opponents, seats and
//! shuffles), so only that seat's play differs. Win-rate points, against the peer field
//! and the data folder's next table, weighted as the optimiser does.
//!
//! `cargo run --release --example policy_ab -- <data dir> <deck file> <games> <flags>...`
//! (`MSH_AI_EXP=<flags>` turns a behaviour on for every seat, the opponents included.)
use msh_deckbuilder::db::CardDb;
use msh_deckbuilder::deck::Deck;
use msh_deckbuilder::optimize::DEFAULT_TABLE_WEIGHT;
use msh_deckbuilder::playtest::Prepared;
use msh_deckbuilder::playtest::lab::play_all;
use std::sync::atomic::{AtomicBool, AtomicUsize};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(&a[0]);
    let mut db = CardDb::load(&dir.join("cards.json")).expect("cards.json");
    db.sim_values = msh_deckbuilder::playtest::simvalues::SimValues::load(&dir);
    let deck = Deck::load(std::path::Path::new(&a[1]), &db).expect("deck");
    let games: u32 = a[2].parse().expect("games");
    let flags: Vec<u32> = a[3..].iter().map(|f| f.parse().expect("flags")).collect();
    let rules = msh_deckbuilder::league::app_rules(&dir, None);
    let table = msh_deckbuilder::table::Table::load(&dir);
    let gauntlets = [
        ("table", msh_deckbuilder::table::gauntlet(&table, &db)),
        (
            "peer field",
            msh_deckbuilder::table::peer_field(&db, &rules),
        ),
    ];
    let base = Prepared::new(&deck, &db);
    let mut preps = vec![base.with_swaps(&[])];
    for &f in &flags {
        let mut p = base.with_swaps(&[]);
        p.ai_flags = f;
        preps.push(p);
    }
    let refs: Vec<&Prepared> = preps.iter().collect();
    let (done, cancel) = (AtomicUsize::new(0), AtomicBool::new(false));
    let mut res = Vec::new();
    for (name, g) in &gauntlets {
        if g.is_empty() {
            continue;
        }
        let o = play_all(&refs, g, &rules, games, 4_242_001, &done, &cancel);
        println!(
            "{} ({} games): base win rate {:.2}%",
            name,
            games,
            o.win_rate(0) * 100.0
        );
        res.push((*name, o));
    }
    for (k, f) in flags.iter().enumerate() {
        let i = k + 1;
        let parts: Vec<String> = res
            .iter()
            .map(|(n, o)| {
                let m = o.diff(i, 0);
                format!("{} {:+.2} ± {:.2}", n, m.gain * 100.0, m.stderr * 100.0)
            })
            .collect();
        // Weighted as the optimiser weighs the table against the peer field.
        let w = if res.len() == 2 {
            DEFAULT_TABLE_WEIGHT
        } else {
            1.0
        };
        let (mut g, mut v) = (0.0, 0.0);
        for (j, (_, o)) in res.iter().enumerate() {
            let m = o.diff(i, 0);
            let wt = if res.len() == 2 {
                if j == 0 { w } else { 1.0 - w }
            } else {
                1.0
            };
            g += wt * m.gain;
            v += (wt * m.stderr).powi(2);
        }
        println!(
            "flags {}: {:+.2} ± {:.2} win-rate pts  ({})",
            f,
            g * 100.0,
            v.sqrt() * 100.0,
            parts.join(", ")
        );
    }
}
