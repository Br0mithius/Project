//! Reviews recorded hand-played games (`<data dir>/games/*.json`): where the player chose
//! differently from the bot, both choices played out from that position.
//!
//! `cargo run --release --example decision_review -- <data dir> [play-outs] [game file]...`
//! `--demo <deck file> <opp deck> <opp deck> <opp deck>`: instead of recorded games, a
//! game by a "player" who skips every other main phase and never attacks - the review
//! should rate those choices below the bot's (a check of the method).
use msh_deckbuilder::db::CardDb;
use msh_deckbuilder::playtest::human::{Answer, Ask, FnIo, Report, Setup, play_with};
use msh_deckbuilder::playtest::review::*;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(a.first().expect("data dir"));
    let mut db = CardDb::load(&dir.join("cards.json")).unwrap();
    db.attach_ratings(&dir);
    let playouts: u32 = a.get(1).and_then(|v| v.parse().ok()).unwrap_or(200);
    let mut records: Vec<GameRecord> = Vec::new();
    if let Some(at) = a.iter().position(|x| x == "--demo") {
        let decks: Vec<(String, String)> = a[at + 1..at + 5]
            .iter()
            .map(|f| (f.clone(), std::fs::read_to_string(f).expect("deck file")))
            .collect();
        let setup = Setup {
            seed: 4242,
            seat: 0,
            opts: Default::default(),
            rules: msh_deckbuilder::league::app_rules(&dir, None),
            decks,
        };
        let mut mains = 0u32;
        let io = FnIo::new(move |ask, _| match ask {
            Ask::Act { window: false, .. } => {
                mains += 1;
                if mains % 2 == 0 {
                    Answer::Pass
                } else {
                    Answer::Auto
                }
            }
            Ask::Attack { .. } => Answer::Attacks(Vec::new()),
            _ => Answer::Auto,
        });
        let (winner, answers, _) = play_with(&setup, &db, Box::new(io));
        records.push(GameRecord {
            version: RECORD_VERSION,
            player: "demo".into(),
            date: String::new(),
            app: env!("CARGO_PKG_VERSION").into(),
            sim: 0,
            winner,
            report: Report {
                note: String::new(),
                setup,
                answers,
            },
        });
    } else {
        let files: Vec<std::path::PathBuf> = if a.len() > 2 {
            a[2..].iter().map(std::path::PathBuf::from).collect()
        } else {
            let mut v: Vec<_> = std::fs::read_dir(dir.join("games"))
                .map(|d| d.flatten().map(|e| e.path()).collect())
                .unwrap_or_default();
            v.sort();
            v
        };
        for f in files {
            match std::fs::read_to_string(&f)
                .ok()
                .and_then(|t| serde_json::from_str::<GameRecord>(&t).ok())
            {
                Some(r) => records.push(r),
                None => eprintln!("skipped {} (not a game record)", f.display()),
            }
        }
    }
    let mut all = Vec::new();
    for rec in &records {
        if rec.edited() {
            eprintln!("skipped a game by {} (sandbox edits were used)", rec.player);
            continue;
        }
        let t0 = std::time::Instant::now();
        let decisions = review(rec, &db, playouts, &ReviewProgress::default());
        println!(
            "== {} {} ({}; {} answers, {} decisions differed, {:.0} s)",
            rec.player,
            rec.date,
            if rec.won() { "won" } else { "lost" },
            rec.report.answers.len(),
            decisions.len(),
            t0.elapsed().as_secs_f32()
        );
        // The ones worth reading: the player's choice wasn't the best by a clear margin, or
        // another choice wins the same game.
        for d in &decisions {
            let flip = d.player().is_some_and(|p| !p.same_game_win)
                && d.choices.iter().any(|c| c.same_game_win);
            if d.left_on_table() >= 10.0 || flip {
                print!("  {}", decision_text(d));
            }
        }
        all.push(Reviewed {
            player: rec.player.clone(),
            date: rec.date.clone(),
            won: rec.won(),
            decisions,
        });
    }
    println!("\n{}", summary(&all));
}
