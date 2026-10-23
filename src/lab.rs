//! Playtest lab: instead of trusting the card scores, play the deck and its variations
//! and measure what each change is worth.
//!
//! The deck builder picks cards by a heuristic score, which can rate a card higher than it
//! plays. The lab takes a finished deck and, with paired playtests (`playtest::lab`: the
//! deck and the changed deck play exactly the same games), measures:
//!
//! 1. What each card is worth: the win rate lost when it's replaced by the best spell left
//!    on the bench. One land more or fewer is a check of its own: a deck at the league's
//!    land goal (26 of 60) is never told to swap a spell for a basic - when one more land
//!    clearly helps, the report says the mana base needs better fixing instead.
//! 2. Additions: the most promising bench cards, each in place of the cards stage 1 found
//!    least valuable.
//! 3. The best combination: the measured gains combined greedily, then confirmed on fresh
//!    games (different seeds, many more of them), with every swap re-checked by leaving it
//!    out. Swaps that don't hold up are dropped. Screening and confirming on different
//!    games keeps the reported number honest (the best of many noisy screens is usually
//!    lucky - the winner's curse).
//! 4. Variations: the deck rebuilt in the other colour sets the commander allows.
//!
//! The normal deck build is untouched; the report is a separate suggestion.

use crate::analysis::{self, resolved};
use crate::card::{BASIC_FOR_COLOR, Card, ColorSet};
use crate::db::{CardDb, Collection, Rules};
use crate::deck::{Deck, PIPER};
use crate::playtest::Prepared;
use crate::playtest::compile::compile;
pub use crate::playtest::lab::Measure;
use crate::playtest::lab::{Outcomes, Variant, play_all};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// How hard the lab looks: games per stage. A 4-player game takes ~3 ms of one core.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Effort {
    pub name: &'static str,
    /// Games per variant when measuring each card's worth.
    pub screen: u32,
    /// Games per variant when trying additions.
    pub additions: u32,
    /// Bench cards tried as additions (best heuristic scores first).
    pub bench: usize,
    /// How many of the least valuable cards each addition is tried in place of.
    pub cut_slots: usize,
    /// Games confirming the best combination, on fresh seeds.
    pub confirm: u32,
    /// Games per colour set.
    pub colors: u32,
    /// Most swaps in the combination.
    pub max_swaps: usize,
}

impl Effort {
    /// About a minute on 4 cores.
    pub const QUICK: Effort = Effort {
        name: "quick",
        screen: 800,
        additions: 600,
        bench: 12,
        cut_slots: 2,
        confirm: 3000,
        colors: 2000,
        max_swaps: 4,
    };
    /// About five minutes on 4 cores.
    pub const THOROUGH: Effort = Effort {
        name: "thorough",
        screen: 3000,
        additions: 2400,
        bench: 15,
        cut_slots: 3,
        confirm: 10000,
        colors: 6000,
        max_swaps: 6,
    };

    pub fn parse(s: &str) -> Option<Effort> {
        match s.trim().to_lowercase().as_str() {
            "" | "quick" | "q" | "fast" => Some(Effort::QUICK),
            "thorough" | "t" | "full" | "slow" => Some(Effort::THOROUGH),
            _ => None,
        }
    }
}

/// Seeds for each stage: every stage plays different games, so a change picked because it
/// screened well is judged on games that didn't pick it.
const SEED_WORTH: u64 = 0x1AB_0001;
const SEED_ADDITIONS: u64 = 0x1AB_0002;
const SEED_CONFIRM: u64 = 0x1AB_0003;
const SEED_RECONFIRM: u64 = 0x1AB_0004;
const SEED_COLORS: u64 = 0x1AB_0005;
const SEED_LANDS: u64 = 0x1AB_0006;

/// The stand-in a card's worth is measured against when the bench has no spell left and
/// the deck is at its land goal (a basic would make the measurement about the land count):
/// a plain colourless 2/2 for two.
pub const FILLER: &str = "Filler (colourless 2/2 for {2})";

fn filler_card() -> Card {
    Card {
        name: FILLER.into(),
        mana_cost: "{2}".into(),
        cmc: 2.0,
        type_line: "Artifact Creature — Construct".into(),
        power: Some("2".into()),
        toughness: Some("2".into()),
        rarity: "common".into(),
        confidence: "verified".into(),
        ..Default::default()
    }
}

/// Shared with the caller while the lab runs: games played so far, the expected total,
/// what it's doing, and a stop button.
#[derive(Default)]
pub struct Progress {
    /// Deck-games played.
    pub done: AtomicUsize,
    /// Expected deck-games in all (an estimate until the last stage is planned).
    pub total: AtomicUsize,
    pub cancel: AtomicBool,
    stage: Mutex<String>,
}

impl Progress {
    pub fn new() -> Progress {
        Progress::default()
    }

    pub fn fraction(&self) -> f32 {
        let t = self.total.load(Ordering::Relaxed).max(1);
        (self.done.load(Ordering::Relaxed) as f32 / t as f32).clamp(0.0, 1.0)
    }

    pub fn stage(&self) -> String {
        self.stage.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn set_stage(&self, s: &str) {
        if let Ok(mut st) = self.stage.lock() {
            *st = s.to_string();
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// One card out, one card in.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Swap {
    pub cut: String,
    pub add: String,
}

impl Swap {
    fn new(cut: &str, add: &str) -> Swap {
        Swap {
            cut: cut.to_string(),
            add: add.to_string(),
        }
    }
}

/// What a card in the deck is worth: win rate lost when one copy is replaced by
/// `replacement` (positive = the card earns its slot).
#[derive(Debug, Clone)]
pub struct CardWorth {
    pub name: String,
    pub copies: u32,
    pub replacement: String,
    pub worth: Measure,
    /// The deck builder's score for it.
    pub heuristic: f32,
    /// Replacing it helped, and by clearly more (2 standard errors) than replacing the
    /// deck's typical (median) card.
    pub dead_weight: bool,
}

/// A bench card's best measured place in the deck.
#[derive(Debug, Clone)]
pub struct Addition {
    pub add: String,
    pub cut: String,
    pub gain: Measure,
    pub heuristic: f32,
}

/// One land more (+1) or fewer (-1): a check of the land count, measured by turning the
/// weakest spell into a basic (or a basic into the best bench card).
#[derive(Debug, Clone)]
pub struct LandChange {
    pub delta: i32,
    pub lands: u32,
    pub swap: Swap,
    pub gain: Measure,
    /// The change goes past the league's land goal from the right side (more lands at or
    /// above it, fewer at or below it): measured, but never suggested as a swap.
    pub past_goal: bool,
    /// One more land measured again on fresh games (done when the deck is at its land goal
    /// and the screen said it gains clearly).
    pub confirmed: Option<Measure>,
}

/// The deck rebuilt in a colour set.
#[derive(Debug, Clone)]
pub struct ColorOption {
    pub colors: ColorSet,
    /// The tested deck is this build.
    pub current: bool,
    pub win: f32,
    /// Win rate against the tested deck on the same opponents and seats (not paired
    /// card-for-card: the decks differ too much).
    pub vs_current: Measure,
    pub games: u32,
    pub deck: Deck,
}

/// Swaps that were combined and confirmed on fresh games.
#[derive(Debug, Clone)]
pub struct Combination {
    /// Each swap with what it adds to the combination (combination minus the
    /// combination without it, on the confirming games).
    pub swaps: Vec<(Swap, Measure)>,
    /// Confirmed gain of the whole combination over the tested deck.
    pub gain: Measure,
    /// What the screening games said (biased upwards: shown for comparison only).
    pub screened: f32,
    /// The tested deck with the swaps made.
    pub deck: Deck,
}

#[derive(Debug, Clone)]
pub struct LabReport {
    pub commander: String,
    pub effort: Effort,
    /// The deck that was tested.
    pub base: Deck,
    /// The deck wasn't complete, so the lab tested the auto-built deck instead.
    pub base_auto: bool,
    /// The deck the lab was given (the same as `base` unless `base_auto`).
    pub source: Deck,
    pub base_win: f32,
    pub base_games: u32,
    /// Every nonland card, most valuable first.
    pub worth: Vec<CardWorth>,
    /// Best measured gain for each bench card tried, best first.
    pub additions: Vec<Addition>,
    pub lands: Vec<LandChange>,
    /// Lands in the tested deck, and the league's land goal (`analysis::land_goal`): at or
    /// above it the lab suggests no swap of a spell for a basic.
    pub land_count: u32,
    pub land_goal: u32,
    /// The deck measured clearly short of mana at its land goal: what to do about it.
    pub mana_note: Option<String>,
    /// Colour sets, best first (empty when the commander allows only one).
    pub colors: Vec<ColorOption>,
    /// The combination the playtests support (confirmed gain > 2 standard errors).
    pub best: Option<Combination>,
    /// The combination tried, when it didn't hold up.
    pub rejected: Option<Combination>,
    pub games: usize,
    pub seconds: f32,
    pub notes: Vec<String>,
}

/// `deck` with the swaps made (one copy out, one in each).
pub fn apply_swaps(deck: &Deck, swaps: &[Swap], db: &CardDb) -> Deck {
    let mut d = deck.clone();
    for s in swaps {
        let key = key_for(&d, &s.cut, db).unwrap_or_else(|| s.cut.clone());
        d.remove(&key, 1);
        let key = key_for(&d, &s.add, db).unwrap_or_else(|| s.add.clone());
        d.add(&key, 1);
    }
    d
}

/// The deck's own spelling of a card name.
fn key_for(deck: &Deck, name: &str, db: &CardDb) -> Option<String> {
    deck.cards
        .keys()
        .find(|k| *k == name || db.get(k).is_some_and(|c| c.name == name))
        .cloned()
}

/// Colours the deck actually casts: its spells' costs, the commander's, its basics.
fn played_colors(deck: &Deck, db: &CardDb) -> ColorSet {
    let mut set = deck
        .commander
        .as_ref()
        .and_then(|c| db.get(c))
        .map(|c| c.cost_colors())
        .unwrap_or_default();
    for (c, _) in resolved(deck, db) {
        if c.is_land() {
            if c.is_basic() {
                set = set.union(c.produces());
            }
        } else {
            set = set.union(c.cost_colors());
            for h in c.hybrid() {
                set = set.union(h);
            }
        }
    }
    match deck.identity(db) {
        Some(id) => ColorSet(set.0 & id.0),
        None => set,
    }
}

/// The basic land that best fits the deck's colour needs.
fn basic_for(deck: &Deck, db: &CardDb) -> Option<String> {
    let split = analysis::basic_split(deck, db, 1);
    (0..5)
        .find(|&i| split[i] > 0)
        .map(|i| BASIC_FOR_COLOR[i].to_string())
        .filter(|n| db.get(n).is_some())
}

/// The colours the deck casts with the fewest mana sources for what its spells ask of them
/// (sources per coloured pip, the commander's counting double): the lowest one, and a
/// second when it's about as short. Empty when nothing stands out.
fn short_colors(deck: &Deck, db: &CardDb) -> String {
    let st = analysis::stats(deck, db);
    let cmd = deck
        .commander
        .as_ref()
        .and_then(|c| db.get(c))
        .map(|c| c.pips())
        .unwrap_or([0; 5]);
    let mut need: Vec<(f32, usize)> = (0..5)
        .filter(|&i| st.pips[i] + cmd[i] > 0)
        .map(|i| {
            let want = (st.pips[i] + 2 * cmd[i]) as f32;
            (st.sources[i] as f32 / want, i)
        })
        .collect();
    if need.len() < 2 {
        return String::new();
    }
    need.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let median = need[need.len() / 2].0;
    let short: Vec<&str> = need
        .iter()
        .take(2)
        .filter(|(r, _)| *r < 0.8 * median)
        .map(|&(_, i)| analysis::color_name(i))
        .collect();
    short.join(" and ").to_lowercase()
}

/// Playtests changes to `deck` (or, if it isn't a complete deck for `commander`, to the
/// deck auto-build makes) against simulated league opponents. Returns None if cancelled.
pub fn explore(
    commander: &Card,
    deck: &Deck,
    db: &CardDb,
    coll: &Collection,
    rules: &Rules,
    effort: Effort,
    progress: &Progress,
) -> Option<LabReport> {
    progress.set_stage("Building the opponents' decks");
    let bed = crate::ranking::TestBed::new(db, rules);
    explore_with(
        commander,
        deck,
        db,
        coll,
        rules,
        effort,
        &bed.gauntlet,
        progress,
    )
}

/// `explore` against the given opponents.
#[allow(clippy::too_many_arguments)]
pub fn explore_with(
    commander: &Card,
    deck: &Deck,
    db: &CardDb,
    coll: &Collection,
    rules: &Rules,
    effort: Effort,
    gauntlet: &[Prepared],
    progress: &Progress,
) -> Option<LabReport> {
    let start = std::time::Instant::now();
    let mut notes = Vec::new();
    let same_cmd = deck.commander.as_deref() == Some(commander.name.as_str());
    let piper = if same_cmd && deck.partner.as_deref() == Some(PIPER) {
        deck.piper_color
    } else {
        None
    };
    let complete = same_cmd && deck.main_count() >= deck.main_target(rules);
    let base = if complete {
        deck.clone()
    } else {
        progress.set_stage("Auto-building the deck to test");
        let mut d = analysis::autobuild(commander, db, coll, rules, None, piper).0;
        d.name = deck.name.clone();
        notes.push(
            "Your deck isn't complete, so the lab tested the deck auto-build makes.".to_string(),
        );
        d
    };
    if gauntlet.is_empty() {
        notes.push("No opponent decks to play against (no league roster?).".to_string());
    }
    let prep = Prepared::new(&base, db);
    let base_lands = analysis::land_count(&base, db);
    // At the league's land goal (26 of 60) a spell isn't swapped for a basic, and below it
    // a basic isn't cut: the land count is a check of its own, reported apart.
    let land_goal = analysis::land_goal(rules);
    let at_goal = base_lands >= land_goal;
    let is_land = |n: &str| db.get(n).is_some_and(|c| c.is_land());
    let swap_ok = |s: &Swap| {
        !(at_goal && is_land(&s.add) && !is_land(&s.cut))
            && !(base_lands <= land_goal && is_land(&s.cut) && !is_land(&s.add))
            && s.add != FILLER
    };

    // ------------------------------------------------------------ the bench
    let plays = played_colors(&base, db);
    let bench: Vec<(String, f32)> =
        analysis::suggestions_within(&base, db, coll, rules, Some(plays))
            .into_iter()
            .filter(|s| {
                rules.max_copies == 0
                    || s.card.is_basic()
                    || base.used(&s.card.name) < rules.max_copies
            })
            .map(|s| (s.card.name.clone(), s.score))
            .collect();
    let mut heuristic: std::collections::HashMap<String, f32> = analysis::weakest(&base, db, rules)
        .into_iter()
        .map(|s| (s.card.name.clone(), s.score))
        .chain(bench.iter().cloned())
        .collect();
    let basic = basic_for(&base, db);
    let filler = compile(&filler_card());
    let variant = |swaps: &[Swap]| -> Variant {
        Variant {
            swaps: swaps
                .iter()
                .filter_map(|s| {
                    let add = if s.add == FILLER {
                        filler.clone()
                    } else {
                        compile(db.get(&s.add)?)
                    };
                    Some((prep.card_index(&s.cut)?, add))
                })
                .collect(),
        }
    };
    let spells: Vec<(&Card, u32)> = resolved(&base, db)
        .into_iter()
        .filter(|(c, _)| !c.is_land())
        .collect();
    let colour_sets = color_options(commander, piper, db, coll, rules);

    // Rough plan of the work, refined as stages are planned.
    let stage_a = spells.len() + 3;
    let stage_b = effort.cut_slots * (effort.bench.min(bench.len()) + 1) + 1;
    let stage_c = 2 * effort.max_swaps.min(4) + 2;
    let colours = if colour_sets.len() > 1 {
        colour_sets.len() + 1
    } else {
        0
    };
    let plan = |a: usize, b: usize, c: usize, d: usize| {
        a * effort.screen as usize
            + b * effort.additions as usize
            + c * effort.confirm as usize
            + d * effort.colors as usize
    };
    progress.total.store(
        progress.done.load(Ordering::Relaxed) + plan(stage_a, stage_b, stage_c, colours),
        Ordering::Relaxed,
    );
    let replan = |b: usize, c: usize, d: usize| {
        progress.total.store(
            progress.done.load(Ordering::Relaxed) + plan(0, b, c, d),
            Ordering::Relaxed,
        );
    };
    let mut base_wins = (0.0f64, 0u32);
    let mut add_base = |o: &Outcomes| {
        base_wins.0 += o.win_rate(0) as f64 * o.games as f64;
        base_wins.1 += o.games;
    };
    let run = |decks: &[Vec<Swap>], games: u32, seed: u64| -> Option<Outcomes> {
        let built: Vec<Prepared> = decks
            .iter()
            .map(|s| prep.with_swaps(&variant(s).swaps))
            .collect();
        let mut all: Vec<&Prepared> = vec![&prep];
        all.extend(built.iter());
        let o = play_all(
            &all,
            gauntlet,
            rules,
            games,
            seed,
            &progress.done,
            &progress.cancel,
        );
        (!progress.cancelled()).then_some(o)
    };
    // Every screened (cut, add) measurement, pooled when measured more than once. Land
    // checks past the goal (not `swap_ok`) are kept apart: they aren't swaps to make.
    let mut screened: Vec<(Swap, Measure)> = Vec::new();
    let mut land_checks: Vec<(Swap, Measure)> = Vec::new();
    let mut record = |s: &Swap, m: Measure| {
        let list = if swap_ok(s) {
            &mut screened
        } else {
            &mut land_checks
        };
        match list.iter_mut().find(|(x, _)| x == s) {
            Some((_, old)) => *old = old.pool(&m),
            None => list.push((s.clone(), m)),
        }
    };

    // ------------------------------------------------------------ 1. each card's worth
    progress.set_stage("Measuring what each card is worth");
    let mut worth_swaps: Vec<Swap> = Vec::new();
    for (c, _) in &spells {
        // The best spell left on the bench (bench cards are never lands); with none, a
        // basic below the land goal, and at it a plain 2/2 stand-in.
        let repl = bench
            .iter()
            .map(|b| b.0.clone())
            .find(|b| *b != c.name)
            .or_else(|| {
                if at_goal {
                    Some(FILLER.to_string())
                } else {
                    basic.clone()
                }
            });
        if let Some(r) = repl {
            worth_swaps.push(Swap::new(&c.name, &r));
        }
    }
    // One land more: the weakest spell by score becomes a basic. One fewer: the most
    // common basic becomes the best bench card.
    let weakest_spell = analysis::weakest(&base, db, rules)
        .first()
        .map(|s| s.card.name.clone());
    let common_basic = base
        .cards
        .iter()
        .filter(|(n, _)| db.get(n).is_some_and(|c| c.is_basic()))
        .max_by_key(|(_, k)| **k)
        .and_then(|(n, _)| db.get(n).map(|c| c.name.clone()));
    let plus_land = weakest_spell
        .zip(basic.clone())
        .map(|(w, b)| Swap::new(&w, &b));
    let minus_land = common_basic
        .zip(bench.first().map(|b| b.0.clone()))
        .map(|(b, s)| Swap::new(&b, &s));
    let mut a_decks: Vec<Vec<Swap>> = worth_swaps.iter().map(|s| vec![s.clone()]).collect();
    a_decks.extend(plus_land.iter().map(|s| vec![s.clone()]));
    a_decks.extend(minus_land.iter().map(|s| vec![s.clone()]));
    let out_a = run(&a_decks, effort.screen, SEED_WORTH)?;
    add_base(&out_a);
    for (i, d) in a_decks.iter().enumerate() {
        record(&d[0], out_a.diff(i + 1, 0));
    }
    let mut worth: Vec<CardWorth> = worth_swaps
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let gain = out_a.diff(i + 1, 0);
            let w = Measure {
                gain: -gain.gain,
                ..gain
            };
            CardWorth {
                name: s.cut.clone(),
                copies: base.count_of(&key_for(&base, &s.cut, db).unwrap_or_default()),
                replacement: s.add.clone(),
                worth: w,
                heuristic: heuristic.get(&s.cut).copied().unwrap_or(0.0),
                dead_weight: false,
            }
        })
        .collect();
    worth.sort_by(|a, b| b.worth.gain.partial_cmp(&a.worth.gain).unwrap());
    // When the best bench card is strong, replacing almost anything with it helps; dead
    // weight stands out from that: the swap helps, and clearly more than for the deck's
    // typical card.
    let typical = worth.get(worth.len() / 2).map_or(0.0, |w| w.worth.gain);
    for w in worth.iter_mut() {
        w.dead_weight = w.worth.gain < 0.0 && w.worth.gain + 2.0 * w.worth.stderr < typical;
    }

    // ------------------------------------------------------------ 2. additions
    progress.set_stage("Trying additions from your pool");
    // The cards to make room from: the least valuable (a basic counts if one land fewer
    // measured well).
    let mut cut_rank: Vec<(String, f32)> = worth
        .iter()
        .map(|w| (w.name.clone(), w.worth.gain))
        .collect();
    if let Some(m) = minus_land.as_ref().filter(|m| swap_ok(m)) {
        let g = out_a.diff(a_decks.len(), 0);
        cut_rank.push((m.cut.clone(), -g.gain));
    }
    cut_rank.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    let cuts: Vec<String> = cut_rank
        .iter()
        .take(effort.cut_slots)
        .map(|c| c.0.clone())
        .collect();
    let mut adds = additions_to_try(&base, &bench, plays, db, coll, rules, effort.bench);
    for name in &adds {
        if !heuristic.contains_key(name) {
            if let Some(c) = db.get(name) {
                heuristic.insert(name.clone(), analysis::score_card(c, &base, db, rules).0);
            }
        }
    }
    // A basic is an addition only below the land goal.
    if !at_goal {
        adds.extend(basic.clone());
    }
    let is_basic = |n: &str| db.get(n).is_some_and(|c| c.is_basic());
    let mut b_decks: Vec<Vec<Swap>> = Vec::new();
    for cut in &cuts {
        for add in &adds {
            let s = Swap::new(cut, add);
            if add != cut && !(is_basic(add) && is_basic(cut)) && swap_ok(&s) {
                b_decks.push(vec![s]);
            }
        }
    }
    replan(b_decks.len() + 1, stage_c, colours);
    let out_b = run(&b_decks, effort.additions, SEED_ADDITIONS)?;
    add_base(&out_b);
    let mut additions: Vec<Addition> = Vec::new();
    for (i, d) in b_decks.iter().enumerate() {
        let m = out_b.diff(i + 1, 0);
        record(&d[0], m);
        let s = &d[0];
        match additions.iter_mut().find(|a| a.add == s.add) {
            Some(a) if a.gain.gain >= m.gain => {}
            Some(a) => {
                a.cut = s.cut.clone();
                a.gain = m;
            }
            None => additions.push(Addition {
                add: s.add.clone(),
                cut: s.cut.clone(),
                gain: m,
                heuristic: heuristic.get(&s.add).copied().unwrap_or(0.0),
            }),
        }
    }
    additions.sort_by(|a, b| b.gain.gain.partial_cmp(&a.gain.gain).unwrap());

    // ------------------------------------------------------------ 3. best combination
    progress.set_stage("Combining the best swaps");
    let mut candidates: Vec<(Swap, Measure)> = screened
        .iter()
        .filter(|(_, m)| m.gain > 0.0 && m.z() >= 0.5)
        .cloned()
        .collect();
    candidates.sort_by(|a, b| {
        b.1.gain
            .partial_cmp(&a.1.gain)
            .unwrap()
            .then_with(|| a.0.cmp(&b.0))
    });
    let mut combo: Vec<(Swap, Measure)> = Vec::new();
    for (s, m) in candidates {
        if combo.len() >= effort.max_swaps {
            break;
        }
        let cut_n = combo.iter().filter(|(x, _)| x.cut == s.cut).count() as u32;
        let add_n = combo.iter().filter(|(x, _)| x.add == s.add).count() as u32;
        let have = key_for(&base, &s.cut, db).map_or(0, |k| base.count_of(&k));
        let can_add = db
            .get(&s.add)
            .map_or(0, |c| analysis::available(c, &base, coll, rules))
            .min(if rules.max_copies > 0 && !is_basic(&s.add) {
                rules.max_copies.saturating_sub(base.used(&s.add))
            } else {
                99
            });
        let crosses = combo.iter().any(|(x, _)| x.cut == s.add || x.add == s.cut);
        if cut_n < have && add_n < can_add && !crosses {
            combo.push((s, m));
        }
    }
    let swaps_of = |c: &[(Swap, Measure)]| -> Vec<Swap> { c.iter().map(|x| x.0.clone()).collect() };
    let mut best = None;
    let mut rejected = None;
    let colours_games = colours;
    if !combo.is_empty() {
        progress.set_stage("Confirming the best combination on fresh games");
        let all = swaps_of(&combo);
        let mut c_decks = vec![all.clone()];
        if all.len() > 1 {
            for i in 0..all.len() {
                let mut without = all.clone();
                without.remove(i);
                c_decks.push(without);
            }
        }
        replan(0, c_decks.len() + 1 + 2, colours_games);
        let out_c = run(&c_decks, effort.confirm, SEED_CONFIRM)?;
        add_base(&out_c);
        let gain_all = out_c.diff(1, 0);
        let contributions: Vec<Measure> = (0..all.len())
            .map(|i| {
                if all.len() > 1 {
                    out_c.diff(1, i + 2)
                } else {
                    gain_all
                }
            })
            .collect();
        let screened_sum: f32 = combo.iter().map(|(_, m)| m.gain).sum();
        let tried = Combination {
            swaps: all
                .iter()
                .cloned()
                .zip(contributions.iter().copied())
                .collect(),
            gain: gain_all,
            screened: screened_sum,
            deck: apply_swaps(&base, &all, db),
        };
        // A swap holds up if the combination is better with it, by at least half a
        // standard error.
        let keep: Vec<(Swap, Measure)> = tried
            .swaps
            .iter()
            .filter(|(_, m)| m.z() > 0.5)
            .cloned()
            .collect();
        let final_combo = if keep.len() == all.len() {
            Some(tried.clone())
        } else if keep.is_empty() {
            None
        } else {
            // Some swaps didn't hold up: judge what's left on fresh games again (the drop
            // was decided on the confirming games).
            progress.set_stage("Re-confirming without the swaps that didn't hold up");
            let kept = swaps_of(&keep);
            let out_d = run(std::slice::from_ref(&kept), effort.confirm, SEED_RECONFIRM)?;
            add_base(&out_d);
            Some(Combination {
                swaps: keep,
                gain: out_d.diff(1, 0),
                screened: screened_sum,
                deck: apply_swaps(&base, &kept, db),
            })
        };
        match final_combo {
            Some(c) if c.gain.z() > 2.0 => best = Some(c),
            Some(c) => rejected = Some(c),
            None => rejected = Some(tried),
        }
    }

    // ------------------------------------------------------------ land count
    let mut lands = Vec::new();
    for (delta, s) in [(1, &plus_land), (-1, &minus_land)] {
        if let Some(s) = s {
            if let Some((_, m)) = screened
                .iter()
                .chain(land_checks.iter())
                .find(|(x, _)| x == s)
            {
                lands.push(LandChange {
                    delta,
                    lands: (base_lands as i32 + delta).max(0) as u32,
                    swap: s.clone(),
                    gain: *m,
                    past_goal: !swap_ok(s),
                    confirmed: None,
                });
            }
        }
    }
    // At the goal, a clear gain from one more land is checked on fresh games: if it holds,
    // the deck is short of mana (or of a colour) - better fixing is the answer, not a spell
    // cut for a basic.
    let mut mana_note = None;
    if let Some(l) = lands
        .iter_mut()
        .find(|l| l.delta > 0 && l.past_goal && l.gain.gain > 0.0 && l.gain.z() >= 2.0)
    {
        progress.set_stage("Checking the mana base on fresh games");
        replan(0, 2, colours);
        let out_l = run(
            std::slice::from_ref(&vec![l.swap.clone()]),
            effort.confirm,
            SEED_LANDS,
        )?;
        add_base(&out_l);
        let m = out_l.diff(1, 0);
        l.confirmed = Some(m);
        if m.z() >= 2.0 && m.gain >= 0.01 {
            mana_note = Some(format!(
                "The deck plays short of mana: one more land measured {} pts on {} fresh games. \
                 It's at the league's {}-land goal, so rather than cutting a spell for a basic, \
                 consider better fixing (dual lands, mana rocks, land search){}.",
                pts_se(&m),
                m.games,
                land_goal,
                match short_colors(&base, db) {
                    s if s.is_empty() => String::new(),
                    s => format!(" and more sources of {}", s),
                }
            ));
        }
    }

    // ------------------------------------------------------------ 4. colour sets
    let mut colors = Vec::new();
    if colour_sets.len() > 1 {
        progress.set_stage("Rebuilding and playtesting other colour sets");
        replan(0, 0, colours_games);
        let mut decks = Vec::new();
        for &set in &colour_sets {
            let mut d = analysis::autobuild(commander, db, coll, rules, Some(set), None).0;
            d.name = base.name.clone();
            decks.push((set, d));
        }
        let preps: Vec<Prepared> = decks.iter().map(|(_, d)| Prepared::new(d, db)).collect();
        let mut all: Vec<&Prepared> = vec![&prep];
        all.extend(preps.iter());
        let out = play_all(
            &all,
            gauntlet,
            rules,
            effort.colors,
            SEED_COLORS,
            &progress.done,
            &progress.cancel,
        );
        if progress.cancelled() {
            return None;
        }
        add_base(&out);
        for (i, (set, d)) in decks.into_iter().enumerate() {
            let current = d.cards == base.cards;
            colors.push(ColorOption {
                colors: set,
                current,
                win: out.win_rate(i + 1),
                vs_current: out.diff(i + 1, 0),
                games: out.games,
                deck: d,
            });
        }
        if !colors.iter().any(|c| c.current) {
            colors.push(ColorOption {
                colors: played_colors(&base, db),
                current: true,
                win: out.win_rate(0),
                vs_current: Measure {
                    games: out.games,
                    ..Default::default()
                },
                games: out.games,
                deck: base.clone(),
            });
        }
        colors.sort_by(|a, b| b.win.partial_cmp(&a.win).unwrap());
    }
    progress
        .total
        .store(progress.done.load(Ordering::Relaxed), Ordering::Relaxed);
    progress.set_stage("Done");
    Some(LabReport {
        commander: commander.name.clone(),
        effort,
        base,
        base_auto: !complete,
        source: deck.clone(),
        base_win: if base_wins.1 > 0 {
            (base_wins.0 / base_wins.1 as f64) as f32
        } else {
            0.25
        },
        base_games: base_wins.1,
        worth,
        additions,
        lands,
        land_count: base_lands,
        land_goal,
        mana_note,
        colors,
        best,
        rejected,
        games: progress.done.load(Ordering::Relaxed),
        seconds: start.elapsed().as_secs_f32(),
        notes,
    })
}

/// The bench cards tried as additions (`n` of them). Mostly the builder's best by score,
/// but its score isn't the whole story - the playtests keep finding big bodies it ranks low
/// (Thor Odinson, Kree Sentinel) - so about a fifth are the biggest creatures it ranks lower
/// (body and keywords), and a sixth the best cards in a colour the deck doesn't cast yet but
/// its lands already make (a Thanos deck's gain lands make all five colours).
fn additions_to_try(
    base: &Deck,
    bench: &[(String, f32)],
    plays: ColorSet,
    db: &CardDb,
    coll: &Collection,
    rules: &Rules,
    n: usize,
) -> Vec<String> {
    let off_n = n / 6;
    let body_n = n / 5;
    let mut out: Vec<String> = bench
        .iter()
        .take(n - off_n - body_n)
        .map(|b| b.0.clone())
        .collect();
    let mut bodies: Vec<(f32, &String)> = bench
        .iter()
        .filter(|b| !out.contains(&b.0))
        .filter_map(|b| {
            let c = db.get(&b.0)?;
            c.is_creature().then(|| {
                let v = crate::value::creature_value(c);
                (v.body + v.keywords, &b.0)
            })
        })
        .collect();
    bodies.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    out.extend(bodies.into_iter().take(body_n).map(|b| b.1.clone()));
    let sources = analysis::stats(base, db).sources;
    let off: Vec<String> = analysis::suggestions_within(base, db, coll, rules, None)
        .into_iter()
        .filter(|s| !s.card.playable_with(plays))
        .filter(|s| rules.max_copies == 0 || base.used(&s.card.name) < rules.max_copies)
        .filter(|s| s.card.cost_colors().indices().all(|i| sources[i] >= 3))
        .map(|s| s.card.name.clone())
        .filter(|name| !out.contains(name))
        .take(off_n)
        .collect();
    out.extend(off);
    // Fill up from the score order when a group came up short.
    for b in bench {
        if out.len() >= n {
            break;
        }
        if !out.contains(&b.0) {
            out.push(b.0.clone());
        }
    }
    out
}

/// Colour sets worth comparing: every subset of the identity that keeps the commander's
/// own cost colours (at most the six with the strongest pools). Just one when there's no
/// choice.
fn color_options(
    cmd: &Card,
    piper: Option<char>,
    db: &CardDb,
    coll: &Collection,
    rules: &Rules,
) -> Vec<ColorSet> {
    let id = cmd.identity();
    if piper.is_some() || id.count() <= 1 {
        return vec![id];
    }
    let base = cmd.cost_colors();
    let mut sets: Vec<(f32, ColorSet)> = (1u8..32)
        .map(ColorSet)
        .filter(|s| s.is_subset_of(id) && base.is_subset_of(*s))
        .map(|s| (analysis::pool_quality_for(cmd, s, db, coll, rules), s))
        .collect();
    sets.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.0.cmp(&b.1.0)));
    sets.into_iter().take(6).map(|(_, s)| s).collect()
}

/// Win-rate points with sign: "+2.1".
pub fn pts(x: f32) -> String {
    format!("{:+.1}", x * 100.0)
}

/// "+2.1 ± 0.8".
pub fn pts_se(m: &Measure) -> String {
    format!("{} ± {:.1}", pts(m.gain), m.stderr * 100.0)
}

impl LabReport {
    /// The report as plain text (for the command line).
    pub fn text(&self) -> String {
        let mut o = String::new();
        let mins = (self.seconds / 60.0).floor();
        o.push_str(&format!(
            "Playtest lab - {} ({}: {} games in {}m {:02}s)\n",
            self.commander,
            self.effort.name,
            self.games,
            mins,
            (self.seconds - mins * 60.0).round() as u32
        ));
        for n in &self.notes {
            o.push_str(&format!("  {}\n", n));
        }
        o.push_str(&format!(
            "The tested deck won {:.1}% of {} games (4 players: 25% = even).\n",
            self.base_win * 100.0,
            self.base_games
        ));
        o.push_str(
            "Numbers are win-rate points ± one standard error, from paired games (the deck and\n\
             the changed deck play the same games). Within ±2 standard errors is noise.\n\n",
        );
        match &self.best {
            Some(c) => {
                o.push_str(&format!(
                    "BEST COMBINATION the playtests support: {} pts (confirmed on {} fresh games)\n",
                    pts_se(&c.gain),
                    c.gain.games
                ));
                for (s, m) in &c.swaps {
                    o.push_str(&format!(
                        "  cut {:<32} add {:<32} {}\n",
                        s.cut,
                        s.add,
                        pts_se(m)
                    ));
                }
                o.push_str(
                    "  (each swap's number: the combination minus the combination without it)\n",
                );
            }
            None => {
                o.push_str("BEST COMBINATION: no change beats the current build.\n");
                if let Some(c) = &self.rejected {
                    let list: Vec<String> = c
                        .swaps
                        .iter()
                        .map(|(s, _)| format!("{} -> {}", s.cut, s.add))
                        .collect();
                    o.push_str(&format!(
                        "  Best try ({}) measured {} pts on {} fresh games - not clearly better.\n",
                        list.join(", "),
                        pts_se(&c.gain),
                        c.gain.games
                    ));
                }
            }
        }
        o.push('\n');
        if let Some(r) = self.worth.first().map(|w| w.replacement.clone()) {
            o.push_str(&format!(
                "WHAT EACH CARD IS WORTH (screening, {} games each): points lost when it's replaced by the\n\
                 best spell left on your bench ({}). Dead weight = replacing it helped, clearly more\n\
                 than replacing a typical card of the deck. Score = the deck builder's estimate.\n",
                self.worth.first().map_or(0, |w| w.worth.games),
                r
            ));
        }
        for w in &self.worth {
            o.push_str(&format!(
                "  {:>12}  {:<34} score {:>5.1}{}{}\n",
                pts_se(&w.worth),
                if w.copies > 1 {
                    format!("{} (1 of {})", w.name, w.copies)
                } else {
                    w.name.clone()
                },
                w.heuristic,
                if w.replacement != self.worth[0].replacement {
                    format!("  (vs {})", w.replacement)
                } else {
                    String::new()
                },
                if w.dead_weight { "  dead weight" } else { "" }
            ));
        }
        if !self.additions.is_empty() {
            o.push_str(&format!(
                "\nADDITIONS (screening, {} games each): each bench card in place of the least valuable cards\n",
                self.additions[0].gain.games
            ));
            for a in &self.additions {
                o.push_str(&format!(
                    "  {:>12}  add {:<32} for {:<30} score {:>5.1}\n",
                    pts_se(&a.gain),
                    a.add,
                    a.cut,
                    a.heuristic
                ));
            }
        }
        if !self.lands.is_empty() {
            o.push_str(&format!(
                "\nLAND COUNT (a check of its own, screening): the deck has {} lands, the league's goal is {}.\n",
                self.land_count, self.land_goal
            ));
            for l in &self.lands {
                o.push_str(&format!(
                    "  {:>12}  {} lands ({}: cut {}, add {}){}\n",
                    pts_se(&l.gain),
                    l.lands,
                    if l.delta > 0 {
                        "one more land"
                    } else {
                        "one fewer land"
                    },
                    l.swap.cut,
                    l.swap.add,
                    if l.past_goal {
                        " - measured only, not a swap to make"
                    } else {
                        ""
                    }
                ));
                if let Some(c) = &l.confirmed {
                    o.push_str(&format!(
                        "  {:>12}  the same on {} fresh games\n",
                        pts_se(c),
                        c.games
                    ));
                }
            }
            if let Some(n) = &self.mana_note {
                o.push_str(&format!("  MANA BASE: {}\n", n));
            }
        }
        if !self.colors.is_empty() {
            o.push_str(&format!(
                "\nCOLOUR SETS (each rebuilt by auto-build, {} games each, same opponents and seats)\n",
                self.colors[0].games
            ));
            for c in &self.colors {
                o.push_str(&format!(
                    "  {:<6} won {:>5.1}%  {}\n",
                    c.colors.to_string(),
                    c.win * 100.0,
                    if c.current {
                        "(the tested deck)".to_string()
                    } else {
                        format!("{} pts vs the tested deck", pts_se(&c.vs_current))
                    }
                ));
            }
        }
        o
    }

    /// Whether the report is about `deck` as it is now (so its suggestions fit it): the
    /// same commander, and the deck the lab was given or tested.
    pub fn fits(&self, deck: &Deck) -> bool {
        deck.commander.as_deref() == Some(self.commander.as_str())
            && (deck.cards == self.base.cards || deck.cards == self.source.cards)
    }

    /// A colour set that measured clearly better than the tested deck.
    pub fn better_colors(&self) -> Option<&ColorOption> {
        self.colors
            .iter()
            .filter(|c| !c.current && c.vs_current.z() > 2.0)
            .max_by(|a, b| a.win.partial_cmp(&b.win).unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::COLORS;
    use crate::playtest::lab::{paired, play_all_threads};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn mk(name: &str, cost: &str, ty: &str, id: &str, p: u32, t: u32) -> Card {
        Card {
            name: name.into(),
            mana_cost: cost.into(),
            cmc: crate::card::symbols_in(cost)
                .iter()
                .map(|s| s.parse::<f32>().unwrap_or(1.0))
                .sum(),
            type_line: ty.into(),
            power: Some(p.to_string()),
            toughness: Some(t.to_string()),
            rarity: "common".into(),
            color_identity: id.chars().map(|c| c.to_string()).collect(),
            confidence: "verified".into(),
            ..Default::default()
        }
    }

    /// A small pool of vanilla creatures: a black-red commander whose identity also has
    /// green, two rival commanders, a strong "Titan" and a useless 0/1 "Wall".
    fn setup() -> (CardDb, Collection) {
        let mut cards = vec![
            mk(
                "Boss",
                "{2}{B}{R}",
                "Legendary Creature — Human Villain",
                "BRG",
                3,
                3,
            ),
            mk("Rival", "{2}{G}{G}", "Legendary Creature — Elf", "G", 4, 4),
            mk(
                "Other Rival",
                "{1}{W}{W}",
                "Legendary Creature — Human",
                "W",
                3,
                3,
            ),
            mk("Titan", "{1}{R}", "Creature — Giant", "R", 6, 6),
            mk("Wall", "{1}", "Artifact Creature — Wall", "", 0, 1),
        ];
        let mut counts = BTreeMap::new();
        for n in ["Boss", "Rival", "Other Rival"] {
            counts.insert(n.to_string(), 1);
        }
        counts.insert("Titan".to_string(), 4);
        counts.insert("Wall".to_string(), 4);
        for i in 0..80 {
            let c = ["B", "R", "G", "W"][i % 4];
            let mv = 1 + (i / 4) % 5;
            let cost = if mv == 1 {
                format!("{{{}}}", c)
            } else {
                format!("{{{}}}{{{}}}", mv - 1, c)
            };
            let name = format!("Goon {}", i);
            let (p, t) = (mv as u32, mv as u32 + (i % 3 == 0) as u32);
            cards.push(mk(&name, &cost, "Creature — Human Soldier", c, p, t));
            counts.insert(name, 1);
        }
        for (i, b) in BASIC_FOR_COLOR.iter().enumerate() {
            cards.push(Card::basic_land(b, COLORS[i]));
        }
        cards.sort_by(|a, b| a.name.cmp(&b.name));
        (CardDb::new(cards), Collection::new(PathBuf::new(), counts))
    }

    fn opponents(db: &CardDb, coll: &Collection, rules: &Rules) -> Vec<Prepared> {
        ["Rival", "Other Rival"]
            .iter()
            .map(|n| {
                let d = analysis::autobuild(db.get(n).unwrap(), db, coll, rules, None, None).0;
                Prepared::new(&d, db)
            })
            .collect()
    }

    /// Boss with twelve Titans, black and red Goons and basics.
    fn titan_deck() -> Deck {
        let mut d = Deck::new("titans");
        d.commander = Some("Boss".into());
        d.add("Titan", 12);
        for i in 0..80 {
            if i % 4 < 2 && d.main_count() < 35 {
                d.add(&format!("Goon {}", i), 1);
            }
        }
        d.add("Mountain", 12);
        d.add("Swamp", 12);
        assert_eq!(d.main_count(), 59);
        d
    }

    const TINY: Effort = Effort {
        name: "test",
        screen: 60,
        additions: 60,
        bench: 3,
        cut_slots: 2,
        confirm: 150,
        colors: 60,
        max_swaps: 3,
    };

    /// Boss with twelve Titans, black and red Goons and `lands` basics (60 cards).
    fn deck_with_lands(lands: u32) -> Deck {
        let mut d = Deck::new("lands");
        d.commander = Some("Boss".into());
        d.add("Titan", 12);
        for i in 0..80 {
            if i % 4 < 2 && d.main_count() < 59 - lands {
                d.add(&format!("Goon {}", i), 1);
            }
        }
        d.add("Mountain", lands / 2);
        d.add("Swamp", lands - lands / 2);
        assert_eq!(d.main_count(), 59);
        d
    }

    /// The additions tried aren't only the builder's top scores: big bodies it ranks lower,
    /// and cards in a colour the deck's lands make but its spells don't use yet.
    #[test]
    fn additions_reach_past_the_top_scores() {
        const GREEN: usize = 4; // W U B R G
        let (db, coll) = setup();
        let mut cards = db.cards.clone();
        let mut grove = mk("Grove", "", "Land", "BG", 0, 0);
        grove.power = None;
        grove.toughness = None;
        grove.oracle_text = "{T}: Add {B} or {G}.".into();
        cards.push(grove);
        cards.sort_by(|a, b| a.name.cmp(&b.name));
        let db = CardDb::new(cards);
        let mut counts = coll.counts.clone();
        counts.insert("Grove".into(), 3);
        let coll = Collection::new(PathBuf::new(), counts);
        let rules = Rules::default();
        let mut deck = titan_deck();
        deck.remove("Swamp", 3);
        deck.add("Grove", 3);
        let plays = played_colors(&deck, &db);
        assert!(!plays.contains(GREEN), "green isn't played: {}", plays);
        let bench: Vec<(String, f32)> =
            analysis::suggestions_within(&deck, &db, &coll, &rules, Some(plays))
                .into_iter()
                .map(|s| (s.card.name.clone(), s.score))
                .collect();
        let adds = additions_to_try(&deck, &bench, plays, &db, &coll, &rules, 12);
        assert_eq!(adds.len(), 12);
        let green = adds
            .iter()
            .filter(|n| db.get(n).unwrap().cost_colors().contains(GREEN))
            .count();
        assert_eq!(green, 2, "{:?}", adds);
        let top: Vec<&String> = bench.iter().take(8).map(|b| &b.0).collect();
        let beyond = adds
            .iter()
            .filter(|n| !top.contains(n) && !db.get(n).unwrap().cost_colors().contains(GREEN))
            .count();
        assert_eq!(beyond, 2, "two big bodies from further down: {:?}", adds);
        // A deck with no green sources gets no green additions.
        let adds = additions_to_try(&titan_deck(), &bench, plays, &db, &coll, &rules, 12);
        assert!(
            adds.iter()
                .all(|n| !db.get(n).unwrap().cost_colors().contains(GREEN)),
            "{:?}",
            adds
        );
    }

    #[test]
    fn no_land_swaps_at_the_land_goal() {
        let (db, coll) = setup();
        let rules = Rules::default();
        assert_eq!(analysis::land_goal(&rules), 26);
        let gauntlet = opponents(&db, &coll, &rules);
        let boss = db.get("Boss").unwrap().clone();
        let is_land = |n: &str| db.get(n).is_some_and(|c| c.is_land());
        let at_goal = deck_with_lands(26);
        let r = explore_with(
            &boss,
            &at_goal,
            &db,
            &coll,
            &rules,
            TINY,
            &gauntlet,
            &Progress::new(),
        )
        .unwrap();
        assert!(!r.base_auto);
        assert_eq!((r.land_count, r.land_goal), (26, 26));
        assert!(r.worth.iter().all(|w| !is_land(&w.replacement)));
        assert!(
            r.additions
                .iter()
                .all(|a| !is_land(&a.add) && !is_land(&a.cut)),
            "no basic in or out at the goal"
        );
        for c in r.best.iter().chain(r.rejected.iter()) {
            assert!(
                c.swaps
                    .iter()
                    .all(|(s, _)| !is_land(&s.add) && !is_land(&s.cut))
            );
        }
        // The land count is still checked, as a measurement only.
        let more = r.lands.iter().find(|l| l.delta > 0).expect("one more land");
        assert_eq!(more.lands, 27);
        assert!(more.past_goal);
        assert!(r.text().contains("LAND COUNT"));
        assert!(r.text().contains("measured only, not a swap to make"));
        // Below the goal a basic is tried as an addition, as before.
        let short = deck_with_lands(24);
        let r = explore_with(
            &boss,
            &short,
            &db,
            &coll,
            &rules,
            TINY,
            &gauntlet,
            &Progress::new(),
        )
        .unwrap();
        assert!(r.additions.iter().any(|a| is_land(&a.add)));
        assert!(!r.lands.iter().find(|l| l.delta > 0).unwrap().past_goal);
    }

    #[test]
    fn identical_variant_gains_exactly_nothing() {
        let (db, coll) = setup();
        let rules = Rules::default();
        let gauntlet = opponents(&db, &coll, &rules);
        let base = Prepared::new(&titan_deck(), &db);
        let titan = base.card_index("Titan").unwrap();
        let same = Variant {
            swaps: vec![(titan, compile(db.get("Titan").unwrap()))],
        };
        let done = AtomicUsize::new(0);
        let m = paired(&base, &[same], &gauntlet, &rules, 120, 5, &done);
        assert_eq!(m[0].gain, 0.0);
        assert_eq!(m[0].stderr, 0.0);
        assert_eq!(m[0].games, 120);
        assert_eq!(done.load(Ordering::Relaxed), 240);
    }

    #[test]
    fn swapping_strong_cards_for_a_vanilla_wall_hurts() {
        let (db, coll) = setup();
        let rules = Rules::default();
        let gauntlet = opponents(&db, &coll, &rules);
        let base = Prepared::new(&titan_deck(), &db);
        let titan = base.card_index("Titan").unwrap();
        let wall = compile(db.get("Wall").unwrap());
        let walls = Variant {
            swaps: (0..12).map(|_| (titan, wall.clone())).collect(),
        };
        let built = base.with_swaps(&walls.swaps);
        assert_eq!(built.copies(titan), 0);
        assert_eq!(built.library_len(), base.library_len());
        let m = paired(
            &base,
            &[walls],
            &gauntlet,
            &rules,
            300,
            11,
            &AtomicUsize::new(0),
        );
        assert!(
            m[0].gain < -0.15 && m[0].z() < -2.0,
            "twelve 6/6s for two mana swapped for 0/1s: {:?}",
            m[0]
        );
    }

    #[test]
    fn results_dont_depend_on_thread_count() {
        let (db, coll) = setup();
        let rules = Rules::default();
        let gauntlet = opponents(&db, &coll, &rules);
        let base = Prepared::new(&titan_deck(), &db);
        let titan = base.card_index("Titan").unwrap();
        let goon = base.card_index("Goon 0").unwrap();
        let v1 = base.with_swaps(&[(titan, compile(db.get("Wall").unwrap()))]);
        let v2 = base.with_swaps(&[(goon, compile(db.get("Goon 2").unwrap()))]);
        let decks = [&base, &v1, &v2];
        let run = |threads| {
            play_all_threads(
                &decks,
                &gauntlet,
                &rules,
                90,
                3,
                &AtomicUsize::new(0),
                &AtomicBool::new(false),
                threads,
            )
        };
        let (one, three) = (run(1), run(3));
        assert!(one == three);
        assert_eq!(one.decks(), 3);
        assert!(one.win_rate(0) > 0.0);
    }

    #[test]
    fn small_exploration_gives_a_consistent_report() {
        let (db, coll) = setup();
        let rules = Rules::default();
        let gauntlet = opponents(&db, &coll, &rules);
        let boss = db.get("Boss").unwrap().clone();
        let effort = Effort {
            name: "test",
            screen: 60,
            additions: 60,
            bench: 3,
            cut_slots: 2,
            confirm: 150,
            colors: 60,
            max_swaps: 3,
        };
        // An empty deck: the lab tests what auto-build makes.
        let mut empty = Deck::new("empty");
        empty.commander = Some("Boss".into());
        let progress = Progress::new();
        let explore =
            |p: &Progress| explore_with(&boss, &empty, &db, &coll, &rules, effort, &gauntlet, p);
        let r = explore(&progress).expect("not cancelled");
        assert!(r.base_auto);
        assert_eq!(r.base.main_count(), r.base.main_target(&rules));
        let spells = resolved(&r.base, &db)
            .iter()
            .filter(|(c, _)| !c.is_land())
            .count();
        assert_eq!(r.worth.len(), spells);
        assert!(
            r.worth
                .windows(2)
                .all(|w| w[0].worth.gain >= w[1].worth.gain)
        );
        assert!(r.worth.iter().all(|w| w.worth.games == 60));
        assert!(!r.additions.is_empty());
        assert!(
            r.additions
                .windows(2)
                .all(|w| w[0].gain.gain >= w[1].gain.gain)
        );
        assert!(r.base_win > 0.0 && r.base_win < 1.0);
        assert_eq!(r.lands.len(), 2);
        // Black-red, or with green too.
        assert_eq!(r.colors.len(), 2);
        assert_eq!(r.colors.iter().filter(|c| c.current).count(), 1);
        if let Some(b) = &r.best {
            assert!(b.gain.z() > 2.0);
            let swaps: Vec<Swap> = b.swaps.iter().map(|s| s.0.clone()).collect();
            assert_eq!(b.deck, apply_swaps(&r.base, &swaps, &db));
            assert_eq!(b.deck.main_count(), r.base.main_count());
        }
        assert_eq!(
            progress.done.load(Ordering::Relaxed),
            progress.total.load(Ordering::Relaxed)
        );
        assert!(r.text().contains("BEST COMBINATION"));
        // Same seeds, same report (the first line has the time taken).
        let again = explore(&Progress::new()).unwrap();
        let body = |r: &LabReport| {
            r.text()
                .lines()
                .skip(1)
                .map(String::from)
                .collect::<Vec<_>>()
        };
        assert_eq!(body(&again), body(&r));
        // Stopping returns nothing.
        let stopped = Progress::new();
        stopped.cancel();
        assert!(explore(&stopped).is_none());
    }
}
