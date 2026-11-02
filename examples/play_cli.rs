//! Plays the Play tab's human seat without a window, to check that games finish:
//! `cargo run --release --example play_cli -- <data dir> <deck file> [games] [--fuzz] [--seed N]`
//! Without `--fuzz` every question is answered "let the bot decide"; with it, by random
//! (mostly legal) answers - tapping lands, casting what the pool pays, random targets,
//! attacks and blocks - to shake out crashes and hangs in the hand-played paths. Prints
//! each game's winner, the number of questions, and the answers' count; a panic is reported
//! with the seed and the answers so far (as a bug report would be).
use msh_deckbuilder::db::CardDb;
use msh_deckbuilder::deck::Deck;
use msh_deckbuilder::playtest::human::*;
use std::cell::{Cell, RefCell};

struct Fuzz {
    state: Cell<u64>,
    view: RefCell<Option<GameView>>,
    ask: RefCell<Option<Ask>>,
    asks: Cell<usize>,
}

impl Fuzz {
    fn rnd(&self, n: usize) -> usize {
        let mut x = self.state.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state.set(x);
        if n == 0 { 0 } else { (x % n as u64) as usize }
    }
    fn tap(&self) -> Option<Answer> {
        let v = self.view.borrow();
        let v = v.as_ref()?;
        let s = v.sources.get(self.rnd(v.sources.len().max(1)))?;
        let colors: Vec<u8> = (0..6).filter(|&c| s.colors & (1 << c) != 0).collect();
        let c = *colors.get(self.rnd(colors.len()))?;
        Some(Answer::Tap { id: s.id, color: c })
    }
}

impl HumanIo for Fuzz {
    fn send(&self, msg: FromEngine) {
        if let FromEngine::Ask { ask, view, .. } = msg {
            *self.view.borrow_mut() = Some(*view);
            *self.ask.borrow_mut() = Some(ask);
            self.asks.set(self.asks.get() + 1);
        }
    }
    fn recv(&self) -> ToEngine {
        let ask = self.ask.borrow().clone();
        let a = match ask {
            None => Answer::Auto,
            Some(Ask::Mulligan { .. }) => Answer::Yes(self.rnd(4) > 0),
            Some(Ask::Act { actions, .. }) => {
                let r = self.rnd(10);
                if r < 3 {
                    self.tap().unwrap_or(Answer::Pass)
                } else if r < 7 && !actions.is_empty() {
                    let k = self.rnd(actions.len());
                    if actions[k].payable {
                        Answer::Pick(vec![k])
                    } else {
                        Answer::AutoTap(k)
                    }
                } else if r < 8 {
                    Answer::Auto
                } else {
                    Answer::Pass
                }
            }
            Some(Ask::Pick {
                options, min, max, ..
            }) => {
                let n = min + self.rnd(max.saturating_sub(min) + 1);
                let mut v: Vec<usize> = (0..options.len()).collect();
                let mut out = Vec::new();
                for _ in 0..n.min(v.len()) {
                    out.push(v.remove(self.rnd(v.len())));
                }
                Answer::Pick(out)
            }
            Some(Ask::YesNo { .. }) => Answer::Yes(self.rnd(2) == 0),
            Some(Ask::Number { min, max, .. }) => {
                Answer::Num(min + self.rnd((max - min).max(0) as usize + 1) as i32)
            }
            Some(Ask::Attack {
                attackers,
                defenders,
                ..
            }) => Answer::Attacks(
                attackers
                    .iter()
                    .filter(|_| self.rnd(2) == 0)
                    .map(|a| (a.0, self.rnd(defenders.len())))
                    .collect(),
            ),
            Some(Ask::Block { blockers, .. }) => Answer::Blocks(
                blockers
                    .iter()
                    .filter(|_| self.rnd(2) == 0)
                    .map(|b| (b.0, b.2[self.rnd(b.2.len())]))
                    .collect(),
            ),
            Some(Ask::Pay { .. }) => match self.rnd(4) {
                0 => Answer::Cancel,
                1 => Answer::Auto,
                _ => self.tap().unwrap_or(Answer::Auto),
            },
            Some(Ask::Continue { .. }) => Answer::Continue,
        };
        ToEngine::Answer(a)
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(&args[1]);
    let deck_file = std::path::PathBuf::from(&args[2]);
    let games: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(5);
    let fuzz = args.iter().any(|a| a == "--fuzz");
    let seed0: u64 = args
        .iter()
        .position(|a| a == "--seed")
        .and_then(|k| args.get(k + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let rules = msh_deckbuilder::league::app_rules(&dir, None);
    let db = CardDb::load(&dir.join("cards.json")).unwrap();
    let mine = Deck::load(&deck_file, &db).unwrap();
    let opps =
        msh_deckbuilder::opponents::decks(&db, &rules, msh_deckbuilder::ranking::GAUNTLET_SEEDS[0]);
    let mut failures = 0;
    for g in 0..games {
        let seed = seed0 + g;
        let mut decks = vec![(mine.name.clone(), mine.to_text())];
        for k in 0..3 {
            let d = &opps[(seed as usize * 3 + k) % opps.len()];
            decks.push((d.name.clone(), d.to_text()));
        }
        let setup = Setup {
            seed,
            seat: 0,
            opts: PlayOptions {
                stop_on_bot_spells: fuzz,
                step_bot_turns: fuzz,
                stop_every_window: fuzz,
                ..Default::default()
            },
            rules: rules.clone(),
            decks,
        };
        let t = std::time::Instant::now();
        let asks = std::rc::Rc::new(Cell::new(0usize));
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let io: Box<dyn HumanIo> = if fuzz {
                Box::new(Fuzz {
                    state: Cell::new(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1),
                    view: RefCell::new(None),
                    ask: RefCell::new(None),
                    asks: Cell::new(0),
                })
            } else {
                Box::new(ScriptIo::new(Vec::new()))
            };
            play_with(&setup, &db, io)
        }));
        match r {
            Ok((w, answers, _)) => println!(
                "seed {}: winner P{}, {} answers, {:.1}s",
                seed,
                w,
                answers.len(),
                t.elapsed().as_secs_f32()
            ),
            Err(e) => {
                failures += 1;
                let msg = e
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default();
                println!("seed {}: PANIC {}", seed, msg);
            }
        }
        let _ = asks;
    }
    println!("{} games, {} failures", games, failures);
}
