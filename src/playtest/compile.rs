//! Cards as the simulator plays them: costs, types, keywords and abilities, compiled once
//! per deck from the ability parser's reading (`abilities`) plus the rules text for the
//! details the game needs (conditions, phases, costs, which side an Aura helps).

use crate::abilities::{self, Effect, Event, When};
use crate::card::Card;

// Keyword bits.
pub const FLYING: u32 = 1;
pub const REACH: u32 = 1 << 1;
pub const TRAMPLE: u32 = 1 << 2;
pub const MENACE: u32 = 1 << 3;
pub const DEATHTOUCH: u32 = 1 << 4;
pub const LIFELINK: u32 = 1 << 5;
pub const HASTE: u32 = 1 << 6;
pub const FIRST_STRIKE: u32 = 1 << 7;
pub const DOUBLE_STRIKE: u32 = 1 << 8;
pub const VIGILANCE: u32 = 1 << 9;
pub const INDESTRUCTIBLE: u32 = 1 << 10;
pub const HEXPROOF: u32 = 1 << 11;
pub const DEFENDER: u32 = 1 << 12;
pub const UNBLOCKABLE: u32 = 1 << 13;
pub const FLASH: u32 = 1 << 14;
pub const PROWESS: u32 = 1 << 15;
pub const FEAR: u32 = 1 << 16;
pub const INTIMIDATE: u32 = 1 << 17;
pub const SKULK: u32 = 1 << 18;
pub const FLANKING: u32 = 1 << 19;
pub const CANT_BLOCK: u32 = 1 << 20;
pub const SHROUD: u32 = 1 << 21;

pub const KEYWORDS: [(&str, u32); 22] = [
    ("flying", FLYING),
    ("reach", REACH),
    ("trample", TRAMPLE),
    ("menace", MENACE),
    ("deathtouch", DEATHTOUCH),
    ("lifelink", LIFELINK),
    ("haste", HASTE),
    ("first strike", FIRST_STRIKE),
    ("double strike", DOUBLE_STRIKE),
    ("vigilance", VIGILANCE),
    ("indestructible", INDESTRUCTIBLE),
    ("hexproof", HEXPROOF),
    ("shroud", SHROUD),
    ("defender", DEFENDER),
    ("unblockable", UNBLOCKABLE),
    ("can't be blocked", UNBLOCKABLE),
    ("flash", FLASH),
    ("prowess", PROWESS),
    ("fear", FEAR),
    ("intimidate", INTIMIDATE),
    ("skulk", SKULK),
    ("flanking", FLANKING),
];

// Card type bits.
pub const T_CREATURE: u16 = 1;
pub const T_ARTIFACT: u16 = 1 << 1;
pub const T_ENCHANTMENT: u16 = 1 << 2;
pub const T_PLANESWALKER: u16 = 1 << 3;
pub const T_LAND: u16 = 1 << 4;
pub const T_INSTANT: u16 = 1 << 5;
pub const T_SORCERY: u16 = 1 << 6;
pub const T_EQUIPMENT: u16 = 1 << 7;
pub const T_AURA: u16 = 1 << 8;
pub const T_VEHICLE: u16 = 1 << 9;
pub const T_LEGENDARY: u16 = 1 << 10;
pub const T_SAGA: u16 = 1 << 11;
pub const T_TOKEN: u16 = 1 << 12;
pub const T_BASIC: u16 = 1 << 13;

/// Bit for "can make colourless mana ({C})" in a source's colour mask (WUBRG are bits 0-4).
pub const COLORLESS: u8 = 1 << 5;

// What a spell targets (for "whenever you cast a spell that targets ...").
pub const TGT_OWN_CREATURE: u8 = 1;
pub const TGT_OPP_CREATURE: u8 = 1 << 1;
pub const TGT_OPPONENT: u8 = 1 << 2;
pub const TGT_ARTIFACT_LAND: u8 = 1 << 3;
/// In a trigger's filter: "an Equipment spell or a spell that targets ...".
pub const TGT_OR_EQUIPMENT: u8 = 1 << 4;

/// What a spell's own abilities target (`TGT_*` bits): helpful effects on a creature of
/// yours, harmful ones on an opponent's creature, an opponent, an artifact or land.
fn spell_targets(abilities: &[SimAbility], types: u16, harmful_aura: bool) -> u8 {
    use crate::abilities::Kind;
    if types & T_AURA != 0 {
        return if harmful_aura {
            TGT_OPP_CREATURE
        } else {
            TGT_OWN_CREATURE
        };
    }
    let mut bits = 0;
    for a in abilities.iter().filter(|a| a.trig == Trig::Spell) {
        if a.text.contains("target creature you control")
            || a.text.contains("target creatures you control")
        {
            bits |= TGT_OWN_CREATURE;
        }
        if a.text.contains("target opponent") || a.text.contains("target player") {
            bits |= TGT_OPPONENT;
        }
        for e in &a.effects {
            let helpful = matches!(
                e,
                Effect::Grant { .. }
                    | Effect::PlusCounters { .. }
                    | Effect::Untap(_)
                    | Effect::Pump { power: 0.., .. }
            );
            let mut e = e.clone();
            for t in e.targets_mut() {
                if t.mass || t.kind == Kind::It || t.restr.has(crate::restrict::PREV) {
                    continue;
                }
                let creature = matches!(
                    t.kind,
                    Kind::Creature | Kind::CreatureOrPlaneswalker | Kind::AnyTarget
                );
                match (creature, t.whose) {
                    (true, abilities::Whose::Yours) => bits |= TGT_OWN_CREATURE,
                    (true, abilities::Whose::Theirs) => bits |= TGT_OPP_CREATURE,
                    (true, _) if helpful => bits |= TGT_OWN_CREATURE,
                    (true, _) => bits |= TGT_OPP_CREATURE,
                    _ => {}
                }
                if t.kind == Kind::AnyTarget {
                    bits |= TGT_OPPONENT;
                }
                if matches!(
                    t.kind,
                    Kind::Artifact | Kind::Land | Kind::ArtifactOrEnchantment
                ) {
                    bits |= TGT_ARTIFACT_LAND;
                }
            }
        }
    }
    bits
}

/// A mana cost: generic plus coloured pips (WUBRG order), {C} that only colourless mana
/// pays, and hybrid symbols (each a mask of the colours that can pay it).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Mana {
    pub generic: u8,
    pub pips: [u8; 5],
    pub x: bool,
    pub colorless: u8,
    pub hybrid: [u8; 3],
}

impl Mana {
    pub fn total(&self) -> u32 {
        self.generic as u32
            + self.pips.iter().map(|&p| p as u32).sum::<u32>()
            + self.colorless as u32
            + self.hybrid.iter().filter(|&&h| h != 0).count() as u32
    }

    pub fn generic(n: u32) -> Mana {
        Mana {
            generic: n.min(255) as u8,
            ..Default::default()
        }
    }

    /// Reads "{2}{U}{U}", "{C}" and hybrid "{U/B}" (Phyrexian and {2/W} symbols count as
    /// generic).
    pub fn parse(cost: &str) -> Mana {
        let mut m = Mana::default();
        for sym in crate::card::symbols_in(cost) {
            let s = sym.to_uppercase();
            let colour = |p: &str| {
                crate::card::COLORS
                    .iter()
                    .position(|c| p.len() == 1 && p.starts_with(*c))
            };
            if let Ok(n) = s.parse::<u8>() {
                m.generic = m.generic.saturating_add(n);
            } else if s == "X" {
                m.x = true;
            } else if s == "C" {
                m.colorless += 1;
            } else if let Some((a, b)) = s.split_once('/')
                && let (Some(i), Some(j)) = (colour(a), colour(b))
            {
                match m.hybrid.iter().position(|&h| h == 0) {
                    Some(slot) => m.hybrid[slot] = (1 << i) | (1 << j),
                    None => m.generic = m.generic.saturating_add(1),
                }
            } else if let Some(i) = crate::card::COLORS
                .iter()
                .position(|c| s.len() == 1 && s.starts_with(*c))
            {
                m.pips[i] += 1;
            } else {
                m.generic = m.generic.saturating_add(1);
            }
        }
        m
    }

    pub fn minus_generic(mut self, n: u32) -> Mana {
        self.generic = (self.generic as u32).saturating_sub(n) as u8;
        self
    }

    pub fn plus_generic(mut self, n: u32) -> Mana {
        self.generic = (self.generic as u32 + n).min(255) as u8;
        self
    }
}

/// Which phase an "at the beginning of" trigger uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    Upkeep,
    /// "At the beginning of your (pre-combat / first) main phase".
    Main,
    Combat,
    End,
}

/// What a triggered ability listens for (beyond its own card's comings and goings).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ev {
    Cast,
    OtherEnters,
    Landfall,
    Death,
    GainLife,
    Draw,
    Discard,
    Damaged,
    Counters,
    Sacrifice,
    Tokens,
    Targeted,
    OthersAttack,
    TeamworkTapped,
    LoseLife,
    Scry,
    LoyaltyActivated,
    AbilityActivated,
    Tapped,
    Untapped,
    /// Any creature dealing combat damage to a player (not just the card itself).
    CombatDamageAny,
    NoncombatDamage,
    LeavesGraveyard,
    Bounced,
    Blocks,
    /// One creature attacking ("whenever a white creature you control attacks"), per attacker.
    EachAttacker,
    Other,
}

pub fn ev_bit(k: Ev) -> u32 {
    1 << (k as u32)
}

/// "a Villain spell", "another creature you control", "an artifact": what an event needs.
#[derive(Clone, Default, PartialEq)]
pub struct Filter {
    /// Only your things (`Some(true)`), only opponents' (`Some(false)`), or anyone's.
    pub yours: Option<bool>,
    /// Card types the thing must have one of (0 = any).
    pub types: u16,
    /// Card types it must not have ("noncreature").
    pub not_types: u16,
    /// A creature type it must have ("hero", "villain").
    pub subtype: Option<String>,
    /// "another"/"other": not the source itself.
    pub other: bool,
    /// "nontoken": real cards only; `token`: tokens only.
    pub nontoken: bool,
    pub token: bool,
    /// Everything else the thing must be (attacking, power 4 or greater, a Gate, blue...).
    pub restr: crate::restrict::Restr,
    /// A cast spell must target one of these (`TGT_*` bits; 0 = anything).
    pub targets: u8,
}

impl std::fmt::Debug for Filter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Filter {{ yours: {:?}, types: {}, not_types: {}, subtype: {:?}, other: {}, nontoken: {}, token: {}",
            self.yours,
            self.types,
            self.not_types,
            self.subtype,
            self.other,
            self.nontoken,
            self.token
        )?;
        if !self.restr.is_empty() {
            write!(f, ", restr: {:?}", self.restr)?;
        }
        if self.targets != 0 {
            write!(f, ", targets: {:#07b}", self.targets)?;
        }
        write!(f, " }}")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Trig {
    Spell,
    Enters,
    Dies,
    Attacks,
    CombatDamage,
    Phase {
        phase: Phase,
        any_player: bool,
        /// "Each opponent's draw step": not on its controller's own turn.
        opponents_only: bool,
    },
    Event(Ev, Filter),
    Activated {
        mana: u8,
        tap: bool,
        sac_self: bool,
        sac_other: bool,
        discard: bool,
        life: u8,
        sorcery: bool,
        /// The rest of the cost (X, counters, tapping others, exiling...).
        extra: ActCost,
    },
    /// Power-up (CR 702.193) with its full mana cost, colours included.
    PowerUp(Mana),
    Loyalty(i8),
    Chapter(u8),
    /// "When you do": fires from its card's counters or a choice (see `SimAbility::counter_goal`).
    Reflexive,
    Static,
}

/// The parts of an activated ability's cost beyond mana, {T}, sacrificing, discarding and
/// life.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ActCost {
    /// Number of {X} symbols ({X}{X} = 2); X is chosen when it's activated.
    pub x: u8,
    /// "Remove N counters from ~" (`plus`: +1/+1 counters, otherwise other counters).
    pub remove: u8,
    pub remove_plus: bool,
    /// "Remove any number of +1/+1 counters": all of them (X = how many).
    pub remove_all: bool,
    /// "Put a stun counter on ~" (1) / "put a page counter on ~" (2).
    pub add_counter: u8,
    /// "Tap N untapped artifacts / Elves you control".
    pub tap_others: u8,
    pub tap_types: u16,
    pub tap_subtype: Option<String>,
    /// "Exile ~" (from the battlefield).
    pub exile_self: bool,
    /// "Return ~ to its owner's hand".
    pub return_self: bool,
    /// "Exile ~ from your graveyard": only works from the graveyard.
    pub from_graveyard: bool,
    /// "Exile ~ from your hand": only works from the hand.
    pub from_hand: bool,
    /// "Exile N other (creature) cards from your graveyard".
    pub exile_cards: u8,
    pub exile_creatures: bool,
    /// "Exile black cards ... with N or more black mana symbols" (Baron Helmut Zemo).
    pub black_pips: u8,
    /// Boast: only if it attacked this turn, once each turn.
    pub boast: bool,
}

/// A cost paid while an ability resolves: "you may sacrifice another creature. If you do,
/// ...", "remove a bait counter from ~. If you do, ...".
#[derive(Debug, Clone, PartialEq)]
pub enum ResCost {
    /// Sacrifice a permanent of these types (`other`: not the source).
    Sacrifice {
        types: u16,
        other: bool,
    },
    /// "Sacrifice an artifact or discard a card".
    SacrificeOrDiscard(u16),
    SacrificeSelf,
    Mana(Mana),
    Life(u8),
    Discard,
    /// Remove a counter from the source (`plus`: a +1/+1 counter).
    RemoveCounter {
        plus: bool,
    },
    ExileSelf,
    /// "Exile this spell and N cards named ~ from your graveyard".
    ExileNamed(u8),
    /// A choice that costs nothing ("you may exile the top card of your library").
    Free,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PayCost {
    pub cost: ResCost,
    /// "You may" pay it (otherwise it's paid if it can be).
    pub optional: bool,
    /// "If it has six or more quest counters on it, sacrifice it. If you do, ...": paid
    /// only when this holds.
    pub cond: Option<Cond>,
    /// Effects before this index happen anyway; the cost sentence's own effects (up to
    /// `first`) are the payment, not effects; the rest need the payment.
    pub skip_from: u8,
    pub first: u8,
}

/// "As an additional cost to cast this spell, ...".
#[derive(Debug, Clone, PartialEq)]
pub enum ExtraCost {
    Sacrifice(u16),
    Discard,
    SacrificeOrPay(u16, Mana),
    DiscardOrPay(Mana),
    /// "Behold a Jace or pay {1}".
    BeholdOrPay(String, Mana),
}

/// Ward costs that aren't (only) mana: "Ward—Pay 2 life", "Ward—Discard a card".
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WardCost {
    pub life: u8,
    pub discard: bool,
    /// Discard a card *or* pay the mana (Titania).
    pub discard_or_mana: bool,
    pub sacrifice: u8,
    pub poison: u8,
}

/// "Spend this mana only to cast a Hero spell or to activate an ability of a Hero source":
/// what a restricted mana ability's mana may pay for.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpendOnly {
    /// Spells it may cast: card types (`ANY_TYPE` = any) - 0 means no spells.
    pub spell_types: u16,
    /// ... of this creature type ("chosen" = the type chosen as it entered).
    pub spell_sub: Option<String>,
    /// Abilities it may pay for: of sources with these types (`ANY_TYPE` = any; 0 = none).
    pub ability_types: u16,
    pub ability_sub: Option<String>,
    /// "This mana can't be spent to cast spells from your hand" (Heartwood Crafter).
    pub not_from_hand: bool,
}

/// Any card type, in `SpendOnly`.
pub const ANY_TYPE: u16 = 0xFFFF;

/// The spending restriction of a card's mana ability, if it has one.
fn spend_only_of(abilities: &[SimAbility]) -> Option<SpendOnly> {
    let a = abilities.iter().find(|a| {
        a.effects.iter().any(|e| matches!(e, Effect::AddMana(_)))
            && (a.text.contains("spend this mana only to")
                || a.text
                    .contains("can't be spent to cast spells from your hand"))
    })?;
    let t = a.text.as_str();
    if t.contains("can't be spent to cast spells from your hand") {
        return Some(SpendOnly {
            spell_types: ANY_TYPE,
            ability_types: ANY_TYPE,
            not_from_hand: true,
            ..Default::default()
        });
    }
    let rule = t.split("spend this mana only to ").nth(1)?;
    let rule = rule.trim_end_matches('.');
    let (spells, abilities) = match rule.find("activate") {
        Some(i) => (&rule[..i], &rule[i..]),
        None => (rule, ""),
    };
    let mut so = SpendOnly::default();
    // "cast a creature spell of the chosen type", "cast elf spells", "cast an artifact spell".
    if spells.contains("cast ") {
        let what = spells
            .split("cast ")
            .nth(1)
            .unwrap_or("")
            .split(" spell")
            .next()
            .unwrap_or("")
            .trim_start_matches("an ")
            .trim_start_matches("a ");
        so.spell_types = match types_of(what) {
            0 => ANY_TYPE,
            t => t,
        };
        so.spell_sub = if spells.contains("of the chosen type") {
            Some("chosen".into())
        } else {
            what.split_whitespace()
                .find_map(crate::restrict::subtype_word)
                .map(|id| crate::restrict::subtype_name(id).to_string())
        };
    }
    // "activate an ability of a Hero source", "activate abilities of creature sources",
    // "activate equip abilities".
    if abilities.contains("activate") {
        let what = abilities.split(" of ").nth(1).unwrap_or("");
        so.ability_types = if what.contains("creature") {
            T_CREATURE
        } else if abilities.contains("equip abilities") {
            T_EQUIPMENT
        } else {
            ANY_TYPE
        };
        so.ability_sub = if abilities.contains("of the chosen type") {
            Some("chosen".into())
        } else {
            what.split_whitespace()
                .find_map(crate::restrict::subtype_word)
                .map(|id| crate::restrict::subtype_name(id).to_string())
        };
    }
    Some(so)
}

/// "This spell costs {X} less to cast, where X is ...".
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SelfLess {
    GreatestToughness,
    GreatestPower,
    TotalPower,
    /// "The total power of creatures you control with flying".
    TotalPowerFlying,
}

/// Conditions ("if", "as long as") the game can check.
#[derive(Debug, Clone, PartialEq)]
pub enum Cond {
    Always,
    Equipped,
    Teamwork,
    Gift,
    Storied,
    AttackedThisTurn,
    CreatureDiedThisTurn,
    /// N or more cards in your graveyard (creature cards only if `creatures`).
    Graveyard(u8, bool),
    ControlPower(u8),
    ControlToughness(u8),
    ControlLands(u8),
    /// You control a permanent of this type/subtype ("another" excludes the source).
    Control(String, bool),
    NoPlaneswalker,
    HasPlaneswalker,
    LifeAtLeast(i32),
    LifeAboveStart,
    OpponentLostLife,
    DrawnThisTurn(u8),
    GainedLife,
    MyTurn,
    NotPrepared,
    CastFromGraveyard,
    Kicked,
    /// The source isn't a token.
    NotToken,
    /// You haven't cast a spell this turn.
    NoSpellsThisTurn,
    /// The source has at least N +1/+1 counters.
    CountersAtLeast(i16),
    /// The source's power is N or less.
    PowerAtMost(i16),
    HasShield,
    /// You control at least N Treasures.
    Treasures(u8),
    /// You control at least N creatures (or permanents) of this type or subtype.
    ControlCount(u8, String),
    /// Creatures you control have total power N or greater.
    TotalPower(u16),
    /// The source's power is N or greater.
    PowerAtLeast(i16),
    /// N or more creatures died this turn.
    CreaturesDied(u8),
    /// An opponent has cast a spell this turn.
    OpponentCastSpell,
    /// An opponent lost life (was dealt damage) last turn.
    OpponentLostLifeLastTurn,
    /// The X paid for the spell or ability is N or more.
    XAtLeast(i32),
    /// An opponent (the one targeted) has exactly N life.
    OpponentLifeExactly(i32),
    /// The creature that triggered it has greater power or toughness than the source.
    SubjectBigger,
    /// The creature that triggered it (that died) was attacking.
    SubjectWasAttacking,
    /// The creature that triggered it (that died) was dealt damage by the source this turn.
    SubjectHurtBySource,
    /// The source isn't a creature right now.
    SourceNotCreature,
    /// The source has this creature type.
    SourceSubtype(String),
    /// N or more loyalty counters among Jaces you control.
    JaceLoyalty(i16),
    /// You activated a loyalty ability this turn.
    LoyaltyActivated,
    /// Any player controls a permanent of these colours.
    AnyPermanentColor(u8),
    /// The source has at least N counters other than +1/+1 (divinity, page...).
    OtherCountersAtLeast(i16),
    /// The source is attacking.
    SourceAttacking,
    /// The source dealt combat damage to a player this turn.
    SourceHitThisTurn,
    /// The source hasn't dealt combat damage yet.
    SourceNeverHit,
    /// You scried or surveilled this turn.
    Scried,
    /// Exactly one creature you control is attacking.
    AttacksAlone,
    /// You attack with N or more creatures.
    AttackersAtLeast(u8),
    /// A creature of this type you control attacked or entered this turn.
    AttackedOrEntered(String),
    /// One of your attackers has this creature type.
    AttackerSubtype(String),
    /// Your attackers have total power N or greater.
    AttackingPower(u16),
    /// You control a Jace or have a Jace card in hand (to behold).
    CanBeholdJace,
    /// It isn't your turn ("whenever you cast a spell during an opponent's turn").
    NotMyTurn,
    /// An opponent was dealt noncombat damage this turn (not just any life loss).
    OpponentNoncombatDamaged,
    /// Mana from a Treasure was spent on the spell just cast.
    TreasureSpent,
    /// This is the Nth time the ability has resolved this turn.
    NthResolution(u8),
    /// The loyalty ability just activated removed at least N loyalty counters.
    LoyaltyCostAtLeast(u8),
    /// N or more cards of these types in your graveyard ("instant and/or sorcery cards").
    GraveyardTypes(u8, u16),
    /// You control another creature with power N or greater.
    ControlOtherPower(u8),
    /// You controlled the permanent the ability just removed ("if you controlled that
    /// creature").
    LastLeftWasYours,
    /// The spell just countered had mana value N or less.
    CounteredMvAtMost(u8),
    /// You put a +1/+1 counter on the source this turn.
    CounteredThisTurn,
    /// The source (as it died) had a counter other than +1/+1 ("a revival counter").
    HadOtherCounters,
    /// What the ability chose is of this creature type ("bear", "another hero").
    TargetIs(String),
    /// The source was cast ("if you cast it"), not put onto the battlefield another way.
    WasCast,
    /// Can't be checked: happens this often.
    Chance(f32),
    /// The other branch of an "X. If <c>, Y instead" / "X if <c>. Otherwise, Y" pair.
    Not(Box<Cond>),
}

#[derive(Debug, Clone)]
pub struct SimAbility {
    pub trig: Trig,
    pub cond: Cond,
    pub optional: bool,
    pub once: bool,
    pub modal: bool,
    /// Modal abilities whose modes have several effects: effects per mode, in order (empty =
    /// each effect is a mode; see `SimAbility::mode_groups`).
    pub modes: Vec<u8>,
    /// Each mode's text after the ability's own ("when ~ enters, choose — destroy target
    /// artifact"), read when that mode resolves.
    pub mode_texts: Vec<String>,
    /// Each mode's `card_restr`.
    pub mode_restr: Vec<crate::restrict::Restr>,
    pub effects: Vec<Effect>,
    /// Lower-case rules text (for X values and details).
    pub text: String,
    /// "Until end of turn" control changes (threaten effects).
    pub until_eot: bool,
    /// Reflexive abilities that fire when the card reaches N counters ("third plan counter").
    pub counter_goal: Option<i16>,
    /// What a static ability's "creatures you control"-style wording covers.
    pub filter: Filter,
    /// "This ability triggers only once each turn".
    pub once_per_turn: bool,
    /// "Whenever you draw your second card each turn" / "casts their second spell each turn".
    pub second_draw: bool,
    /// Works while the card is in its owner's graveyard ("return ~ from your graveyard to
    /// the battlefield", "exile ~ from your graveyard: ..."), not on the battlefield.
    pub from_graveyard: bool,
    /// What the card it returns, finds or makes players sacrifice must be ("creature card
    /// with mana value 2 or less", "a creature with the greatest mana value").
    pub card_restr: crate::restrict::Restr,
    /// A cost paid as it resolves ("you may pay {2}. If you do, ...").
    pub pay: Option<PayCost>,
    /// "... If it's a Zombie card, draw a card.": effects that need the card in question
    /// (the top card, the returned card, the destroyed permanent) to be of a kind.
    pub gate: Option<Gate>,
}

/// Does this ability work from the graveyard (and only there)?
pub fn graveyard_ability(text: &str) -> bool {
    text.contains("~ from your graveyard")
}

/// Effects `from..to` of an ability happen only if the card it dealt with is of a kind.
#[derive(Debug, Clone, PartialEq)]
pub struct Gate {
    pub from: u8,
    pub to: u8,
    /// Card types it must have all of ("legendary enchantment").
    pub types: u16,
    pub subtype: Option<String>,
    /// The card is the top card of your library (looked at as it resolves).
    pub top: bool,
    /// Or a condition instead of a card ("... If he's attacking, untap him").
    pub cond: Option<Cond>,
}

impl SimAbility {
    /// The text and card restriction mode `m` resolves with.
    pub fn mode_text(&self, m: usize) -> (&str, crate::restrict::Restr) {
        match (self.mode_texts.get(m), self.mode_restr.get(m)) {
            (Some(t), Some(r)) if self.mode_texts.len() == self.mode_groups().len() => (t, *r),
            _ => (&self.text, self.card_restr),
        }
    }

    /// The effects of each mode of a modal ability, as index ranges into `effects` (one
    /// effect per mode when the modes weren't recorded).
    pub fn mode_groups(&self) -> Vec<std::ops::Range<usize>> {
        let n = self.effects.len();
        if self.modes.iter().map(|&m| m as usize).sum::<usize>() != n || self.modes.is_empty() {
            return (0..n).map(|k| k..k + 1).collect();
        }
        let mut out = Vec::new();
        let mut at = 0;
        for &m in &self.modes {
            out.push(at..at + m as usize);
            at += m as usize;
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BackKind {
    /// Cast the Adventure, then the card from exile later (CR 715).
    Adventure,
    /// A creature's prepare spell: castable as a copy while it's prepared (CR 722).
    Prepare,
    /// Other double-faced cards: the back's spell is cast once instead.
    Other,
}

#[derive(Debug, Clone)]
pub struct BackFace {
    pub kind: BackKind,
    pub cost: Mana,
    pub types: u16,
    pub abilities: Vec<SimAbility>,
    pub extra_cost: Option<ExtraCost>,
    /// What its spell targets (`TGT_*` bits).
    pub spell_targets: u8,
}

#[derive(Debug, Clone)]
pub struct SimCard {
    pub name: String,
    pub cost: Mana,
    pub mv: u8,
    pub colors: u8,
    pub types: u16,
    pub subtypes: Vec<String>,
    pub power: i16,
    pub toughness: i16,
    pub loyalty: i16,
    pub kw: u32,
    /// Ward's mana part (and `ward_cost` for life, discard, sacrifice, poison).
    pub ward: u8,
    pub ward_cost: WardCost,
    /// Colours it has protection from.
    pub protection: u8,
    pub toxic: u8,
    /// Colours a land (or mana ability) makes.
    pub produces: u8,
    pub mana_amount: u8,
    pub enters_tapped: bool,
    /// "Enters tapped unless you control a planeswalker / two or more other lands".
    pub enters_tapped_unless: Option<Cond>,
    /// "~ doesn't untap during your untap step" (Traxos; Bombur unless an enduring story).
    pub no_untap: bool,
    /// "As this land enters, you may behold a Jace. If you don't, it enters tapped."
    pub behold_untapped: bool,
    pub equip: Option<Mana>,
    /// "Equip—{2}, Pay 2 life".
    pub equip_life: u8,
    pub crew: Option<u8>,
    /// Cycling cost and what it finds: "" = draws a card, "land" = a basic land, or a
    /// land type / creature type ("mountain", "halfling").
    pub cycling: Option<(u8, String)>,
    pub flashback: Option<Mana>,
    /// "Flashback—{1}{U/R}{U/R}, Discard a card".
    pub flashback_discard: bool,
    pub kicker: Option<Mana>,
    pub overload: Option<Mana>,
    /// MSH Sneak: an alternative cost when you attacked this turn.
    pub sneak: Option<Mana>,
    pub teamwork: Option<u8>,
    pub offspring: Option<u8>,
    pub gift: bool,
    pub extort: bool,
    pub convoke: bool,
    pub improvise: bool,
    pub affinity: bool,
    pub living_weapon: bool,
    pub changeling: bool,
    pub cant_be_countered: bool,
    /// An Aura that hurts what it enchants (cast on an opponent's creature).
    pub harmful_aura: bool,
    pub chapters: u8,
    pub storied: bool,
    /// "Spells you cast cost {N} less" (with an optional subtype/type filter).
    pub cost_reduction: Option<(u8, Filter)>,
    /// "This spell costs {X} less to cast, where X is ..." (the Ghaltas).
    pub self_less: Option<SelfLess>,
    /// "Power-up abilities of other creatures you control cost {N} less to activate."
    pub power_up_less: u8,
    /// "As an additional cost to cast this spell, ...".
    pub extra_cost: Option<ExtraCost>,
    /// A mana ability: {T}: Add ... (rocks, dorks, Treasures-like permanents).
    pub mana_ability: bool,
    /// What its mana ability counts ("Elf you control", "~'s power"), if it does.
    pub mana_count: Option<String>,
    pub abilities: Vec<SimAbility>,
    pub back: Option<BackFace>,
    /// Rough worth of the card on its own (for the AI's quick estimates).
    pub value: f32,
    /// Bit per `Ev` kind its abilities listen for (so events skip cards that don't care).
    pub ev_mask: u32,
    /// The same for abilities that work from the graveyard.
    pub gy_ev_mask: u32,
    pub has_static: bool,
    /// Counters it enters with: (kind - "+1/+1", "shield", other names -, amount, condition).
    pub enters_counters: Vec<(String, EntersCounters, Cond)>,
    /// Power and/or toughness defined by a count ("*/*": Crusader of Odric, Tarmogoyf).
    pub cda: Option<Cda>,
    /// What casting it targets (`TGT_*` bits), for "a spell that targets ..." triggers.
    pub spell_targets: u8,
    /// What its mana may be spent on, if its mana ability says ("spend this mana only to
    /// cast a Hero spell"); `free_mask` is what its other mana abilities make for anything.
    pub spend_only: Option<SpendOnly>,
    pub free_mask: u8,
    /// "You can't cast this spell unless ..." (Proft): when it may be cast.
    pub cast_cond: Option<Cond>,
    /// "Affinity for Elves": the creature or land type it counts (None: artifacts).
    pub affinity_for: Option<String>,
    /// Instances of prowess ("Prowess, prowess" triggers twice).
    pub prowess: u8,
    /// "Equip Wizard {1}" / "Equip worthy {1}": a cost for creatures of a kind only (`equip`
    /// is then the cost for any other creature, if there is one).
    pub equip_for: Option<(Mana, String)>,
}

/// A characteristic-defining ability (CR 604.3): power and/or toughness equal to a count.
#[derive(Debug, Clone, PartialEq)]
pub struct Cda {
    /// What is counted ("creatures you control", "card types among cards in all graveyards").
    pub phrase: String,
    pub power: bool,
    pub toughness: bool,
    /// "Its toughness is equal to that number plus 1".
    pub plus_t: i16,
}

/// "~'s power and toughness are each equal to the number of creatures you control", "~'s
/// power is equal to the number of ...".
fn cda_of(text: &str) -> Option<Cda> {
    for line in text.lines() {
        let l = line.trim();
        let l = l.split(" — ").last().unwrap_or(l);
        if !l.starts_with("~'s power") {
            continue;
        }
        let both = l.starts_with("~'s power and toughness are each equal to the number of ");
        let power = both || l.starts_with("~'s power is equal to the number of ");
        if !power {
            continue;
        }
        let phrase = l
            .split("the number of ")
            .nth(1)?
            .trim_end_matches('.')
            .to_string();
        let plus_t = if l.contains("toughness is equal to that number plus 1") {
            1
        } else {
            0
        };
        return Some(Cda {
            phrase,
            power: true,
            toughness: both || l.contains("its toughness is equal to that number"),
            plus_t,
        });
    }
    None
}

impl SimAbility {
    /// The mode effect `k` belongs to.
    pub fn mode_of(&self, k: usize) -> usize {
        self.mode_groups()
            .iter()
            .position(|r| r.contains(&k))
            .unwrap_or(k)
    }

    /// Works only while the card is in the graveyard ("return ~ from your graveyard to your
    /// hand", "exile ~ from your graveyard: ...").
    pub fn graveyard_only(&self) -> bool {
        self.text.contains("~ from your graveyard")
    }
}

impl SimCard {
    pub fn is(&self, t: u16) -> bool {
        self.types & t != 0
    }

    pub fn is_permanent(&self) -> bool {
        !self.is(T_INSTANT | T_SORCERY)
    }

    pub fn has_subtype(&self, s: &str) -> bool {
        self.changeling && self.is(T_CREATURE) || self.subtypes.iter().any(|x| x == s)
    }
}

fn types_of(type_line: &str) -> u16 {
    let t = type_line.to_lowercase();
    let main = t.split('—').next().unwrap_or(&t);
    let mut bits = 0;
    for (word, bit) in [
        ("creature", T_CREATURE),
        ("artifact", T_ARTIFACT),
        ("enchantment", T_ENCHANTMENT),
        ("planeswalker", T_PLANESWALKER),
        ("land", T_LAND),
        ("instant", T_INSTANT),
        ("sorcery", T_SORCERY),
        ("legendary", T_LEGENDARY),
        ("basic", T_BASIC),
    ] {
        if main.contains(word) {
            bits |= bit;
        }
    }
    let sub = t.split('—').nth(1).unwrap_or("");
    for (word, bit) in [
        ("equipment", T_EQUIPMENT),
        ("aura", T_AURA),
        ("vehicle", T_VEHICLE),
        ("saga", T_SAGA),
    ] {
        if sub.contains(word) {
            bits |= bit;
        }
    }
    bits
}

fn number_word(w: &str) -> Option<u8> {
    Some(match w {
        "a" | "an" | "one" | "first" => 1,
        "two" | "second" => 2,
        "three" | "third" => 3,
        "four" | "fourth" => 4,
        "five" | "fifth" => 5,
        "six" | "sixth" => 6,
        "seven" | "seventh" => 7,
        "eight" | "eighth" => 8,
        "nine" | "ninth" => 9,
        "ten" | "tenth" => 10,
        "twelve" => 12,
        "thirteen" => 13,
        "fifteen" => 15,
        "twenty" => 20,
        "twenty-five" => 25,
        d => d
            .trim_end_matches(|c: char| c.is_alphabetic())
            .parse()
            .ok()?,
    })
}

/// The number right after `key` in `text` ("equip {2}" -> 2, "crew 3" -> 3).
fn number_after(text: &str, key: &str) -> Option<u8> {
    let i = text.find(key)?;
    let rest = text[i + key.len()..].trim_start();
    let rest = rest.trim_start_matches(['{', '—', '-']);
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// The number before `key` ("three or more cards" with key " or more").
fn number_before(text: &str, key: &str) -> Option<u8> {
    let i = text.find(key)?;
    let word = text[..i].split_whitespace().last()?;
    number_word(word)
}

pub fn filter_of(what: &str) -> Filter {
    let w = what.to_lowercase();
    let mut f = Filter::default();
    if w.contains("you control")
        || w.contains("your ")
        || w.starts_with("you ")
        || w.contains("you cast")
        || w.starts_with("whenever you ")
    {
        f.yours = Some(true);
    } else if w.contains("opponent") {
        f.yours = Some(false);
    }
    // Whole words only: "orc" is not in "sorcery", "elf" not in "itself", "another Elf" is
    // an Elf (not a subtype "other").
    let words: Vec<&str> = w
        .split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '\''))
        .filter(|x| !x.is_empty())
        .collect();
    f.other = words.iter().any(|x| *x == "another" || *x == "other");
    f.nontoken = words.iter().any(|x| *x == "nontoken" || *x == "nontokens");
    f.token = !f.nontoken && words.iter().any(|x| *x == "token" || *x == "tokens");
    for (word, bit) in [
        ("creature", T_CREATURE),
        ("artifact", T_ARTIFACT),
        ("enchantment", T_ENCHANTMENT),
        ("planeswalker", T_PLANESWALKER),
        ("instant", T_INSTANT),
        ("sorcery", T_SORCERY),
        ("legendary", T_LEGENDARY),
        ("equipment", T_EQUIPMENT),
        ("land", T_LAND),
    ] {
        let is = |x: &str| {
            x == word || x.strip_suffix('s') == Some(word) || word == "sorcery" && x == "sorceries"
        };
        if words.iter().any(|x| {
            x.strip_prefix("non-")
                .or_else(|| x.strip_prefix("non"))
                .is_some_and(is)
        }) {
            f.not_types |= bit;
        } else if words.iter().any(|x| is(x)) {
            f.types |= bit;
        }
    }
    let mut r = crate::restrict::Restr::read(&w);
    if r.has(crate::restrict::SUB_OR_TYPE) || r.subtype_ids().count() >= 2 {
        // "A Villain and/or artifact", "a noncreature or Dragon spell", "a Dwarf or
        // Equipment": the restriction decides.
        f.types = 0;
        f.not_types = 0;
    } else {
        // The card types are checked above; the restriction keeps the rest.
        r.any_types = 0;
        r.qual_types = 0;
    }
    f.subtype = r
        .subtype_ids()
        .next()
        .map(|i| crate::restrict::subtype_name(i).to_string());
    f.restr = r;
    f
}

/// "Whenever a creature you control attacks", "whenever an equipped creature you control
/// attacks a player alone": a trigger about one attacking creature (not "whenever you attack"
/// or "one or more creatures").
fn one_attacker(trigger: &str) -> bool {
    ["whenever a ", "whenever an ", "whenever another "]
        .iter()
        .any(|w| trigger.starts_with(w))
        && trigger.contains(" attacks")
        && !trigger.contains("one or more")
        // "Attacks alone" is the whole attack (`Ev::OthersAttack` about the lone attacker).
        && !trigger.contains(" alone")
}

/// The trigger of a triggered ability ("whenever a nontoken, non-Angel creature you control
/// dies") without its effect.
pub fn trigger_part(text: &str) -> &str {
    if !(text.starts_with("when") || text.starts_with("at ")) {
        return text;
    }
    let mut from = 0;
    while let Some(off) = text[from..].find(", ") {
        let i = from + off;
        let next = &text[i + 2..];
        let word = next.split([' ', ',']).next().unwrap_or("");
        // "a goblin, orc, or army you control dies", "a nontoken, non-angel creature".
        let listed = next.starts_with("non")
            || next.starts_with("or ")
            || next.starts_with("and/or ")
            || crate::restrict::subtype_word(word).is_some()
            || [
                "artifact",
                "enchantment",
                "creature",
                "planeswalker",
                "land",
            ]
            .contains(&word);
        if !listed {
            return &text[..i];
        }
        from = i + 2;
    }
    text
}

/// The word after `key` ("one or more merfolk you control" with key "one or more ").
fn word_after<'t>(text: &'t str, key: &str) -> Option<&'t str> {
    let i = text.find(key)?;
    text[i + key.len()..]
        .split([' ', ',', '.'])
        .next()
        .filter(|w| !w.is_empty())
}

/// "creatures" -> "creature", "elves" -> "elf".
fn singular(w: &str) -> String {
    if let Some(s) = w.strip_suffix("ves") {
        format!("{}f", s)
    } else {
        w.strip_suffix('s').unwrap_or(w).to_string()
    }
}

/// The part of an ability's text that states its condition ("if you control three or more
/// creatures", "as long as it's attacking"), so the trigger's own words ("at the beginning
/// of combat on your turn") aren't read as one.
fn cond_clause(t: &str) -> Option<&str> {
    let mut best: Option<usize> = None;
    for key in [
        "if ",
        "as long as ",
        "during your turn",
        " unless ",
        "while ~",
        "while it",
    ] {
        let mut from = 0;
        while let Some(off) = t[from..].find(key) {
            let i = from + off;
            from = i + key.len();
            if key == "if "
                && (i > 0 && t[..i].ends_with(|c: char| c.is_alphanumeric())
                    || t[i..].starts_with("if you do")
                    || t[i..].starts_with("if you don't")
                    || t[..i].ends_with("even ")
                    || t[..i].ends_with("as though "))
            {
                continue;
            }
            if best.is_none_or(|b| i < b) {
                best = Some(i);
            }
            break;
        }
    }
    let rest = t[best?..].trim();
    let end = rest
        .find(", ")
        .into_iter()
        .chain(rest.find(". "))
        .min()
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

/// Reads the condition in an ability's text.
pub fn cond_of(text: &str, conditional: bool) -> Cond {
    let t = text;
    // Trigger filters the event itself doesn't express.
    if t.contains("attacks alone") || t.contains("attacks a player alone") {
        return Cond::AttacksAlone;
    }
    if t.contains("attack with") && t.contains(" or more creatures") {
        return Cond::AttackersAtLeast(number_before(t, " or more creatures").unwrap_or(3));
    }
    if t.contains("attack with creatures with total power") {
        return Cond::AttackingPower(number_after(t, "total power").unwrap_or(10) as u16);
    }
    if let Some(w) = word_after(t, "whenever one or more ")
        && t.contains(" you control attack")
        && w != "creatures"
    {
        return Cond::AttackerSubtype(singular(w));
    }
    if t.contains("until end of turn, whenever ~ deals combat damage")
        || t.contains("dealt combat damage by ~ this turn")
    {
        return Cond::SourceHitThisTurn;
    }
    if t.contains("first time during each of your turns") {
        return Cond::MyTurn;
    }
    // "Whenever you cast a spell during an opponent's turn", "whenever a creature you
    // control deals combat damage during your turn": when the trigger counts.
    if t.starts_with("when") || t.starts_with("at ") {
        let head = trigger_part(t);
        if head.contains("during an opponent's turn") {
            return Cond::NotMyTurn;
        }
        if head.contains("during your turn") && !head.contains("becomes tapped") {
            return Cond::MyTurn;
        }
    }
    // "You gain 1 life if this is the first time this ability has resolved this turn. If
    // it's the second time, ...": each part is gated as the ability resolves; a split-off
    // "if this is the second time" part needs that count.
    if t.contains("time this ability has resolved this turn") {
        if let Some(rest) = t.split(" if this is the ").nth(1).filter(|_| conditional) {
            let n = number_word(rest.split_whitespace().next().unwrap_or("")).unwrap_or(1);
            if !t.contains("if it's the ") {
                return Cond::NthResolution(n);
            }
        }
        return Cond::Always;
    }
    // "If you controlled that creature, draw a card" (Azog): the permanent it just
    // destroyed.
    if t.contains("if you controlled that creature") {
        return Cond::LastLeftWasYours;
    }
    // "Choose one. If this spell was cast using teamwork, choose both instead", "put a
    // creature card ... If cast using teamwork, put any number ... instead": teamwork only
    // changes how much it does (checked as it resolves).
    if t.contains("teamwork") && t.contains("instead") && !t.starts_with("if ") {
        let before = t
            .split("if ~ was cast using teamwork")
            .next()
            .unwrap_or("")
            .trim();
        if !before.is_empty() && !before.ends_with(',') {
            return Cond::Always;
        }
    }
    // "Whenever a creature dealt damage by ~ this turn dies" (Predator Ooze).
    if t.contains("dealt damage by ~ this turn dies") {
        return Cond::SubjectHurtBySource;
    }
    // "A spell with mana value of the chosen quality" (Gollum): about half of them.
    if t.contains("casts a spell with mana value of the chosen quality") {
        return Cond::Chance(0.5);
    }
    // "When one or more Equipment become attached to that creature this way" (Thorin):
    // only with an Equipment to attach.
    if t.contains("equipment become attached") {
        return Cond::Control("equipment".into(), false);
    }
    if t.contains("when there are") && t.contains("counters on ~") {
        return Cond::OtherCountersAtLeast(number_before(t, " or more").unwrap_or(1) as i16);
    }
    if t.contains("enduring story") {
        return Cond::Storied;
    }
    if t.contains("teamwork")
        && (t.contains("cast using teamwork") || t.contains("teamwork cost was paid"))
    {
        return Cond::Teamwork;
    }
    if t.contains("gift was promised") {
        return Cond::Gift;
    }
    if t.contains("was kicked") {
        return Cond::Kicked;
    }
    if !conditional {
        // "Whenever ~ attacks, draw a card if her power is 4 or greater": a condition at
        // the end of the ability's only sentence.
        if let Some(clause) = cond_clause(t) {
            let at = t.find(clause).unwrap_or(0);
            if !t[..at].contains(". ") {
                if let c @ (Cond::PowerAtLeast(_) | Cond::SourceAttacking) = cond_from(clause) {
                    return c;
                }
            }
        }
        return Cond::Always;
    }
    // Replacement effects ("if ... would ..., ... instead") always apply; "for the first
    // time each turn" is a once-a-turn trigger (`once_per_turn`), not a condition.
    if t.contains(" would ") && t.contains("instead")
        || t.contains("for the first time each turn") && !t.contains(" if ")
    {
        return Cond::Always;
    }
    // "When ~ enters, if you cast it": not when it's put onto the battlefield.
    if [
        "if you cast it",
        "if you cast him",
        "if you cast her",
        "if you cast them",
    ]
    .iter()
    .any(|w| t.contains(w))
    {
        return Cond::WasCast;
    }
    // Wordings that hold whenever the ability comes up in a game.
    for always in [
        "attacks each combat if able",
        "fewer than ten cards in hand",
        "if it wasn't a",
        "if they were a creature",
        "if it's a creature",
        "if it's a planeswalker",
        "triggers an additional time",
        // A replacement for being discarded (Wilt-Leaf Liege): not a condition on the rest.
        "instead of putting it into",
    ] {
        if t.contains(always) {
            return Cond::Always;
        }
    }
    // "When this creature dies, if it had a revival counter on it, return it": checked on
    // the creature as it died.
    if t.contains("if it had a ") && t.contains(" counter on it") {
        return Cond::HadOtherCounters;
    }
    // The condition's own clause first ("if you control three or more creatures"), so a
    // trigger's "on your turn" isn't read as the condition.
    let clause = cond_clause(t);
    if let Some(clause) = clause {
        let c = cond_from(clause);
        if !matches!(c, Cond::Chance(_)) {
            return c;
        }
    }
    match cond_from(t) {
        // Only "if you do" / "if you don't" (a choice made as it resolves): no condition.
        Cond::Chance(_) if clause.is_none() => Cond::Always,
        // A punisher ("loses 5 life unless they discard a card"): the player picks as it
        // resolves (see `Effect::Drain`).
        Cond::Chance(_)
            if clause.is_some_and(|c| {
                c.starts_with("unless that player") || c.starts_with("unless they ")
            }) =>
        {
            Cond::Always
        }
        c => c,
    }
}

/// The condition a piece of rules text states (`Chance` when it can't be checked).
fn cond_from(t: &str) -> Cond {
    if t.contains("dealt noncombat damage this turn") {
        return Cond::OpponentNoncombatDamaged;
    }
    if t.contains("mana from a treasure was spent") {
        return Cond::TreasureSpent;
    }
    if t.contains("removed two or more loyalty counters") {
        return Cond::LoyaltyCostAtLeast(2);
    }
    if t.contains("spell's mana value was") && t.contains(" or less") {
        return Cond::CounteredMvAtMost(number_after(t, "mana value was").unwrap_or(2));
    }
    if t.contains("+1/+1 counters on ~ this turn") || t.contains("+1/+1 counter on ~ this turn") {
        return Cond::CounteredThisTurn;
    }
    // "If you control at least five other Forests": the one entering and five more.
    if let Some(rest) = t.split("control at least ").nth(1) {
        let mut w = rest.split_whitespace();
        if let (Some(n), Some("other"), Some(what)) =
            (w.next().and_then(number_word), w.next(), w.next())
        {
            return Cond::ControlCount(n + 1, singular(what.trim_end_matches([',', '.'])));
        }
    }
    // "Two or more instant and/or sorcery cards in your graveyard".
    if t.contains("in your graveyard") && t.contains(" or more ") {
        let what = t.split(" or more ").nth(1).unwrap_or("");
        let what = what.split(" in your graveyard").next().unwrap_or("");
        let mut types = 0;
        for (w, bit) in [
            ("instant", T_INSTANT),
            ("sorcery", T_SORCERY),
            ("artifact", T_ARTIFACT),
            ("land", T_LAND),
            ("enchantment", T_ENCHANTMENT),
        ] {
            if what.contains(w) {
                types |= bit;
            }
        }
        if types != 0 {
            return Cond::GraveyardTypes(number_before(t, " or more").unwrap_or(2), types);
        }
    }
    if t.contains("another creature with power") && t.contains("or greater") {
        return Cond::ControlOtherPower(number_after(t, "with power").unwrap_or(4));
    }
    if t.contains("he's equipped") || t.contains("she's equipped") || t.contains("it's equipped") {
        return Cond::Equipped;
    }
    if t.contains("isn't a token") || t.contains("not a token") {
        return Cond::NotToken;
    }
    if t.contains("didn't cast a spell this turn") {
        return Cond::NoSpellsThisTurn;
    }
    if t.contains("shield counter on") {
        return Cond::HasShield;
    }
    if t.contains("or more +1/+1 counters on") {
        return Cond::CountersAtLeast(number_before(t, " or more +1/+1").unwrap_or(3) as i16);
    }
    if t.contains("power is") && t.contains("or less") {
        return Cond::PowerAtMost(number_after(t, "power is").unwrap_or(1) as i16);
    }
    if t.contains("power is") && t.contains("or greater") && !t.contains("with power") {
        return Cond::PowerAtLeast(number_after(t, "power is").unwrap_or(4) as i16);
    }
    if t.contains("or more treasures") {
        return Cond::Treasures(number_before(t, " or more treasures").unwrap_or(4));
    }
    if t.contains("if x is") && t.contains(" or more") {
        return Cond::XAtLeast(number_after(t, "if x is").unwrap_or(10) as i32);
    }
    if t.contains("has exactly") && t.contains(" life") {
        return Cond::OpponentLifeExactly(number_after(t, "has exactly").unwrap_or(10) as i32);
    }
    if t.contains("greater power or toughness than ~") {
        return Cond::SubjectBigger;
    }
    if t.contains("if it was attacking") {
        return Cond::SubjectWasAttacking;
    }
    if t.contains("dealt damage by ~ this turn") {
        return Cond::SubjectHurtBySource;
    }
    if t.contains("~ isn't a creature") {
        return Cond::SourceNotCreature;
    }
    if let Some(w) = word_after(t, "if ~ is a ") {
        return Cond::SourceSubtype(w.to_string());
    }
    if t.contains("loyalty counters among jaces") {
        return Cond::JaceLoyalty(number_before(t, " or more loyalty").unwrap_or(25) as i16);
    }
    if t.contains("activated a loyalty ability this turn") {
        return Cond::LoyaltyActivated;
    }
    if t.contains("any player controls a") && t.contains(" permanent") {
        let m = color_bits(t.split("any player controls a").nth(1).unwrap_or(""));
        if m != 0 {
            return Cond::AnyPermanentColor(m);
        }
    }
    if t.contains("haven't dealt combat damage") || t.contains("hasn't dealt combat damage") {
        return Cond::SourceNeverHit;
    }
    if t.contains("it has a ") && t.contains(" counter on it") && !t.contains("+1/+1") {
        return Cond::OtherCountersAtLeast(1);
    }
    if t.contains("it's attacking") || t.contains("he's attacking") || t.contains("~ is attacking")
    {
        return Cond::SourceAttacking;
    }
    if t.contains("scried or surveilled this turn") {
        return Cond::Scried;
    }
    if t.contains("opponent has cast a spell this turn") {
        return Cond::OpponentCastSpell;
    }
    if t.contains("noncombat damage last turn") {
        return Cond::OpponentLostLifeLastTurn;
    }
    if t.contains("or more creatures died this turn") {
        return Cond::CreaturesDied(number_before(t, " or more creatures died").unwrap_or(3));
    }
    if t.contains("died this turn") {
        return Cond::CreatureDiedThisTurn;
    }
    if t.contains("total power") && t.contains("or greater") {
        return Cond::TotalPower(number_after(t, "total power").unwrap_or(8) as u16);
    }
    // "If you controlled that creature" (Azog): its removal goes at opponents' creatures.
    if t.contains("if you controlled that creature") {
        return Cond::Chance(0.05);
    }
    if t.contains("is equipped") || t.contains("~ equipped") {
        return Cond::Equipped;
    }
    if t.contains("isn't prepared") {
        return Cond::NotPrepared;
    }
    if t.contains("cast from a graveyard")
        || t.contains("cast it from your graveyard")
        || t.contains("wasn't cast from your hand")
    {
        return Cond::CastFromGraveyard;
    }
    // "If you attacked with a Hero this turn or a Hero entered the battlefield under your
    // control this turn".
    if let Some(w) = word_after(t, "attacked with a ").filter(|_| t.contains(" entered")) {
        return Cond::AttackedOrEntered(w.to_string());
    }
    if t.contains("attacked this turn")
        || t.contains("attacked with")
        || t.contains("dealt combat damage by")
    {
        return Cond::AttackedThisTurn;
    }
    if t.contains("in your graveyard") {
        let n = number_before(t, " or more").unwrap_or(4);
        return Cond::Graveyard(n, t.contains("creature card"));
    }
    if t.contains("with power") && t.contains("or greater") {
        return Cond::ControlPower(number_after(t, "with power").unwrap_or(4));
    }
    if t.contains("with toughness") && t.contains("or greater") {
        return Cond::ControlToughness(number_after(t, "with toughness").unwrap_or(4));
    }
    if t.contains("unless you control a planeswalker") || t.contains("don't control a planeswalker")
    {
        return Cond::NoPlaneswalker;
    }
    if t.contains("control a planeswalker") {
        return Cond::HasPlaneswalker;
    }
    if t.contains("or more lands") || t.contains("or more other lands") {
        return Cond::ControlLands(number_before(t, " or more").unwrap_or(5));
    }
    if t.contains("you control") && t.contains(" or more ") {
        // "Two or more other Wolves": besides this one (itself a Wolf).
        if let (Some(n), Some(w)) = (
            number_before(t, " or more other "),
            word_after(t, " or more other "),
        ) {
            return Cond::ControlCount(n + 1, singular(w));
        }
        if let (Some(n), Some(w)) = (
            number_before(t, " or more "),
            word_after(t, " or more ").filter(|w| *w != "life" && !w.contains("counter")),
        ) {
            return Cond::ControlCount(n, singular(w));
        }
    }
    if t.contains("or more life") {
        return Cond::LifeAtLeast(number_before(t, " or more life").unwrap_or(20) as i32);
    }
    if t.contains("starting life total") {
        return Cond::LifeAboveStart;
    }
    if t.contains("opponent lost life") || t.contains("been dealt noncombat damage") {
        return Cond::OpponentLostLife;
    }
    if t.contains("cards this turn") && t.contains("drawn") {
        return Cond::DrawnThisTurn(number_before(t, " or more").unwrap_or(2));
    }
    if t.contains("gained life this turn") {
        return Cond::GainedLife;
    }
    if t.contains("your turn") {
        return Cond::MyTurn;
    }
    // "Another" first: "if you control another Elf" is an Elf other than this one, not a
    // permanent of type "other".
    for key in [
        "if you control another ",
        "as long as you control another ",
        "if you control an ",
        "as long as you control an ",
        "if you control a ",
        "as long as you control a ",
    ] {
        if let Some(i) = t.find(key) {
            let rest = &t[i + key.len()..];
            // "An artifact creature or a Plan": either of those (each word must fit).
            let phrase = rest.split([',', '.']).next().unwrap_or("");
            if phrase.contains(" or a ") || phrase.contains(" or an ") {
                return Cond::Control(phrase.to_string(), key.contains("another"));
            }
            let word = rest.split([' ', ',', '.']).next().unwrap_or("");
            // Plurals and whole words ("Elves", "Wolves") name the subtype.
            let what = crate::restrict::subtype_word(word)
                .map(|id| crate::restrict::subtype_name(id).to_string())
                .unwrap_or_else(|| word.to_string());
            if !what.is_empty() {
                return Cond::Control(what, key.contains("another"));
            }
        }
    }
    if t.contains("behold a jace") {
        return Cond::CanBeholdJace;
    }
    Cond::Chance(0.5)
}

fn trig_of(w: &When, a: &abilities::Ability) -> Trig {
    let t = a.text.as_str();
    let head = trigger_part(t);
    match w {
        When::Spell => Trig::Spell,
        When::Enters => Trig::Enters,
        When::Dies => Trig::Dies,
        // An Aura's or Equipment's "whenever equipped creature attacks".
        When::Attacks
            if t.starts_with("whenever equipped creature")
                || t.starts_with("whenever enchanted creature") =>
        {
            Trig::Event(Ev::EachAttacker, Filter::default())
        }
        When::Attacks => Trig::Attacks,
        When::CombatDamage => Trig::CombatDamage,
        When::EachTurn => Trig::Phase {
            phase: if t.contains("end step") {
                Phase::End
            } else if t.contains("beginning of combat") || t.contains("beginning of each combat") {
                Phase::Combat
            } else if t.contains("main phase") {
                Phase::Main
            } else {
                Phase::Upkeep
            },
            any_player: t.contains("each upkeep")
                || t.contains("each end step")
                || t.contains("each combat")
                || t.contains("each player's")
                || t.contains("each opponent's"),
            opponents_only: t.contains("each opponent's"),
        },
        When::Event(e) => {
            let (kind, what) = match e {
                // "You cast a spell that targets ..., during an opponent's turn": the
                // spell is yours; what it targets or when isn't what it is.
                Event::Cast(what) => (
                    Ev::Cast,
                    what.split(" that target")
                        .next()
                        .unwrap_or("")
                        .split(" during ")
                        .next()
                        .unwrap_or(""),
                ),
                Event::OtherEnters(what) => (Ev::OtherEnters, what.as_str()),
                Event::Landfall => (Ev::Landfall, "a land you control"),
                Event::Death => (Ev::Death, trigger_part(t)),
                Event::GainLife => (Ev::GainLife, "you"),
                // "Whenever an opponent draws ...", "whenever a player draws ...".
                Event::Draw => (
                    Ev::Draw,
                    if trigger_part(t).contains("opponent") || trigger_part(t).contains("player") {
                        trigger_part(t)
                    } else {
                        "you"
                    },
                ),
                Event::Discard => (Ev::Discard, trigger_part(t)),
                Event::Damaged => (Ev::Damaged, "~"),
                Event::Counters => (Ev::Counters, trigger_part(t)),
                Event::Sacrifice => (Ev::Sacrifice, trigger_part(t)),
                Event::Tokens => (Ev::Tokens, "you"),
                Event::Targeted => (Ev::Targeted, "~"),
                // "Whenever a creature you control attacks (alone)": about each attacker.
                Event::OthersAttack if one_attacker(trigger_part(t)) => {
                    (Ev::EachAttacker, trigger_part(t))
                }
                Event::OthersAttack => (Ev::OthersAttack, "you"),
                Event::Other(what) => {
                    let k = if what.contains("teamwork") {
                        Ev::TeamworkTapped
                    } else if what.contains("loses life") || what.contains("lose life") {
                        Ev::LoseLife
                    } else if t.contains("scry or surveil") {
                        Ev::Scry
                    } else if t.contains("loyalty ability") {
                        Ev::LoyaltyActivated
                    } else if t.contains("activate an ability") {
                        Ev::AbilityActivated
                    } else if t.contains("becomes untapped") || t.contains("become untapped") {
                        Ev::Untapped
                    } else if t.contains("becomes tapped") || t.contains("become tapped") {
                        Ev::Tapped
                    } else if t.contains("noncombat damage") {
                        Ev::NoncombatDamage
                    } else if t.contains("combat damage") || t.contains("deal damage to a player") {
                        Ev::CombatDamageAny
                    } else if t.contains("leaves your graveyard") {
                        Ev::LeavesGraveyard
                    } else if t.contains("owner's hand") {
                        Ev::Bounced
                    } else if t.contains(" blocks") {
                        Ev::Blocks
                    } else if t.contains("targets ~") {
                        Ev::Targeted
                    } else if t.contains(" casts") || t.contains(" cast ") {
                        Ev::Cast
                    } else if t.contains(" dies") || t.contains(" die,") {
                        Ev::Death
                    } else {
                        Ev::Other
                    };
                    (k, what.as_str())
                }
            };
            let mut f = filter_of(what);
            // "Whenever you sacrifice a token", "whenever you put counters on ...": yours.
            if f.yours.is_none()
                && (head.starts_with("whenever you ") || head.starts_with("when you "))
            {
                f.yours = Some(true);
            }
            if matches!(
                kind,
                Ev::Cast
                    | Ev::GainLife
                    | Ev::Draw
                    | Ev::Tokens
                    | Ev::OthersAttack
                    | Ev::EachAttacker
                    | Ev::Landfall
                    | Ev::Scry
                    | Ev::LoyaltyActivated
                    | Ev::AbilityActivated
                    | Ev::CombatDamageAny
                    | Ev::LeavesGraveyard
            ) && f.yours.is_none()
                && !what.contains("player")
                && !what.contains("opponent")
            {
                f.yours = Some(true);
            }
            if kind == Ev::NoncombatDamage {
                // "an opponent is dealt noncombat damage": the event is about the opponent.
                f.yours = Some(false);
                f.types = 0;
            }
            if matches!(kind, Ev::Cast | Ev::LoyaltyActivated | Ev::Draw)
                && what.contains("opponent")
            {
                f.yours = Some(false);
            }
            // "Whenever you cast a spell that targets a creature you control" (Colleen Wing,
            // Ms. Marvel), "an Equipment spell or a spell that targets ..." (Danitha): tricks,
            // Auras and Equipment - noncreature spells - not creature spells. A spell of a
            // named kind ("an instant or sorcery spell that targets") keeps its kind.
            if matches!(e, Event::Cast(w)
                if w.split(" that target").next().is_some_and(|h| h.ends_with("a spell"))
                    && w.contains("that targets"))
            {
                f.types = 0;
                f.not_types = T_CREATURE | T_LAND;
                f.subtype = None;
                f.restr = Default::default();
            }
            // ... and what it must target.
            if let Event::Cast(w) = e {
                if let Some(what) = w.split("that targets ").nth(1) {
                    let mut t = 0;
                    if what.contains("you control") || what.starts_with('~') {
                        t |= TGT_OWN_CREATURE;
                    }
                    if what.contains("opponent") {
                        t |= TGT_OPP_CREATURE | TGT_OPPONENT;
                    }
                    if what.contains("artifact") || what.contains("land") {
                        t |= TGT_ARTIFACT_LAND;
                    }
                    if w.starts_with("an equipment spell or") {
                        t |= TGT_OR_EQUIPMENT;
                    }
                    f.targets = t;
                }
            }
            if matches!(e, Event::Cast(_)) {
                f.yours = Some(true);
            }
            Trig::Event(kind, f)
        }
        // The parser counts each {X} as 2; the game picks X when it's activated.
        When::Activated(c) => Trig::Activated {
            mana: c.mana.saturating_sub(2 * act_cost(t).x as u32).min(255) as u8,
            tap: c.tap,
            sac_self: c.sacrifice && !c.discard,
            sac_other: c.sacrifice_other,
            discard: c.discard,
            life: if c.life {
                number_after(t, "pay").unwrap_or(2)
            } else {
                0
            },
            sorcery: t.contains("only as a sorcery"),
            extra: act_cost(t),
        },
        When::PowerUp(c) => {
            // The parser keeps only the total; read the coloured cost from the text
            // ("power-up — {c}{w}{u}{b}{r}{g}: ...").
            let cost_text = t.split(':').next().unwrap_or("");
            let m = Mana::parse(cost_text);
            Trig::PowerUp(if m.total() > 0 {
                m
            } else {
                Mana::generic(c.mana)
            })
        }
        When::Loyalty(n) => Trig::Loyalty((*n).clamp(-20, 20) as i8),
        When::Chapter(n) => Trig::Chapter((*n).min(10) as u8),
        When::Reflexive => Trig::Reflexive,
        When::Static => Trig::Static,
    }
}

/// Card types named in a cost ("a creature or planeswalker", "another creature or artifact").
fn types_in(phrase: &str) -> u16 {
    let mut bits = 0;
    for (word, bit) in [
        ("creature", T_CREATURE),
        ("artifact", T_ARTIFACT),
        ("enchantment", T_ENCHANTMENT),
        ("planeswalker", T_PLANESWALKER),
        ("land", T_LAND),
    ] {
        if phrase.contains(word) && !phrase.contains(&format!("non{}", word)) {
            bits |= bit;
        }
    }
    if bits == 0 {
        // "a permanent", "a nonland permanent"
        bits = if phrase.contains("nonland") {
            0xFFFF & !T_LAND
        } else {
            0xFFFF
        };
    }
    bits
}

/// The parts of an activated ability's cost beyond mana, {T}, sacrifices, discards and life.
fn act_cost(text: &str) -> ActCost {
    let cost = text.split(": ").next().unwrap_or("");
    let mut a = ActCost {
        x: crate::card::symbols_in(cost)
            .iter()
            .filter(|s| s.eq_ignore_ascii_case("x"))
            .count() as u8,
        boast: text.contains("activate only if ~ attacked this turn"),
        ..Default::default()
    };
    for part in cost.split(", ") {
        let p = part.trim();
        let count = |rest: &str| {
            rest.split_whitespace()
                .next()
                .and_then(|w| {
                    if w == "another" {
                        Some(1)
                    } else {
                        number_word(w)
                    }
                })
                .unwrap_or(1)
        };
        if let Some(rest) = p.strip_prefix("remove ") {
            // "Remove three incubation counters from ~", "remove a +1/+1 counter from ~".
            a.remove = count(rest);
            a.remove_plus = rest.contains("+1/+1");
            a.remove_all = rest.starts_with("any number");
        } else if p.starts_with("put a ") && p.contains(" counter on ~") {
            a.add_counter = if p.contains("stun") { 1 } else { 2 };
        } else if let Some(rest) = p.strip_prefix("tap ").filter(|r| r.contains("untapped ")) {
            // "Tap two untapped artifacts you control", "tap ten untapped elves you control".
            a.tap_others = count(rest);
            let what = rest.split("untapped ").nth(1).unwrap_or("");
            let w = singular(what.split_whitespace().next().unwrap_or(""));
            match w.as_str() {
                "artifact" => a.tap_types = T_ARTIFACT,
                "creature" => a.tap_types = T_CREATURE,
                "land" => a.tap_types = T_LAND,
                _ => {
                    a.tap_types = T_CREATURE;
                    a.tap_subtype = Some(w);
                }
            }
        } else if p.starts_with("exile ~ from your graveyard") {
            a.from_graveyard = true;
        } else if p.starts_with("exile ~ from your hand") {
            a.from_hand = true;
        } else if p == "exile ~" {
            a.exile_self = true;
        } else if p.starts_with("return ~ to") {
            a.return_self = true;
        } else if let Some(rest) = p
            .strip_prefix("exile ")
            .filter(|_| p.contains("from your graveyard"))
        {
            if p.contains("black mana symbols") {
                a.black_pips = number_before(p, " or more black").unwrap_or(15);
            } else {
                // "Exile another creature card from your graveyard", "exile two cards".
                a.exile_cards = count(rest);
                a.exile_creatures = rest.contains("creature");
            }
        }
    }
    a
}

/// A cost paid while the ability resolves: "you may pay {2}. If you do, ...", "remove a
/// bait counter from ~. If you do, ...", "you may sacrifice another creature. When you do".
fn pay_of(text: &str) -> Option<PayCost> {
    let end = [". if you do,", ". when you do,"]
        .iter()
        .filter_map(|m| text.find(m))
        .min()?;
    // Gifts and overload have their own rules; "when the fourth plan counter is put on ~,
    // sacrifice it. When you do" is sacrificed as the counter goes on.
    if text.contains("gift") || text.contains("overload") || text.contains(" counter is put on") {
        return None;
    }
    let before = &text[..end];
    let start = before.rfind(". ").map(|i| i + 2).unwrap_or(0);
    let sentence = &before[start..];
    // A condition on the payment itself ("if it has six or more quest counters on it,
    // sacrifice it").
    let cond = sentence
        .strip_prefix("then ")
        .unwrap_or(sentence)
        .strip_prefix("if ")
        .map(|c| {
            let c = c.replace(" of more ", " or more ");
            if c.contains(" counters on it") && c.contains(" or more ") {
                Cond::OtherCountersAtLeast(number_before(&c, " or more").unwrap_or(1) as i16)
            } else {
                cond_from(&c)
            }
        });
    let (optional, phrase) = match sentence.find("you may ") {
        Some(i) => (true, sentence[i + 8..].trim()),
        None => (
            false,
            sentence
                .rsplit(", ")
                .next()
                .unwrap_or(sentence)
                .trim()
                .trim_start_matches("then "),
        ),
    };
    let cost = if phrase.starts_with("sacrifice ") {
        if phrase == "sacrifice ~" || phrase == "sacrifice it" {
            ResCost::SacrificeSelf
        } else if phrase.contains(" or discard a card") {
            ResCost::SacrificeOrDiscard(types_in(phrase.split(" or discard").next().unwrap_or("")))
        } else {
            ResCost::Sacrifice {
                types: types_in(phrase),
                other: phrase.contains("another"),
            }
        }
    } else if let Some(rest) = phrase.strip_prefix("pay ") {
        if rest.contains("life") {
            ResCost::Life(number_word(rest.split_whitespace().next().unwrap_or("")).unwrap_or(2))
        } else {
            ResCost::Mana(Mana::parse(rest))
        }
    } else if phrase.starts_with("discard a card") {
        ResCost::Discard
    } else if phrase.starts_with("remove ") && phrase.contains("counter") {
        ResCost::RemoveCounter {
            plus: phrase.contains("+1/+1"),
        }
    } else if phrase.starts_with("exile ~ and") && phrase.contains("named ~") {
        ResCost::ExileNamed(number_before(phrase, " cards named").unwrap_or(4))
    } else if phrase == "exile it" || phrase == "exile ~" {
        ResCost::ExileSelf
    } else if optional {
        ResCost::Free
    } else {
        return None;
    };
    let skip_from = abilities::effects_count(&text[..start]).min(255) as u8;
    let first = if cost == ResCost::Free {
        skip_from
    } else {
        abilities::effects_count(before).min(255) as u8
    };
    Some(PayCost {
        cost,
        optional,
        cond,
        skip_from,
        first: first.max(skip_from),
    })
}

/// "Return target creature card from your graveyard to your hand. If it's a Zombie card,
/// draw a card." / "Reveal the top card of your library. If it's an artifact card, draw a
/// card." / "Destroy target artifact or enchantment. If that permanent was a legendary
/// enchantment, draw a card."
fn gate_of(text: &str) -> Option<Gate> {
    // "Put a +1/+1 counter on him. If he's attacking, untap him and there is an additional
    // combat phase": only that part needs it.
    if let Some(start) = [
        ". if he's attacking",
        ". if it's attacking",
        ". if she's attacking",
        ". if that spell's mana value",
    ]
    .iter()
    .filter_map(|k| text.find(k))
    .min()
    {
        let start = start + 2;
        let sentence = text[start..].split(". ").next().unwrap_or("");
        let through = format!("{}{}.", &text[..start], sentence);
        return Some(Gate {
            from: abilities::effects_count(&text[..start]).min(255) as u8,
            to: abilities::effects_count(&through).min(255) as u8,
            types: 0,
            subtype: None,
            top: false,
            cond: Some(if sentence.contains("attacking") {
                Cond::SourceAttacking
            } else {
                cond_from(sentence)
            }),
        });
    }
    // "Untap another target creature you control. If that creature is a Bear, put a +1/+1
    // counter on it": the creature it chose.
    if let Some(start) = text.find(". if that creature is ") {
        let start = start + 2;
        let sentence = text[start..].split(". ").next().unwrap_or("");
        let what = sentence["if that creature is ".len()..]
            .split(',')
            .next()
            .unwrap_or("")
            .trim_start_matches("an ")
            .trim_start_matches("a ")
            .to_string();
        let through = format!("{}{}.", &text[..start], sentence);
        return Some(Gate {
            from: abilities::effects_count(&text[..start]).min(255) as u8,
            to: abilities::effects_count(&through).min(255) as u8,
            types: 0,
            subtype: None,
            top: false,
            cond: Some(Cond::TargetIs(what)),
        });
    }
    card_gate(text)
}

/// The card-kind form of `gate_of`.
fn card_gate(text: &str) -> Option<Gate> {
    let start = [
        ". if it's a",
        ". if that card is a",
        ". if that permanent was a",
    ]
    .iter()
    .filter_map(|k| text.find(k))
    .min()?
        + 2;
    let before = &text[..start];
    let top = before.contains("top card of your library");
    if !(top
        || before.contains("destroy target")
        || before.contains("return target") && before.contains("to your hand"))
    {
        return None;
    }
    let sentence = text[start..].split(". ").next().unwrap_or("");
    let (cond, _) = sentence.split_once(", ")?;
    let mut types = 0;
    let mut subtype = None;
    for w in cond
        .split_whitespace()
        .skip_while(|w| *w != "a" && *w != "an")
        .skip(1)
    {
        match w {
            "creature" => types |= T_CREATURE,
            "artifact" => types |= T_ARTIFACT,
            "enchantment" => types |= T_ENCHANTMENT,
            "land" => types |= T_LAND,
            "planeswalker" => types |= T_PLANESWALKER,
            "instant" => types |= T_INSTANT,
            "sorcery" => types |= T_SORCERY,
            "legendary" => types |= T_LEGENDARY,
            "card" | "permanent" => {}
            w => subtype = Some(w.to_string()),
        }
    }
    let through = format!("{}{}.", &text[..start], sentence);
    Some(Gate {
        from: abilities::effects_count(before).min(255) as u8,
        to: abilities::effects_count(&through).min(255) as u8,
        types,
        subtype,
        top,
        cond: None,
    })
}

/// "As an additional cost to cast this spell, sacrifice a creature or pay {3}{B}."
fn extra_cost_of(abilities: &[SimAbility]) -> Option<ExtraCost> {
    let key = "as an additional cost to cast ~, ";
    let t = abilities
        .iter()
        .find(|a| a.text.contains(key))?
        .text
        .as_str();
    let rest = t.split(key).nth(1)?;
    let rest = rest.split('.').next().unwrap_or(rest);
    let (what, pay) = match rest.split_once(" or pay ") {
        Some((w, m)) => (w, Some(Mana::parse(m))),
        None => (rest, None),
    };
    Some(if what.starts_with("sacrifice ") {
        match pay {
            Some(m) => ExtraCost::SacrificeOrPay(types_in(what), m),
            None => ExtraCost::Sacrifice(types_in(what)),
        }
    } else if what.starts_with("discard a card") {
        match pay {
            Some(m) => ExtraCost::DiscardOrPay(m),
            None => ExtraCost::Discard,
        }
    } else if let Some(sub) = what
        .strip_prefix("behold an ")
        .or_else(|| what.strip_prefix("behold a "))
    {
        ExtraCost::BeholdOrPay(sub.trim().to_string(), pay.unwrap_or_default())
    } else {
        return None;
    })
}

/// "Ward {2}", "Ward—Pay 2 life", "Ward—{3}, Pay 3 life", "Ward—Discard a card or pay {2}".
fn ward_of(words: &[String]) -> (u8, WardCost) {
    let Some(k) = words.iter().find(|k| k.starts_with("ward")) else {
        return (0, WardCost::default());
    };
    let rest = k["ward".len()..].trim_start_matches(['—', '-', ' ']);
    let mana = Mana::parse(rest).total().min(255) as u8;
    let mut w = WardCost::default();
    if rest.contains(" life") {
        w.life = number_after(rest, "pay").unwrap_or(2);
    }
    if rest.contains("discard") {
        w.discard = true;
        w.discard_or_mana = rest.contains(" or pay");
    }
    if let Some(n) = word_after(rest, "sacrifice ").and_then(number_word) {
        w.sacrifice = n;
    }
    if rest.contains("poison") {
        w.poison = word_after(rest, "get ").and_then(number_word).unwrap_or(3);
    }
    if mana == 0 && w == WardCost::default() {
        return (2, w);
    }
    (mana, w)
}

/// Undoes the parser's "the first card drawn only replaces the one paid" reading where the
/// simulator pays the discard itself: the draw is the full number again.
fn redraw(effects: &mut [Effect]) {
    if let Some(e) = effects
        .iter_mut()
        .find(|e| matches!(e, Effect::Loot) || matches!(e, Effect::Draw { you: true, .. }))
    {
        *e = match e {
            Effect::Draw { amount, .. } => Effect::Draw {
                amount: abilities::Amount {
                    n: amount.n + 1,
                    x: amount.x,
                },
                you: true,
            },
            _ => Effect::Draw {
                amount: abilities::Amount::fixed(1),
                you: true,
            },
        };
    }
}

/// "When the third plan counter is put on ~": the counter count a reflexive ability waits for.
fn counter_goal(text: &str) -> Option<i16> {
    let i = text.find(" counter is put")?;
    let before: Vec<&str> = text[..i].split_whitespace().collect();
    before
        .iter()
        .rev()
        .take(3)
        .find_map(|w| number_word(w).filter(|&n| n > 1))
        .map(|n| n as i16)
}

fn compile_abilities(parsed: &abilities::Parsed) -> Vec<SimAbility> {
    let mut out = Vec::new();
    let mut instead: Vec<bool> = Vec::new();
    for a in &parsed.abilities {
        let mut cond = cond_of(&a.text, a.conditional);
        // "~ can't attack unless you control two or more other Wolves": the condition under
        // which it may attack (see `Game::can_attack`).
        if a.when.iter().all(|w| *w == When::Static)
            && let Some(rest) = a.text.split("~ can't attack unless ").nth(1)
        {
            cond = cond_from(rest);
        }
        let whens: Vec<&When> = if a.when.is_empty() {
            vec![&When::Static]
        } else {
            a.when.iter().collect()
        };
        let pay = if a.modal { None } else { pay_of(&a.text) };
        for w in whens {
            let trig = trig_of(w, a);
            let mut effects = a.effects.clone();
            // Discards the simulator pays itself: the parser's "draw one fewer" undone.
            let body = a.text.split(": ").nth(1).unwrap_or("");
            let pays_discard = matches!(trig, Trig::Activated { discard: true, .. })
                && !body.contains("discard")
                && !body.contains("connive")
                || pay.as_ref().is_some_and(|p| {
                    matches!(p.cost, ResCost::Discard | ResCost::SacrificeOrDiscard(_))
                        && a.text.contains("if you do, draw")
                });
            if pays_discard {
                redraw(&mut effects);
            }
            let event = matches!(trig, Trig::Event(..));
            // A trigger about another creature: "it"/"that creature" may be that one (not
            // when the trigger is about the card itself: "whenever ~ is dealt damage, put a
            // counter on him").
            if event && !trigger_part(&a.text).contains('~') {
                for e in effects.iter_mut() {
                    for t in e.targets_mut() {
                        if t.restr.has(crate::restrict::PREV) {
                            t.restr.flags |= crate::restrict::SUBJECT;
                        }
                    }
                }
            }
            out.push(SimAbility {
                trig,
                cond: cond.clone(),
                optional: a.optional,
                once: a.once,
                modal: a.modal,
                modes: a.modes.clone(),
                mode_texts: a
                    .mode_texts
                    .iter()
                    .map(|m| format!("{} — {}", a.text, m))
                    .collect(),
                mode_restr: a
                    .mode_texts
                    .iter()
                    .map(|m| crate::restrict::object_restr(m))
                    .collect(),
                effects,
                text: a.text.clone(),
                until_eot: a.text.contains("until end of turn"),
                counter_goal: if *w == When::Reflexive {
                    counter_goal(&a.text)
                } else {
                    None
                },
                filter: filter_of(&a.text),
                // "For the first time each turn", "your first noncreature spell each turn":
                // the first such event each turn.
                once_per_turn: a.text.contains("only once each turn")
                    || a.text.contains("for the first time each turn")
                    || a.text.contains("for the first time during each")
                    || a.text.contains(" first ") && a.text.contains(" each turn"),
                second_draw: a.text.contains("second card") || a.text.contains("second spell"),
                from_graveyard: graveyard_ability(&a.text)
                    && !matches!(w, When::Dies | When::Spell | When::Static),
                pay: pay.clone(),
                gate: gate_of(&a.text),
                card_restr: crate::restrict::object_restr(&a.text),
            });
            instead.push(a.instead);
        }
    }
    // A reflexive "when you do" without its own counter goal belongs to the counters of the
    // ability before it ("put a plan counter ... when the third is put, sacrifice it. When you
    // do, ..."): take the goal from the card's other text.
    let goal = parsed.abilities.iter().find_map(|a| counter_goal(&a.text));
    for a in out.iter_mut() {
        if a.trig == Trig::Reflexive && a.counter_goal.is_none() {
            a.counter_goal = goal;
        }
    }
    // "Put a +1/+1 counter on target creature. If that creature is another Hero, put two
    // +1/+1 counters on it instead": one ability, the extra counter only for a Hero.
    for i in (1..out.len()).rev() {
        let what = out[i]
            .text
            .split("if that creature is ")
            .nth(1)
            .and_then(|r| r.split(',').next())
            .map(|w| {
                w.trim_start_matches("an ")
                    .trim_start_matches("a ")
                    .to_string()
            });
        let (Some(what), true) = (what, instead[i] && out[i].trig == out[i - 1].trig) else {
            continue;
        };
        let (
            [Effect::PlusCounters { amount: a, .. }],
            [
                Effect::PlusCounters {
                    amount: b, to: bt, ..
                },
            ],
        ) = (&out[i - 1].effects[..], &out[i].effects[..])
        else {
            continue;
        };
        let extra = Effect::PlusCounters {
            amount: abilities::Amount::fixed(b.n - a.n),
            to: *bt,
        };
        let alt = out.remove(i);
        instead.remove(i);
        let base = &mut out[i - 1];
        base.effects.push(extra);
        base.cond = Cond::Always;
        base.gate = Some(Gate {
            from: 1,
            to: 2,
            types: 0,
            subtype: None,
            top: false,
            cond: Some(Cond::TargetIs(what)),
        });
        let _ = alt;
    }
    // Alternatives the card reader split in two: only one of them happens.
    for i in 1..out.len() {
        if out[i].trig != out[i - 1].trig {
            continue;
        }
        if out[i].text.contains("if not, ") {
            // "X if <c>. Otherwise, Y": Y when the condition fails.
            out[i].cond = Cond::Not(Box::new(out[i - 1].cond.clone()));
        } else if (instead[i] || out[i].text.replace("instead of", "").contains("instead"))
            && out[i].cond != Cond::Always
        {
            // "X. If <c>, Y instead": X only when the condition fails.
            out[i - 1].cond = Cond::Not(Box::new(out[i].cond.clone()));
        }
        // "Create a token that's a copy of target creature. If this spell was kicked, create
        // five of those tokens instead": "those tokens" are the copies the first part makes.
        if instead[i] && out[i].text.contains("of those tokens") {
            let base = out[i - 1].text.clone();
            let alt = out[i].text.clone();
            out[i].text = format!("{} {}", base, alt);
        }
    }
    // "II — This Saga gains 'Landfall — Whenever a land you control enters, create a 1/1
    // Elf'", "{1}{R}: If ~ is a Scout, it becomes a Human Faerie Detective and gains
    // 'Whenever ~ deals combat damage to a player, ...'": the granted trigger is the card's
    // own from then on (from that chapter / while it's that type), not an effect now.
    let mut granted = Vec::new();
    for a in out.iter_mut() {
        let Some((head, rest)) = a.text.split_once("gains \"") else {
            continue;
        };
        let inner = rest.rsplit_once('"').map_or(rest, |x| x.0);
        let body = inner.split(" — ").last().unwrap_or(inner);
        if !(body.starts_with("when") || body.starts_with("at the beginning")) {
            continue;
        }
        let from = match a.trig {
            Trig::Chapter(n) if head.contains("~ ") => Cond::OtherCountersAtLeast(n as i16),
            _ => match head.split("becomes a ").nth(1) {
                Some(kind) if head.contains("if ~ is a ") => {
                    let kind = kind.split(" and").next().unwrap_or(kind);
                    match kind.split_whitespace().last() {
                        Some(w) => Cond::SourceSubtype(w.to_string()),
                        None => continue,
                    }
                }
                _ => continue,
            },
        };
        let card = Card {
            name: "\u{1}".into(),
            type_line: "Creature".into(),
            oracle_text: inner.to_string(),
            ..Default::default()
        };
        for mut g in compile_abilities(&abilities::parse(&card)) {
            g.cond = from.clone();
            granted.push(g);
        }
        a.effects = vec![Effect::Rule("grants an ability".into())];
    }
    out.extend(granted);
    // "Then if you control four or more Treasures, sacrifice ~. If you do, create a Dragon":
    // the condition is the payment's, not the whole ability's ("create a Treasure" always
    // happens).
    for a in out.iter_mut() {
        if a.pay
            .as_ref()
            .is_some_and(|p| p.cond.as_ref() == Some(&a.cond) && p.skip_from > 0)
        {
            a.cond = Cond::Always;
        }
    }
    out
}

/// The cost after a keyword ("flashback {2}{b}{b}", "kicker—{1}{w}").
fn keyword_cost(words: &[String], key: &str) -> Option<Mana> {
    let k = words.iter().find(|w| w.starts_with(key))?;
    let rest = k[key.len()..].trim_start_matches(['—', ' ']);
    let cost = rest.split(',').next().unwrap_or(rest);
    Some(Mana::parse(cost))
}

/// Keyword bits in a list of keyword words.
pub fn keyword_bits(words: &[String]) -> u32 {
    let mut bits = 0;
    for w in words {
        for (k, bit) in KEYWORDS {
            if w == k || w.starts_with(&format!("{} ", k)) && k != "flash" {
                bits |= bit;
            }
        }
        if w.starts_with("ward") || w.starts_with("hexproof") {
            bits |= HEXPROOF * u32::from(w.starts_with("hexproof"));
        }
    }
    bits
}

fn color_bits(text: &str) -> u8 {
    let mut m = 0;
    for (i, c) in ["white", "blue", "black", "red", "green"]
        .iter()
        .enumerate()
    {
        if text.contains(c) {
            m |= 1 << i;
        }
    }
    if text.contains("everything") || text.contains("all colors") {
        m = 0b11111;
    }
    m
}

/// "This land enters tapped unless you control a planeswalker": what keeps it untapped.
fn enters_tapped_unless(text: &str) -> Option<Cond> {
    let i = text.find("enters tapped unless you control ")?;
    let what = &text[i + "enters tapped unless you control ".len()..];
    Some(if what.starts_with("two or more other lands") {
        // Counted with itself already on the battlefield.
        Cond::ControlLands(3)
    } else if what.starts_with("a planeswalker") {
        Cond::HasPlaneswalker
    } else {
        let w = what
            .trim_start_matches("an ")
            .trim_start_matches("a ")
            .split([' ', '.', ','])
            .next()
            .unwrap_or("");
        Cond::Control(w.to_string(), false)
    })
}

/// How many counters a permanent enters with.
#[derive(Debug, Clone, PartialEq)]
pub enum EntersCounters {
    Fixed(u8),
    /// The X paid for it.
    X,
    /// "Where X is the greatest power among other creatures you control" (Zegana).
    GreatestOtherPower,
    /// "A fellowship counter on it for each creature you control of the chosen type".
    Count(String),
}

/// "~ enters with four +1/+1 counters on it", "enters with X +1/+1 counters", "enters with
/// a shield counter", "if it was kicked, it enters with two +1/+1 counters": (counter kind,
/// how many, condition).
fn enters_counters(text: &str) -> Vec<(String, EntersCounters, Cond)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        let Some(i) = l.find("enters with ") else {
            continue;
        };
        // Only the card itself ("~ enters", "this creature enters", "it enters").
        let head = &l[..i];
        if !(head.ends_with("~ ") || head.ends_with("it ")) || head.contains("each other") {
            continue;
        }
        let rest = &l[i + "enters with ".len()..];
        let mut words = rest.split_whitespace();
        let n = words.next().unwrap_or("");
        let kind = words
            .next()
            .unwrap_or("")
            .trim_start_matches("additional")
            .to_string();
        let kind = if kind.is_empty() {
            words.next().unwrap_or("").to_string()
        } else {
            kind
        };
        if !rest.contains(" counter") {
            continue;
        }
        let amount = if let Some(what) = rest.split("for each ").nth(1) {
            EntersCounters::Count(what.trim_end_matches('.').to_string())
        } else if n == "x" {
            if rest.contains("greatest power among other creatures") {
                EntersCounters::GreatestOtherPower
            } else {
                EntersCounters::X
            }
        } else {
            match number_word(n) {
                Some(k) => EntersCounters::Fixed(k),
                None => continue,
            }
        };
        let cond = cond_of(
            l,
            l.contains(" if ") || l.starts_with("if ") || l.contains("raid"),
        );
        out.push((kind, amount, cond));
    }
    out
}

/// Back face: "// Back face: Name {cost}\nType line\n..." in the rules text.
fn back_face(card: &Card, parsed: &abilities::Parsed) -> Option<BackFace> {
    let back = parsed.back.as_ref()?;
    let raw = card.oracle_text.split("// Back face:").nth(1)?;
    let mut lines = raw.lines();
    let head = lines.next().unwrap_or("");
    let type_line = lines.next().unwrap_or("");
    let cost = Mana::parse(head);
    let types = types_of(type_line);
    let kind = if type_line.contains("Adventure") {
        BackKind::Adventure
    } else if parsed
        .abilities
        .iter()
        .any(|a| a.effects.contains(&Effect::Prepared))
        || card.text_lower().contains("prepare")
    {
        BackKind::Prepare
    } else {
        BackKind::Other
    };
    let abilities = compile_abilities(back);
    Some(BackFace {
        kind,
        cost,
        types,
        extra_cost: extra_cost_of(&abilities),
        spell_targets: spell_targets(&abilities, types, false),
        abilities,
    })
}

/// Reads a card for the simulator.
pub fn compile(card: &Card) -> SimCard {
    let parsed = card.parsed();
    let text = card.text_lower().to_string();
    let front_text = text
        .split("// back face:")
        .next()
        .unwrap_or(&text)
        .to_string();
    let types = types_of(&card.type_line);
    let cost = Mana::parse(&card.mana_cost);
    // The front face's rules text, reminders dropped and the card's name as "~".
    let own_text = crate::abilities::self_refs(
        crate::card::strip_reminder(&card.oracle_text)
            .split("// Back face:")
            .next()
            .unwrap_or(""),
        card,
    );
    let mut colors = 0u8;
    for (i, &p) in cost.pips.iter().enumerate() {
        if p > 0 {
            colors |= 1 << i;
        }
    }
    let kw_words = &parsed.keywords;
    let mut kw = keyword_bits(kw_words);
    // A line of its own: "This creature can't block." (`own_text` has the card as "~"; a
    // condition or "this turn" makes it something else).
    let own_line = |s: &str| own_text.lines().any(|l| l.trim() == s);
    if own_line("~ can't be blocked.")
        || front_text.contains("~ can't be blocked")
        || front_text.contains("can't be blocked.") && front_text.starts_with("~")
    {
        kw |= UNBLOCKABLE;
    }
    if own_line("~ can't block.")
        || front_text.contains("~ can't block")
        || front_text.contains("can't block.") && !front_text.contains("target")
    {
        kw |= CANT_BLOCK;
    }
    // The first number after the keyword ("equip—{2}, pay 2 life" is 2, not 22).
    let kw_num = |key: &str| {
        kw_words.iter().find(|k| k.starts_with(key)).map(|k| {
            let digits: String = k
                .chars()
                .skip_while(|c| !c.is_ascii_digit())
                .take_while(|c| c.is_ascii_digit())
                .collect();
            digits.parse::<u8>().unwrap_or(2)
        })
    };
    let protection = kw_words
        .iter()
        .filter(|k| k.starts_with("protection"))
        .fold(0, |m, k| m | color_bits(k));
    let abilities = compile_abilities(parsed);
    // The front face's mana (a back face's "{T}: Add {R} or {G}" token isn't this card's).
    let front_card = Card {
        oracle_text: card
            .oracle_text
            .split("// Back face:")
            .next()
            .unwrap_or("")
            .to_string(),
        ..card.clone()
    };
    let mana_ability = abilities.iter().any(|a| {
        matches!(
            a.trig,
            Trig::Activated {
                tap: true,
                mana: 0,
                sac_self: false,
                ..
            }
        ) && a.effects.iter().any(|e| matches!(e, Effect::AddMana(_)))
    });
    let mut produces = if types & T_LAND != 0 || mana_ability {
        front_card.produces().0
    } else {
        0
    };
    if (types & T_LAND != 0 || mana_ability) && produces == 0 {
        // "A colour among..." sources: count as the card's colours.
        produces = if card.identity().0 != 0 && types & T_LAND != 0 {
            card.identity().0 & 0b11111
        } else {
            0
        };
    }
    // Sources of {C} (Thanos's power-up and other {C} costs need one). A land with no mana
    // ability at all (Evolving Wilds, Hobbit Hole) makes no mana.
    let land_mana = types & T_LAND != 0
        && (front_card.produces().0 != 0
            || front_card.makes_colorless()
            || front_text.contains("add ")
            || types & T_BASIC != 0);
    if (land_mana || mana_ability) && (front_card.makes_colorless() || produces == 0) {
        produces |= COLORLESS;
    }
    if types & T_LAND != 0 && !land_mana && !mana_ability {
        produces = 0;
    }
    // "Add one mana of the chosen color" (Uncharted Haven, Heraldic Banner): whichever colour
    // the deck needs.
    if (land_mana || mana_ability)
        && (front_text.contains("mana of the chosen color")
            || front_text.contains("choose a color. add one mana of that color"))
    {
        produces = 0b11111;
    }
    let mana_amount = abilities
        .iter()
        .filter(|a| matches!(a.trig, Trig::Activated { tap: true, .. }))
        .flat_map(|a| a.effects.iter())
        .find_map(|e| match e {
            Effect::AddMana(n) => Some(n.n.clamp(1, 4) as u8),
            _ => None,
        })
        .unwrap_or(1);
    // "Add {G} for each Elf you control", "add X mana, where X is ~'s power": counted when
    // it's tapped.
    let mana_count = abilities
        .iter()
        .filter(|a| {
            matches!(
                a.trig,
                Trig::Activated {
                    tap: true,
                    mana: 0,
                    ..
                }
            )
        })
        .filter(|a| {
            a.effects
                .iter()
                .any(|e| matches!(e, Effect::AddMana(n) if n.x))
        })
        .find_map(|a| {
            a.text
                .split("for each ")
                .nth(1)
                .or_else(|| a.text.split("where x is ").nth(1))
                .map(|r| r.trim_end_matches('.').to_string())
        });
    let harmful_aura = types & T_AURA != 0
        && abilities.iter().any(|a| {
            a.text.starts_with("enchanted creature")
                && (a.text.contains("can't attack")
                    || a.text.contains("can't block")
                    || a.text.contains("doesn't untap")
                    || a.text.contains("loses all abilities")
                    || a.text.contains("gets -"))
        });
    let cost_reduction = abilities.iter().find_map(|a| {
        if a.trig != Trig::Static {
            return None;
        }
        let i = a.text.find("cost {")?;
        // "Power-up abilities ... cost {3} less to activate", "equip abilities ... cost {2}
        // less" aren't about spells.
        if !a.text.contains("less")
            || a.text.contains("opponent")
            || a.text.contains("activate")
            || !a.text[..i].contains("spell")
        {
            return None;
        }
        let n: u8 = a.text[i + 6..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .ok()?;
        let what = a.text[..i].to_string();
        Some((n, filter_of(&what)))
    });
    let power_up_less = abilities
        .iter()
        .filter(|a| a.trig == Trig::Static && a.text.contains("power-up abilities"))
        .find_map(|a| number_after(&a.text, "cost {"))
        .unwrap_or(0);
    let self_less = if front_text.contains("costs {x} less to cast") {
        if front_text.contains("greatest toughness") {
            Some(SelfLess::GreatestToughness)
        } else if front_text.contains("greatest power") {
            Some(SelfLess::GreatestPower)
        } else if front_text.contains("total power of creatures you control with flying") {
            Some(SelfLess::TotalPowerFlying)
        } else if front_text.contains("total power") {
            Some(SelfLess::TotalPower)
        } else {
            None
        }
    } else {
        None
    };
    let (ward, ward_cost) = ward_of(kw_words);
    let chapters = abilities
        .iter()
        .filter_map(|a| match a.trig {
            Trig::Chapter(n) => Some(n),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let power = card
        .power
        .as_deref()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let toughness = card
        .toughness
        .as_deref()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    // The card database has no starting loyalty; planeswalkers start with about their
    // mana value (most 3-5 mana walkers start at 3-5).
    let loyalty = if types & T_PLANESWALKER != 0 {
        card.mana_value().clamp(3, 6) as i16
    } else {
        0
    };
    let mut sc = SimCard {
        name: card.name.clone(),
        cost,
        mv: card.mana_value().min(255) as u8,
        colors,
        types,
        subtypes: card.subtypes().iter().map(|s| s.to_lowercase()).collect(),
        power,
        toughness,
        loyalty,
        kw,
        ward,
        ward_cost,
        protection,
        toxic: kw_num("toxic").unwrap_or(0),
        produces,
        mana_amount,
        enters_tapped: front_text.contains("enters tapped") && !front_text.contains("unless"),
        enters_tapped_unless: enters_tapped_unless(&own_text),
        no_untap: own_text.contains("~ doesn't untap during your untap step"),
        behold_untapped: front_text.contains("you may behold a jace. if you don't"),
        // ("Equip {3}. This ability costs {1} less ..." isn't a keyword line.)
        equip: (types & T_EQUIPMENT != 0).then(|| {
            keyword_cost(kw_words, "equip")
                .or_else(|| number_after(&front_text, "equip {").map(|n| Mana::generic(n as u32)))
                .unwrap_or(Mana::generic(2))
        }),
        equip_life: kw_words
            .iter()
            .find(|k| k.starts_with("equip") && k.contains(" life"))
            .and_then(|k| number_after(k, "pay"))
            .unwrap_or(0),
        crew: kw_words
            .iter()
            .any(|k| k.starts_with("crew"))
            .then(|| kw_num("crew").unwrap_or(2)),
        cycling: kw_words.iter().find(|k| k.contains("cycling")).map(|k| {
            let digits: String = k.chars().filter(|c| c.is_ascii_digit()).collect();
            let kind = k.split("cycling").next().unwrap_or("").trim();
            let find = if kind.ends_with("land") || kind == "basic land" {
                "land".to_string()
            } else {
                kind.to_string()
            };
            (digits.parse().unwrap_or(2), find)
        }),
        flashback: keyword_cost(kw_words, "flashback"),
        flashback_discard: kw_words
            .iter()
            .any(|k| k.starts_with("flashback") && k.contains("discard")),
        kicker: keyword_cost(kw_words, "kicker"),
        overload: keyword_cost(kw_words, "overload"),
        sneak: keyword_cost(kw_words, "sneak"),
        teamwork: kw_words
            .iter()
            .any(|k| k.starts_with("teamwork"))
            .then(|| kw_num("teamwork").unwrap_or(3)),
        offspring: kw_words
            .iter()
            .any(|k| k.starts_with("offspring"))
            .then(|| kw_num("offspring").unwrap_or(2)),
        gift: kw_words.iter().any(|k| k.starts_with("gift")) || front_text.contains("gift a"),
        extort: kw_words.iter().any(|k| k == "extort"),
        convoke: kw_words.iter().any(|k| k == "convoke"),
        improvise: kw_words.iter().any(|k| k == "improvise"),
        affinity: kw_words.iter().any(|k| k.starts_with("affinity")),
        living_weapon: kw_words.iter().any(|k| k == "living weapon"),
        changeling: kw_words.iter().any(|k| k == "changeling"),
        cant_be_countered: own_text.contains("~ can't be countered"),
        harmful_aura,
        chapters,
        storied: kw_words.iter().any(|k| k == "storied"),
        cost_reduction,
        self_less,
        power_up_less,
        extra_cost: extra_cost_of(&abilities),
        mana_ability,
        mana_count,
        abilities,
        back: back_face(card, parsed),
        value: 0.0,
        ev_mask: 0,
        gy_ev_mask: 0,
        has_static: false,
        enters_counters: enters_counters(&own_text),
        cda: cda_of(&own_text),
        spell_targets: 0,
        spend_only: None,
        free_mask: 0,
        cast_cond: None,
        affinity_for: kw_words
            .iter()
            .find_map(|k| k.strip_prefix("affinity for "))
            .and_then(|w| crate::restrict::subtype_word(w.trim()))
            .map(|id| crate::restrict::subtype_name(id).to_string()),
        prowess: kw_words.iter().filter(|k| *k == "prowess").count().min(255) as u8,
        equip_for: kw_words.iter().find_map(|k| {
            let rest = k.strip_prefix("equip ")?;
            let i = rest.find(" {")?;
            let what = &rest[..i];
            (!what.is_empty()).then(|| (Mana::parse(&rest[i..]), what.to_string()))
        }),
    };
    // "Equip {3}" next to "Equip Wizard {1}": the plain one is for anyone; with only
    // "Equip worthy {1}" there's no plain one.
    if sc.equip_for.is_some() {
        sc.equip = kw_words
            .iter()
            .find(|k| k.starts_with("equip {") || k.starts_with("equip—"))
            .map(|k| Mana::parse(&k["equip".len()..]));
    }
    sc.cast_cond = sc
        .abilities
        .iter()
        .find_map(|a| a.text.split("you can't cast ~ unless ").nth(1))
        .map(cond_from);
    sc.spell_targets = spell_targets(&sc.abilities, sc.types, sc.harmful_aura);
    sc.spend_only = spend_only_of(&sc.abilities);
    if sc.spend_only.is_some() {
        // Its unrestricted mana: "{T}: Add {C}." next to the restricted one.
        sc.free_mask = if own_text.contains("{t}: add {c}.")
            && !own_text.contains("{t}: add {c}. this mana")
        {
            COLORLESS
        } else {
            0
        };
    }
    sc.value = static_value(&sc);
    sc.ev_mask = sc.abilities.iter().fold(0, |m, a| match &a.trig {
        Trig::Event(k, _) if !a.from_graveyard => m | ev_bit(*k),
        _ => m,
    });
    sc.gy_ev_mask = sc.abilities.iter().fold(0, |m, a| match &a.trig {
        Trig::Event(k, _) if a.from_graveyard => m | ev_bit(*k),
        _ => m,
    });
    sc.has_static = sc.abilities.iter().any(|a| a.trig == Trig::Static);
    sc
}

pub use crate::value::effect_value;

fn static_value(c: &SimCard) -> f32 {
    let mut v = 0.0;
    if c.is(T_CREATURE) {
        v += c.power.max(0) as f32 + c.toughness.max(0) as f32 * 0.5;
        v += (c.kw.count_ones() as f32) * 0.4;
    }
    if c.is(T_LAND) {
        return 1.0;
    }
    if c.is(T_PLANESWALKER) {
        v += 2.0 + c.loyalty as f32 * 0.4;
    }
    for a in &c.abilities {
        let mut ab: f32 = a.effects.iter().map(effect_value).sum();
        if a.modal {
            ab = a
                .mode_groups()
                .into_iter()
                .map(|r| a.effects[r].iter().map(effect_value).sum::<f32>())
                .fold(0.0, f32::max);
        }
        let repeat = match &a.trig {
            Trig::Phase { .. } => 3.0,
            Trig::Event(..) => 1.8,
            Trig::Attacks | Trig::CombatDamage => 1.5,
            Trig::Activated {
                sac_self: false, ..
            } => 2.0,
            Trig::Loyalty(_) => 1.2,
            Trig::Static => 1.5,
            _ => 1.0,
        };
        v += ab
            * repeat
            * if matches!(a.cond, Cond::Always) {
                1.0
            } else {
                0.6
            };
    }
    if c.mana_ability && !c.is(T_LAND) {
        v += 2.0;
    }
    if let Some(b) = &c.back {
        v += b
            .abilities
            .iter()
            .flat_map(|a| a.effects.iter())
            .map(effect_value)
            .sum::<f32>()
            * 0.8;
    }
    v.max(0.3)
}
