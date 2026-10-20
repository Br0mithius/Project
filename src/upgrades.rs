//! Upgrade planner: cards you don't have yet that would make your current deck better, and
//! how you could get them - claiming from the Public Pool now (VP cost, rarity floor), or
//! opening them in the league packs still to come (with the odds).

use crate::analysis::{score_card, weakest};
use crate::card::Card;
use crate::db::{CardDb, Collection, Rules};
use crate::deck::Deck;
use crate::league::claim_cost;
use crate::packs::{chance_per_pack, packs_to_come, season_packs, spread_odds};

/// Players at your draft table (§2: drafts happen within your seeded 4-player table).
pub const TABLE_PLAYERS: u32 = 4;

#[derive(Debug, Clone)]
pub enum Source {
    /// In the Public Pool right now.
    Pool {
        cost: u32,
        /// What the §4 rarity floor asks you to trade in that same round.
        floor: Option<&'static str>,
    },
    /// In packs still to be handed out.
    Packs {
        set_name: &'static str,
        /// Packs of that set you still get (0 = only other players open it).
        packs: u32,
        /// First session the card could be in your deck.
        usable_from: u32,
        /// Chance you open at least one.
        you: f32,
        /// Chance it's opened at your 4-player draft table (so it's in your draft).
        table: f32,
        /// Chance anyone in the league opens one (it could then reach the pool).
        league: f32,
    },
}

#[derive(Debug, Clone)]
pub struct Upgrade {
    pub card: Card,
    /// How much better it scores than the weakest card in the deck.
    pub gain: f32,
    pub replaces: String,
    pub reasons: Vec<String>,
    pub source: Source,
}

fn floor_for(card: &Card) -> Option<&'static str> {
    match card.rarity.to_lowercase().as_str() {
        "mythic" => Some("1 mythic or 2 rares"),
        "rare" => Some("1 rare"),
        _ => None,
    }
}

/// The Public Pool list saved from Mana Mixer (`data/public_pool.txt`).
pub fn load_public_pool(data_dir: &std::path::Path) -> Collection {
    let path = data_dir.join("public_pool.txt");
    Collection::load(&path).unwrap_or_else(|_| Collection::new(path, Default::default()))
}

/// Saves a pasted Public Pool list (any format the pool import reads).
pub fn save_public_pool(data_dir: &std::path::Path, text: &str) -> Result<usize, String> {
    let lines = crate::league::parse_import(text);
    let mut out = format!(
        "# The league's Public Pool, copied from Mana Mixer on {}.\n",
        crate::ratings::today()
    );
    for l in &lines {
        out += &format!("{} {}\n", l.count, l.name);
    }
    std::fs::write(data_dir.join("public_pool.txt"), out).map_err(|e| e.to_string())?;
    Ok(lines.len())
}

/// Upgrades for `deck`, best first: Public Pool claims (cards in `public_pool`) and cards
/// from packs handed out from session `next_session` on.
pub fn upgrades(
    deck: &Deck,
    db: &CardDb,
    coll: &Collection,
    rules: &Rules,
    public_pool: &Collection,
    next_session: u32,
) -> Vec<Upgrade> {
    let Some(identity) = deck.identity(db) else {
        return Vec::new();
    };
    let weak = weakest(deck, db, rules);
    let Some(worst) = weak.first() else {
        return Vec::new();
    };
    // Judge each candidate as if it took the weakest card's slot.
    let mut without = deck.clone();
    without.remove(&worst.card.name, 1);
    // For each set still to be opened: your packs, each other player's packs, and the
    // first session its cards could be in a deck.
    let roster = &rules.roster;
    let mut sets: Vec<(String, u32, Vec<u32>, u32)> = Vec::new();
    for player in std::iter::once(&roster.me).chain(&roster.others) {
        for d in season_packs(db, player) {
            if d.session < next_session || d.set.is_empty() || sets.iter().any(|s| s.0 == d.set) {
                continue;
            }
            let mine = packs_to_come(db, &roster.me, &d.set, next_session);
            let others: Vec<u32> = roster
                .others
                .iter()
                .map(|o| packs_to_come(db, o, &d.set, next_session))
                .collect();
            sets.push((d.set.clone(), mine, others, d.usable_from()));
        }
    }
    let mut out = Vec::new();
    for card in db.cards.iter().filter(|c| {
        !c.is_land()
            && !c.is_basic()
            && c.identity().is_subset_of(identity)
            && c.name != worst.card.name
    }) {
        if deck.commanders().iter().any(|c| **c == card.name) {
            continue;
        }
        // Spare copies you already own show up as normal suggestions instead.
        if coll.owned(&card.name) > deck.used(&card.name) {
            continue;
        }
        if rules.singleton && deck.used(&card.name) > 0 {
            continue;
        }
        let in_pool = public_pool.owned(&card.name) > 0;
        let coming = sets.iter().find(|s| s.0 == card.set);
        if !in_pool && coming.is_none() {
            continue;
        }
        let (score, mut reasons) = score_card(card, &without, db, rules);
        if deck.count_of(&card.name) > 0 {
            reasons.insert(0, "another copy".into());
        }
        let gain = score - worst.score;
        if gain < 0.25 {
            continue;
        }
        let mut push = |source| {
            out.push(Upgrade {
                card: card.clone(),
                gain,
                replaces: worst.card.name.clone(),
                reasons: reasons.clone(),
                source,
            })
        };
        if in_pool {
            push(Source::Pool {
                cost: claim_cost(card),
                floor: floor_for(card),
            });
        }
        if let Some((set, mine, others, usable_from)) = coming {
            let p = chance_per_pack(card, db);
            let (you, table, league) = spread_odds(p, *mine, others, TABLE_PLAYERS as usize);
            push(Source::Packs {
                set_name: crate::packs::set_name(set),
                packs: *mine,
                usable_from: *usable_from,
                you,
                table,
                league,
            });
        }
    }
    out.sort_by(|a, b| b.gain.partial_cmp(&a.gain).unwrap());
    out
}

/// Spare cards to trade in so you can afford claiming `u` this round: at least its cost
/// (and the 10 VP weekly minimum), meeting its rarity floor.
pub fn trade_ins_for<'a>(
    u: &Upgrade,
    deck: &Deck,
    db: &'a CardDb,
    coll: &Collection,
    rules: &Rules,
) -> Vec<crate::league::TradeIn<'a>> {
    let Source::Pool { cost, .. } = u.source else {
        return Vec::new();
    };
    let need = match u.card.rarity.to_lowercase().as_str() {
        "mythic" => Some("mythic"),
        "rare" => Some("rare"),
        _ => None,
    };
    crate::league::plan_trade_in(deck, db, coll, rules, cost.max(10), need)
}

/// One line describing how to get an upgrade.
pub fn describe(u: &Upgrade) -> String {
    match &u.source {
        Source::Pool { cost, floor } => format!(
            "Public Pool: claim for {} VP{}",
            cost,
            floor
                .map(|f| format!(" (trade in {} that round)", f))
                .unwrap_or_default()
        ),
        Source::Packs {
            set_name,
            packs,
            usable_from,
            you,
            table,
            league,
        } => format!(
            "{} ({}, usable from Session {}): you open it {:.0}% · your table {:.0}% · anyone in the league {:.0}%",
            set_name,
            if *packs == 0 {
                "not in your packs - draft it from a table-mate".to_string()
            } else {
                format!("{} of your packs", packs)
            },
            usable_from,
            you * 100.0,
            table * 100.0,
            league * 100.0
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn finds_pack_and_pool_upgrades() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
        let db = CardDb::load(&dir.join("cards.json")).unwrap();
        let mut counts = BTreeMap::new();
        for c in db
            .cards
            .iter()
            .filter(|c| c.set == "MSH" && c.identity().count() <= 1)
            .take(150)
        {
            counts.insert(c.name.clone(), 1);
        }
        let cmd = db
            .cards
            .iter()
            .find(|c| c.can_be_commander() && c.identity().count() == 2 && c.set == "MSH")
            .unwrap();
        counts.insert(cmd.name.clone(), 1);
        let coll = Collection::new(dir.join("none.txt"), counts);
        let rules = Rules::default();
        let (deck, _) = crate::analysis::autobuild(cmd, &db, &coll, &rules, None, None);
        let pool_card = db
            .cards
            .iter()
            .find(|c| {
                c.set == "FDN"
                    && c.rarity == "rare"
                    && c.identity().is_subset_of(cmd.identity())
                    && !c.is_land()
            })
            .unwrap();
        let mut pool = BTreeMap::new();
        pool.insert(pool_card.name.clone(), 1);
        let public = Collection::new(dir.join("none.txt"), pool);
        let ups = upgrades(&deck, &db, &coll, &rules, &public, 2);
        assert!(ups.iter().any(|u| matches!(u.source, Source::Packs { .. })));
        for u in &ups {
            if let Source::Packs {
                you, table, league, ..
            } = u.source
            {
                assert!(you <= table && table <= league && league <= 1.0);
            }
            assert!(u.gain >= 0.25);
        }
    }
}
