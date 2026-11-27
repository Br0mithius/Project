//! Rebuilds My table (`<data>/table.json`): each "Player=Commander" gets the strong
//! auto-build of their commander (every league card available), and every player's trade
//! picks come from a Mana Mixer league overview JSON (its "results" section).
//!
//! `cargo run --release --example table_setup -- <data dir> <league overview json>
//! "Drake=Tatyova, Benthic Druid" "Will=Smaug, Wicked Worm" ...`
//! Players named with a leading `*` sit at the next table.

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
    for spec in &args[2..] {
        let (who, cmd) = spec.split_once('=').expect("Player=Commander");
        let (next, who) = match who.strip_prefix('*') {
            Some(w) => (true, w),
            None => (false, who),
        };
        let deck = strong_build(cmd, &db, &rules).expect("commander");
        table
            .opponents
            .retain(|o| !o.player.eq_ignore_ascii_case(who));
        table.opponents.push(RealOpponent {
            player: who.to_string(),
            commander: cmd.to_string(),
            decklist: deck.to_text(),
            at_next_table: next,
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
