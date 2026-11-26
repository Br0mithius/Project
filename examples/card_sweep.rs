//! Plays every mythic and rare in the card database, and every uncommon that does a job for
//! a control plan (`cohesion::jobs`), in a deck: each in place of the weakest cards it may
//! replace (`optimize::test_claims`, two slots tried), screened on `games` games, the top
//! `confirm` measured again on fresh games. Lands (rare and uncommon) are tested apart, each
//! in place of the deck's most common basic. Against the peer field and the data folder's
//! next table, weighted as the optimiser does.
//!
//! `cargo run --release --example card_sweep -- <data dir> <deck file> <out tsv> [games]
//! [confirm] [session]`
use msh_deckbuilder::db::{CardDb, Collection};
use msh_deckbuilder::deck::Deck;
use msh_deckbuilder::lab::Progress;
use msh_deckbuilder::optimize::{DEFAULT_TABLE_WEIGHT, Opponents, compare, test_claims};
use std::fmt::Write;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(&a[0]);
    let mut db = CardDb::load(&dir.join("cards.json")).expect("cards.json");
    db.sim_values = msh_deckbuilder::playtest::simvalues::SimValues::load(&dir);
    let deck = Deck::load(std::path::Path::new(&a[1]), &db).expect("deck");
    let out = &a[2];
    let games: u32 = a.get(3).and_then(|g| g.parse().ok()).unwrap_or(1000);
    let confirm: usize = a.get(4).and_then(|g| g.parse().ok()).unwrap_or(40);
    let rules = msh_deckbuilder::league::app_rules(&dir, a.get(5).and_then(|s| s.parse().ok()));
    let _ = Collection::load(&dir.join("collection.txt"));
    let id = deck.identity(&db).expect("identity");
    let of_interest = |c: &msh_deckbuilder::card::Card| {
        c.identity().is_subset_of(id)
            && match c.rarity.as_str() {
                "mythic" | "rare" => true,
                "uncommon" => c.is_land() || !msh_deckbuilder::cohesion::is_filler(c),
                _ => false,
            }
    };
    let spells: Vec<String> = db
        .cards
        .iter()
        .filter(|c| !c.is_land() && of_interest(c) && deck.count_of(&c.name) == 0)
        .map(|c| c.name.clone())
        .collect();
    let lands: Vec<String> = db
        .cards
        .iter()
        .filter(|c| c.is_land() && !c.is_basic() && of_interest(c) && deck.count_of(&c.name) == 0)
        .map(|c| c.name.clone())
        .collect();
    eprintln!("{} spells and {} lands to test", spells.len(), lands.len());
    let table = msh_deckbuilder::table::Table::load(&dir);
    let opp = Opponents {
        table: msh_deckbuilder::table::gauntlet(&table, &db),
        league: msh_deckbuilder::table::peer_field(&db, &rules),
        table_weight: DEFAULT_TABLE_WEIGHT,
    };
    let mut tsv = String::from(
        "card\trarity\tset\tkind\treplaces\tscreen\tscreen_se\tconfirmed\tconfirmed_se\n",
    );
    let t0 = std::time::Instant::now();
    let r = test_claims(
        &deck,
        &spells,
        &db,
        &rules,
        &opp,
        games,
        confirm,
        &Progress::new(),
    )
    .expect("claims");
    eprintln!(
        "spells done in {:.0} min",
        t0.elapsed().as_secs_f32() / 60.0
    );
    for c in &r.claims {
        let card = db.get(&c.card).unwrap();
        let _ = writeln!(
            tsv,
            "{}\t{}\t{}\tspell\t{}\t{:.2}\t{:.2}\t{}\t{}",
            c.card,
            card.rarity,
            card.set,
            c.cut,
            c.screened.gain * 100.0,
            c.screened.stderr * 100.0,
            c.confirmed
                .map_or(String::new(), |m| format!("{:.2}", m.gain * 100.0)),
            c.confirmed
                .map_or(String::new(), |m| format!("{:.2}", m.stderr * 100.0)),
        );
    }
    std::fs::write(out, &tsv).expect("write");
    // Lands: each in place of the most common basic.
    let basic = deck
        .cards
        .iter()
        .filter(|(n, _)| db.get(n).is_some_and(|c| c.is_basic()))
        .max_by_key(|(_, k)| **k)
        .map(|(n, _)| n.clone())
        .expect("a basic");
    let decks: Vec<Deck> = lands
        .iter()
        .map(|l| {
            let mut d = deck.clone();
            d.remove(&basic, 1);
            d.add(l, 1);
            d
        })
        .collect();
    let g = compare(
        &deck,
        &decks,
        &db,
        &opp,
        &rules,
        games * 3,
        0x1A9D,
        &Progress::new(),
    );
    for (l, m) in lands.iter().zip(g.iter()) {
        let card = db.get(l).unwrap();
        let _ = writeln!(
            tsv,
            "{}\t{}\t{}\tland\t{}\t\t\t{:.2}\t{:.2}",
            l,
            card.rarity,
            card.set,
            basic,
            m.score.gain * 100.0,
            m.score.stderr * 100.0
        );
    }
    std::fs::write(out, &tsv).expect("write");
    eprintln!("all done in {:.0} min", t0.elapsed().as_secs_f32() / 60.0);
}
