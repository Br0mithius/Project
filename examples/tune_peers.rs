//! Tunes every deck of a peer field with the deck optimiser, each inside the realistic pool
//! it was built from (`table::realistic_pool`, the same seed as `peer_field_real`), against
//! the real table and the other decks of the field - so the field is as strong as its pools
//! allow, not a first draft. A tuned list is kept only when it beats its start on fresh
//! games by one standard error or more.
//!
//! `cargo run --release --example tune_peers -- <data dir> <field file> <out file>
//! [--rares R] [--mythics M] [--seed N] [--rounds N] [--quick]`
use msh_deckbuilder::db::CardDb;
use msh_deckbuilder::deck::Deck;
use msh_deckbuilder::lab::{Effort, Progress};
use msh_deckbuilder::optimize::{DEFAULT_TABLE_WEIGHT, Opponents, optimize};
use msh_deckbuilder::playtest::Prepared;

fn arg(a: &[String], name: &str, default: usize) -> usize {
    a.iter()
        .position(|x| x == name)
        .and_then(|i| a.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(&a[0]);
    let (rares, mythics) = (arg(&a, "--rares", 66), arg(&a, "--mythics", 14));
    let field_seed = arg(&a, "--seed", 0) as u64 * 0x9E37_79B9;
    let rounds = arg(&a, "--rounds", 2);
    let effort = if a.iter().any(|x| x == "--quick") {
        Effort::QUICK
    } else {
        Effort::REFINE
    };
    let mut db = CardDb::load(&dir.join("cards.json")).expect("cards.json");
    db.attach_ratings(&dir);
    let rules = msh_deckbuilder::league::app_rules(&dir, None);
    let text = std::fs::read_to_string(&a[1]).expect("field file");
    let head: String = text
        .lines()
        .take_while(|l| l.starts_with('#'))
        .map(|l| format!("{}\n", l))
        .collect();
    let mut decks: Vec<Deck> = text
        .split("===")
        .skip(1)
        .map(|t| Deck::from_text("peer", t, &db))
        .filter(|d| d.commander.is_some())
        .collect();
    let table = msh_deckbuilder::table::Table::load(&dir);
    let t0 = std::time::Instant::now();
    for i in 0..decks.len() {
        let deck = decks[i].clone();
        let cmd = db
            .get(deck.commander.as_deref().unwrap())
            .expect("commander card");
        let pool = msh_deckbuilder::table::realistic_pool(
            cmd,
            &db,
            rares,
            mythics,
            msh_deckbuilder::table::name_seed(&cmd.name) ^ field_seed,
        );
        for (name, n) in &deck.cards {
            let basic = db.get(name).is_some_and(|c| c.type_line.contains("Basic"));
            if !basic && pool.owned(name) < *n {
                eprintln!("  note: {} is not in {}'s pool (other seed or rarity counts?)", name, cmd.name);
            }
        }
        // The rest of the field as it stands now: decks tuned earlier are met tuned.
        let others: Vec<Prepared> = decks
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, d)| Prepared::new(d, &db))
            .collect();
        let opp = Opponents {
            table: msh_deckbuilder::table::gauntlet(&table, &db),
            league: others,
            table_weight: DEFAULT_TABLE_WEIGHT,
        };
        let r = optimize(
            cmd,
            &deck,
            &db,
            &pool,
            &rules,
            &opp,
            effort,
            rounds,
            &Progress::new(),
        )
        .expect("not cancelled");
        let gain = r.final_gain.as_ref().map(|g| g.score);
        let better = gain.as_ref().is_some_and(|g| g.gain > 0.0 && g.z() >= 1.0);
        println!(
            "[{}/{}] {}: {} ({} steps, {:.0} s, total {:.0} min){}",
            i + 1,
            decks.len(),
            cmd.name,
            match &gain {
                Some(g) => format!("{:+.1} ± {:.1} pts", g.gain * 100.0, g.stderr * 100.0),
                None => "no change".to_string(),
            },
            r.steps.len(),
            r.seconds,
            t0.elapsed().as_secs_f32() / 60.0,
            if better { "" } else { " - kept as it was" }
        );
        for s in &r.steps {
            println!(
                "    {} ({:+.1} ± {:.1})",
                s.what,
                s.gain.gain * 100.0,
                s.gain.stderr * 100.0
            );
        }
        if better {
            let mut best = r.best.clone();
            best.name = deck.name.clone();
            decks[i] = best;
        }
        // Saved after every deck: a stopped run keeps what it has.
        let mut out = head.clone();
        for d in &decks {
            out += "===\n";
            out += &d.to_text();
        }
        std::fs::write(&a[2], out).expect("write");
    }
}
