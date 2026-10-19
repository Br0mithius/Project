//! A deck (commander + main deck) and its validation against the format rules.

use crate::card::ColorSet;
use crate::db::{CardDb, Collection, Rules, normalize, parse_count_line};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Deck {
    pub name: String,
    pub commander: Option<String>,
    /// Main deck only; the commander is not in here.
    pub cards: BTreeMap<String, u32>,
}

#[derive(Debug, PartialEq)]
pub enum Severity {
    Error,
    Warning,
}

pub struct Problem {
    pub severity: Severity,
    pub message: String,
}

impl Deck {
    pub fn new(name: &str) -> Deck {
        Deck {
            name: name.to_string(),
            ..Default::default()
        }
    }

    pub fn main_count(&self) -> u32 {
        self.cards.values().sum()
    }

    pub fn total_count(&self, rules: &Rules) -> u32 {
        self.main_count()
            + if rules.commander_counts_toward_size && self.commander.is_some() {
                1
            } else {
                0
            }
    }

    pub fn count_of(&self, name: &str) -> u32 {
        *self.cards.get(name).unwrap_or(&0)
    }

    /// Copies of a card used by this deck, including the commander.
    pub fn used(&self, name: &str) -> u32 {
        self.count_of(name)
            + if self.commander.as_deref() == Some(name) {
                1
            } else {
                0
            }
    }

    pub fn add(&mut self, name: &str, n: u32) {
        *self.cards.entry(name.to_string()).or_insert(0) += n;
    }

    pub fn remove(&mut self, name: &str, n: u32) -> u32 {
        let cur = self.count_of(name);
        let take = n.min(cur);
        if cur - take == 0 {
            self.cards.remove(name);
        } else {
            self.cards.insert(name.to_string(), cur - take);
        }
        take
    }

    pub fn identity(&self, db: &CardDb) -> Option<ColorSet> {
        self.commander
            .as_ref()
            .and_then(|c| db.get(c))
            .map(|c| c.identity())
    }

    /// Text format understood by Arena/Moxfield/Archidekt importers.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        if let Some(c) = &self.commander {
            out.push_str(&format!("Commander\n1 {}\n\n", c));
        }
        out.push_str("Deck\n");
        for (name, n) in &self.cards {
            out.push_str(&format!("{} {}\n", n, name));
        }
        out
    }

    pub fn from_text(name: &str, text: &str, db: &CardDb) -> Deck {
        let mut deck = Deck::new(name);
        let mut section = "deck";
        for line in text.lines() {
            let l = line.trim().to_lowercase();
            match l.trim_end_matches(':') {
                "commander" => {
                    section = "commander";
                    continue;
                }
                "deck" | "main" | "mainboard" => {
                    section = "deck";
                    continue;
                }
                "sideboard" | "maybeboard" => {
                    section = "skip";
                    continue;
                }
                _ => {}
            }
            let Some((n, raw)) = parse_count_line(line) else {
                continue;
            };
            let canonical = db.get(&raw).map(|c| c.name.clone()).unwrap_or(raw);
            match section {
                "commander" => deck.commander = Some(canonical),
                "deck" => deck.add(&canonical, n),
                _ => {}
            }
        }
        deck
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        fs::write(path, self.to_text())
            .map_err(|e| format!("could not write {}: {}", path.display(), e))
    }

    pub fn load(path: &Path, db: &CardDb) -> Result<Deck, String> {
        let text = fs::read_to_string(path)
            .map_err(|e| format!("could not read {}: {}", path.display(), e))?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        Ok(Deck::from_text(&name, &text, db))
    }

    pub fn validate(&self, db: &CardDb, coll: &Collection, rules: &Rules) -> Vec<Problem> {
        let mut out = Vec::new();
        let mut err = |s: String| {
            out.push(Problem {
                severity: Severity::Error,
                message: s,
            })
        };
        let identity = self.identity(db);
        match &self.commander {
            None => err("No commander chosen (use `commander <name>`).".into()),
            Some(c) => match db.get(c) {
                None => err(format!("Commander '{}' is not in the card database.", c)),
                Some(card) if !card.can_be_commander() => err(format!(
                    "{} is not a legendary creature, so it can't be your commander.",
                    c
                )),
                _ => {}
            },
        }
        let total = self.total_count(rules);
        if total != rules.deck_size {
            err(format!(
                "Deck has {} cards{}; the format needs exactly {}.",
                total,
                if rules.commander_counts_toward_size {
                    " (including commander)"
                } else {
                    ""
                },
                rules.deck_size
            ));
        }
        let mut names: Vec<&String> = self.cards.keys().collect();
        if let Some(c) = &self.commander {
            names.push(c);
        }
        names.sort();
        names.dedup();
        let mut warnings = Vec::new();
        for name in names {
            let used = self.used(name);
            let Some(card) = db.get(name) else {
                err(format!(
                    "'{}' is not in the card database (check spelling).",
                    name
                ));
                continue;
            };
            let basic_free = card.is_basic() && rules.unlimited_basics;
            if rules.enforce_collection && !basic_free {
                let owned = coll.owned(name);
                if used > owned {
                    err(format!(
                        "{}: using {} but you only own {}.",
                        name, used, owned
                    ));
                }
            }
            if rules.max_copies > 0 && !card.is_basic() && used > rules.max_copies {
                err(format!(
                    "{}: {} copies is over the limit of {}.",
                    name, used, rules.max_copies
                ));
            }
            if rules.enforce_color_identity {
                if let Some(id) = identity {
                    if !card.identity().is_subset_of(id) {
                        err(format!(
                            "{} ({}) is outside your commander's color identity ({}).",
                            name,
                            card.identity(),
                            id
                        ));
                    }
                }
            }
            if !card.is_verified() && !card.is_basic() {
                warnings.push(format!(
                    "{}: card text is only {} - double-check it against the physical card.",
                    name,
                    if card.confidence.is_empty() {
                        "unverified"
                    } else {
                        &card.confidence
                    }
                ));
            }
        }
        for w in warnings {
            out.push(Problem {
                severity: Severity::Warning,
                message: w,
            });
        }
        out
    }
}

/// Finds a deck entry by loose name, e.g. "hulk" -> "Red Hulk".
pub fn find_in_deck<'a>(deck: &'a Deck, query: &str) -> Vec<&'a String> {
    let q = normalize(query);
    let exact: Vec<&String> = deck.cards.keys().filter(|k| normalize(k) == q).collect();
    if !exact.is_empty() {
        return exact;
    }
    deck.cards
        .keys()
        .filter(|k| normalize(k).contains(&q))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::Card;
    use std::path::PathBuf;

    fn db() -> CardDb {
        CardDb {
            cards: vec![
                Card {
                    name: "Red Hulk".into(),
                    mana_cost: "{4}{R}{R}".into(),
                    type_line: "Legendary Creature — Gamma Berserker Villain".into(),
                    color_identity: vec!["R".into()],
                    confidence: "verified".into(),
                    ..Default::default()
                },
                Card {
                    name: "Web Up".into(),
                    mana_cost: "{2}{W}".into(),
                    type_line: "Enchantment".into(),
                    color_identity: vec!["W".into()],
                    confidence: "verified".into(),
                    ..Default::default()
                },
                Card::basic_land("Mountain", 'R'),
            ],
        }
    }

    fn coll() -> Collection {
        let mut counts = BTreeMap::new();
        counts.insert("Red Hulk".to_string(), 1);
        counts.insert("Web Up".to_string(), 2);
        Collection {
            path: PathBuf::new(),
            counts,
        }
    }

    #[test]
    fn round_trip_text() {
        let db = db();
        let mut d = Deck::new("t");
        d.commander = Some("Red Hulk".into());
        d.add("Mountain", 20);
        let back = Deck::from_text("t", &d.to_text(), &db);
        assert_eq!(back, d);
    }

    #[test]
    fn validation_catches_problems() {
        let db = db();
        let rules = Rules::default();
        let mut d = Deck::new("t");
        d.commander = Some("Red Hulk".into());
        d.add("Web Up", 3);
        d.add("Mountain", 56);
        let msgs: Vec<String> = d
            .validate(&db, &coll(), &rules)
            .into_iter()
            .map(|p| p.message)
            .collect();
        assert!(msgs.iter().any(|m| m.contains("only own 2")), "{:?}", msgs);
        assert!(
            msgs.iter().any(|m| m.contains("color identity")),
            "{:?}",
            msgs
        );
        assert!(
            !msgs.iter().any(|m| m.contains("needs exactly")),
            "{:?}",
            msgs
        );

        d.remove("Web Up", 3);
        d.add("Mountain", 3);
        assert!(d.validate(&db, &coll(), &rules).is_empty());
    }
}
