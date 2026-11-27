//! Why a card is or isn't in an auto-build: the colours the build plays, and each named
//! card's availability, colour fit and score against the built deck.
//! `cargo run --release --example build_debug -- <data dir> "<commander>" <card>...`
use msh_deckbuilder::db::{CardDb, Collection};
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(&a[0]);
    let mut db = CardDb::load(&dir.join("cards.json")).unwrap();
    db.attach_ratings(&dir);
    let rules = msh_deckbuilder::league::app_rules(&dir, None);
    let coll = Collection::load(&dir.join("collection.txt")).unwrap();
    let cmd = db.get(&a[1]).unwrap().clone();
    let plays = msh_deckbuilder::analysis::best_colors(&cmd, &db, &coll, &rules);
    println!("best_colors: {:?}", plays);
    let (deck, _, arch) =
        msh_deckbuilder::archetypes::autobuild(&cmd, &db, &coll, &rules, None, None);
    println!("archetype: {:?}", arch.map(|x| (x.name, x.colors)));
    let weak = msh_deckbuilder::analysis::weakest(&deck, &db, &rules);
    println!(
        "weakest in deck: {}",
        weak.iter()
            .take(6)
            .map(|w| format!("{} {:.2}", w.card.name, w.score))
            .collect::<Vec<_>>()
            .join(" | ")
    );
    for n in &a[2..] {
        let c = db.get(n).unwrap();
        println!(
            "{}: in deck {} | available {} | playable_with(best) {} | score {:.2} | job-safe cut of weakest? {}",
            n,
            deck.count_of(&c.name),
            msh_deckbuilder::analysis::available(c, &deck, &coll, &rules),
            c.playable_with(plays),
            msh_deckbuilder::analysis::score_card(c, &deck, &db, &rules).0,
            weak.iter()
                .take(6)
                .map(|w| format!(
                    "{}:{}",
                    w.card.name,
                    msh_deckbuilder::cohesion::swap_keeps_quotas(
                        &msh_deckbuilder::cohesion::build_quotas(),
                        &deck,
                        &db,
                        &w.card.name,
                        &c.name,
                        1
                    )
                ))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
}
