//! Builds one commander in several colour sets and playtests each against the same
//! simulated league decks:
//! `cargo run --release --example compare_builds -- <data dir> "<commander>" WBR WUBRG ... [--session N]`
//! The league is the one the app uses for that data folder (rules.json or the upcoming session,
//! with the imported pool, pins and cached season).
//! Prints win rate, keepable hands, coloured sources per colour, and how often a "look at
//! the top seven for a Hero, Equipment or Vehicle" (Nick Fury's power-up) would hit.
use msh_deckbuilder::{analysis, card::ColorSet, db::*, playtest, ranking};

fn choose(n: u64, k: u64) -> f64 {
    if k > n {
        return 0.0;
    }
    (0..k).fold(1.0, |acc, i| acc * (n - i) as f64 / (i + 1) as f64)
}

fn main() {
    let all: Vec<String> = std::env::args().collect();
    let session = msh_deckbuilder::league::session_arg(&all);
    let args: Vec<&String> = all
        .iter()
        .enumerate()
        .filter(|(i, a)| !a.starts_with("--") && !(*i > 0 && all[i - 1] == "--session"))
        .map(|(_, a)| a)
        .collect();
    let dir = std::path::PathBuf::from(args[1]);
    let mut db = CardDb::load(&dir.join("cards.json")).unwrap();
    db.attach_ratings(&dir);
    let coll = Collection::load(&dir.join("collection.txt")).unwrap();
    let rules = msh_deckbuilder::league::app_rules(&dir, session);
    println!(
        "{}",
        msh_deckbuilder::opponents::league_header(&db, &rules, 2026)
    );
    let cmd = db.get(args[2]).expect("commander").clone();
    let bed = ranking::TestBed::new(&db, &rules);
    // More games for closer calls: COMPARE_GAMES=4800.
    let games: u32 = std::env::var("COMPARE_GAMES")
        .ok()
        .and_then(|g| g.parse().ok())
        .unwrap_or(1200);
    for colors in &args[3..] {
        let set = ColorSet::parse(colors).expect("colours like WUBRG");
        let (deck, short) = analysis::autobuild(&cmd, &db, &coll, &rules, Some(set), None);
        let st = analysis::stats(&deck, &db);
        let sim = analysis::simulate(&deck, &db, 5000);
        let wr = playtest::win_rate(&deck, &bed.gauntlet, &db, &rules, games, 7);
        // Hero / Equipment / Vehicle cards in the library.
        let hits: u32 = deck
            .cards
            .iter()
            .filter_map(|(n, k)| db.get(n).map(|c| (c, *k)))
            .filter(|(c, _)| {
                let t = c.type_line.to_lowercase();
                t.contains("hero") || t.contains("equipment") || t.contains("vehicle")
            })
            .map(|(_, k)| k)
            .sum();
        let lib = deck.main_count() as u64;
        let miss = choose(lib - hits as u64, 7) / choose(lib, 7);
        println!(
            "{:<6} won {:>4.1}% of {} games | keepable {:.0}% | commander on time {:.0}% | short {} | sources W{} U{} B{} R{} G{} | pips W{} U{} B{} R{} G{} | Hero/Equip/Vehicle {} -> top-7 hit {:.0}%",
            colors,
            wr * 100.0,
            games,
            sim.keepable * 100.0,
            sim.commander_on_curve.map(|(_, p)| p).unwrap_or(0.0) * 100.0,
            short,
            st.sources[0],
            st.sources[1],
            st.sources[2],
            st.sources[3],
            st.sources[4],
            st.pips[0],
            st.pips[1],
            st.pips[2],
            st.pips[3],
            st.pips[4],
            hits,
            (1.0 - miss) * 100.0
        );
    }
}
