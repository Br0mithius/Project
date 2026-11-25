//! Writes each named commander's strong build (`table::strong_build`: auto-build from every
//! league card) to `<out dir>/<n>.txt`, for comparing builder changes on the same games:
//! `cargo run --release --example write_builds -- <data dir> <out dir> "<commander>"...`
use msh_deckbuilder::db::{CardDb, Rules};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(&a[0]);
    let out = std::path::PathBuf::from(&a[1]);
    let _ = std::fs::create_dir_all(&out);
    let db = CardDb::load(&dir.join("cards.json")).expect("cards.json");
    let rules = Rules::default();
    for (k, c) in a[2..].iter().enumerate() {
        match msh_deckbuilder::table::strong_build(c, &db, &rules) {
            Ok(d) => {
                let _ = std::fs::write(out.join(format!("{:02}.txt", k)), d.to_text());
            }
            Err(e) => eprintln!("{}: {}", c, e),
        }
    }
}
