//! Colour packs: for each two-colour pair, the groups of cards the pair is best at, as
//! slots with minimum counts - what the lock-out control plan (`cohesion::QUOTAS`) is for
//! a control deck. A pack is a way to build a deck; whether it is the right one for a
//! commander and a pool is settled by playtests: the optimiser races every pack that fits
//! the commander's colours against the current deck (`optimize`), and `packs test` in the
//! command line prints the race.
//!
//! Three-colour packs combine what the three pairs inside them share (white-blue-black:
//! the full control suite; black-red-green: removal, ramp and big creatures; and so on).
//!
//! What each pair excels at follows the usual reading of the colour pairs: white-blue
//! control and fliers; blue-black control with evasion; black-red removal and
//! aggression; red-green big creatures; green-white going wide with tokens; white-black
//! attrition, life loss and lifegain; blue-red spells; black-green the graveyard; red-white
//! cheap attackers and Equipment; green-blue ramp, cards and big creatures made bigger.

use crate::card::{Card, ColorSet};
use crate::cohesion::{Job, jobs};
use crate::db::{CardDb, Collection, Rules};
use crate::deck::Deck;

/// A kind of card a pack wants, read from the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ramp,
    Cards,
    Removal,
    Sweeper,
    /// Counterspells, taxes, tappers, exile-until.
    Lock,
    Flyer,
    /// Deathtouch or lifelink.
    Deterrent,
    /// Each opponent loses life.
    Drain,
    /// A creature with 5 or more power.
    Big,
    /// A cheap attacker: two or more power for two mana or less, or haste for three.
    Aggro,
    /// Makes the team bigger or gets it through: anthems, trample, double strike, Overrun.
    Finisher,
    /// Makes creature tokens.
    Tokens,
    /// Returns cards from the graveyard.
    Recursion,
    /// Gains life again and again (or lifelink).
    Lifegain,
    /// Puts +1/+1 counters on creatures.
    Counters,
    /// Sacrifices creatures for value.
    Sacrifice,
    /// Equipment and Auras for your own creatures.
    Gear,
    /// Pays off casting instants and sorceries.
    SpellPayoff,
    /// A cheap instant or sorcery (three mana or less).
    CheapSpell,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Ramp => "ramp",
            Kind::Cards => "card advantage",
            Kind::Removal => "removal",
            Kind::Sweeper => "sweepers",
            Kind::Lock => "counters and locks",
            Kind::Flyer => "fliers",
            Kind::Deterrent => "deathtouch/lifelink",
            Kind::Drain => "drain",
            Kind::Big => "big creatures",
            Kind::Aggro => "cheap attackers",
            Kind::Finisher => "team pump and trample",
            Kind::Tokens => "token makers",
            Kind::Recursion => "recursion",
            Kind::Lifegain => "lifegain",
            Kind::Counters => "+1/+1 counters",
            Kind::Sacrifice => "sacrifice",
            Kind::Gear => "equipment and auras",
            Kind::SpellPayoff => "spell payoffs",
            Kind::CheapSpell => "cheap spells",
        }
    }

    /// Is `card` of this kind?
    pub fn is(self, card: &Card) -> bool {
        if card.is_land() {
            return false;
        }
        let j = jobs(card);
        // Rules text without reminder text.
        let mut t = String::new();
        let mut depth = 0;
        for ch in card.oracle_text.to_lowercase().chars() {
            match ch {
                '(' => depth += 1,
                ')' => depth = (depth - 1i32).max(0),
                c if depth == 0 => t.push(c),
                _ => {}
            }
        }
        let has = |s: &str| t.contains(s);
        let power = card
            .power
            .as_deref()
            .and_then(|p| p.parse::<i32>().ok())
            .unwrap_or(0);
        let creature = card.is_creature();
        let kw = |k: &str| card.parsed().has_keyword(k);
        match self {
            Kind::Ramp => j.contains(&Job::Ramp),
            Kind::Cards => j.contains(&Job::CardAdvantage),
            Kind::Removal => j.contains(&Job::Removal) || j.contains(&Job::Sweeper),
            Kind::Sweeper => j.contains(&Job::Sweeper),
            Kind::Lock => j.contains(&Job::Lock),
            Kind::Flyer => j.contains(&Job::Evasion),
            Kind::Deterrent => j.contains(&Job::Deterrent),
            Kind::Drain => j.contains(&Job::Drain),
            Kind::Big => creature && power >= 5,
            Kind::Aggro => {
                creature
                    && (card.mana_value() <= 2 && power >= 2
                        || card.mana_value() <= 3 && kw("haste"))
            }
            Kind::Finisher => {
                has("creatures you control get +")
                    || has("creatures you control gain trample")
                    || has("creatures you control have trample")
                    || has("creatures you control gain double strike")
                    || has("gets +") && has("gains trample")
                    || has("double the power")
                    || has("+1/+1 counter on each creature you control")
            }
            Kind::Tokens => has("create") && has("creature token"),
            Kind::Recursion => {
                has("from your graveyard") && (has("return") || has("put"))
                    || has("you may cast") && has("from your graveyard")
            }
            Kind::Lifegain => {
                kw("lifelink")
                    || has("you gain")
                        && has("life")
                        && (has("whenever") || has("at the beginning"))
                    || has("whenever you gain life")
            }
            Kind::Counters => {
                has("+1/+1 counter")
                    && (has("each") || has("target") || has("another") || has("whenever"))
            }
            Kind::Sacrifice => {
                has("sacrifice a creature")
                    || has("sacrifice another")
                    || has("whenever a creature you control dies")
                    || has("whenever another creature you control dies")
            }
            Kind::Gear => {
                card.type_line.contains("Equipment")
                    || card.type_line.contains("Aura") && has("enchant creature") && has("gets +")
                    || has("equipped creature")
                    || has("for each equipment")
            }
            Kind::SpellPayoff => {
                has("whenever you cast an instant or sorcery")
                    || has("whenever you cast a noncreature spell")
                    || has("instant and sorcery spells you cast")
                    || has("whenever you cast your second spell")
            }
            Kind::CheapSpell => {
                (card.type_line.contains("Instant") || card.type_line.contains("Sorcery"))
                    && card.mana_value() <= 3
            }
        }
    }
}

/// A two- or three-colour pack: the colours' plan as slots with minimum counts (per 60 cards).
#[derive(Debug)]
pub struct Pack {
    pub name: &'static str,
    /// Its colours, WUBRG letters.
    pub colors: &'static str,
    pub plan: &'static str,
    pub slots: &'static [(u32, Kind)],
}

pub const PACKS: &[Pack] = &[
    Pack {
        name: "White-blue fliers and control",
        colors: "WU",
        plan: "Hold the ground with locks and counters, draw cards, win in the air.",
        slots: &[
            (6, Kind::Flyer),
            (4, Kind::Removal),
            (4, Kind::Cards),
            (2, Kind::Lock),
        ],
    },
    Pack {
        name: "Blue-black control",
        colors: "UB",
        plan: "Kill or counter what matters, out-draw the table, finish with evasive threats.",
        slots: &[
            (8, Kind::Removal),
            (7, Kind::Cards),
            (4, Kind::Lock),
            (5, Kind::Flyer),
        ],
    },
    Pack {
        name: "Black-red removal and pressure",
        colors: "BR",
        plan: "Cheap attackers backed by the most removal, with drain to finish.",
        slots: &[
            (9, Kind::Removal),
            (7, Kind::Aggro),
            (4, Kind::Drain),
            (3, Kind::Sacrifice),
        ],
    },
    Pack {
        name: "Red-green stompy",
        colors: "RG",
        plan: "Ramp into the biggest creatures and give them trample.",
        slots: &[
            (7, Kind::Ramp),
            (9, Kind::Big),
            (5, Kind::Removal),
            (3, Kind::Finisher),
        ],
    },
    Pack {
        name: "Green-white go-wide",
        colors: "GW",
        plan: "Flood the board with tokens, then pump the whole team.",
        slots: &[
            (9, Kind::Tokens),
            (4, Kind::Finisher),
            (5, Kind::Ramp),
            (4, Kind::Removal),
            (3, Kind::Lifegain),
        ],
    },
    Pack {
        name: "White-black attrition",
        colors: "WB",
        plan: "Trade one for one, drain life, gain it back, and bring the best creatures back.",
        slots: &[
            (8, Kind::Removal),
            (5, Kind::Drain),
            (5, Kind::Lifegain),
            (3, Kind::Recursion),
            (4, Kind::Cards),
        ],
    },
    Pack {
        name: "Blue-red spells",
        colors: "UR",
        plan: "Burn and card draw, creatures that pay off casting spells, and fliers to finish.",
        slots: &[
            (7, Kind::Removal),
            (5, Kind::Cards),
            (3, Kind::SpellPayoff),
            (4, Kind::Flyer),
        ],
    },
    Pack {
        name: "Black-green graveyard value",
        colors: "BG",
        plan: "Trade resources, bring creatures back, and grind with removal and deathtouch.",
        slots: &[
            (7, Kind::Removal),
            (4, Kind::Recursion),
            (5, Kind::Ramp),
            (4, Kind::Deterrent),
            (5, Kind::Cards),
        ],
    },
    Pack {
        name: "Red-white aggro and equipment",
        colors: "RW",
        plan: "Cheap attackers, Equipment to keep them relevant, and a team pump to end it.",
        slots: &[
            (6, Kind::Removal),
            (5, Kind::Aggro),
            (3, Kind::Finisher),
            (3, Kind::Gear),
        ],
    },
    Pack {
        name: "Green-blue ramp and big threats",
        colors: "GU",
        plan: "Ramp, draw, land big creatures and make them bigger, with counters as backup.",
        slots: &[
            (8, Kind::Ramp),
            (7, Kind::Cards),
            (7, Kind::Big),
            (3, Kind::Finisher),
            (2, Kind::Lock),
        ],
    },
    // ---- three colours: what the three pairs inside share.
    Pack {
        name: "White-blue-black control",
        colors: "WUB",
        plan: "The full control suite: removal, sweepers, counters and card draw, with fliers to finish.",
        slots: &[
            (9, Kind::Removal),
            (7, Kind::Cards),
            (5, Kind::Lock),
            (5, Kind::Flyer),
        ],
    },
    Pack {
        name: "Blue-black-red removal and cards",
        colors: "UBR",
        plan: "Kill everything, keep drawing, and let drain and a few evasive threats close.",
        slots: &[
            (10, Kind::Removal),
            (7, Kind::Cards),
            (3, Kind::Lock),
            (4, Kind::Drain),
        ],
    },
    Pack {
        name: "Black-red-green midrange",
        colors: "BRG",
        plan: "Ramp, the best removal, and big creatures that come back.",
        slots: &[
            (8, Kind::Removal),
            (6, Kind::Ramp),
            (7, Kind::Big),
            (3, Kind::Recursion),
        ],
    },
    Pack {
        name: "Red-green-white big creatures",
        colors: "RGW",
        plan: "Ramp into big creatures and tokens, then pump the whole team.",
        slots: &[
            (6, Kind::Ramp),
            (8, Kind::Big),
            (5, Kind::Tokens),
            (3, Kind::Finisher),
            (4, Kind::Removal),
        ],
    },
    Pack {
        name: "Green-white-blue ramp and fliers",
        colors: "GWU",
        plan: "Ramp and draw, hold the ground with locks, win in the air.",
        slots: &[
            (6, Kind::Ramp),
            (6, Kind::Cards),
            (6, Kind::Flyer),
            (4, Kind::Lock),
            (4, Kind::Removal),
        ],
    },
    Pack {
        name: "White-black-green counters and attrition",
        colors: "WBG",
        plan: "Removal and recursion grind the table down while +1/+1 counters and lifegain build a lead.",
        slots: &[
            (8, Kind::Removal),
            (4, Kind::Recursion),
            (4, Kind::Counters),
            (4, Kind::Lifegain),
            (5, Kind::Ramp),
        ],
    },
    Pack {
        name: "Blue-red-white fliers and burn",
        colors: "URW",
        plan: "Burn and counters clear the way for fliers; card draw keeps it going.",
        slots: &[
            (8, Kind::Removal),
            (7, Kind::Flyer),
            (5, Kind::Cards),
            (3, Kind::Lock),
        ],
    },
    Pack {
        name: "Black-green-blue ramp and value",
        colors: "BGU",
        plan: "Ramp, draw the most cards, kill what matters and reuse the graveyard.",
        slots: &[
            (7, Kind::Ramp),
            (7, Kind::Cards),
            (7, Kind::Removal),
            (3, Kind::Recursion),
        ],
    },
    Pack {
        name: "Red-white-black aggression",
        colors: "RWB",
        plan: "Cheap attackers and tokens, backed by removal and drain.",
        slots: &[
            (8, Kind::Removal),
            (5, Kind::Aggro),
            (4, Kind::Tokens),
            (4, Kind::Drain),
        ],
    },
    Pack {
        name: "Green-blue-red ramp and big spells",
        colors: "GUR",
        plan: "Ramp hard, draw cards, land the biggest creatures, burn what blocks.",
        slots: &[
            (8, Kind::Ramp),
            (6, Kind::Cards),
            (7, Kind::Big),
            (5, Kind::Removal),
        ],
    },
];

impl Pack {
    pub fn color_set(&self) -> ColorSet {
        ColorSet::parse(self.colors).unwrap_or_default()
    }

    /// How many cards of each slot `deck` holds: (kind, have, wanted).
    pub fn census(&self, deck: &Deck, db: &CardDb) -> Vec<(Kind, u32, u32)> {
        self.slots
            .iter()
            .map(|(min, kind)| {
                let have = deck
                    .cards
                    .iter()
                    .filter(|(n, _)| db.get(n).is_some_and(|c| kind.is(c)))
                    .map(|(_, k)| *k)
                    .sum();
                (*kind, have, *min)
            })
            .collect()
    }
}

/// The packs a commander can play: both of the pack's colours in its colour identity.
pub fn for_commander(cmd: &Card) -> Vec<&'static Pack> {
    PACKS
        .iter()
        .filter(|p| p.color_set().is_subset_of(cmd.identity()))
        .collect()
}

/// `cmd`'s deck built on `pack`: the commander's archetype package, then the pack's slots,
/// each filled with the best cards of its kind in the pack's colours (or colourless),
/// then the builder's usual fill.
pub fn build(pack: &Pack, cmd: &Card, db: &CardDb, coll: &Collection, rules: &Rules) -> Deck {
    let colors = pack.color_set();
    let preds: Vec<Box<dyn Fn(&Card) -> bool>> = pack
        .slots
        .iter()
        .map(|(_, kind)| {
            let kind = *kind;
            Box::new(move |c: &Card| kind.is(c) && c.identity().is_subset_of(colors))
                as Box<dyn Fn(&Card) -> bool>
        })
        .collect();
    let quotas: Vec<crate::archetypes::Quota> = pack
        .slots
        .iter()
        .zip(preds.iter())
        .map(|((min, _), f)| (*min, f.as_ref() as &dyn Fn(&Card) -> bool))
        .collect();
    // On top of the commander's own archetype (its package of cards that work with the
    // commander stays): the pack adds the colour pair's slots. (Playtests: packs built
    // without the commander's package measured 1 to 5 points worse than auto-build.)
    let arch = crate::archetypes::for_commander(&cmd.name).unwrap_or_else(|| {
        crate::archetypes::ARCHETYPES
            .iter()
            .find(|a| a.name == "Midrange value")
            .expect("the Midrange value archetype")
    });
    let t = crate::archetypes::targets(arch);
    let (mut deck, _) = crate::archetypes::build_with(arch, cmd, db, coll, rules, &t, &quotas);
    deck.name = format!("{} ({})", pack.name, cmd.name);
    deck
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> CardDb {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
        CardDb::load(&dir.join("cards.json")).unwrap()
    }

    #[test]
    fn kinds_read_cards_and_packs_build() {
        let db = db();
        let is = |k: Kind, n: &str| k.is(db.get(n).unwrap());
        assert!(is(Kind::Ramp, "Circuitous Route"));
        assert!(is(Kind::Big, "Pelakka Wurm"));
        assert!(is(Kind::Finisher, "Overrun"));
        assert!(is(Kind::Gear, "Swiftfoot Boots"));
        assert!(is(Kind::Lock, "Essence Scatter"));
        assert!(is(Kind::Cards, "Prime Speaker Zegana"));
        assert!(!is(Kind::Big, "Llanowar Elves"));
        // Ten pairs and ten three-colour sets, each a different one.
        assert_eq!(PACKS.len(), 20);
        let mut seen: Vec<ColorSet> = PACKS.iter().map(|p| p.color_set()).collect();
        seen.sort_by_key(|c| c.0);
        seen.dedup();
        assert_eq!(seen.len(), 20);
        // A blue-green commander gets the blue-green pack only; a five-colour one all twenty.
        let tatyova = db.get("Tatyova, Benthic Druid").unwrap();
        let fits = for_commander(tatyova);
        assert_eq!(fits.len(), 1);
        assert_eq!(fits[0].colors, "GU");
        assert_eq!(
            for_commander(db.get("Thanos, the Mad Titan").unwrap()).len(),
            20
        );
        // Built from every card, the pack's slots are filled.
        let all = Collection::new(
            std::path::PathBuf::new(),
            db.cards.iter().map(|c| (c.name.clone(), 1)).collect(),
        );
        let rules = Rules::default();
        let deck = build(fits[0], tatyova, &db, &all, &rules);
        assert_eq!(deck.main_count(), deck.main_target(&rules));
        for (kind, have, want) in fits[0].census(&deck, &db) {
            assert!(have >= want, "{}: {} of {}", kind.label(), have, want);
        }
    }
}
