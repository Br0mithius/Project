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

/// A mana cost: generic plus coloured pips (WUBRG order).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Mana {
    pub generic: u8,
    pub pips: [u8; 5],
    pub x: bool,
}

impl Mana {
    pub fn total(&self) -> u32 {
        self.generic as u32 + self.pips.iter().map(|&p| p as u32).sum::<u32>()
    }

    pub fn generic(n: u32) -> Mana {
        Mana {
            generic: n.min(255) as u8,
            ..Default::default()
        }
    }

    /// Reads "{2}{U}{U}" (hybrid and Phyrexian symbols count as generic).
    pub fn parse(cost: &str) -> Mana {
        let mut m = Mana::default();
        for sym in crate::card::symbols_in(cost) {
            let s = sym.to_uppercase();
            if let Ok(n) = s.parse::<u8>() {
                m.generic = m.generic.saturating_add(n);
            } else if s == "X" {
                m.x = true;
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
    Other,
}

pub fn ev_bit(k: Ev) -> u32 {
    1 << (k as u32)
}

/// "a Villain spell", "another creature you control", "an artifact": what an event needs.
#[derive(Debug, Clone, Default, PartialEq)]
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
    },
    PowerUp(u8),
    Loyalty(i8),
    Chapter(u8),
    /// "When you do": fires from its card's counters or a choice (see `SimAbility::counter_goal`).
    Reflexive,
    Static,
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
    /// Can't be checked: happens this often.
    Chance(f32),
}

#[derive(Debug, Clone)]
pub struct SimAbility {
    pub trig: Trig,
    pub cond: Cond,
    pub optional: bool,
    pub once: bool,
    pub modal: bool,
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
    /// "Whenever you draw your second card each turn".
    pub second_draw: bool,
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
    pub ward: u8,
    /// Colours it has protection from.
    pub protection: u8,
    pub toxic: u8,
    /// Colours a land (or mana ability) makes.
    pub produces: u8,
    pub mana_amount: u8,
    pub enters_tapped: bool,
    pub equip: Option<u8>,
    pub crew: Option<u8>,
    /// Cycling cost and what it finds: "" = draws a card, "land" = a basic land, or a
    /// land type / creature type ("mountain", "halfling").
    pub cycling: Option<(u8, String)>,
    pub flashback: Option<Mana>,
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
    /// A mana ability: {T}: Add ... (rocks, dorks, Treasures-like permanents).
    pub mana_ability: bool,
    pub abilities: Vec<SimAbility>,
    pub back: Option<BackFace>,
    /// Rough worth of the card on its own (for the AI's quick estimates).
    pub value: f32,
    /// Bit per `Ev` kind its abilities listen for (so events skip cards that don't care).
    pub ev_mask: u32,
    pub has_static: bool,
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
        "thirteen" => 13,
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
    {
        f.yours = Some(true);
    } else if w.contains("opponent") {
        f.yours = Some(false);
    }
    f.other = w.contains("another") || w.contains("other ");
    f.nontoken = w.contains("nontoken");
    f.token = !f.nontoken && w.contains("token");
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
        if w.contains(&format!("non{}", word)) {
            f.not_types |= bit;
        } else if w.contains(word) {
            f.types |= bit;
        }
    }
    for sub in [
        "hero", "villain", "robot", "human", "dwarf", "elf", "goblin", "hobbit", "halfling",
        "wizard", "warrior", "soldier", "zombie", "dragon", "vampire", "beast", "spider", "mutant",
        "avenger", "inhuman", "army", "orc", "wolf", "cat", "bird", "knight", "rogue", "spy",
    ] {
        if w.contains(sub) {
            f.subtype = Some(sub.to_string());
            break;
        }
    }
    f
}

/// Reads the condition in an ability's text.
pub fn cond_of(text: &str, conditional: bool) -> Cond {
    let t = text;
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
        return Cond::Always;
    }
    // Wordings that hold whenever the ability comes up in a game.
    for always in [
        "if you cast it",
        "if you cast him",
        "if you cast her",
        "if you cast them",
        "attacks each combat if able",
        "fewer than ten cards in hand",
        "if it wasn't a",
        "if they were a creature",
        "if it's a creature",
        "if it's a planeswalker",
        "if it had a revival counter",
    ] {
        if t.contains(always) {
            return Cond::Always;
        }
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
    if t.contains("or more treasures") {
        return Cond::Treasures(number_before(t, " or more treasures").unwrap_or(4));
    }
    if t.contains("equipped") {
        return Cond::Equipped;
    }
    if t.contains("isn't prepared") {
        return Cond::NotPrepared;
    }
    if t.contains("attacked this turn") || t.contains("attacked with") {
        return Cond::AttackedThisTurn;
    }
    if t.contains("died this turn") {
        return Cond::CreatureDiedThisTurn;
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
    if t.contains("cast from a graveyard") || t.contains("cast it from your graveyard") {
        return Cond::CastFromGraveyard;
    }
    for key in [
        "if you control a",
        "as long as you control a",
        "if you control another",
        "as long as you control another",
    ] {
        if let Some(i) = t.find(key) {
            let rest = t[i + key.len()..].trim_start_matches(['n', ' ']);
            let what: String = rest.split([' ', ',', '.']).next().unwrap_or("").to_string();
            if !what.is_empty() {
                return Cond::Control(what, key.contains("another"));
            }
        }
    }
    Cond::Chance(0.5)
}

fn trig_of(w: &When, a: &abilities::Ability) -> Trig {
    let t = a.text.as_str();
    match w {
        When::Spell => Trig::Spell,
        When::Enters => Trig::Enters,
        When::Dies => Trig::Dies,
        When::Attacks => Trig::Attacks,
        When::CombatDamage => Trig::CombatDamage,
        When::EachTurn => Trig::Phase {
            phase: if t.contains("end step") {
                Phase::End
            } else if t.contains("beginning of combat") {
                Phase::Combat
            } else {
                Phase::Upkeep
            },
            any_player: t.contains("each upkeep")
                || t.contains("each end step")
                || t.contains("each player's")
                || t.contains("each opponent's"),
        },
        When::Event(e) => {
            let (kind, what) = match e {
                Event::Cast(what) => (Ev::Cast, what.as_str()),
                Event::OtherEnters(what) => (Ev::OtherEnters, what.as_str()),
                Event::Landfall => (Ev::Landfall, "a land you control"),
                Event::Death => (Ev::Death, t),
                Event::GainLife => (Ev::GainLife, "you"),
                Event::Draw => (Ev::Draw, "you"),
                Event::Discard => (Ev::Discard, t),
                Event::Damaged => (Ev::Damaged, "~"),
                Event::Counters => (Ev::Counters, t),
                Event::Sacrifice => (Ev::Sacrifice, t),
                Event::Tokens => (Ev::Tokens, "you"),
                Event::Targeted => (Ev::Targeted, "~"),
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
            if matches!(
                kind,
                Ev::Cast
                    | Ev::GainLife
                    | Ev::Draw
                    | Ev::Tokens
                    | Ev::OthersAttack
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
            if matches!(kind, Ev::Cast | Ev::LoyaltyActivated) && what.contains("opponent") {
                f.yours = Some(false);
            }
            Trig::Event(kind, f)
        }
        When::Activated(c) => Trig::Activated {
            mana: c.mana.min(255) as u8,
            tap: c.tap,
            sac_self: c.sacrifice && (t.contains("sacrifice ~") || t.contains("sacrifice this")),
            sac_other: c.sacrifice && !(t.contains("sacrifice ~") || t.contains("sacrifice this")),
            discard: c.discard,
            life: if c.life {
                number_after(t, "pay").unwrap_or(2)
            } else {
                0
            },
            sorcery: t.contains("only as a sorcery"),
        },
        When::PowerUp(c) => Trig::PowerUp(c.mana.min(255) as u8),
        When::Loyalty(n) => Trig::Loyalty((*n).clamp(-20, 20) as i8),
        When::Chapter(n) => Trig::Chapter((*n).min(10) as u8),
        When::Reflexive => Trig::Reflexive,
        When::Static => Trig::Static,
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
    for a in &parsed.abilities {
        let cond = cond_of(&a.text, a.conditional);
        let whens: Vec<&When> = if a.when.is_empty() {
            vec![&When::Static]
        } else {
            a.when.iter().collect()
        };
        for w in whens {
            out.push(SimAbility {
                trig: trig_of(w, a),
                cond: cond.clone(),
                optional: a.optional,
                once: a.once,
                modal: a.modal,
                effects: a.effects.clone(),
                text: a.text.clone(),
                until_eot: a.text.contains("until end of turn"),
                counter_goal: if *w == When::Reflexive {
                    counter_goal(&a.text)
                } else {
                    None
                },
                filter: filter_of(&a.text),
                once_per_turn: a.text.contains("only once each turn"),
                second_draw: a.text.contains("second card"),
            });
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
    Some(BackFace {
        kind,
        cost,
        types,
        abilities: compile_abilities(back),
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
    let mut colors = 0u8;
    for (i, &p) in cost.pips.iter().enumerate() {
        if p > 0 {
            colors |= 1 << i;
        }
    }
    let kw_words = &parsed.keywords;
    let mut kw = keyword_bits(kw_words);
    if front_text.contains("~ can't be blocked")
        || front_text.contains("can't be blocked.") && front_text.starts_with("~")
    {
        kw |= UNBLOCKABLE;
    }
    if front_text.contains("~ can't block")
        || front_text.contains("can't block.") && !front_text.contains("target")
    {
        kw |= CANT_BLOCK;
    }
    let kw_num = |key: &str| {
        kw_words.iter().find(|k| k.starts_with(key)).map(|k| {
            let digits: String = k.chars().filter(|c| c.is_ascii_digit()).collect();
            digits.parse::<u8>().unwrap_or(2)
        })
    };
    let protection = kw_words
        .iter()
        .filter(|k| k.starts_with("protection"))
        .fold(0, |m, k| m | color_bits(k));
    let abilities = compile_abilities(parsed);
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
        card.produces().0
    } else {
        0
    };
    if (types & T_LAND != 0 || mana_ability) && produces == 0 {
        // Colourless or "a colour among..." sources: count as the card's colours, else generic.
        produces = if card.identity().0 != 0 && types & T_LAND != 0 {
            card.identity().0 & 0b11111
        } else {
            0
        };
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
        if !a.text.contains("less") || a.text.contains("opponent") {
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
        ward: if kw_words.iter().any(|k| k.starts_with("ward")) {
            kw_num("ward").unwrap_or(2)
        } else {
            0
        },
        protection,
        toxic: kw_num("toxic").unwrap_or(0),
        produces,
        mana_amount,
        enters_tapped: front_text.contains("enters tapped") && !front_text.contains("unless"),
        equip: (types & T_EQUIPMENT != 0).then(|| kw_num("equip").unwrap_or(2)),
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
        cant_be_countered: front_text.contains("can't be countered"),
        harmful_aura,
        chapters,
        storied: kw_words.iter().any(|k| k == "storied"),
        cost_reduction,
        mana_ability,
        abilities,
        back: back_face(card, parsed),
        value: 0.0,
        ev_mask: 0,
        has_static: false,
    };
    sc.value = static_value(&sc);
    sc.ev_mask = sc.abilities.iter().fold(0, |m, a| match &a.trig {
        Trig::Event(k, _) => m | ev_bit(*k),
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
            ab = a.effects.iter().map(effect_value).fold(0.0, f32::max);
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
