//! Where does the bot's own choice lose? Bot-against-bot games with one seat recorded;
//! sampled decision points of that seat, every choice tried with the bot playing on
//! (reshuffled libraries, the same luck for every choice). A quick screen, then the points
//! where some choice beats the bot's clearly are played out again on other luck.
//!
//! `cargo run --release --example bot_regret -- <data dir> <games> <out.tsv> <deck file>
//! <opp deck> <opp deck> <opp deck> [...more opponent decks]`
//! Env: `POINTS` per game (default 8), `SCREEN` play-outs (24), `GAP` points (15),
//! `CONFIRM` play-outs (160), `SEED`.
use msh_deckbuilder::db::CardDb;
use msh_deckbuilder::playtest::human::{Answer, FnIo, Report, Setup, play_with};
use msh_deckbuilder::playtest::review::*;
use std::io::Write;

fn env(k: &str, d: u64) -> u64 {
    std::env::var(k)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(d)
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(&a[0]);
    let games: u64 = a[1].parse().expect("games");
    let mut out = std::fs::File::create(&a[2]).expect("out file");
    let mut db = CardDb::load(&dir.join("cards.json")).unwrap();
    db.attach_ratings(&dir);
    let read = |f: &String| (f.clone(), std::fs::read_to_string(f).expect("deck file"));
    let me = read(&a[3]);
    let opps: Vec<(String, String)> = a[4..].iter().map(read).collect();
    let rules = msh_deckbuilder::league::app_rules(&dir, None);
    let (points, screen, gap, confirm, seed0) = (
        env("POINTS", 8) as usize,
        env("SCREEN", 24) as u32,
        env("GAP", 15) as f32,
        env("CONFIRM", 160) as u32,
        env("SEED", 9000),
    );
    writeln!(
        out,
        "game\tseat\tround\tkind\twhat\tbot_pct\tbest_pct\tgap\tse\tplayouts\tbot_turn\tbest_choice\tbest_turn"
    )
    .unwrap();
    let t0 = std::time::Instant::now();
    let mut found = 0;
    for g in 0..games {
        // Rotate the opponents and our seat.
        let seat = (g % 4) as usize;
        let mut decks: Vec<(String, String)> = (0..3)
            .map(|i| opps[(g as usize + i) % opps.len()].clone())
            .collect();
        decks.insert(seat, me.clone());
        let setup = Setup {
            seed: seed0 + g,
            seat,
            opts: Default::default(),
            rules: rules.clone(),
            decks,
        };
        let (winner, answers, _) = play_with(&setup, &db, Box::new(FnIo::new(|_, _| Answer::Auto)));
        let rec = GameRecord {
            version: RECORD_VERSION,
            player: "bot".into(),
            date: String::new(),
            app: String::new(),
            sim: 0,
            winner,
            report: Report {
                note: String::new(),
                setup,
                answers,
            },
        };
        let ds = review_with(
            &rec,
            &db,
            screen,
            Some((points, seed0 + g)),
            Some((gap, confirm)),
            &ReviewProgress::default(),
        );
        for d in ds {
            let (Some(bot), Some(best)) = (d.bot(), d.best()) else {
                continue;
            };
            let n = d.playouts.max(1) as f32;
            // Paired difference best - bot isn't kept per play-out: an unpaired bound.
            let (pb, pm) = (bot.wins as f32 / n, best.wins as f32 / n);
            let se = ((pb * (1.0 - pb) + pm * (1.0 - pm)) / n).sqrt();
            writeln!(
                out,
                "{}\t{}\t{}\t{:?}\t{}\t{:.0}\t{:.0}\t{:.0}\t{:.0}\t{}\t{}\t{}\t{}",
                g,
                seat,
                d.round,
                d.kind,
                d.what,
                100.0 * pb,
                100.0 * pm,
                100.0 * (pm - pb),
                100.0 * se,
                d.playouts,
                bot.then,
                best.label,
                best.then
            )
            .unwrap();
            found += 1;
        }
        out.flush().unwrap();
        if (g + 1) % 10 == 0 {
            eprintln!(
                "{} games, {} confirmed points, {:.0} s",
                g + 1,
                found,
                t0.elapsed().as_secs_f32()
            );
        }
    }
}
