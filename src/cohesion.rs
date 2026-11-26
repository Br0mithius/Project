//! What job a card does in the deck, for decks where every card must pull its weight (the
//! lock-out control plan: answers, locks, card advantage, ramp, and creatures that matter).
//!
//! A card with no job is filler: it may still win simulated games against a weak table, but
//! it doesn't add to the plan. Creatures are welcome when they have a use - evasion,
//! deathtouch or lifelink (they stop attacks), an effect that answers, draws, ramps or locks,
//! or a drain. Vanilla stats, reach, prowess and one-shot Treasure/Food don't count, and a
//! single "exile the top card, you may play it" isn't card advantage.

use crate::card::Card;
use crate::db::Collection;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Job {
    /// Exile-until-it-leaves, counterspells, "can't attack/block", tappers, taxes.
    Lock,
    /// Targeted removal (spells or creatures that deal with a threat).
    Removal,
    /// Board wipes and one-sided shrinks.
    Sweeper,
    /// Two or more cards, or repeatable draw.
    CardAdvantage,
    /// Repeatable mana or land search.
    Ramp,
    /// Flying: a threat that gets through, and a blocker against flyers.
    Evasion,
    /// Deathtouch or lifelink: opponents don't want to attack into it.
    Deterrent,
    /// Each opponent loses life / takes damage: a way to close.
    Drain,
    /// Several real bodies from one card (tokens with 3+ power or flying).
    Board,
}

impl Job {
    pub fn label(self) -> &'static str {
        match self {
            Job::Lock => "lock",
            Job::Removal => "removal",
            Job::Sweeper => "sweeper",
            Job::CardAdvantage => "card advantage",
            Job::Ramp => "ramp",
            Job::Evasion => "flying",
            Job::Deterrent => "deathtouch/lifelink",
            Job::Drain => "drain",
            Job::Board => "bodies",
        }
    }
}

fn has_kw(card: &Card, kw: &str) -> bool {
    card.parsed().has_keyword(kw)
        || card
            .oracle_text
            .lines()
            .next()
            .is_some_and(|l| l.to_lowercase().split(", ").any(|k| k.trim() == kw))
}

/// The jobs `card` does (empty for lands and for filler).
pub fn jobs(card: &Card) -> Vec<Job> {
    let mut out = Vec::new();
    if card.is_land() {
        return out;
    }
    // Reminder text (in parentheses) explains keywords - a Treasure's "{T}, Sacrifice: add
    // one mana" isn't repeatable ramp - so it's left out.
    let mut t = String::new();
    let mut depth = 0;
    for ch in card.oracle_text.to_lowercase().chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth = (depth - 1).max(0),
            c if depth == 0 => t.push(c),
            _ => {}
        }
    }
    let has = |s: &str| t.contains(s);
    let creature = card.is_creature();
    // Locks and answers that hold a permanent.
    let exiles = has("exile target") || has("exile up to one target");
    if (exiles && (has("leaves the battlefield") || has("until ~ leaves")))
        || has("counter target")
        || has("can't attack")
        || has("can't block")
        || has("doesn't untap")
        || has("don't untap")
        || has("tap target")
        || (has("cost") && has("more to cast"))
        || has("more for each")
        || has("can't cast")
    {
        out.push(Job::Lock);
    }
    let roles = card.roles();
    if roles.contains(&crate::card::Role::Sweeper)
        || has("destroy all")
        || has("creatures your opponents control get -")
        || ((has("destroy each") || has("exile each")) && has("creature"))
        || (has("each creature") && (has("gets -") || has("damage")))
        || (has("damage to each") && has("creature"))
    {
        out.push(Job::Sweeper);
    }
    if roles.contains(&crate::card::Role::Removal)
        || has("destroy target")
        || (has("exile target") && !has("from your graveyard") && !has("card from"))
        || (has("deals")
            && (has("to any target") || has("any other target") || has("target creature")))
        || (has("deals damage equal") && has("noncreature spell"))
    {
        if !out.contains(&Job::Lock) || has("destroy target") {
            out.push(Job::Removal);
        }
    }
    // Card advantage: two or more cards, or a repeatable draw.
    let repeatable = has("whenever") || has("at the beginning") || has("{t}:") || has(": draw");
    if has("draw two")
        || has("draws two")
        || has("draw three")
        || has("draw x")
        || has("draw cards equal")
        || has("draws cards equal")
        || has("connive")
        // Looting ("discard a card: draw a card", Óin the Brave) trades one card for one:
        // card filtering, not card advantage.
        || (has("draw a card") && repeatable && !has("discard a card"))
        || (has("draw a card") && has("each opponent discards"))
    {
        out.push(Job::CardAdvantage);
    }
    // Repeatable mana (not restricted to artifacts) or land search.
    let taps_for_mana = t.lines().any(|l| {
        (l.contains("{t}") || l.contains("{t},"))
            && (l.contains("add {") || l.contains("add one mana") || l.contains("add x mana"))
            && !l.contains("can't be spent")
    });
    // Landcycling isn't ramp: the card is a land in all but name and counts toward the land
    // cap (see `mana`).
    // Land search of any wording ("for up to two basic land cards", "for a land card").
    let finds_land = t.lines().any(|l| {
        l.contains("search your library for")
            && l.contains("land")
            && !l.contains("landcycling")
            && !l.contains("cycling")
    });
    if taps_for_mana || finds_land || has("play an additional land") {
        out.push(Job::Ramp);
    }
    if creature {
        if has_kw(card, "flying") {
            out.push(Job::Evasion);
        }
        if has_kw(card, "deathtouch") || has_kw(card, "lifelink") {
            out.push(Job::Deterrent);
        }
    }
    if has("each opponent loses") || has("damage to each opponent") || has("each opponent discards")
    {
        out.push(Job::Drain);
    }
    // A token every turn or on a trigger that keeps happening is a board that rebuilds
    // itself (Alien Invasion: a hasty token each combat, one counter bigger each time -
    // measured +4.6 win-rate points in the lock-out deck, where it was being left out as
    // filler).
    let token_engine = t.lines().any(|l| {
        l.contains("create")
            && l.contains("creature token")
            && (l.starts_with("at the beginning of") || l.starts_with("whenever"))
    });
    if has("create two")
        || (has("create") && has("creature token") && has("flying"))
        || token_engine
    {
        out.push(Job::Board);
    }
    out.sort();
    out.dedup();
    out
}

/// A card's main role in the deck, for grouping and colour-coding: the first of its jobs in
/// the order a control deck cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Lock,
    Answer,
    Cards,
    Ramp,
    Threat,
    Filler,
    Land,
}

impl Category {
    pub const ALL: [Category; 7] = [
        Category::Lock,
        Category::Answer,
        Category::Cards,
        Category::Ramp,
        Category::Threat,
        Category::Filler,
        Category::Land,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Lock => "Locks",
            Category::Answer => "Answers",
            Category::Cards => "Card advantage",
            Category::Ramp => "Ramp",
            Category::Threat => "Threats",
            Category::Filler => "Filler",
            Category::Land => "Lands",
        }
    }
}

/// `card`'s main role (see `Category`).
pub fn category(card: &Card) -> Category {
    if card.is_land() {
        return Category::Land;
    }
    let j = jobs(card);
    if j.contains(&Job::Lock) {
        Category::Lock
    } else if j.contains(&Job::Removal) || j.contains(&Job::Sweeper) {
        Category::Answer
    } else if j.contains(&Job::CardAdvantage) {
        Category::Cards
    } else if j.contains(&Job::Ramp) {
        Category::Ramp
    } else if j.is_empty() {
        Category::Filler
    } else {
        Category::Threat
    }
}

/// What a lock-out control deck must hold: at least this many cards (of 60) doing one of
/// these jobs. Auto-build fills them first, and the swap search never trades below them.
pub const QUOTAS: [(u32, &[Job]); 4] = [
    (5, &[Job::Lock]),
    (9, &[Job::Removal, Job::Sweeper]),
    (7, &[Job::CardAdvantage]),
    (8, &[Job::Ramp]),
];

/// The plan's minimums as they apply now: `QUOTAS`, unless a playtest experiment changed
/// the counts (`set_quota_counts`).
pub fn quotas() -> [(u32, &'static [Job]); 4] {
    use std::sync::atomic::Ordering;
    let mut q = QUOTAS;
    for (k, slot) in q.iter_mut().enumerate() {
        let v = QUOTA_COUNTS[k].load(Ordering::Relaxed);
        if v != u32::MAX {
            slot.0 = v;
        }
    }
    q
}

static QUOTA_COUNTS: [std::sync::atomic::AtomicU32; 4] = [
    std::sync::atomic::AtomicU32::new(u32::MAX),
    std::sync::atomic::AtomicU32::new(u32::MAX),
    std::sync::atomic::AtomicU32::new(u32::MAX),
    std::sync::atomic::AtomicU32::new(u32::MAX),
];
static QUOTA_FLOOR: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Playtest experiments: the plan's minimum counts (locks, answers, card advantage, ramp),
/// and whether a slot takes only cards above the builder's quality floor. `None` puts the
/// shipped counts back.
pub fn set_quota_counts(counts: Option<[u32; 4]>, floor: bool) {
    use std::sync::atomic::Ordering;
    for k in 0..4 {
        QUOTA_COUNTS[k].store(counts.map_or(u32::MAX, |c| c[k]), Ordering::Relaxed);
    }
    QUOTA_FLOOR.store(floor, Ordering::Relaxed);
}

/// Whether the plan's slots take only cards above the builder's quality floor.
pub fn quota_floor() -> bool {
    QUOTA_FLOOR.load(std::sync::atomic::Ordering::Relaxed)
}

/// Does swapping `cut` for `add` keep `deck`'s plan? A card that fills one of the plan's
/// quotas (`QUOTAS`) is only swapped for a card doing that same job, unless the deck holds
/// `slack` more of them than the quota asks (room for that many swaps at once). So a lock
/// may be upgraded to a better lock, never traded for a body.
///
/// Why: playtests score a card by the opponents they are played against. Against weak
/// simulated decks nothing needs an answer and removal reads as dead weight (five removal
/// spells measured 4.6 win-rate points worse than five ordinary creatures against the
/// simulated league, about even against the real pod's decks, and 5.4 to 6.5 points
/// better against decks as strong as ours), so the measurements alone would strip a
/// control deck of the answers it needs when the opposition is real.
pub fn swap_keeps_plan(
    deck: &crate::deck::Deck,
    db: &crate::db::CardDb,
    cut: &str,
    add: &str,
    slack: u32,
) -> bool {
    let Some(cut_card) = db.get(cut) else {
        return true;
    };
    let cut_jobs = jobs(cut_card);
    let add_jobs = db.get(add).map(jobs).unwrap_or_default();
    quotas().iter().all(|(min, js)| {
        let in_group = |j: &[Job]| j.iter().any(|x| js.contains(x));
        if !in_group(&cut_jobs) || in_group(&add_jobs) {
            return true;
        }
        let have: u32 = deck
            .cards
            .iter()
            .filter(|(n, _)| db.get(n).is_some_and(|c| in_group(&jobs(c))))
            .map(|(_, k)| *k)
            .sum();
        have >= min + slack.max(1)
    })
}

/// Does `candidate` keep the plan `current` has? In each of the plan's groups (`QUOTAS`) it
/// holds at least the quota, or as many as `current` when that already has fewer. A
/// whole-deck alternative for a control deck must pass: otherwise a build with three fewer
/// locks could replace the deck in one step, past the like-for-like rule for single swaps.
pub fn keeps_plan(
    current: &crate::deck::Deck,
    candidate: &crate::deck::Deck,
    db: &crate::db::CardDb,
) -> bool {
    quotas().iter().all(|(min, js)| {
        let count = |d: &crate::deck::Deck| -> u32 {
            d.cards
                .iter()
                .filter(|(n, _)| {
                    db.get(n)
                        .is_some_and(|c| jobs(c).iter().any(|x| js.contains(x)))
                })
                .map(|(_, k)| *k)
                .sum()
        };
        count(candidate) >= (*min).min(count(current))
    })
}

/// True when `card` does no job (lands never count as filler).
pub fn is_filler(card: &Card) -> bool {
    !card.is_land() && jobs(card).is_empty()
}

/// `coll` without filler: the pool a cohesive build may draw from (lands kept).
pub fn without_filler(coll: &Collection, db: &crate::db::CardDb) -> Collection {
    // Rebuilt with `Collection::new`, not by editing `counts`: ownership lookups go
    // through the collection's index, which must drop the filler too.
    coll.filtered(|name| db.get(name).is_none_or(|card| !is_filler(card)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::CardDb;

    fn db() -> CardDb {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
        CardDb::load(&dir.join("cards.json")).unwrap()
    }

    #[test]
    fn filler_and_jobs_match_a_control_players_reading() {
        let db = db();
        let j = |n: &str| jobs(db.get(n).unwrap());
        // No real job: bodies, one-shot Treasure, one-shot impulse draw, reach.
        for n in [
            "Crimson Operative",
            "Ant-Man's Army",
            "Pet Avengers",
            "Hydraulic Helper",
        ] {
            assert!(j(n).is_empty(), "{} should be filler: {:?}", n, j(n));
        }
        // Creatures with a use.
        assert!(j("Vampire Nighthawk").contains(&Job::Evasion));
        assert!(j("Vampire Nighthawk").contains(&Job::Deterrent));
        assert!(j("Ravening Warg").contains(&Job::Deterrent));
        assert!(j("Cloudblazer").contains(&Job::CardAdvantage));
        assert!(j("Undercover Skrull").contains(&Job::Ramp));
        assert!(j("Doc Samson, Super Psychiatrist").contains(&Job::Ramp));
        assert!(j("Massacre Wurm").contains(&Job::Sweeper));
        assert!(j("Kang, Temporal Tyrant").contains(&Job::CardAdvantage));
        assert!(j("Arbiter of Woe").contains(&Job::Drain));
        // Locks and answers.
        for n in [
            "Web Up",
            "Stasis Snare",
            "Super Villain Lockup",
            "Sound the Trumpets",
        ] {
            assert!(j(n).contains(&Job::Lock), "{}: {:?}", n, j(n));
        }
        assert!(j("Fumigate").contains(&Job::Sweeper));
        assert!(j("Celebrate the Mountain-king").contains(&Job::Lock));
        assert!(!j("Political Triumph").contains(&Job::Sweeper));
        assert!(j("Hero's Downfall").contains(&Job::Removal));
        assert!(j("Futurist Forge").contains(&Job::CardAdvantage));
        assert!(j("Sol Ring").contains(&Job::Ramp));
        assert!(j("Alien Invasion").contains(&Job::Board));
        assert!(!j("Óin the Brave").contains(&Job::CardAdvantage));
        assert!(j("Essence Scatter").contains(&Job::Lock));
        assert!(j("Prime Speaker Zegana").contains(&Job::CardAdvantage));
        for n in ["Circuitous Route", "Springbloom Druid", "Burnished Hart"] {
            assert!(j(n).contains(&Job::Ramp), "{}: {:?}", n, j(n));
        }
        // The job-only pool really drops filler (ownership included).
        let coll = Collection::new(
            std::path::PathBuf::new(),
            [
                ("Crimson Operative".to_string(), 1),
                ("Sol Ring".to_string(), 1),
            ]
            .into(),
        );
        // The plan guard: a lock is swapped for a lock, not for a body, when the deck holds
        // no more locks than the plan asks.
        let mut deck = crate::deck::Deck::default();
        for n in [
            "Web Up",
            "Stasis Snare",
            "Super Villain Lockup",
            "Celebrate the Mountain-king",
            "Storm, Windrider",
        ] {
            deck.add(n, 1);
        }
        assert!(!swap_keeps_plan(
            &deck,
            &db,
            "Stasis Snare",
            "Ravening Warg",
            1
        ));
        assert!(swap_keeps_plan(
            &deck,
            &db,
            "Stasis Snare",
            "Sound the Trumpets",
            1
        ));
        assert!(swap_keeps_plan(&deck, &db, "Storm, Windrider", "Web Up", 1));
        let jc = without_filler(&coll, &db);
        assert_eq!(jc.owned("Crimson Operative"), 0);
        assert_eq!(jc.owned("Sol Ring"), 1);
    }
}
