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

    /// Typical value: X counts as 3, or 1 when it counts something on the board.
    pub fn value(&self) -> f32 {
        self.n as f32
    }
}

fn number(word: &str, x_small: bool) -> Option<Amount> {
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
        "x" | "that" | "many" => {
            return Some(Amount {
                n: if x_small { 1 } else { 3 },
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
        }
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
                    c.mana +=
                        s.parse::<u32>()
                            .unwrap_or(if s.eq_ignore_ascii_case("x") { 0 } else { 1 })
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
        c.other = c.other || t.contains("exile") || t.contains("remove") || t.contains("return");
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
    pub text: String,
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
            exile: re(r"exile (?:up to {N} )?(?:(other|another) )?target ([^.;]*?)(?: until ([^.;]*))?(?:\.|$|,| and )"),
            exile_all: re(r"exile (?:all|each) ([^.;]*)"),
            damage: re(r"deals? (\d+|x|that much|damage equal to [^,.]*?|damage)(?: damage)?(?: divided as you choose among| to) ([^.;]*)"),
            fight: re(r"fights? (?:up to one )?(?:(other|another) )?(target [^.;]*|it|that creature)"),
            shrink: re(r"(target [^.;]*?|enchanted creature|equipped creature) gets? -(\d+|x)/-(\d+|x)( until end of turn)?"),
            shrink_all: re(r"(?:all |each )?((?:other )?creatures(?: [a-z' ]*?)?(?:your opponents control|you don't control|an opponent controls)?|(?:all|each) [^.;]*?) gets? -(\d+|x)/-(\d+|x)"),
            bounce: re(r"return (?:up to {N} |one or two )?(?:(other|another) )?target ([^.;]*?) to (?:its|their) owners?(?:'s|s'|')? hands?"),
            bounce_all: re(r"return (?:all|each) ([^.;]*?) to (?:its|their) owners?(?:'s|s'|')? hands?"),
            tap_lock: re(r"(?:\btap (?:up to {N} )?(?:target ([^.;]*?)|(enchanted creature))(?:\.|$|,| and )|(enchanted creature|target creature[^.;]*?) (?:can't attack or block|can't block|can't attack|loses all abilities|doesn't untap))"),
            edict: re(r"(each opponent|target opponent|target player|each player|each other player) sacrifices? (?:[a-z]+|\d+) (?:nontoken )?(creatures?|nonland permanents?|permanents?|creature or planeswalker)"),
            draw: re(r"(?:(target player|each player|that player|its controller) )?draws? (?:{N} cards?|cards equal to|a card for each|x cards)"),
            tokens: re(r"creates? (?:([a-z' ]+?), a (?:legendary )?|{N} (?:tapped )?)(\d+|x)/(\d+|x) ([^.;]*?)tokens?(?: with ([^.;]*))?"),
            tokens_plain: re(r"creates? {N} (?:tapped )?([^.;]*?)creature tokens?(?: with ([^.;]*))?"),
            enters_with: re(r"enters with {N} \+1/\+1 counters?"),
            minus_counters: re(r"put {N} -1/-1 counters? on (?:up to (?:a|an|one|two|three|x|\d+) )?(?:target )?([^.;]*)"),
            other_counters: re(r"\b(?:a|an|one|two|three|four|five|six|seven|eight|nine|ten|x|\d+|that many) ((?:[a-z]+ )?[a-z]+) counters? on"),
            tuck: re(r"(?:the owner of (?:up to one )?(?:other )?target ([^.;]*?) (?:puts|shuffles) it|target ([^.;]*?)'s owner (?:puts|shuffles) it)"),
            item_tokens: re(r"creates? {N} (?:tapped )?(?:colorless )?(treasure|food|clue|map|blood|powerstone|junk|gold|incubator|heartwood|equipment)"),
            counters: re(r"put {N} \+1/\+1 counters?(?: and [^.;]*?)? on (?:up to (?:a|an|one|two|three|four|x|\d+) )?([^.;]*)"),
            pump: re(r"([^.;,]*?) gets? \+(\d+|x)/([+-](?:\d+|x))(?: and (?:gains?|has|have) ([^.;]*?))?( until end of turn)?(?:\.|$|,| for | as long| and )"),
            grant: re(r#"([^.;,]*?) (?:gains?|has|have) ([a-z ,{}0-9]*?(?:flying|indestructible|hexproof|first strike|double strike|trample|menace|lifelink|deathtouch|vigilance|haste|reach|ward|prowess|protection from [a-z]+)[a-z ,{}0-9]*?)( until end of turn)?(?:\.|$| as long| and "| and can't)"#),
            gain_life: re(r"(?:you |and )gains? (\d+|x|that much) life|you gain life equal to"),
            drain: re(r"(each opponent|target opponent|target player|each player|that player|defending player|its controller) loses? (\d+|x|that much) life|deals? (\d+|x) damage to (each opponent|target opponent|target player or planeswalker|each player)"),
            add_mana: re(r"add (\{[^}]+\}(?:(?:, | or |)\{[^}]+\})*|[a-z]+ mana|mana)"),
            mill: re(r"mills? {N} cards?"),
            scry: re(r"(scry|surveil) (\d+|x)"),
            empower: re(r"empower (?:jace|~) (\d+|x)"),
            amass: re(r"amass [a-z]+ (\d+|x)"),
            number_list: N,
        }
    })
}

fn amount(s: &str, line: &str) -> Amount {
    let x_small = line.contains("where x is")
        || line.contains("for each")
        || line.contains("equal to the number");
    number(s, x_small).unwrap_or(Amount::fixed(1))
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
    // Abilities in quotes on a token ("with \"Landfall - put a +1/+1 counter on ~\"")
    // belong to the token, not this card.
    if core.contains("token") && core.contains('"') {
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
    // Mass removal first, so "destroy all creatures" isn't also read as single-target.
    for c in p.destroy_all.captures_iter(s) {
        out.push(Effect::Destroy(read_target(&c[1], true)));
    }
    for c in p.exile_all.captures_iter(s) {
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
    for c in p.damage.captures_iter(s) {
        let amt = if c[1].contains("number of +1/+1 counters") {
            // Grows over the game from small numbers.
            Amount { n: 2, x: true }
        } else if c[1].starts_with("damage equal") || c[1] == *"that much" || c[1] == *"damage" {
            Amount { n: 3, x: true }
        } else {
            amount(&c[1], s)
        };
        let to = &c[2];
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
        let mass = to.starts_with("each ") || to.starts_with("all ");
        let to = to
            .trim_start_matches("any number of ")
            .trim_start_matches("one, two, or three ")
            .trim_start_matches("one or two ");
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
        }
        t.other = to.contains("other") || to.contains("another");
        if to.starts_with("any target")
            || to.starts_with("any other target")
            || to.starts_with("targets")
        {
            t.kind = Kind::AnyTarget;
        }
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
        out.push(Effect::Bounce(t));
    }
    for c in p.tap_lock.captures_iter(s) {
        let (phrase, lasting) = match (c.get(2), c.get(3), c.get(4)) {
            (Some(m), _, _) => (m.as_str().to_string(), s.contains("doesn't untap")),
            (None, Some(m), _) => (m.as_str().to_string(), s.contains("doesn't untap")),
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
    // Conniving also grows the creature when a nonland card is discarded.
    if s.contains("connive") {
        out.push(Effect::PlusCounters {
            amount: Amount::fixed(1),
            to: Target::it(),
        });
    }
    for c in p.draw.captures_iter(s) {
        if loot && c.get(0).is_some_and(|m| m.as_str().contains("a card")) {
            continue;
        }
        let you = c.get(1).is_none_or(|m| m.as_str() == "target player");
        let amt = c
            .get(2)
            .map(|m| amount(m.as_str(), s))
            .unwrap_or(Amount { n: 2, x: true });
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
    let given_away = |start: usize| {
        let head = &full[..start];
        head.ends_with("its controller ")
            || head.ends_with("that player ")
            || head.ends_with("their controller ")
    };
    for c in p.tokens.captures_iter(full) {
        if given_away(c.get(0).unwrap().start()) {
            continue;
        }
        let n = c
            .get(2)
            .map(|m| amount(m.as_str(), s))
            .unwrap_or(Amount::fixed(1));
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
            if given_away(c.get(0).unwrap().start()) {
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
                amount: amount(&c[1], s),
                power: 1,
                toughness: 1,
                keywords: kws,
            });
        }
    }
    for c in p.item_tokens.captures_iter(full) {
        if given_away(c.get(0).unwrap().start()) {
            continue;
        }
        // "a Food token or a Treasure token": a choice - the Treasure is the better one.
        let kind = if full.contains("food token or a treasure token") {
            "treasure".to_string()
        } else {
            c[2].to_string()
        };
        out.push(Effect::ItemTokens {
            amount: amount(&c[1], s),
            kind,
        });
    }
    for c in p.counters.captures_iter(s) {
        let n = amount(&c[1], s);
        let phrase = c[2].trim();
        let t = if phrase.starts_with('~')
            || phrase.starts_with("it")
            || phrase.starts_with("him")
            || phrase.starts_with("her")
        {
            Target::it()
        } else if phrase.starts_with("each creature you control")
            || phrase.starts_with("each other creature you control")
        {
            read_target("creature you control", true)
        } else {
            read_target(phrase.trim_start_matches("target "), false)
        };
        out.push(Effect::PlusCounters { amount: n, to: t });
    }
    for c in p.pump.captures_iter(s) {
        let who = c[1].trim();
        let to = pump_target(who);
        // "+1/+0 for each other artifact you control": counts something on the board.
        let pw = amount(&c[2], s).n * if s.contains("for each") { 2 } else { 1 };
        let t = if c[3].starts_with('-') {
            -amount(&c[3][1..], s).n
        } else {
            amount(&c[3].trim_start_matches('+'), s).n
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
            let kws: Vec<String> = KEYWORDS
                .iter()
                .filter(|kw| k.as_str().contains(*kw))
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
        for c in p.grant.captures_iter(s) {
            let to = pump_target(c[1].trim());
            let kws: Vec<String> = KEYWORDS
                .iter()
                .filter(|kw| c[2].contains(*kw))
                .map(|kw| kw.to_string())
                .collect();
            if !kws.is_empty() {
                let eot = c.get(3).is_some()
                    || s.starts_with("until end of turn")
                    || s.contains("this turn");
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
            .map(|m| amount(m.as_str(), s))
            .unwrap_or(Amount { n: 3, x: true });
        out.push(Effect::GainLife(amt));
    }
    for c in p.drain.captures_iter(s) {
        // The damage pattern already handled "deals N damage to each opponent".
        if let (Some(who), Some(n)) = (c.get(1), c.get(2)) {
            out.push(Effect::Drain {
                amount: amount(n.as_str(), s),
                each: who.as_str().starts_with("each"),
            });
        }
    }
    for c in p.add_mana.captures_iter(s) {
        let n = if c[1].starts_with('{') {
            // "{R} or {G}" is one mana; "{R}{R}" two.
            if c[1].contains(" or ") || c[1].contains(", ") {
                1
            } else {
                crate::card::symbols_in(&c[1]).len().max(1) as i32
            }
        } else {
            number(c[1].split_whitespace().next().unwrap_or("one"), false)
                .map(|a| a.n)
                .unwrap_or(1)
        };
        out.push(Effect::AddMana(Amount::fixed(n)));
    }
    if s.contains("search your library") {
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
    } else if s.contains("from your graveyard to your hand")
        || s.contains("graveyard into your hand")
        || s.contains("return this card from your graveyard")
    {
        out.push(Effect::Regrowth);
    }
    if s.contains("untap target") || s.contains("untap up to") || s.contains("untap all") {
        let phrase = s
            .split("untap ")
            .nth(1)
            .unwrap_or("creature")
            .trim_start_matches("all ")
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
    if s.contains("token that's a copy")
        || s.contains("tokens that are copies")
        || s.contains("token copy")
    {
        out.push(Effect::ItemTokens {
            amount: Amount::fixed(1),
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
            let to = if s.contains("on target") {
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
        // "Second from the top or on the bottom": the owner picks, and redraws it soon -
        // closer to bouncing it than to exiling it.
        if s.contains("from the top") {
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
    if s.contains("that many plus one") {
        // One extra counter or token each time (Hardened Scales), weaker than doubling.
        out.push(Effect::Doubler("plus one".into()));
    } else if s.contains("twice that many") || s.contains("double the number of") {
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
    if s.contains("prevent all combat damage") {
        out.push(Effect::Fog);
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
    // "Exile ... then return it to the battlefield" is a flicker (already read above), not
    // bringing something back from the graveyard.
    if (s.contains("return it to the battlefield")
        || s.contains("return that card to the battlefield")
        || s.contains("return them to the battlefield"))
        && !out.contains(&Effect::Blink)
    {
        out.push(Effect::Reanimate);
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
    if s.contains("can't be blocked")
        && !s.contains("can't be blocked except")
        && !s.contains("by more than")
        && !s.contains("as long as")
    {
        let to = if let Some(i) = s.find("target ") {
            read_target(&s[i + 7..], false)
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
    let _ = p.number_list;
    out
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
    if (body.contains("reveals their hand") || body.contains("reveal their hand"))
        && (body.contains("discards that card")
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
            if !has(effects, |e| matches!(e, Effect::Exile { .. })) {
                effects.push(Effect::Exile {
                    target: read_target(phrase, false),
                    until: false,
                });
            }
        }
    }
    if body.contains("graveyard") && body.contains("card") {
        if (body.contains("to the battlefield") || body.contains("onto the battlefield"))
            && !has(effects, |e| matches!(e, Effect::Reanimate))
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
    let who = who.trim();
    if who.ends_with('~')
        || who == "it"
        || who.ends_with(" it")
        || who == "he"
        || who == "she"
        || who.ends_with("this creature")
    {
        return Target::it();
    }
    if who.contains("creatures you control")
        || who.contains("creature you control") && who.starts_with("each")
    {
        let mut t = read_target("creature you control", true);
        t.other = who.contains("other");
        return t;
    }
    if who.contains("equipped creature") || who.contains("enchanted creature") {
        return read_target("creature you control", false);
    }
    // "Other Villains you control", "Heroes you control": a team boost for that type.
    if let Some(head) = who.strip_suffix(" you control") {
        let head = head.trim();
        if head.ends_with('s') && !head.ends_with("creatures") {
            let mut t = read_target("creature you control", true);
            t.other = head.contains("other");
            t.restricted = true;
            return t;
        }
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

fn self_refs(text: &str, card: &Card) -> String {
    let mut t = fold_accents(
        &text
            .to_lowercase()
            .replace('’', "'")
            .replace('−', "-")
            .replace(['“', '”'], "\""),
    );
    let name = fold_accents(&card.name.to_lowercase());
    t = t.replace(&name, "~");
    // Short names: "Bilbo" for "Bilbo Baggins, Burglar", "Ka-Zar" for "Ka-Zar of the Savage Land".
    let short = name
        .split_once(", ")
        .map(|x| x.0)
        .or_else(|| name.split_once(" of the ").map(|x| x.0))
        .or_else(|| {
            name.split_once(" the ")
                .map(|x| x.0)
                .filter(|s| !s.contains(' '))
        })
        .filter(|s| s.len() >= 3);
    if let Some(short) = short {
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
                a.effects.clear();
                for m in &modes {
                    let mut e = read_effects(m);
                    body_effects(m, &mut e);
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
                } else {
                    a.modal = true;
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
        let (line, alternative) = split_instead(&line);
        if let Some(alt) = alternative.and_then(|alt| read_line(&alt, card, is_spell)) {
            if !alt.effects.is_empty() {
                if let Some(base) =
                    read_line(&line, card, is_spell).filter(|a| !a.effects.is_empty())
                {
                    let mut base = base;
                    // "X if <condition>. Otherwise, Y": X needs the condition too.
                    if lines[i - 1].contains(". otherwise, ") {
                        base.conditional = true;
                    }
                    out.abilities.push(base);
                    let mut alt = alt;
                    alt.conditional = true;
                    out.abilities.push(alt);
                    continue;
                }
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
    // Teamwork, Storied and friends live in the keyword list; Sagas' "I, II —" lines are
    // chapters (handled in read_line).
    out
}

/// "X. If <condition>, Y instead." -> ("X.", Some("<same trigger/cost>, if <condition>, Y
/// instead")). Replacement effects ("if ... would ..., ... instead") are left alone: they
/// aren't alternatives to an earlier sentence.
fn split_instead(line: &str) -> (String, Option<String>) {
    // "X if <condition>. Otherwise, Y." - two alternatives (HYDRA Troopers).
    if let Some(i) = line.find(". otherwise, ") {
        let sentence = line[i + 2..].trim().trim_end_matches('.');
        let alt_body = sentence.trim_start_matches("otherwise, ");
        if !sentence.contains(". ") {
            let base = format!("{}.", &line[..i]);
            return (
                base.clone(),
                Some(with_same_start(&base, &format!("if not, {}", alt_body))),
            );
        }
    }
    let Some(i) = line.find(". if ") else {
        return (line.to_string(), None);
    };
    let sentence = line[i + 2..].trim().trim_end_matches('.');
    // "If <condition>, Y instead." and "If <condition>, instead Y." (Cruel Alliance).
    let instead_form = sentence.ends_with("instead") || sentence.contains(", instead ");
    if !instead_form || sentence.contains(" would ") || sentence.contains(". ") {
        return (line.to_string(), None);
    }
    let sentence = sentence.replace(", instead ", ", ");
    let base = format!("{}.", &line[..i]);
    (base.clone(), Some(with_same_start(&base, &sentence)))
}

/// Puts `sentence` behind the same trigger or cost as `base` ("When ~ enters, " / "{2}: ").
fn with_same_start(base: &str, sentence: &str) -> String {
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
    match line.find(", ") {
        Some(i) => (&line[..i], line[i + 2..].trim()),
        None => (line, ""),
    }
}

fn read_trigger(trigger: &str) -> Vec<When> {
    let t = trigger;
    let mut w = Vec::new();
    if t.starts_with("at the beginning of") || t.starts_with("at end of") {
        w.push(When::EachTurn);
        return w;
    }
    let self_subject =
        t.contains('~') || t.starts_with("when this") || t.starts_with("whenever this");
    if self_subject && t.contains(" enter") {
        w.push(When::Enters);
    }
    if self_subject && (t.contains(" dies") || t.contains("is put into a graveyard")) {
        w.push(When::Dies);
    }
    if (self_subject && (t.contains(" attacks") || t.contains("attacks or blocks")))
        || t.contains("whenever you attack")
    {
        w.push(When::Attacks);
    }
    if self_subject && t.contains("deals combat damage") {
        w.push(When::CombatDamage);
    }
    // Becoming tapped is mostly attacking (or tapping it for an ability).
    if self_subject && t.contains("becomes tapped") && w.is_empty() {
        w.push(When::Attacks);
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
    let mut line = line.trim().to_string();
    // Ability words ("Landfall — Whenever ...") only label the ability (CR 207.2c); MSH also
    // uses flavour labels ("Radar Sense — ...").
    let mut power_up = false;
    // Infinity Stones' "∞ —" abilities only work once the stone is harnessed.
    let mut harness = false;
    if let Some((label, rest)) = line.split_once(" — ") {
        let label = label.trim();
        harness = label == "∞";
        if label == "power-up" {
            power_up = true;
            line = rest.to_string();
        } else if label.split_whitespace().count() <= 4
            && !label.contains(':')
            && !label.contains('{')
        {
            let chapter = match label {
                "i" => Some(1),
                "ii" => Some(2),
                "iii" => Some(3),
                "iv" => Some(4),
                "v" => Some(5),
                "i, ii" => Some(1),
                "ii, iii" => Some(2),
                _ => None,
            };
            if let Some(n) = chapter {
                let mut effects: Vec<Effect> = sentences(rest)
                    .iter()
                    .flat_map(|s| read_effects(s))
                    .collect();
                body_effects(rest, &mut effects);
                return Some(Ability {
                    when: vec![When::Chapter(n)],
                    effects,
                    optional: rest.contains("you may"),
                    conditional: rest.contains(" if "),
                    once: false,
                    modal: false,
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
        once: power_up || line.contains("activate only once") || line.contains("only once each"),
        modal: false,
        text: line.clone(),
    };
    let body: String;
    if let Some(rest) = line.strip_prefix('[') {
        // Loyalty ability: "[+1]: ..."
        let (cost, effect) = rest.split_once("]:")?;
        let n = cost.replace('−', "-").replace('+', "").parse().unwrap_or(0);
        ability.when = vec![When::Loyalty(n)];
        body = effect.trim().to_string();
    } else if line.starts_with("when ") || line.starts_with("whenever ") || line.starts_with("at ")
    {
        let (trigger, rest) = split_trigger(&line);
        if trigger.starts_with("when you do")
            || trigger.starts_with("when the ") && trigger.contains("counter is put")
        {
            ability.when = vec![When::Reflexive];
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
    // Only an "if" that gates the whole ability counts (an intervening-if, set above, or a
    // body that opens with one); "if you do" / "if that creature..." in a later sentence
    // only affects the extra part.
    if body.starts_with("if ") || body.starts_with("as long as") {
        ability.conditional = true;
    }
    for s in sentences(&body) {
        ability.effects.extend(read_effects(&s));
    }
    body_effects(&body, &mut ability.effects);
    // Self-referential "enters prepared" lines are statics on the face.
    if ability.effects.is_empty() && line.contains("prepared") {
        ability.effects.push(Effect::Prepared);
    }
    if harness || line.contains("activate only if") {
        ability.conditional = true;
    }
    // Drawing that costs a card first ("{T}, Discard a card: Draw a card..."; "You may
    // sacrifice an artifact or discard a card. If you do, draw two cards") nets one card
    // fewer: the first draw only replaces what was paid.
    let costs_a_card = matches!(ability.when.first(), Some(When::Activated(c)) if c.discard)
        || (body.contains("discard a card") || body.contains("sacrifice an artifact"))
            && body.contains("if you do, draw");
    if costs_a_card && !ability.effects.contains(&Effect::Loot) {
        if let Some(pos) = ability
            .effects
            .iter()
            .position(|e| matches!(e, Effect::Draw { you: true, .. }))
        {
            let n = match &ability.effects[pos] {
                Effect::Draw { amount, .. } => amount.n,
                _ => 1,
            };
            ability.effects[pos] = if n <= 1 {
                Effect::Loot
            } else {
                Effect::Draw {
                    amount: Amount::fixed(n - 1),
                    you: true,
                }
            };
        }
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
    (looks_like_cost && !c.starts_with("when") && c.len() < 90).then_some((c, effect.trim()))
}

fn sentences(text: &str) -> Vec<String> {
    text.split(". ")
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
                if let Some((pts, label, mass, one_sided)) = grade(e) {
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
                || matches!(e, Effect::ItemTokens { kind, .. } if kind == "treasure")
                    && !a.when.iter().any(|w| matches!(w, When::Activated(c) if c.mana > 0))
        })
    }

    /// Creature tokens, Armies, and Treasure/Food/Clue/copy tokens ("tokens you control"
    /// payoffs count them all).
    pub fn makes_tokens(&self) -> bool {
        self.effects().any(|(_, e)| {
            matches!(
                e,
                Effect::Tokens { .. } | Effect::Amass(_) | Effect::ItemTokens { .. }
            )
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
