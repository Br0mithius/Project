//! Card database, owned collection and format rules, all stored as plain files under `data/`.

use crate::card::{BASIC_FOR_COLOR, COLORS, Card};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Lower-cases and strips punctuation so "Ka-Zar" matches "kazar" and curly quotes match straight ones.
pub fn normalize(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub struct CardDb {
    pub cards: Vec<Card>,
}

pub enum Lookup<'a> {
    Found(&'a Card),
    Ambiguous(Vec<&'a Card>),
    NotFound(Vec<&'a Card>),
}

impl CardDb {
    pub fn load(path: &Path) -> Result<CardDb, String> {
        let text = fs::read_to_string(path)
            .map_err(|e| format!("could not read {}: {}", path.display(), e))?;
        let mut cards: Vec<Card> = serde_json::from_str(&text)
            .map_err(|e| format!("could not parse {}: {}", path.display(), e))?;
        for (i, name) in BASIC_FOR_COLOR.iter().enumerate() {
            if !cards.iter().any(|c| c.name == *name) {
                cards.push(Card::basic_land(name, COLORS[i]));
            }
        }
        cards.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(CardDb { cards })
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let custom: Vec<&Card> = self.cards.iter().filter(|c| !c.is_basic()).collect();
        let json = serde_json::to_string_pretty(&custom).map_err(|e| e.to_string())?;
        fs::write(path, json + "\n")
            .map_err(|e| format!("could not write {}: {}", path.display(), e))
    }

    pub fn get(&self, name: &str) -> Option<&Card> {
        let n = normalize(name);
        self.cards.iter().find(|c| normalize(&c.name) == n)
    }

    /// Resolves what the user typed: exact, then unique prefix, then unique substring,
    /// then every word present. Falls back to close spellings for "did you mean".
    pub fn lookup(&self, query: &str) -> Lookup<'_> {
        let q = normalize(query);
        if q.is_empty() {
            return Lookup::NotFound(vec![]);
        }
        if let Some(c) = self.get(query) {
            return Lookup::Found(c);
        }
        let tiers: [Box<dyn Fn(&str) -> bool>; 3] = [
            Box::new(|n: &str| n.starts_with(&q)),
            Box::new(|n: &str| n.contains(&q)),
            Box::new(|n: &str| q.split(' ').all(|w| n.contains(w))),
        ];
        for tier in tiers.iter() {
            let hits: Vec<&Card> = self
                .cards
                .iter()
                .filter(|c| tier(&normalize(&c.name)))
                .collect();
            match hits.len() {
                0 => continue,
                1 => return Lookup::Found(hits[0]),
                _ => return Lookup::Ambiguous(hits),
            }
        }
        let mut close: Vec<(usize, &Card)> = self
            .cards
            .iter()
            .map(|c| (edit_distance(&q, &normalize(&c.name)), c))
            .filter(|(d, _)| *d <= 4.max(q.len() / 3))
            .collect();
        close.sort_by_key(|(d, _)| *d);
        Lookup::NotFound(close.into_iter().take(5).map(|(_, c)| c).collect())
    }
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Parses "2 Roxxon Brutes", "2x Roxxon Brutes" or "Roxxon Brutes" into (count, name).
pub fn parse_count_line(line: &str) -> Option<(u32, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
        return None;
    }
    let (first, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    let num = first.trim_end_matches(['x', 'X']);
    match num.parse::<u32>() {
        Ok(n) if !rest.trim().is_empty() => Some((n, rest.trim().to_string())),
        _ => Some((1, line.to_string())),
    }
}

/// The cards you physically own, kept in `data/collection.txt` as "count name" lines.
pub struct Collection {
    pub path: PathBuf,
    pub counts: BTreeMap<String, u32>,
}

impl Collection {
    pub fn load(path: &Path) -> Result<Collection, String> {
        let mut counts = BTreeMap::new();
        if path.exists() {
            let text = fs::read_to_string(path)
                .map_err(|e| format!("could not read {}: {}", path.display(), e))?;
            for line in text.lines() {
                if let Some((n, name)) = parse_count_line(line) {
                    *counts.entry(name).or_insert(0) += n;
                }
            }
        }
        Ok(Collection {
            path: path.to_path_buf(),
            counts,
        })
    }

    pub fn owned(&self, name: &str) -> u32 {
        let n = normalize(name);
        self.counts
            .iter()
            .filter(|(k, _)| normalize(k) == n)
            .map(|(_, v)| *v)
            .sum()
    }

    pub fn total(&self) -> u32 {
        self.counts.values().sum()
    }

    /// Adjusts a count (negative removes) and rewrites the file, keeping comment lines.
    pub fn adjust(&mut self, name: &str, delta: i64) -> Result<u32, String> {
        let key = self
            .counts
            .keys()
            .find(|k| normalize(k) == normalize(name))
            .cloned()
            .unwrap_or_else(|| name.to_string());
        let cur = *self.counts.get(&key).unwrap_or(&0) as i64;
        let new = (cur + delta).max(0) as u32;
        if new == 0 {
            self.counts.remove(&key);
        } else {
            self.counts.insert(key.clone(), new);
        }
        self.rewrite(&key, new)?;
        Ok(new)
    }

    fn rewrite(&self, key: &str, new: u32) -> Result<(), String> {
        let text = fs::read_to_string(&self.path).unwrap_or_default();
        let mut out = Vec::new();
        let mut written = false;
        for line in text.lines() {
            match parse_count_line(line) {
                Some((_, name)) if normalize(&name) == normalize(key) => {
                    if !written && new > 0 {
                        out.push(format!("{} {}", new, key));
                    }
                    written = true;
                }
                _ => out.push(line.to_string()),
            }
        }
        if !written && new > 0 {
            out.push(format!("{} {}", new, key));
        }
        fs::write(&self.path, out.join("\n") + "\n")
            .map_err(|e| format!("could not write {}: {}", self.path.display(), e))
    }
}

/// House rules for the format; editable in-app with `rules <key> <value>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rules {
    pub deck_size: u32,
    /// Whether the commander is one of the `deck_size` cards.
    pub commander_counts_toward_size: bool,
    /// 0 = no limit beyond what you own.
    pub max_copies: u32,
    /// Basic lands are free and unlimited (like a draft/sealed land station).
    pub unlimited_basics: bool,
    pub enforce_color_identity: bool,
    /// Only allow cards from the collection file.
    pub enforce_collection: bool,
    pub starting_life: u32,
    /// One copy of each card (basic lands excepted). Turned on by the league from Session 6.
    #[serde(default)]
    pub singleton: bool,
    /// League session the rules were set for (0 = custom).
    #[serde(default)]
    pub session: u32,
    /// The Prismatic Piper is available without owning it (league partner rule).
    #[serde(default = "yes")]
    pub piper_free: bool,
}

fn yes() -> bool {
    true
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            deck_size: 60,
            commander_counts_toward_size: true,
            max_copies: 0,
            unlimited_basics: true,
            enforce_color_identity: true,
            enforce_collection: true,
            starting_life: 30,
            singleton: false,
            session: 0,
            piper_free: true,
        }
    }
}

impl Rules {
    pub fn load(path: &Path) -> Rules {
        fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(path, json + "\n")
            .map_err(|e| format!("could not write {}: {}", path.display(), e))
    }

    /// Cards that go in the library (everything but the commander).
    pub fn main_deck_size(&self) -> u32 {
        if self.commander_counts_toward_size {
            self.deck_size.saturating_sub(1)
        } else {
            self.deck_size
        }
    }

    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let parse_bool = |v: &str| match v.to_lowercase().as_str() {
            "true" | "yes" | "on" | "1" => Ok(true),
            "false" | "no" | "off" | "0" => Ok(false),
            _ => Err(format!("'{}' is not yes/no", v)),
        };
        let parse_num = |v: &str| {
            v.parse::<u32>()
                .map_err(|_| format!("'{}' is not a number", v))
        };
        match key {
            "deck_size" => self.deck_size = parse_num(value)?,
            "commander_counts_toward_size" => {
                self.commander_counts_toward_size = parse_bool(value)?
            }
            "max_copies" => self.max_copies = parse_num(value)?,
            "unlimited_basics" => self.unlimited_basics = parse_bool(value)?,
            "enforce_color_identity" => self.enforce_color_identity = parse_bool(value)?,
            "enforce_collection" => self.enforce_collection = parse_bool(value)?,
            "starting_life" => self.starting_life = parse_num(value)?,
            "singleton" => self.singleton = parse_bool(value)?,
            "piper_free" => self.piper_free = parse_bool(value)?,
            _ => return Err(format!("unknown rule '{}'", key)),
        }
        // Any manual change means the rules no longer match a league session exactly.
        self.session = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_lines() {
        assert_eq!(
            parse_count_line("2 Roxxon Brutes"),
            Some((2, "Roxxon Brutes".into()))
        );
        assert_eq!(parse_count_line("3x Web Up"), Some((3, "Web Up".into())));
        assert_eq!(parse_count_line("Red Hulk"), Some((1, "Red Hulk".into())));
        assert_eq!(parse_count_line("# comment"), None);
    }

    #[test]
    fn lookup_is_forgiving() {
        let db = CardDb {
            cards: vec![
                Card {
                    name: "Ka-Zar of the Savage Land".into(),
                    ..Default::default()
                },
                Card {
                    name: "Red Hulk".into(),
                    ..Default::default()
                },
                Card {
                    name: "Ultron Drone".into(),
                    ..Default::default()
                },
                Card {
                    name: "Ultron, Artificial Malevolence".into(),
                    ..Default::default()
                },
            ],
        };
        assert!(matches!(db.lookup("kazar"), Lookup::Found(c) if c.name.starts_with("Ka-Zar")));
        assert!(matches!(db.lookup("red hlk"), Lookup::NotFound(v) if v[0].name == "Red Hulk"));
        assert!(matches!(db.lookup("ultron"), Lookup::Ambiguous(v) if v.len() == 2));
        assert!(matches!(db.lookup("ultron drone"), Lookup::Found(_)));
    }
}
