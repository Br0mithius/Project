//! Reads a card's rules text into structured abilities, following the Comprehensive Rules'
//! ability types: keywords (702), static abilities (604), triggered abilities - "when",
//! "whenever", "at" (603), activated abilities - "[cost]: [effect]" (602), loyalty
//! abilities, spell effects, Saga chapters, and the league sets' own mechanics (Power-up,
//! Teamwork, Empower, prepared, recruit, connive, amass, Storied...).
//!
//! Every ability records *when* it happens, *what* it does (destroy, damage, draw, tokens,
//! counters...) and *what it can hit*, so the rest of the app can ask precise questions:
//! is this real removal or a conditional one, does it repeat every turn, does it only hit
//! your own creatures, is it a once-only Power-up? Lines it can't read are kept in
//! `unparsed` so they can be reported and improved.

use crate::card::Card;
use regex::Regex;
use std::sync::OnceLock;

/// A number in rules text. `x` = the amount depends on the game (an estimate is used).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Amount {
    pub n: i32,
    pub x: bool,
}

impl Amount {
    pub fn fixed(n: i32) -> Amount {
        Amount { n, x: false }
    }

    /// Typical value: X (or a count of things on the board) counts as 3.
    pub fn value(&self) -> f32 {
        self.n as f32
    }
}

/// A typical X, and a typical count for "for each creature you control"-style amounts.
const TYPICAL_X: i32 = 3;

fn number(word: &str) -> Option<Amount> {
    let w = word.trim_matches(|c: char| !c.is_alphanumeric());
    let n = match w {
        "a" | "an" | "one" | "another" => 1,
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        "eight" => 8,
        "nine" => 9,
        "ten" => 10,
        "x" | "that" | "many" | "that many" => {
            return Some(Amount {
                n: TYPICAL_X,
                x: true,
            });
        }
        d => d.parse().ok()?,
    };
    Some(Amount::fixed(n))
}

/// What kind of thing an effect can hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Creature,
    /// "creature or planeswalker"
    CreatureOrPlaneswalker,
    NonlandPermanent,
    Permanent,
    Artifact,
    Enchantment,
    ArtifactOrEnchantment,
    Planeswalker,
    Land,
    /// "any target": a creature, planeswalker or player.
    AnyTarget,
    Player,
    Spell,
    /// The card itself ("~").
    It,
    Other,
}

/// Whose things an effect can hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Whose {
    Any,
    /// An opponent's ("an opponent controls", "you don't control").
    Theirs,
    /// Your own ("you control").
    Yours,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Target {
    pub kind: Kind,
    pub whose: Whose,
    /// "each" / "all": a mass effect.
    pub mass: bool,
    /// Excludes the card itself ("other", "another").
    pub other: bool,
    /// Narrowed by a condition: mana value, power, tapped, attacking, flying...
    pub restricted: bool,
    /// "up to N" or "N target".
    pub count: u32,
    /// Exactly what qualifies (types, mana value, power, tapped, attacking, colours...).
    pub restr: crate::restrict::Restr,
}

impl Target {
    fn it() -> Target {
        Target {
            kind: Kind::It,
            whose: Whose::Yours,
            mass: false,
            other: false,
            restricted: false,
            count: 1,
            restr: crate::restrict::Restr::default(),
        }
    }

    /// "It" / "him" / "they": the card itself unless the ability just chose something else
    /// (or its trigger is about another creature).
    fn pronoun() -> Target {
        let mut t = Target::it();
        t.restr.flags = crate::restrict::PREV;
        t
    }

    /// Can this hit an opponent's creature?
    pub fn hits_their_creatures(&self) -> bool {
        self.whose != Whose::Yours
            && matches!(
                self.kind,
                Kind::Creature
                    | Kind::CreatureOrPlaneswalker
                    | Kind::NonlandPermanent
                    | Kind::Permanent
                    | Kind::AnyTarget
            )
    }
}

/// What a phrase like "each Equipment you control" can hit (see `read_target`).
pub fn target_of(phrase: &str, mass: bool) -> Target {
    read_target(phrase, mass)
}

/// Reads "target creature an opponent controls with mana value 3 or less" and the like.
fn read_target(phrase: &str, mass: bool) -> Target {
    // "each creature except for tokens you control": the exception doesn't decide whose.
    let p = phrase.trim();
    let p = p.split(" except").next().unwrap_or(p).trim();
    let kind = if p.starts_with("creature or planeswalker")
        || p.starts_with("creature and/or planeswalker")
    {
        Kind::CreatureOrPlaneswalker
    } else if p.starts_with("any target") || p.starts_with("any other target") {
        Kind::AnyTarget
    } else if p.starts_with("nonland permanent") || p.starts_with("noncreature, nonland permanent")
    {
        Kind::NonlandPermanent
    } else if p.starts_with("noncreature artifact") {
        Kind::Artifact
    } else if p.starts_with("noncreature enchantment") {
        Kind::Enchantment
    } else if p.contains(" or creature")
        || p.contains(", or creature")
        || p.contains("or creature with")
    {
        // "artifact, enchantment, or creature with flying"
        Kind::Creature
    } else if p.starts_with("artifact or enchantment")
        || p.starts_with("artifact and/or enchantment")
    {
        Kind::ArtifactOrEnchantment
    } else if p.starts_with("equipment") {
        // "Exile up to one target Equipment attached to that creature".
        Kind::Artifact
    } else if p.starts_with("permanent") || p.starts_with("noncreature permanent") {
        Kind::Permanent
    } else if p.starts_with("artifact") {
        Kind::Artifact
    } else if p.starts_with("enchantment") {
        Kind::Enchantment
    } else if p.starts_with("planeswalker") {
        Kind::Planeswalker
    } else if p.starts_with("land") || p.starts_with("nonbasic land") {
        Kind::Land
    } else if p.starts_with("player") || p.starts_with("opponent") {
        Kind::Player
    } else if p.contains("spell") && !p.contains("creature") {
        Kind::Spell
    } else if p.replace("noncreature", "").contains("creature") {
        Kind::Creature
    } else if p.starts_with('~') || p.starts_with("it ") || p == "it" {
        Kind::It
    } else {
        Kind::Other
    };
    let whose = if p.contains("an opponent controls")
        || p.contains("you don't control")
        || p.contains("your opponents control")
        || p.contains("opponents control")
        || p.starts_with("opponent")
        || p.contains("defending player controls")
        || p.contains("that player controls")
    {
        Whose::Theirs
    } else if p.contains("you control") || p.contains("you own") {
        Whose::Yours
    } else {
        Whose::Any
    };
    let restricted = [
        "mana value",
        "power ",
        "toughness",
        "tapped",
        "untapped",
        "attacking",
        "blocking",
        "with flying",
        "with defender",
        "with a counter",
        "token",
        "that's",
        "that attacked",
        "that blocked",
        "that was dealt",
        "dealt damage",
        "legendary",
        "multicolored",
        "monocolored",
        "colorless",
        "white",
        "blue",
        "black",
        "red",
        "green",
    ]
    .iter()
    .any(|r| p.contains(r) && !p.starts_with("nontoken"));
    Target {
        kind,
        whose,
        mass,
        other: false,
        restricted,
        count: 1,
        restr: crate::restrict::Restr::read(p),
    }
}

/// What an ability does.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Destroy(Target),
    /// `until`: comes back when the card leaves ("until ~ leaves the battlefield").
    Exile {
        target: Target,
        until: bool,
    },
    Damage {
        amount: Amount,
        to: Target,
    },
    /// One of your creatures fights the target.
    Fight(Target),
    /// -N/-N.
    Shrink {
        amount: Amount,
        to: Target,
        until_eot: bool,
    },
    Bounce(Target),
    /// Tap it, keep it tapped, or stop it attacking/blocking / strip its abilities.
    Lock {
        target: Target,
        lasting: bool,
    },
    /// Opponents (or a player) sacrifice a creature of their choice.
    Edict {
        each_opponent: bool,
    },
    CounterSpell,
    Draw {
        amount: Amount,
        you: bool,
    },
    /// Draw then discard (loot, connive, recruit): filtering, not card advantage.
    Loot,
    Discard {
        opponents: bool,
    },
    Mill {
        amount: Amount,
    },
    Scry(Amount),
    Tokens {
        amount: Amount,
        power: i32,
        toughness: i32,
        keywords: Vec<String>,
    },
    /// Treasure, Food, Clue, Map... tokens.
    ItemTokens {
        amount: Amount,
        kind: String,
    },
    PlusCounters {
        amount: Amount,
        to: Target,
    },
    Pump {
        power: i32,
        toughness: i32,
        to: Target,
        until_eot: bool,
    },
    /// Gives keywords (flying, indestructible, hexproof...).
    Grant {
        keywords: Vec<String>,
        to: Target,
        until_eot: bool,
    },
    GainLife(Amount),
    /// Opponents (each, or a target one) lose life or are dealt damage.
    Drain {
        amount: Amount,
        each: bool,
    },
    AddMana(Amount),
    /// Searches for a land: `battlefield` = ramp, otherwise fixing.
    FetchLand {
        battlefield: bool,
    },
    Reanimate,
    Regrowth,
    Untap(Target),
    GainControl(Target),
    Copy,
    /// FRA: put loyalty on your Jace token (its abilities surveil or draw).
    Empower(Amount),
    /// HOB: grow your Army token.
    Amass(Amount),
    Goad,
    Proliferate,
    ExtraCombat,
    /// Flicker: exile and return.
    Blink,
    /// Put a card from your library into your hand.
    Tutor,
    /// Cast a spell for free / from exile / copies.
    FreeSpell,
    /// FRA "prepared": cast a copy of the card's back-face spell.
    Prepared,
    /// Attach an Equipment or Aura.
    Attach,
    /// Doubles tokens, counters or damage (replacement effects).
    Doubler(String),
    /// Prevents combat damage (a Fog).
    Fog,
    /// Look at / reveal the top cards and take one: card selection close to a draw.
    Dig,
    /// Counters other than +1/+1 (loyalty, shield, charge, plan, hone...).
    OtherCounters(String),
    /// Cast a card from your graveyard (flashback granted, "you may cast ... from your graveyard").
    Recast,
    /// A recognised special rule the scoring doesn't model in numbers (replacement effects,
    /// restrictions, control swaps...) - read and named, so nothing is silently ignored.
    Rule(String),
}

/// When an ability happens.
#[derive(Debug, Clone, PartialEq)]
pub enum When {
    /// An instant or sorcery resolving.
    Spell,
    /// "When ~ enters".
    Enters,
    Dies,
    /// "Whenever ~ attacks" (and "whenever you attack").
    Attacks,
    /// "Whenever ~ deals combat damage to a player".
    CombatDamage,
    /// "At the beginning of ..." (upkeep, end step, combat).
    EachTurn,
    /// "Whenever <something else happens>" - see `Event`.
    Event(Event),
    /// "[cost]: effect".
    Activated(Cost),
    /// MSH Power-up: an activated ability usable once (cheaper the turn it entered).
    PowerUp(Cost),
    /// Planeswalker loyalty ability.
    Loyalty(i32),
    /// Saga chapter.
    Chapter(u32),
    /// "When you do, ..." after a choice or a counter milestone.
    Reflexive,
    /// Continuous effect (anthems, Equipment and Aura bonuses, "can't be blocked"...).
    Static,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// "Whenever you cast ..." (`what` = the spell description, e.g. "a villain spell").
    Cast(String),
    /// Another creature (or permanent) entering under your control.
    OtherEnters(String),
    Landfall,
    /// A creature dying (yours or any).
    Death,
    GainLife,
    /// Drawing (e.g. your second card each turn).
    Draw,
    Discard,
    /// Being dealt damage (Enrage).
    Damaged,
    /// Counters being put on things.
    Counters,
    Sacrifice,
    Tokens,
    /// Becoming the target of a spell or ability.
    Targeted,
    /// Attacking with other creatures.
    OthersAttack,
    Other(String),
}

impl When {
    /// About how many times a turn cycle this happens once it's in play (0 = once only).
    pub fn rate(&self) -> f32 {
        match self {
            When::EachTurn => 1.0,
            When::Attacks => 0.6,
            When::CombatDamage => 0.4,
            When::Event(e) => match e {
                Event::Cast(what) if what.contains("instant") || what.contains("noncreature") => {
                    0.5
                }
                Event::Cast(_) => 0.7,
                Event::OtherEnters(_) => 0.6,
                Event::Landfall => 0.8,
                Event::Death => 0.5,
                Event::OthersAttack => 0.6,
                Event::Draw => 0.4,
                _ => 0.3,
            },
            When::Activated(c) => c.repeat_rate(),
            When::Loyalty(_) => 1.0,
            _ => 0.0,
        }
    }

    pub fn repeats(&self) -> bool {
        self.rate() > 0.0
    }
}

/// The cost of an activated ability.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cost {
    /// Total mana.
    pub mana: u32,
    pub tap: bool,
    /// Sacrifices (or discards) the card itself: the ability is one-shot.
    pub sacrifice: bool,
    /// Sacrifices something else (a sacrifice outlet).
    pub sacrifice_other: bool,
    pub discard: bool,
    pub life: bool,
    /// Exile cards, remove counters, return to hand...
    pub other: bool,
}

impl Cost {
    fn read(text: &str) -> Cost {
        let mut c = Cost::default();
        for sym in crate::card::symbols_in(text) {
            match sym.as_str() {
                "T" | "t" => c.tap = true,
                "Q" | "q" => c.other = true,
                s => {
                    // X is paid each time: count a typical X of 2.
                    c.mana +=
                        s.parse::<u32>()
                            .unwrap_or(if s.eq_ignore_ascii_case("x") { 2 } else { 1 })
                }
            }
        }
        let t = text.to_lowercase();
        // Sacrificing or discarding the card itself makes the ability one-shot.
        c.sacrifice = t.contains("sacrifice ~")
            || t.contains("sacrifice this")
            || t.contains("discard ~")
            || t.contains("discard this card");
        c.sacrifice_other = t.contains("sacrifice") && !c.sacrifice;
        c.discard = t.contains("discard");
        c.life = t.contains("life");
        c.other = c.other
            || t.contains("exile")
            || t.contains("remove")
            || t.contains("return")
            || t.contains("tap ") && t.contains("untapped");
        c
    }

    /// How often an ability with this cost gets used in a turn cycle.
    fn repeat_rate(&self) -> f32 {
        if self.sacrifice {
            return 0.0; // one-shot (the source or fodder is gone)
        }
        let base = if self.tap { 0.8 } else { 0.6 };
        // Expensive abilities get used less often.
        base * (1.0 - (self.mana as f32 * 0.12).min(0.6))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ability {
    pub when: Vec<When>,
    pub effects: Vec<Effect>,
    /// "you may" (the controller chooses).
    pub optional: bool,
    /// Has an "if ..." condition (intervening-if or "as long as").
    pub conditional: bool,
    /// Activate only once (Power-up, "only once").
    pub once: bool,
    /// "Choose one —": the effects are alternatives.
    pub modal: bool,
    /// The "Y instead" half of "X. If <condition>, Y instead.": it replaces the ability
    /// before it when the condition holds.
    pub instead: bool,
    /// For modal abilities: how many of `effects` each mode has, in order ("You lose 2
    /// life. Destroy target creature." is one mode of two effects). Empty when every mode
    /// is one effect.
    pub modes: Vec<u8>,
    /// For a modal ability: each mode's own text (the bullet), for the details its effects
    /// read ("any target", "two cards", "equal to ...").
    pub mode_texts: Vec<String>,
    pub text: String,
}

/// Which mode effect `k` belongs to, given how many effects each mode has (`modes`; empty:
/// one effect per mode).
pub fn mode_of(modes: &[u8], k: usize) -> usize {
    let mut end = 0;
    for (m, &n) in modes.iter().enumerate() {
        end += n as usize;
        if k < end {
            return m;
        }
    }
    if modes.is_empty() { k } else { modes.len() }
}

impl Ability {
    pub fn is(&self, w: &When) -> bool {
        self.when.contains(w)
    }

    /// Happens once, on its own, when the card is cast or enters (or is a one-shot spell).
    pub fn on_arrival(&self) -> bool {
        self.when
            .iter()
            .any(|w| matches!(w, When::Spell | When::Enters))
    }

    /// How many times per turn cycle it happens while the card is in play (0 = not repeating).
    pub fn rate(&self) -> f32 {
        let r = self.when.iter().map(When::rate).fold(0.0, f32::max);
        if self.once { 0.0 } else { r }
    }
}

/// A card read into structure.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Parsed {
    /// Keyword abilities, lower-case ("flying", "ward", "teamwork 4", "equip 2"...).
    pub keywords: Vec<String>,
    pub abilities: Vec<Ability>,
    /// Lines the parser couldn't make sense of.
    pub unparsed: Vec<String>,
    /// The back face's spell for "prepared" cards (and other double-faced cards).
    pub back: Option<Box<Parsed>>,
}

const KEYWORDS: [&str; 40] = [
    "flying",
    "reach",
    "trample",
    "menace",
    "deathtouch",
    "lifelink",
    "haste",
    "first strike",
    "double strike",
    "vigilance",
    "indestructible",
    "hexproof",
    "shroud",
    "defender",
    "flash",
    "prowess",
    "ward",
    "protection",
    "storied",
    "teamwork",
    "equip",
    "crew",
    "partner",
    "convoke",
    "improvise",
    "affinity",
    "extort",
    "changeling",
    "landcycling",
    "basic landcycling",
    "cycling",
    "enchant",
    "living weapon",
    "offspring",
    "flanking",
    "fear",
    "intimidate",
    "skulk",
    "toxic",
    "gift",
];

/// Ability words: flavour labels before "—" that don't change the rules (CR 207.2c).
const ABILITY_WORDS: [&str; 14] = [
    "landfall",
    "raid",
    "threshold",
    "morbid",
    "ferocious",
    "domain",
    "enrage",
    "delirium",
    "revolt",
    "metalcraft",
    "spell mastery",
    "coven",
    "formidable",
    "heroic",
];

impl Parsed {
    pub fn has_keyword(&self, kw: &str) -> bool {
        self.keywords.iter().any(|k| {
            k == kw || k.starts_with(&format!("{} ", kw)) || k.starts_with(&format!("{}—", kw))
        })
    }

    /// The number after a keyword ("teamwork 4" -> 4).
    pub fn keyword_value(&self, kw: &str) -> Option<i32> {
        self.keywords
            .iter()
            .find(|k| k.starts_with(kw))
            .and_then(|k| {
                k[kw.len()..]
                    .trim()
                    .trim_start_matches('{')
                    .trim_end_matches('}')
                    .parse()
                    .ok()
            })
    }

    /// Every effect, including the back face's (Adventure and prepare spells, transformed
    /// sides) since the card can do those too.
    pub fn effects(&self) -> impl Iterator<Item = (&Ability, &Effect)> {
        self.all_abilities()
            .flat_map(|a| a.effects.iter().map(move |e| (a, e)))
    }

    pub fn all_abilities(&self) -> impl Iterator<Item = &Ability> {
        self.abilities
            .iter()
            .chain(self.back.iter().flat_map(|b| b.abilities.iter()))
    }

    /// Share of the card's lines that were understood.
    pub fn coverage(&self) -> f32 {
        let total =
            self.abilities.len() + self.unparsed.len() + usize::from(!self.keywords.is_empty());
        if total == 0 {
            1.0
        } else {
            1.0 - self.unparsed.len() as f32 / total as f32
        }
    }
}

struct Patterns {
    destroy: Regex,
    destroy_all: Regex,
    exile: Regex,
    exile_all: Regex,
    damage: Regex,
    fight: Regex,
    shrink: Regex,
    shrink_all: Regex,
    bounce: Regex,
    bounce_all: Regex,
    tap_lock: Regex,
    edict: Regex,
    draw: Regex,
    tokens: Regex,
    tokens_plain: Regex,
    enters_with: Regex,
    minus_counters: Regex,
    other_counters: Regex,
    tuck: Regex,
    item_tokens: Regex,
    counters: Regex,
    pump: Regex,
    grant: Regex,
    gain_life: Regex,
    drain: Regex,
    add_mana: Regex,
    mill: Regex,
    scry: Regex,
    empower: Regex,
    amass: Regex,
    number_list: &'static str,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| {
        const N: &str = r"(a|an|one|two|three|four|five|six|seven|eight|nine|ten|x|that many|\d+)";
        let re = |s: &str| Regex::new(&s.replace("{N}", N)).unwrap();
        Patterns {
            destroy: re(r"destroy (?:up to {N} )?(?:(other|another) )?target ([^.;]*)"),
            destroy_all: re(r"destroy (?:all|each) ([^.;]*)"),
            exile: re(r"exile (?:up to )?(?:{N} )?(?:(other|another) )?target ([^.;]*?)(?: until ([^.;]*))?(?:\.|$|,| and )"),
            exile_all: re(r"exile (?:all|each) ([^.;]*)"),
            damage: re(r"deals? (\d+|x|that much|damage equal to [^,.]*?|damage)(?: damage)?(?: divided as you choose among| to) ([^.;]*)"),
            fight: re(r"fights? (?:up to one )?(?:(other|another) )?(target [^.;]*|it|that creature)"),
            shrink: re(r"(target [^.;]*?|enchanted creature|equipped creature) gets? -(\d+|x)/-(\d+|x)( until end of turn)?"),
            shrink_all: re(r"(?:all |each )?((?:other )?creatures(?: [a-z' ]*?)?(?:your opponents control|you don't control|an opponent controls)?|(?:all|each) [^.;]*?) gets? -(\d+|x)/-(\d+|x)"),
            bounce: re(r"return (?:up to {N} |one or two )?(?:(other|another) )?target ([^.;]*?) to (?:its|their) owners?(?:'s|s'|')? hands?"),
            bounce_all: re(r"return (?:all|each) ([^.;]*?) to (?:its|their) owners?(?:'s|s'|')? hands?"),
            tap_lock: re(r"(?:\btap (?:up to {N} |one or two )?(?:target ([^.;]*?)|(enchanted creature))(?:\.|$|,| and )|(enchanted creature|target creature[^.;]*?) (?:can't attack or block|can't block|can't attack|loses all abilities|doesn't untap))"),
            edict: re(r"(each opponent|target opponent|target player|each player|each other player) sacrifices? (?:[a-z]+|\d+) (?:nontoken )?(creatures?|nonland permanents?|permanents?|creature or planeswalker)"),
            draw: re(r"(?:(target player|each player|that player|its controller|each opponent|target opponent|two target players) )?draws? (?:a card for each|{N} cards?|cards equal to|x cards)"),
            tokens: re(r"creates? (?:([a-z' ]+?), a (?:legendary )?|{N} (?:tapped )?)(\d+|x)/(\d+|x) ([^.;]*?)tokens?(?: with ([^.;]*))?"),
            tokens_plain: re(r"creates? {N} (?:tapped )?([^.;]*?)creature tokens?(?: with ([^.;]*))?"),
            enters_with: re(r"enters with {N} \+1/\+1 counters?"),
            minus_counters: re(r"put {N} -1/-1 counters? on (?:up to (?:a|an|one|two|three|x|\d+) )?(?:target )?([^.;]*)"),
            other_counters: re(r"\b(?:a|an|one|two|three|four|five|six|seven|eight|nine|ten|x|\d+|that many) ((?:[a-z]+ )?[a-z]+) counters? on"),
            tuck: re(r"(?:the owner of (?:up to one )?(?:other )?target ([^.;]*?) (?:puts|shuffles) it|target ([^.;]*?)'s owner (?:puts|shuffles) it)"),
            item_tokens: re(r"creates? {N} (?:tapped )?(?:colorless )?(?:artifact token named )?(treasure|food|clue|map|blood|powerstone|junk|gold|incubator|heartwood|lotus|equipment)"),
            counters: re(r"put {N} \+1/\+1 counters?(?: and [^.;]*?)? on (?:up to (?:a|an|one|two|three|four|x|\d+) )?([^.;]*)"),
            pump: re(r"([^.;,]*?) gets? \+(\d+|x)/([+-](?:\d+|x))(?: and (?:gains?|has|have) ([^.;]*?))?( until end of turn)?(?:\.|$|,| for | as long| and )"),
            grant: re(r#"([^.;,]*?) (?:gains?|has|have) ([a-z ,{}0-9]*?(?:flying|indestructible|hexproof|first strike|double strike|trample|menace|lifelink|deathtouch|vigilance|haste|reach|ward|prowess|protection from [a-z]+)[a-z ,{}0-9]*?)( until end of turn)?(?:\.|$| as long| and "| and can't| and gets? [+-])"#),
            gain_life: re(r"(?:you |and |target player )gains? (\d+|x|that much) life|you gain life equal to"),
            drain: re(r"(each opponent|target opponent|target player|each player|that player|defending player|its controller) loses? (\d+|x|that much|life equal to)(?: life)?|deals? (\d+|x) damage to (each opponent|target opponent|target player or planeswalker|each player)"),
            add_mana: re(r"add (\{[^}]+\}(?:(?:, | or |)\{[^}]+\})*|[a-z]+ mana|mana)"),
            mill: re(r"mills? {N} cards?"),
            scry: re(r"(scry|surveil) (\d+|x)"),
            empower: re(r"empower (?:jace|~) (\d+|x)"),
            amass: re(r"amass(?:es)? [a-z]+ (\d+|x)"),
            number_list: N,
        }
    })
}

fn amount(s: &str, _line: &str) -> Amount {
    number(s).unwrap_or(Amount::fixed(1))
}

/// "Create a 1/1 token for each creature you control", "add {G} for each Elf": the amount
/// scales with something on the board, so a fixed "a" is really a typical count.
fn counted(a: Amount, s: &str) -> Amount {
    if a.x || a.n == 0 || !(s.contains("for each") || s.contains("where x is")) {
        return a;
    }
    // "A +1/+1 counter for each of that spell's colors" (Ramos): most spells have one colour.
    let typical = if s.contains("for each of that spell's colors") {
        1
    } else {
        TYPICAL_X
    };
    Amount {
        n: a.n * typical,
        x: true,
    }
}

/// Reads one effect sentence into effects. Returns None if nothing in it was recognised.
fn read_effects(sentence: &str) -> Vec<Effect> {
    let p = patterns();
    let full = sentence;
    let mut core = sentence.to_string();
    // "becomes a copy of ..., except he has vigilance": the exceptions describe the copy.
    if let Some(i) = core.find("becomes a copy of") {
        if let Some(j) = core[i..].find(", except") {
            core.truncate(i + j);
        }
    }
    // Abilities in quotes ("tokens with \"...\"", "equipped creature has \"...\"", "is a land
    // with \"{T}: Add {C}\"", emblems) belong to whatever gets them, not to this card.
    // Except what the card gives itself ("~ gains \"{T}: ~ deals damage ...\" until end of
    // turn", "it becomes a Bear ... and gains \"...\""): that is its own ability.
    let self_grant = ["~ gains \"", "~ has \"", "it gains \""]
        .iter()
        .any(|g| core.contains(g))
        || (core.contains("~ becomes") || core.contains("it becomes")) && core.contains('"');
    if self_grant {
        core = core.replace('"', "");
    } else if core.contains('"') {
        let mut kept = String::new();
        let mut inside = false;
        for ch in core.chars() {
            if ch == '"' {
                inside = !inside;
            } else if !inside {
                kept.push(ch);
            }
        }
        core = kept;
    }
    let s = core.as_str();
    let mut out = Vec::new();
    // A mana ability given to your own lands (New Horizons, Chromatic Lantern) is ramp.
    for grantee in [
        "enchanted land has \"{t}: add ",
        "lands you control have \"{t}: add ",
        "target land gains \"{t}: add ",
    ] {
        if let Some(i) = full.find(grantee) {
            let produced = full[i + grantee.len()..].split('"').next().unwrap_or("");
            if let Some(c) = p.add_mana.captures(&format!("add {}", produced)) {
                out.push(Effect::AddMana(mana_amount(&c[1])));
            }
        }
    }
    // Mass removal first, so "destroy all creatures" isn't also read as single-target.
    for c in p.destroy_all.captures_iter(s) {
        out.push(Effect::Destroy(read_target(&c[1], true)));
    }
    for c in p.exile_all.captures_iter(s) {
        // "Then exile all other Nightmare tokens you control" (Abyssal Harvester) only tidies
        // up the card's own tokens.
        if c[1].starts_with("other") && c[1].contains("tokens you control") {
            continue;
        }
        // "Exile all but the bottom card of each opponent's library" (Jace, Reality
        // Sculptor): libraries, not permanents.
        if c[1].contains("library") {
            out.push(Effect::Rule("exiles opponents' libraries".into()));
            continue;
        }
        if !c[1].starts_with("cards")
            && !c[1].starts_with("the cards")
            && !c[1].contains("graveyard")
            && !c[1].contains("from your hand")
        {
            out.push(Effect::Exile {
                target: read_target(&c[1], true),
                until: false,
            });
        }
    }
    for c in p.destroy.captures_iter(s) {
        let mut t = read_target(&c[3], false);
        t.other = c.get(2).is_some();
        t.count = c
            .get(1)
            .map(|m| amount(m.as_str(), s).n.max(1) as u32)
            .unwrap_or(1);
        out.push(Effect::Destroy(t));
    }
    if !s.contains("exile the top")
        && !s.contains("from your graveyard")
        && !s.contains("from a graveyard")
    {
        for c in p.exile.captures_iter(s) {
            let mut t = read_target(&c[3], false);
            t.other = c.get(2).is_some();
            t.count = c
                .get(1)
                .map(|m| amount(m.as_str(), s).n.max(1) as u32)
                .unwrap_or(1);
            if t.kind == Kind::Other && c[3].contains("card") {
                continue; // exiling cards from graveyards isn't removal
            }
            let until = c.get(4).is_some();
            if s.contains("return it")
                || s.contains("return that card")
                || s.contains("return them")
            {
                if t.whose == Whose::Yours
                    || s.contains("under your control")
                    || s.contains("under its owner")
                {
                    out.push(Effect::Blink);
                    continue;
                }
            }
            out.push(Effect::Exile { target: t, until });
        }
    }
    // "It deals 4 damage to any target and 3 damage to each of up to two other targets": two
    // separate hits.
    let ds = match s.find(" damage to ").and_then(|i| {
        let rest = &s[i..];
        let j = rest.find(" and ")?;
        let after = &rest[j + 5..];
        let n = after.split_whitespace().next()?;
        (number(n).is_some() && after[n.len()..].starts_with(" damage to "))
            .then(|| format!("{}; deals {}", &s[..i + j], after))
    }) {
        Some(d) => d,
        None => s.to_string(),
    };
    for c in p.damage.captures_iter(&ds) {
        // "If a source would deal damage to ..." is a replacement effect, not damage.
        let start = c.get(0).unwrap().start();
        if ds[..start].ends_with("would ") {
            continue;
        }
        let amt = if c[1].contains("number of +1/+1 counters") {
            // Grows over the game from small numbers.
            Amount { n: 2, x: true }
        } else if c[1].starts_with("damage equal") || c[1] == *"that much" || c[1] == *"damage" {
            Amount { n: 3, x: true }
        } else {
            counted(amount(&c[1], s), s)
        };
        let to = &c[2];
        // "~ deals 2 damage to you" (pain lands, Ancient Tomb, Sulfuric Vortex): a drawback.
        if to.starts_with("you") && !to.starts_with("your") {
            out.push(Effect::Rule("damage to you".into()));
            continue;
        }
        if to.starts_with("that creature's controller")
            || to.starts_with("its controller")
            || to.starts_with("that player")
        {
            out.push(Effect::Drain {
                amount: amt,
                each: false,
            });
            continue;
        }
        if to.contains("and each opponent") {
            out.push(Effect::Drain {
                amount: amt,
                each: true,
            });
        }
        if to.starts_with("each opponent")
            || to.starts_with("target opponent")
            || to.starts_with("each player")
            || to.starts_with("target player")
        {
            out.push(Effect::Drain {
                amount: amt,
                each: to.starts_with("each"),
            });
            if to.contains("each creature") || to.contains("and each creature") {
                out.push(Effect::Damage {
                    amount: amt,
                    to: read_target("creature", true),
                });
            }
            continue;
        }
        // "Each of up to two targets", "one or two target creatures", "divided as you choose
        // among any number of targets": several things hit.
        let to = to.trim_start_matches("each of ");
        let count = if to.starts_with("one or two")
            || to.starts_with("up to two")
            || to.starts_with("two target")
        {
            2
        } else if to.starts_with("one, two, or three")
            || to.starts_with("up to three")
            || to.starts_with("any number of")
        {
            3
        } else {
            1
        };
        let mass = to.starts_with("each ") || to.starts_with("all ");
        let to = to
            .trim_start_matches("any number of ")
            .trim_start_matches("one, two, or three ")
            .trim_start_matches("one or two ")
            .trim_start_matches("up to three ")
            .trim_start_matches("two ");
        let phrase = to
            .trim_start_matches("each ")
            .trim_start_matches("all ")
            .trim_start_matches("up to one ")
            .trim_start_matches("up to two ")
            .trim_start_matches("another ")
            .trim_start_matches("other ")
            .trim_start_matches("target ");
        let mut t = read_target(phrase, mass);
        // "Target creature you control deals damage equal to its power to that permanent"
        // (a "bite"): "that ..." refers back to the opponent's creature targeted earlier.
        if t.kind == Kind::Other && (phrase.starts_with("that ") || phrase.starts_with("it")) {
            t = read_target("creature an opponent controls", false);
            t.restr.flags |= crate::restrict::PREV;
        }
        t.other = to.contains("other") || to.contains("another");
        if to.starts_with("any target")
            || to.starts_with("any other target")
            || to.starts_with("targets")
            || to.starts_with("other targets")
            || phrase.starts_with("targets")
        {
            t.kind = Kind::AnyTarget;
        }
        t.count = count;
        out.push(Effect::Damage { amount: amt, to: t });
    }
    for c in p.fight.captures_iter(s) {
        let t = read_target(c[2].trim_start_matches("target "), false);
        out.push(Effect::Fight(t));
    }
    for c in p.shrink_all.captures_iter(s) {
        if amount(&c[3], s).n == 0 {
            continue; // -N/-0 only weakens attacks
        }
        out.push(Effect::Shrink {
            amount: amount(&c[3], s),
            to: read_target(&c[1], true),
            until_eot: s.contains("until end of turn"),
        });
    }
    for c in p.shrink.captures_iter(s) {
        let phrase = c[1].trim_start_matches("target ");
        let mut t = read_target(phrase, false);
        if c[1].starts_with("enchanted") || c[1].starts_with("equipped") {
            t = read_target("creature", false);
        }
        if amount(&c[3], s).n == 0 {
            continue;
        }
        out.push(Effect::Shrink {
            amount: amount(&c[3], s),
            to: t,
            until_eot: c.get(4).is_some(),
        });
    }
    for c in p.bounce_all.captures_iter(s) {
        out.push(Effect::Bounce(read_target(&c[1], true)));
    }
    for c in p.bounce.captures_iter(s) {
        let mut t = read_target(&c[3], false);
        t.other = c.get(2).is_some();
        t.count = if c[0].contains("one or two") {
            2
        } else {
            c.get(1)
                .map_or(1, |m| amount(m.as_str(), s).n.max(1) as u32)
        };
        out.push(Effect::Bounce(t));
    }
    // "Choose two target creatures controlled by different players. Return those creatures
    // to their owners' hands" (Run Away Together).
    if s.contains("return those creatures to their owners' hands") {
        let mut t = read_target("creature", false);
        t.count = 2;
        out.push(Effect::Bounce(t));
    }
    for c in p.tap_lock.captures_iter(s) {
        let (phrase, lasting) = match (c.get(2), c.get(3), c.get(4)) {
            (Some(m), _, _) => (m.as_str().to_string(), lasting_lock(s)),
            (None, Some(m), _) => (m.as_str().to_string(), lasting_lock(s)),
            // "Target creature can't block (this turn)" only clears the way for an attack.
            (None, None, Some(_)) if c[0].ends_with("can't block") && !lasting_lock(s) => {
                continue;
            }
            (None, None, Some(m)) => (
                m.as_str().trim_start_matches("target ").to_string(),
                !s.contains("until end of turn") && !s.contains("this turn"),
            ),
            _ => continue,
        };
        let t = if phrase.starts_with("enchanted") {
            read_target("creature", false)
        } else {
            read_target(&phrase, false)
        };
        out.push(Effect::Lock { target: t, lasting });
    }
    for c in p.edict.captures_iter(s) {
        // "Each player sacrifices three creatures" / "thirteen creatures" (Blasphemous Edict):
        // a board wipe for everyone, not a one-creature edict.
        let n = c[0]
            .split_whitespace()
            .skip_while(|w| !w.starts_with("sacrifice"))
            .nth(1)
            .unwrap_or("a");
        if c[1].starts_with("each player")
            && (number(n).is_some_and(|a| a.n >= 3) || n == "thirteen" || n == "all")
        {
            out.push(Effect::Destroy(read_target("creature", true)));
            continue;
        }
        out.push(Effect::Edict {
            each_opponent: c[1].starts_with("each"),
        });
    }
    if s.contains("counter target") && s.contains("spell") {
        out.push(Effect::CounterSpell);
    }
    // Card flow. Looting first (draw then discard is filtering, not advantage).
    let loot = s.contains("draw a card, then discard a card")
        || s.contains("draws a card, then discards a card")
        || s.contains("discard a card, then draw a card")
        || s.contains("connive")
        || s.contains("recruit");
    if loot {
        out.push(Effect::Loot);
    }
    // Recruit also leaves a 1/1 Hero token behind.
    if s.contains("recruit") {
        out.push(Effect::Tokens {
            amount: Amount::fixed(1),
            power: 1,
            toughness: 1,
            keywords: Vec::new(),
        });
    }
    // "Draw two cards, then discard a card" / "draw three cards, then discard two": the
    // net cards gained (looting when it nets nothing).
    let discard_after = [", then discard ", ". then discard ", "if you do, discard "]
        .iter()
        .find_map(|k| s.find(k).map(|i| (i, k.len())));
    let net_draw = if loot {
        None
    } else {
        discard_after.and_then(|(i, len)| {
            let drawn = p.draw.captures(&s[..i])?;
            let n = drawn.get(2).map(|m| amount(m.as_str(), s).n).unwrap_or(2);
            let lost =
                number(s[i + len..].split_whitespace().next().unwrap_or("a")).map_or(1, |a| a.n);
            Some(n - lost)
        })
    };
    if let Some(net) = net_draw {
        out.push(if net <= 0 {
            Effect::Loot
        } else {
            Effect::Draw {
                amount: Amount::fixed(net),
                you: true,
            }
        });
    }
    // Conniving also grows the creature when a nonland card is discarded.
    if let Some(i) = s.find(" connive") {
        // Whoever connives gets the counter: "~ connives", "target Villain you control
        // connives", "you may have it connive", "the attacking creature connives".
        let who = s[..i].rsplit(", ").next().unwrap_or("");
        let who = who.trim_start_matches("you may have ").trim();
        let to = if who.ends_with('~') || who.is_empty() {
            Target::it()
        } else if let Some(t) = who.strip_prefix("target ") {
            read_target(t, false)
        } else if crate::restrict::refers_back(who) {
            Target::pronoun()
        } else {
            Target::it()
        };
        out.push(Effect::PlusCounters {
            amount: Amount::fixed(1),
            to,
        });
    }
    for c in p.draw.captures_iter(s) {
        if loot && c.get(0).is_some_and(|m| m.as_str().contains("a card")) {
            continue;
        }
        if net_draw.is_some() && c.get(1).is_none() {
            continue; // already counted net of the discard
        }
        let mut you = c.get(1).is_none_or(|m| m.as_str() == "target player");
        // "Each player discards their hand, then draws seven cards": still each player.
        if c.get(1).is_none()
            && c[0].starts_with("draws")
            && (s.starts_with("each player ") || s.starts_with("each opponent "))
        {
            you = false;
        }
        // "Draw a card for each card you've discarded this turn" (Misty Knight, whose cost
        // discards one): typically one card, not a board count.
        let typical = if s.contains("for each card you've discarded this turn") {
            1
        } else {
            TYPICAL_X
        };
        let amt = c.get(2).map(|m| amount(m.as_str(), s)).unwrap_or(Amount {
            n: typical,
            x: true,
        });
        out.push(Effect::Draw { amount: amt, you });
    }
    if s.contains("investigate") {
        out.push(Effect::ItemTokens {
            amount: Amount::fixed(1),
            kind: "clue".into(),
        });
    }
    if (s.contains("each opponent discards")
        || s.contains("target opponent discards")
        || s.contains("target player discards")
        || s.contains("each player discards"))
        && !loot
    {
        out.push(Effect::Discard { opponents: true });
    }
    for c in p.mill.captures_iter(s) {
        out.push(Effect::Mill {
            amount: amount(&c[1], s),
        });
    }
    for c in p.scry.captures_iter(s) {
        out.push(Effect::Scry(amount(&c[2], s)));
    }
    // Tokens.
    let before = out.len();
    // "Its controller creates a 3/3 Beast" (Beast Within, Pongify) gives the token away.
    let mut gave = false;
    // Tokens are read from the full text (to see who gets them), so skip those made by a
    // quoted ability that belongs to something else.
    let quoted = |start: usize| !self_grant && full[..start].matches('"').count() % 2 == 1;
    let given_away = |start: usize| {
        let head = &full[..start];
        head.ends_with("its controller ")
            || head.ends_with("that player ")
            || head.ends_with("their controller ")
            || head.ends_with("target opponent ")
            || head.ends_with("each opponent ")
            || head.ends_with("an opponent ")
            || head.ends_with("that opponent ")
            || head.ends_with("they ")
            || head.ends_with("defending player ")
    };
    for c in p.tokens.captures_iter(full) {
        if quoted(c.get(0).unwrap().start()) {
            continue;
        }
        if given_away(c.get(0).unwrap().start()) {
            gave = true;
            continue;
        }
        let n = counted(
            c.get(2)
                .map(|m| amount(m.as_str(), s))
                .unwrap_or(Amount::fixed(1)),
            s,
        );
        // "Create a number of 1/1 Rabbit tokens equal to ...".
        let n = if c[0].contains("a number of") {
            Amount {
                n: TYPICAL_X,
                x: true,
            }
        } else {
            n
        };
        let pw = amount(&c[3], s).n;
        let t = amount(&c[4], s).n;
        let kws = c
            .get(6)
            .map(|m| {
                KEYWORDS
                    .iter()
                    .filter(|k| m.as_str().contains(*k))
                    .map(|k| k.to_string())
                    .collect()
            })
            .unwrap_or_default();
        out.push(Effect::Tokens {
            amount: n,
            power: pw,
            toughness: t,
            keywords: kws,
        });
    }
    if out.len() == before {
        for c in p.tokens_plain.captures_iter(full) {
            if quoted(c.get(0).unwrap().start()) {
                continue;
            }
            if given_away(c.get(0).unwrap().start()) {
                gave = true;
                continue;
            }
            let kws = c
                .get(3)
                .map(|m| {
                    KEYWORDS
                        .iter()
                        .filter(|k| m.as_str().contains(*k))
                        .map(|k| k.to_string())
                        .collect()
                })
                .unwrap_or_default();
            out.push(Effect::Tokens {
                amount: if c[0].contains("a number of") {
                    Amount {
                        n: TYPICAL_X,
                        x: true,
                    }
                } else {
                    counted(amount(&c[1], s), s)
                },
                power: 1,
                toughness: 1,
                keywords: kws,
            });
        }
    }
    for c in p.item_tokens.captures_iter(full) {
        if quoted(c.get(0).unwrap().start()) {
            continue;
        }
        // "Destroy target permanent an opponent controls. They create a Treasure token"
        // (Vraska): read, and handed to that player as it resolves.
        let start = c.get(0).unwrap().start();
        if given_away(start) && full[..start].trim() != "they" {
            gave = true;
            continue;
        }
        // "a Food token or a Treasure token": a choice - the Treasure is the better one.
        let kind = if full.contains("food token or a treasure token") {
            "treasure".to_string()
        } else {
            c[2].to_string()
        };
        out.push(Effect::ItemTokens {
            amount: counted(amount(&c[1], s), s),
            kind,
        });
    }
    for c in p.counters.captures_iter(s) {
        let n = counted(amount(&c[1], s), s);
        let phrase = c[2].trim();
        let t = if phrase.starts_with('~') {
            Target::it()
        } else if phrase.starts_with("it")
            || phrase.starts_with("him")
            || phrase.starts_with("her")
            || phrase.starts_with("them")
        {
            Target::pronoun()
        } else if let Some((count, rest)) = phrase
            .strip_prefix("each of ")
            .and_then(|r| r.split_once("target "))
            .filter(|(n, _)| !n.contains(' ') || n.starts_with("up to") || n.starts_with("one or"))
        {
            // "Each of up to two (other) target creatures", "each of one or two target
            // creatures": one counter on each of them.
            let mut t = read_target(rest, false);
            t.count = if count.contains("three") { 3 } else { 2 };
            t.other |= count.contains("other");
            t
        } else if phrase.starts_with("each ") && !phrase.contains("you control") {
            // "Each creature target player controls", "each other creature".
            let mut t = read_target(
                phrase.trim_start_matches("each ").trim_start_matches("of "),
                true,
            );
            t.other |= phrase.starts_with("each other");
            t
        } else if phrase.starts_with("each ") && phrase.contains("you control") {
            // "Each creature you control", "each other Merfolk you control", "each creature
            // you control with flying".
            pump_target(phrase)
        } else {
            read_target(phrase.trim_start_matches("target "), false)
        };
        out.push(Effect::PlusCounters { amount: n, to: t });
    }
    // "Put a number of +1/+1 counters on ~ equal to the sacrificed creature's power".
    if let Some(rest) = s.split("put a number of +1/+1 counters on ").nth(1) {
        let t = if rest.starts_with('~') {
            Target::it()
        } else {
            read_target(rest.trim_start_matches("target "), false)
        };
        out.push(Effect::PlusCounters {
            amount: Amount {
                n: TYPICAL_X,
                x: true,
            },
            to: t,
        });
    }
    for c in p.pump.captures_iter(s) {
        let who = c[1].trim();
        let to = pump_target(who);
        // "+1/+1 for each other artifact you control": counts something on the board.
        let pw = counted(amount(&c[2], s), s).n;
        let t = if c[3].starts_with('-') {
            -amount(&c[3][1..], s).n
        } else {
            counted(amount(c[3].trim_start_matches('+'), s), s).n
        };
        let eot =
            c.get(5).is_some() || s.starts_with("until end of turn") || s.contains("this turn");
        out.push(Effect::Pump {
            power: pw,
            toughness: t,
            to,
            until_eot: eot,
        });
        if let Some(k) = c.get(4) {
            // "Has double strike and trample": every keyword to the end of the sentence.
            let rest = s[k.start()..].split(['"', '(']).next().unwrap_or("");
            let rest = rest.split(" until ").next().unwrap_or(rest);
            let kws: Vec<String> = KEYWORDS
                .iter()
                .filter(|kw| rest.contains(*kw))
                .map(|kw| kw.to_string())
                .collect();
            if !kws.is_empty() {
                out.push(Effect::Grant {
                    keywords: kws,
                    to,
                    until_eot: eot,
                });
            }
        }
    }
    if !out.iter().any(|e| matches!(e, Effect::Grant { .. })) {
        // "As long as ~ has a shield counter on him, you and other Heroes you control have
        // hexproof": the condition's "has" isn't the grant.
        let gs = match s.strip_prefix("as long as ") {
            Some(rest) => rest.split_once(", ").map_or(s, |x| x.1),
            None => s,
        };
        for c in p.grant.captures_iter(gs) {
            // "A token that's a copy of it, except it has haste": describes the token.
            if c[1].trim().starts_with("except") || c[1].contains(", except") {
                continue;
            }
            let to = pump_target(c[1].trim());
            let kws: Vec<String> = KEYWORDS
                .iter()
                .filter(|kw| c[2].contains(*kw))
                .map(|kw| kw.to_string())
                .collect();
            if !kws.is_empty() {
                // "Gain trample and get +X/+0 until end of turn": the duration comes later.
                let eot = c.get(3).is_some()
                    || s.starts_with("until end of turn")
                    || s.contains("this turn")
                    || (c[0].ends_with('+') || c[0].ends_with('-'))
                        && s.contains("until end of turn");
                out.push(Effect::Grant {
                    keywords: kws,
                    to,
                    until_eot: eot,
                });
            }
        }
    }
    for c in p.gain_life.captures_iter(s) {
        let amt = c
            .get(1)
            .map(|m| counted(amount(m.as_str(), s), s))
            .unwrap_or(Amount {
                n: TYPICAL_X,
                x: true,
            });
        out.push(Effect::GainLife(amt));
    }
    for c in p.drain.captures_iter(s) {
        // The damage pattern already handled "deals N damage to each opponent".
        if let (Some(who), Some(n)) = (c.get(1), c.get(2)) {
            let amt = if n.as_str() == "life equal to" {
                Amount {
                    n: TYPICAL_X,
                    x: true,
                }
            } else {
                counted(amount(n.as_str(), s), s)
            };
            out.push(Effect::Drain {
                amount: amt,
                each: who.as_str().starts_with("each"),
            });
        }
    }
    // Paying life as part of an effect ("you lose 2 life", Phyrexian Arena's drawback), also
    // after a draw ("you draw a card and lose 1 life", "target player draws three cards and
    // loses 3 life").
    let drawer_loses = [
        "draw a card and lose ",
        // "You draw a card, lose 1 life, and put a plan counter on ~" (Robot Domination).
        "draw a card, lose ",
        "cards and lose ",
        "draws a card and loses ",
        "cards and loses ",
    ]
    .iter()
    .find_map(|k| s.find(k).map(|i| i + k.len()))
    .filter(|_| !s.contains("each opponent draws") && !s.contains("each player draws"));
    if let Some(i) = s.find("you lose ").map(|i| i + 9).or(drawer_loses) {
        let n = s[i..].split_whitespace().next().unwrap_or("");
        if let Some(a) = number(n).filter(|_| s[i..].contains(" life")) {
            out.push(Effect::Rule(format!("you lose {} life", a.n)));
        }
    }
    // "Each opponent discards a card and loses 2 life" (Arbiter of Woe).
    if let Some(i) = s.find("each opponent discards a card and loses ") {
        let n = s[i + 40..].split_whitespace().next().unwrap_or("");
        if let Some(a) = number(n) {
            out.push(Effect::Drain {
                amount: a,
                each: true,
            });
        }
    }
    // "You lose life equal to that permanent's mana value" (Feed the Swarm), "its controller
    // gains life equal to its power" (Swords to Plowshares).
    if s.contains("you lose life equal to") {
        out.push(Effect::Rule("you lose life equal to it".into()));
    }
    if s.contains("its controller gains life equal to its power") {
        out.push(Effect::Rule(
            "its controller gains life equal to its power".into(),
        ));
    }
    // "Discard your hand, then draw ..." / "each player discards their hand": the discard
    // comes first.
    if s.contains("each player discards their hand")
        || s.contains("each player may discard their hand")
    {
        out.insert(0, Effect::Rule("each player discards their hand".into()));
    } else if s.contains("discard your hand") {
        out.insert(0, Effect::Rule("discards your hand".into()));
    }
    // "You lose the game" as an effect (Demonic Pact's last mode).
    if s.starts_with("you lose the game") {
        out.push(Effect::Rule("you lose the game".into()));
    }
    // "Distribute three +1/+1 counters among one, two, or three target creatures".
    if let Some(i) = s.find("distribute ") {
        let n = s[i + 11..].split_whitespace().next().unwrap_or("");
        if let Some(a) = number(n).filter(|_| s[i..].contains("+1/+1 counter")) {
            out.push(Effect::PlusCounters {
                amount: a,
                to: read_target("creature you control", false),
            });
        }
    }
    for c in p.add_mana.captures_iter(s) {
        out.push(Effect::AddMana(counted(mana_amount(&c[1]), s)));
    }
    // "Target player searches their library for a basic land" (Restorative Technique,
    // Cultivate-style gifts) is still a land search; the player is usually you.
    let searches = s.contains("search your library")
        || s.contains("searches their library") && !s.starts_with("its controller");
    if searches && !s.starts_with("if you search") {
        if s.contains("land card")
            || s.contains("basic land")
            || s.contains("forest")
            || s.contains("plains")
            || s.contains("island")
            || s.contains("swamp")
            || s.contains("mountain")
        {
            out.push(Effect::FetchLand {
                battlefield: s.contains("onto the battlefield"),
            });
        } else {
            out.push(Effect::Tutor);
        }
    }
    if (s.contains("from your graveyard to the battlefield")
        || s.contains("from a graveyard") && s.contains("onto the battlefield")
        || s.contains("graveyard onto the battlefield"))
        && !s.contains("land card")
    {
        out.push(Effect::Reanimate);
    } else if s.contains("land card")
        && (s.contains("from your graveyard to the battlefield")
            || s.contains("graveyard onto the battlefield"))
    {
        // A land back from the graveyard is an extra land drop.
        out.push(Effect::FetchLand { battlefield: true });
    } else if s.contains("from your graveyard to your hand")
        || s.contains("graveyard into your hand")
        || s.contains("return this card from your graveyard")
    {
        out.push(Effect::Regrowth);
    }
    if s.contains("untap target")
        || s.contains("untap another target")
        || s.contains("untap up to")
        || s.contains("untap all")
    {
        let phrase = s
            .split("untap ")
            .nth(1)
            .unwrap_or("creature")
            .trim_start_matches("all ")
            .trim_start_matches("another ")
            .trim_start_matches("up to one ")
            .trim_start_matches("up to two ")
            .trim_start_matches("target ");
        out.push(Effect::Untap(read_target(phrase, s.contains("untap all"))));
    } else if s.contains("untap that creature") || s.starts_with("untap it") {
        out.push(Effect::Untap(read_target("creature you control", false)));
    }
    if s.contains("gain control of") {
        out.push(Effect::GainControl(read_target(
            s.split("gain control of")
                .nth(1)
                .unwrap_or("")
                .trim_start_matches(" target "),
            false,
        )));
    }
    let token_copy = s.contains("token that's a copy")
        || s.contains("tokens that are copies")
        || s.contains("token copy");
    if !token_copy
        && (s.contains("copy target")
            || s.contains("copy that spell")
            || s.contains("copy it")
            || s.contains("copy of"))
    {
        out.push(Effect::Copy);
    }
    if token_copy {
        // "Create two tokens that are copies of it", "create X tokens that are copies".
        let n = s
            .find(" tokens that are copies")
            .and_then(|i| s[..i].rsplit(' ').next())
            .and_then(number)
            .unwrap_or(Amount::fixed(1));
        out.push(Effect::ItemTokens {
            amount: counted(n, s),
            kind: "copy".into(),
        });
    }
    for c in p.empower.captures_iter(s) {
        out.push(Effect::Empower(amount(&c[1], s)));
    }
    for c in p.amass.captures_iter(s) {
        out.push(Effect::Amass(amount(&c[1], s)));
    }
    if s.contains("goad") {
        out.push(Effect::Goad);
    }
    if s.contains("proliferate") || s.contains("another counter of that kind") {
        out.push(Effect::Proliferate);
    }
    if s.contains("additional combat phase") {
        out.push(Effect::ExtraCombat);
    }
    if s.contains("without paying its mana cost")
        || s.contains("without paying their mana costs")
        || s.contains("you may cast") && s.contains("from exile")
    {
        out.push(Effect::FreeSpell);
    }
    if s.contains("enters prepared") || s.contains("becomes prepared") {
        out.push(Effect::Prepared);
    }
    for c in p.enters_with.captures_iter(s) {
        out.push(Effect::PlusCounters {
            amount: amount(&c[1], s),
            to: Target::it(),
        });
    }
    for c in p.other_counters.captures_iter(s) {
        let kind = c[1].trim().to_string();
        if kind.contains("+1/+1")
            || kind.ends_with("+1")
            || kind.starts_with("or ")
            || kind.starts_with("more")
        {
            continue;
        }
        // Keyword counters (CR 122.1b) give the keyword.
        if KEYWORDS.contains(&kind.as_str()) {
            let to = if s.contains("on target") || s.contains("on up to one target") {
                read_target("creature you control", false)
            } else {
                Target::it()
            };
            out.push(Effect::Grant {
                keywords: vec![kind.clone()],
                to,
                until_eot: false,
            });
        }
        out.push(Effect::OtherCounters(kind));
    }
    for c in p.minus_counters.captures_iter(s) {
        let t = read_target(&c[2], false);
        out.push(Effect::Shrink {
            amount: amount(&c[1], s),
            to: t,
            until_eot: false,
        });
    }
    for c in p.tuck.captures_iter(s) {
        let phrase = c
            .get(1)
            .or(c.get(2))
            .map(|m| m.as_str())
            .unwrap_or("creature");
        let target = read_target(phrase, false);
        // "Second from the top or on the bottom", "the top or bottom of their library": the
        // owner picks, and redraws it soon - closer to bouncing it than to exiling it.
        if s.contains("from the top") || s.contains("top or bottom") {
            out.push(Effect::Bounce(target));
        } else {
            out.push(Effect::Exile {
                target,
                until: false,
            });
        }
    }
    if s.contains("attach ") {
        out.push(Effect::Attach);
    }
    // "At the beginning of the end step, sacrifice ~" (Ball Lightning), "then if ..., sacrifice
    // ~": the card goes (not a cost - costs are read before the colon).
    if (s.starts_with("sacrifice ~")
        || s.contains(", sacrifice ~")
        || s.contains("then sacrifice ~"))
        && !s.contains("then if")
    {
        out.push(Effect::Rule("sacrifices itself".into()));
    }
    if s.contains("that many plus one") {
        // One extra counter or token each time (Hardened Scales), weaker than doubling.
        out.push(Effect::Doubler("plus one".into()));
    } else if s.contains("double the number of") {
        // A one-off doubling of the counters there now (Mossborn Hydra, Biogenic Upgrade).
        out.push(Effect::Rule("doubles counters".into()));
    } else if s.contains("twice that many") {
        out.push(Effect::Doubler(if s.contains("token") {
            "tokens".into()
        } else {
            "counters".into()
        }));
    }
    if s.contains("deals double")
        || s.contains("it deals double")
        || s.contains("double that damage")
        || s.contains("double all damage")
    {
        out.push(Effect::Doubler("damage".into()));
    }
    // "Prevent all combat damage that would be dealt this turn" is a Fog; "... dealt to it /
    // by that creature this turn" protects (or blanks) one creature only.
    if s.contains("prevent all combat damage")
        && !s.contains("dealt to it")
        && !s.contains("dealt by it")
        && !s.contains("to that creature")
        && !s.contains("by that creature")
        && !s.contains("to and dealt by")
    {
        out.push(Effect::Fog);
    } else if s.contains("prevent all combat damage") {
        out.push(Effect::Rule(
            "prevents combat damage to or by one creature".into(),
        ));
    }
    if (s.contains("look at the top")
        || s.contains("reveal the top")
        || s.contains("reveal cards from the top")
        || s.contains("from among the milled cards"))
        && (s.contains("into your hand") || s.contains("in your hand"))
    {
        out.push(Effect::Dig);
    } else if (s.contains("look at the top") || s.contains("reveal the top"))
        && s.contains("onto the battlefield")
    {
        out.push(Effect::FreeSpell);
    }
    // Impulse draw: exiled cards you may play (CR 601.2 lets you cast from exile when an
    // effect allows it) - card access close to drawing.
    if s.contains("you may play that card")
        || s.contains("you may play those cards")
        || s.contains("you may play it")
        || s.contains("you may play them")
        || s.contains("you may cast that card")
        || s.contains("you may cast it this turn")
        || s.contains("you may play that card this turn")
    {
        out.push(Effect::Dig);
    }
    if s.contains("play lands from the top of your library") {
        out.push(Effect::Dig);
    }
    if s.contains("untap ~") {
        out.push(Effect::Untap(Target::it()));
    }
    // "... gains indestructible until end of turn. Tap it." (Immersturm Predator).
    if s == "tap it" || s == "tap ~" {
        out.push(Effect::Rule("taps itself".into()));
    }
    // "Exile ... then return it to the battlefield" is a flicker (already read above), not
    // bringing something back from the graveyard.
    if (s.contains("return it to the battlefield")
        || s.contains("return that card to the battlefield")
        || s.contains("return them to the battlefield")
        || s.contains("return her to the battlefield")
        || s.contains("return him to the battlefield"))
        && !out.contains(&Effect::Blink)
        && !s.contains("land")
    {
        out.push(Effect::Reanimate);
    }
    // "Enchanted creature, land, or planeswalker is a colorless land ... and loses all other
    // card types and abilities" (Imprisoned in the Moon): the creature is gone for good.
    if s.starts_with("enchanted ")
        && (s.contains("is a colorless land") || s.contains("loses all other card types"))
    {
        out.push(Effect::Lock {
            target: read_target("creature", false),
            lasting: true,
        });
    }
    if s.contains("base power and toughness 0/0")
        || s.contains("base power and toughness become 0/0")
    {
        out.push(Effect::Shrink {
            amount: Amount::fixed(20),
            to: read_target("creature", false),
            until_eot: true,
        });
    }
    // "Can't be blocked by creatures with power 2 or less" / "by Walls" is only partial
    // evasion, not unblockable.
    if s.contains("can't be blocked")
        && !s.contains("can't be blocked except")
        && !s.contains("can't be blocked by")
        && !s.contains("by more than")
        // (Threshold: "~ can't be blocked as long as there are seven or more cards in your
        // graveyard" - the ability's condition.)
        && (!s.contains("as long as") || s.starts_with("~ can't be blocked as long as"))
    {
        let to = if let Some(i) = s.find("target ") {
            read_target(&s[i + 7..], false)
        } else if let Some(i) = s
            .find(" can't be blocked")
            .filter(|_| s.contains("you control"))
        {
            // "Creatures you control with power 2 or less can't be blocked".
            pump_target(&s[..i])
        } else {
            Target::it()
        };
        out.push(Effect::Grant {
            keywords: vec!["unblockable".into()],
            to,
            until_eot: s.contains("this turn"),
        });
    }
    if s.contains("reveals their hand") && s.contains("discard")
        || s.contains("reveals their hand") && s.contains("exile")
    {
        out.push(Effect::Discard { opponents: true });
    }
    if s.contains("double target creature's power") || s.contains("double its power") {
        out.push(Effect::Pump {
            power: 3,
            toughness: 0,
            to: read_target("creature", false),
            until_eot: true,
        });
    }
    if s.starts_with("you have hexproof") || s.starts_with("you have shroud") {
        out.push(Effect::Grant {
            keywords: vec!["hexproof".into()],
            to: read_target("player", false),
            until_eot: false,
        });
    }
    // "Destroy target permanent. Its controller creates a 3/3 green Beast creature token."
    if gave {
        if let Some(c) = Regex::new(r"(?:its controller|their controller) creates? an? (\d+)/(\d+)")
            .ok()
            .and_then(|re| re.captures(full).map(|c| format!("{}/{}", &c[1], &c[2])))
        {
            out.push(Effect::Rule(format!("its controller gets a {} token", c)));
        }
    }
    // Graveyard hate: "exile up to one target card from a graveyard", "exile target player's
    // graveyard".
    // (Cards in a graveyard, not a permanent: no `Effect::Exile` for them.)
    if s.contains("exile target player's graveyard")
        || s.contains("exile each opponent's graveyard")
        || s.contains("exile all cards from target player's graveyard")
    {
        out.retain(|e| !matches!(e, Effect::Exile { .. }));
        out.push(Effect::Rule("exiles a graveyard".into()));
    } else if s.starts_with("exile")
        && (s.contains("card from a graveyard")
            || s.contains("cards from a single graveyard")
            || s.contains("card from an opponent's graveyard")
            || s.contains("target card from a graveyard"))
        && !s.contains("creature card from a graveyard")
    {
        if !s.contains("permanent") {
            out.retain(|e| !matches!(e, Effect::Exile { .. }));
        }
        out.push(Effect::Rule("exiles a graveyard card".into()));
    }
    // "You may play an additional land this turn".
    if s.contains("play an additional land this turn") {
        out.push(Effect::Rule("extra land drop".into()));
    }
    // Only a quoted ability: it's given to something else (an emblem, a token, an equipped
    // creature) - named, but not counted as this card's own effect.
    if out.is_empty() && gave {
        out.push(Effect::Rule("gives an opponent a token".into()));
    } else if out.is_empty() && full.contains('"') {
        out.push(Effect::Rule(
            if full.contains("emblem") {
                "gets an emblem"
            } else {
                "grants an ability"
            }
            .into(),
        ));
    }
    // "For each opponent, destroy up to one target creature that player controls": one
    // pick among each opponent's things.
    if s.contains("for each opponent, ") {
        for e in out.iter_mut() {
            for t in e.targets_mut() {
                if t.whose == Whose::Theirs && !t.mass {
                    t.restr.flags |= crate::restrict::PER_OPPONENT;
                }
            }
        }
    }
    // "Each player discards their hand, then draws seven cards": the discard comes first.
    if s.contains("discards their hand, then draw") || s.contains("discard your hand, then draw") {
        if let Some(d) = out.iter().position(|e| matches!(e, Effect::Discard { .. })) {
            let e = out.remove(d);
            let first_draw = out
                .iter()
                .position(|e| matches!(e, Effect::Draw { .. }))
                .unwrap_or(out.len());
            out.insert(first_draw, e);
        }
    }
    let _ = p.number_list;
    out
}

/// How much "add {R}{R}" / "add {R} or {G}" / "add three mana" produces.
fn mana_amount(what: &str) -> Amount {
    if what.starts_with('{') {
        // "{R} or {G}" is one mana; "{R}{R}" two.
        Amount::fixed(if what.contains(" or ") || what.contains(", ") {
            1
        } else {
            crate::card::symbols_in(what).len().max(1) as i32
        })
    } else {
        number(what.split_whitespace().next().unwrap_or("one")).unwrap_or(Amount::fixed(1))
    }
}

/// A tap or restriction that keeps the creature out of the game, not just for a turn.
fn lasting_lock(s: &str) -> bool {
    s.contains("doesn't untap")
        || s.contains("can't become untapped")
        || s.contains("for as long as")
        || s.contains("loses all abilities")
}

/// Special rules recognised by their wording: (phrase, plain-language label).
const RULES: &[(&str, &str)] = &[
    (
        "is the chosen type in addition",
        "becomes the chosen creature type",
    ),
    ("return ~ to its owner's hand", "returns itself to hand"),
    ("change the target of", "redirects a spell"),
    ("gets -1/-0", "weakens a creature's attack"),
    ("gets -4/-0", "weakens a creature's attack"),
    ("gets -5/-0", "weakens attackers"),
    ("gets -x/-0", "weakens a creature's attack"),
    (
        "you control target opponent during their next turn",
        "you control an opponent's next turn",
    ),
    (
        "shuffle it into its owner's library instead",
        "can't stay in a graveyard",
    ),
    (
        "would be put into a graveyard from anywhere, exile it instead",
        "exiles cards instead of the graveyard",
    ),
    ("poison counter", "gives poison counters"),
    ("base power and toughness", "sets base power/toughness"),
    ("players can't gain life", "stops all life gain"),
    (
        "creatures with flying can't attack you or block",
        "flyers can't attack you or block your creatures",
    ),
    ("put its counters on", "moves its counters"),
    (
        "gains control of target permanent you control",
        "gives a permanent away",
    ),
    (
        "you may put the exiled card into its owner's hand",
        "returns the exiled card",
    ),
    (
        "activate loyalty abilities",
        "loyalty abilities at instant speed",
    ),
    ("remove up to", "removes counters"),
    (
        "put target card from your graveyard on the bottom",
        "recycles a graveyard card",
    ),
    ("you can't cast ~ unless", "casting restriction"),
    (
        "enchanted permanent is a",
        "turns a permanent into a creature",
    ),
    ("by removing", "alternative casting cost"),
    (
        "put a card exiled with ~ into its owner's hand",
        "returns an exiled card",
    ),
    ("choose any card name", "names a card"),
    (
        "can't be activated unless they're mana abilities",
        "shuts off a named card's abilities",
    ),
    ("harness ~", "harness (turns on its infinity ability)"),
    (
        "has all activated abilities",
        "copies abilities from the graveyard",
    ),
    ("damage plus", "adds to noncombat damage"),
    (
        "no more than one creature can attack",
        "protects planeswalkers",
    ),
    (
        "spend mana as though it were mana of any type",
        "mana of any type for creatures",
    ),
    (
        "causes you to discard ~, put it onto the battlefield",
        "enters if an opponent makes you discard it",
    ),
    (
        "all other damage already dealt to him is healed",
        "heals himself",
    ),
    (
        "put ~ from your graveyard on top of your library",
        "returns to the top of the library",
    ),
    (
        "shuffle your graveyard into your library",
        "reshuffles the graveyard",
    ),
    ("must be blocked", "must be blocked"),
    (
        "from outside the game",
        "fetches a card from outside the game",
    ),
    ("exile ~", "exiles itself"),
    ("untap it", "untaps your creatures"),
    ("draws an additional card", "everyone draws an extra card"),
    ("look at an opponent's hand", "looks at an opponent's hand"),
    (
        "you control enchanted permanent",
        "steals the enchanted permanent",
    ),
    ("puts those cards into their graveyard", "mills opponents"),
    ("can't be blocked by", "hard to block"),
    (
        "whenever you tap a mountain for mana",
        "mountains make an extra red mana",
    ),
    ("exchange control of two target", "exchanges control"),
    ("you lose the game", "you lose the game"),
];

/// Effects spread over several sentences of one ability.
fn body_effects(body: &str, effects: &mut Vec<Effect>) {
    let has = |effects: &Vec<Effect>, f: fn(&Effect) -> bool| effects.iter().any(f);
    let looks = body.contains("look at the top")
        || body.contains("reveal the top")
        || body.contains("reveal cards from the top");
    if looks && body.contains("into your hand") && !has(effects, |e| matches!(e, Effect::Dig)) {
        effects.push(Effect::Dig);
    }
    if looks
        && body.contains("onto the battlefield")
        && !has(effects, |e| matches!(e, Effect::FreeSpell))
    {
        effects.push(Effect::FreeSpell);
    }
    // "Look at the top three... put one back, the rest into your graveyard": card selection.
    if looks
        && body.contains("into your graveyard")
        && !has(effects, |e| matches!(e, Effect::Scry(_) | Effect::Dig))
    {
        effects.push(Effect::Scry(Amount { n: 3, x: false }));
    }
    // "Choose target creature you own. ... Return each chosen creature to your hand" (The
    // Eagles Are Coming!): before what it does for each one returned.
    if body.contains("return each chosen creature to your hand")
        && !has(effects, |e| matches!(e, Effect::Bounce(_)))
    {
        effects.insert(0, Effect::Bounce(read_target("creature you own", false)));
    }
    // "Choose target nonland permanent. Its owner may put it on the top of their library. If
    // they do, ~ deals 2 damage to them. If they didn't ..., they put it on the bottom" (Clash
    // of Elements).
    if body.contains("its owner may put it on the top of their library")
        && !has(effects, |e| matches!(e, Effect::Bounce(_)))
    {
        effects.insert(0, Effect::Bounce(read_target("nonland permanent", false)));
    }
    // A dying attacker returning to hand (Ares): recursion of your own creatures.
    if body.contains("return that card to its owner's hand") {
        effects.push(Effect::Regrowth);
    }
    // "If one or more artifact tokens would be created ... that many 5/5 Dragons instead".
    if body.contains("tokens would be created")
        && !has(effects, |e| matches!(e, Effect::Doubler(_)))
    {
        effects.push(Effect::Doubler("tokens".into()));
    }
    if (body.contains("reveals their hand")
        || body.contains("reveal their hand")
        || body.contains("reveals a number of cards from their hand"))
        && (body.contains("discards that card")
            || body.contains("discard that card")
            || body.contains("exile that card")
            || body.contains("exile a nonland card"))
        && !has(effects, |e| matches!(e, Effect::Discard { .. }))
    {
        effects.push(Effect::Discard { opponents: true });
    }
    if body.contains("shuffles it into their library")
        || body.contains("their choice of the top or bottom")
    {
        if let Some(i) = body.find("target ") {
            let phrase = body[i + 7..].split(['.', ';']).next().unwrap_or("creature");
            let target = read_target(phrase, false);
            if !has(effects, |e| {
                matches!(e, Effect::Exile { .. } | Effect::Bounce(_))
            }) {
                // On top or bottom, they can draw it again soon: like a bounce.
                effects.push(if body.contains("top or bottom") {
                    Effect::Bounce(target)
                } else {
                    Effect::Exile {
                        target,
                        until: false,
                    }
                });
            }
        }
    }
    // "Exile target creature you control. Return it to the battlefield under its owner's
    // control" across two sentences: a flicker, not removal plus reanimation.
    let returns = [
        "return it to the battlefield",
        "return that card to the battlefield",
        "return them to the battlefield",
        "return those cards to the battlefield",
        "return the exiled card to the battlefield",
        "return the exiled cards to the battlefield",
    ]
    .iter()
    .any(|r| body.contains(r));
    if returns
        && body.contains("exile ")
        && has(effects, |e| matches!(e, Effect::Reanimate))
        && !body.contains("graveyard")
    {
        // "Exile target creature. ... return it to the battlefield under your control"
        // (Vindictive Triumph): removal that may steal it, not a flicker.
        let stolen = body.contains("under your control")
            && !has(
                effects,
                |e| matches!(e, Effect::Exile { target, .. } if target.whose == Whose::Yours),
            );
        if stolen {
            effects.retain(|e| !matches!(e, Effect::Reanimate));
        } else {
            effects.retain(|e| !matches!(e, Effect::Reanimate | Effect::Exile { .. }));
            if !effects.contains(&Effect::Blink) {
                effects.push(Effect::Blink);
            }
        }
    }
    // "Tap target creature. It doesn't untap / can't become untapped for as long as you
    // control ~" across sentences: locked down for good.
    if lasting_lock(body) {
        for e in effects.iter_mut() {
            if let Effect::Lock { lasting, .. } = e {
                *lasting = true;
            }
        }
    }
    // "Mill three cards. Put a creature card from among them into your hand": selection.
    if body.contains("from among them")
        && (body.contains("into your hand") || body.contains("in your hand"))
        && !has(effects, |e| matches!(e, Effect::Dig | Effect::Regrowth))
    {
        effects.push(Effect::Dig);
    }
    // (Cards put *into* a graveyard - "put the rest into your graveyard" - aren't coming back.)
    let from_gy = body.contains("from your graveyard")
        || body.contains("from a graveyard")
        || body.contains("from graveyards")
        || body.contains("from all graveyards")
        || body.contains("graveyards that were put there")
        || body.contains("in your graveyard")
        || body.contains("from among them")
            && body.contains("graveyard")
            && !body.contains("into your graveyard");
    if body.contains("graveyard") && body.contains("card") && from_gy {
        // "Search your library and/or graveyard for ... put it onto the battlefield" is a
        // tutor (already read), not a separate reanimation.
        if (body.contains("to the battlefield") || body.contains("onto the battlefield"))
            && !body.contains("land card")
            && !has(effects, |e| matches!(e, Effect::Reanimate | Effect::Tutor))
        {
            effects.push(Effect::Reanimate);
        } else if (body.contains("to your hand") || body.contains("into your hand"))
            && !has(effects, |e| matches!(e, Effect::Regrowth | Effect::Dig))
        {
            effects.push(Effect::Regrowth);
        }
    }
    if (body.contains("from your graveyard") && body.contains("you may cast"))
        || body.contains("gains flashback")
    {
        effects.push(Effect::Recast);
    }
    if body.contains("cast creature spells from the top of your library") {
        effects.push(Effect::Dig);
    }
    if body.contains("sacrifices the rest") {
        effects.push(Effect::Edict {
            each_opponent: true,
        });
    }
    if effects.is_empty() || body.contains("instead") || body.contains("can't") {
        for (phrase, label) in RULES {
            if body.contains(phrase)
                && !effects
                    .iter()
                    .any(|e| matches!(e, Effect::Rule(l) if l == label))
            {
                effects.push(Effect::Rule(label.to_string()));
            }
        }
    }
}

/// Who a pump or keyword grant affects.
fn pump_target(who: &str) -> Target {
    let who = who
        .trim()
        .trim_start_matches("then ")
        .trim_start_matches("and ")
        .trim_start_matches("until end of turn, ");
    // "Any number of target creatures you control": all of them, if you like.
    if who.contains("any number of target") {
        return read_target(who.split("target ").nth(1).unwrap_or("creature"), true);
    }
    // A team: "creatures you control", "each other creature you control", "other Merfolk
    // you control", "creatures you control with a +1/+1 counter on them", "lands you
    // control". Narrowed by a type, colour or condition, it's a restricted mass effect.
    if let Some(i) = who.find(" you control").filter(|_| !who.contains("target")) {
        let head = who[..i]
            .trim()
            .trim_start_matches("each ")
            .trim_start_matches("all ");
        let tail = who[i + 12..].trim();
        let other = head.starts_with("other ") || head.starts_with("another ");
        let noun = head
            .trim_start_matches("other ")
            .trim_start_matches("another ");
        let mut t = read_target(&format!("{} you control", noun), true);
        if t.kind == Kind::Other {
            // A creature type ("Villains", "Merfolk").
            t.kind = Kind::Creature;
            t.restricted = true;
        }
        let plain = [
            "creature",
            "creatures",
            "creature tokens",
            "nontoken creatures",
        ];
        let narrowed = ["with ", "of ", "that ", "who "]
            .iter()
            .any(|w| tail.starts_with(w));
        t.restricted |= narrowed || t.kind == Kind::Creature && !plain.contains(&noun);
        t.other = other;
        t.restr = crate::restrict::Restr::read(who);
        return t;
    }
    // "She loses all abilities and gains haste": the card itself (back from the graveyard).
    if ["she ", "he ", "they ", "it "]
        .iter()
        .any(|w| who.starts_with(w) && who.contains("loses all abilities"))
    {
        return Target::it();
    }
    if who.ends_with('~') || who.ends_with("this creature") {
        return Target::it();
    }
    if who == "it" || who.ends_with(" it") || who == "he" || who == "she" {
        return Target::pronoun();
    }
    // "They" / "those creatures": the group just named (or chosen).
    if who == "they" || who.starts_with("those ") {
        let mut t = read_target("creature", false);
        t.restr.flags |= crate::restrict::PREV;
        return t;
    }
    if who.contains("equipped creature") || who.contains("enchanted creature") {
        return read_target("creature you control", false);
    }
    // "Each other Villain gets +1/+1": every creature of that type, yours mostly.
    if let Some(rest) = who.strip_prefix("each other ") {
        let mut t = read_target(rest, true);
        if t.kind == Kind::Other {
            t.kind = Kind::Creature;
            t.whose = Whose::Yours;
            t.restricted = true;
        }
        t.other = true;
        return t;
    }
    if who.contains("all creatures") || who.contains("each creature") {
        return read_target("creature", true);
    }
    if let Some(i) = who.find("target ") {
        return read_target(&who[i + 7..], false);
    }
    read_target(who, false)
}

/// Things a line can say that need no effect: costs, restrictions, timing notes.
fn ignorable(s: &str) -> bool {
    const NOTES: &[&str] = &[
        "can't be countered",
        "costs {",
        "cost {",
        "less to cast",
        "more to cast",
        "enters tapped",
        "this ability triggers only",
        "activate only",
        "as an additional cost",
        "you may cast this spell as though",
        "then shuffle",
        "reveal it",
        "it's still a",
        "if you do",
        "you may pay",
        "spend only",
        "this spell can't",
        "can block an additional",
        "can block any number",
        "doesn't untap during your untap step",
        "you may play",
        "skip your",
        "maximum hand size",
        "if this spell was cast using teamwork",
        "choose one",
        "choose two",
        "choose any number",
        "can't block",
        "can't attack",
        "attacks each combat if able",
        "as long as",
        "for each",
        "power is equal to",
        "power and toughness are each equal",
        "toughness is equal to",
        "assigns combat damage",
        "as ~ enters, choose",
        "a deck can have any number",
        "look at the top card of your library any time",
        "as though it had flash",
        "as though they had flash",
        "begin the game with",
        "is the chosen color",
        "players can't cast spells",
        "opponents can't cast spells",
        "don't cause abilities to trigger",
        "can't lose the game",
        "end the turn",
        "you may activate abilities of creatures you control as though",
        "can be activated an additional time",
        "if ~ is your commander",
        "split second",
        "triggers an additional time",
        "aren't put into their owners' graveyards",
        "enter tapped",
        "card from a graveyard",
        "cards from target player's graveyard",
        "• target",
        "sacrifice ~",
        "would die, exile it instead",
        "can attack as though",
        "have improvise",
        "prevent all noncombat damage",
    ];
    NOTES.iter().any(|n| s.contains(n))
}

/// Replaces the card's own name and "this creature"-style references with "~".
/// Drops accents so "Thrór's Map" matches rules text that writes "Thror's Map".
fn fold_accents(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ñ' => 'n',
            other => other,
        })
        .collect()
}

pub fn self_refs(text: &str, card: &Card) -> String {
    let mut t = fold_accents(
        &text
            .to_lowercase()
            .replace('’', "'")
            .replace('−', "-")
            .replace(['“', '”'], "\""),
    );
    let name = fold_accents(&card.name.to_lowercase());
    t = t.replace(&name, "~");
    // Short names: "Bilbo" for "Bilbo Baggins, Burglar", "Ka-Zar" for "Ka-Zar of the Savage
    // Land", "Kiora" for "Kiora of Fire and Ashes", "Dain" for the legendary "Dáin Ironfoot".
    const NOT_NAMES: &[&str] = &[
        "the", "lord", "king", "lady", "captain", "doctor", "general", "agent", "high", "queen",
        "prince", "princess", "master", "mister", "madame", "great", "white", "black", "red",
        "blue", "green", "iron", "dark", "old", "young", "little", "big", "grand", "elder",
        "ancient", "first", "last", "baron", "count", "sir", "saint", "ghost", "giant", "storm",
        "fire", "death", "night", "shadow", "steel", "golden", "silver",
    ];
    let mut shorts: Vec<&str> = Vec::new();
    // The part before a comma is a real name even when it's a word ("Ghost, Spectral
    // Saboteur").
    let comma_name = name.split_once(", ").map(|(s, _)| s);
    if let Some((s, _)) = name.split_once(" of ") {
        shorts.push(s);
    }
    if let Some((s, _)) = name.split_once(" the ").filter(|x| !x.0.contains(' ')) {
        shorts.push(s);
    }
    let legendary = card.type_line.contains("Legendary")
        && (card.type_line.contains("Creature") || card.type_line.contains("Planeswalker"));
    if legendary {
        if let Some(first) = name.split([' ', ',']).next() {
            if name.contains(' ') && first.len() >= 4 && !NOT_NAMES.contains(&first) {
                shorts.push(first);
            }
        }
    }
    shorts.retain(|s| !NOT_NAMES.contains(s));
    // Longest first, so "Bilbo Baggins" goes before "Bilbo".
    shorts.extend(comma_name);
    shorts.sort_by_key(|s| std::cmp::Reverse(s.len()));
    for short in shorts {
        if short.len() < 3 {
            continue;
        }
        // Whole words only: "Ori" mustn't turn "storied" into "st~ed".
        if let Ok(re) = Regex::new(&format!(r"\b{}\b", regex::escape(short))) {
            t = re.replace_all(&t, "~").into_owned();
        }
    }
    for this in [
        "this creature",
        "this enchantment",
        "this artifact",
        "this land",
        "this spell",
        "this card",
        "this token",
        "this vehicle",
        "this planeswalker",
        "this equipment",
        "this aura",
        "this saga",
        "this permanent",
    ] {
        t = t.replace(this, "~");
    }
    t
}

fn is_keyword_line(line: &str) -> Option<Vec<String>> {
    let l = line.trim().trim_end_matches('.');
    // Keywords with a non-mana cost after a dash: "Ward—{3}, pay 3 life", "Equip—{2}, pay 2 life".
    for k in ["ward—", "equip—", "flashback—", "kicker—", "cycling—"] {
        if l.starts_with(k) && !l.contains(':') {
            return Some(vec![l.to_string()]);
        }
    }
    if l.starts_with("enchant ") && !l.contains(':') && l.len() < 50 {
        return Some(vec![l.to_string()]);
    }
    let parts: Vec<&str> = l.split(", ").collect();
    let mut out = Vec::new();
    for p in parts {
        let p = p.trim();
        let known = KEYWORDS.iter().any(|k| {
            p == *k
                || p.starts_with(&format!("{} ", k))
                || p.starts_with(&format!("{}—", k))
                || p.starts_with(&format!("{} {{", k))
        }) || p.starts_with("hexproof from")
            || p.starts_with("protection from")
            || p.starts_with("affinity for")
            || p.starts_with("flashback")
            || p.starts_with("kicker")
            || p.starts_with("overload")
            || p.starts_with("sneak")
            || p.starts_with("equip—")
            || p.starts_with("split second")
            || p.contains("cycling") && p.len() < 30;
        let known = known || (p.starts_with("enchant ") && !p.contains(':'));
        if !known || p.contains(':') || p.len() > 40 {
            return None;
        }
        out.push(p.to_string());
    }
    Some(out)
}

/// Reads a whole card.
pub fn parse(card: &Card) -> Parsed {
    let full = crate::card::strip_reminder(&card.oracle_text);
    let mut faces = full.splitn(2, "// Back face:");
    let front = faces.next().unwrap_or("");
    let mut parsed = parse_face(front, card);
    if let Some(back) = faces.next() {
        // "Name {cost}\nType\n[P/T]\ntext" - parse the rules text of the back face.
        let lines: Vec<&str> = back
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        let is_spell = lines
            .get(1)
            .is_some_and(|t| t.contains("Instant") || t.contains("Sorcery"));
        let body: Vec<&str> = lines
            .iter()
            .skip(2)
            .filter(|l| !l.contains('/') || l.contains(' '))
            .copied()
            .collect();
        let mut back_card = card.clone();
        back_card.type_line = lines.get(1).map(|s| s.to_string()).unwrap_or_default();
        back_card.name = lines
            .first()
            .map(|s| s.split(" {").next().unwrap_or(s).to_string())
            .unwrap_or_default();
        let mut b = parse_face(&body.join("\n"), &back_card);
        if is_spell {
            for a in b.abilities.iter_mut() {
                a.when = vec![When::Spell];
            }
        }
        parsed.back = Some(Box::new(b));
    }
    // "Gets +X/+0, where X is War Machine's power", "add X mana ..., where X is this
    // creature's power", "where X is the number of Equipment attached to him": the X is the
    // card's own printed power (or one Equipment), not a typical X.
    let own_power = card
        .power
        .as_deref()
        .and_then(|p| p.parse::<i32>().ok())
        .filter(|&p| p > 0);
    for a in parsed.abilities.iter_mut() {
        let n = if a.text.contains("where x is ~'s power")
            || a.text.contains("where x is this creature's power")
        {
            own_power
        } else if a
            .text
            .contains("where x is the number of equipment attached")
        {
            Some(1)
        } else {
            None
        };
        if let Some(n) = n {
            for e in a.effects.iter_mut() {
                set_x(e, n, &a.text);
            }
        }
    }
    // "When you cast this spell, copy it X times": the spell's effects happen X+1 times
    // (Photon Blast Barrage), rather than a separate "copy" worth little on its own.
    if card.text_lower().contains("copy it x times") {
        let spell_effects: Vec<Effect> = parsed
            .abilities
            .iter()
            .filter(|a| a.when.contains(&When::Spell) && !a.effects.contains(&Effect::Copy))
            .flat_map(|a| a.effects.iter().cloned())
            .collect();
        for a in parsed.abilities.iter_mut() {
            if a.when.contains(&When::Spell) && a.effects.contains(&Effect::Copy) {
                a.effects = spell_effects
                    .iter()
                    .cloned()
                    .chain(spell_effects.iter().cloned())
                    .collect();
                a.optional = false;
            }
        }
    }
    // "Destroy each nonland permanent with mana value X whose controller was dealt combat
    // damage by ~ this turn" (Steel Hellkite): only after it connects, and only that
    // player's permanents of one mana value - a few of theirs, never yours.
    for a in parsed.abilities.iter_mut() {
        if !a.text.contains("was dealt combat damage by") {
            continue;
        }
        for e in a.effects.iter_mut() {
            if let Effect::Destroy(t) = e {
                t.whose = Whose::Theirs;
                t.mass = false;
                t.count = 2;
            }
        }
        a.conditional = true;
    }
    parsed
}

fn parse_face(text: &str, card: &Card) -> Parsed {
    let mut out = Parsed::default();
    let lower_type = card.type_line.to_lowercase();
    let is_spell = lower_type.contains("instant") || lower_type.contains("sorcery");
    let t = self_refs(text, card);
    let lines: Vec<String> = t
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].clone();
        i += 1;
        if let Some(kws) = is_keyword_line(&line) {
            out.keywords.extend(kws);
            continue;
        }
        // "Choose one —" (or "When ~ enters, choose two —", "Choose one. If ..., choose both
        // instead.", "When you do, choose up to that many.") with bullet modes after it.
        let choose_at = line.find("choose ");
        if let Some(choose_at) = choose_at
            && (line.ends_with('—') || line.ends_with('-') || line.ends_with('.'))
            && i < lines.len()
            && lines[i].starts_with('•')
        {
            let mut modes = Vec::new();
            while i < lines.len() && lines[i].starts_with('•') {
                modes.push(lines[i].trim_start_matches('•').trim().to_string());
                i += 1;
            }
            // Read the header for its trigger/cost, the modes for the effects.
            let header = line[..choose_at].trim().trim_end_matches(',').to_string();
            let header_line = if header.is_empty() {
                "choose".to_string()
            } else {
                format!("{}, choose", header)
            };
            let mut ability = read_line(&header_line, card, is_spell);
            if let Some(a) = ability.as_mut() {
                // The whole header ("choose one that hasn't been chosen", "choose two"),
                // without an ability-word label in front.
                a.text = normalize_label(&line);
                if let Some((label, rest)) = a.text.clone().split_once(" — ") {
                    if !label.contains("choose") {
                        a.text = rest.to_string();
                    }
                }
                a.effects.clear();
                a.modes.clear();
                for m in &modes {
                    let mut e = Vec::new();
                    for s in sentences(m) {
                        e.extend(read_effects(&s));
                    }
                    body_effects(m, &mut e);
                    a.modes.push(e.len().min(255) as u8);
                    a.mode_texts.push(m.clone());
                    a.effects.extend(e);
                }
                // Modes that only pick targets ("• Target creature card.") with the effect in
                // the header ("Choose up to two. Return those cards ... to your hand.").
                if a.effects.is_empty() {
                    if let Some((_, rest)) = line[choose_at..].split_once(". ") {
                        let times = if line.contains("choose two") || line.contains("up to two") {
                            2
                        } else {
                            1
                        };
                        let mut e = read_effects(rest);
                        body_effects(rest, &mut e);
                        for _ in 0..times {
                            a.effects.extend(e.iter().cloned());
                        }
                    }
                    a.modal = false;
                    a.modes.clear();
                    a.mode_texts.clear();
                } else {
                    a.modal = true;
                    // Modes of one effect each need no grouping.
                    if a.modes.iter().all(|&n| n == 1) {
                        a.modes.clear();
                    }
                }
                // A mode with a cost of its own ("Remove a counter ... If you do, draw a card").
                if modes.iter().any(|m| m.contains("if you do")) {
                    a.conditional = true;
                }
                if a.when.is_empty() {
                    a.when = vec![if is_spell { When::Spell } else { When::Static }];
                }
            }
            match ability {
                Some(a) if !a.effects.is_empty() => out.abilities.push(a),
                _ => out.unparsed.push(line),
            }
            continue;
        }
        // "X. If <condition>, Y instead." - Y replaces X when the condition holds (Tidings of
        // War: amass 1, or 3 if cast from a graveyard). Read as the plain ability plus a
        // conditional alternative, so the two aren't added together.
        let (base_line, alternative, instead) = split_instead(&line);
        // "Deals 2 damage to target attacking or blocking creature. If ..., it deals 4
        // damage to that creature instead" (Helicarrier Strike): still the same kind of
        // creature.
        let alternative = alternative.map(|alt| {
            if instead {
                same_target_as(&base_line, &alt)
            } else {
                alt
            }
        });
        if let Some(alt_line) = alternative {
            let base = read_line(&base_line, card, is_spell).filter(|a| !a.effects.is_empty());
            if let (Some(mut base), Some(mut alt)) = (base, read_line(&alt_line, card, is_spell)) {
                // "Deals 2 damage to any target. If ~ was kicked, it deals 4 damage instead":
                // the alternative says only what changes, so it's the same effects changed.
                if alt.effects.is_empty() && instead {
                    alt.effects = changed_effects(&base.effects, &alt_line);
                }
                if !alt.effects.is_empty() {
                    // "X if <condition>. Otherwise, Y": X needs the condition too.
                    if lines[i - 1].contains(". otherwise, ") {
                        base.conditional = true;
                    }
                    out.abilities.push(base);
                    alt.conditional = true;
                    alt.instead = instead;
                    out.abilities.push(alt);
                    continue;
                }
            }
        } else if base_line != line {
            // "Choose target creature card with mana value 4 or less. If ~ was cast using
            // teamwork, instead choose target creature card. Return the chosen card ...":
            // the condition only widens the choice; the rest always happens.
            if let Some(a) = read_line(&base_line, card, is_spell).filter(|a| !a.effects.is_empty())
            {
                out.abilities.push(a);
                continue;
            }
        }
        match read_line(&line, card, is_spell) {
            Some(a) if !a.effects.is_empty() || ignorable(&a.text) => out.abilities.push(a),
            Some(a) => {
                if ignorable(&line) {
                    out.abilities.push(a);
                } else {
                    out.unparsed.push(line);
                }
            }
            None => {
                if !ignorable(&line) {
                    out.unparsed.push(line);
                }
            }
        }
    }
    // "As an additional cost to cast this spell, discard a card. / Draw two cards.": the
    // first card drawn only replaces the one paid.
    if is_spell && t.contains("as an additional cost to cast this spell, discard a card") {
        if let Some(e) = out
            .abilities
            .iter_mut()
            .flat_map(|a| a.effects.iter_mut())
            .find(|e| matches!(e, Effect::Draw { you: true, .. }))
        {
            let n = match e {
                Effect::Draw { amount, .. } => amount.n,
                _ => 1,
            };
            *e = if n <= 1 {
                Effect::Loot
            } else {
                Effect::Draw {
                    amount: Amount::fixed(n - 1),
                    you: true,
                }
            };
        }
    }
    // Teamwork, Storied and friends live in the keyword list; Sagas' "I, II —" lines are
    // chapters (handled in read_line).
    out
}

/// Puts a known value in place of an effect's estimated X ("where X is ~'s power").
fn set_x(e: &mut Effect, n: i32, text: &str) {
    match e {
        Effect::Draw { amount, .. }
        | Effect::Mill { amount }
        | Effect::Tokens { amount, .. }
        | Effect::PlusCounters { amount, .. }
        | Effect::Damage { amount, .. }
        | Effect::Shrink { amount, .. }
        | Effect::Drain { amount, .. }
        | Effect::Scry(amount)
        | Effect::GainLife(amount)
        | Effect::AddMana(amount)
        | Effect::Amass(amount)
            if amount.x =>
        {
            amount.n = n;
        }
        Effect::Pump {
            power, toughness, ..
        } => {
            if text.contains("+x/") {
                *power = n;
            }
            if text.contains("/+x") {
                *toughness = n;
            }
        }
        _ => {}
    }
}

/// "That creature" in an alternative ("..., it deals 4 damage to that creature instead") is
/// the creature the base sentence targets: written out the same way ("target attacking or
/// blocking creature"), so it keeps that sentence's restrictions. "That creature's
/// controller" is left alone.
fn same_target_as(base: &str, alt: &str) -> String {
    let Some(start) = base.find("target ") else {
        return alt.to_string();
    };
    let rest = &base[start..];
    let Some(end) = rest.find("creature") else {
        return alt.to_string();
    };
    let mut phrase = rest[..end + "creature".len()].to_string();
    if base[..start].ends_with("another ") {
        phrase.insert_str(0, "another ");
    }
    for owner in [" you control", " an opponent controls"] {
        if rest[end + "creature".len()..].starts_with(owner) {
            phrase.push_str(owner);
        }
    }
    // A phrase that runs past one target ("target player ... target creature") is not one.
    if phrase.contains(['.', ',']) || phrase.matches("target").count() > 1 {
        return alt.to_string();
    }
    // Only in what happens, not in the condition ("if that creature is another Hero, put
    // two +1/+1 counters on it instead" is about the same target).
    let cond_end = alt
        .find("if ")
        .and_then(|i| alt[i..].find(", ").map(|j| i + j + 2))
        .unwrap_or(0);
    let mut out = alt[..cond_end].to_string();
    let mut tail = &alt[cond_end..];
    while let Some(i) = tail.find("that creature") {
        let after = &tail[i + "that creature".len()..];
        out.push_str(&tail[..i]);
        if after.starts_with('\'') || after.starts_with('’') {
            out.push_str("that creature");
        } else {
            out.push_str(&phrase);
        }
        tail = after;
    }
    out.push_str(tail);
    out
}

/// "X. If <condition>, Y instead." -> ("X.", Some("<same trigger/cost>, if <condition>, Y
/// instead")). Replacement effects ("if ... would ..., ... instead") are left alone: they
/// aren't alternatives to an earlier sentence.
/// The third value says the alternative replaces the base ("instead") rather than adding
/// to it. A base different from `line` with no alternative is the line without a sentence
/// that only widened a choice.
fn split_instead(line: &str) -> (String, Option<String>, bool) {
    // "X if <condition>. Otherwise, Y." - two alternatives (HYDRA Troopers).
    if let Some(i) = line.find(". otherwise, ") {
        let sentence = line[i + 2..].trim().trim_end_matches('.');
        let alt_body = sentence.trim_start_matches("otherwise, ");
        if !sentence.contains(". ") {
            let base = format!("{}.", &line[..i]);
            return (
                base.clone(),
                Some(with_same_start(&base, &format!("if not, {}", alt_body))),
                false,
            );
        }
    }
    let Some(i) = line.find(". if ").or_else(|| line.find(". then if ")) else {
        return (line.to_string(), None, false);
    };
    let sentence = line[i + 2..].trim().trim_end_matches('.');
    // "If <condition>, Y instead." and "If <condition>, instead Y." (Cruel Alliance), or an
    // extra "If <condition>, it also ..." (Repulsor Blast's teamwork bonus).
    let instead_form = sentence.ends_with("instead") || sentence.contains(", instead ");
    let also_form = sentence.contains(" also ");
    if (instead_form || also_form) && !sentence.contains(" would ") && !sentence.contains(". ") {
        let sentence = sentence.replace(", instead ", ", ");
        let base = format!("{}.", &line[..i]);
        return (
            base.clone(),
            Some(with_same_start(&base, &sentence)),
            instead_form,
        );
    }
    // "Choose target creature you own. If ~ was kicked, instead choose any number of target
    // creatures you own. Return each chosen creature ...": the condition only widens the
    // choice, and the sentences after it happen either way.
    if sentence.starts_with("if ") && !sentence.contains(" would ") {
        if let Some((head, tail)) = sentence.split_once(". ") {
            if head.contains(", instead ") && !tail.is_empty() {
                return (format!("{}. {}.", &line[..i], tail), None, false);
            }
        }
    }
    // Any other later "If <condition>, Y." only gates Y: "Draw a card. If you control a
    // Wizard, scry 2." The rest of the line happens regardless. Conditions that look back
    // at the earlier sentence ("if it's a creature card", "if you do") stay with it, and
    // "would" sentences are replacement effects.
    let (head, tail) = sentence.split_once(". ").unwrap_or((sentence, ""));
    let head = head.trim_start_matches("then ");
    const LOOKS_BACK: &[&str] = &[
        "if it",
        "if that",
        "if they",
        "if those",
        "if the ",
        "if you do",
        "if you don't",
        "if you didn't",
        "if not",
        "if no",
        "if a card",
        "if one",
        "if both",
        "if two",
        "if he",
        "if she",
        "if this way",
        "if x",
    ];
    let (cond, _) = head.split_once(", ").unwrap_or((head, ""));
    if head.contains(" would ")
        || head.contains("instead")
        || head.contains("this way")
        || cond.ends_with(" does")
        || cond.ends_with(" do")
        || cond.ends_with(" did")
        || LOOKS_BACK.iter().any(|l| head.starts_with(l))
        || !head.contains(", ")
    {
        return (line.to_string(), None, false);
    }
    // "Then if you control four or more Treasures, sacrifice ~. If you do, create a Dragon":
    // the "if you do" belongs with the conditional part.
    if tail.starts_with("if you do") || tail.starts_with("when you do") {
        let base = format!("{}.", &line[..i]);
        let alt = format!("{}. {}", head, tail);
        return (base.clone(), Some(with_same_start(&base, &alt)), false);
    }
    let base = if tail.is_empty() {
        format!("{}.", &line[..i])
    } else {
        format!("{}. {}.", &line[..i], tail)
    };
    (base.clone(), Some(with_same_start(&base, head)), false)
}

/// The effects of "..., it deals 4 damage instead" / "..., create five of those tokens
/// instead": the base effects with the new number.
fn changed_effects(base: &[Effect], alt: &str) -> Vec<Effect> {
    let damage = Regex::new(r"deals? (\d+) damage")
        .ok()
        .and_then(|re| re.captures(alt))
        .and_then(|c| c[1].parse::<i32>().ok());
    let times = Regex::new(r"create ([a-z]+|\d+) of those")
        .ok()
        .and_then(|re| re.captures(alt))
        .and_then(|c| number(&c[1]))
        .map(|a| a.n.clamp(1, 10) as usize)
        .unwrap_or(1);
    // "That creature gets -13/-13 instead".
    let shrink = Regex::new(r"gets? -(\d+)/-\d+")
        .ok()
        .and_then(|re| re.captures(alt))
        .and_then(|c| c[1].parse::<i32>().ok());
    let mut out = Vec::new();
    for _ in 0..times {
        for e in base {
            out.push(match (e, damage, shrink) {
                (Effect::Damage { to, .. }, Some(n), _) => Effect::Damage {
                    amount: Amount::fixed(n),
                    to: *to,
                },
                (Effect::Shrink { to, until_eot, .. }, _, Some(n)) => Effect::Shrink {
                    amount: Amount::fixed(n),
                    to: *to,
                    until_eot: *until_eot,
                },
                _ => e.clone(),
            });
        }
    }
    out
}

/// How many effects the parser reads from `text`: the start of an ability's text (trigger,
/// cost or label, then sentences). The simulator uses it to tell which effects come after a
/// cost paid while the ability resolves ("... you may sacrifice a creature. If you do, ...").
pub fn effects_count(text: &str) -> usize {
    let mut body = text.trim();
    if let Some((label, rest)) = body.split_once(" — ") {
        if label.split_whitespace().count() <= 4 && !label.contains(':') && !label.contains('{') {
            body = rest;
        }
    }
    if let Some(rest) = body.strip_prefix('[') {
        body = rest.split_once("]:").map(|x| x.1).unwrap_or(rest);
    } else if body.starts_with("when ") || body.starts_with("whenever ") || body.starts_with("at ")
    {
        body = split_trigger(body).1;
        if let Some(cond) = body.strip_prefix("if ") {
            body = cond.split_once(", ").map(|x| x.1).unwrap_or("");
        }
    } else if let Some((_, effect)) = activated_split(body) {
        body = effect;
    }
    sentences(body).iter().map(|s| read_effects(s).len()).sum()
}

/// Puts `sentence` behind the same trigger or cost as `base` ("When ~ enters, " / "{2}: ").
fn with_same_start(base: &str, sentence: &str) -> String {
    // Keep a label ("Morbid — ", "Power-up — ") in front so the alternative reads the same.
    if let Some((label, rest)) = base.split_once(" — ") {
        if label.split_whitespace().count() <= 4 && !label.contains(':') && !label.contains('{') {
            return format!("{} — {}", label, with_same_start(rest, sentence));
        }
    }
    if base.starts_with('[') {
        if let Some((cost, _)) = base.split_once("]: ") {
            return format!("{}]: {}", cost, sentence);
        }
    }
    if base.starts_with("when ") || base.starts_with("whenever ") || base.starts_with("at ") {
        let (trigger, _) = split_trigger(base);
        format!("{}, {}", trigger, sentence)
    } else if let Some((cost, _)) = activated_split(base) {
        format!("{}: {}", cost, sentence)
    } else {
        sentence.to_string()
    }
}

/// Splits a triggered ability into (trigger, rest) at the first comma outside the trigger
/// wording.
fn split_trigger(line: &str) -> (&str, &str) {
    let mut from = 0;
    while let Some(off) = line[from..].find(", ") {
        let i = from + off;
        // "Whenever a nontoken, non-Angel creature dies", "whenever another Goblin, Orc, or
        // Army you control dies": a list, not the end of the trigger.
        let next = &line[i + 2..];
        let word = next.split([' ', ',']).next().unwrap_or("");
        let next_is_adjective = next.starts_with("non")
            || next.starts_with("or ")
            || next.starts_with("and/or ")
            || crate::restrict::subtype_word(word).is_some() && !matches!(word, "it" | "its" | "~");
        if !next_is_adjective {
            return (&line[..i], line[i + 2..].trim());
        }
        from = i + 2;
    }
    (line, "")
}

/// "Exhaust - {5}{B}: ..." -> "exhaust — {5}{b}: ...": labels written with an ASCII hyphen.
fn normalize_label(line: &str) -> String {
    if !line.contains(" — ") {
        if let Some(i) = line.find(" - ") {
            let label = &line[..i];
            if label.split_whitespace().count() <= 3 && !label.contains(':') && !label.contains('{')
            {
                return format!("{} — {}", label, &line[i + 3..]);
            }
        }
    }
    line.to_string()
}

fn read_trigger(trigger: &str) -> Vec<When> {
    let t = trigger;
    let mut w = Vec::new();
    if t.starts_with("at the beginning of") || t.starts_with("at end of") {
        w.push(When::EachTurn);
        return w;
    }
    // The card itself is the subject only when the trigger opens with it ("whenever a
    // creature dealt damage by ~ this turn dies" is about other creatures).
    let self_subject = t.starts_with("when ~")
        || t.starts_with("whenever ~")
        || t.starts_with("when this")
        || t.starts_with("whenever this")
        || t.starts_with("when equipped")
        || t.starts_with("whenever equipped")
        || t.starts_with("when enchanted")
        || t.starts_with("whenever enchanted");
    if t.contains("teamwork") {
        return vec![When::Event(Event::Other(t.to_string()))];
    }
    if self_subject && t.contains(" enter") {
        w.push(When::Enters);
    }
    // "When Tom, Bert, and William die" (a plural name).
    if self_subject
        && (t.contains(" dies") || t.ends_with(" die") || t.contains("is put into a graveyard"))
    {
        w.push(When::Dies);
    }
    if self_subject && (t.contains(" attacks") || t.contains("attacks or blocks")) {
        w.push(When::Attacks);
    }
    if self_subject && t.contains("deals combat damage") {
        w.push(When::CombatDamage);
    }
    // "Whenever ~ becomes tapped": attacking, or tapping it for an ability or a cost.
    if self_subject && t.contains("becomes tapped") && w.is_empty() {
        return vec![When::Event(Event::Other(t.to_string()))];
    }
    // "Whenever ~ or another creature you control enters/dies": also every other one.
    if self_subject && t.contains("or another") {
        if t.contains(" enter") {
            w.push(When::Event(Event::OtherEnters(t.to_string())));
        } else if t.contains(" die") {
            w.push(When::Event(Event::Death));
        }
    }
    if t.starts_with("whenever you attack") {
        return vec![When::Event(Event::OthersAttack)];
    }
    // "When you cast this spell, copy it": part of casting the spell, not an engine.
    if t.contains("you cast ~") || t.contains("you cast this spell") {
        return vec![When::Spell];
    }
    if !w.is_empty() {
        return w;
    }
    if t.contains("become tapped") || t.contains("becomes tapped") {
        return vec![When::Event(Event::OthersAttack)];
    }
    let event = if t.contains("you cast") || t.contains("you copy") {
        let what = t.split("you cast").nth(1).unwrap_or("").trim().to_string();
        Event::Cast(what)
    } else if t.contains("land you control enters") || t.contains("lands you control enter") {
        Event::Landfall
    } else if t.contains("enter") {
        Event::OtherEnters(t.to_string())
    } else if t.contains("dies")
        || t.contains("die")
        || t.contains("put into a graveyard")
        || t.contains("put into your graveyard")
    {
        Event::Death
    } else if t.contains("gain life") {
        Event::GainLife
    } else if t.contains("draw") {
        Event::Draw
    } else if t.contains("discard") {
        Event::Discard
    } else if t.contains("dealt damage") {
        Event::Damaged
    } else if t.contains("counter") {
        Event::Counters
    } else if t.contains("sacrifice") {
        Event::Sacrifice
    } else if t.contains("token") {
        Event::Tokens
    } else if t.contains("becomes the target") {
        Event::Targeted
    } else if t.contains("attack") {
        Event::OthersAttack
    } else {
        Event::Other(t.to_string())
    };
    vec![When::Event(event)]
}

/// Reads one line (not a keyword line) into an ability.
fn read_line(line: &str, card: &Card, is_spell: bool) -> Option<Ability> {
    let mut line = normalize_label(line.trim());
    // Ability words ("Landfall — Whenever ...") only label the ability (CR 207.2c); MSH also
    // uses flavour labels ("Radar Sense — ...").
    let mut power_up = false;
    // Infinity Stones' "∞ —" abilities only work once the stone is harnessed.
    let mut harness = false;
    // Exhaust (activate only once) and Boast (only if it attacked this turn).
    let mut exhaust = false;
    let mut boast = false;
    if let Some((label, rest)) = line.split_once(" — ") {
        let label = label.trim();
        harness = label == "∞";
        exhaust = label == "exhaust";
        boast = label == "boast";
        if label == "power-up" {
            power_up = true;
            line = rest.to_string();
        } else if label.split_whitespace().count() <= 4
            && !label.contains(':')
            && !label.contains('{')
        {
            // Saga chapters: "I —", "II, III —", "I, II, III, IV —" (one trigger per chapter).
            let chapters: Vec<u32> = label
                .split(", ")
                .map(|n| match n.trim() {
                    "i" => Some(1),
                    "ii" => Some(2),
                    "iii" => Some(3),
                    "iv" => Some(4),
                    "v" => Some(5),
                    "vi" => Some(6),
                    _ => None,
                })
                .collect::<Option<Vec<u32>>>()
                .unwrap_or_default();
            if !chapters.is_empty() {
                let mut effects: Vec<Effect> = sentences(rest)
                    .iter()
                    .flat_map(|s| read_effects(s))
                    .collect();
                body_effects(rest, &mut effects);
                return Some(Ability {
                    when: chapters.into_iter().map(When::Chapter).collect(),
                    effects,
                    optional: rest.contains("you may"),
                    conditional: rest.starts_with("if ") || rest.contains(" if "),
                    once: false,
                    modal: false,
                    instead: false,
                    modes: Vec::new(),
                    mode_texts: Vec::new(),
                    text: line.clone(),
                });
            }
            let _ = ABILITY_WORDS;
            line = rest.to_string();
        }
    }
    let mut ability = Ability {
        when: Vec::new(),
        effects: Vec::new(),
        optional: line.contains("you may"),
        conditional: false,
        // "Activate only once each turn" / "triggers only once each turn" still repeat every
        // turn; only Power-up, Exhaust and "activate only once" are one-shot.
        once: power_up
            || exhaust
            || line.contains("activate only once") && !line.contains("only once each turn"),
        modal: false,
        instead: false,
        modes: Vec::new(),
        mode_texts: Vec::new(),
        text: line.clone(),
    };
    let body: String;
    if let Some(rest) = line.strip_prefix('[') {
        // Loyalty ability: "[+1]: ..."
        let (cost, effect) = rest.split_once("]:")?;
        let cost = cost.replace('−', "-").replace('+', "");
        // "[-X]": a typical X.
        let n = cost
            .parse()
            .unwrap_or(if cost.trim() == "-x" { -TYPICAL_X } else { 0 });
        ability.when = vec![When::Loyalty(n)];
        body = effect.trim().to_string();
    } else if line.starts_with("when ") || line.starts_with("whenever ") || line.starts_with("at ")
    {
        let (trigger, rest) = split_trigger(&line);
        if trigger.starts_with("when you do")
            || trigger.starts_with("when the ") && trigger.contains("counter is put")
        {
            ability.when = vec![When::Reflexive];
        } else if is_spell && trigger.contains(" your next ") {
            // "When you cast your next instant or sorcery spell this turn, copy that spell"
            // on an instant: a delayed trigger the spell sets up as it resolves.
            ability.when = vec![When::Spell];
        } else {
            ability.when = read_trigger(trigger);
        }
        let mut rest = rest.to_string();
        // Intervening "if" clause (CR 603.4).
        if let Some(cond) = rest.strip_prefix("if ") {
            ability.conditional = true;
            rest = cond
                .split_once(", ")
                .map(|(_, r)| r.to_string())
                .unwrap_or_default();
        }
        if trigger.contains(" if ") {
            ability.conditional = true;
        }
        body = rest;
    } else if let Some((cost, effect)) = activated_split(&line) {
        let c = Cost::read(cost);
        ability.when = vec![if power_up {
            When::PowerUp(c)
        } else {
            When::Activated(c)
        }];
        body = effect.to_string();
    } else if is_spell {
        ability.when = vec![When::Spell];
        body = line.clone();
    } else {
        ability.when = vec![When::Static];
        ability.conditional = line.starts_with("as long as") || line.contains(" if ");
        body = line.clone();
    }
    // Conditions in the middle of the line: "has flying as long as...", "attacks while you
    // control...", "during your turn", "unless that player..." (the opponent picks the
    // cheaper option, so a punisher counts as conditional).
    // "For as long as you control ~" is how long it lasts, not a condition, and "during your
    // turn" in a trigger only says when it triggers.
    let cond_text = line.replace("for as long as", "");
    let punisher = [
        " unless that player",
        " unless its controller",
        " unless any player",
        " unless an opponent",
        " unless they ",
    ];
    if [
        " as long as ",
        " while you control",
        " while it's",
        " while ~",
        "threshold",
        "for the first time each turn",
    ]
    .iter()
    .chain(punisher.iter())
    .any(|c| cond_text.contains(c))
        || body.contains("during your turn")
        || boast
    {
        ability.conditional = true;
    }
    // Only an "if" that gates the whole ability counts (an intervening-if, set above, or a
    // body that opens with one); "if you do" / "if that creature..." in a later sentence
    // only affects the extra part.
    if body.starts_with("if ") || body.starts_with("as long as") {
        ability.conditional = true;
    }
    for s in sentences(&body) {
        for e in read_effects(&s) {
            // "Gain control of target creature. ... gain control of it" is one steal.
            if !(matches!(e, Effect::GainControl(_) | Effect::Rule(_))
                && ability.effects.contains(&e))
            {
                ability.effects.push(e);
            }
        }
    }
    body_effects(&body, &mut ability.effects);
    // "Choose up to that many target creatures you control. Put a +1/+1 counter on each of
    // them": "them" are the chosen targets.
    if let Some(i) = body.find("choose up to ").or_else(|| body.find("choose ")) {
        let rest = &body[i..];
        if let Some(j) = rest.find("target ") {
            let words = &rest[..j];
            let phrase = rest[j + 7..].split(['.', ';']).next().unwrap_or("");
            let count = if words.contains("two") {
                2
            } else if words.contains("three") || words.contains("that many") {
                3
            } else {
                1
            };
            for e in ability.effects.iter_mut() {
                for t in e.targets_mut() {
                    if t.mass && t.restr.has(crate::restrict::PREV) {
                        let mut n = read_target(phrase, false);
                        n.count = count;
                        *t = n;
                    }
                }
            }
        }
    }
    // Self-referential "enters prepared" lines are statics on the face.
    if ability.effects.is_empty() && line.contains("prepared") {
        ability.effects.push(Effect::Prepared);
    }
    if harness || line.contains("activate only if") {
        ability.conditional = true;
    }
    // An Aura's "...that player draws a card" goes to the enchanted creature's controller -
    // normally you (Super Intelligence).
    if card.type_line.contains("Aura") && line.contains("enchanted creature") {
        for e in ability.effects.iter_mut() {
            if let Effect::Draw { you, .. } = e {
                *you = true;
            }
        }
    }
    // Drawing that costs a card first ("{T}, Discard a card: Draw a card..."; "You may
    // sacrifice an artifact or discard a card. If you do, draw two cards") nets one card
    // fewer: the first draw only replaces what was paid.
    let costs_a_card = matches!(ability.when.first(), Some(When::Activated(c)) if c.discard)
        || (body.contains("discard a card") || body.contains("sacrifice an artifact"))
            && body.contains("if you do, draw")
        // "Draw a card. If you do, discard a card" / "Draw two cards. Then discard a card".
        || body.contains(". if you do, discard a card")
        || body.contains(". then discard a card");
    if costs_a_card && !ability.effects.contains(&Effect::Loot) {
        if let Some(pos) = ability
            .effects
            .iter()
            .position(|e| matches!(e, Effect::Draw { you: true, .. }))
        {
            let (n, x) = match &ability.effects[pos] {
                Effect::Draw { amount, .. } => (amount.n, amount.x),
                _ => (1, false),
            };
            // ("Draw a card for each card you've discarded this turn" stays a count: the
            // simulator counts it when it resolves.)
            ability.effects[pos] = if n <= 1 {
                Effect::Loot
            } else {
                Effect::Draw {
                    amount: Amount { n: n - 1, x },
                    you: true,
                }
            };
        }
    }
    // Boast's rules live in reminder text (stripped): keep them with the ability.
    if boast {
        ability
            .text
            .push_str(" activate only if ~ attacked this turn and only once each turn.");
    }
    let _ = card;
    Some(ability)
}

/// "{2}{B}, {T}: effect" -> ("{2}{B}, {T}", "effect"). Only when the part before ":" is a cost.
fn activated_split(line: &str) -> Option<(&str, &str)> {
    let (cost, effect) = line.split_once(": ")?;
    let c = cost.trim();
    let looks_like_cost = c.contains('{')
        || c.starts_with("sacrifice")
        || c.starts_with("discard")
        || c.starts_with("pay ")
        || c.starts_with("exile")
        || c.starts_with("remove")
        || c.starts_with("tap ")
        || c.starts_with("return");
    // A colon inside quotes is a granted ability ("... is a land with \"{T}: Add {C}\"").
    (looks_like_cost && !c.starts_with("when") && !c.contains('"') && c.len() < 130)
        .then_some((c, effect.trim()))
}

/// Splits at ". ", but not inside a quoted ability ("... with \"Sacrifice this. Draw a
/// card.\"" stays whole), and after a quote that ends a sentence ("if able.\" Put ...").
fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut inside = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        cur.push(ch);
        let next_space = chars.peek() == Some(&' ');
        if ch == '"' {
            inside = !inside;
            if !inside && next_space && cur.ends_with(".\"") {
                out.push(std::mem::take(&mut cur));
            }
        } else if ch == '.' && !inside && next_space {
            out.push(std::mem::take(&mut cur));
        }
    }
    out.push(cur);
    out.iter()
        .map(|s| s.trim().trim_end_matches('.').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

impl When {
    /// Plain words for when an ability happens.
    pub fn describe(&self) -> String {
        match self {
            When::Spell => "When cast".into(),
            When::Enters => "When it enters".into(),
            When::Dies => "When it dies".into(),
            When::Attacks => "When it attacks".into(),
            When::CombatDamage => "When it deals combat damage".into(),
            When::EachTurn => "Every turn".into(),
            When::Event(e) => match e {
                Event::Cast(w) => format!(
                    "Whenever you cast {}",
                    if w.is_empty() { "a spell" } else { w }
                ),
                Event::OtherEnters(_) => "Whenever another permanent enters".into(),
                Event::Landfall => "Landfall".into(),
                Event::Death => "Whenever a creature dies".into(),
                Event::GainLife => "Whenever you gain life".into(),
                Event::Draw => "Whenever you draw (2nd card)".into(),
                Event::Discard => "Whenever you discard".into(),
                Event::Damaged => "Whenever it's dealt damage".into(),
                Event::Counters => "Whenever counters are put".into(),
                Event::Sacrifice => "Whenever you sacrifice".into(),
                Event::Tokens => "Whenever tokens enter".into(),
                Event::Targeted => "Whenever it becomes a target".into(),
                Event::OthersAttack => "Whenever your creatures attack".into(),
                Event::Other(t) => format!("On \"{}\"", t),
            },
            When::Activated(c) => format!("Activated ({})", c.describe()),
            When::PowerUp(c) => format!(
                "Power-up ({}, once; cheaper the turn it enters)",
                c.describe()
            ),
            When::Loyalty(n) => format!("Loyalty [{:+}]", n),
            When::Chapter(n) => format!("Chapter {}", n),
            When::Reflexive => "Then (when you do)".into(),
            When::Static => "Always".into(),
        }
    }
}

impl Cost {
    fn describe(&self) -> String {
        let mut parts = Vec::new();
        if self.mana > 0 {
            parts.push(format!("{} mana", self.mana));
        }
        if self.tap {
            parts.push("tap".into());
        }
        if self.sacrifice {
            parts.push("sacrifice".into());
        }
        if self.discard {
            parts.push("discard".into());
        }
        if self.life {
            parts.push("life".into());
        }
        if parts.is_empty() {
            "free".into()
        } else {
            parts.join(" + ")
        }
    }
}

impl Effect {
    /// The targets of an effect, to adjust them.
    pub fn targets_mut(&mut self) -> Vec<&mut Target> {
        match self {
            Effect::Destroy(t)
            | Effect::Fight(t)
            | Effect::Bounce(t)
            | Effect::Untap(t)
            | Effect::GainControl(t) => vec![t],
            Effect::Exile { target, .. } | Effect::Lock { target, .. } => vec![target],
            Effect::Damage { to, .. }
            | Effect::Shrink { to, .. }
            | Effect::PlusCounters { to, .. }
            | Effect::Pump { to, .. }
            | Effect::Grant { to, .. } => vec![to],
            _ => Vec::new(),
        }
    }

    /// Plain words for what an effect does.
    pub fn describe(&self) -> String {
        let what = |t: &Target| {
            let k = match t.kind {
                Kind::Creature => "creature",
                Kind::CreatureOrPlaneswalker => "creature/planeswalker",
                Kind::NonlandPermanent => "nonland permanent",
                Kind::Permanent => "permanent",
                Kind::Artifact => "artifact",
                Kind::Enchantment => "enchantment",
                Kind::ArtifactOrEnchantment => "artifact/enchantment",
                Kind::Planeswalker => "planeswalker",
                Kind::Land => "land",
                Kind::AnyTarget => "any target",
                Kind::Player => "player",
                Kind::Spell => "spell",
                Kind::It => return "itself".to_string(),
                Kind::Other => "thing",
            };
            let article = match (t.mass, t.whose) {
                (true, Whose::Yours) => "each of your",
                (true, Whose::Theirs) => "each opponent's",
                (true, Whose::Any) => "each",
                (false, Whose::Yours) => "one of your",
                (false, Whose::Theirs) => "an opponent's",
                (false, Whose::Any) => "a",
            };
            format!(
                "{} {}{}{}",
                article,
                if t.other { "other " } else { "" },
                k,
                if t.restricted {
                    " (with a condition)"
                } else {
                    ""
                }
            )
        };
        match self {
            Effect::Destroy(t) => format!("destroy {}", what(t)),
            Effect::Exile { target, until } => format!(
                "exile {}{}",
                what(target),
                if *until { " until it leaves" } else { "" }
            ),
            Effect::Damage { amount, to } => format!("{} damage to {}", amount.n, what(to)),
            Effect::Fight(t) => format!("fight {}", what(t)),
            Effect::Shrink {
                amount,
                to,
                until_eot,
            } => format!(
                "-{}/-{} to {}{}",
                amount.n,
                amount.n,
                what(to),
                if *until_eot { " this turn" } else { "" }
            ),
            Effect::Bounce(t) => format!("return {} to hand", what(t)),
            Effect::Lock { target, lasting } => format!(
                "{} {}",
                if *lasting { "lock down" } else { "tap" },
                what(target)
            ),
            Effect::Edict { each_opponent } => format!(
                "{} sacrifices a creature",
                if *each_opponent {
                    "each opponent"
                } else {
                    "a player"
                }
            ),
            Effect::CounterSpell => "counter a spell".into(),
            Effect::Draw { amount, you } => format!(
                "{} draws {}",
                if *you { "you" } else { "a player" },
                amount.n
            ),
            Effect::Loot => "loot (draw, then discard)".into(),
            Effect::Discard { .. } => "opponent discards".into(),
            Effect::Mill { amount } => format!("mill {}", amount.n),
            Effect::Scry(n) => format!("scry/surveil {}", n.n),
            Effect::Tokens {
                amount,
                power,
                toughness,
                keywords,
            } => {
                format!(
                    "{} {}/{} token(s){}",
                    amount.n,
                    power,
                    toughness,
                    if keywords.is_empty() {
                        String::new()
                    } else {
                        format!(" with {}", keywords.join(", "))
                    }
                )
            }
            Effect::ItemTokens { amount, kind } => format!("{} {} token(s)", amount.n, kind),
            Effect::PlusCounters { amount, to } => {
                format!("{} +1/+1 counter(s) on {}", amount.n, what(to))
            }
            Effect::Pump {
                power,
                toughness,
                to,
                until_eot,
            } => format!(
                "+{}/+{} to {}{}",
                power,
                toughness,
                what(to),
                if *until_eot { " this turn" } else { "" }
            ),
            Effect::Grant {
                keywords,
                to,
                until_eot,
            } => format!(
                "{} gets {}{}",
                what(to),
                keywords.join(", "),
                if *until_eot { " this turn" } else { "" }
            ),
            Effect::GainLife(n) => format!("gain {} life", n.n),
            Effect::Drain { amount, each } => format!(
                "{} loses {} life",
                if *each {
                    "each opponent"
                } else {
                    "an opponent"
                },
                amount.n
            ),
            Effect::AddMana(n) => format!("add {} mana", n.n),
            Effect::FetchLand { battlefield } => {
                if *battlefield {
                    "put a land onto the battlefield".into()
                } else {
                    "fetch a land to hand".into()
                }
            }
            Effect::Reanimate => "return a creature from the graveyard to play".into(),
            Effect::Regrowth => "return a card from the graveyard to hand".into(),
            Effect::Untap(_) => "untap".into(),
            Effect::GainControl(t) => format!("gain control of {}", what(t)),
            Effect::Copy => "copy".into(),
            Effect::Empower(n) => format!("empower Jace {}", n.n),
            Effect::Amass(n) => format!("amass {}", n.n),
            Effect::Goad => "goad".into(),
            Effect::Proliferate => "proliferate".into(),
            Effect::ExtraCombat => "extra combat".into(),
            Effect::Blink => "flicker".into(),
            Effect::Tutor => "search for a card".into(),
            Effect::FreeSpell => "cast/put a card for free".into(),
            Effect::Prepared => "prepared (cast a copy of its spell)".into(),
            Effect::Attach => "attach".into(),
            Effect::Doubler(k) => format!("doubles {}", k),
            Effect::Fog => "prevent combat damage".into(),
            Effect::Dig => "dig for a card".into(),
            Effect::OtherCounters(k) => format!("{} counter", k),
            Effect::Recast => "cast a card from the graveyard".into(),
            Effect::Rule(label) => format!("special rule: {}", label),
        }
    }
}

impl Parsed {
    /// One plain-language line per ability, for showing how the app reads a card.
    pub fn describe(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.keywords.is_empty() {
            out.push(format!("Keywords: {}", self.keywords.join(", ")));
        }
        let mut push = |a: &Ability, prefix: &str| {
            let when = a
                .when
                .iter()
                .map(When::describe)
                .collect::<Vec<_>>()
                .join(" / ");
            let effects: Vec<String> = a.effects.iter().map(Effect::describe).collect();
            let flags = [
                (a.conditional, "if"),
                (a.optional, "may"),
                (a.modal, "choose"),
            ]
            .iter()
            .filter(|f| f.0)
            .map(|f| f.1)
            .collect::<Vec<_>>();
            if effects.is_empty() {
                return;
            }
            out.push(format!(
                "{}{}{}: {}",
                prefix,
                when,
                if flags.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", flags.join(", "))
                },
                effects.join("; ")
            ));
        };
        for a in &self.abilities {
            push(a, "");
        }
        if let Some(b) = &self.back {
            for a in &b.abilities {
                push(a, "Back face - ");
            }
        }
        out
    }
}

/// How good a card's removal is, from the best removal ability it has.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Removal {
    /// Points for card scoring (3 = unconditional destroy/exile of any creature).
    pub points: f32,
    pub label: &'static str,
    /// Hits several creatures (a board wipe).
    pub mass: bool,
    /// Only hits opponents' creatures (one-sided wipe or "an opponent controls").
    pub one_sided: bool,
    /// Happens repeatedly (activated or triggered every turn).
    pub repeatable: bool,
}

/// Grades one removal effect (None if it isn't removal of opponents' creatures).
fn grade(effect: &Effect) -> Option<(f32, &'static str, bool, bool)> {
    let (points, label, t) = match effect {
        Effect::Destroy(t) | Effect::Exile { target: t, .. } if t.hits_their_creatures() => {
            let until = matches!(effect, Effect::Exile { until: true, .. });
            if t.restricted {
                (2.2, "conditional removal", t)
            } else if until {
                (2.6, "exile until it leaves", t)
            } else if matches!(effect, Effect::Exile { .. }) {
                (3.1, "unconditional exile", t)
            } else {
                (3.0, "unconditional destroy", t)
            }
        }
        Effect::Damage { amount, to } if to.hits_their_creatures() => {
            let n = amount.value();
            ((0.9 + 0.45 * n).min(2.8), "damage-based removal", to)
        }
        Effect::Fight(t) if t.hits_their_creatures() => (1.7, "fight", t),
        Effect::Shrink {
            amount,
            to,
            until_eot,
        } if to.hits_their_creatures() => {
            let n = amount.value();
            let base = (0.9 + 0.5 * n).min(2.8);
            (
                if *until_eot { base } else { base + 0.2 },
                "-X/-X removal",
                to,
            )
        }
        Effect::Bounce(t) if t.hits_their_creatures() => (1.0, "bounce", t),
        Effect::Lock { target, lasting } if target.hits_their_creatures() => (
            if *lasting { 1.6 } else { 0.5 },
            if *lasting {
                "lockdown (pseudo-removal)"
            } else {
                "tap effect"
            },
            target,
        ),
        Effect::Edict { each_opponent } => {
            return Some((
                if *each_opponent { 2.4 } else { 1.8 },
                "edict (they choose)",
                *each_opponent,
                true,
            ));
        }
        Effect::GainControl(t) if t.hits_their_creatures() => (3.0, "steals a creature", t),
        _ => return None,
    };
    Some((points, label, t.mass, t.whose == Whose::Theirs))
}

impl Parsed {
    /// The card's best removal, graded (None if it has none).
    pub fn removal(&self) -> Option<Removal> {
        let mut best: Option<Removal> = None;
        for a in self.all_abilities() {
            if matches!(a.when.first(), Some(When::Static))
                && !a.text.starts_with("enchanted creature")
                && !a.text.starts_with("enchanted permanent")
            {
                continue;
            }
            // How usable the ability is: spells and enter triggers fully; repeatable ones
            // more; once-only or sacrifice-cost ones less.
            let rate = a.rate();
            let factor = if a.when.iter().any(|w| matches!(w, When::PowerUp(_))) {
                0.6
            } else if a
                .when
                .iter()
                .any(|w| matches!(w, When::Activated(c) if c.sacrifice))
            {
                0.7
            } else if rate > 0.0 {
                1.0 + 0.6 * rate.min(1.0)
            } else if a.conditional {
                0.7
            } else {
                1.0
            };
            for e in &a.effects {
                // "Gain control of target creature until end of turn. Untap it. It gains
                // haste": a one-turn swing (Threaten), not a lasting answer.
                let temporary = matches!(e, Effect::GainControl(_))
                    && (a.text.contains("until end of turn") || a.text.contains("this turn"));
                let graded = if temporary {
                    grade(e)
                        .map(|(_, _, mass, one_sided)| (0.6, "temporary steal", mass, one_sided))
                } else {
                    grade(e)
                };
                if let Some((pts, label, mass, one_sided)) = graded {
                    // "For each opponent, destroy up to one target creature that player
                    // controls" answers a creature from every opponent.
                    let per_opponent = if a.text.contains("for each opponent") {
                        1.4
                    } else {
                        1.0
                    };
                    let pts = pts * factor * per_opponent * if a.modal { 0.9 } else { 1.0 };
                    if best.is_none_or(|b| pts > b.points) {
                        best = Some(Removal {
                            points: pts,
                            label,
                            mass,
                            one_sided,
                            repeatable: rate > 0.0,
                        });
                    }
                }
            }
        }
        best
    }

    /// Destroys, exiles, damages or shrinks several creatures at once.
    pub fn sweeper(&self) -> bool {
        self.effects().any(|(_, e)| match e {
            Effect::Destroy(t) | Effect::Exile { target: t, .. } | Effect::Bounce(t) => {
                t.mass
                    && matches!(
                        t.kind,
                        Kind::Creature
                            | Kind::NonlandPermanent
                            | Kind::Permanent
                            | Kind::CreatureOrPlaneswalker
                    )
                    && t.whose != Whose::Yours
            }
            Effect::Damage { to, .. } | Effect::Shrink { to, .. } => {
                to.mass
                    && to.whose != Whose::Yours
                    && matches!(to.kind, Kind::Creature | Kind::CreatureOrPlaneswalker)
            }
            _ => false,
        })
    }

    /// Hits every opponent: drains, damage to each opponent, discard, edicts.
    pub fn each_opponent(&self) -> bool {
        self.effects().any(|(a, e)| {
            matches!(
                e,
                Effect::Drain { each: true, .. }
                    | Effect::Edict {
                        each_opponent: true
                    }
                    | Effect::Discard { opponents: true }
            ) || (a.text.contains("for each opponent") && grade(e).is_some())
        })
    }

    /// Real card advantage (not looting): draws for you, Clues, Jace empowering.
    pub fn draws(&self) -> bool {
        self.effects().any(|(_, e)| {
            matches!(e, Effect::Draw { you: true, .. } | Effect::Empower(_))
                || matches!(e, Effect::ItemTokens { kind, .. } if kind == "clue")
        })
    }

    pub fn ramps(&self) -> bool {
        self.effects().any(|(a, e)| {
            matches!(e, Effect::FetchLand { battlefield: true })
                || matches!(e, Effect::AddMana(_))
                    && !a.text.contains("spend this mana only")
                    && !a.text.contains("can't be spent")
                // A Treasure bought with mana ("{2}, {T}: create a Treasure") stores mana
                // rather than adding it.
                || matches!(e, Effect::ItemTokens { kind, .. }
                    if kind == "treasure" || kind == "lotus" || kind == "heartwood")
                    && !a.when.iter().any(|w| matches!(w, When::Activated(c) if c.mana > 0))
        })
    }

    /// Creature tokens, Armies, and Treasure/Food/Clue/copy tokens ("tokens you control"
    /// payoffs count them all).
    pub fn makes_tokens(&self) -> bool {
        self.effects().any(|(a, e)| match e {
            Effect::Tokens { .. } | Effect::Amass(_) => true,
            // "They create a Treasure token" (Vraska): the opponent's.
            Effect::ItemTokens { .. } => !a.text.contains(". they create "),
            _ => false,
        })
    }

    /// Creature tokens specifically (bodies on the board).
    pub fn makes_creature_tokens(&self) -> bool {
        self.effects()
            .any(|(_, e)| matches!(e, Effect::Tokens { .. } | Effect::Amass(_)))
    }

    pub fn evasive(&self) -> bool {
        ["flying", "trample", "menace", "skulk", "fear", "intimidate"]
            .iter()
            .any(|k| self.has_keyword(k))
            || self.abilities.iter().any(|a| {
                a.when.first() == Some(&When::Static) && a.text.contains("~ can't be blocked")
            })
    }

    pub fn protected(&self) -> bool {
        ["hexproof", "indestructible", "ward", "protection", "shroud"].iter().any(|k| self.has_keyword(k))
            || self.effects().any(|(_, e)| {
                matches!(e, Effect::Grant { keywords, .. } if keywords.iter().any(|k| k == "hexproof" || k == "indestructible" || k.starts_with("protection")))
            })
    }

    pub fn gains_life(&self) -> bool {
        self.has_keyword("lifelink")
            || self
                .effects()
                .any(|(_, e)| matches!(e, Effect::GainLife(_)))
    }

    /// Any triggered or activated ability that happens over and over.
    pub fn repeatable_value(&self) -> f32 {
        self.abilities
            .iter()
            .filter(|a| !a.effects.is_empty())
            .map(|a| a.rate())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::CardDb;

    fn db() -> CardDb {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
        CardDb::load(&dir.join("cards.json")).unwrap()
    }

    fn p(db: &CardDb, name: &str) -> Parsed {
        parse(
            db.get(name)
                .unwrap_or_else(|| panic!("{} not in the database", name)),
        )
    }

    /// Hand-checked against each card's printed text.
    #[test]
    fn reads_league_cards_correctly() {
        let db = db();
        let removal = |n: &str| p(&db, n).removal().map(|r| (r.points, r.label));
        let has = |n: &str, f: &dyn Fn(&Effect) -> bool| p(&db, n).effects().any(|(_, e)| f(e));
        let when = |n: &str, f: &dyn Fn(&When) -> bool| {
            p(&db, n).abilities.iter().any(|a| a.when.iter().any(f))
        };

        // --- Staples, Skill Tree and Shop
        assert!(p(&db, "Sol Ring").ramps(), "Sol Ring adds mana");
        assert!(
            has("Sol Ring", &|e| matches!(e, Effect::AddMana(a) if a.n == 2)),
            "Sol Ring adds two"
        );
        assert!(p(&db, "Arcane Signet").ramps());
        assert!(
            p(&db, "Mind Stone").ramps() && p(&db, "Mind Stone").draws(),
            "Mind Stone: mana, sac to draw"
        );
        let sac = p(&db, "Mind Stone")
            .abilities
            .iter()
            .find(|a| a.effects.iter().any(|e| matches!(e, Effect::Draw { .. })))
            .unwrap()
            .rate();
        assert_eq!(sac, 0.0, "sacrificing Mind Stone to draw is one-shot");
        assert!(p(&db, "Commander's Sphere").ramps());
        assert!(
            p(&db, "Cultivate").ramps(),
            "Cultivate puts a land onto the battlefield"
        );
        assert_eq!(
            removal("Swords to Plowshares").map(|r| r.1),
            Some("unconditional exile")
        );
        assert_eq!(
            removal("Beast Within").map(|r| r.1),
            Some("unconditional destroy")
        );
        assert!(
            !p(&db, "Beast Within").makes_tokens(),
            "Beast Within's token goes to the opponent"
        );
        assert!(!p(&db, "Pongify").makes_tokens());
        assert!(
            removal("Chaos Warp").is_some(),
            "Chaos Warp shuffles a permanent away"
        );
        assert_eq!(
            removal("Feed the Swarm").map(|r| r.1),
            Some("unconditional destroy")
        );
        assert!(
            removal("Vandalblast").is_none(),
            "Vandalblast only hits artifacts"
        );
        assert!(p(&db, "Blasphemous Act").sweeper());
        assert!(p(&db, "Crux of Fate").sweeper());
        assert!(p(&db, "Austere Command").sweeper());
        assert!(has("Counterspell", &|e| matches!(e, Effect::CounterSpell)));
        assert!(has("Lull", &|e| matches!(e, Effect::Fog)));
        assert!(p(&db, "Lull").has_keyword("cycling"));
        assert!(p(&db, "Phyrexian Arena").draws());
        assert_eq!(
            p(&db, "Phyrexian Arena").abilities[0].when,
            vec![When::EachTurn]
        );
        assert!(p(&db, "Swiftfoot Boots").has_keyword("equip"));
        assert!(has(
            "Swiftfoot Boots",
            &|e| matches!(e, Effect::Grant { keywords, .. } if keywords.contains(&"hexproof".to_string()))
        ));
        assert!(has(
            "Rogue's Passage",
            &|e| matches!(e, Effect::Grant { keywords, .. } if keywords.contains(&"unblockable".to_string()))
        ));
        assert!(has(
            "Monologue Tax",
            &|e| matches!(e, Effect::ItemTokens { kind, .. } if kind == "treasure")
        ));

        // --- Marvel Super Heroes
        let thanos = p(&db, "Thanos, the Mad Titan");
        assert!(thanos.has_keyword("deathtouch") && thanos.has_keyword("lifelink"));
        assert!(
            thanos
                .abilities
                .iter()
                .any(|a| a.once && matches!(a.when[0], When::PowerUp(_))),
            "Thanos's power-up is once-only"
        );
        assert!(
            thanos.sweeper(),
            "Thanos's power-up destroys each other creature of a parity"
        );
        let hydra = p(&db, "Madame Hydra");
        assert!(
            matches!(&hydra.abilities[0].when[0], When::Event(Event::Cast(w)) if w.contains("villain"))
        );
        assert!(hydra.makes_tokens());
        let whip = p(&db, "Whiplash, Vengeful Engineer");
        let a = whip
            .abilities
            .iter()
            .find(|a| a.is(&When::Attacks))
            .expect("Whiplash attack trigger");
        assert!(a.conditional, "only if he's equipped");
        assert!(p(&db, "Whiplash, Vengeful Engineer").each_opponent());
        assert_eq!(
            removal("Abomination, Terrifying Titan").map(|r| r.1),
            Some("fight")
        );
        assert!(when(
            "Aerial Doombot",
            &|w| matches!(w, When::PowerUp(c) if c.mana == 6)
        ));
        assert!(p(&db, "Cruel Alliance").has_keyword("teamwork"));
        assert_eq!(p(&db, "Cruel Alliance").keyword_value("teamwork"), Some(2));
        assert!(removal("Cruel Alliance").is_some());
        assert!(p(&db, "Atlantis Attacks").makes_tokens());
        assert!(has(
            "Claim the Kingdom",
            &|e| matches!(e, Effect::OtherCounters(k) if k == "plan")
        ));
        assert!(when("Claim the Kingdom", &|w| matches!(
            w,
            When::Event(Event::Landfall)
        )));
        assert!(when("Claim the Kingdom", &|w| matches!(w, When::Reflexive)));
        assert!(when("Construct a Cosmic Cube", &|w| matches!(
            w,
            When::Event(Event::Draw)
        )));
        assert_eq!(
            removal("Web Up").map(|r| r.1),
            Some("exile until it leaves")
        );
        assert!(
            has("A.I.M. Scientists", &|e| matches!(e, Effect::Loot)),
            "connive is looting"
        );
        assert!(
            !p(&db, "A.I.M. Scientists").draws(),
            "conniving isn't card advantage"
        );
        assert!(p(&db, "A.I.M. Scientists").has_keyword("basic landcycling"));
        assert!(
            when("Red Hulk", &|w| matches!(w, When::Event(Event::Damaged))),
            "Red Hulk is an Enrage card"
        );
        assert!(
            p(&db, "Red Hulk").has_keyword("trample") && p(&db, "Red Hulk").has_keyword("reach")
        );

        // --- The Hobbit
        let azog = p(&db, "Azog, Moria's Ruin");
        assert!(azog.abilities[0].is(&When::Enters));
        assert_eq!(
            removal("Azog, Moria's Ruin").map(|r| r.1),
            Some("unconditional destroy")
        );
        assert!(
            !azog.abilities[0].conditional,
            "the 'if' only gates the card draw"
        );
        assert!(p(&db, "Balin, Loremaster").has_keyword("storied"));
        let bard = p(&db, "Bard's Company");
        assert!(
            bard.abilities
                .iter()
                .any(|a| a.is(&When::Enters) && a.is(&When::Attacks)),
            "enters or attacks"
        );
        assert!(has(
            "Bard's Company",
            &|e| matches!(e, Effect::Pump { to, until_eot: false, .. } if to.mass && to.other)
        ));
        assert!(has("Along the Crooked Way", &|e| matches!(
            e,
            Effect::Regrowth
        )));
        assert!(has("Along the Crooked Way", &|e| matches!(
            e,
            Effect::Amass(_)
        )));
        let bilbo = p(&db, "Bilbo Baggins, Burglar");
        assert!(bilbo.draws());
        assert!(
            bilbo
                .back
                .as_ref()
                .is_some_and(|b| b.abilities.iter().any(|a| a.is(&When::Spell))),
            "Adventure half is a spell"
        );

        // --- Reality Fracture
        assert!(has(
            "Arcane Amphisbaena",
            &|e| matches!(e, Effect::Empower(a) if a.n == 2)
        ));
        assert!(p(&db, "Arcane Amphisbaena").has_keyword("deathtouch"));
        assert!(has(
            "Way of the Deathbringer",
            &|e| matches!(e, Effect::Empower(a) if a.n == 5)
        ));
        let angel = p(&db, "Blossom-Blessed Angel");
        assert!(has("Blossom-Blessed Angel", &|e| matches!(
            e,
            Effect::Prepared
        )));
        assert!(
            angel.back.is_some(),
            "prepared spell read from the back face"
        );
        assert!(when("Bloodline Recollector", &|w| matches!(
            w,
            When::EachTurn
        )));
        let ajani = p(&db, "Ajani Unrelenting");
        assert!(
            ajani
                .abilities
                .iter()
                .any(|a| matches!(a.when[0], When::Loyalty(-3)))
        );
        assert!(
            ajani.sweeper(),
            "-3 damages each creature except your tokens"
        );
        assert!(removal("Garruk, Veiled Butcher").is_some());

        // --- League-wide misreadings (hand-checked against the printed text)
        let ability_with = |n: &str, f: &dyn Fn(&Effect) -> bool| {
            p(&db, n)
                .all_abilities()
                .find(|a| a.effects.iter().any(f))
                .cloned()
                .unwrap_or_else(|| panic!("{}: no such ability", n))
        };
        // Short self-names and "~ or another" triggers.
        assert!(when("Kiora of Fire and Ashes", &|w| *w == When::Enters));
        assert!(when("Dáin Ironfoot", &|w| *w == When::Enters));
        assert!(when("Dáin Ironfoot", &|w| *w == When::Attacks));
        assert!(when("Arahbo, the First Fang", &|w| *w == When::Enters));
        assert!(when("Arahbo, the First Fang", &|w| matches!(
            w,
            When::Event(Event::OtherEnters(_))
        )));
        assert!(p(&db, "Ghost, Spectral Saboteur").evasive());
        // Tokens an opponent gets aren't yours.
        assert!(!p(&db, "The Sentry, Golden Guardian").makes_tokens());
        assert!(!p(&db, "Vraska, the Cutting Glare").makes_tokens());
        // "Triggers only once each turn" repeats; Exhaust is once.
        assert!(
            !ability_with("Loki, God of Mischief", &|e| matches!(
                e,
                Effect::Draw { .. }
            ))
            .once
        );
        assert!(
            !ability_with("Ant-Man, Colony Commander", &|e| matches!(
                e,
                Effect::Tokens { .. }
            ))
            .once
        );
        assert!(ability_with("Liliana the Repentant", &|e| matches!(e, Effect::Reanimate)).once);
        // Later "If ..." sentences only gate their own effect.
        let roil = ability_with("Into the Roil", &|e| matches!(e, Effect::Draw { .. }));
        assert!(roil.conditional, "Into the Roil draws only if kicked");
        assert!(!ability_with("Into the Roil", &|e| matches!(e, Effect::Bounce(_))).conditional);
        assert!(
            ability_with("Angel of Vitality", &|e| matches!(e, Effect::Pump { .. })).conditional
        );
        // Saga chapter lists and [-X].
        assert!(when("Burn, Burn, Tree and Fern", &|w| *w == When::Chapter(4)));
        assert!(when("Chandra, Chill of Compliance", &|w| *w == When::Loyalty(-3)));
        // Emblems and granted abilities aren't the card's own.
        assert!(!has("Ajani Resolute", &|e| matches!(
            e,
            Effect::Pump { .. }
        )));
        assert!(!has("Undying Malice", &|e| matches!(e, Effect::Reanimate)));
        assert!(
            has("Iron Fist, Living Weapon", &|e| matches!(
                e,
                Effect::Damage { .. }
            )),
            "Iron Fist grants itself the ability"
        );
        assert!(
            p(&db, "New Horizons").ramps(),
            "a mana ability on your own land"
        );
        assert!(removal("Imprisoned in the Moon").is_some_and(|r| r.1.starts_with("lockdown")));
        // Partial evasion isn't unblockable.
        for n in ["Juggernaut", "Stromkirk Noble", "Gate Colossus"] {
            assert!(
                !has(
                    n,
                    &|e| matches!(e, Effect::Grant { keywords, .. } if keywords.contains(&"unblockable".to_string()))
                ),
                "{}",
                n
            );
        }
        assert!(has(
            "Tetsuko Umezawa, Fugitive",
            &|e| matches!(e, Effect::Grant { to, .. } if to.mass && to.restricted)
        ));
        // Draw-then-discard nets.
        assert!(
            !p(&db, "Seasoned Cryomancer").abilities[0]
                .effects
                .iter()
                .any(|e| matches!(e, Effect::Draw { .. }))
        );
        assert!(
            !p(&db, "Kiora, the Rising Tide")
                .abilities
                .iter()
                .any(|a| a.is(&When::Enters)
                    && a.effects.iter().any(|e| matches!(e, Effect::Draw { .. })))
        );
        assert!(has(
            "Thranduil, the Elvenking",
            &|e| matches!(e, Effect::Draw { amount, .. } if amount.n == 1)
        ));
        // Counts: "for each", "where X is", several targets.
        assert!(has(
            "Brass's Bounty",
            &|e| matches!(e, Effect::ItemTokens { amount, .. } if amount.x && amount.n >= 3)
        ));
        assert!(has(
            "Krenko, Mob Boss",
            &|e| matches!(e, Effect::Tokens { amount, .. } if amount.n >= 3)
        ));
        assert!(has(
            "Doc Samson, Super Psychiatrist",
            &|e| matches!(e, Effect::AddMana(a) if a.n == 3)
        ));
        assert!(has("Master's Councillors", &|e| matches!(
            e,
            Effect::Pump { power: 6, .. }
        )));
        assert!(has(
            "Gandalf, Spark Starter",
            &|e| matches!(e, Effect::Damage { to, .. } if to.count == 3)
        ));
        assert!(has(
            "Death to Our Enemies",
            &|e| matches!(e, Effect::Damage { to, .. } if to.count == 2)
        ));
        assert!(has(
            "Run Away Together",
            &|e| matches!(e, Effect::Bounce(t) if t.count == 2)
        ));
        // Locks, steals, tucks, flickers.
        assert!(has("Spider-Woman, Secret Agent", &|e| matches!(
            e,
            Effect::Lock { lasting: true, .. }
        )));
        assert!(
            !has("Sower of Chaos", &|e| matches!(e, Effect::Lock { .. })),
            "can't block isn't removal"
        );
        assert_eq!(
            removal("Evil's Thrall").map(|r| r.1),
            Some("temporary steal")
        );
        assert_eq!(
            removal("Involuntary Employment").map(|r| r.1),
            Some("temporary steal")
        );
        assert_eq!(
            removal("Plan for All Outcomes").map(|r| r.1),
            Some("bounce")
        );
        assert_eq!(
            removal("Vindictive Triumph").map(|r| r.1),
            Some("unconditional exile")
        );
        assert!(has("S.H.I.E.L.D. Flying Car", &|e| matches!(
            e,
            Effect::Blink
        )));
        assert!(!has("S.H.I.E.L.D. Flying Car", &|e| matches!(
            e,
            Effect::Reanimate
        )));
        assert!(p(&db, "Blasphemous Edict").sweeper());
        // Replacement effects and damage to yourself.
        assert!(!has("Gratuitous Violence", &|e| matches!(
            e,
            Effect::Damage { .. }
        )));
        assert!(!has("Midnight Reaper", &|e| matches!(
            e,
            Effect::Damage { .. }
        )));
        assert!(has(
            "Midnight Reaper",
            &|e| matches!(e, Effect::Rule(r) if r == "damage to you")
        ));
        assert!(has(
            "Demonic Pact",
            &|e| matches!(e, Effect::Rule(r) if r == "you lose the game")
        ));
        // Team effects narrowed by type or condition.
        assert!(has(
            "Attuma, Atlantean Warlord",
            &|e| matches!(e, Effect::Pump { to, .. } if to.mass && to.other && to.restricted)
        ));
        assert!(has(
            "Marwyn, the Preserver",
            &|e| matches!(e, Effect::Grant { to, .. } if to.kind == Kind::Land)
        ));
        assert!(has(
            "Hire a Crew",
            &|e| matches!(e, Effect::Pump { to, .. } if to.mass && !to.restricted)
        ));
        // Ramp read from new places.
        assert!(p(&db, "Restorative Technique").ramps());
        assert!(p(&db, "Primal Witchstalker").ramps());
        assert!(
            has("Bard's Company", &|e| matches!(e, Effect::Tokens { .. })),
            "recruit leaves a token"
        );
        // Value model: '*' bodies, symmetric draws.
        assert!(crate::value::creature_value(db.get("Tarmogoyf").unwrap()).efficiency > 0.8);
        assert!(
            crate::value::creature_value(db.get("Dragon Mage").unwrap()).efficiency < 2.0,
            "everyone draws seven"
        );
    }

    /// Hand-checked against the printed text of pool cards the review found misread.
    #[test]
    fn reads_pool_review_cards() {
        let db = db();
        // Helicarrier Strike: "deals 2 damage to target attacking or blocking creature. If
        // ... teamwork, it deals 4 damage to that creature instead" - still an attacker or
        // blocker.
        let h = p(&db, "Helicarrier Strike");
        let alt = h
            .abilities
            .iter()
            .find(|a| a.instead)
            .expect("teamwork half");
        assert!(matches!(
            alt.effects.as_slice(),
            [Effect::Damage { amount, to }]
                if amount.n == 4
                    && to.restr.has(crate::restrict::ATTACKING)
                    && to.restr.has(crate::restrict::BLOCKING)
        ));
        // "That creature" in a condition stays as it is (Wakandan Royal Guard).
        assert!(
            p(&db, "Wakandan Royal Guard")
                .abilities
                .iter()
                .all(|a| !a.text.contains("if target creature")),
        );
        // "That creature's controller" stays as it is (Repulsor Blast's teamwork bonus).
        assert!(
            p(&db, "Repulsor Blast")
                .abilities
                .iter()
                .any(|a| a.text.contains("that creature's controller")),
        );
        // War Machine: "+X/+0, where X is War Machine's power" - his power is 1.
        assert!(
            p(&db, "War Machine, Legacy of Iron")
                .effects()
                .any(|(_, e)| matches!(
                    e,
                    Effect::Pump {
                        power: 1,
                        toughness: 0,
                        ..
                    }
                ))
        );
        // Whiplash: "X is the number of Equipment attached to him" - one, typically.
        assert!(
            p(&db, "Whiplash, Vengeful Engineer")
                .effects()
                .any(|(_, e)| matches!(e, Effect::Drain { amount, each: true } if amount.n == 1))
        );
        // Doc Samson: "add X mana, where X is Doc Samson's power" - 3.
        assert!(
            p(&db, "Doc Samson, Super Psychiatrist")
                .effects()
                .any(|(_, e)| matches!(e, Effect::AddMana(a) if a.n == 3))
        );
        // Vision Quest searches "library and/or graveyard" for an artifact creature card.
        let r = crate::restrict::object_restr(
            &db.get("Vision Quest")
                .unwrap()
                .text_lower()
                .replace('\n', " "),
        );
        let artifact = crate::playtest::compile::T_ARTIFACT;
        assert_eq!(r.all_types & artifact, artifact);
    }

    #[test]
    fn parses_every_line() {
        let db = db();
        let missed: Vec<String> = db
            .cards
            .iter()
            .flat_map(|c| {
                parse(c)
                    .unparsed
                    .into_iter()
                    .map(move |l| format!("{}: {}", c.name, l))
            })
            .collect();
        assert!(
            missed.is_empty(),
            "lines not understood:\n{}",
            missed.join("\n")
        );
    }
}
