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
        || has("counter target spell")
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
        || has("connive")
        || (has("draw a card") && repeatable)
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
    if taps_for_mana || has("search your library for a basic land") {
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
    if has("create two") || (has("create") && has("creature token") && has("flying")) {
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
        // The job-only pool really drops filler (ownership included).
        let coll = Collection::new(
            std::path::PathBuf::new(),
            [
                ("Crimson Operative".to_string(), 1),
                ("Sol Ring".to_string(), 1),
            ]
            .into(),
        );
        let jc = without_filler(&coll, &db);
        assert_eq!(jc.owned("Crimson Operative"), 0);
        assert_eq!(jc.owned("Sol Ring"), 1);
    }
}
