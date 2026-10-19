//! Card data model plus the rules-text heuristics used for analysis.

use serde::{Deserialize, Serialize};
use std::fmt;

/// WUBRG order, the canonical Magic colour order.
pub const COLORS: [char; 5] = ['W', 'U', 'B', 'R', 'G'];
pub const BASIC_FOR_COLOR: [&str; 5] = ["Plains", "Island", "Swamp", "Mountain", "Forest"];

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Card {
    pub name: String,
    #[serde(default)]
    pub mana_cost: String,
    #[serde(default)]
    pub cmc: f32,
    #[serde(default)]
    pub type_line: String,
    #[serde(default)]
    pub oracle_text: String,
    #[serde(default)]
    pub power: Option<String>,
    #[serde(default)]
    pub toughness: Option<String>,
    #[serde(default)]
    pub rarity: String,
    #[serde(default)]
    pub color_identity: Vec<String>,
    #[serde(default)]
    pub set: String,
    #[serde(default)]
    pub collector_number: Option<String>,
    /// "verified", "partial" or "not_found" - how sure we are about the text.
    #[serde(default)]
    pub confidence: String,
    #[serde(default)]
    pub note: String,
}

/// A set of colours stored as a bitmask in WUBRG order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct ColorSet(pub u8);

impl ColorSet {
    pub fn from_char(c: char) -> ColorSet {
        match COLORS.iter().position(|&x| x == c.to_ascii_uppercase()) {
            Some(i) => ColorSet(1 << i),
            None => ColorSet(0),
        }
    }

    /// Parses things like "RG", "wu", "red", "colorless".
    pub fn parse(s: &str) -> Option<ColorSet> {
        let s = s.trim().to_lowercase();
        let named = [
            ("white", 'W'),
            ("blue", 'U'),
            ("black", 'B'),
            ("red", 'R'),
            ("green", 'G'),
        ];
        if let Some((_, c)) = named.iter().find(|(n, _)| *n == s) {
            return Some(ColorSet::from_char(*c));
        }
        if s == "c" || s == "colorless" {
            return Some(ColorSet(0));
        }
        let mut set = ColorSet(0);
        for ch in s.chars() {
            let one = ColorSet::from_char(ch);
            if one.0 == 0 {
                return None;
            }
            set = set.union(one);
        }
        Some(set)
    }

    pub fn union(self, other: ColorSet) -> ColorSet {
        ColorSet(self.0 | other.0)
    }

    pub fn contains(self, i: usize) -> bool {
        self.0 & (1 << i) != 0
    }

    pub fn is_subset_of(self, other: ColorSet) -> bool {
        self.0 & !other.0 == 0
    }

    pub fn count(self) -> u32 {
        self.0.count_ones()
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn indices(self) -> impl Iterator<Item = usize> {
        (0..5).filter(move |&i| self.contains(i))
    }
}

impl fmt::Display for ColorSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return write!(f, "C");
        }
        for i in self.indices() {
            write!(f, "{}", COLORS[i])?;
        }
        Ok(())
    }
}

/// What a card does in a deck, guessed from its rules text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    Removal,
    Bounce,
    Counter,
    Draw,
    Ramp,
    Tokens,
    Trick,
    Evasion,
    Protection,
    Lifegain,
    Teamwork,
    Enrage,
}

impl Role {
    pub const ALL: [Role; 12] = [
        Role::Removal,
        Role::Bounce,
        Role::Counter,
        Role::Draw,
        Role::Ramp,
        Role::Tokens,
        Role::Trick,
        Role::Evasion,
        Role::Protection,
        Role::Lifegain,
        Role::Teamwork,
        Role::Enrage,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Role::Removal => "removal",
            Role::Bounce => "bounce",
            Role::Counter => "counter",
            Role::Draw => "draw",
            Role::Ramp => "ramp",
            Role::Tokens => "tokens",
            Role::Trick => "trick",
            Role::Evasion => "evasion",
            Role::Protection => "protection",
            Role::Lifegain => "lifegain",
            Role::Teamwork => "teamwork",
            Role::Enrage => "enrage",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        let s = s.to_lowercase();
        Role::ALL.into_iter().find(|r| r.label() == s)
    }
}

impl Card {
    pub fn basic_land(name: &str, color: char) -> Card {
        Card {
            name: name.to_string(),
            type_line: format!("Basic Land — {}", name),
            oracle_text: format!("({{T}}: Add {{{}}}.)", color),
            rarity: "basic".into(),
            color_identity: vec![color.to_string()],
            confidence: "verified".into(),
            ..Default::default()
        }
    }

    /// Splits "{4}{R}{R}" into ["4", "R", "R"].
    pub fn cost_symbols(&self) -> Vec<String> {
        symbols_in(&self.mana_cost)
    }

    /// Coloured pips in the mana cost, WUBRG order. Hybrid symbols count for both colours.
    pub fn pips(&self) -> [u32; 5] {
        let mut out = [0; 5];
        for sym in self.cost_symbols() {
            for part in sym.split('/') {
                if let Some(i) = COLORS.iter().position(|c| part == c.to_string()) {
                    out[i] += 1;
                }
            }
        }
        out
    }

    pub fn identity(&self) -> ColorSet {
        if !self.color_identity.is_empty() {
            return self
                .color_identity
                .iter()
                .filter_map(|s| s.chars().next())
                .fold(ColorSet(0), |acc, c| acc.union(ColorSet::from_char(c)));
        }
        // Fall back to every coloured symbol in the cost and rules text.
        let mut set = ColorSet(0);
        for sym in symbols_in(&self.mana_cost)
            .into_iter()
            .chain(symbols_in(&self.oracle_text))
        {
            for part in sym.split('/') {
                if part.len() == 1 {
                    set = set.union(ColorSet::from_char(part.chars().next().unwrap()));
                }
            }
        }
        set
    }

    /// Colours the mana cost itself needs (ignores colours that only appear in rules text).
    pub fn cost_colors(&self) -> ColorSet {
        let p = self.pips();
        ColorSet(
            (0..5)
                .filter(|&i| p[i] > 0)
                .fold(0, |acc, i| acc | (1 << i)),
        )
    }

    pub fn mana_value(&self) -> u32 {
        self.cmc.round() as u32
    }

    fn type_lower(&self) -> String {
        self.type_line.to_lowercase()
    }

    pub fn is_land(&self) -> bool {
        self.type_lower().contains("land")
    }

    pub fn is_basic(&self) -> bool {
        self.type_lower().contains("basic")
    }

    pub fn is_creature(&self) -> bool {
        self.type_lower().contains("creature")
    }

    pub fn is_legendary(&self) -> bool {
        self.type_lower().contains("legendary")
    }

    pub fn can_be_commander(&self) -> bool {
        (self.is_legendary() && self.is_creature())
            || self
                .oracle_text
                .to_lowercase()
                .contains("can be your commander")
    }

    /// Single bucket used for type breakdowns.
    pub fn main_type(&self) -> &'static str {
        let t = self.type_lower();
        for (needle, label) in [
            ("land", "Land"),
            ("creature", "Creature"),
            ("planeswalker", "Planeswalker"),
            ("instant", "Instant"),
            ("sorcery", "Sorcery"),
            ("artifact", "Artifact"),
            ("enchantment", "Enchantment"),
            ("battle", "Battle"),
        ] {
            if t.contains(needle) {
                return label;
            }
        }
        "Other"
    }

    /// Creature/artifact subtypes after the dash, e.g. ["Gamma", "Berserker", "Villain"].
    pub fn subtypes(&self) -> Vec<String> {
        self.type_line
            .split(['—', '-'])
            .nth(1)
            .map(|s| s.split_whitespace().map(|w| w.to_string()).collect())
            .unwrap_or_default()
    }

    pub fn power_toughness(&self) -> Option<String> {
        match (&self.power, &self.toughness) {
            (Some(p), Some(t)) => Some(format!("{}/{}", p, t)),
            _ => None,
        }
    }

    /// Colours of mana a land (or mana rock) can produce, from "Add {R}" style text.
    pub fn produces(&self) -> ColorSet {
        let text = self.oracle_text.to_lowercase();
        let mut set = ColorSet(0);
        if text.contains("mana of any color") || text.contains("one mana of any") {
            return ColorSet(0b11111);
        }
        for (i, line) in text.split("add ").enumerate() {
            if i == 0 {
                continue;
            }
            let chunk: String = line.chars().take(40).collect();
            for sym in symbols_in(&chunk) {
                if sym.len() == 1 {
                    set = set.union(ColorSet::from_char(sym.chars().next().unwrap()));
                }
            }
        }
        set
    }

    pub fn roles(&self) -> Vec<Role> {
        let t = self.oracle_text.to_lowercase();
        let has = |s: &str| t.contains(s);
        let mut roles = Vec::new();
        if has("destroy target")
            || has("exile target")
            || has("damage to target")
            || has("damage to any target")
            || has("damage to another target")
            || has("damage to each creature")
            || has("damage to each opponent's creature")
            || has("fights")
            || has("fight target")
            || has("gets -")
            || has("sacrifices a creature")
            || has("loses all abilities")
            || has("doesn't untap")
            || has("can't attack or block")
        {
            roles.push(Role::Removal);
        }
        if has("to its owner's hand")
            || has("to their owners' hands")
            || has("to their owner's hand")
        {
            roles.push(Role::Bounce);
        }
        if has("counter target") {
            roles.push(Role::Counter);
        }
        if has("draw a card")
            || has("draw two")
            || has("draw three")
            || has("draws a card")
            || has("investigate")
            || has("clue")
            || (has("look at the top") && has("into your hand"))
        {
            roles.push(Role::Draw);
        }
        if (has("add {") && !self.is_land())
            || has("search your library for a basic land")
            || has("treasure")
        {
            roles.push(Role::Ramp);
        }
        if has("create") && has("token") {
            roles.push(Role::Tokens);
        }
        let ty = self.type_lower();
        if ty.contains("instant") && (has("gets +") || has("gains") || has("gain ")) {
            roles.push(Role::Trick);
        }
        if has("flying") || has("trample") || has("menace") || has("can't be blocked") {
            roles.push(Role::Evasion);
        }
        if has("hexproof") || has("indestructible") || has("ward") || has("protection from") {
            roles.push(Role::Protection);
        }
        if has("gain life") || has("gains life") || has("you gain") || has("lifelink") {
            roles.push(Role::Lifegain);
        }
        if has("teamwork") {
            roles.push(Role::Teamwork);
        }
        if has("enrage") || has("is dealt damage") {
            roles.push(Role::Enrage);
        }
        roles
    }

    pub fn has_role(&self, role: Role) -> bool {
        self.roles().contains(&role)
    }

    pub fn rarity_rank(&self) -> u32 {
        match self.rarity.to_lowercase().as_str() {
            "mythic" => 4,
            "rare" => 3,
            "uncommon" => 2,
            "common" => 1,
            _ => 0,
        }
    }

    pub fn is_verified(&self) -> bool {
        self.confidence == "verified"
    }
}

/// Extracts brace-delimited symbols: "{2}{U}: Draw" -> ["2", "U"].
pub fn symbols_in(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    for ch in s.chars() {
        match ch {
            '{' => cur = Some(String::new()),
            '}' => {
                if let Some(sym) = cur.take() {
                    out.push(sym.to_uppercase());
                }
            }
            c => {
                if let Some(sym) = cur.as_mut() {
                    sym.push(c);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(cost: &str, ty: &str, text: &str) -> Card {
        Card {
            name: "Test".into(),
            mana_cost: cost.into(),
            type_line: ty.into(),
            oracle_text: text.into(),
            ..Default::default()
        }
    }

    #[test]
    fn pips_and_identity() {
        let c = card(
            "{4}{R}{R}",
            "Legendary Creature — Villain",
            "{G}: Regenerate.",
        );
        assert_eq!(c.pips(), [0, 0, 0, 2, 0]);
        assert_eq!(c.identity().to_string(), "RG");
        assert!(c.can_be_commander());
        assert_eq!(c.subtypes(), vec!["Villain"]);
    }

    #[test]
    fn colorset_parse() {
        assert_eq!(ColorSet::parse("rg").unwrap().to_string(), "RG");
        assert_eq!(ColorSet::parse("red").unwrap().to_string(), "R");
        assert!(ColorSet::parse("xyz").is_none());
        assert!(
            ColorSet::parse("r")
                .unwrap()
                .is_subset_of(ColorSet::parse("rg").unwrap())
        );
    }

    #[test]
    fn roles_from_text() {
        let c = card(
            "{3}{R}",
            "Sorcery",
            "Repulsor Blast deals 5 damage to target creature.",
        );
        assert!(c.has_role(Role::Removal));
        let land = card("", "Land", "{T}: Add {R} or {G}.");
        assert_eq!(land.produces().to_string(), "RG");
        assert!(!land.has_role(Role::Ramp));
    }
}
