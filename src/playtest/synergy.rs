//! Are some cards worth more together than apart? Paired playtests of a deck with parts of
//! a package replaced by neutral stand-ins (a colourless vanilla N/N for {N}, N = the
//! card's mana value), all on the same games (`lab::play_all`):
//!
//! - each card alone: the deck with the rest of the package out, minus the deck with all
//!   of it out;
//! - each card in the package: the full deck minus the deck with only that card out;
//! - the package: the full deck minus the deck with all of it out;
//! - the interaction: the package minus the sum of the cards alone (positive = synergy),
//!   with its standard error from the paired games.

use super::Prepared;
use super::compile::{SimCard, compile};
use super::lab::{Measure, Outcomes, play_all};
use super::simvalues::vanilla;
use crate::card::Card;
use crate::db::{CardDb, Rules};
use crate::deck::Deck;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The neutral stand-in for `card`: a vanilla N/N for {N}, N its mana value (1..7).
pub fn stand_in(card: &Card) -> SimCard {
    compile(&vanilla(card.mana_value().max(1)))
}

/// Results of a package test (win-rate fractions; 0.02 = two points).
#[derive(Debug, Clone)]
pub struct PackageReport {
    /// The package's cards, as in the deck.
    pub cards: Vec<String>,
    /// Package cards that weren't in the deck: (card, the deck card it replaced).
    pub swapped_in: Vec<(String, String)>,
    pub games: u32,
    /// Win rate of the full deck, and of the deck with the whole package out.
    pub full_win: f32,
    pub out_win: f32,
    /// Each card's value alone (the rest of the package out).
    pub alone: Vec<Measure>,
    /// Each card's value in the full package (leave-one-out).
    pub in_package: Vec<Measure>,
    /// The whole package's value.
    pub package: Measure,
    pub sum_alone: Measure,
    pub sum_in_package: Measure,
    /// Package minus the sum of the cards alone.
    pub interaction: Measure,
}

/// Deck-games a package test of `n` cards plays (for progress bars).
pub fn package_size(n: usize, games: u32) -> usize {
    (2 + 2 * n) * games as usize
}

/// Reads the outcomes of the `2 + 2n` decks `package_decks` made.
fn read(
    o: &Outcomes,
    n: usize,
) -> (
    Vec<Measure>,
    Vec<Measure>,
    Measure,
    Measure,
    Measure,
    Measure,
) {
    let alone: Vec<Measure> = (0..n).map(|i| o.diff(2 + i, 1)).collect();
    let in_package: Vec<Measure> = (0..n).map(|i| o.diff(0, 2 + n + i)).collect();
    let package = o.diff(0, 1);
    let sum_alone = o.contrast(
        &(0..n)
            .flat_map(|i| [(2 + i, 1.0), (1usize, -1.0)])
            .collect::<Vec<_>>(),
    );
    let sum_in = o.contrast(
        &(0..n)
            .flat_map(|i| [(0usize, 1.0), (2 + n + i, -1.0)])
            .collect::<Vec<_>>(),
    );
    let mut terms = vec![(0usize, 1.0f64), (1, -1.0)];
    for i in 0..n {
        terms.push((2 + i, -1.0));
        terms.push((1, 1.0));
    }
    (
        alone,
        in_package,
        package,
        sum_alone,
        sum_in,
        o.contrast(&terms),
    )
}

/// The decks of a package test: 0 the full deck, 1 the package out, 2..2+n each card alone
/// (the rest out), then each card left out. `pkg` pairs each card's index in `base` with its
/// stand-in.
pub fn package_decks(base: &Prepared, pkg: &[(u16, SimCard)]) -> Vec<Prepared> {
    let n = pkg.len();
    let out = |skip: &dyn Fn(usize) -> bool| -> Prepared {
        let swaps: Vec<(u16, SimCard)> = (0..n)
            .filter(|&i| skip(i))
            .map(|i| pkg[i].clone())
            .collect();
        base.with_swaps(&swaps)
    };
    let mut decks = vec![base.with_swaps(&[]), out(&|_| true)];
    for i in 0..n {
        decks.push(out(&|j| j != i));
    }
    for i in 0..n {
        decks.push(out(&|j| j == i));
    }
    decks
}

/// Plays the package test on `base` (see the module notes). None when cancelled.
#[allow(clippy::too_many_arguments)]
pub fn package_test(
    base: &Prepared,
    pkg: &[(u16, SimCard)],
    gauntlet: &[Prepared],
    rules: &Rules,
    games: u32,
    seed: u64,
    done: &AtomicUsize,
    cancel: &AtomicBool,
) -> Option<(Outcomes, usize)> {
    let decks = package_decks(base, pkg);
    let refs: Vec<&Prepared> = decks.iter().collect();
    let o = play_all(&refs, gauntlet, rules, games, seed, done, cancel);
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    Some((o, pkg.len()))
}

/// Tests the package `cards` in `deck` against `gauntlet`. Cards not in the deck (from the
/// pool) are first put in place of the deck's weakest spells by the builder's score (never
/// a package card). Err when a card is unknown, the deck has no room, or the package isn't
/// 2-8 cards; Ok(None) when cancelled.
#[allow(clippy::too_many_arguments)]
pub fn package_synergy(
    deck: &Deck,
    cards: &[String],
    db: &CardDb,
    rules: &Rules,
    gauntlet: &[Prepared],
    games: u32,
    seed: u64,
    done: &AtomicUsize,
    cancel: &AtomicBool,
) -> Result<Option<PackageReport>, String> {
    if !(2..=8).contains(&cards.len()) {
        return Err("Pick 2 to 8 cards.".into());
    }
    if gauntlet.is_empty() {
        return Err("No opponent decks to play against.".into());
    }
    let mut names: Vec<String> = Vec::new();
    for n in cards {
        let c = db
            .get(n)
            .ok_or_else(|| format!("{} isn't in the card database.", n))?;
        if c.is_land() {
            return Err(format!("{} is a land: pick spells.", c.name));
        }
        if !names.contains(&c.name) {
            names.push(c.name.clone());
        }
    }
    if names.len() < 2 {
        return Err("Pick 2 to 8 different cards.".into());
    }
    let mut base = deck.clone();
    let mut swapped_in = Vec::new();
    let missing: Vec<&String> = names.iter().filter(|n| base.count_of(n) == 0).collect();
    if !missing.is_empty() {
        let cuts: Vec<String> = crate::analysis::weakest(&base, db, rules)
            .into_iter()
            .map(|s| s.card.name.clone())
            .filter(|n| !names.contains(n) && base.commanders().iter().all(|c| *c != n))
            .collect();
        if cuts.len() < missing.len() {
            return Err("The deck has too few other spells to make room.".into());
        }
        for (add, cut) in missing.iter().zip(cuts) {
            base.remove(&cut, 1);
            base.add(add, 1);
            swapped_in.push(((*add).clone(), cut));
        }
    }
    let prep = Prepared::new(&base, db);
    let pkg: Vec<(u16, SimCard)> = names
        .iter()
        .map(|n| {
            let idx = prep
                .card_index(n)
                .ok_or_else(|| format!("{} is not in the deck.", n))?;
            Ok((idx, stand_in(db.get(n).unwrap())))
        })
        .collect::<Result<_, String>>()?;
    let Some((o, n)) = package_test(&prep, &pkg, gauntlet, rules, games, seed, done, cancel) else {
        return Ok(None);
    };
    let (alone, in_package, package, sum_alone, sum_in_package, interaction) = read(&o, n);
    Ok(Some(PackageReport {
        cards: names,
        swapped_in,
        games: o.games,
        full_win: o.win_rate(0),
        out_win: o.win_rate(1),
        alone,
        in_package,
        package,
        sum_alone,
        sum_in_package,
        interaction,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Collection;
    use std::path::PathBuf;

    fn db() -> CardDb {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
        CardDb::load(&dir.join("cards.json")).unwrap()
    }

    #[test]
    fn package_synergy_reports_consistent_numbers() {
        let db = db();
        let rules = Rules::default();
        let all = Collection::new(
            PathBuf::new(),
            db.cards.iter().map(|c| (c.name.clone(), 1)).collect(),
        );
        let cmds: Vec<&Card> = db.cards.iter().filter(|c| c.can_be_commander()).collect();
        let deck = crate::analysis::autobuild(cmds[0], &db, &all, &rules, None, None).0;
        let opp: Vec<Prepared> = cmds[1..4]
            .iter()
            .map(|c| {
                Prepared::new(
                    &crate::analysis::autobuild(c, &db, &all, &rules, None, None).0,
                    &db,
                )
            })
            .collect();
        let spells: Vec<String> = deck
            .cards
            .keys()
            .filter(|n| db.get(n).is_some_and(|c| !c.is_land()))
            .take(2)
            .cloned()
            .collect();
        // One more from outside the deck, in the deck's colours.
        let id = deck.identity(&db).unwrap();
        let outside = db
            .cards
            .iter()
            .find(|c| {
                !c.is_land()
                    && !c.can_be_commander()
                    && deck.count_of(&c.name) == 0
                    && c.identity().is_subset_of(id)
            })
            .unwrap()
            .name
            .clone();
        let mut pkg = spells.clone();
        pkg.push(outside.clone());
        let games = 40;
        let done = AtomicUsize::new(0);
        let r = package_synergy(
            &deck,
            &pkg,
            &db,
            &rules,
            &opp,
            games,
            7,
            &done,
            &AtomicBool::new(false),
        )
        .unwrap()
        .unwrap();
        assert_eq!(done.load(Ordering::Relaxed), package_size(3, games));
        assert_eq!(r.cards.len(), 3);
        assert_eq!(r.swapped_in.len(), 1);
        assert_eq!(r.swapped_in[0].0, outside);
        assert_eq!(r.alone.len(), 3);
        assert_eq!(r.in_package.len(), 3);
        // The package value is the full deck's win rate minus the package-out one.
        assert!((r.package.gain - (r.full_win - r.out_win)).abs() < 1e-4);
        // Interaction = package - sum alone, game by game (so also on average).
        let sum: f32 = r.alone.iter().map(|m| m.gain).sum();
        assert!((r.sum_alone.gain - sum).abs() < 1e-4);
        assert!((r.interaction.gain - (r.package.gain - r.sum_alone.gain)).abs() < 1e-4);
        // Bad input.
        assert!(
            package_synergy(
                &deck,
                &spells[..1],
                &db,
                &rules,
                &opp,
                games,
                7,
                &done,
                &AtomicBool::new(false)
            )
            .is_err()
        );
        // Cancelled before it starts: no report.
        assert!(
            package_synergy(
                &deck,
                &spells,
                &db,
                &rules,
                &opp,
                games,
                7,
                &done,
                &AtomicBool::new(true)
            )
            .unwrap()
            .is_none()
        );
    }
}
