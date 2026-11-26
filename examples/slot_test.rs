//! Paired test of cards in one slot: the deck as it is against the deck with `slot` swapped
//! for each candidate, on the same games. Against the peer field and the data folder's next
//! table, weighted as the optimiser does.
//!
//! `cargo run --release --example slot_test -- <data dir> <deck file> <slot card> <games>
//! <candidate>...`
//!
//! A candidate is a card name (it takes the slot card's place) or a list of swaps,
//! `Out>In|Out>In`, for changes to several slots at once. `SEED=<n>` plays another set of
//! games (default 77123).
use msh_deckbuilder::db::CardDb;
use msh_deckbuilder::deck::Deck;
use msh_deckbuilder::lab::Progress;
use msh_deckbuilder::optimize::{DEFAULT_TABLE_WEIGHT, Opponents, compare};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(&a[0]);
    let mut db = CardDb::load(&dir.join("cards.json")).expect("cards.json");
    db.sim_values = msh_deckbuilder::playtest::simvalues::SimValues::load(&dir);
    let deck = Deck::load(std::path::Path::new(&a[1]), &db).expect("deck");
    let slot = &a[2];
    let games: u32 = a[3].parse().expect("games");
    let cands = &a[4..];
    let rules = msh_deckbuilder::league::app_rules(&dir, None);
    let table = msh_deckbuilder::table::Table::load(&dir);
    let opp = Opponents {
        table: msh_deckbuilder::table::gauntlet(&table, &db),
        league: msh_deckbuilder::table::peer_field(&db, &rules),
        table_weight: DEFAULT_TABLE_WEIGHT,
    };
    assert!(deck.count_of(slot) > 0, "{} is not in the deck", slot);
    let decks: Vec<Deck> = cands
        .iter()
        .map(|c| {
            let swaps: Vec<(&str, &str)> = if c.contains('>') {
                c.split('|')
                    .map(|s| s.split_once('>').expect("Out>In"))
                    .collect()
            } else {
                vec![(slot.as_str(), c.as_str())]
            };
            let mut d = deck.clone();
            for (out, inn) in swaps {
                assert!(db.get(inn).is_some(), "unknown card {}", inn);
                assert_eq!(d.remove(out, 1), 1, "{} is not in the deck", out);
                d.add(inn, 1);
            }
            d
        })
        .collect();
    let r = compare(
        &deck,
        &decks,
        &db,
        &opp,
        &rules,
        games,
        std::env::var("SEED")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(77_123),
        &Progress::new(),
    );
    println!(
        "in place of {} ({} games each; points of win rate, + = better than {}):",
        slot, games, slot
    );
    for (c, g) in cands.iter().zip(&r) {
        let part = |m: Option<msh_deckbuilder::playtest::lab::Measure>| {
            m.map_or("-".to_string(), |m| {
                format!("{:+.2} ± {:.2}", m.gain * 100.0, m.stderr * 100.0)
            })
        };
        println!(
            "{:<34} {:+.2} ± {:.2}  (table {}, peer field {})",
            c,
            g.score.gain * 100.0,
            g.score.stderr * 100.0,
            part(g.table),
            part(g.league)
        );
    }
}
