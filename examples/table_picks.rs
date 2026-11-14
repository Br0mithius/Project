//! What each real opponent's trade picks change in their deck: the picks that fit their
//! colours and go in (assumed played), and the ones left out.
//!
//! `cargo run --release --example table_picks -- <data dir> [out dir]`: with an out dir,
//! each opponent's deck with the picks in is written there as `<player>.txt`.

use msh_deckbuilder::db::CardDb;
use msh_deckbuilder::table::Table;
use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "data".into()));
    let db = CardDb::load(&dir.join("cards.json")).expect("cards.json");
    let table = Table::load(&dir);
    let out = std::env::args().nth(2).map(PathBuf::from);
    for o in &table.opponents {
        let t = std::time::Instant::now();
        let Ok(base) = o.playable(&db) else {
            println!("{}: not playable", o.player);
            continue;
        };
        let (merged, added) = table.with_picks(o, base.clone(), &db);
        if let Some(out) = &out {
            let _ = std::fs::create_dir_all(out);
            let _ = std::fs::write(out.join(format!("{}.txt", o.player)), merged.to_text());
        }
        let picks = table.picks_of(&o.player);
        let already: Vec<&str> = picks
            .iter()
            .map(|p| p.card.as_str())
            .filter(|c| base.count_of(c) > 0)
            .collect();
        println!(
            "{} ({}): {} picks tracked · {} already in the list · {} added: {} ({:.1}s)",
            o.player,
            base.commander.clone().unwrap_or_default(),
            picks.len(),
            already.len(),
            added.len(),
            added.join(", "),
            t.elapsed().as_secs_f32()
        );
    }
}
