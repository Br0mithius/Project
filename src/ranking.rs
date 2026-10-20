//! Commander ranking: each commander (and each mono-colour commander paired with The
//! Prismatic Piper) is judged by the actual deck auto-build makes from your pool, plus the
//! commander's own strength.
//!
//! Each option is graded on seven parts:
//! * **Cards**       average strength of the deck's spells (17Lands win rates when
//!   downloaded, roles, synergy with the commander, 4-player adjustments)
//! * **Curve**       how close the mana curve is to the commander's plan (aggro decks want
//!   a lower curve, big-mana decks a higher one - see `analysis::plan_for`)
//! * **Creatures**   creature count vs. the plan's range
//! * **Interaction** removal (board wipes count extra) and card draw vs. the plan
//! * **Mana**        simulated opening hands: keepable hands, commander cast on time
//! * **Commander**   the commander card itself: 17Lands, Playgroup.gg win rate in real
//!   4-player games, and its abilities (repeatable triggers, mana sinks, evasion, cost)
//! * **Playtest**    win rate in simulated 4-player games against decks built from
//!   simulated league pools (`playtest`)
//!
//! Parts come in different units, so before weighting each one is put on the same scale:
//! how far above or below your typical option it is (median, scaled by the median absolute
//! deviation, with a minimum spread so tiny differences aren't blown up). The weights are then simply how much
//! each part matters relative to the others, and the total runs 0-100 with 50 = the middle
//! of your options.

use crate::analysis::{
    self, Archetype, autobuild, best_colors, card_power, deck_plan, simulate, stats, weakest,
};
use crate::card::{COLORS, Card, ColorSet, Role};
use crate::db::{CardDb, Collection, Rules};
use crate::deck::{Deck, PIPER};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Weights {
    pub cards: f32,
    pub curve: f32,
    pub creatures: f32,
    pub interaction: f32,
    pub mana: f32,
    pub commander: f32,
    pub playtest: f32,
}

impl Default for Weights {
    fn default() -> Self {
        Weights {
            cards: 1.0,
            curve: 1.0,
            creatures: 1.0,
            interaction: 1.0,
            mana: 1.0,
            commander: 1.0,
            playtest: 1.0,
        }
    }
}

impl Weights {
    pub fn values(&self) -> [f32; 7] {
        [
            self.cards,
            self.curve,
            self.creatures,
            self.interaction,
            self.mana,
            self.commander,
            self.playtest,
        ]
    }
}

#[derive(Debug, Clone, Default)]
pub struct Parts {
    pub cards: f32,
    pub curve: f32,
    pub creatures: f32,
    pub interaction: f32,
    pub mana: f32,
    pub commander: f32,
    /// Win rate in simulated 4-player games (0.25 = as good as the test decks).
    pub playtest: f32,
}

/// The smallest difference in each part (in its own units) that's worth caring about.
/// Used as the minimum spread when normalising, so near-identical options stay close.
const MIN_SPREAD: [f32; 7] = [0.3, 0.25, 0.3, 0.3, 0.25, 0.4, 0.04];

impl Parts {
    pub const LABELS: [&'static str; 7] = [
        "Cards",
        "Curve",
        "Creatures",
        "Interaction",
        "Mana",
        "Commander",
        "Playtest",
    ];

    pub fn values(&self) -> [f32; 7] {
        [
            self.cards,
            self.curve,
            self.creatures,
            self.interaction,
            self.mana,
            self.commander,
            self.playtest,
        ]
    }
}

#[derive(Debug, Clone)]
pub struct Evaluation {
    pub commander: String,
    /// Colour The Prismatic Piper adds, if this option uses it.
    pub piper: Option<char>,
    pub identity: ColorSet,
    pub plays: ColorSet,
    /// Raw part values, in their own units.
    pub parts: Parts,
    /// Parts on a common scale: spreads above/below your middle option (-2.5..2.5).
    pub scaled: [f32; 7],
    /// 0-100; 50 = the middle of the options ranked together.
    pub total: f32,
    pub archetype: Archetype,
    pub creatures: u32,
    pub removal: u32,
    pub draw: u32,
    pub keepable: f32,
    pub on_curve: f32,
    /// Spell slots the pool couldn't fill (padded with basic lands).
    pub shortfall: u32,
    pub notes: Vec<String>,
    pub deck: Deck,
}

impl Evaluation {
    pub fn label(&self) -> String {
        match self.piper {
            Some(c) => format!("{} + Piper ({})", self.commander, c),
            None => self.commander.clone(),
        }
    }
}

/// How strong the commander card is on its own (roughly -1 .. +3.5).
pub fn commander_power(cmd: &Card, db: &CardDb) -> (f32, Vec<String>) {
    let mut score = 0.0;
    let mut why = Vec::new();
    let (power, w, note) = card_power(cmd, db);
    if w > 0.0 {
        // Only the part above an average card, so commanders without data aren't punished.
        score += (power - 1.5) * 0.5;
        if let Some(n) = note {
            why.push(n);
        }
    }
    if let Some(s) = db.commander_stat(&cmd.name) {
        // Playgroup.gg: 25% is average in 4-player pods. Shrunk toward average when the
        // sample is small, and capped - it's 100-card decks, not league pools.
        let trust = s.games as f32 / (s.games as f32 + 100.0);
        let pts = ((s.win_rate - 0.25) * trust * 8.0).clamp(-0.8, 1.2);
        score += pts;
        why.push(format!(
            "{:.0}% win rate as a commander in {} Playgroup games ({:+.1})",
            s.win_rate * 100.0,
            s.games,
            pts
        ));
    }
    // Abilities, read by the parser (see `abilities`).
    let parsed = cmd.parsed();
    use crate::abilities::{Effect, When};
    let engine = parsed
        .abilities
        .iter()
        .filter(|a| !a.effects.is_empty() && a.rate() > 0.0)
        .filter(|a| !a.effects.iter().all(|e| matches!(e, Effect::AddMana(_))))
        .map(|a| a.rate())
        .sum::<f32>();
    if engine > 0.0 {
        let pts = (0.4 + 0.4 * engine).min(1.2);
        score += pts;
        why.push(format!("repeatable abilities ({:+.1})", pts));
    }
    if parsed
        .abilities
        .iter()
        .any(|a| a.when.iter().any(|w| matches!(w, When::PowerUp(_))))
    {
        score += 0.3;
        why.push("power-up (cheap to use the turn it's cast)".into());
    } else if parsed.abilities.iter().any(|a| {
        a.when
            .iter()
            .any(|w| matches!(w, When::Activated(c) if c.mana > 0))
    }) {
        score += 0.3;
        why.push("mana sink".into());
    }
    if let Some(r) = parsed.removal() {
        if r.repeatable {
            score += 0.8;
            why.push(format!("repeatable removal ({})", r.label));
        } else {
            score += 0.3;
            why.push(format!("removal when it arrives ({})", r.label));
        }
    }
    if parsed.each_opponent() {
        score += 0.3;
        why.push("hits every opponent".into());
    }
    let keywords = [
        "flying",
        "trample",
        "menace",
        "deathtouch",
        "lifelink",
        "hexproof",
        "indestructible",
        "ward",
        "vigilance",
        "first strike",
        "double strike",
        "reach",
        "haste",
    ];
    let k = keywords.iter().filter(|k| parsed.has_keyword(k)).count() as f32;
    if k > 0.0 {
        score += (k * 0.15).min(0.6);
    }
    match cmd.mana_value() {
        0..=3 => {
            score += 0.4;
            why.push("cheap to cast and recast".into());
        }
        4 => score += 0.2,
        5 => {}
        _ => {
            score -= 0.3;
            why.push("expensive to recast".into());
        }
    }
    if cmd
        .toughness
        .as_deref()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(0.0)
        >= 4.0
    {
        score += 0.2;
    }
    (score, why)
}

fn parts_for(
    deck: &Deck,
    db: &CardDb,
    rules: &Rules,
    shortfall: u32,
) -> (Parts, u32, u32, u32, f32, f32, Vec<String>) {
    let st = stats(deck, db);
    let plan = deck_plan(deck, db, rules);
    let mut notes = vec![format!(
        "plays as {} ({}): aims for {}-{} creatures, ~{:.0} removal, ~{:.0} draw",
        plan.archetype.label(),
        plan.signals.join(", "),
        plan.creatures.0,
        plan.creatures.1,
        plan.removal,
        plan.draw
    )];
    // Cards: average strength of each spell copy in the finished deck.
    let scored = weakest(deck, db, rules);
    let (sum, n) = scored.iter().fold((0.0, 0u32), |acc, s| {
        let copies = deck.count_of(&s.card.name);
        (acc.0 + s.score * copies as f32, acc.1 + copies)
    });
    let mut cards = if n > 0 { sum / n as f32 } else { 0.0 };
    if shortfall > 0 {
        cards -= 0.25 * shortfall as f32;
        notes.push(format!(
            "pool {} spells short - padded with basics",
            shortfall
        ));
    }
    // Curve: 1.0 = exactly on the plan's targets.
    let targets = plan.curve;
    let off: u32 = (0..6).map(|b| st.curve[b].abs_diff(targets[b])).sum();
    let size: u32 = targets.iter().sum::<u32>() + st.curve.iter().sum::<u32>();
    let fit = 1.0 - off as f32 / size.max(1) as f32;
    let curve = (fit - 0.75) * 4.0;
    // Creatures: inside the plan's range.
    let (lo, hi) = plan.creatures;
    let creatures = if st.creatures < lo {
        notes.push(format!("only {} creatures", st.creatures));
        -0.25 * (lo - st.creatures) as f32
    } else if st.creatures > hi + 2 {
        -0.1 * (st.creatures - hi - 2) as f32
    } else {
        0.5
    };
    // Interaction: removal (wipes count 1.5) and card draw, for 4-player games.
    let removal_n = *st.roles.get(&Role::Removal).unwrap_or(&0);
    let sweepers = *st.roles.get(&Role::Sweeper).unwrap_or(&0);
    let draw_n = *st.roles.get(&Role::Draw).unwrap_or(&0);
    let removal_eff = removal_n as f32 + 0.5 * sweepers as f32;
    let removal_target = plan.removal;
    let draw_target = plan.draw;
    let mut interaction = ((removal_eff - removal_target) * 0.25).clamp(-2.0, 0.75)
        + ((draw_n as f32 - draw_target) * 0.15).clamp(-0.75, 0.45);
    if removal_eff < removal_target - 2.0 {
        notes.push(format!("light on removal ({})", removal_n));
    }
    if sweepers > 0 {
        interaction += 0.2;
    }
    // Mana: simulated hands.
    let sim = simulate(deck, db, 2500);
    let on_curve = sim.commander_on_curve.map(|(_, p)| p).unwrap_or(0.6);
    let mana = (sim.keepable - 0.85) * 6.0 + (on_curve - 0.6) * 2.0;
    (
        Parts {
            cards,
            curve,
            creatures,
            interaction,
            mana,
            commander: 0.0,
            playtest: 0.25,
        },
        st.creatures,
        removal_n,
        draw_n,
        sim.keepable,
        on_curve,
        notes,
    )
}

/// What every option is tested against: opponent decks for the playtest and how many
/// games to play. Built once per ranking.
pub struct TestBed {
    pub gauntlet: Vec<crate::playtest::Prepared>,
    pub games: u32,
}

impl TestBed {
    /// Opponents: one deck per other league player, from simulated pools of their packs.
    pub fn new(db: &CardDb, rules: &Rules) -> TestBed {
        TestBed {
            gauntlet: crate::playtest::gauntlet(db, rules, 2026),
            games: 1000,
        }
    }

    /// No playtesting (every option gets the neutral 25%).
    pub fn none() -> TestBed {
        TestBed {
            gauntlet: Vec::new(),
            games: 0,
        }
    }
}

/// Builds and grades one option (its total is filled in by `finalize`).
pub fn evaluate(
    cmd: &Card,
    piper: Option<char>,
    db: &CardDb,
    coll: &Collection,
    rules: &Rules,
    bed: &TestBed,
) -> Evaluation {
    let plays = match piper {
        Some(c) => cmd.identity().union(ColorSet::from_char(c)),
        None => best_colors(cmd, db, coll, rules),
    };
    let (deck, shortfall) = autobuild(cmd, db, coll, rules, Some(plays), piper);
    let (mut parts, creatures, removal, draw, keepable, on_curve, mut notes) =
        parts_for(&deck, db, rules, shortfall);
    let (cmd_pts, mut cmd_why) = commander_power(cmd, db);
    parts.commander = cmd_pts;
    if piper.is_some() {
        // The Piper is a vanilla 5-mana 3/3 taking a deck slot.
        parts.commander -= 0.3;
        cmd_why.push("The Prismatic Piper takes a slot (-0.3)".into());
    }
    notes.append(&mut cmd_why);
    if bed.games > 0 && !bed.gauntlet.is_empty() {
        // Same seed for every option, so they all face the same opponents and shuffles.
        parts.playtest = crate::playtest::win_rate(&deck, &bed.gauntlet, db, rules, bed.games, 7);
        notes.push(format!(
            "won {:.0}% of {} simulated 4-player games vs. decks from simulated league pools (25% = even)",
            parts.playtest * 100.0,
            bed.games
        ));
    }
    let archetype = deck_plan(&deck, db, rules).archetype;
    Evaluation {
        commander: cmd.name.clone(),
        piper,
        identity: cmd.identity(),
        plays,
        parts,
        scaled: [0.0; 7],
        total: 50.0,
        archetype,
        creatures,
        removal,
        draw,
        keepable,
        on_curve,
        shortfall,
        notes,
        deck,
    }
}

/// Puts every part on a common scale across `evals`, applies the weights, and sorts best
/// first. Cheap - call it again whenever the weights change.
pub fn finalize(evals: &mut [Evaluation], weights: &Weights) {
    if evals.is_empty() {
        return;
    }
    for k in 0..7 {
        let vals: Vec<f32> = evals.iter().map(|e| e.parts.values()[k]).collect();
        // Median and median absolute deviation: one freak option (say, a colourless
        // commander with no deck) can't squash everyone else together.
        let mid = median(&vals);
        let mad = median(&vals.iter().map(|v| (v - mid).abs()).collect::<Vec<_>>());
        let spread = (1.4826 * mad).max(MIN_SPREAD[k]);
        for (e, v) in evals.iter_mut().zip(&vals) {
            e.scaled[k] = ((v - mid) / spread).clamp(-2.5, 2.5);
        }
    }
    let w = weights.values();
    let wsum = w.iter().sum::<f32>().max(1e-6);
    for e in evals.iter_mut() {
        let z: f32 = e.scaled.iter().zip(&w).map(|(z, w)| z * w).sum::<f32>() / wsum;
        e.total = (50.0 + 20.0 * z).clamp(0.0, 100.0);
    }
    evals.sort_by(|a, b| b.total.partial_cmp(&a.total).unwrap());
}

/// Every option worth building: each owned commander, and each mono-colour commander with
/// The Prismatic Piper in its two most promising extra colours.
pub fn candidates(db: &CardDb, coll: &Collection, rules: &Rules) -> Vec<(String, Option<char>)> {
    let mut out = Vec::new();
    for cmd in db
        .cards
        .iter()
        .filter(|c| c.can_be_commander() && c.name != PIPER && coll.owned(&c.name) > 0)
    {
        out.push((cmd.name.clone(), None));
        if cmd.identity().count() == 1 {
            // Quick estimate of each extra colour, then fully build the best two.
            let mut colours: Vec<(f32, char)> = (0..5)
                .filter(|&i| !cmd.identity().contains(i))
                .map(|i| {
                    let c = COLORS[i];
                    let q = analysis::pool_quality_for(
                        cmd,
                        cmd.identity().union(ColorSet::from_char(c)),
                        db,
                        coll,
                        rules,
                    );
                    (q, c)
                })
                .collect();
            colours.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
            for (_, c) in colours.into_iter().take(2) {
                out.push((cmd.name.clone(), Some(c)));
            }
        }
    }
    out
}

fn median(vals: &[f32]) -> f32 {
    let mut v = vals.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    match n {
        0 => 0.0,
        _ if n % 2 == 1 => v[n / 2],
        _ => (v[n / 2 - 1] + v[n / 2]) / 2.0,
    }
}

/// Ranks every option. `done` counts finished builds (for a progress bar).
pub fn rank_all(
    db: &CardDb,
    coll: &Collection,
    rules: &Rules,
    weights: &Weights,
    done: &AtomicUsize,
) -> Vec<Evaluation> {
    let options = candidates(db, coll, rules);
    if options.is_empty() {
        return Vec::new();
    }
    let bed = TestBed::new(db, rules);
    // Options are independent, so build and playtest them on every CPU core.
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .clamp(1, options.len());
    let next = AtomicUsize::new(0);
    let results = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some((name, piper)) = options.get(i) else {
                        break;
                    };
                    if let Some(cmd) = db.get(name) {
                        let e = evaluate(cmd, *piper, db, coll, rules, &bed);
                        results.lock().unwrap().push((i, e));
                    }
                    done.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    // Keep a fixed order before sorting, so ties always land the same way.
    results.sort_by_key(|(i, _)| *i);
    let mut out: Vec<Evaluation> = results.into_iter().map(|(_, e)| e).collect();
    finalize(&mut out, weights);
    out
}

/// The rules decks are built under once singleton starts (Sessions 6-9), for previewing
/// how each commander holds up. `None` if singleton already applies.
pub fn singleton_rules(rules: &Rules) -> Option<Rules> {
    if rules.singleton {
        return None;
    }
    let mut r = rules.clone();
    r.singleton = true;
    Some(r)
}

/// The ranking under the current rules, plus a preview under singleton rules while
/// singleton hasn't started yet.
#[derive(Clone)]
pub struct Rankings {
    pub now: Vec<Evaluation>,
    pub singleton: Option<Vec<Evaluation>>,
}

impl Rankings {
    /// Where a commander's best option places in the singleton preview (1-based) and its total.
    pub fn singleton_place(&self, commander: &str) -> Option<(usize, f32)> {
        let groups = best_per_commander(self.singleton.as_ref()?);
        groups
            .iter()
            .position(|g| g[0].commander == commander)
            .map(|i| (i + 1, groups[i][0].total))
    }

    pub fn reweigh(&mut self, weights: &Weights) {
        finalize(&mut self.now, weights);
        if let Some(s) = &mut self.singleton {
            finalize(s, weights);
        }
    }
}

/// Ranks under the current rules and, before Session 6, under singleton rules too.
/// `done` counts finished builds across both.
pub fn rank_with_preview(
    db: &CardDb,
    coll: &Collection,
    rules: &Rules,
    weights: &Weights,
    done: &AtomicUsize,
) -> Rankings {
    let now = rank_all(db, coll, rules, weights, done);
    let singleton = singleton_rules(rules).map(|r| rank_all(db, coll, &r, weights, done));
    Rankings { now, singleton }
}

/// How many builds `rank_with_preview` will make (for a progress bar).
pub fn build_count(db: &CardDb, coll: &Collection, rules: &Rules) -> usize {
    let n = candidates(db, coll, rules).len();
    n + singleton_rules(rules).map_or(0, |r| candidates(db, coll, &r).len())
}

/// Changes to any of these mean the ranking must be rebuilt.
pub fn ranking_key(db: &CardDb, coll: &Collection, rules: &Rules) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{:?}",
        coll.total(),
        coll.counts.len(),
        rules.deck_size,
        rules.singleton,
        rules.players,
        rules.session,
        db.ratings_date,
        db.commander_stats.len(),
        rules.roster
    )
}

/// Re-totals and re-sorts after the weights change (no rebuilding needed).
pub fn reweigh(evals: &mut [Evaluation], weights: &Weights) {
    finalize(evals, weights);
}

/// One row per commander: its best option first, plus the other options for display.
pub fn best_per_commander(evals: &[Evaluation]) -> Vec<Vec<&Evaluation>> {
    let mut groups: Vec<Vec<&Evaluation>> = Vec::new();
    for e in evals {
        match groups.iter_mut().find(|g| g[0].commander == e.commander) {
            Some(g) => g.push(e),
            None => groups.push(vec![e]),
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalised_totals_follow_weights() {
        let mk = |cards: f32, curve: f32| Evaluation {
            commander: format!("C{}", cards),
            piper: None,
            identity: ColorSet(1),
            plays: ColorSet(1),
            parts: Parts {
                cards,
                curve,
                playtest: 0.25,
                ..Default::default()
            },
            scaled: [0.0; 7],
            total: 0.0,
            archetype: Archetype::Midrange,
            creatures: 0,
            removal: 0,
            draw: 0,
            keepable: 0.0,
            on_curve: 0.0,
            shortfall: 0,
            notes: Vec::new(),
            deck: Deck::new("x"),
        };
        // A is better on cards, B on curve.
        let mut evals = vec![mk(3.0, -1.0), mk(1.0, 1.0)];
        let mut w = Weights::default();
        finalize(&mut evals, &w);
        assert!((evals[0].total - evals[1].total).abs() < 1e-3);
        w.curve = 3.0;
        finalize(&mut evals, &w);
        assert_eq!(evals[0].parts.curve, 1.0);
        assert!(evals[0].total > 50.0 && evals[1].total < 50.0);
    }
}
