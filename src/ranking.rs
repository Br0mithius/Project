//! Commander ranking: each commander (and each mono-colour commander paired with The
//! Prismatic Piper) is judged by the actual deck auto-build makes from your pool, plus the
//! commander's own strength.
//!
//! Score = sum of weighted parts, each roughly "points above/below a typical deck":
//! * **Cards**       average strength of the deck's spells (17Lands win rates when
//!   downloaded, roles, synergy with the commander, 4-player adjustments)
//! * **Curve**       how close the mana curve is to the targets
//! * **Creatures**   creature count vs. the 15-20 a 60-card deck wants
//! * **Interaction** removal (board wipes count extra) and card draw
//! * **Mana**        simulated opening hands: keepable hands, commander cast on time
//! * **Commander**   the commander card itself: 17Lands, Playgroup.gg win rate in real
//!   4-player games, and its abilities (repeatable triggers, mana sinks, evasion, cost)
//!
//! Every part has a weight you can change, so curve or creature count can matter more or less.

use crate::analysis::{
    self, autobuild, best_colors, card_power, curve_targets, simulate, stats, weakest,
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
        }
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
}

impl Parts {
    pub fn total(&self, w: &Weights) -> f32 {
        self.cards * w.cards
            + self.curve * w.curve
            + self.creatures * w.creatures
            + self.interaction * w.interaction
            + self.mana * w.mana
            + self.commander * w.commander
    }

    pub const LABELS: [&'static str; 6] = [
        "Cards",
        "Curve",
        "Creatures",
        "Interaction",
        "Mana",
        "Commander",
    ];

    pub fn values(&self) -> [f32; 6] {
        [
            self.cards,
            self.curve,
            self.creatures,
            self.interaction,
            self.mana,
            self.commander,
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
    pub parts: Parts,
    pub total: f32,
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
    let t = cmd.text_lower();
    if t.contains("whenever") || t.contains("at the beginning of") {
        score += 0.6;
        why.push("repeatable trigger".into());
    }
    if t.contains("power-up") || t.contains(": transform") {
        score += 0.3;
        why.push("mana sink (power-up / transform)".into());
    }
    let activated = t
        .lines()
        .any(|l| l.contains(':') && !l.starts_with("power-up") && !l.contains('—'));
    if activated {
        score += 0.3;
        why.push("activated ability".into());
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
        "can't be blocked",
        "vigilance",
        "first strike",
        "reach",
    ];
    let k = keywords.iter().filter(|k| t.contains(*k)).count() as f32;
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
    let mut notes = Vec::new();
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
    // Curve: 1.0 = exactly on target.
    let targets = curve_targets(rules);
    let off: u32 = (0..6).map(|b| st.curve[b].abs_diff(targets[b])).sum();
    let size: u32 = targets.iter().sum::<u32>() + st.curve.iter().sum::<u32>();
    let fit = 1.0 - off as f32 / size.max(1) as f32;
    let curve = (fit - 0.75) * 4.0;
    // Creatures: 15-20 per 60 cards.
    let scale = rules.deck_size as f32 / 60.0;
    let (lo, hi) = ((15.0 * scale) as u32, (20.0 * scale) as u32);
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
    let removal_target = 7.0 * scale;
    let draw_target = 5.0 * scale;
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
        },
        st.creatures,
        removal_n,
        draw_n,
        sim.keepable,
        on_curve,
        notes,
    )
}

/// Builds and grades one option.
pub fn evaluate(
    cmd: &Card,
    piper: Option<char>,
    db: &CardDb,
    coll: &Collection,
    rules: &Rules,
    weights: &Weights,
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
    let total = parts.total(weights);
    Evaluation {
        commander: cmd.name.clone(),
        piper,
        identity: cmd.identity(),
        plays,
        parts,
        total,
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

/// Ranks every option. `done` counts finished builds (for a progress bar).
pub fn rank_all(
    db: &CardDb,
    coll: &Collection,
    rules: &Rules,
    weights: &Weights,
    done: &AtomicUsize,
) -> Vec<Evaluation> {
    let mut out: Vec<Evaluation> = Vec::new();
    for (name, piper) in candidates(db, coll, rules) {
        if let Some(cmd) = db.get(&name) {
            out.push(evaluate(cmd, piper, db, coll, rules, weights));
        }
        done.fetch_add(1, Ordering::Relaxed);
    }
    out.sort_by(|a, b| b.total.partial_cmp(&a.total).unwrap());
    out
}

/// Re-totals and re-sorts after the weights change (no rebuilding needed).
pub fn reweigh(evals: &mut [Evaluation], weights: &Weights) {
    for e in evals.iter_mut() {
        e.total = e.parts.total(weights);
    }
    evals.sort_by(|a, b| b.total.partial_cmp(&a.total).unwrap());
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
    fn weights_change_totals() {
        let p = Parts {
            cards: 5.0,
            curve: 1.0,
            creatures: -1.0,
            interaction: 0.5,
            mana: 0.2,
            commander: 1.0,
        };
        let mut w = Weights::default();
        assert!((p.total(&w) - 6.7).abs() < 1e-4);
        w.creatures = 2.0;
        assert!((p.total(&w) - 5.7).abs() < 1e-4);
    }
}
