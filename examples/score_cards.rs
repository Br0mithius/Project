//! Scores cards for a commander's auto-built deck:
//! `cargo run --release --example score_cards -- <data dir> "<commander>" "<card>" ["<card>"...]`
use msh_deckbuilder::{analysis, db::*};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(&args[1]);
    let db = CardDb::load(&dir.join("cards.json")).unwrap();
    let coll = Collection::load(&dir.join("collection.txt")).unwrap();
    let rules = Rules::default();
    let cmd = db.get(&args[2]).expect("commander");
    let (deck, _) = analysis::autobuild(cmd, &db, &coll, &rules, None, None);
    for name in &args[3..] {
        let card = db.get(name).expect("card");
        let mut d = deck.clone();
        d.remove(name, 1);
        let (s, why) = analysis::score_card(card, &d, &db, &rules);
        println!("{:>5.1}  {:<28} {}", s, name, why.join("; "));
    }
}
