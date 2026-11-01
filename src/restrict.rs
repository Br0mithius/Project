//! What exactly an effect can hit: the words that narrow a target or a group ("other Elves
//! you control", "creature with mana value 3 or less", "tapped creature", "attacking
//! creatures", "artifact token", "Dragon creature card", "black or green creature"...),
//! read once from the rules text into a compact descriptor (`Restr`) that the simulator
//! checks against permanents, spells and cards.
//!
//! Words are matched whole ("orc" is not in "sorcery", "elf" not in "itself").

use crate::playtest::compile::{
    T_ARTIFACT, T_BASIC, T_CREATURE, T_ENCHANTMENT, T_INSTANT, T_LAND, T_LEGENDARY, T_PLANESWALKER,
    T_SORCERY,
};

/// Subtypes the league's rules text names (creature, land and artifact types).
pub const SUBTYPES: &[&str] = &[
    "advisor",
    "aetherborn",
    "alien",
    "ally",
    "angel",
    "ape",
    "archer",
    "army",
    "artificer",
    "assassin",
    "aura",
    "avatar",
    "barbarian",
    "bard",
    "bat",
    "bear",
    "beast",
    "berserker",
    "bird",
    "boar",
    "cat",
    "cave",
    "centaur",
    "chimera",
    "citizen",
    "cleric",
    "construct",
    "crab",
    "cyclops",
    "demigod",
    "demon",
    "desert",
    "detective",
    "devil",
    "dinosaur",
    "djinn",
    "doctor",
    "dog",
    "dragon",
    "drake",
    "druid",
    "dryad",
    "dwarf",
    "eldrazi",
    "elemental",
    "elephant",
    "elf",
    "elk",
    "equipment",
    "faerie",
    "fish",
    "forest",
    "fox",
    "frog",
    "fungus",
    "gamma",
    "gargoyle",
    "gate",
    "giant",
    "gnome",
    "goat",
    "goblin",
    "god",
    "golem",
    "gorgon",
    "gremlin",
    "halfling",
    "hero",
    "hippo",
    "homunculus",
    "horror",
    "horse",
    "hound",
    "human",
    "hydra",
    "hyena",
    "illusion",
    "imp",
    "inhuman",
    "insect",
    "island",
    "jace",
    "juggernaut",
    "kithkin",
    "knight",
    "kor",
    "kraken",
    "kree",
    "leviathan",
    "lizard",
    "mercenary",
    "merfolk",
    "minotaur",
    "monk",
    "monkey",
    "mountain",
    "mouse",
    "mutant",
    "nightmare",
    "ninja",
    "noble",
    "octopus",
    "ogre",
    "ooze",
    "orc",
    "otter",
    "ox",
    "peasant",
    "pegasus",
    "performer",
    "pest",
    "phoenix",
    "phyrexian",
    "pilot",
    "pirate",
    "plains",
    "plant",
    "rabbit",
    "ranger",
    "rat",
    "rhino",
    "robot",
    "rogue",
    "saga",
    "samurai",
    "saproling",
    "satyr",
    "scientist",
    "scout",
    "serpent",
    "servo",
    "shade",
    "shaman",
    "shapeshifter",
    "shark",
    "siren",
    "skeleton",
    "skrull",
    "snake",
    "soldier",
    "sorcerer",
    "sphinx",
    "spider",
    "spirit",
    "spy",
    "squirrel",
    "swamp",
    "thopter",
    "town",
    "treefolk",
    "troll",
    "turtle",
    "unicorn",
    "vampire",
    "vedalken",
    "vehicle",
    "villain",
    "wall",
    "warlock",
    "warrior",
    "werewolf",
    "whale",
    "wizard",
    "wolf",
    "wurm",
    "yeti",
    "zombie",
];

/// The subtype a (possibly plural) word names: "Elves" -> elf, "Wolves" -> wolf, "Cats" -> cat.
pub fn subtype_word(word: &str) -> Option<u16> {
    let w = word.trim_matches(|c: char| !c.is_alphabetic() && c != '\'');
    let w = w.trim_end_matches("'s").trim_end_matches('\'');
    if w.len() < 2 {
        return None;
    }
    let find = |s: &str| SUBTYPES.iter().position(|x| *x == s).map(|i| i as u16 + 1);
    let special = match w {
        "elves" => Some("elf"),
        "dwarves" => Some("dwarf"),
        "wolves" => Some("wolf"),
        "werewolves" => Some("werewolf"),
        "mice" => Some("mouse"),
        "fungi" => Some("fungus"),
        "octopi" => Some("octopus"),
        "oxen" => Some("ox"),
        "allies" => Some("ally"),
        "mercenaries" => Some("mercenary"),
        "sphinxes" => Some("sphinx"),
        "foxes" => Some("fox"),
        "phoenixes" => Some("phoenix"),
        "djinns" | "djinni" => Some("djinn"),
        "plains" => Some("plains"),
        _ => None,
    };
    if let Some(s) = special {
        return find(s);
    }
    find(w).or_else(|| {
        let stem = w.strip_suffix('s')?;
        find(stem).or_else(|| find(stem.strip_suffix('e')?))
    })
}

pub fn subtype_name(id: u16) -> &'static str {
    SUBTYPES
        .get(id.wrapping_sub(1) as usize)
        .copied()
        .unwrap_or("")
}

/// Does a text mention this subtype as a whole word ("orc" is not in "sorcery")?
pub fn mentions_subtype(text: &str, sub: &str) -> bool {
    words(text).any(|w| subtype_word(w).is_some_and(|i| subtype_name(i) == sub))
}

fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '-' || c == '+' || c == '/'))
        .filter(|w| !w.is_empty())
}

// Flags.
pub const TAPPED: u32 = 1;
pub const UNTAPPED: u32 = 1 << 1;
/// Attacking (with BLOCKING too: attacking or blocking).
pub const ATTACKING: u32 = 1 << 2;
pub const BLOCKING: u32 = 1 << 3;
pub const FLYING: u32 = 1 << 4;
pub const NO_FLYING: u32 = 1 << 5;
pub const TOKEN: u32 = 1 << 6;
pub const NONTOKEN: u32 = 1 << 7;
/// With a +1/+1 counter on it.
pub const COUNTER: u32 = 1 << 8;
pub const DEATHTOUCH: u32 = 1 << 9;
pub const HASTE: u32 = 1 << 10;
pub const EQUIPPED: u32 = 1 << 11;
/// Entered the battlefield this turn.
pub const ENTERED: u32 = 1 << 12;
/// Was dealt damage this turn.
pub const DAMAGED: u32 = 1 << 13;
pub const MULTICOLORED: u32 = 1 << 14;
pub const MONOCOLORED: u32 = 1 << 15;
pub const COLORLESS: u32 = 1 << 16;
/// "with the greatest mana value among ..." (the biggest of each player's).
pub const GREATEST_MV: u32 = 1 << 17;
pub const GREATEST_POWER: u32 = 1 << 18;
/// "of the chosen type": the type most of its controller's creatures share.
pub const CHOSEN_TYPE: u32 = 1 << 19;
/// "of the chosen color": the colour most of its controller's creatures have.
pub const CHOSEN_COLOR: u32 = 1 << 20;
/// "... target player controls": everything of one player (an opponent for harm).
pub const ONE_PLAYER: u32 = 1 << 21;
/// "For each opponent, ... that player controls": one pick per opponent.
pub const PER_OPPONENT: u32 = 1 << 22;
/// "Mana value X or less": X is the spell's X.
pub const MV_LE_X: u32 = 1 << 23;
/// "Mana value less than or equal to ~'s power".
pub const MV_LE_SRC_POWER: u32 = 1 << 24;
/// A card in a graveyard that got there this turn.
pub const THIS_TURN: u32 = 1 << 25;
pub const VIGILANCE: u32 = 1 << 26;
pub const DEFENDER: u32 = 1 << 27;
/// "It", "that creature", "those creatures": what the ability just chose (or made), else
/// (with `SUBJECT`) the creature its trigger is about.
pub const PREV: u32 = 1 << 28;
/// A triggered ability about another creature: "it"/"that creature" can be that creature.
pub const SUBJECT: u32 = 1 << 29;
/// "That aren't of the chosen type".
pub const NOT_CHOSEN_TYPE: u32 = 1 << 30;
/// "A Villain and/or artifact", "a Dwarf or Equipment": its subtypes or card types, either.
pub const SUB_OR_TYPE: u32 = 1 << 31;

/// What an effect's object must be, beyond its kind and controller.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct Restr {
    pub flags: u32,
    /// Card types it must have one of ("artifact or creature card"); 0 = any.
    pub any_types: u16,
    /// Of `any_types`, those the qualifiers ("with flying", "with power 4 or greater") apply
    /// to: "artifact, enchantment, or creature with flying" (0 = all of them).
    pub qual_types: u16,
    /// Card types it must have all of ("artifact creature", "legendary creature").
    pub all_types: u16,
    pub not_types: u16,
    /// Colours it must have one of, and must not have.
    pub colors: u8,
    pub not_colors: u8,
    /// Subtypes (`SUBTYPES` index + 1) it must have one of; 0 = unused.
    pub subtypes: [u16; 3],
    pub not_subtype: u16,
    pub mv_min: Option<u8>,
    pub mv_max: Option<u8>,
    pub power_min: Option<i8>,
    pub power_max: Option<i8>,
    pub tough_min: Option<i8>,
    pub tough_max: Option<i8>,
    /// "power or toughness N or less".
    pub pt_or_max: Option<i8>,
}

const FLAG_NAMES: &[(u32, &str)] = &[
    (TAPPED, "tapped"),
    (UNTAPPED, "untapped"),
    (ATTACKING, "attacking"),
    (BLOCKING, "blocking"),
    (FLYING, "flying"),
    (NO_FLYING, "no flying"),
    (TOKEN, "token"),
    (NONTOKEN, "nontoken"),
    (COUNTER, "with +1/+1 counter"),
    (DEATHTOUCH, "deathtouch"),
    (HASTE, "haste"),
    (EQUIPPED, "equipped"),
    (ENTERED, "entered this turn"),
    (DAMAGED, "damaged this turn"),
    (MULTICOLORED, "multicolored"),
    (MONOCOLORED, "monocolored"),
    (COLORLESS, "colorless"),
    (GREATEST_MV, "greatest mana value"),
    (GREATEST_POWER, "greatest power"),
    (CHOSEN_TYPE, "chosen type"),
    (CHOSEN_COLOR, "chosen color"),
    (ONE_PLAYER, "one player's"),
    (PER_OPPONENT, "one per opponent"),
    (MV_LE_X, "mana value <= X"),
    (MV_LE_SRC_POWER, "mana value <= its power"),
    (THIS_TURN, "this turn"),
    (VIGILANCE, "vigilance"),
    (DEFENDER, "defender"),
    (PREV, "the one just chosen"),
    (SUBJECT, "or the trigger's creature"),
    (NOT_CHOSEN_TYPE, "not the chosen type"),
    (SUB_OR_TYPE, "subtype or card type"),
];

fn type_names(bits: u16) -> String {
    let all = [
        (T_CREATURE, "creature"),
        (T_ARTIFACT, "artifact"),
        (T_ENCHANTMENT, "enchantment"),
        (T_PLANESWALKER, "planeswalker"),
        (T_LAND, "land"),
        (T_INSTANT, "instant"),
        (T_SORCERY, "sorcery"),
        (T_LEGENDARY, "legendary"),
        (T_BASIC, "basic"),
    ];
    all.iter()
        .filter(|(b, _)| bits & b != 0)
        .map(|(_, n)| *n)
        .collect::<Vec<_>>()
        .join("/")
}

fn color_names(bits: u8) -> String {
    (0..5)
        .filter(|i| bits & (1 << i) != 0)
        .map(|i| COLOR_WORDS[i])
        .collect::<Vec<_>>()
        .join("/")
}

/// Compact: only what's set ("Restr(attacking, mana value <= 3, subtype elf)").
impl std::fmt::Debug for Restr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts: Vec<String> = FLAG_NAMES
            .iter()
            .filter(|(b, _)| self.flags & b != 0)
            .map(|(_, n)| n.to_string())
            .collect();
        if self.any_types != 0 {
            parts.push(format!("one of {}", type_names(self.any_types)));
        }
        if self.qual_types != 0 {
            parts.push(format!("qualifiers for {}", type_names(self.qual_types)));
        }
        if self.all_types != 0 {
            parts.push(format!("all of {}", type_names(self.all_types)));
        }
        if self.not_types != 0 {
            parts.push(format!("not {}", type_names(self.not_types)));
        }
        if self.colors != 0 {
            parts.push(format!("colour {}", color_names(self.colors)));
        }
        if self.not_colors != 0 {
            parts.push(format!("not {}", color_names(self.not_colors)));
        }
        let subs: Vec<&str> = self.subtype_ids().map(subtype_name).collect();
        if !subs.is_empty() {
            parts.push(format!("subtype {}", subs.join("/")));
        }
        if self.not_subtype != 0 {
            parts.push(format!("not {}", subtype_name(self.not_subtype)));
        }
        for (name, lo, hi) in [
            (
                "mana value",
                self.mv_min.map(|n| n as i32),
                self.mv_max.map(|n| n as i32),
            ),
            (
                "power",
                self.power_min.map(|n| n as i32),
                self.power_max.map(|n| n as i32),
            ),
            (
                "toughness",
                self.tough_min.map(|n| n as i32),
                self.tough_max.map(|n| n as i32),
            ),
        ] {
            match (lo, hi) {
                (Some(a), Some(b)) if a == b => parts.push(format!("{} {}", name, a)),
                (Some(a), Some(b)) => parts.push(format!("{} {}-{}", name, a, b)),
                (Some(a), None) => parts.push(format!("{} >= {}", name, a)),
                (None, Some(b)) => parts.push(format!("{} <= {}", name, b)),
                _ => {}
            }
        }
        if let Some(n) = self.pt_or_max {
            parts.push(format!("power or toughness <= {}", n));
        }
        write!(f, "Restr({})", parts.join(", "))
    }
}

/// The facts about a permanent, spell or card that a restriction can ask about.
pub struct Obj<'a> {
    pub types: u16,
    pub subtypes: &'a [String],
    pub mv: u8,
    pub colors: u8,
    pub power: i16,
    pub toughness: i16,
    /// Keyword bits (`playtest::compile`).
    pub kw: u32,
    pub token: bool,
    pub tapped: bool,
    pub attacking: bool,
    pub blocking: bool,
    pub counters: i16,
    pub equipped: bool,
    pub entered: bool,
    pub damaged: bool,
    /// Dealt damage this turn.
    pub dealt: bool,
}

/// Values only the game knows when the effect happens.
#[derive(Default, Clone, Copy)]
pub struct Dyn {
    pub x: Option<i32>,
    pub src_power: Option<i32>,
    pub chosen_type: Option<u16>,
    pub chosen_color: u8,
    pub this_turn: bool,
}

const TYPE_WORDS: &[(&str, u16)] = &[
    ("creature", T_CREATURE),
    ("creatures", T_CREATURE),
    ("artifact", T_ARTIFACT),
    ("artifacts", T_ARTIFACT),
    ("enchantment", T_ENCHANTMENT),
    ("enchantments", T_ENCHANTMENT),
    ("planeswalker", T_PLANESWALKER),
    ("planeswalkers", T_PLANESWALKER),
    ("land", T_LAND),
    ("lands", T_LAND),
    ("instant", T_INSTANT),
    ("instants", T_INSTANT),
    ("sorcery", T_SORCERY),
    ("sorceries", T_SORCERY),
];

const COLOR_WORDS: &[&str] = &["white", "blue", "black", "red", "green"];

fn type_word(w: &str) -> Option<u16> {
    TYPE_WORDS.iter().find(|(t, _)| *t == w).map(|(_, b)| *b)
}

fn color_word(w: &str) -> Option<u8> {
    COLOR_WORDS.iter().position(|c| *c == w).map(|i| 1 << i)
}

fn num(w: &str) -> Option<i32> {
    Some(match w {
        "zero" => 0,
        "one" => 1,
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        d => d.parse().ok()?,
    })
}

/// Cuts a phrase where the description of the object ends ("target creature if it's
/// tapped", "target creature and you gain 2 life", "creatures you control get +1/+1").
pub fn object_part(phrase: &str) -> &str {
    let mut end = phrase.len();
    for stop in [
        ". ",
        "; ",
        " if ",
        " unless ",
        ", then",
        " then ",
        " until ",
        " gets ",
        " get ",
        " gains ",
        " gain ",
        " has ",
        " have ",
        " can't ",
        " deals ",
        " deal ",
        " fights ",
        " fight ",
        " becomes ",
        " to its owner",
        " to their owner",
        " to the battlefield",
        " onto the battlefield",
        " into your hand",
        " to your hand",
        " and you ",
        " and it ",
        " and its ",
        " and that ",
        " and put ",
        " and draw",
        " and create",
        " and return",
        " and exile",
        " and destroy",
        " and tap",
        " and untap",
        " and gain",
        " and deal",
        " and each",
        " and target",
        " and ~",
        " where ",
        " for each",
        " except",
        ", where",
        ", each",
        ", you ",
        ", draw",
        ", put ",
        ", create",
        ", return",
        ", ~ ",
    ] {
        if let Some(i) = phrase.find(stop) {
            end = end.min(i);
        }
    }
    // "... equal to the number of Goblins you control" is an amount, but "mana value less
    // than or equal to ~'s power" is a bound.
    for (i, _) in phrase.match_indices(" equal to") {
        if !phrase[..i].ends_with(" or") {
            end = end.min(i);
        }
    }
    &phrase[..end]
}

/// "That creature", "those creatures", "it", "the token": something named before.
pub fn refers_back(phrase: &str) -> bool {
    let p = phrase.trim();
    [
        "that creature",
        "that permanent",
        "that token",
        "those creatures",
        "those permanents",
        "those tokens",
        "the creature",
        "the token",
        "the targeted",
        "the attacking creature",
        "the blocking creature",
        "it",
        "they",
        "them",
        "him",
        "her",
    ]
    .iter()
    .any(|w| p == *w || p.starts_with(&format!("{} ", w)) || p.starts_with(&format!("{}'", w)))
}

impl Restr {
    pub fn is_empty(&self) -> bool {
        *self == Restr::default()
    }

    pub fn has(&self, f: u32) -> bool {
        self.flags & f != 0
    }

    fn add_subtype(&mut self, id: u16) {
        if self.subtypes.contains(&id) {
            return;
        }
        if let Some(slot) = self.subtypes.iter_mut().find(|s| **s == 0) {
            *slot = id;
        }
    }

    pub fn subtype_ids(&self) -> impl Iterator<Item = u16> + '_ {
        self.subtypes.iter().copied().filter(|&s| s != 0)
    }

    /// Reads the restrictions in a phrase naming what an effect hits ("target creature an
    /// opponent controls with mana value 3 or less", "other Elves you control", "creature
    /// card with mana value 2 or less from your graveyard").
    pub fn read(phrase: &str) -> Restr {
        let p = object_part(&phrase.to_lowercase()).to_string();
        let mut r = Restr::default();
        if refers_back(&p) {
            r.flags |= PREV;
        }
        let ws: Vec<&str> = words(&p).collect();
        let at = |i: usize| ws.get(i).copied().unwrap_or("");
        // Nouns joined by "or"/"and/or"/commas form a list of alternatives; a qualifier
        // after the last noun ("with flying") only narrows that noun when the list mixes
        // card types ("artifact, enchantment, or creature with flying").
        let mut listed_types = 0u16;
        let mut last_type = 0u16;
        for i in 0..ws.len() {
            let w = at(i);
            let (neg, base) = if let Some(b) = w.strip_prefix("non-") {
                (true, b)
            } else if let Some(b) = w.strip_prefix("non").filter(|b| {
                type_word(b).is_some()
                    || color_word(b).is_some()
                    || matches!(*b, "token" | "basic" | "legendary")
            }) {
                (true, b)
            } else {
                (false, w)
            };
            if neg {
                if let Some(t) = type_word(base) {
                    r.not_types |= t;
                } else if let Some(c) = color_word(base) {
                    r.not_colors |= c;
                } else if base == "token" {
                    r.flags |= NONTOKEN;
                } else if base == "basic" {
                    r.not_types |= T_BASIC;
                } else if base == "legendary" {
                    r.not_types |= T_LEGENDARY;
                } else if let Some(s) = subtype_word(base) {
                    r.not_subtype = s;
                }
                continue;
            }
            if let Some(t) = type_word(w) {
                // "artifact creature": both; "artifact or creature": either.
                if matches!(at(i + 1), "creature" | "creatures") && t != T_CREATURE {
                    r.all_types |= t;
                } else {
                    listed_types |= t;
                    last_type = t;
                }
                continue;
            }
            match w {
                "legendary" => r.all_types |= T_LEGENDARY,
                "basic" => r.all_types |= T_BASIC,
                "tapped" => r.flags |= TAPPED,
                "untapped" => r.flags |= UNTAPPED,
                "attacking" => r.flags |= ATTACKING,
                "blocking" => r.flags |= BLOCKING,
                "token" | "tokens" => r.flags |= TOKEN,
                "multicolored" => r.flags |= MULTICOLORED,
                "monocolored" => r.flags |= MONOCOLORED,
                "colorless" => r.flags |= COLORLESS,
                "equipped" if i > 0 => r.flags |= EQUIPPED,
                _ => {}
            }
            if let Some(c) = color_word(w) {
                // Not "protection from black" or "a 1/1 black token" made by the effect.
                if at(i.wrapping_sub(1)) != "from" {
                    r.colors |= c;
                }
            }
            if let Some(s) = subtype_word(w) {
                // "Plains" etc. are words too; a subtype is a noun or an adjective before
                // one, never right after "chosen".
                if at(i.wrapping_sub(1)) != "chosen" {
                    r.add_subtype(s);
                }
            }
        }
        if listed_types != 0 {
            r.any_types = listed_types;
        }
        // "A Villain and/or artifact", "a noncreature or Dragon spell": a subtype or a card
        // type (or a "non-" one), either will do.
        if r.subtypes[0] != 0
            && (listed_types != 0 || r.not_types != 0)
            && (p.contains(" or ") || p.contains("and/or"))
        {
            r.flags |= SUB_OR_TYPE;
        }
        // Qualifiers.
        let with = |s: &str| p.contains(s);
        if with("with flying") {
            r.flags |= FLYING;
        }
        if with("without flying") {
            r.flags = (r.flags & !FLYING) | NO_FLYING;
        }
        if with("with deathtouch") {
            r.flags |= DEATHTOUCH;
        }
        if with("with haste") {
            r.flags |= HASTE;
        }
        if with("with vigilance") {
            r.flags |= VIGILANCE;
        }
        if with("with defender") {
            r.flags |= DEFENDER;
        }
        if with("+1/+1 counter on") || with("+1/+1 counters on") {
            r.flags |= COUNTER;
        }
        if with("entered this turn") || with("entered the battlefield this turn") {
            r.flags |= ENTERED;
        }
        if with("that dealt damage this turn") || with("that dealt combat damage this turn") {
            // Dealt (not was dealt): see `Obj::dealt`.
            r.flags |= DAMAGED | ENTERED;
        } else if with("dealt damage this turn") || with("was dealt damage") {
            r.flags |= DAMAGED;
        }
        if with("this turn") && (with("graveyard") || with("card")) && !with("entered") {
            r.flags |= THIS_TURN;
        }
        if with("aren't of the chosen") || with("not of the chosen") {
            r.flags |= NOT_CHOSEN_TYPE;
        } else if with("chosen type") || with("chosen creature type") {
            r.flags |= CHOSEN_TYPE;
        }
        if with("chosen color") {
            r.flags |= CHOSEN_COLOR;
        }
        if with("greatest mana value") {
            r.flags |= GREATEST_MV;
        }
        if with("greatest power") {
            r.flags |= GREATEST_POWER;
        }
        if with("target player controls")
            || with("target opponent controls")
            || with("target player's")
            || with("target opponent's")
        {
            r.flags |= ONE_PLAYER;
        }
        // Numbers: "mana value 3 or less", "power 4 or greater", "toughness 2 or less",
        // "power or toughness 1 or less", "mana value X or less", "mana value 2".
        for (key, which) in [
            ("power or toughness ", 3),
            ("mana value ", 0),
            ("power ", 1),
            ("toughness ", 2),
        ] {
            let mut from = 0;
            while let Some(off) = p[from..].find(key) {
                let i = from + off;
                from = i + key.len();
                // "power or toughness" was read already; "toughness" inside it isn't a bound.
                if which != 3 && p[..i].ends_with("power or ") {
                    continue;
                }
                if which == 1 && p[from..].starts_with("or toughness") {
                    continue;
                }
                let rest: Vec<&str> = p[from..].split_whitespace().take(8).collect();
                let w0 = rest.first().copied().unwrap_or("");
                let tail = rest.get(1..).map(|t| t.join(" ")).unwrap_or_default();
                let (lo, hi, dynamic) = if w0 == "x" && tail.starts_with("or less") {
                    (None, None, MV_LE_X)
                } else if w0 == "less" && tail.contains("~'s power") {
                    (None, None, MV_LE_SRC_POWER)
                } else if let Some(n) = num(w0) {
                    if tail.starts_with("or less") {
                        (None, Some(n), 0)
                    } else if tail.starts_with("or greater") || tail.starts_with("or more") {
                        (Some(n), None, 0)
                    } else if which == 0 {
                        (Some(n), Some(n), 0)
                    } else {
                        (None, None, 0)
                    }
                } else if w0 == "less" || w0 == "greater" {
                    // "power less than or equal to 2", "greater than 3"; an amount the game
                    // decides ("the amount of life you gained") isn't a bound here.
                    match rest.iter().find_map(|w| num(w)) {
                        Some(n) if w0 == "less" => (None, Some(n), 0),
                        Some(n) => (Some(n), None, 0),
                        None => (None, None, 0),
                    }
                } else {
                    (None, None, 0)
                };
                r.flags |= dynamic;
                let c8 = |n: i32| n.clamp(-100, 100) as i8;
                match which {
                    0 => {
                        r.mv_min = lo.map(|n| n.clamp(0, 255) as u8).or(r.mv_min);
                        r.mv_max = hi.map(|n| n.clamp(0, 255) as u8).or(r.mv_max);
                    }
                    1 => {
                        r.power_min = lo.map(c8).or(r.power_min);
                        r.power_max = hi.map(c8).or(r.power_max);
                    }
                    2 => {
                        r.tough_min = lo.map(c8).or(r.tough_min);
                        r.tough_max = hi.map(c8).or(r.tough_max);
                    }
                    _ => r.pt_or_max = hi.map(c8).or(r.pt_or_max),
                }
            }
        }
        let qualified = r.flags
            & (FLYING
                | NO_FLYING
                | DEATHTOUCH
                | HASTE
                | COUNTER
                | TAPPED
                | UNTAPPED
                | ATTACKING
                | BLOCKING)
            != 0
            || r.mv_min.is_some()
            || r.mv_max.is_some()
            || r.power_min.is_some()
            || r.power_max.is_some()
            || r.tough_min.is_some()
            || r.tough_max.is_some()
            || r.pt_or_max.is_some();
        // "artifact, enchantment, or creature with flying": the qualifier is the last noun's.
        if qualified && r.any_types.count_ones() > 1 && last_type == T_CREATURE {
            r.qual_types = T_CREATURE;
        }
        r
    }

    /// Does `o` qualify? `d` holds the values only the game knows.
    pub fn matches(&self, o: &Obj, d: &Dyn) -> bool {
        let r = self;
        if r.all_types != 0 && o.types & r.all_types != r.all_types {
            return false;
        }
        if r.not_types != 0 && o.types & r.not_types != 0 && !r.has(SUB_OR_TYPE) {
            return false;
        }
        // Qualifiers only for the qualified card types (when the list mixes types).
        let base_ok =
            r.any_types != 0 && r.qual_types != 0 && o.types & (r.any_types & !r.qual_types) != 0;
        let has_sub = |id: u16| o.subtypes.iter().any(|s| s == subtype_name(id));
        // "A Villain and/or artifact": either will do.
        let sub_or_type = r.has(SUB_OR_TYPE)
            && (r.any_types != 0 && o.types & r.any_types != 0
                || r.not_types != 0 && o.types & r.not_types == 0
                || r.subtype_ids().any(has_sub));
        if r.has(SUB_OR_TYPE) && !sub_or_type {
            return false;
        }
        if !r.has(SUB_OR_TYPE) && r.any_types != 0 && o.types & r.any_types == 0 {
            return false;
        }
        if r.colors != 0 && o.colors & r.colors == 0 {
            return false;
        }
        if r.not_colors != 0 && o.colors & r.not_colors != 0 {
            return false;
        }
        let ncol = o.colors.count_ones();
        if r.has(MULTICOLORED) && ncol < 2
            || r.has(MONOCOLORED) && ncol != 1
            || r.has(COLORLESS) && ncol != 0
        {
            return false;
        }
        if r.subtypes[0] != 0 && !r.has(SUB_OR_TYPE) && !r.subtype_ids().any(has_sub) {
            return false;
        }
        if r.not_subtype != 0 && has_sub(r.not_subtype) {
            return false;
        }
        if r.has(CHOSEN_TYPE) {
            match d.chosen_type {
                Some(id) if has_sub(id) => {}
                _ => return false,
            }
        }
        if r.has(NOT_CHOSEN_TYPE) && d.chosen_type.is_some_and(has_sub) {
            return false;
        }
        if r.has(CHOSEN_COLOR) && o.colors & d.chosen_color == 0 {
            return false;
        }
        if r.has(TOKEN) && !o.token || r.has(NONTOKEN) && o.token {
            return false;
        }
        if r.has(THIS_TURN) && !d.this_turn {
            return false;
        }
        if base_ok {
            return true;
        }
        use crate::playtest::compile as k;
        let kw = |b: u32| o.kw & b != 0;
        if r.has(TAPPED) && !o.tapped || r.has(UNTAPPED) && o.tapped {
            return false;
        }
        let combat = r.flags & (ATTACKING | BLOCKING);
        if combat == ATTACKING && !o.attacking
            || combat == BLOCKING && !o.blocking
            || combat == ATTACKING | BLOCKING && !(o.attacking || o.blocking)
        {
            return false;
        }
        if r.has(FLYING) && !kw(k::FLYING)
            || r.has(NO_FLYING) && kw(k::FLYING)
            || r.has(DEATHTOUCH) && !kw(k::DEATHTOUCH)
            || r.has(HASTE) && !kw(k::HASTE)
            || r.has(VIGILANCE) && !kw(k::VIGILANCE)
            || r.has(DEFENDER) && !kw(k::DEFENDER)
        {
            return false;
        }
        // DAMAGED with ENTERED: "that dealt damage this turn".
        let dealt = r.has(DAMAGED) && r.has(ENTERED);
        if r.has(COUNTER) && o.counters <= 0
            || r.has(EQUIPPED) && !o.equipped
            || dealt && !o.dealt
            || !dealt && r.has(ENTERED) && !o.entered
            || !dealt && r.has(DAMAGED) && !o.damaged
        {
            return false;
        }
        let mv = o.mv as i32;
        if r.mv_min.is_some_and(|n| mv < n as i32) || r.mv_max.is_some_and(|n| mv > n as i32) {
            return false;
        }
        if r.has(MV_LE_X) && d.x.is_some_and(|x| mv > x) {
            return false;
        }
        if r.has(MV_LE_SRC_POWER) && d.src_power.is_some_and(|x| mv > x) {
            return false;
        }
        let (pw, tg) = (o.power as i32, o.toughness as i32);
        if r.power_min.is_some_and(|n| pw < n as i32)
            || r.power_max.is_some_and(|n| pw > n as i32)
            || r.tough_min.is_some_and(|n| tg < n as i32)
            || r.tough_max.is_some_and(|n| tg > n as i32)
            || r.pt_or_max.is_some_and(|n| pw > n as i32 && tg > n as i32)
        {
            return false;
        }
        true
    }
}

/// The restriction on what an ability's card-moving or sacrificing part picks: "return
/// target creature card with mana value 2 or less from your graveyard", "return target
/// Zombie creature card", "each opponent sacrifices a creature with the greatest mana value
/// among creatures they control", "search your library for a Gate card".
pub fn object_restr(text: &str) -> Restr {
    use regex::Regex;
    use std::sync::OnceLock;
    static CARD: OnceLock<Regex> = OnceLock::new();
    let card = CARD.get_or_init(|| {
        Regex::new(
            r"(?:return|put|search (?:your|their) library(?: and/or graveyard)? for|choose|exile) (?:up to (?:one|two|three|x|\d+) |all |each |any number of )?(?:other |another )?(?:target )?(?:other |another )?([^.;:]*?\bcards?\b[^.;:]*?)(?: from | to the | to your | to its | to their | onto | into | in your| on top|,|\.|;|$)",
        )
        .unwrap()
    });
    // A sacrifice (unless the ability also moves a card: that's what needs choosing well).
    if let Some(i) = text
        .find(" sacrifices ")
        .filter(|_| !text.contains(" card"))
    {
        let rest = &text[i + 12..];
        // "... sacrifices a creature or planeswalker of their choice".
        let rest = rest.split(" of their choice").next().unwrap_or(rest);
        return Restr::read(rest.split(" among ").next().unwrap_or(rest).trim_end())
            .with_greatest(rest);
    }
    // The card a "return"/"put" moves; else one a search or choice finds.
    let mut best: Option<Restr> = None;
    let mut from_exile = false;
    for c in card.captures_iter(text) {
        let verb = c.get(0).map(|m| m.as_str()).unwrap_or("");
        let phrase = &c[1];
        // "Exile ... cards from graveyards" isn't what's put back.
        if verb.starts_with("exile") && best.is_some() {
            continue;
        }
        // "Return the chosen card / that card": the one chosen before.
        let moves = verb.starts_with("return") || verb.starts_with("put");
        let back = [
            "the chosen card",
            "that card",
            "those cards",
            "the exiled card",
        ]
        .iter()
        .any(|w| phrase.trim().starts_with(w));
        if moves && best.is_some() && back {
            break;
        }
        // "Choose target creature card in your graveyard with mana value 4 or less": the
        // whole sentence says what fits.
        let r = if verb.starts_with("choose") {
            let start = c.get(1).map_or(0, |m| m.start());
            Restr::read(text[start..].split(". ").next().unwrap_or(phrase))
        } else {
            Restr::read(phrase)
        };
        // "Exile ~ and four cards named ~ ... If you do, search your library for a Sphinx
        // creature card": the search is what finds a card.
        let search = verb.starts_with("search") && from_exile;
        if best.is_none() || moves || search {
            best = Some(r);
            from_exile = verb.starts_with("exile");
            if moves {
                break;
            }
        }
    }
    best.unwrap_or_default()
}

impl Restr {
    /// "with the greatest mana value among ..." also counts when the phrase was cut before
    /// "among".
    fn with_greatest(mut self, full: &str) -> Restr {
        if full.contains("greatest mana value") {
            self.flags |= GREATEST_MV;
        }
        if full.contains("greatest power") {
            self.flags |= GREATEST_POWER;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_restrictions() {
        assert_eq!(subtype_word("sorcery"), None);
        assert_eq!(subtype_word("elves").map(subtype_name), Some("elf"));
        assert_eq!(subtype_word("itself"), None);
        assert!(!mentions_subtype("instant and sorcery spells", "orc"));
        let r = Restr::read("creatures with mana value 3 or less");
        assert_eq!((r.mv_min, r.mv_max), (None, Some(3)));
        let r = Restr::read("creatures with mana value 4 or greater");
        assert_eq!((r.mv_min, r.mv_max), (Some(4), None));
        let r = Restr::read("non-dragon creatures");
        assert_eq!(subtype_name(r.not_subtype), "dragon");
        let r = Restr::read("dragon creatures");
        assert_eq!(subtype_name(r.subtypes[0]), "dragon");
        let r = Restr::read("artifact, enchantment, or creature with flying");
        assert!(r.has(FLYING));
        assert_eq!(r.qual_types, T_CREATURE);
        assert_eq!(r.any_types, T_ARTIFACT | T_ENCHANTMENT | T_CREATURE);
        let r = Restr::read("creature with power or toughness 1 or less");
        assert_eq!(
            (r.pt_or_max, r.power_max, r.tough_max),
            (Some(1), None, None)
        );
        let r = Restr::read("bear, spider, or wolf you control");
        assert_eq!(r.subtype_ids().count(), 3);
        let r = Restr::read("black or green creature or planeswalker");
        assert_eq!(r.colors, 0b10100);
        let r = Restr::read("creature card with mana value less than or equal to ~'s power");
        assert!(r.has(MV_LE_SRC_POWER));
        assert_eq!(r.any_types, T_CREATURE);
        let r = object_restr(
            "when ~ dies, return target creature card with mana value 2 or less from your graveyard to the battlefield.",
        );
        assert_eq!((r.any_types, r.mv_max), (T_CREATURE, Some(2)));
        let r = object_restr(
            "each opponent sacrifices a creature with the greatest mana value among creatures they control.",
        );
        assert!(r.has(GREATEST_MV));
        let r = object_restr(
            "return target zombie creature card from your graveyard to the battlefield.",
        );
        assert_eq!(subtype_name(r.subtypes[0]), "zombie");
        let r = object_restr(
            "when ~ dies, return another target non-bear creature card with mana value less than or equal to ~'s power from your graveyard to the battlefield.",
        );
        assert!(r.has(MV_LE_SRC_POWER), "{:?}", r);
        assert_eq!(subtype_name(r.not_subtype), "bear");
    }
}
