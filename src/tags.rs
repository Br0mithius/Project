//! Scryfall Tagger tags ("otag"): what the Magic community has tagged each card as good at
//! (removal, ramp, sacrifice outlet...). Used as a second opinion next to the app's own
//! reading of the rules text: a tag adds a role the reader missed, and counts the card as
//! an enabler for a commander that needs that kind of card.
//!
//! Downloaded on your PC only when you ask (Update ratings), with one Scryfall search per
//! tag limited to the league's sets, and saved in `data/scryfall_tags.json`.

use crate::card::Role;
use crate::db::CardDb;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// What a tag means to the app.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Use {
    Role(Role),
    /// Counts as an enabler for the mechanic with this `need` (see `analysis::MECHANICS`).
    Feeds(&'static str),
}

/// Tags the app asks for. Several spellings are listed where Tagger's naming isn't certain;
/// only the ones on Scryfall's published tag list are downloaded.
pub const TAGS: &[(&str, Use)] = &[
    ("removal", Use::Role(Role::Removal)),
    ("creature-removal", Use::Role(Role::Removal)),
    ("spot-removal", Use::Role(Role::Removal)),
    ("board-wipe", Use::Role(Role::Sweeper)),
    ("sweeper", Use::Role(Role::Sweeper)),
    ("wrath", Use::Role(Role::Sweeper)),
    ("counterspell", Use::Role(Role::Counter)),
    ("bounce", Use::Role(Role::Bounce)),
    ("card-advantage", Use::Role(Role::Draw)),
    ("draw", Use::Role(Role::Draw)),
    ("card-draw", Use::Role(Role::Draw)),
    ("ramp", Use::Role(Role::Ramp)),
    ("mana-rock", Use::Role(Role::Ramp)),
    ("mana-dork", Use::Role(Role::Ramp)),
    ("token-generator", Use::Role(Role::Tokens)),
    ("tokens", Use::Role(Role::Tokens)),
    ("lifegain", Use::Role(Role::Lifegain)),
    ("life-gain", Use::Role(Role::Lifegain)),
    ("evasion", Use::Role(Role::Evasion)),
    ("gives-evasion", Use::Role(Role::Evasion)),
    ("protection", Use::Role(Role::Protection)),
    ("gives-protection", Use::Role(Role::Protection)),
    ("gives-hexproof", Use::Role(Role::Protection)),
    ("gives-indestructible", Use::Role(Role::Protection)),
    ("combat-trick", Use::Role(Role::Trick)),
    ("sacrifice-outlet", Use::Feeds("sacrifice outlets / fodder")),
    ("self-mill", Use::Feeds("graveyard fillers")),
    ("mill", Use::Feeds("graveyard fillers")),
    ("discard-outlet", Use::Feeds("discard outlets")),
    ("counters-matter", Use::Feeds("+1/+1 counter cards")),
    ("plus-one-counters", Use::Feeds("+1/+1 counter cards")),
    ("land-ramp", Use::Feeds("land fetchers")),
    ("extra-land", Use::Feeds("land fetchers")),
    ("burn", Use::Feeds("damage spells")),
    ("blink", Use::Feeds("enters-effect creatures")),
    ("flicker", Use::Feeds("enters-effect creatures")),
    ("etb", Use::Feeds("enters-effect creatures")),
    ("gives-haste", Use::Feeds("evasive/haste creatures")),
];

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TagsFile {
    pub fetched: String,
    /// Card name -> tags (only the tags in `TAGS`).
    pub cards: BTreeMap<String, Vec<String>>,
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("scryfall_tags.json")
}

pub fn load_file(data_dir: &Path) -> TagsFile {
    std::fs::read_to_string(path(data_dir))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn use_of(tag: &str) -> Option<Use> {
    TAGS.iter().find(|(t, _)| *t == tag).map(|(_, u)| *u)
}

/// Roles the tags give a card.
pub fn roles(tags: &[String]) -> Vec<Role> {
    let mut out = Vec::new();
    for t in tags {
        if let Some(Use::Role(r)) = use_of(t) {
            if !out.contains(&r) {
                out.push(r);
            }
        }
    }
    out
}

/// Whether the tags say the card feeds the mechanic with this `need`.
pub fn feeds(tags: &[String], need: &str) -> bool {
    tags.iter()
        .any(|t| matches!(use_of(t), Some(Use::Feeds(n)) if n == need))
        || match need {
            // Role tags that are also enablers.
            "token makers" => roles(tags).contains(&Role::Tokens),
            "life gain cards" => roles(tags).contains(&Role::Lifegain),
            "card draw" => roles(tags).contains(&Role::Draw),
            "mana sources" => roles(tags).contains(&Role::Ramp),
            "evasive/haste creatures" => roles(tags).contains(&Role::Evasion),
            _ => false,
        }
}

/// Tag names found on Scryfall's published tag list page.
pub fn parse_tag_list(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    for marker in ["otag:", "otag%3A", "oracletag:", "oracletag%3A"] {
        for (i, _) in html.match_indices(marker) {
            let tag: String = html[i + marker.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect::<String>()
                .to_lowercase();
            if !tag.is_empty() && !out.contains(&tag) {
                out.push(tag);
            }
        }
    }
    out
}

/// Pulls card front names out of a Scryfall search page.
pub fn parse_search(v: &serde_json::Value) -> Vec<String> {
    v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["name"].as_str())
        .map(|n| n.split(" // ").next().unwrap_or(n).to_string())
        .collect()
}

fn get(url: &str) -> Result<String, ureq::Error> {
    ureq::get(url)
        .header("User-Agent", "MSHDeckbuilder/0.5 (personal deck builder)")
        .header("Accept", "application/json, text/html")
        .call()?
        .body_mut()
        .with_config()
        .limit(16 * 1024 * 1024)
        .read_to_string()
}

fn scryfall_api() -> String {
    std::env::var("MSH_SCRYFALL_API").unwrap_or_else(|_| "https://api.scryfall.com".into())
}

fn scryfall_site() -> String {
    std::env::var("MSH_SCRYFALL_SITE").unwrap_or_else(|_| "https://scryfall.com".into())
}

/// Downloads the tags in `TAGS` for every card in the league sets. Returns (cards tagged,
/// tags downloaded).
pub fn fetch(data_dir: &Path, db: &CardDb) -> Result<(usize, usize), String> {
    // Only ask for tags Scryfall lists (one page), so misspelt guesses cost nothing.
    let listed = get(&format!("{}/docs/tagger-tags", scryfall_site()))
        .map(|h| parse_tag_list(&h))
        .unwrap_or_default();
    let wanted: Vec<&str> = TAGS
        .iter()
        .map(|(t, _)| *t)
        .filter(|t| listed.is_empty() || listed.iter().any(|l| l == t))
        .collect();
    let sets = db.league_sets();
    let sets_q = sets
        .iter()
        .map(|s| format!("e%3A{}", s.to_lowercase()))
        .collect::<Vec<_>>()
        .join("+or+");
    let mut cards: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut found_tags = 0;
    let mut failures = 0;
    for tag in wanted {
        if failures >= 3 && found_tags == 0 {
            break; // Scryfall unreachable
        }
        let mut url = Some(format!(
            "{}/cards/search?q=otag%3A{}+%28{}%29&unique=cards",
            scryfall_api(),
            tag,
            sets_q
        ));
        let mut any = false;
        while let Some(u) = url.take() {
            std::thread::sleep(std::time::Duration::from_millis(150));
            match get(&u) {
                Ok(body) => {
                    let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
                        break;
                    };
                    for name in parse_search(&v) {
                        let list = cards.entry(name).or_default();
                        if !list.iter().any(|t| t == tag) {
                            list.push(tag.to_string());
                        }
                        any = true;
                    }
                    if v["has_more"].as_bool() == Some(true) {
                        url = v["next_page"].as_str().map(String::from);
                    }
                }
                // Scryfall answers 404 when no card in these sets has the tag.
                Err(ureq::Error::StatusCode(404)) => any = true,
                Err(_) => failures += 1,
            }
        }
        found_tags += usize::from(any);
    }
    if found_tags == 0 {
        return Err("Scryfall unavailable (offline?)".into());
    }
    let file = TagsFile {
        fetched: crate::ratings::today(),
        cards,
    };
    let n = file.cards.len();
    let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    std::fs::write(path(data_dir), json).map_err(|e| e.to_string())?;
    Ok((n, found_tags))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tag_list_and_meanings() {
        let html = r#"<a href="/search?q=otag%3Aremoval">removal</a> <code>otag:sacrifice-outlet</code>
            <a href="/search?q=otag%3Aramp">ramp</a>"#;
        assert_eq!(
            parse_tag_list(html),
            vec!["sacrifice-outlet", "removal", "ramp"]
        );
        let tags = vec!["removal".to_string(), "sacrifice-outlet".to_string()];
        assert_eq!(roles(&tags), vec![Role::Removal]);
        assert!(feeds(&tags, "sacrifice outlets / fodder"));
        assert!(!feeds(&tags, "Equipment"));
        let v = serde_json::json!({"data": [{"name": "Bilbo Baggins, Burglar // Take a Glance"}]});
        assert_eq!(parse_search(&v), vec!["Bilbo Baggins, Burglar"]);
    }
}
