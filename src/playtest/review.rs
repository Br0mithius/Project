//! Decision review: a recorded hand-played game (`GameRecord`), replayed to find where the
//! player chose differently from the bot, with both choices played out from that position.
//!
//! A game is exact from its seed, decks and answers (`human::Report`). For a decision at
//! answer `k`, two lines are played: the player's (the answers up to and including the
//! decision) and the bot's (the answers before it, then `Answer::Auto`). From there the bot
//! plays the seat in both, and the game is re-randomised right after the decision
//! (`HumanLink::fork`: a new seed and every library reshuffled), the same way in both lines
//! for each play-out. The difference in games won is what the choice was worth - as far as
//! bots playing the rest can tell.
//!
//! The reshuffle comes right before the decision where the engine can do it there (main
//! phase, response, attack and block questions). A choice asked while something resolves
//! (a target, a yes/no) is reshuffled at the next such point instead, so a choice that
//! draws cards is compared on the recorded game's draw: read those with care.
//!
//! Decisions reviewed: each main phase as a whole (what was played, against what the bot
//! would have played), attack and block declarations, and answers given outside a main
//! phase where the bot suggested something else. Mulligans are not (the line after a
//! mulligan depends on one new hand).

use super::human::{Answer, Ask, Report, ScriptIo, SharedIo, play_forked};
use crate::db::CardDb;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;

/// A finished hand-played game, as the app saves it (`games/*.json` in the data folder).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameRecord {
    /// The record format.
    pub version: u32,
    pub player: String,
    pub date: String,
    /// The app build that played it (`CARGO_PKG_VERSION`) and its engine version.
    pub app: String,
    pub sim: u32,
    pub winner: usize,
    pub report: Report,
}

pub const RECORD_VERSION: u32 = 1;

impl GameRecord {
    pub fn won(&self) -> bool {
        self.winner == self.report.setup.seat
    }
    /// Sandbox edits were used: the game isn't a fair record of play.
    pub fn edited(&self) -> bool {
        self.report
            .answers
            .iter()
            .any(|a| matches!(a, Answer::Edit(_)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Kind {
    MainPhase,
    Attack,
    Block,
    Response,
    Choice,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::MainPhase => "main phase (what to play)",
            Kind::Attack => "attacks",
            Kind::Block => "blocks",
            Kind::Response => "responses and instant-speed plays",
            Kind::Choice => "other choices (targets, modes, yes/no)",
        }
    }
}

/// One way a decision could have gone, and how it did.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    pub label: String,
    /// What the player did / what the bot would have done.
    pub player: bool,
    pub bot: bool,
    /// The same game - same seed, same shuffles - with this choice and the bot playing on:
    /// did the seat win, and in which round the game ended.
    pub same_game_win: bool,
    pub same_game_round: u32,
    /// What the seat went on to play that turn in the same game.
    #[serde(default)]
    pub then: String,
    /// Play-outs from this position with fresh luck (the same luck for every choice).
    pub wins: u32,
}

/// One decision point of a game, with every choice tried.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub kind: Kind,
    pub round: u32,
    pub what: String,
    pub choices: Vec<Choice>,
    pub playouts: u32,
    /// Play-outs where only the player's choice won, and where only the bot's did.
    pub only_player: u32,
    pub only_bot: u32,
}

impl Decision {
    pub fn player(&self) -> Option<&Choice> {
        self.choices.iter().find(|c| c.player)
    }
    pub fn bot(&self) -> Option<&Choice> {
        self.choices.iter().find(|c| c.bot)
    }
    /// The choice that won most play-outs.
    pub fn best(&self) -> Option<&Choice> {
        self.choices
            .iter()
            .max_by_key(|c| (c.wins, c.same_game_win))
    }
    /// The player's choice minus the bot's, in points of win rate, and its standard error
    /// (paired: each play-out uses the same luck for both).
    pub fn gain(&self) -> (f32, f32) {
        let n = self.playouts.max(1) as f32;
        let d = (self.only_player as f32 - self.only_bot as f32) / n;
        let var = ((self.only_player + self.only_bot) as f32 / n - d * d).max(0.0) / n;
        (100.0 * d, 100.0 * var.sqrt())
    }
    /// What the player's choice gave up against the best one, in points (0 if it was best).
    pub fn left_on_table(&self) -> f32 {
        let n = self.playouts.max(1) as f32;
        match (self.best(), self.player()) {
            (Some(b), Some(p)) => 100.0 * (b.wins as f32 - p.wins as f32) / n,
            _ => 0.0,
        }
    }
}

/// What a replay of `answers` (then the bot) shows: the questions, the log and where the log
/// stood at each question, and the winner.
struct Line {
    asks: Vec<Ask>,
    log: Vec<String>,
    log_at: Vec<usize>,
    winner: usize,
}

fn run(report: &Report, db: &CardDb, answers: Vec<Answer>, fork: Option<(usize, u64)>) -> Line {
    let io = std::rc::Rc::new(ScriptIo {
        answers: RefCell::new(answers.into()),
        ..Default::default()
    });
    let (winner, _, _) = play_forked(&report.setup, db, Box::new(SharedIo(io.clone())), fork);
    Line {
        asks: io.asks.borrow().clone(),
        log: io.log.borrow().clone(),
        log_at: io.log_at.borrow().clone(),
        winner,
    }
}

/// The seat's own plays in the log from `from` on, until its turn's round is over.
fn plays(log: &[String], from: usize, seat: usize) -> Vec<String> {
    let me = format!(" P{} ", seat);
    let round = log
        .get(from)
        .and_then(|l| l.split_whitespace().next())
        .unwrap_or("")
        .to_string();
    let mut out = Vec::new();
    for l in log.iter().skip(from).take(600) {
        if !l.starts_with(&round) || l.contains("ELIMINATED") {
            break;
        }
        // Another player's turn began.
        if l.contains(" draws ") && !l.contains(&me) && !l.contains('[') {
            break;
        }
        let Some(at) = l.find(&me) else { continue };
        let rest = &l[at + me.len()..];
        if ["casts ", "plays land ", "activates ", "equips ", "cycles "]
            .iter()
            .any(|k| rest.starts_with(k))
        {
            let short = rest.split(" for {").next().unwrap_or(rest);
            let short = short.split(" (MV").next().unwrap_or(short);
            let short = short.split(" -> ").next().unwrap_or(short);
            out.push(short.trim().to_string());
        }
    }
    out
}

fn round_of(log: &[String], at: usize) -> u32 {
    log.get(at.min(log.len().saturating_sub(1)))
        .and_then(|l| l.split_whitespace().next())
        .and_then(|r| r.strip_prefix('R'))
        .and_then(|r| r.parse().ok())
        .unwrap_or(0)
}

fn names(list: &[(u32, String)], id: u32) -> String {
    list.iter()
        .find(|x| x.0 == id)
        .map(|x| {
            x.1.split(" (")
                .next()
                .unwrap_or(&x.1)
                .trim_end_matches(" (must attack)")
                .to_string()
        })
        .unwrap_or_else(|| "?".into())
}

fn describe(ask: &Ask, ans: Option<&Answer>) -> String {
    // `ans` None: the bot's suggestion.
    match ask {
        Ask::Attack {
            attackers,
            defenders,
            suggested,
        } => {
            let v: Vec<(u32, usize)> = match ans {
                None => suggested.clone(),
                Some(Answer::Attacks(v)) => v.clone(),
                _ => Vec::new(),
            };
            if v.is_empty() {
                return "no attack".into();
            }
            v.iter()
                .map(|(id, d)| {
                    format!(
                        "{} -> {}",
                        names(attackers, *id),
                        defenders.get(*d).map_or("?", |s| s.as_str())
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        }
        Ask::Block {
            attackers,
            blockers,
            suggested,
        } => {
            let v: Vec<(u32, u32)> = match ans {
                None => suggested.clone(),
                Some(Answer::Blocks(v)) => v.clone(),
                _ => Vec::new(),
            };
            if v.is_empty() {
                return "no blocks".into();
            }
            let bl: Vec<(u32, String)> = blockers.iter().map(|b| (b.0, b.1.clone())).collect();
            v.iter()
                .map(|(b, a)| format!("{} blocks {}", names(&bl, *b), names(attackers, *a)))
                .collect::<Vec<_>>()
                .join(", ")
        }
        Ask::Act {
            actions, suggested, ..
        } => {
            let i = match ans {
                None => *suggested,
                Some(Answer::Pick(v)) => v.first().copied(),
                Some(Answer::AutoTap(i)) => Some(*i),
                _ => None,
            };
            i.and_then(|i| actions.get(i))
                .map_or("pass".into(), |a| a.label.clone())
        }
        Ask::Pick {
            options, suggested, ..
        } => {
            let v: Vec<usize> = match ans {
                None => suggested.clone(),
                Some(Answer::Pick(v)) => v.clone(),
                _ => Vec::new(),
            };
            if v.is_empty() {
                return "none".into();
            }
            v.iter()
                .filter_map(|&i| options.get(i).cloned())
                .collect::<Vec<_>>()
                .join(", ")
        }
        Ask::YesNo { suggested, .. } => {
            let y = match ans {
                None => *suggested,
                Some(Answer::Yes(y)) => *y,
                _ => *suggested,
            };
            if y { "yes" } else { "no" }.into()
        }
        Ask::Number { suggested, .. } => match ans {
            Some(Answer::Num(n)) => n.to_string(),
            _ => suggested.to_string(),
        },
        _ => String::new(),
    }
}

fn prompt_of(ask: &Ask) -> String {
    match ask {
        Ask::Attack { .. } => "Declare attackers".into(),
        Ask::Block { .. } => "Declare blockers".into(),
        Ask::Act { phase, .. } => phase.clone(),
        Ask::Pick { prompt, .. } | Ask::YesNo { prompt, .. } | Ask::Number { prompt, .. } => {
            prompt.clone()
        }
        _ => String::new(),
    }
}

/// Progress of a review, for the GUI.
#[derive(Default)]
pub struct ReviewProgress {
    pub done: std::sync::atomic::AtomicUsize,
    pub total: std::sync::atomic::AtomicUsize,
    pub cancel: std::sync::atomic::AtomicBool,
}

/// Every answer worth trying at a question: (label, answer). The bot's own choice
/// (`Answer::Auto`) is always among them.
fn options(ask: &Ask, given: &Answer) -> Vec<(String, Answer)> {
    let mut out: Vec<(String, Answer)> = Vec::new();
    match ask {
        Ask::Act { actions, .. } => {
            out.push(("pass".into(), Answer::Pass));
            for (i, a) in actions.iter().enumerate() {
                if a.payable && a.auto_ok && !out.iter().any(|o| o.0 == a.label) {
                    out.push((a.label.clone(), Answer::AutoTap(i)));
                }
            }
        }
        Ask::Pick {
            options, min, max, ..
        } if *max == 1 => {
            if *min == 0 {
                out.push(("none".into(), Answer::Pick(Vec::new())));
            }
            for (i, o) in options.iter().enumerate().take(12) {
                out.push((o.clone(), Answer::Pick(vec![i])));
            }
        }
        Ask::YesNo { .. } => {
            out.push(("yes".into(), Answer::Yes(true)));
            out.push(("no".into(), Answer::Yes(false)));
        }
        Ask::Number { min, max, .. } if max - min <= 6 => {
            for n in *min..=*max {
                out.push((n.to_string(), Answer::Num(n)));
            }
        }
        Ask::Attack {
            attackers,
            defenders,
            ..
        } => {
            out.push(("no attack".into(), Answer::Attacks(Vec::new())));
            for (d, name) in defenders.iter().enumerate().take(4) {
                out.push((
                    format!("everything -> {}", name),
                    Answer::Attacks(attackers.iter().map(|a| (a.0, d)).collect()),
                ));
            }
        }
        Ask::Block { .. } => out.push(("no blocks".into(), Answer::Blocks(Vec::new()))),
        _ => {}
    }
    // What was actually answered, if it isn't one of those (several picks, a mixed attack).
    let side = matches!(
        given,
        Answer::Tap { .. } | Answer::Filter { .. } | Answer::Edit(_) | Answer::Concede
    );
    if !side && !matches!(given, Answer::Auto) && !out.iter().any(|o| same(&o.1, given)) {
        out.push((describe(ask, Some(given)), given.clone()));
    }
    out.push(("the bot's choice".into(), Answer::Auto));
    out
}

/// Two answers that do the same thing at one question.
fn same(a: &Answer, b: &Answer) -> bool {
    let norm = |x: &Answer| match x {
        Answer::Pick(v) if v.len() == 1 => Answer::AutoTap(v[0]),
        Answer::Continue | Answer::Cancel => Answer::Pass,
        Answer::Attacks(v) => {
            let mut v = v.clone();
            v.sort_unstable();
            Answer::Attacks(v)
        }
        Answer::Blocks(v) => {
            let mut v = v.clone();
            v.sort_unstable();
            Answer::Blocks(v)
        }
        x => x.clone(),
    };
    norm(a) == norm(b)
}

/// Reviews one game by brute force. At every question the player answered, every choice is
/// tried with the bot playing on from there: once in the very same game (same seed and
/// shuffles, only the choice differs), and `playouts` times with fresh luck (so a choice
/// isn't rated by one lucky draw). Decision points where every choice came out the same
/// are left out.
pub fn review(
    rec: &GameRecord,
    db: &CardDb,
    playouts: u32,
    prog: &ReviewProgress,
) -> Vec<Decision> {
    review_with(rec, db, playouts, None, None, prog)
}

/// `review` at scale. `sample` (count, seed): only that many decision points, picked at
/// random. `confirm` (gap, play-outs): a point where some choice beats the bot's by `gap`
/// points or more on the first `playouts` is played out that many more times on other luck,
/// and those results replace the first ones (a reported gap isn't the screen's own
/// selection); the other points are dropped.
pub fn review_with(
    rec: &GameRecord,
    db: &CardDb,
    playouts: u32,
    sample: Option<(usize, u64)>,
    confirm: Option<(f32, u32)>,
    prog: &ReviewProgress,
) -> Vec<Decision> {
    use std::sync::atomic::Ordering::Relaxed;
    let report = &rec.report;
    let seat = report.setup.seat;
    let actual = run(report, db, report.answers.clone(), None);
    let n = report.answers.len().min(actual.asks.len());
    // The decision points and their choices.
    let mut points: Vec<(usize, Kind, Vec<(String, Answer)>)> = Vec::new();
    for k in 0..n {
        let (ask, given) = (&actual.asks[k], &report.answers[k]);
        if matches!(given, Answer::Concede) {
            break;
        }
        let kind = match ask {
            Ask::Act { window: false, .. } => Kind::MainPhase,
            Ask::Act { window: true, .. } => Kind::Response,
            Ask::Attack { .. } => Kind::Attack,
            Ask::Block { .. } => Kind::Block,
            Ask::Pick { .. } | Ask::YesNo { .. } | Ask::Number { .. } => Kind::Choice,
            _ => continue,
        };
        if matches!(
            given,
            Answer::Tap { .. } | Answer::Filter { .. } | Answer::Edit(_)
        ) {
            continue;
        }
        let opts = options(ask, given);
        if opts.len() >= 2 {
            points.push((k, kind, opts));
        }
    }
    if let Some((count, seed)) = sample {
        let mut rng = crate::analysis::Rng::new(seed);
        rng.shuffle(&mut points);
        points.truncate(count);
        points.sort_by_key(|p| p.0);
    }
    let jobs: usize = points.iter().map(|p| p.2.len()).sum();
    prog.total.store(jobs * (1 + playouts as usize), Relaxed);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut out = Vec::new();
    for (k, kind, opts) in points {
        if prog.cancel.load(Relaxed) {
            break;
        }
        let line = |ans: &Answer| {
            let mut v = report.answers[..k].to_vec();
            v.push(ans.clone());
            v
        };
        // The very same game with each choice.
        let exact: Vec<Line> = opts
            .iter()
            .map(|o| run(report, db, line(&o.1), None))
            .collect();
        prog.done.fetch_add(opts.len(), Relaxed);
        let given = &report.answers[k];
        let from = actual.log_at.get(k).copied().unwrap_or(0);
        // Which choice the bot's is, and which the player's: by what the seat then did.
        let did = |l: &Line| plays(&l.log, from, seat).into_iter().next();
        let bot_idx = opts.len() - 1;
        let player_idx = if matches!(given, Answer::Auto) {
            bot_idx
        } else {
            opts.iter()
                .position(|o| same(&o.1, given))
                .unwrap_or(bot_idx)
        };
        // Fresh luck, the same for every choice.
        let play = |count: u32, salt: u64| -> Vec<Vec<bool>> {
            let next = std::sync::atomic::AtomicUsize::new(0);
            std::thread::scope(|s| {
                let hs: Vec<_> = (0..threads)
                    .map(|_| {
                        s.spawn(|| {
                            let mut r: Vec<(usize, Vec<bool>)> = Vec::new();
                            loop {
                                let i = next.fetch_add(1, Relaxed);
                                if i >= count as usize || prog.cancel.load(Relaxed) {
                                    break;
                                }
                                let seed =
                                    0x5EED_0000 + salt + i as u64 * 7919 + k as u64 * 104_729;
                                let row = opts
                                    .iter()
                                    .map(|o| {
                                        run(report, db, line(&o.1), Some((k, seed))).winner == seat
                                    })
                                    .collect();
                                prog.done.fetch_add(opts.len(), Relaxed);
                                r.push((i, row));
                            }
                            r
                        })
                    })
                    .collect();
                let mut all: Vec<(usize, Vec<bool>)> =
                    hs.into_iter().flat_map(|h| h.join().unwrap()).collect();
                all.sort_by_key(|x| x.0);
                all.into_iter().map(|x| x.1).collect()
            })
        };
        let mut wins = play(playouts, 0);
        if let Some((gap, more)) = confirm {
            let n = wins.len().max(1) as f32;
            let of = |i: usize| wins.iter().filter(|r| r[i]).count() as f32 / n;
            let best = (0..opts.len()).map(of).fold(0.0, f32::max);
            if 100.0 * (best - of(bot_idx)) < gap {
                continue;
            }
            wins = play(more, 0x00C0_FFEE_0000);
        }
        let count = |i: usize| wins.iter().filter(|r| r[i]).count() as u32;
        let choices: Vec<Choice> = opts
            .iter()
            .enumerate()
            .map(|(i, o)| Choice {
                label: if i == bot_idx {
                    match &actual.asks[k] {
                        Ask::Act { window: false, .. } => format!(
                            "the bot's choice ({})",
                            did(&exact[i]).unwrap_or_else(|| "pass".into())
                        ),
                        a => format!("the bot's choice ({})", describe(a, None)),
                    }
                } else {
                    o.0.clone()
                },
                player: i == player_idx,
                bot: i == bot_idx,
                same_game_win: exact[i].winner == seat,
                same_game_round: round_of(&exact[i].log, exact[i].log.len().saturating_sub(1)),
                then: plays(&exact[i].log, from, seat).join("; "),
                wins: count(i),
            })
            .collect();
        // Nothing to learn where every choice did the same.
        let flat = choices
            .iter()
            .all(|c| c.same_game_win == choices[0].same_game_win)
            && choices.iter().all(|c| c.wins == choices[0].wins);
        // (At scale every sampled point is kept: averages over all of them are what is
        // unbiased, the ones with no difference included.)
        if flat && sample.is_none() {
            continue;
        }
        out.push(Decision {
            kind,
            round: round_of(&actual.log, from),
            what: prompt_of(&actual.asks[k]),
            playouts: wins.len() as u32,
            only_player: wins.iter().filter(|r| r[player_idx] && !r[bot_idx]).count() as u32,
            only_bot: wins.iter().filter(|r| !r[player_idx] && r[bot_idx]).count() as u32,
            choices,
        });
    }
    out
}

/// A game's review, as saved next to its record and sent with it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reviewed {
    pub player: String,
    pub date: String,
    pub won: bool,
    pub decisions: Vec<Decision>,
}

/// The reviews of many games, by kind of decision.
pub fn summary(all: &[Reviewed]) -> String {
    let mut out = String::new();
    let won = all.iter().filter(|r| r.won).count();
    out.push_str(&format!(
        "{} game(s), {} won. Every choice at every decision was tried with the bot playing \
         on: in the same game, and with fresh luck.\n\n",
        all.len(),
        won
    ));
    let mut kinds: Vec<Kind> = all
        .iter()
        .flat_map(|r| r.decisions.iter().map(|d| d.kind))
        .collect();
    kinds.sort();
    kinds.dedup();
    for k in kinds {
        let ds: Vec<&Decision> = all
            .iter()
            .flat_map(|r| r.decisions.iter())
            .filter(|d| d.kind == k)
            .collect();
        let differ: Vec<&&Decision> = ds
            .iter()
            .filter(|d| d.player().map(|c| c.bot) == Some(false))
            .collect();
        let n: u32 = differ.iter().map(|d| d.playouts).sum();
        let op: u32 = differ.iter().map(|d| d.only_player).sum();
        let ob: u32 = differ.iter().map(|d| d.only_bot).sum();
        let nf = n.max(1) as f32;
        let d = (op as f32 - ob as f32) / nf;
        let se = (((op + ob) as f32 / nf - d * d).max(0.0) / nf).sqrt();
        let lost: f32 = ds.iter().map(|d| d.left_on_table()).sum::<f32>() / ds.len().max(1) as f32;
        let flips = ds
            .iter()
            .filter(|d| {
                d.player().is_some_and(|p| !p.same_game_win)
                    && d.choices.iter().any(|c| c.same_game_win)
            })
            .count();
        out.push_str(&format!(
            "{}: {} decision(s) that mattered; the player differed from the bot in {} \
             ({:+.1} ± {:.1} pts for the player's choice on average); best choice missed by \
             {:.1} pts on average; {} where another choice wins the same game the player \
             lost\n",
            k.label(),
            ds.len(),
            differ.len(),
            100.0 * d,
            100.0 * se,
            lost,
            flips
        ));
    }
    out
}

/// One decision as text: every choice, how it did in the same game and over the play-outs.
pub fn decision_text(d: &Decision) -> String {
    let n = d.playouts.max(1);
    let mut s = format!("R{} {}:\n", d.round, d.what);
    let mut cs: Vec<&Choice> = d.choices.iter().collect();
    cs.sort_by_key(|c| std::cmp::Reverse((c.wins, c.same_game_win)));
    for c in cs {
        s.push_str(&format!(
            "    {:>3}% of {} play-outs, same game: {} in round {:<2}  {}{}{}\n",
            100 * c.wins / n,
            n,
            if c.same_game_win { "WON " } else { "lost" },
            c.same_game_round,
            c.label,
            match (c.player, c.bot) {
                (true, true) => "   <- you (as the bot would)",
                (true, false) => "   <- you",
                _ => "",
            },
            if c.then.is_empty() {
                String::new()
            } else {
                format!("   [turn: {}]", c.then)
            }
        ));
    }
    s
}
