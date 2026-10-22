//! Writes full logs of simulated games for one commander's deck against the simulated
//! league opponents, for auditing the simulator's rules and AI:
//! `cargo run --release --example sim_audit -- <data dir> "<commander>" <colours> <out dir> [games] [--gauntlet]`
//! (or a deck file such as `decks/thanos.txt` in place of the commander, with "-" for colours).
//! With `--gauntlet` the tables are the league opponents only (the commander and colours are
//! then ignored; pass "-" for them).
use msh_deckbuilder::analysis::{self, Rng};
use msh_deckbuilder::card::ColorSet;
use msh_deckbuilder::db::*;
use msh_deckbuilder::playtest::{self, Prepared};

fn main() {
    let all: Vec<String> = std::env::args().collect();
    let gauntlet_only = all.iter().any(|a| a == "--gauntlet");
    let args: Vec<&String> = all.iter().filter(|a| !a.starts_with("--")).collect();
    let dir = std::path::PathBuf::from(args[1]);
    let out = std::path::PathBuf::from(args[4]);
    let games: u64 = args.get(5).and_then(|g| g.parse().ok()).unwrap_or(20);
    let mut db = CardDb::load(&dir.join("cards.json")).unwrap();
    db.attach_ratings(&dir);
    let coll = Collection::load(&dir.join("collection.txt")).unwrap();
    let mut rules = Rules::default();
    rules.data_dir = Some(dir.clone());
    let me = (!gauntlet_only).then(|| {
        // A deck file plays that exact list (pass "-" for the colours).
        if args[2].ends_with(".txt") {
            let deck = msh_deckbuilder::deck::Deck::load(std::path::Path::new(args[2]), &db)
                .expect("deck file");
            return Prepared::new(&deck, &db);
        }
        let cmd = db.get(args[2]).expect("commander").clone();
        let colors = ColorSet::parse(args[3]).expect("colours");
        let (deck, _) = analysis::autobuild(&cmd, &db, &coll, &rules, Some(colors), None);
        Prepared::new(&deck, &db)
    });
    let gauntlet = playtest::gauntlet(&db, &rules, 2026);
    std::fs::create_dir_all(&out).unwrap();
    let mut decks = String::new();
    for (k, d) in me.iter().chain(gauntlet.iter()).enumerate() {
        let k = if me.is_some() { k } else { k + 1 };
        decks += &format!(
            "== deck {} ({}) ==\n",
            k,
            if k == 0 { "ours" } else { "league opponent" }
        );
        decks += &format!("commander: {}\n", d.commanders_names().join(" + "));
        for c in &d.cards {
            decks += &format!("{}\n", c.name);
        }
    }
    std::fs::write(out.join("decks.txt"), decks).unwrap();
    let n = gauntlet.len();
    for k in 0..games {
        // Opponents and their seats are drawn at random for each game (seeded).
        let mut picks: Vec<usize> = (1..=n).collect();
        Rng::new(0x5EA7_0000 + k).shuffle(&mut picks);
        let (table, ids, header): (Vec<&Prepared>, Vec<usize>, String) = match &me {
            Some(me) => {
                let mut table: Vec<&Prepared> =
                    picks[..3].iter().map(|&d| &gauntlet[d - 1]).collect();
                let mut ids: Vec<usize> = picks[..3].to_vec();
                let seat = k as usize % 4;
                table.insert(seat, me);
                ids.insert(seat, 0);
                let h = format!(
                    "GAME {} | our deck ({}) in seat {}",
                    k,
                    me.commanders_names().join(" + "),
                    seat
                );
                (table, ids, h)
            }
            None => {
                let ids: Vec<usize> = picks[..4].to_vec();
                let table = ids.iter().map(|&d| &gauntlet[d - 1]).collect();
                (table, ids, format!("GAME {} | league opponents only", k))
            }
        };
        let mut g =
            playtest::state::Game::new_audited(&table, &rules, Rng::new(1000 + k), true, true);
        let winner = playtest::play(&mut g);
        let mut text = format!("{} | decks by seat {:?} (see decks.txt)\n", header, ids);
        for (p, d) in table.iter().enumerate() {
            let c = d.commanders_names().join(" + ");
            text += &format!("seat {} (P{}): deck {} - commander {}\n", p, p, ids[p], c);
        }
        text += "Line format: R<round> ...; P<n> = seat n. '+' enters, '-' leaves the battlefield; \
                 'AI' lines are the AI's reasoning (v = value it puts on an option, c = mana).\n";
        for line in g.log.take().unwrap_or_default() {
            text += &line;
            text.push('\n');
        }
        text += &format!(
            "RESULT: winner seat {} after {} rounds; life {:?}; poison {:?}\n",
            if winner == playtest::NO_WINNER {
                "none (a draw)".to_string()
            } else {
                winner.to_string()
            },
            g.round,
            g.players.iter().map(|p| p.life).collect::<Vec<_>>(),
            g.players.iter().map(|p| p.poison).collect::<Vec<_>>()
        );
        if let Some(s) = &g.stats {
            for e in &s.elims {
                text += &format!(
                    "  P{} out in round {}: {}{}\n",
                    e.seat,
                    e.round,
                    e.cause.label(),
                    e.by.map(|b| format!(" by P{}", b)).unwrap_or_default()
                );
            }
        }
        std::fs::write(out.join(format!("game_{:02}.txt", k)), text).unwrap();
    }
}
