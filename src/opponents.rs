//! The playtest opponents: the season simulation (`season`) plus what the user knows for
//! real - commanders and decklists pinned in `data/opponents.json`, and the Public Pool
//! imported from Mana Mixer - cached in `data/gauntlet_cache.json` so playtests are
//! consistent and fast. Nothing here goes online.

use crate::db::{CardDb, Collection, Lookup, Rules};
use crate::deck::Deck;
use crate::season::{self, Pin, Pins, Season};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

pub const PINS_FILE: &str = "opponents.json";
pub const CACHE_FILE: &str = "gauntlet_cache.json";
/// Bump when the simulation changes, so old cached seasons are rebuilt. The key also holds
/// `SIM_FINGERPRINT` (build.rs: a hash of the source files the season depends on), so a
/// code change rebuilds the cache even when nobody remembers to bump this.
const SIM_VERSION: u32 = 2;
const SIM_FINGERPRINT: &str = env!("SIM_FINGERPRINT");
/// Seasons kept in the cache file (different seeds, sessions, rule previews).
const CACHE_ENTRIES: usize = 24;

pub fn load_pins(dir: &Path) -> Pins {
    std::fs::read_to_string(dir.join(PINS_FILE))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_pins(dir: &Path, pins: &Pins) -> Result<(), String> {
    let json = serde_json::to_string_pretty(pins).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(PINS_FILE), json + "\n").map_err(|e| e.to_string())
}

/// The imported Public Pool, if there is one.
fn real_pool(dir: &Path) -> Option<Collection> {
    Some(crate::upgrades::load_public_pool(dir)).filter(|c| c.total() > 0)
}

/// FNV-1a: a hash that stays the same across program versions and runs.
fn fnv(text: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

/// Everything a simulated season depends on.
pub fn cache_key(
    db: &CardDb,
    rules: &Rules,
    seed: u64,
    pins: &Pins,
    pool: Option<&Collection>,
) -> String {
    fnv(&format!(
        "v{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        SIM_VERSION,
        SIM_FINGERPRINT,
        seed,
        serde_json::to_string(rules).unwrap_or_default(),
        serde_json::to_string(pins).unwrap_or_default(),
        pool.map(|p| serde_json::to_string(&p.counts).unwrap_or_default())
            .unwrap_or_default(),
        db.cards.len(),
        db.tags_date,
        db.commander_stats.len(),
        db.edhrec.fetched,
        db.edhrec.commander_count(),
    ))
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct CacheFile {
    entries: Vec<(String, Season)>,
}

/// Seasons already worked out by this program run.
static MEMO: Mutex<Vec<(String, Arc<Season>)>> = Mutex::new(Vec::new());

fn remember(key: &str, s: &Arc<Season>) {
    let mut memo = MEMO.lock().unwrap();
    memo.retain(|(k, _)| k != key);
    memo.push((key.to_string(), s.clone()));
    if memo.len() > CACHE_ENTRIES {
        memo.remove(0);
    }
}

fn read_cache(dir: &Path) -> CacheFile {
    std::fs::read_to_string(dir.join(CACHE_FILE))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn write_cache(dir: &Path, key: &str, s: &Season) {
    let mut file = read_cache(dir);
    file.entries.retain(|(k, _)| k != key);
    file.entries.push((key.to_string(), s.clone()));
    let excess = file.entries.len().saturating_sub(CACHE_ENTRIES);
    file.entries.drain(..excess);
    if let Ok(json) = serde_json::to_string(&file) {
        // Write then rename, so a reader never sees half a file.
        let tmp = dir.join(format!("{}.tmp{}", CACHE_FILE, std::process::id()));
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, dir.join(CACHE_FILE));
        }
    }
}

/// The simulated season for these rules and seed, with the user's pinned info and Public
/// Pool when the rules come from a data folder. Cached in memory and in the data folder.
pub fn season(db: &CardDb, rules: &Rules, seed: u64) -> Arc<Season> {
    let dir = rules.data_dir.as_deref();
    let pins = dir.map(load_pins).unwrap_or_default();
    let pool = dir.and_then(real_pool);
    let key = cache_key(db, rules, seed, &pins, pool.as_ref());
    if let Some((_, s)) = MEMO.lock().unwrap().iter().find(|(k, _)| *k == key) {
        return s.clone();
    }
    if let Some(dir) = dir {
        if let Some((_, s)) = read_cache(dir).entries.into_iter().find(|(k, _)| *k == key) {
            let s = Arc::new(s);
            remember(&key, &s);
            return s;
        }
    }
    let s = Arc::new(season::simulate(db, rules, seed, pool.as_ref(), &pins));
    remember(&key, &s);
    if let Some(dir) = dir {
        write_cache(dir, &key, &s);
    }
    s
}

/// One line saying which league a tool is measuring: session, seed, imported pool, pins and
/// the season's cache key.
pub fn league_header(db: &CardDb, rules: &Rules, seed: u64) -> String {
    let dir = rules.data_dir.as_deref();
    let pins = dir.map(load_pins).unwrap_or_default();
    let pool = dir.and_then(real_pool);
    format!(
        "league: Session {} (deck size {}{}), seed {}, data {}, imported pool {}, {} pinned players, season cache key {}",
        rules.session.clamp(1, 13),
        rules.deck_size,
        if rules.singleton { ", singleton" } else { "" },
        seed,
        dir.map(|d| d.display().to_string())
            .unwrap_or_else(|| "none".into()),
        pool.as_ref()
            .map(|p| format!("{} cards", p.total()))
            .unwrap_or_else(|| "none".into()),
        pins.players.len(),
        cache_key(db, rules, seed, &pins, pool.as_ref())
    )
}

/// The opponents' decks (one per other player with a commander), for playtests.
pub fn decks(db: &CardDb, rules: &Rules, seed: u64) -> Vec<Deck> {
    season(db, rules, seed).decks()
}

/// Forgets every cached season, so the next playtest simulates again.
pub fn refresh(dir: &Path) {
    MEMO.lock().unwrap().clear();
    let _ = std::fs::remove_file(dir.join(CACHE_FILE));
}

/// Changes when the pinned info or the imported Public Pool changes (modification times),
/// for keys of results that depend on the opponents.
pub fn inputs_stamp(rules: &Rules) -> String {
    let Some(dir) = rules.data_dir.as_deref() else {
        return String::new();
    };
    [PINS_FILE, "public_pool.txt"]
        .iter()
        .map(|f| {
            std::fs::metadata(dir.join(f))
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis().to_string())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Finds a roster player by number (1-7, as `roster` lists them) or name.
pub fn find_player(rules: &Rules, query: &str) -> Result<String, String> {
    let q = query.trim().to_lowercase();
    let others = &rules.roster.others;
    if let Ok(i) = q.parse::<usize>() {
        if (1..=others.len()).contains(&i) {
            return Ok(others[i - 1].name.clone());
        }
    }
    if let Some(p) = others.iter().find(|p| p.name.to_lowercase() == q) {
        return Ok(p.name.clone());
    }
    let hits: Vec<&String> = others
        .iter()
        .map(|p| &p.name)
        .filter(|n| n.to_lowercase().starts_with(&q))
        .collect();
    match hits.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(format!(
            "No player '{}'. The others are: {}",
            query,
            others
                .iter()
                .enumerate()
                .map(|(i, p)| format!("{} {}", i + 1, p.name))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        _ => Err(format!(
            "'{}' could be {}.",
            query,
            hits.iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(" or ")
        )),
    }
}

/// Sets or clears what's known about one opponent. `what` is "commander <card>",
/// "deck <decklist text>" or "clear". Returns a message for the user.
pub fn pin(db: &CardDb, rules: &Rules, player: &str, what: &str) -> Result<String, String> {
    let dir = rules
        .data_dir
        .as_deref()
        .ok_or("No data folder to save opponents.json in.")?;
    let name = find_player(rules, player)?;
    let mut pins = load_pins(dir);
    let (kind, value) = what.split_once(char::is_whitespace).unwrap_or((what, ""));
    let value = value.trim();
    let msg = match kind.to_lowercase().as_str() {
        "clear" | "reset" => {
            pins.players.retain(|k, _| !k.eq_ignore_ascii_case(&name));
            format!("{} is simulated again.", name)
        }
        "commander" | "cmd" => {
            let card = match db.lookup(value) {
                Lookup::Found(c) => c,
                Lookup::Ambiguous(v) => {
                    return Err(format!(
                        "'{}' could be: {}",
                        value,
                        v.iter()
                            .take(6)
                            .map(|c| c.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                Lookup::NotFound(_) => return Err(format!("No card called '{}'.", value)),
            };
            if !card.can_be_commander() {
                return Err(format!("{} can't be a commander.", card.name));
            }
            pin_entry(&mut pins, &name).commander = Some(card.name.clone());
            format!("{} now plays {} in the simulation.", name, card.name)
        }
        "deck" | "decklist" => {
            let d = Deck::from_text(&name, value, db);
            let known_cmd =
                d.commander.is_some() || pins.get(&name).is_some_and(|p| p.commander.is_some());
            if d.main_count() == 0 || !known_cmd {
                return Err("That decklist needs cards and a commander (a \"Commander\" section, or pin the commander first).".into());
            }
            let unknown: Vec<&String> = d.cards.keys().filter(|n| db.get(n).is_none()).collect();
            let n = d.main_count();
            pin_entry(&mut pins, &name).deck = Some(value.to_string());
            let mut m = format!("{}'s real decklist saved ({} cards).", name, n);
            if !unknown.is_empty() {
                m += &format!(
                    " Not in the card database (left out of playtests): {}",
                    unknown
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            m
        }
        _ => return Err("Use: commander <card>, deck <file>, or clear.".into()),
    };
    pins.players.retain(|_, p| !p.is_empty());
    save_pins(dir, &pins)?;
    Ok(msg)
}

fn pin_entry<'a>(pins: &'a mut Pins, name: &str) -> &'a mut Pin {
    let key = pins
        .players
        .keys()
        .find(|k| k.eq_ignore_ascii_case(name))
        .cloned()
        .unwrap_or_else(|| name.to_string());
    pins.players.entry(key).or_default()
}

/// A text report: each opponent's deck and (with `full`) their session-by-session log.
pub fn report(db: &CardDb, s: &Season, full: bool) -> String {
    let mut out = format!(
        "Simulated opponents after Session {}'s trading round (seed {}){}.\n",
        s.session,
        s.seed,
        if s.real_pool {
            ", claiming from your imported Public Pool this session"
        } else {
            ""
        }
    );
    if s.held_back > 0 {
        out += &format!(
            "{} cards still in your imported pool were kept out of the simulated earlier sessions (nobody claimed them).\n",
            s.held_back
        );
    }
    for (i, o) in s.opponents.iter().enumerate() {
        let pinned = match o.pinned.as_str() {
            "commander" => "  [commander pinned]",
            "decklist" => "  [real decklist]",
            _ => "",
        };
        out += &format!(
            "\n{} {} - {}{}  ({} VP banked, {} Skill Tree staples)\n",
            i + 1,
            o.name,
            o.commander.as_deref().unwrap_or("no commander"),
            pinned,
            o.vp,
            o.staples
        );
        if let Some(d) = o.deck() {
            let mut spells: Vec<(f32, &str)> = analysis_spells(&d, db);
            spells.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
            out += &format!(
                "  best cards: {}\n",
                spells
                    .iter()
                    .take(8)
                    .map(|(_, n)| *n)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        if full {
            for l in &o.log {
                for line in l.lines() {
                    out += &format!("  {}\n", line);
                }
            }
        }
    }
    out
}

/// Non-land cards of a deck with their raw strength (for the "best cards" line).
fn analysis_spells<'a>(d: &'a Deck, db: &CardDb) -> Vec<(f32, &'a str)> {
    d.cards
        .keys()
        .filter_map(|n| db.get(n).map(|c| (c, n)))
        .filter(|(c, _)| !c.is_land())
        .map(|(c, n)| {
            (
                crate::analysis::card_power(c) + 0.1 * c.mana_value() as f32,
                n.as_str(),
            )
        })
        .collect()
}

/// The `opponents` command: `opponents` lists the simulated decks with their logs,
/// `opponents <player>` shows one, `opponents refresh` simulates again.
pub fn command(db: &CardDb, rules: &Rules, arg: &str) -> String {
    let arg = arg.trim();
    if arg.eq_ignore_ascii_case("refresh") {
        if let Some(dir) = rules.data_dir.as_deref() {
            refresh(dir);
        }
        let s = season(db, rules, crate::ranking::GAUNTLET_SEEDS[0]);
        return format!("Simulated the season again.\n{}", report(db, &s, false));
    }
    let s = season(db, rules, crate::ranking::GAUNTLET_SEEDS[0]);
    if arg.is_empty() {
        return report(db, &s, true);
    }
    match find_player(rules, arg) {
        Ok(name) => {
            let one = Season {
                opponents: s
                    .opponents
                    .iter()
                    .filter(|o| o.name == name)
                    .cloned()
                    .collect(),
                ..(*s).clone()
            };
            let mut out = report(db, &one, true);
            if let Some(d) = one.opponents.first().and_then(|o| o.deck()) {
                out += "\n";
                out += &d.to_text();
            }
            out
        }
        Err(e) => e,
    }
}

/// The `opponent <player> commander <card> | deck <file> | clear` command.
pub fn pin_command(db: &CardDb, rules: &Rules, arg: &str) -> String {
    let lower = arg.to_lowercase();
    let at = [" commander ", " cmd ", " deck ", " decklist "]
        .iter()
        .filter_map(|k| lower.find(k))
        .min()
        .or_else(|| {
            [" clear", " reset"]
                .iter()
                .filter_map(|k| lower.strip_suffix(k).map(|p| p.len()))
                .next()
        });
    let Some(at) = at else {
        return "Usage: opponent <player> commander <card> | deck <file> | clear   (player = number from `roster` or name)".into();
    };
    let (player, what) = (arg[..at].trim(), arg[at..].trim());
    // A decklist is read from a file.
    let what = match what.split_once(char::is_whitespace) {
        Some((k, path)) if k.eq_ignore_ascii_case("deck") || k.eq_ignore_ascii_case("decklist") => {
            let path = path.trim().trim_matches('"');
            match std::fs::read(path) {
                Ok(b) => format!("deck {}", crate::league::decode_text(&b)),
                Err(e) => return format!("Couldn't read {}: {}", path, e),
            }
        }
        _ => what.to_string(),
    };
    match pin(db, rules, player, &what) {
        Ok(m) => m + " Playtests use it from now on (`opponents` shows the decks).",
        Err(e) => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_and_cache_round_trip() {
        let db = CardDb::load(
            &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data/cards.json"),
        )
        .unwrap();
        let dir = std::env::temp_dir().join(format!("msh-opponents-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut rules = Rules::default();
        crate::league::apply_session(&mut rules, 1).unwrap();
        rules.data_dir = Some(dir.clone());
        assert_eq!(find_player(&rules, "2").unwrap(), "Mixed player");
        assert_eq!(
            find_player(&rules, "hobbit player 4").unwrap(),
            "Hobbit player 4"
        );
        assert!(find_player(&rules, "hobbit").is_err());
        assert!(find_player(&rules, "nobody").is_err());
        let before = cache_key(&db, &rules, 1, &load_pins(&dir), None);
        let msg = pin_command(
            &db,
            &rules,
            "mixed player commander Gandalf, Wandering Wizard",
        );
        assert!(msg.contains("now plays"), "{}", msg);
        assert!(pin_command(&db, &rules, "3 commander Sol Ring").contains("can't be a commander"));
        let pins = load_pins(&dir);
        assert_eq!(
            pins.get("Mixed player").unwrap().commander.as_deref(),
            Some("Gandalf, Wandering Wizard")
        );
        assert_ne!(before, cache_key(&db, &rules, 1, &pins, None));
        // Simulated once, then read back from the cache file.
        let s = season(&db, &rules, 1);
        assert_eq!(
            s.opponents[1].commander.as_deref(),
            Some("Gandalf, Wandering Wizard")
        );
        assert!(dir.join(CACHE_FILE).exists());
        MEMO.lock().unwrap().clear();
        let again = season(&db, &rules, 1);
        assert_eq!(*s, *again);
        assert!(command(&db, &rules, "mixed").contains("Gandalf, Wandering Wizard"));
        assert!(pin_command(&db, &rules, "mixed clear").contains("simulated again"));
        assert!(load_pins(&dir).players.is_empty());
        refresh(&dir);
        assert!(!dir.join(CACHE_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
