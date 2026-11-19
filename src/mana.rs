//! The mana base: how much mana a deck counts as having, and how often it can cast what it
//! needs.
//!
//! * Land-equivalents: lands plus cards that are lands in all but name (landcycling). No
//!   builder or playtest change takes a deck past the land cap (`analysis::land_cap`, 26 of
//!   60) in land-equivalents: the simulator's mana sinks hide flooding, so it keeps rating
//!   "one more mana source" as a gain, and a landcycler let it slip a 27th land in.
//! * Castability: for each spell, the chance of having enough lands and enough sources of
//!   each colour it needs by the turn its mana value says (exact hypergeometric odds over the
//!   deck's real counts). The basics are chosen to make that as high as possible across the
//!   deck, rather than split by pip counts.

use crate::card::Card;
use crate::db::{CardDb, Rules};
use crate::deck::Deck;

/// True for cards that count as lands for the land cap: lands, and landcycling cards.
pub fn is_land_equivalent(card: &Card) -> bool {
    card.is_land() || card.oracle_text.to_lowercase().contains("landcycling")
}

/// Lands plus landcycling cards in the main deck.
pub fn land_equivalents(deck: &Deck, db: &CardDb) -> u32 {
    crate::analysis::resolved(deck, db)
        .iter()
        .filter(|(c, _)| is_land_equivalent(c))
        .map(|(_, n)| *n)
        .sum()
}

fn ln_choose(n: u32, k: u32) -> f64 {
    if k > n {
        return f64::NEG_INFINITY;
    }
    let lg = |x: u32| (1..=x).map(|i| (i as f64).ln()).sum::<f64>();
    lg(n) - lg(k) - lg(n - k)
}

/// P(at least `k` successes) drawing `n` cards from `pop` that hold `succ` successes.
pub fn at_least(pop: u32, succ: u32, n: u32, k: u32) -> f64 {
    if k == 0 {
        return 1.0;
    }
    let n = n.min(pop);
    let total = ln_choose(pop, n);
    let mut p = 0.0;
    for x in k..=n.min(succ) {
        if n - x > pop - succ {
            continue;
        }
        p += (ln_choose(succ, x) + ln_choose(pop - succ, n - x) - total).exp();
    }
    p.clamp(0.0, 1.0)
}

/// How castable the deck's spells are.
#[derive(Debug, Clone)]
pub struct Castability {
    /// Each spell (and the commander) with its chance to be castable on curve, worst first.
    pub cards: Vec<(String, f32)>,
    /// Average over the deck's spells (each copy counts).
    pub mean: f32,
}

/// Colour sources by colour (W U B R G): lands that make that colour.
fn sources(deck: &Deck, db: &CardDb) -> ([u32; 5], u32) {
    let mut src = [0u32; 5];
    let mut lands = 0;
    for (c, n) in crate::analysis::resolved(deck, db) {
        if !c.is_land() {
            continue;
        }
        lands += n;
        for i in c.produces().indices() {
            src[i] += n;
        }
    }
    (src, lands)
}

/// The chance `card` is castable on the turn its mana value says (at least turn 1), with
/// `src` colour sources and `lands` lands in a `pop`-card library.
fn card_odds(card: &Card, src: &[u32; 5], lands: u32, pop: u32) -> f32 {
    let mv = card.mana_value().max(1);
    // Cards seen by that turn: the opening seven and a draw a turn.
    let seen = 7 + mv - 1;
    let p_lands = at_least(pop, lands, seen, mv);
    let pips = card.pips();
    let mut p_colours = 1.0;
    for i in 0..5 {
        if pips[i] > 0 {
            p_colours *= at_least(pop, src[i], seen, pips[i]);
        }
    }
    // The land count and each colour constrain the same draws: the binding constraint is
    // the closest to the truth of the simple estimates (a product double-counts).
    p_lands.min(p_colours) as f32
}

/// Castability of `deck` (see the module notes).
pub fn castability(deck: &Deck, db: &CardDb) -> Castability {
    let (src, lands) = sources(deck, db);
    let pop = deck.main_count().max(1);
    let mut cards = Vec::new();
    let (mut sum, mut count) = (0.0, 0u32);
    let mut all: Vec<(&Card, u32)> = crate::analysis::resolved(deck, db)
        .into_iter()
        .filter(|(c, _)| !c.is_land())
        .collect();
    for name in deck.commanders() {
        if let Some(c) = db.get(name) {
            all.push((c, 2));
        }
    }
    for (c, n) in all {
        let p = card_odds(c, &src, lands, pop);
        cards.push((c.name.clone(), p));
        sum += p * n as f32;
        count += n;
    }
    cards.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    Castability {
        cards,
        mean: if count == 0 { 0.0 } else { sum / count as f32 },
    }
}

/// `count` basics for `deck`, chosen one at a time to raise castability the most (inside the
/// commander's colours). Ties go to the colour with the most pips.
pub fn best_basics(deck: &Deck, db: &CardDb, count: u32) -> [u32; 5] {
    let mut out = [0u32; 5];
    let Some(identity) = deck.identity(db) else {
        return out;
    };
    let colours: Vec<usize> = identity.indices().collect();
    if colours.is_empty() || count == 0 {
        return out;
    }
    let mut d = deck.clone();
    let st = crate::analysis::stats(deck, db);
    for _ in 0..count {
        let mut best: Option<(f32, u32, usize)> = None;
        for &i in &colours {
            let mut t = d.clone();
            t.add(crate::card::BASIC_FOR_COLOR[i], 1);
            let m = castability(&t, db).mean;
            let key = (m, st.pips[i]);
            if best.is_none_or(|(bm, bp, _)| {
                key.0 > bm + 1e-6 || ((key.0 - bm).abs() <= 1e-6 && key.1 > bp)
            }) {
                best = Some((m, st.pips[i], i));
            }
        }
        let (_, _, i) = best.expect("a colour");
        out[i] += 1;
        d.add(crate::card::BASIC_FOR_COLOR[i], 1);
    }
    out
}

/// Re-chooses `deck`'s basics for castability (same number of basics). Returns the
/// castability before and after.
pub fn rebalance_basics(deck: &mut Deck, db: &CardDb) -> (f32, f32) {
    let before = castability(deck, db).mean;
    let mut basics = 0;
    for name in crate::card::BASIC_FOR_COLOR {
        basics += deck.remove(name, 999);
    }
    let split = best_basics(deck, db, basics);
    for (i, n) in split.iter().enumerate() {
        if *n > 0 {
            deck.add(crate::card::BASIC_FOR_COLOR[i], *n);
        }
    }
    (before, castability(deck, db).mean)
}

/// Text report: land-equivalents against the cap, castability, and the hardest spells.
pub fn report(deck: &Deck, db: &CardDb, rules: &Rules) -> String {
    let c = castability(deck, db);
    let (src, lands) = sources(deck, db);
    let mut s = format!(
        "Lands {} · land-equivalents {} (cap {}) · castable on curve {:.0}% on average\n",
        lands,
        land_equivalents(deck, db),
        crate::analysis::land_cap(rules),
        c.mean * 100.0
    );
    s += &format!(
        "Colour sources: W {} · U {} · B {} · R {} · G {}\n",
        src[0], src[1], src[2], src[3], src[4]
    );
    s += "Hardest to cast on curve:\n";
    for (n, p) in c.cards.iter().take(8) {
        s += &format!("  {:>3.0}%  {}\n", p * 100.0, n);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hypergeometric_odds() {
        // Drawing everything: certain. Needing more than exist: impossible.
        assert!((at_least(10, 4, 10, 4) - 1.0).abs() < 1e-9);
        assert_eq!(at_least(10, 3, 10, 4), 0.0);
        // One land among 60 cards, one card drawn.
        assert!((at_least(60, 1, 1, 1) - 1.0 / 60.0).abs() < 1e-9);
        // 26 lands in 59, 9 seen, at least 3: about 78% (the familiar turn-3 land odds).
        let p = at_least(59, 26, 9, 3);
        assert!(p > 0.7 && p < 0.9, "{}", p);
    }

    #[test]
    fn basics_follow_castability() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
        let db = CardDb::load(&dir.join("cards.json")).unwrap();
        // A W/B commander with heavy black costs and one white card: black gets more basics.
        let mut d = Deck::new("t");
        d.commander = Some("Winter Soldier, Icy Assassin".into());
        // A realistic size: 34 spells (heavy black, light white), then 25 basics.
        for (n, k) in [
            ("Massacre Wurm", 12),
            ("Hero's Downfall", 12),
            ("Web Up", 3),
            ("Sol Ring", 7),
        ] {
            d.add(n, k);
        }
        let b = best_basics(&d, &db, 25);
        assert_eq!(b.iter().sum::<u32>(), 25);
        assert!(b[2] > b[0], "{:?}", b);
        // Landcyclers count toward the land cap.
        assert!(is_land_equivalent(db.get("Kree Sentinel").unwrap()));
        assert!(!is_land_equivalent(db.get("Sol Ring").unwrap()));
        // The never-use list wins: an avoided card is never available.
        let mut rules = crate::db::Rules::default();
        rules.enforce_collection = false;
        let quinjet = db.get("Dependable Quinjet").unwrap();
        assert!(
            crate::analysis::available(
                quinjet,
                &d,
                &crate::db::Collection::new(Default::default(), Default::default()),
                &rules
            ) > 0
        );
        rules.avoid.push("Dependable Quinjet".into());
        assert_eq!(
            crate::analysis::available(
                quinjet,
                &d,
                &crate::db::Collection::new(Default::default(), Default::default()),
                &rules
            ),
            0
        );
    }
}
