//! Card power measured by playtests, kept across decks.
//!
//! Each measurement is one card in one deck: the deck playing the card against the same deck
//! with a colourless vanilla N/N for {N} in its place (N = the card's mana value), on the same
//! paired games against simulated league opponents. Like `card_calibration`, the value is put
//! on one scale - win-rate points over a vanilla 3-drop - by adding what the card's own
//! stand-in is worth over the 3-mana one, so cards of different costs compare directly.
//!
//! The measurements are stored in `<data>/sim_card_values.tsv` (one row per card and deck,
//! tagged with the simulator version) and only re-measured on request. A card's power is the
//! confidence-weighted mean over every deck it was measured in, with how many decks that is
//! and how much it varies between them: a card that does well in many decks, consistently,
//! is trusted most. The deck builder uses it as the main part of a card's score.

use super::Prepared;
use super::compile::T_LAND;
use super::compile::compile;
use super::lab::play_all;
use crate::card::Card;
use crate::db::Rules;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize};

/// Bumped whenever the simulator changes how cards play: older measurements stop counting
/// (they stay in the file, marked with their version).
pub const SIM_VERSION: u32 = 1;

pub const FILE: &str = "sim_card_values.tsv";

/// One card measured in one deck.
#[derive(Clone, Debug, PartialEq)]
pub struct Measurement {
    pub card: String,
    /// Which deck it was measured in ("Thanos, the Mad Titan", "league: Bifur, Melodic Rider").
    pub deck: String,
    /// Win-rate points over a vanilla 3-drop (1.0 = one percentage point).
    pub value: f32,
    /// Its standard error, in the same points.
    pub se: f32,
    pub games: u32,
    pub sim: u32,
    pub date: String,
}

/// A card's measured power over all the decks it was measured in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Power {
    /// Confidence-weighted mean, win-rate points over a vanilla 3-drop.
    pub value: f32,
    /// Standard error of that mean.
    pub se: f32,
    /// Decks it was measured in.
    pub decks: usize,
    /// How much it varies from deck to deck (weighted standard deviation; 0 for one deck).
    pub spread: f32,
}

impl Power {
    /// Measured in three or more decks, doing about as well in each.
    pub fn consistent(&self) -> bool {
        self.decks >= 3 && self.spread <= (0.4 * self.value.abs()).max(0.8)
    }
}

#[derive(Clone, Debug, Default)]
pub struct SimValues {
    pub rows: Vec<Measurement>,
    power: BTreeMap<String, Power>,
}

impl SimValues {
    /// The stored measurements (none if the file is missing or unreadable).
    pub fn load(dir: &Path) -> SimValues {
        let text = std::fs::read_to_string(dir.join(FILE)).unwrap_or_default();
        let rows = text
            .lines()
            .filter(|l| !l.starts_with('#') && !l.starts_with("card\t") && !l.trim().is_empty())
            .filter_map(|l| {
                let f: Vec<&str> = l.split('\t').collect();
                if f.len() < 7 {
                    return None;
                }
                Some(Measurement {
                    card: f[0].to_string(),
                    deck: f[1].to_string(),
                    value: f[2].parse().ok()?,
                    se: f[3].parse().ok()?,
                    games: f[4].parse().ok()?,
                    sim: f[5].parse().ok()?,
                    date: f[6].to_string(),
                })
            })
            .collect();
        let mut v = SimValues {
            rows,
            power: BTreeMap::new(),
        };
        v.recompute();
        v
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        let mut out = String::from(
            "# Card power measured by playtests (see src/playtest/simvalues.rs): win-rate points\n\
             # over a vanilla 3-drop, one row per card and deck. Re-measure from the app.\n\
             card\tdeck\tvalue\tse\tgames\tsim\tdate\n",
        );
        for m in &self.rows {
            out += &format!(
                "{}\t{}\t{:.3}\t{:.3}\t{}\t{}\t{}\n",
                m.card, m.deck, m.value, m.se, m.games, m.sim, m.date
            );
        }
        std::fs::write(dir.join(FILE), out)
    }

    /// Adds measurements, replacing any earlier one of the same card in the same deck.
    pub fn add(&mut self, new: Vec<Measurement>) {
        for m in new {
            self.rows
                .retain(|r| !(r.card == m.card && r.deck == m.deck));
            self.rows.push(m);
        }
        self.recompute();
    }

    /// A card's measured power (current simulator version only).
    pub fn power(&self, card: &str) -> Option<Power> {
        self.power.get(card).copied()
    }

    /// Every measured card's power, best first.
    pub fn all(&self) -> Vec<(String, Power)> {
        let mut v: Vec<(String, Power)> = self.power.iter().map(|(k, p)| (k.clone(), *p)).collect();
        v.sort_by(|a, b| b.1.value.partial_cmp(&a.1.value).unwrap());
        v
    }

    /// The decks measured with the current simulator, and how many cards each.
    pub fn decks(&self) -> BTreeMap<String, usize> {
        let mut out = BTreeMap::new();
        for m in self.rows.iter().filter(|m| m.sim == SIM_VERSION) {
            *out.entry(m.deck.clone()).or_insert(0) += 1;
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.power.is_empty()
    }

    fn recompute(&mut self) {
        let mut by: BTreeMap<&str, Vec<&Measurement>> = BTreeMap::new();
        for m in self.rows.iter().filter(|m| m.sim == SIM_VERSION) {
            by.entry(&m.card).or_default().push(m);
        }
        self.power = by
            .into_iter()
            .map(|(card, ms)| (card.to_string(), combine(&ms)))
            .collect();
    }
}

/// Inverse-variance mean over decks, with the between-deck spread.
fn combine(ms: &[&Measurement]) -> Power {
    let w: Vec<f32> = ms.iter().map(|m| 1.0 / m.se.max(0.05).powi(2)).collect();
    let sw: f32 = w.iter().sum();
    let mean = ms.iter().zip(&w).map(|(m, w)| m.value * w).sum::<f32>() / sw;
    let var = ms
        .iter()
        .zip(&w)
        .map(|(m, w)| w * (m.value - mean).powi(2))
        .sum::<f32>()
        / sw;
    Power {
        value: mean,
        se: (1.0 / sw).sqrt(),
        decks: ms.len(),
        spread: if ms.len() > 1 { var.sqrt() } else { 0.0 },
    }
}

/// A colourless vanilla N/N for {N} (N = 1..7): the neutral stand-in a card is measured
/// against.
pub fn vanilla(mv: u32) -> Card {
    let mv = mv.clamp(1, 7);
    Card {
        name: format!("Vanilla {}/{} for {{{}}}", mv, mv, mv),
        mana_cost: format!("{{{}}}", mv),
        cmc: mv as f32,
        type_line: "Artifact Creature — Construct".into(),
        power: Some(mv.to_string()),
        toughness: Some(mv.to_string()),
        rarity: "common".into(),
        confidence: "verified".into(),
        ..Default::default()
    }
}

/// Measures every spell of `deck` (against a vanilla stand-in of its mana value), and each
/// of `extra` (cards not in the deck, played in place of one of its 3-ish-mana spells), on
/// `games` paired games against `opponents`. `deck_name` labels the measurements.
#[allow(clippy::too_many_arguments)]
pub fn measure(
    deck_name: &str,
    deck: &Prepared,
    extra: &[&Card],
    opponents: &[Prepared],
    rules: &Rules,
    games: u32,
    seed: u64,
    done: &AtomicUsize,
    cancel: &AtomicBool,
) -> Vec<Measurement> {
    // The deck's spells, once each (by card index), and the slot an outside card goes in.
    let mut spells: Vec<u16> = Vec::new();
    for i in 0..deck.library_len() {
        let idx = deck.library_card(i);
        let c = &deck.cards[idx as usize];
        if !c.is(T_LAND) && !spells.contains(&idx) {
            spells.push(idx);
        }
    }
    let Some(&slot) = spells
        .iter()
        .min_by_key(|&&i| (deck.cards[i as usize].mv as i32 - 3).abs())
    else {
        return Vec::new();
    };
    // Variants: 0 the deck; 1..=7 the slot as an N-mana stand-in; then each deck spell as
    // its stand-in; then each outside card in the slot.
    let mut variants: Vec<Prepared> = (1..=7u32)
        .map(|mv| deck.with_swaps(&[(slot, compile(&vanilla(mv)))]))
        .collect();
    for &i in &spells {
        let mv = deck.cards[i as usize].mv as u32;
        variants.push(deck.with_swaps(&[(i, compile(&vanilla(mv)))]));
    }
    for c in extra {
        variants.push(deck.with_swaps(&[(slot, compile(c))]));
    }
    let mut all: Vec<&Prepared> = vec![deck];
    all.extend(variants.iter());
    let o = play_all(&all, opponents, rules, games, seed, done, cancel);
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return Vec::new();
    }
    let van = |mv: u32| mv.clamp(1, 7) as usize;
    let date = crate::ratings::today();
    let mut out = Vec::new();
    for (k, &i) in spells.iter().enumerate() {
        let c = &deck.cards[i as usize];
        let mv = c.mv as u32;
        // (deck - deck with the stand-in) + (the stand-in over a vanilla 3-drop)
        let vv = o.diff(0, 8 + k);
        let shift = o.diff(van(mv), van(3));
        out.push(Measurement {
            card: c.name.clone(),
            deck: deck_name.to_string(),
            value: (vv.gain + shift.gain) * 100.0,
            se: (vv.stderr.powi(2) + shift.stderr.powi(2)).sqrt() * 100.0,
            games,
            sim: SIM_VERSION,
            date: date.clone(),
        });
    }
    for (k, c) in extra.iter().enumerate() {
        let m = o.diff(8 + spells.len() + k, van(3));
        out.push(Measurement {
            card: c.name.clone(),
            deck: deck_name.to_string(),
            value: m.gain * 100.0,
            se: m.stderr * 100.0,
            games,
            sim: SIM_VERSION,
            date: date.clone(),
        });
    }
    out
}

/// How many deck-games `measure` plays (for progress bars).
pub fn measure_size(deck: &Prepared, extra: usize, games: u32) -> usize {
    let mut seen: Vec<u16> = Vec::new();
    for i in 0..deck.library_len() {
        let idx = deck.library_card(i);
        if !deck.cards[idx as usize].is(T_LAND) && !seen.contains(&idx) {
            seen.push(idx);
        }
    }
    (1 + 7 + seen.len() + extra) * games as usize
}

/// Measures the cards of every league opponent's deck, each against the others: a card's
/// power across a variety of decks and commanders.
pub fn measure_league(
    gauntlet: &[Prepared],
    rules: &Rules,
    games: u32,
    seed: u64,
    done: &AtomicUsize,
    cancel: &AtomicBool,
) -> Vec<Measurement> {
    let mut out = Vec::new();
    for (i, d) in gauntlet.iter().enumerate() {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        let others: Vec<Prepared> = gauntlet
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, p)| p.with_swaps(&[]))
            .collect();
        let name = format!("league: {}", d.commanders_names().join(" + "));
        out.extend(measure(
            &name,
            d,
            &[],
            &others,
            rules,
            games,
            seed ^ (i as u64 * 7919),
            done,
            cancel,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(card: &str, deck: &str, value: f32, se: f32) -> Measurement {
        Measurement {
            card: card.into(),
            deck: deck.into(),
            value,
            se,
            games: 1000,
            sim: SIM_VERSION,
            date: "2026-11-05".into(),
        }
    }

    #[test]
    fn power_is_the_weighted_mean_over_decks() {
        let mut v = SimValues::default();
        v.add(vec![
            m("Sol Ring", "a", 5.0, 0.5),
            m("Sol Ring", "b", 4.0, 0.5),
            m("Sol Ring", "c", 4.5, 0.5),
            m("Truck Toss", "a", -0.3, 0.3),
        ]);
        let p = v.power("Sol Ring").unwrap();
        assert!((p.value - 4.5).abs() < 1e-4);
        assert_eq!(p.decks, 3);
        assert!(p.consistent());
        assert!(p.se < 0.5, "three decks pin it down better than one");
        assert!(!v.power("Truck Toss").unwrap().consistent(), "one deck");
        // A re-measure replaces the same card in the same deck.
        v.add(vec![m("Sol Ring", "a", 6.0, 0.5)]);
        assert_eq!(v.power("Sol Ring").unwrap().decks, 3);
        // Another simulator version doesn't count.
        let mut old = m("Ramos, Dragon Engine", "a", 3.0, 0.3);
        old.sim = SIM_VERSION + 100;
        v.add(vec![old]);
        assert!(v.power("Ramos, Dragon Engine").is_none());
    }

    #[test]
    fn values_survive_a_save_and_load() {
        let dir = std::env::temp_dir().join(format!("simvalues-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut v = SimValues::default();
        v.add(vec![
            m("Sol Ring", "a", 5.0, 0.5),
            m("Hero's Downfall", "b", 0.4, 0.3),
        ]);
        v.save(&dir).unwrap();
        let back = SimValues::load(&dir);
        assert_eq!(back.rows.len(), 2);
        assert_eq!(back.power("Sol Ring"), v.power("Sol Ring"));
        std::fs::remove_dir_all(&dir).ok();
    }
}

/// What `measure_pool` will measure: the deck, the owned spells inside the commander's colour
/// identity that aren't in it, and (with `league`) every league opponent's deck.
pub struct PoolJob {
    pub deck_name: String,
    pub deck: Prepared,
    pub extra: Vec<Card>,
    pub opponents: Vec<Prepared>,
    pub league: bool,
    pub games: u32,
}

impl PoolJob {
    pub fn new(
        deck: &crate::deck::Deck,
        db: &crate::db::CardDb,
        coll: &crate::db::Collection,
        rules: &Rules,
        games: u32,
        league: bool,
    ) -> PoolJob {
        let identity = deck.identity(db);
        let mut extra: Vec<Card> = Vec::new();
        for name in coll.counts.keys() {
            let Some(c) = db.get(name) else { continue };
            if c.is_land()
                || deck.count_of(&c.name) > 0
                || deck.commander.as_deref() == Some(c.name.as_str())
                || extra.iter().any(|e| e.name == c.name)
                || identity.is_some_and(|id| !c.identity().is_subset_of(id))
            {
                continue;
            }
            extra.push(c.clone());
        }
        PoolJob {
            deck_name: deck
                .commanders()
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(" + "),
            deck: Prepared::new(deck, db),
            extra,
            opponents: crate::ranking::TestBed::new(db, rules).gauntlet,
            league,
            games,
        }
    }

    /// Deck-games the whole job plays (for a progress bar).
    pub fn size(&self) -> usize {
        let mut n = measure_size(&self.deck, self.extra.len(), self.games);
        if self.league {
            let g = (self.games / 2).max(500);
            n += self
                .opponents
                .iter()
                .map(|d| measure_size(d, 0, g))
                .sum::<usize>();
        }
        n
    }

    pub fn run(&self, rules: &Rules, done: &AtomicUsize, cancel: &AtomicBool) -> Vec<Measurement> {
        let extra: Vec<&Card> = self.extra.iter().collect();
        let mut out = measure(
            &self.deck_name,
            &self.deck,
            &extra,
            &self.opponents,
            rules,
            self.games,
            0x5EED_CA2D,
            done,
            cancel,
        );
        if self.league && !cancel.load(std::sync::atomic::Ordering::Relaxed) {
            let g = (self.games / 2).max(500);
            out.extend(measure_league(
                &self.opponents,
                rules,
                g,
                0x1EA6,
                done,
                cancel,
            ));
        }
        out
    }
}
