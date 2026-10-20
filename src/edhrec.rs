//! EDHREC (edhrec.com) data: what real Commander players put in their decks.
//!
//! * **Card rank**: EDHREC's overall popularity rank of each card across all Commander decks
//!   (1 = the most played card), read from Scryfall, which publishes EDHREC's rank with
//!   every card. Cards are compared with their own set, since a brand-new set hasn't had
//!   time to climb the overall list.
//! * **Commander pages**: for each commander you own, the cards EDHREC players run with it -
//!   how many of that commander's decks include each card ("inclusion") and how much more
//!   than in other decks of those colours ("synergy").
//!
//! Downloaded on your PC only when you ask (Update ratings), saved in `data/edhrec.json`,
//! and reused from there: one Scryfall request per 75 cards, one EDHREC request per
//! commander, with a pause between requests.

use crate::db::{CardDb, normalize};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// One card on a commander's EDHREC page.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
pub struct Pick {
    /// Share of the commander's decks that run the card (0-1).
    pub inclusion: f32,
    /// Inclusion minus the card's usual inclusion in decks of those colours (-1..1).
    pub synergy: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CommanderPage {
    /// Decks EDHREC has seen with this commander.
    pub decks: u32,
    /// EDHREC's popularity rank among commanders, when the page gives it.
    #[serde(default)]
    pub rank: Option<u32>,
    pub fetched: String,
    /// Card name -> pick, for every card the page lists.
    pub picks: BTreeMap<String, Pick>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EdhrecFile {
    /// When the card ranks were downloaded ("" = never).
    pub fetched: String,
    /// Card name -> EDHREC rank.
    pub ranks: BTreeMap<String, u32>,
    pub commanders: BTreeMap<String, CommanderPage>,
    /// Commanders EDHREC has no page for yet (not asked again until a forced update).
    #[serde(default)]
    pub missing: Vec<String>,
}

/// The saved data, indexed for quick lookups by the card scorer.
#[derive(Debug, Clone, Default)]
pub struct Edhrec {
    pub fetched: String,
    /// Normalised card name -> (rank, where it sits in its set: 1 = most played, 0 = least,
    /// how far to trust that: new sets haven't been in many decks yet).
    ranks: HashMap<String, (u32, f32, f32)>,
    /// Normalised commander name -> page with normalised card names.
    pages: HashMap<String, CommanderPage>,
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("edhrec.json")
}

pub fn load_file(data_dir: &Path) -> EdhrecFile {
    std::fs::read_to_string(path(data_dir))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

impl Edhrec {
    /// Indexes the saved file against the card database (percentiles are within each set).
    pub fn new(file: EdhrecFile, db: &CardDb) -> Edhrec {
        let mut by_set: HashMap<&str, Vec<(String, u32)>> = HashMap::new();
        for (name, rank) in &file.ranks {
            let key = normalize(name);
            let set = db.get(name).map(|c| c.set.as_str()).unwrap_or("");
            by_set.entry(set).or_default().push((key, *rank));
        }
        let mut ranks = HashMap::new();
        for (set, mut list) in by_set {
            let trust = set_trust(set);
            list.sort_by_key(|(_, r)| *r);
            let n = list.len();
            for (i, (key, rank)) in list.into_iter().enumerate() {
                let pct = if n > 1 {
                    1.0 - i as f32 / (n - 1) as f32
                } else {
                    0.5
                };
                ranks.insert(key, (rank, pct, trust));
            }
        }
        let pages = file
            .commanders
            .into_iter()
            .map(|(name, mut page)| {
                page.picks = page
                    .picks
                    .into_iter()
                    .map(|(k, v)| (normalize(&k), v))
                    .collect();
                (normalize(&name), page)
            })
            .collect();
        Edhrec {
            fetched: file.fetched,
            ranks,
            pages,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.ranks.is_empty() && self.pages.is_empty()
    }

    pub fn ranked_cards(&self) -> usize {
        self.ranks.len()
    }

    pub fn commander_count(&self) -> usize {
        self.pages.len()
    }

    /// EDHREC rank and where it sits in its set (1 = the set's most played card).
    pub fn rank(&self, card: &str) -> Option<(u32, f32)> {
        self.ranks.get(&normalize(card)).map(|&(r, p, _)| (r, p))
    }

    pub fn page(&self, commander: &str) -> Option<&CommanderPage> {
        self.pages.get(&normalize(commander))
    }

    /// How a card shows up on a commander's page (`None` if the page doesn't list it).
    pub fn pick(&self, commander: &str, card: &str) -> Option<Pick> {
        self.page(commander)?.picks.get(&normalize(card)).copied()
    }
}

/// How far to trust EDHREC's numbers for a set: a set out for a few weeks has only been in
/// a few decks, so its ranks and inclusion rates still move a lot (half trust at 90 days).
fn set_trust(set: &str) -> f32 {
    match crate::ratings::set_release(set) {
        Some(date) => {
            let days = crate::ratings::age_days(date).clamp(0, 3650) as f32;
            days / (days + 90.0)
        }
        // Staples and older printings: years of data.
        None => 0.9,
    }
}

/// Decks behind a commander page needed for half trust in its inclusion rates.
const PAGE_TRUST_DECKS: f32 = 200.0;

/// How far to trust a commander page's numbers (0..1) from how many decks it's built on.
pub fn page_trust(page: &CommanderPage) -> f32 {
    page.decks as f32 / (page.decks as f32 + PAGE_TRUST_DECKS)
}

/// Points for how much Commander players play a card overall, compared with its set
/// (at most -0.4 .. +0.4, less for new sets), with an explanation.
pub fn rank_points(e: &Edhrec, card: &str) -> Option<(f32, String)> {
    let &(rank, pct, trust) = e.ranks.get(&normalize(card))?;
    let pts = (pct - 0.5) * 0.8 * trust;
    Some((
        pts,
        format!(
            "EDHREC rank #{} (more played than {:.0}% of its set)",
            rank,
            pct * 100.0
        ),
    ))
}

/// Points for a card that EDHREC players run with this commander (0 .. 1.5).
pub fn pick_points(p: Pick) -> f32 {
    (p.synergy.clamp(-0.3, 0.8) * 2.0 + p.inclusion * 1.5).clamp(0.0, 1.5)
}

/// The card's best EDHREC pick bonus across `commanders`, scaled by how many decks each
/// page is built on, with an explanation.
pub fn pick_bonus(e: &Edhrec, commanders: &[&String], card: &str) -> Option<(f32, String)> {
    commanders
        .iter()
        .filter_map(|c| {
            let page = e.page(c)?;
            let p = *page.picks.get(&normalize(card))?;
            let pts = pick_points(p) * page_trust(page);
            (pts > 0.0).then(|| (pts, describe_pick(c, p)))
        })
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
}

pub fn describe_pick(commander: &str, p: Pick) -> String {
    format!(
        "EDHREC: in {:.0}% of {} decks (synergy {:+.0}%)",
        p.inclusion * 100.0,
        short_name(commander),
        p.synergy * 100.0
    )
}

/// Lines for the card inspector: overall rank, how it's played with `commanders`, and its
/// own page when it is a commander.
pub fn card_summary(e: &Edhrec, card: &str, commanders: &[&String]) -> Vec<String> {
    let mut out = Vec::new();
    if let Some((_, note)) = rank_points(e, card) {
        out.push(note);
    }
    for c in commanders {
        if normalize(c) == normalize(card) {
            continue;
        }
        match e.pick(c, card) {
            Some(p) => out.push(describe_pick(c, p)),
            None if e.page(c).is_some() => out.push(format!(
                "EDHREC: not among {}'s listed cards",
                short_name(c)
            )),
            None => {}
        }
    }
    if let Some(p) = e.page(card) {
        out.push(format!(
            "EDHREC: {} decks with it as commander{}",
            p.decks,
            p.rank
                .map(|r| format!(" (commander rank #{})", r))
                .unwrap_or_default()
        ));
    }
    out
}

fn short_name(name: &str) -> &str {
    name.split(',').next().unwrap_or(name)
}

/// EDHREC's address form of a name: "Glóin, the Mighty" -> "gloin-the-mighty".
pub fn slug(name: &str) -> String {
    let front = name.split(" // ").next().unwrap_or(name);
    let mut out = String::new();
    for c in front.to_lowercase().chars() {
        let c = match c {
            'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            other => other,
        };
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if (c == ' ' || c == '-') && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

// ---------------------------------------------------------------- reading the pages

fn as_u32(v: &serde_json::Value) -> Option<u32> {
    v.as_u64()
        .map(|n| n as u32)
        .or_else(|| v.as_f64().map(|f| f as u32))
}

/// Reads one card entry ("cardview") from a commander page.
fn read_cardview(v: &serde_json::Value) -> Option<(String, Pick)> {
    let name = v["name"].as_str()?;
    let decks = as_u32(&v["num_decks"]).or_else(|| as_u32(&v["inclusion"]));
    let potential = as_u32(&v["potential_decks"]);
    let inclusion = match (decks, potential) {
        (Some(d), Some(p)) if p > 0 => d as f32 / p as f32,
        _ => {
            let raw = v["inclusion"].as_f64()? as f32;
            if raw <= 1.0 { raw } else { raw / 100.0 }
        }
    };
    let synergy = v["synergy"].as_f64().unwrap_or(0.0) as f32;
    // Synergy is a fraction on EDHREC's pages; accept percentages too.
    let synergy = if synergy.abs() > 1.0 {
        synergy / 100.0
    } else {
        synergy
    };
    Some((
        name.split(" // ").next().unwrap_or(name).to_string(),
        Pick {
            inclusion: inclusion.clamp(0.0, 1.0),
            synergy: synergy.clamp(-1.0, 1.0),
        },
    ))
}

/// Collects every card list ("cardlists" -> "cardviews") anywhere in the page.
fn collect_cardviews(v: &serde_json::Value, out: &mut BTreeMap<String, Pick>, max_pot: &mut u32) {
    match v {
        serde_json::Value::Object(map) => {
            if let Some(views) = map.get("cardviews").and_then(|c| c.as_array()) {
                for cv in views {
                    if let Some((name, pick)) = read_cardview(cv) {
                        *max_pot = (*max_pot).max(as_u32(&cv["potential_decks"]).unwrap_or(0));
                        // A card can be in several lists (e.g. "High Synergy" and "Creatures").
                        out.entry(name).or_insert(pick);
                    }
                }
            }
            for x in map.values() {
                collect_cardviews(x, out, max_pot);
            }
        }
        serde_json::Value::Array(a) => {
            for x in a {
                collect_cardviews(x, out, max_pot);
            }
        }
        _ => {}
    }
}

/// Reads an EDHREC commander page (json.edhrec.com/pages/commanders/{slug}.json).
pub fn parse_commander_page(json: &str, commander: &str) -> Result<CommanderPage, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let mut picks = BTreeMap::new();
    let mut max_pot = 0;
    collect_cardviews(&v, &mut picks, &mut max_pot);
    // The commander isn't a pick for itself.
    let me = normalize(commander);
    picks.retain(|k, _| normalize(k) != me);
    if picks.is_empty() {
        return Err("no card lists on the page".into());
    }
    let card = &v["container"]["json_dict"]["card"];
    // Deck count: from the commander's own entry, else the largest "could include" count.
    let decks = as_u32(&card["num_decks"])
        .or_else(|| as_u32(&v["num_decks_avg"]))
        .filter(|&d| d > 0)
        .unwrap_or(max_pot);
    Ok(CommanderPage {
        decks,
        rank: as_u32(&card["rank"]),
        fetched: crate::ratings::today(),
        picks,
    })
}

// ---------------------------------------------------------------- downloading

fn edhrec_base() -> String {
    std::env::var("MSH_EDHREC_BASE").unwrap_or_else(|_| "https://json.edhrec.com".into())
}

fn scryfall_api() -> String {
    std::env::var("MSH_SCRYFALL_API").unwrap_or_else(|_| "https://api.scryfall.com".into())
}

fn agent() -> &'static str {
    "MSHDeckbuilder/0.5 (personal deck builder)"
}

fn read_body(
    resp: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<String, String> {
    resp.map_err(|e| e.to_string())?
        .body_mut()
        .with_config()
        .limit(32 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| e.to_string())
}

/// EDHREC ranks for every card in the database, via Scryfall's card collection lookup
/// (75 names per request). Returns name -> rank.
pub fn fetch_ranks(db: &CardDb) -> Result<BTreeMap<String, u32>, String> {
    let names: Vec<&str> = db
        .cards
        .iter()
        .filter(|c| !c.is_basic())
        .map(|c| c.name.as_str())
        .collect();
    let mut ranks = BTreeMap::new();
    let mut errors = Vec::new();
    for chunk in names.chunks(75) {
        let ids: Vec<serde_json::Value> = chunk
            .iter()
            .map(|n| serde_json::json!({ "name": n }))
            .collect();
        let body = serde_json::json!({ "identifiers": ids }).to_string();
        let resp = ureq::post(&format!("{}/cards/collection", scryfall_api()))
            .header("User-Agent", agent())
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .send(body.as_str());
        match read_body(resp)
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).map_err(|e| e.to_string()))
        {
            Ok(v) => ranks.extend(parse_scryfall_ranks(&v)),
            Err(e) => {
                errors.push(e);
                // Unreachable: don't keep knocking.
                if ranks.is_empty() && errors.len() >= 2 {
                    break;
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    if ranks.is_empty() {
        return Err(format!(
            "Scryfall unavailable ({})",
            errors.first().cloned().unwrap_or_default()
        ));
    }
    Ok(ranks)
}

/// Pulls (front name, edhrec_rank) out of a Scryfall list response.
pub fn parse_scryfall_ranks(v: &serde_json::Value) -> BTreeMap<String, u32> {
    let mut out = BTreeMap::new();
    for card in v["data"].as_array().into_iter().flatten() {
        let (Some(name), Some(rank)) = (card["name"].as_str(), as_u32(&card["edhrec_rank"])) else {
            continue;
        };
        out.insert(name.split(" // ").next().unwrap_or(name).to_string(), rank);
    }
    out
}

/// What a download did.
#[derive(Debug, Default)]
pub struct Fetched {
    pub ranks: usize,
    pub pages: usize,
    pub no_page: usize,
    pub kept: usize,
}

/// Downloads card ranks and the commander pages for `commanders`. Pages already saved are
/// kept unless `refresh` is set (commanders EDHREC has no page for are only re-asked then too).
pub fn fetch(
    data_dir: &Path,
    db: &CardDb,
    commanders: &[String],
    refresh: bool,
) -> Result<Fetched, String> {
    let mut file = load_file(data_dir);
    let mut got = Fetched::default();
    let mut errors = Vec::new();
    match fetch_ranks(db) {
        Ok(r) => {
            got.ranks = r.len();
            file.ranks = r;
            file.fetched = crate::ratings::today();
        }
        Err(e) => errors.push(e),
    }
    if refresh {
        file.missing.clear();
    }
    let mut failures = 0;
    for name in commanders {
        if name == crate::deck::PIPER {
            continue;
        }
        let saved = file.commanders.contains_key(name);
        if !refresh && (saved || file.missing.contains(name)) {
            got.kept += usize::from(saved);
            continue;
        }
        // Several failures in a row with nothing found: the site is unreachable, stop asking.
        if failures >= 3 && got.pages == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1000));
        let url = format!("{}/pages/commanders/{}.json", edhrec_base(), slug(name));
        let resp = ureq::get(&url)
            .header("User-Agent", agent())
            .header("Accept", "application/json")
            .call();
        match resp {
            Err(ureq::Error::StatusCode(404)) => {
                file.missing.push(name.clone());
                got.no_page += 1;
            }
            other => match read_body(other).and_then(|t| parse_commander_page(&t, name)) {
                Ok(page) => {
                    file.commanders.insert(name.clone(), page);
                    got.pages += 1;
                    failures = 0;
                }
                Err(e) => {
                    failures += 1;
                    errors.push(format!("{}: {}", name, e));
                }
            },
        }
    }
    if got.ranks == 0 && got.pages == 0 && got.no_page == 0 {
        return Err(errors.join("; "));
    }
    let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    std::fs::write(path(data_dir), json).map_err(|e| e.to_string())?;
    Ok(got)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_match_edhrec_addresses() {
        assert_eq!(slug("Thanos, the Mad Titan"), "thanos-the-mad-titan");
        assert_eq!(slug("Glóin the Mighty"), "gloin-the-mighty");
        assert_eq!(
            slug("Bilbo Baggins, Burglar // Take a Glance"),
            "bilbo-baggins-burglar"
        );
        assert_eq!(slug("Spider-Man 2099"), "spider-man-2099");
        assert_eq!(
            slug("Ka'Zar, Lord of the Savage Land"),
            "kazar-lord-of-the-savage-land"
        );
    }

    #[test]
    fn reads_commander_page() {
        let json = r#"{
            "header": "Thanos, the Mad Titan (Commander)",
            "container": {"json_dict": {
                "card": {"name": "Thanos, the Mad Titan", "num_decks": 812, "rank": 140},
                "cardlists": [
                    {"header": "High Synergy Cards", "tag": "highsynergycards", "cardviews": [
                        {"name": "Infinity Gauntlet", "num_decks": 600, "potential_decks": 800, "synergy": 0.71},
                        {"name": "Thanos, the Mad Titan", "num_decks": 812, "potential_decks": 812, "synergy": 0}
                    ]},
                    {"header": "Creatures", "tag": "creatures", "cardviews": [
                        {"name": "Bilbo Baggins, Burglar // Take a Glance", "inclusion": 200, "potential_decks": 800, "synergy": -0.05},
                        {"name": "Infinity Gauntlet", "num_decks": 1, "potential_decks": 800, "synergy": 0.1}
                    ]}
                ]
            }}
        }"#;
        let p = parse_commander_page(json, "Thanos, the Mad Titan").unwrap();
        assert_eq!(p.decks, 812);
        assert_eq!(p.rank, Some(140));
        assert_eq!(p.picks.len(), 2);
        let g = p.picks["Infinity Gauntlet"];
        assert!((g.inclusion - 0.75).abs() < 1e-4 && (g.synergy - 0.71).abs() < 1e-4);
        assert!((p.picks["Bilbo Baggins, Burglar"].inclusion - 0.25).abs() < 1e-4);
        assert!(parse_commander_page("{}", "X").is_err());
    }

    #[test]
    fn ranks_compare_within_set() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
        let db = CardDb::load(&dir.join("cards.json")).unwrap();
        let msh: Vec<&str> = db
            .cards
            .iter()
            .filter(|c| c.set == "MSH")
            .take(3)
            .map(|c| c.name.as_str())
            .collect();
        let v = serde_json::json!({"data": [
            {"name": msh[0], "edhrec_rank": 900},
            {"name": msh[1], "edhrec_rank": 15000},
            {"name": msh[2], "edhrec_rank": 4000},
            {"name": "Unranked"}
        ]});
        let ranks = parse_scryfall_ranks(&v);
        assert_eq!(ranks.len(), 3);
        let e = Edhrec::new(
            EdhrecFile {
                ranks,
                ..Default::default()
            },
            &db,
        );
        assert_eq!(e.rank(msh[0]), Some((900, 1.0)));
        assert_eq!(e.rank(msh[1]), Some((15000, 0.0)));
        assert!(rank_points(&e, msh[0]).unwrap().0 > 0.0);
        assert!(rank_points(&e, msh[1]).unwrap().0 < 0.0);
    }
}
