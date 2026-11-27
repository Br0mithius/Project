//! The deck optimiser and the claim tester: searches for the strongest deck by playtests,
//! against the decks you will actually meet (your next table, with their trade picks) as
//! well as the simulated league.
//!
//! Single runs are lucky: the best of several candidates always looks better than it is
//! (the winner's curse). So every choice is made on one set of games and judged again on
//! fresh games before it is reported, and the final deck is compared with the starting
//! deck on games no stage has seen.
//!
//! * `optimize`: whole-deck candidates first (the current deck, the auto-builds - default,
//!   singleton, and every archetype the commander fits), raced on paired games; then the
//!   playtest lab's swap search, round after round, until no change holds up on fresh
//!   games; then the final check against the starting deck.
//! * `test_claims`: every affordable card you could claim (the Public Pool or a pasted
//!   list), played in your deck in place of its weakest spells, ranked by measured gain;
//!   the best are measured again on fresh games, with the VP each costs.

use crate::archetypes;
use crate::card::Card;
use crate::db::{CardDb, Collection, Rules};
use crate::deck::Deck;
use crate::lab::{Effort, Progress, explore_with};
use crate::playtest::Prepared;
use crate::playtest::compile::compile;
use crate::playtest::lab::{Measure, play_all};
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

/// The table's default share of the score; the rest is the peer field (decks about as
/// strong as the user's). The stand-ins for real opponents are guesses built around their
/// commanders and trade picks, and even tuned by this optimiser they stay well below a
/// tuned deck (it won about 49% of four-player games against them, 35% against the peer
/// field), so they are the smaller share: a deck is judged mostly by opposition as good
/// as itself, where answers matter.
pub const DEFAULT_TABLE_WEIGHT: f32 = 0.3;

/// Who the games are against: the next table's decks (trade picks in) and the simulated
/// league, weighted. With no table, only the league counts.
pub struct Opponents {
    pub table: Vec<Prepared>,
    pub league: Vec<Prepared>,
    /// Share of the score that comes from the table (0..1).
    pub table_weight: f32,
}

/// The opponents and weighting every measurement uses (the referee's version: bump it when
/// the opponent sets or the weighting change, so results from before can be told apart).
pub const REFEREE_VERSION: u32 = 2;

impl Opponents {
    /// The one standard referee: the data folder's next-table decks (30% of the score) and
    /// the peer field (70%). Every tool that judges a deck, a card or a builder change uses
    /// this, so the values the builder ranks by and the tests that judge its decks agree.
    pub fn standard(dir: &std::path::Path, db: &CardDb, rules: &Rules) -> Opponents {
        let table = crate::table::Table::load(dir);
        Opponents {
            table: crate::table::gauntlet(&table, db),
            league: crate::table::peer_field(db, rules),
            table_weight: DEFAULT_TABLE_WEIGHT,
        }
    }

    /// Win rates of `decks` (each in our seat, the same games for all) and each one's
    /// paired difference from the first, on the standard weighting.
    pub fn play(
        &self,
        decks: &[&Prepared],
        rules: &Rules,
        games: u32,
        seed: u64,
    ) -> Vec<(f32, Measure)> {
        let (done, cancel) = (
            std::sync::atomic::AtomicUsize::new(0),
            std::sync::atomic::AtomicBool::new(false),
        );
        let run = |g: &[Prepared], s: u64| {
            (!g.is_empty()).then(|| play_all(decks, g, rules, games, s, &done, &cancel))
        };
        let (t, l) = (run(&self.table, seed), run(&self.league, seed ^ 0x5EED));
        let w = self.w();
        (0..decks.len())
            .map(|i| match (&t, &l) {
                (Some(a), Some(b)) => (
                    w * a.win_rate(i) + (1.0 - w) * b.win_rate(i),
                    weighted(w, a.diff(i, 0), b.diff(i, 0)),
                ),
                (Some(a), None) => (a.win_rate(i), a.diff(i, 0)),
                (None, Some(b)) => (b.win_rate(i), b.diff(i, 0)),
                (None, None) => (0.0, Measure::default()),
            })
            .collect()
    }

    fn w(&self) -> f32 {
        if self.table.is_empty() {
            0.0
        } else if self.league.is_empty() {
            1.0
        } else {
            self.table_weight.clamp(0.0, 1.0)
        }
    }

    /// One gauntlet with the table's share of seats about its weight (the lab's swap
    /// search takes a single gauntlet).
    pub fn mixed(&self) -> Vec<Prepared> {
        let w = self.w();
        if w <= 0.0 {
            return self.league.iter().map(|p| p.with_swaps(&[])).collect();
        }
        if w >= 1.0 {
            return self.table.iter().map(|p| p.with_swaps(&[])).collect();
        }
        // copies * t / (copies * t + l) = w
        let (t, l) = (self.table.len() as f32, self.league.len() as f32);
        let copies = ((w * l) / ((1.0 - w) * t)).round().max(1.0) as usize;
        let mut out = Vec::new();
        for _ in 0..copies {
            out.extend(self.table.iter().map(|p| p.with_swaps(&[])));
        }
        out.extend(self.league.iter().map(|p| p.with_swaps(&[])));
        out
    }
}

/// A deck's measured gain over the base deck: weighted, and against each opponent set.
#[derive(Debug, Clone, Copy, Default)]
pub struct Gain {
    pub score: Measure,
    pub table: Option<Measure>,
    pub league: Option<Measure>,
}

fn weighted(w: f32, t: Measure, l: Measure) -> Measure {
    Measure {
        gain: w * t.gain + (1.0 - w) * l.gain,
        stderr: ((w * t.stderr).powi(2) + ((1.0 - w) * l.stderr).powi(2)).sqrt(),
        games: t.games + l.games,
    }
}

/// `deck` as swaps of `base` (paired with it card for card), when it has the same
/// commander and the same number of cards.
fn paired(base: &Deck, base_prep: &Prepared, deck: &Deck, db: &CardDb) -> Option<Prepared> {
    if base.commanders() != deck.commanders() {
        return None;
    }
    let canon = |d: &Deck| -> Option<BTreeMap<String, i64>> {
        let mut m = BTreeMap::new();
        for (n, &k) in &d.cards {
            *m.entry(db.get(n)?.name.clone()).or_insert(0) += k as i64;
        }
        Some(m)
    };
    let (a, b) = (canon(base)?, canon(deck)?);
    let (mut out, mut add): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    for n in a.keys().chain(b.keys()) {
        let d = b.get(n).copied().unwrap_or(0) - a.get(n).copied().unwrap_or(0);
        if d < 0 && !out.contains(n) {
            out.extend(std::iter::repeat_n(n.clone(), (-d) as usize));
        } else if d > 0 && !add.contains(n) {
            add.extend(std::iter::repeat_n(n.clone(), d as usize));
        }
    }
    if out.len() != add.len() {
        return None;
    }
    let swaps: Vec<(u16, _)> = out
        .iter()
        .zip(&add)
        .map(|(o, n)| Some((base_prep.card_index(o)?, compile(db.get(n)?))))
        .collect::<Option<_>>()?;
    Some(base_prep.with_swaps(&swaps))
}

/// Each deck's gain over `base` on the same games (paired when it differs from `base` by
/// swaps, otherwise on the same opponents and seats).
#[allow(clippy::too_many_arguments)]
pub fn compare(
    base: &Deck,
    decks: &[Deck],
    db: &CardDb,
    opp: &Opponents,
    rules: &Rules,
    games: u32,
    seed: u64,
    progress: &Progress,
) -> Vec<Gain> {
    // Every deck compared must be the legal size: a 59-card deck draws its best cards more
    // often and reads as an improvement it isn't.
    let size = base.total_count(rules);
    assert!(
        decks.iter().all(|d| d.total_count(rules) == size),
        "compare: decks of different sizes ({} vs {:?})",
        size,
        decks
            .iter()
            .map(|d| d.total_count(rules))
            .collect::<Vec<_>>()
    );
    let base_prep = Prepared::new(base, db);
    let mut preps = vec![base_prep.with_swaps(&[])];
    for d in decks {
        preps.push(paired(base, &base_prep, d, db).unwrap_or_else(|| Prepared::new(d, db)));
    }
    let refs: Vec<&Prepared> = preps.iter().collect();
    let cancel = &progress.cancel;
    let run = |g: &[Prepared]| {
        (!g.is_empty()).then(|| play_all(&refs, g, rules, games, seed, &progress.done, cancel))
    };
    let (t, l) = (run(&opp.table), run(&opp.league));
    let w = opp.w();
    (1..preps.len())
        .map(|i| {
            let tm = t.as_ref().map(|o| o.diff(i, 0));
            let lm = l.as_ref().map(|o| o.diff(i, 0));
            let score = match (tm, lm) {
                (Some(a), Some(b)) => weighted(w, a, b),
                (Some(a), None) => a,
                (None, Some(b)) => b,
                (None, None) => Measure::default(),
            };
            Gain {
                score,
                table: tm,
                league: lm,
            }
        })
        .collect()
}

fn canon(d: &Deck) -> Vec<(String, u32)> {
    let mut v: Vec<(String, u32)> = d.cards.iter().map(|(k, v)| (k.clone(), *v)).collect();
    v.sort();
    v
}

/// One accepted change on the way to the optimised deck.
#[derive(Debug, Clone)]
pub struct Step {
    pub what: String,
    /// Its gain, measured on games that didn't choose it.
    pub gain: Measure,
}

#[derive(Debug, Clone)]
pub struct OptReport {
    pub start: Deck,
    pub best: Deck,
    pub steps: Vec<Step>,
    /// The optimised deck against the starting deck on fresh games.
    pub final_gain: Option<Gain>,
    pub table_decks: usize,
    pub table_weight: f32,
    pub notes: Vec<String>,
    pub seconds: f32,
}

fn pts(m: &Measure) -> String {
    format!("{:+.1} ± {:.1} pts", m.gain * 100.0, m.stderr * 100.0)
}

/// Lines describing how `best` differs from `start`.
pub fn differences(start: &Deck, best: &Deck) -> Vec<String> {
    let mut out = Vec::new();
    let names: std::collections::BTreeSet<&String> =
        start.cards.keys().chain(best.cards.keys()).collect();
    for n in names {
        let (a, b) = (start.count_of(n), best.count_of(n));
        if b > a {
            out.push(format!("  + {} {}", b - a, n));
        }
    }
    let names: std::collections::BTreeSet<&String> =
        start.cards.keys().chain(best.cards.keys()).collect();
    for n in names {
        let (a, b) = (start.count_of(n), best.count_of(n));
        if a > b {
            out.push(format!("  - {} {}", a - b, n));
        }
    }
    out
}

impl OptReport {
    pub fn text(&self) -> String {
        let mut s = format!(
            "Deck optimiser - {} ({:.0} s)\nGames against {}.\n\n",
            self.start.commander.clone().unwrap_or_default(),
            self.seconds,
            if self.table_decks > 0 {
                format!(
                    "your next table ({} decks, trade picks in; {:.0}% of the score) and a peer field (decks about as strong as yours)",
                    self.table_decks,
                    self.table_weight * 100.0
                )
            } else {
                "a peer field (decks about as strong as yours) (tick your next table on Season & Rules to play against it)"
                    .to_string()
            }
        );
        if self.steps.is_empty() {
            s += "No change held up on fresh games: your deck is already the strongest found.\n";
        } else {
            s += "Steps (each measured on games that didn't pick it):\n";
            for st in &self.steps {
                s += &format!("  {}  {}\n", pts(&st.gain), st.what);
            }
        }
        if let Some(g) = &self.final_gain {
            s += &format!(
                "\nOPTIMISED DECK vs YOUR DECK on fresh games: {}",
                pts(&g.score)
            );
            if let Some(t) = &g.table {
                s += &format!("  (your table {}", pts(t));
                if let Some(l) = &g.league {
                    s += &format!(", peer field {}", pts(l));
                }
                s += ")";
            }
            s += "\n";
            if g.score.z() >= 2.0 {
                s += "That's clearly better (2+ standard errors): consider playing it.\n";
            } else if g.score.gain > 0.0 {
                s += "Better on these games, but within noise: the gain may be small or none.\n";
            } else {
                s += "Not better on fresh games: keep your deck.\n";
            }
            let diff = differences(&self.start, &self.best);
            if !diff.is_empty() {
                s += "\nChanges:\n";
                for l in diff {
                    s += &l;
                    s += "\n";
                }
            }
        }
        for n in &self.notes {
            s += &format!("\nNote: {}", n);
        }
        s
    }

    /// The deck worth playing: the optimised one when it held up clearly on fresh games.
    pub fn recommended(&self) -> &Deck {
        match &self.final_gain {
            Some(g) if g.score.z() >= 2.0 => &self.best,
            _ => &self.start,
        }
    }
}

/// `deck` with every filler card (no job) swapped for the best job card left in `coll`.
fn replace_filler(
    deck: &Deck,
    db: &CardDb,
    coll: &Collection,
    rules: &Rules,
    cohesive: bool,
    notes: &mut Vec<String>,
) -> Deck {
    let mut d = deck.clone();
    // Never above the land cap: trim basics (the most plentiful first) for the best spells.
    let cap = crate::analysis::land_cap(rules);
    let mut trimmed = Vec::new();
    while crate::mana::land_equivalents(&d, db) > cap {
        let Some(basic) = d
            .cards
            .iter()
            .filter(|(n, _)| db.get(n).is_some_and(|c| c.is_basic()))
            .max_by_key(|(_, k)| **k)
            .map(|(n, _)| n.clone())
        else {
            break;
        };
        let Some(s) = crate::analysis::suggestions(&d, db, coll, rules)
            .into_iter()
            .find(|s| !s.card.is_land())
        else {
            break;
        };
        d.remove(&basic, 1);
        d.add(&s.card.name, 1);
        trimmed.push(format!("{} -> {}", basic, s.card.name));
    }
    if !trimmed.is_empty() {
        notes.push(format!(
            "Lands trimmed to the cap of {}: {}.",
            cap,
            trimmed.join(", ")
        ));
    }
    if !cohesive {
        return d;
    }
    let filler: Vec<String> = d
        .cards
        .keys()
        .filter(|n| db.get(n).is_some_and(crate::cohesion::is_filler))
        .cloned()
        .collect();
    let mut swaps = Vec::new();
    for f in filler {
        let copies = d.count_of(&f);
        d.remove(&f, copies);
        for _ in 0..copies {
            match crate::analysis::suggestions(&d, db, coll, rules)
                .into_iter()
                .next()
            {
                Some(s) => {
                    swaps.push(format!("{} -> {}", f, s.card.name));
                    d.add(&s.card.name, 1);
                }
                None => d.add(&f, 1),
            }
        }
    }
    if !swaps.is_empty() {
        notes.push(format!(
            "Filler swapped for cards that do a job before the search: {}.",
            swaps.join(", ")
        ));
    }
    d
}

/// Whole-deck candidates for `cmd`: the auto-builds the program knows (default, singleton,
/// every archetype the commander fits), complete and distinct.
fn candidates(cmd: &Card, db: &CardDb, coll: &Collection, rules: &Rules) -> Vec<(String, Deck)> {
    let mut out: Vec<(String, Deck)> = Vec::new();
    let mut push = |label: String, d: Deck| {
        if d.main_count() >= d.main_target(rules) && !out.iter().any(|(_, o)| canon(o) == canon(&d))
        {
            out.push((label, d));
        }
    };
    let (d, _, arch) = archetypes::autobuild(cmd, db, coll, rules, None, None);
    push(
        format!(
            "auto-build ({})",
            arch.map(|a| a.name).unwrap_or("default targets")
        ),
        d,
    );
    if rules.max_copies != 1 {
        let mut single = rules.clone();
        single.max_copies = 1;
        let (d, _, _) = archetypes::autobuild(cmd, db, coll, &single, None, None);
        push("auto-build, singleton".into(), d);
    }
    for a in archetypes::ARCHETYPES
        .iter()
        .filter(|a| !a.sparse && archetypes::commander_fits(a, cmd))
        .take(8)
    {
        let (d, _) = archetypes::autobuild_as(a, cmd, db, coll, rules);
        push(format!("auto-build as {}", a.name), d);
    }
    // Three or more colours: the builds on fewer of them (the strongest pools first). A
    // five-colour commander's all-colours build can be far weaker than its best three
    // (Nick Fury: 2% against the peer field on five colours, 11-20% on three).
    if cmd.identity().count() >= 3 {
        for set in crate::lab::color_options(cmd, None, db, coll, rules)
            .into_iter()
            .filter(|s| s.count() < cmd.identity().count())
            .take(3)
        {
            let letters: String = (0..5)
                .filter(|&i| set.contains(i))
                .map(|i| crate::card::COLORS[i])
                .collect();
            let (d, _) = crate::analysis::autobuild(cmd, db, coll, rules, Some(set), None);
            push(format!("auto-build in {}", letters), d);
        }
    }
    // The colour packs the commander's colours allow (see `colorpacks`).
    for pack in crate::colorpacks::for_commander(cmd) {
        push(
            format!("{} pack", pack.name),
            crate::colorpacks::build(pack, cmd, db, coll, rules),
        );
    }
    out
}

/// Searches for the strongest deck for `deck`'s commander (see the module notes). `rounds`
/// caps the swap-search rounds. Returns None if cancelled.
#[allow(clippy::too_many_arguments)]
pub fn optimize(
    cmd: &Card,
    deck: &Deck,
    db: &CardDb,
    coll: &Collection,
    rules: &Rules,
    opp: &Opponents,
    effort: Effort,
    rounds: usize,
    progress: &Progress,
) -> Option<OptReport> {
    let t0 = std::time::Instant::now();
    let mut notes = Vec::new();
    let mut steps = Vec::new();
    let screen = effort.confirm / 2;
    // Control decks: only cards that do a job, and any filler already in the deck is
    // swapped for the best card that does one before the search starts.
    let cohesive = crate::analysis::cohesive_plan(cmd);
    let job_coll;
    let coll = if cohesive {
        job_coll = crate::cohesion::without_filler(coll, db);
        &job_coll
    } else {
        coll
    };
    let original = deck;
    let deck = &replace_filler(deck, db, coll, rules, cohesive, &mut notes);
    // ------------------------------------------------ whole-deck candidates
    progress.set_stage("Building candidate decks");
    let cands: Vec<(String, Deck)> = candidates(cmd, db, coll, rules)
        .into_iter()
        .filter(|(_, d)| canon(d) != canon(deck))
        // A control deck's alternatives keep its plan (locks, answers, card advantage, ramp).
        .filter(|(_, d)| !cohesive || crate::cohesion::keeps_plan(deck, d, db))
        // Cards locked in (`lab_keep`) stay: a whole-deck alternative without them is out.
        .filter(|(_, d)| {
            rules
                .lab_keep
                .iter()
                .all(|k| deck.count_of(k) == 0 || d.count_of(k) > 0)
        })
        .collect();
    let per = (opp.table.len().min(1) + opp.league.len().min(1)) as u32;
    progress.total.store(
        (screen as usize) * (cands.len() + 1) * per as usize
            + (effort.confirm as usize) * 4 * per as usize
            + rounds * 80_000,
        Ordering::Relaxed,
    );
    let mut cur = deck.clone();
    if !cands.is_empty() {
        progress.set_stage("Racing whole-deck candidates");
        let decks: Vec<Deck> = cands.iter().map(|(_, d)| d.clone()).collect();
        let g = compare(deck, &decks, db, opp, rules, screen, 0x0F7_0001, progress);
        if progress.cancelled() {
            return None;
        }
        let mut order: Vec<usize> = (0..g.len()).collect();
        order.sort_by(|&a, &b| g[b].score.gain.partial_cmp(&g[a].score.gain).unwrap());
        let top: Vec<usize> = order.into_iter().take(3).collect();
        // The leaders again on fresh games: picked on the first games, judged on these.
        progress.set_stage("Checking the leading candidates on fresh games");
        let decks: Vec<Deck> = top.iter().map(|&i| cands[i].1.clone()).collect();
        let g2 = compare(
            deck,
            &decks,
            db,
            opp,
            rules,
            effort.confirm,
            0x0F7_0002,
            progress,
        );
        if progress.cancelled() {
            return None;
        }
        if let Some((k, best)) = g2
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.score.gain.partial_cmp(&b.1.score.gain).unwrap())
        {
            if best.score.z() >= 1.0 {
                cur = cands[top[k]].1.clone();
                steps.push(Step {
                    what: format!("start from the {}", cands[top[k]].0),
                    gain: best.score,
                });
            } else {
                notes.push(format!(
                    "Best whole-deck alternative ({}) measured {} on fresh games: not enough to switch.",
                    cands[top[k]].0,
                    pts(&best.score)
                ));
            }
        }
    }
    // ------------------------------------------------ swap search, round after round
    // Two paths, judged against each other at the end: single swaps from the deck as it
    // is, and (when a whole-deck alternative led the race) single swaps from that. Taking
    // only the alternative's path lost to three plain swaps of the original deck: a new
    // build that beats the deck can still be worse than the deck with its best upgrades.
    let mixed = opp.mixed();
    let mut starts: Vec<(Deck, Vec<Step>)> = vec![(deck.clone(), Vec::new())];
    if canon(&cur) != canon(deck) {
        starts.push((cur.clone(), std::mem::take(&mut steps)));
    }
    let mut paths: Vec<(Deck, Vec<Step>)> = Vec::new();
    for (start, mut path_steps) in starts {
        let mut d = start;
        for r in 0..rounds {
            progress.set_stage(&format!("Swap search, round {} of up to {}", r + 1, rounds));
            let rep = explore_with(cmd, &d, db, coll, rules, effort, &mixed, progress)?;
            match rep.best {
                Some(b) => {
                    let what = b
                        .swaps
                        .iter()
                        .map(|(s, _)| format!("{} -> {}", s.cut, s.add))
                        .collect::<Vec<_>>()
                        .join(", ");
                    path_steps.push(Step { what, gain: b.gain });
                    d = b.deck;
                }
                None => break,
            }
        }
        // Basics re-chosen for castability (how often each spell is castable on curve) - as
        // a second contender, not over the measured deck: the rule of thumb turned the
        // Mountain HULK SMASH! measured better with back into a Plains. The final check on
        // fresh games picks between them.
        let mut rebalanced = d.clone();
        crate::mana::rebalance_basics(&mut rebalanced, db);
        for deck in [d, rebalanced] {
            if !paths.iter().any(|(p, _)| canon(p) == canon(&deck)) {
                paths.push((deck, path_steps.clone()));
            }
        }
    }
    // ------------------------------------------------ final check on games no stage saw
    let contenders: Vec<&(Deck, Vec<Step>)> = paths
        .iter()
        .filter(|(d, _)| canon(d) != canon(original))
        .collect();
    let mut cur = original.clone();
    let mut final_gain = None;
    if !contenders.is_empty() {
        progress.set_stage("Final check against your deck on fresh games");
        let decks: Vec<Deck> = contenders.iter().map(|(d, _)| d.clone()).collect();
        let g = compare(
            original,
            &decks,
            db,
            opp,
            rules,
            effort.confirm * 2,
            0x0F7_0003,
            progress,
        );
        if progress.cancelled() {
            return None;
        }
        let best = (0..g.len())
            .max_by(|&x, &y| g[x].score.gain.partial_cmp(&g[y].score.gain).unwrap())
            .expect("a contender");
        for (k, gain) in g.iter().enumerate() {
            if k != best {
                notes.push(format!(
                    "The other path ({}) measured {} on the same fresh games: the better one is shown.",
                    contenders[k]
                        .1
                        .first()
                        .map_or("swaps from your deck".to_string(), |s| s.what.clone()),
                    pts(&gain.score)
                ));
            }
        }
        cur = contenders[best].0.clone();
        steps = contenders[best].1.clone();
        final_gain = g.into_iter().nth(best);
    }
    Some(OptReport {
        start: original.clone(),
        best: cur,
        steps,
        final_gain,
        table_decks: opp.table.len(),
        table_weight: opp.w(),
        notes,
        seconds: t0.elapsed().as_secs_f32(),
    })
}

/// A card you could claim, measured in your deck.
#[derive(Debug, Clone)]
pub struct Claim {
    pub card: String,
    /// VP to claim it (twice its trade-in value; 0 = a shop or Skill Tree card, whose price
    /// the league sets).
    pub vp: u32,
    /// The deck card it replaced in its best test.
    pub cut: String,
    /// Screening gain (biased upwards for the best few).
    pub screened: Measure,
    /// The gain again on fresh games (the leaders only).
    pub confirmed: Option<Measure>,
}

#[derive(Debug, Clone)]
pub struct ClaimReport {
    pub claims: Vec<Claim>,
    pub tested: usize,
    pub skipped: usize,
    pub seconds: f32,
}

fn vp_text(vp: u32) -> String {
    if vp == 0 {
        "shop/tree".to_string()
    } else {
        format!("{:>2} VP    ", vp)
    }
}

impl ClaimReport {
    pub fn text(&self) -> String {
        let mut s = format!(
            "Claims tested in your deck: {} cards ({} skipped: not in your colours, or already in the deck), {:.0} s.\n\
             Gain = win-rate points over your deck, the card in place of the deck card shown.\n\n",
            self.tested, self.skipped, self.seconds
        );
        let confirmed: Vec<&Claim> = self
            .claims
            .iter()
            .filter(|c| c.confirmed.is_some())
            .collect();
        if !confirmed.is_empty() {
            s += "BEST CLAIMS (measured again on fresh games):\n";
            for c in confirmed {
                let m = c.confirmed.unwrap();
                s += &format!(
                    "  {}  {:<34} {}  for {}{}\n",
                    pts(&m),
                    c.card,
                    vp_text(c.vp),
                    c.cut,
                    if m.z() >= 2.0 {
                        "  - clearly better"
                    } else {
                        ""
                    }
                );
            }
            s += "\n";
        }
        s += "All screened (best first; the top few read high - see the fresh-game numbers above):\n";
        let mut all: Vec<&Claim> = self.claims.iter().collect();
        all.sort_by(|a, b| b.screened.gain.partial_cmp(&a.screened.gain).unwrap());
        for c in all.into_iter().take(40) {
            s += &format!(
                "  {}  {:<34} {}  for {}\n",
                pts(&c.screened),
                c.card,
                vp_text(c.vp),
                c.cut
            );
        }
        s
    }
}

/// Plays each of `names` in `deck` in place of its two weakest spells (by the builder's
/// score), keeps each card's better result, and measures the best `confirm_top` again on
/// fresh games. Cards outside the deck's colours, already in it, or unknown are skipped.
#[allow(clippy::too_many_arguments)]
pub fn test_claims(
    deck: &Deck,
    names: &[String],
    db: &CardDb,
    rules: &Rules,
    opp: &Opponents,
    games: u32,
    confirm_top: usize,
    progress: &Progress,
) -> Option<ClaimReport> {
    let t0 = std::time::Instant::now();
    let id = deck.identity(db)?;
    let kept = |n: &str| rules.lab_keep.iter().any(|k| k == n);
    // The cards a claim may replace: the weakest ones that aren't locked in. For a control
    // deck each claim is tried in place of the weakest cards it may replace under the plan
    // (`cohesion::swap_keeps_plan`): a creature isn't measured in a lock's slot.
    let weakest: Vec<String> = crate::analysis::weakest(deck, db, rules)
        .into_iter()
        .filter(|w| !kept(&w.card.name))
        .map(|w| w.card.name.clone())
        .collect();
    let guard = deck
        .commander
        .as_deref()
        .and_then(|c| db.get(c))
        .is_some_and(crate::analysis::cohesive_plan);
    let cuts_for = |add: &str| -> Vec<String> {
        weakest
            .iter()
            .filter(|cut| !guard || crate::cohesion::swap_keeps_plan(deck, db, cut, add, 1))
            .take(2)
            .cloned()
            .collect()
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut cards: Vec<&Card> = Vec::new();
    let mut skipped = 0;
    for n in names {
        match db.get(n) {
            Some(c)
                if !c.is_land()
                    && c.identity().is_subset_of(id)
                    && deck.count_of(&c.name) == 0
                    && deck.commander.as_deref() != Some(c.name.as_str()) =>
            {
                if seen.insert(c.name.clone()) {
                    cards.push(c);
                }
            }
            _ => skipped += 1,
        }
    }
    let variants: Vec<(usize, String, Deck)> = cards
        .iter()
        .enumerate()
        .flat_map(|(i, c)| {
            cuts_for(&c.name).into_iter().map(move |cut| {
                let mut d = deck.clone();
                d.remove(&cut, 1);
                d.add(&c.name, 1);
                (i, cut, d)
            })
        })
        .collect();
    progress.total.store(
        variants.len() * games as usize * 2 + confirm_top * games as usize * 6,
        Ordering::Relaxed,
    );
    progress.set_stage(&format!("Screening {} cards in your deck", cards.len()));
    let decks: Vec<Deck> = variants.iter().map(|v| v.2.clone()).collect();
    let g = compare(deck, &decks, db, opp, rules, games, 0xC1A_0001, progress);
    if progress.cancelled() {
        return None;
    }
    let mut best: Vec<Option<(usize, Measure)>> = vec![None; cards.len()];
    for (k, (i, _, _)) in variants.iter().enumerate() {
        if best[*i].is_none_or(|(_, m)| g[k].score.gain > m.gain) {
            best[*i] = Some((k, g[k].score));
        }
    }
    let mut claims: Vec<(usize, Claim)> = best
        .iter()
        .enumerate()
        .filter_map(|(i, b)| {
            let (k, m) = (*b)?;
            Some((
                k,
                Claim {
                    card: cards[i].name.clone(),
                    vp: crate::league::vp_value(cards[i]) * 2,
                    cut: variants[k].1.clone(),
                    screened: m,
                    confirmed: None,
                },
            ))
        })
        .collect();
    claims.sort_by(|a, b| b.1.screened.gain.partial_cmp(&a.1.screened.gain).unwrap());
    let top: Vec<usize> = (0..claims.len().min(confirm_top)).collect();
    if !top.is_empty() {
        progress.set_stage("Measuring the best claims again on fresh games");
        let decks: Vec<Deck> = top
            .iter()
            .map(|&j| variants[claims[j].0].2.clone())
            .collect();
        let g2 = compare(
            deck,
            &decks,
            db,
            opp,
            rules,
            games * 3,
            0xC1A_0002,
            progress,
        );
        if progress.cancelled() {
            return None;
        }
        for (n, &j) in top.iter().enumerate() {
            claims[j].1.confirmed = Some(g2[n].score);
        }
    }
    let mut claims: Vec<Claim> = claims.into_iter().map(|(_, c)| c).collect();
    // Confirmed ones in confirmed order first, then the rest as screened.
    claims.sort_by(|a, b| match (a.confirmed, b.confirmed) {
        (Some(x), Some(y)) => y.gain.partial_cmp(&x.gain).unwrap(),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => b.screened.gain.partial_cmp(&a.screened.gain).unwrap(),
    });
    Some(ClaimReport {
        claims,
        tested: cards.len(),
        skipped,
        seconds: t0.elapsed().as_secs_f32(),
    })
}
