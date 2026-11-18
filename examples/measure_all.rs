//! Builds the shipped card-power table (`data/sim_card_values_default.tsv`): every spell of
//! each given deck measured against a vanilla stand-in of its cost, on paired games against
//! the simulated league, plus (with `--league`) every simulated league deck's own cards
//! against the rest of the league. Progress is saved after each deck, and decks already in
//! the output are skipped, so a stopped run picks up where it left off.
//!
//! `cargo run --release --example measure_all -- <data dir> <games> <out dir> [--league]
//! <deck file>...`  (the table is written to `<out dir>/sim_card_values.tsv`)

use msh_deckbuilder::db::CardDb;
use msh_deckbuilder::deck::Deck;
use msh_deckbuilder::playtest::Prepared;
use msh_deckbuilder::playtest::simvalues::{self, SimValues};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = PathBuf::from(&args[0]);
    let games: u32 = args[1].parse().expect("games");
    let out = PathBuf::from(&args[2]);
    let league = args.iter().any(|a| a == "--league");
    let files: Vec<&String> = args[3..].iter().filter(|a| !a.starts_with("--")).collect();
    std::fs::create_dir_all(&out).unwrap();
    let mut db = CardDb::load(&data.join("cards.json")).expect("cards.json");
    db.attach_ratings(&data);
    let rules = msh_deckbuilder::league::app_rules(&data, None);
    let gauntlet = msh_deckbuilder::ranking::TestBed::new(&db, &rules).gauntlet;
    let mut table = SimValues::load_own(&out);
    let done_decks = table.decks();
    let (done, cancel) = (AtomicUsize::new(0), AtomicBool::new(false));
    let t0 = std::time::Instant::now();
    for (k, f) in files.iter().enumerate() {
        let text = std::fs::read_to_string(f).expect("deck file");
        let deck = Deck::from_text("deck", &text, &db);
        let Some(cmd) = deck.commander.clone() else {
            eprintln!("{}: no commander, skipped", f);
            continue;
        };
        let stem = PathBuf::from(f.as_str())
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = format!("{} ({})", cmd, stem);
        if done_decks.contains_key(&name) {
            println!("[{}/{}] {} - already measured", k + 1, files.len(), name);
            continue;
        }
        let p = Prepared::new(&deck, &db);
        let t = std::time::Instant::now();
        let ms = simvalues::measure(
            &name,
            &p,
            &[],
            &gauntlet,
            &rules,
            games,
            0xC0FFEE ^ (k as u64 * 104729),
            &done,
            &cancel,
        );
        println!(
            "[{}/{}] {}: {} cards in {:.0}s (total {:.0} min)",
            k + 1,
            files.len(),
            name,
            ms.len(),
            t.elapsed().as_secs_f32(),
            t0.elapsed().as_secs_f32() / 60.0
        );
        table.add(ms);
        table.save(&out).expect("save");
    }
    if league {
        let have = table.decks();
        if have.keys().any(|d| d.starts_with("league: ")) {
            println!("league decks already measured");
        } else {
            let ms = simvalues::measure_league(&gauntlet, &rules, games, 0xBEEF, &done, &cancel);
            println!(
                "league: {} measurements ({:.0} min)",
                ms.len(),
                t0.elapsed().as_secs_f32() / 60.0
            );
            table.add(ms);
            table.save(&out).expect("save");
        }
    }
    let all = table.all();
    println!(
        "{} cards measured over {} decks.",
        all.len(),
        table.decks().len()
    );
}
