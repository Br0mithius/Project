//! Debug of one `bot_regret` point: `regret_debug <data dir> <game> <round> <phase text>
//! <option label prefix> <fork seed index or -1> <deck> <opps...>` prints the log after the
//! decision for the bot's line and for the option's line.
use msh_deckbuilder::db::CardDb;
use msh_deckbuilder::playtest::human::*;
use std::cell::RefCell;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(&a[0]);
    let g: u64 = a[1].parse().unwrap();
    let round: u32 = a[2].parse().unwrap();
    let (phase, label, fi) = (&a[3], &a[4], a[5].parse::<i64>().unwrap());
    let mut db = CardDb::load(&dir.join("cards.json")).unwrap();
    db.attach_ratings(&dir);
    let read = |f: &String| (f.clone(), std::fs::read_to_string(f).unwrap());
    let me = read(&a[6]);
    let opps: Vec<(String, String)> = a[7..].iter().map(read).collect();
    let seat = (g % 4) as usize;
    let mut decks: Vec<(String, String)> = (0..3)
        .map(|i| opps[(g as usize + i) % opps.len()].clone())
        .collect();
    decks.insert(seat, me);
    let setup = Setup {
        seed: 9000 + g,
        seat,
        opts: Default::default(),
        rules: msh_deckbuilder::league::app_rules(&dir, None),
        decks,
    };
    let run = |answers: Vec<Answer>, fork: Option<(usize, u64)>| {
        let io = std::rc::Rc::new(ScriptIo {
            answers: RefCell::new(answers.into()),
            ..Default::default()
        });
        let (w, _, _) = play_forked(&setup, &db, Box::new(SharedIo(io.clone())), fork);
        let r = (
            io.asks.borrow().clone(),
            io.log.borrow().clone(),
            io.log_at.borrow().clone(),
            w,
        );
        r
    };
    let (asks, log, log_at, w) = run(Vec::new(), None);
    println!("actual winner {} (seat {})", w, seat);
    let k = (0..asks.len())
        .find(|&k| {
            let r = log
                .get(log_at[k].saturating_sub(1))
                .and_then(|l| l.split_whitespace().next())
                .and_then(|r| r.strip_prefix('R'))
                .and_then(|r| r.parse::<u32>().ok())
                .unwrap_or(0);
            r == round && matches!(&asks[k], Ask::Act { phase: p, .. } if p == phase)
        })
        .expect("no such point");
    let Ask::Act { actions, .. } = &asks[k] else {
        unreachable!()
    };
    for (i, x) in actions.iter().enumerate() {
        println!(
            "  option {}: {} payable={} auto_ok={}",
            i, x.label, x.payable, x.auto_ok
        );
    }
    let i = actions
        .iter()
        .position(|x| x.label.starts_with(label.as_str()))
        .expect("label");
    if fi == -2 {
        let mut wins = [0u32; 2];
        let mut both = [[0u32; 2]; 2];
        for f in 0..60u64 {
            let fork = Some((
                k,
                0x5EED_0000 + 0x00C0_FFEE_0000 + f * 7919 + k as u64 * 104_729,
            ));
            let mut r = [false; 2];
            for (j, ans) in [Answer::Auto, Answer::AutoTap(i)].into_iter().enumerate() {
                let mut v = vec![Answer::Auto; k];
                v.push(ans);
                r[j] = run(v, fork).3 == seat;
                wins[j] += r[j] as u32;
            }
            both[r[0] as usize][r[1] as usize] += 1;
        }
        println!(
            "of 60: bot line wins {}, option line wins {}; table {:?}",
            wins[0], wins[1], both
        );
        return;
    }
    let fork = (fi >= 0).then(|| {
        (
            k,
            0x5EED_0000 + 0x00C0_FFEE_0000 + fi as u64 * 7919 + k as u64 * 104_729,
        )
    });
    for (name, ans) in [("BOT", Answer::Auto), ("OPTION", Answer::AutoTap(i))] {
        let mut v = vec![Answer::Auto; k];
        v.push(ans);
        let (_, l, _, w) = run(v, fork);
        println!("\n===== {} line: winner {} =====", name, w);
        for line in l.iter().skip(log_at[k].saturating_sub(3)).take(90) {
            println!("{}", &line[..line.len().min(230)]);
        }
        println!("... {}", l.last().map_or("", |s| s.as_str()));
    }
}
