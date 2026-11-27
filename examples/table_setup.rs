//! Rebuilds My table (`<data>/table.json`): each "Player=Commander" gets the strong
//! auto-build of their commander (every league card available), and every player's trade
//! picks come from a Mana Mixer league overview JSON (its "results" section).
//!
//! `cargo run --release --example table_setup -- <data dir> <league overview json>
//! "Drake=Tatyova, Benthic Druid" "Will=Smaug, Wicked Worm" ...`
//! Players named with a leading `*` sit at the next table. `--realistic`: built from a pool a
//! player could own (every common and uncommon, sampled rares and mythics, their picks,
//! without what they traded away) instead of every league card. `Commander@WBRG` limits the deck to those colours. `+Player=Commander` keeps
//! the list they have and only replaces cards they traded away.

use msh_deckbuilder::db::CardDb;
use msh_deckbuilder::table::{RealOpponent, Table, picks_from_league, strong_build};
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = PathBuf::from(&args[0]);
    let mut db = CardDb::load(&dir.join("cards.json")).expect("cards.json");
    // The measured card values and ratings, as the app loads them.
    db.attach_ratings(&dir);
    let json = std::fs::read_to_string(&args[1]).expect("league json");
    let mut table = Table::load(&dir);
    table.picks = picks_from_league(&json).expect("picks");
    let rules = msh_deckbuilder::league::app_rules(&dir, None);
    // Cards each player traded away (theirs no longer), from the overview's "tradeIns".
    let traded: std::collections::BTreeMap<String, Vec<String>> = serde_json::from_str::<
        serde_json::Value,
    >(&json)
    .ok()
    .and_then(|v| {
        let t = v
            .get("results")
            .unwrap_or(&v)
            .get("tradeIns")?
            .as_object()?
            .clone();
        Some(
            t.into_iter()
                .map(|(who, list)| {
                    let names = list
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.get("cardName")?.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    (who, names)
                })
                .collect(),
        )
    })
    .unwrap_or_default();
    let realistic = args.iter().any(|a| a == "--realistic");
    for spec in args[2..].iter().filter(|a| !a.starts_with("--")) {
        let (who, cmd) = spec.split_once('=').expect("Player=Commander");
        // `Commander@WBRG`: the colours they play (fewer than the commander's identity).
        let (cmd, colors) = match cmd.split_once('@') {
            Some((c, cols)) => (
                c,
                Some(
                    cols.chars()
                        .fold(msh_deckbuilder::card::ColorSet(0), |a, ch| {
                            a.union(msh_deckbuilder::card::ColorSet::from_char(ch))
                        }),
                ),
            ),
            None => (cmd, None),
        };
        let (next, who) = match who.strip_prefix('*') {
            Some(w) => (true, w),
            None => (false, who),
        };
        // `+Player=Commander`: keep the list they have (tuned, or pasted from the league),
        // only replacing cards they have since traded away.
        let (keep, who) = match who.strip_prefix('+') {
            Some(w) => (true, w),
            None => (false, who),
        };
        let gone: Vec<&String> = traded
            .iter()
            .filter(|(p, _)| p.eq_ignore_ascii_case(who))
            .flat_map(|(_, v)| v.iter())
            .collect();
        let picked: Vec<String> = table.picks_of(who).iter().map(|p| p.card.clone()).collect();
        // A card traded away and not picked again is no longer theirs.
        let lost = |n: &str| gone.iter().any(|g| g.as_str() == n) && !picked.iter().any(|p| p == n);
        let cmd_card = db.get(cmd).expect("commander").clone();
        // What they could own: every common and uncommon, a sample of rares and mythics
        // (as the peer field), their picks - without what they traded away.
        let mut pool = msh_deckbuilder::table::realistic_pool(
            &cmd_card,
            &db,
            66,
            14,
            msh_deckbuilder::table::name_seed(who),
        );
        for p in &picked {
            if let Some(c) = db.get(p) {
                pool.counts.insert(c.name.clone(), 1);
            }
        }
        pool.counts.retain(|n, _| !lost(n));
        let existing = table
            .opponents
            .iter()
            .find(|o| o.player.eq_ignore_ascii_case(who))
            .cloned();
        let (deck, was_next) = match (&existing, keep) {
            (Some(o), true) => {
                let mut d = o.playable(&db).expect("their list");
                let out: Vec<String> = d.cards.keys().filter(|n| lost(n)).cloned().collect();
                for n in &out {
                    let k = d.remove(n, 99);
                    for _ in 0..k {
                        let add = msh_deckbuilder::analysis::suggestions_within(
                            &d, &db, &pool, &rules, None,
                        )
                        .into_iter()
                        .find(|s| !s.card.is_land())
                        .map(|s| s.card.name.clone());
                        match add {
                            Some(a) => d.add(&a, 1),
                            None => break,
                        }
                    }
                }
                msh_deckbuilder::analysis::fill_basics(&mut d, &db, &rules);
                println!(
                    "{}: kept their list, replaced {} traded-away card(s): {}",
                    who,
                    out.len(),
                    out.join(", ")
                );
                (d, o.at_next_table)
            }
            _ if realistic => {
                // Only their colours' cards (and colourless) are theirs to play.
                if let Some(cs) = colors {
                    pool.counts
                        .retain(|n, _| db.get(n).is_none_or(|c| c.identity().is_subset_of(cs)));
                }
                let (d, _, _) = msh_deckbuilder::archetypes::autobuild(
                    &cmd_card, &db, &pool, &rules, colors, None,
                );
                (d, false)
            }
            _ => (strong_build(cmd, &db, &rules).expect("commander"), false),
        };
        table
            .opponents
            .retain(|o| !o.player.eq_ignore_ascii_case(who));
        table.opponents.push(RealOpponent {
            player: who.to_string(),
            commander: cmd.to_string(),
            decklist: deck.to_text(),
            at_next_table: next || was_next,
        });
        println!("{}: {} ({} cards)", who, cmd, deck.main_count());
    }
    table.save(&dir).expect("save");
    println!(
        "Saved table.json: {} opponents, picks for {} players.",
        table.opponents.len(),
        table.picks.len()
    );
}
