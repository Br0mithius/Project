//! Replays a Play-tab bug report to the moment it was saved:
//! `cargo run --release --example replay_bug -- <data dir> <report file> [log lines]`
//! The report holds the seed, the options, every deck and every answer the player gave, so
//! the same game is played again (same shuffles, same AI choices) up to the question that
//! was pending when the bug was reported. Prints the player's note, the last `log lines` of
//! the log (default 60), the pending question and the full state dump; a crash in the engine
//! is reported with its message.
use msh_deckbuilder::db::CardDb;
use msh_deckbuilder::playtest::human::{parse_report, replay};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(&args[1]);
    let text = std::fs::read_to_string(&args[2]).expect("report file");
    let tail: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(60);
    let report = parse_report(&text).expect("no replay data in that file");
    let db = CardDb::load(&dir.join("cards.json")).unwrap();
    let r = replay(&report, &db);
    println!("== Note\n{}\n", report.note);
    println!(
        "== Replayed {} answers (seed {}): {} log lines",
        report.answers.len(),
        report.setup.seed,
        r.log.len()
    );
    for l in r.log.iter().skip(r.log.len().saturating_sub(tail)) {
        println!("{}", l);
    }
    if let Some(c) = &r.crash {
        println!("\n== The engine crashed: {}", c);
    }
    match (&r.dump, r.asks.last()) {
        (Some(d), Some(a)) => println!("\n== Pending question\n{:?}\n\n== State\n{}", a, d),
        _ => println!("\n== The game ended before the answers ran out"),
    }
}
