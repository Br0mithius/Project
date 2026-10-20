//! Lists creatures by value for their mana cost:
//! `cargo run --release --example creature_values -- <data dir> [--pool] [--top N]`
//! (`--pool` = only cards in collection.txt).
use msh_deckbuilder::{db::*, value};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(args.get(1).expect("data dir"));
    let db = CardDb::load(&dir.join("cards.json")).unwrap();
    let coll = Collection::load(&dir.join("collection.txt")).unwrap();
    let pool = args.iter().any(|a| a == "--pool");
    let top: usize = args
        .iter()
        .position(|a| a == "--top")
        .and_then(|i| args.get(i + 1))
        .and_then(|n| n.parse().ok())
        .unwrap_or(25);
    let mut rows: Vec<(f32, String, value::CreatureValue)> = db
        .cards
        .iter()
        .filter(|c| c.is_creature() && (!pool || coll.owned(&c.name) > 0))
        .map(|c| {
            let v = value::creature_value(c);
            (value::creature_points(&v), c.name.clone(), v)
        })
        .collect();
    rows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    let show = |r: &(f32, String, value::CreatureValue)| {
        let parts: Vec<String> =
            r.2.parts
                .iter()
                .take(4)
                .map(|(l, v)| format!("{} {:+.1}", l, v))
                .collect();
        println!(
            "{:>+5.1}  {:<40} {:>4.0}% of fair ({:.1}/{:.1})  {}",
            r.0,
            r.1,
            r.2.efficiency * 100.0,
            r.2.total,
            r.2.fair,
            parts.join(", ")
        );
    };
    println!("Best {} creatures:", top);
    rows.iter().take(top).for_each(show);
    println!("\nWorst {}:", top.min(10));
    rows.iter().rev().take(top.min(10)).for_each(show);
}
