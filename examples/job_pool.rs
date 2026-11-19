//! Lists the cards in a pool by job (filler last): `job_pool <data dir>`.
use msh_deckbuilder::db::{CardDb, Collection};
fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("data dir"));
    let db = CardDb::load(&dir.join("cards.json")).unwrap();
    let coll = Collection::load(&dir.join("collection.txt")).unwrap();
    let (mut jobs, mut filler) = (Vec::new(), Vec::new());
    for (name, n) in &coll.counts {
        let Some(c) = db.get(name) else { continue };
        if c.is_land() {
            continue;
        }
        let j = msh_deckbuilder::cohesion::jobs(c);
        if j.is_empty() {
            filler.push(format!("{} x{}", c.name, n));
        } else {
            jobs.push(format!(
                "{} x{} [{}]",
                c.name,
                n,
                j.iter().map(|j| j.label()).collect::<Vec<_>>().join(", ")
            ));
        }
    }
    println!("JOB CARDS ({}):\n  {}", jobs.len(), jobs.join("\n  "));
    println!("FILLER ({}):\n  {}", filler.len(), filler.join("\n  "));
}
