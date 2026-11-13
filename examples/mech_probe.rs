//! Prints how the deck builder reads cards for each mechanic in `analysis::MECHANICS` (payoff
//! and enabler strength) and their keywords:
//! `cargo run --release --example mech_probe -- <data dir> "<card>"...`
use msh_deckbuilder::analysis;
use msh_deckbuilder::db::CardDb;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let db = CardDb::load(&std::path::PathBuf::from(&a[1]).join("cards.json")).unwrap();
    for name in &a[2..] {
        let c = db.get(name).expect(name);
        let mut out = Vec::new();
        for n in analysis::mechanic_needs() {
            let p = analysis::mechanic_payoff(n, c).unwrap();
            let f = analysis::mechanic_feed(n, c).unwrap();
            if p > 0.0 || f > 0.0 {
                out.push(format!("{}[p{:.1} f{:.1}]", n, p, f));
            }
        }
        println!(
            "{} | kw {:?} | {}",
            c.name,
            c.parsed().keywords,
            out.join("; ")
        );
    }
}
