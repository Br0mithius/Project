//! What's in a booster, which packs the league hands out when, and the odds of a given card
//! turning up.
//!
//! Packs are Play Boosters: 6 commons + 1 more common slot, 3 uncommons, 1 rare or mythic
//! (mythic about 1 pack in 7), 1 wildcard of any rarity and 1 traditional foil of any rarity.
//! The wildcard and foil rates below are approximations from published odds for recent
//! sets; each set's exact numbers differ slightly.

use crate::analysis::Rng;
use crate::card::Card;
use crate::db::{CardDb, Collection};

/// Rarity order used throughout: common, uncommon, rare, mythic.
pub const RARITIES: [&str; 4] = ["common", "uncommon", "rare", "mythic"];

/// Chance the wildcard slot is each rarity (C, U, R, M).
const WILDCARD: [f32; 4] = [0.504, 0.30, 0.173, 0.023];
/// Chance the foil slot is each rarity (C, U, R, M).
const FOIL: [f32; 4] = [0.60, 0.32, 0.065, 0.015];
const MYTHIC_IN_RARE_SLOT: f32 = 1.0 / 7.0;

/// Expected cards of each rarity in one pack.
pub fn per_pack() -> [f32; 4] {
    [
        7.0 + WILDCARD[0] + FOIL[0],
        3.0 + WILDCARD[1] + FOIL[1],
        (1.0 - MYTHIC_IN_RARE_SLOT) + WILDCARD[2] + FOIL[2],
        MYTHIC_IN_RARE_SLOT + WILDCARD[3] + FOIL[3],
    ]
}

pub fn rarity_index(rarity: &str) -> Option<usize> {
    RARITIES.iter().position(|r| r.eq_ignore_ascii_case(rarity))
}

/// Cards a pack of `set` can contain, grouped by rarity (basics and league extras left out).
pub fn set_contents<'a>(db: &'a CardDb, set: &str) -> [Vec<&'a Card>; 4] {
    let mut out: [Vec<&Card>; 4] = Default::default();
    for c in db.cards.iter().filter(|c| c.set == set && !c.is_basic()) {
        if let Some(r) = rarity_index(&c.rarity) {
            out[r].push(c);
        }
    }
    out
}

/// Chance that one pack of the card's set holds at least one copy of it.
pub fn chance_per_pack(card: &Card, db: &CardDb) -> f32 {
    let Some(r) = rarity_index(&card.rarity) else {
        return 0.0;
    };
    let n = db
        .cards
        .iter()
        .filter(|c| {
            c.set == card.set && !c.is_basic() && c.rarity.eq_ignore_ascii_case(&card.rarity)
        })
        .count();
    if n == 0 {
        return 0.0;
    }
    // Copies per pack follow (very nearly) a Poisson distribution.
    1.0 - (-per_pack()[r] / n as f32).exp()
}

/// Chance a card turns up at least once in `packs` packs.
pub fn chance_in(packs: u32, per_pack: f32) -> f32 {
    1.0 - (1.0 - per_pack).powi(packs as i32)
}

/// Opens one simulated pack of a set.
pub fn open_pack<'a>(contents: &[Vec<&'a Card>; 4], rng: &mut Rng) -> Vec<&'a Card> {
    let mut pack = Vec::new();
    let pick = |r: usize, rng: &mut Rng, pack: &mut Vec<&'a Card>| {
        // Fall back to the next rarity down if a set has none (e.g. no mythics).
        let mut r = r;
        while contents[r].is_empty() && r > 0 {
            r -= 1;
        }
        if !contents[r].is_empty() {
            pack.push(contents[r][(rng.next() % contents[r].len() as u64) as usize]);
        }
    };
    let roll = |rng: &mut Rng| (rng.next() % 1_000_000) as f32 / 1_000_000.0;
    let from = |odds: &[f32; 4], x: f32| {
        let mut acc = 0.0;
        for (i, p) in odds.iter().enumerate() {
            acc += p;
            if x < acc {
                return i;
            }
        }
        0
    };
    for _ in 0..7 {
        pick(0, rng, &mut pack);
    }
    for _ in 0..3 {
        pick(1, rng, &mut pack);
    }
    let rare = if roll(rng) < MYTHIC_IN_RARE_SLOT {
        3
    } else {
        2
    };
    pick(rare, rng, &mut pack);
    let w = from(&WILDCARD, roll(rng));
    pick(w, rng, &mut pack);
    let f = from(&FOIL, roll(rng));
    pick(f, rng, &mut pack);
    pack
}

/// One delivery of league packs: `count` packs of `set` handed out at `session`, first
/// usable in decks the session after (the kickoff's 9 are used the same day).
#[derive(Debug, Clone)]
pub struct Delivery {
    pub session: u32,
    pub set: String,
    pub set_name: &'static str,
    pub count: u32,
}

impl Delivery {
    pub fn usable_from(&self) -> u32 {
        if self.session == 1 {
            1
        } else {
            self.session + 1
        }
    }
}

/// Every pack each player receives over the season (§2), for a starter set of `starter`.
/// Nauctis cards are only known once its set is in the card database.
pub fn season_packs(db: &CardDb, starter: &str) -> Vec<Delivery> {
    let starter_name = if starter == "HOB" {
        "The Hobbit"
    } else {
        "Marvel Super Heroes"
    };
    let mut out = vec![Delivery {
        session: 1,
        set: starter.to_string(),
        set_name: starter_name,
        count: 9,
    }];
    let mut push = |session, set: &str, name, count| {
        out.push(Delivery {
            session,
            set: set.to_string(),
            set_name: name,
            count,
        })
    };
    for s in 2..=4 {
        push(s, starter, starter_name, 1);
    }
    push(5, "FRA", "Reality Fracture", 3);
    for s in 6..=8 {
        push(s, "FRA", "Reality Fracture", 1);
    }
    let nauctis = nauctis_code(db).unwrap_or_default();
    push(9, &nauctis, "Nauctis: The Sunken Realms", 3);
    for s in 10..=12 {
        push(s, &nauctis, "Nauctis: The Sunken Realms", 1);
    }
    out
}

/// The Nauctis set code, once its cards have been added (Settings > Update card database).
pub fn nauctis_code(db: &CardDb) -> Option<String> {
    const KNOWN: [&str; 6] = ["MSH", "HOB", "FRA", "FDN", "LEAGUE", "CMR"];
    db.cards
        .iter()
        .map(|c| c.set.as_str())
        .find(|s| !s.is_empty() && !KNOWN.contains(s))
        .map(String::from)
}

/// Which starter set a pool came from: whichever of MSH / HOB it holds more of.
pub fn starter_set(db: &CardDb, coll: &Collection) -> String {
    let count = |set: &str| -> u32 {
        coll.counts
            .iter()
            .filter(|(n, _)| db.get(n).is_some_and(|c| c.set == set))
            .map(|(_, k)| k)
            .sum()
    };
    if count("HOB") > count("MSH") {
        "HOB".into()
    } else {
        "MSH".into()
    }
}

/// Packs (set, count) a pool holds by the time decks are built for `session`.
pub fn packs_by_session(db: &CardDb, starter: &str, session: u32) -> Vec<(String, u32)> {
    let mut out: Vec<(String, u32)> = Vec::new();
    for d in season_packs(db, starter) {
        if d.set.is_empty() || d.usable_from() > session {
            continue;
        }
        match out.iter_mut().find(|(s, _)| *s == d.set) {
            Some(e) => e.1 += d.count,
            None => out.push((d.set.clone(), d.count)),
        }
    }
    out
}

/// A simulated league pool: the packs a player would have for `session`.
pub fn simulated_pool(db: &CardDb, starter: &str, session: u32, rng: &mut Rng) -> Collection {
    let mut counts = std::collections::BTreeMap::new();
    for (set, n) in packs_by_session(db, starter, session) {
        let contents = set_contents(db, &set);
        for _ in 0..n {
            for c in open_pack(&contents, rng) {
                *counts.entry(c.name.clone()).or_insert(0) += 1;
            }
        }
    }
    Collection::new(std::path::PathBuf::from("simulated-pool.txt"), counts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_odds_add_up() {
        let e = per_pack();
        // 13 cards of the main set per pack (the 14th is a land).
        assert!((e.iter().sum::<f32>() - 13.0).abs() < 0.01);
        assert!((chance_in(1, 0.1) - 0.1).abs() < 1e-6);
        assert!((chance_in(2, 0.5) - 0.75).abs() < 1e-6);
    }
}
