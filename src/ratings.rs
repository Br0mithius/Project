//! Outside data that sharpens the rankings, downloaded on your PC and saved in `data/`:
//!
//! * **17Lands** card ratings for Marvel Super Heroes Premier Draft: how often decks win
//!   when a card was drawn ("GIH WR"), from many thousands of real MTG Arena games.
//! * **Playgroup.gg** commander win rates from logged casual Commander games (mostly 4-player
//!   pods; a 25% win rate is average). These are 100-card constructed decks, so they're a
//!   weaker signal for league pools and are weighted lightly.
//!
//! Both are optional: without them the app falls back to rarity and its own heuristics.

use crate::db::normalize;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Marvel Super Heroes release date - the start of 17Lands' data window.
const SET_RELEASE: &str = "2026-06-26";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Rating {
    /// Win rate of games where the card was drawn or in the opening hand (0-1).
    pub gih_wr: Option<f32>,
    /// Games behind `gih_wr`.
    pub games: u32,
    /// Average pick position it was last seen at (lower = drafters value it more).
    pub alsa: Option<f32>,
    /// Improvement in win rate when drawn ("IWD").
    pub iwd: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RatingsFile {
    pub fetched: String,
    pub source: String,
    pub cards: BTreeMap<String, Rating>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommanderStat {
    /// Share of games won (0-1); 0.25 is average in a 4-player pod.
    pub win_rate: f32,
    pub games: u32,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CommanderStatsFile {
    pub fetched: String,
    pub commanders: BTreeMap<String, CommanderStat>,
}

pub fn ratings_path(data_dir: &Path) -> PathBuf {
    data_dir.join("ratings_17lands.json")
}

pub fn commander_stats_path(data_dir: &Path) -> PathBuf {
    data_dir.join("commander_stats.json")
}

// ---------------------------------------------------------------- dates

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn today_days() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64 / 86400)
        .unwrap_or(0)
}

pub fn today() -> String {
    let (y, m, d) = civil_from_days(today_days());
    format!("{:04}-{:02}-{:02}", y, m, d)
}

/// Days since a "YYYY-MM-DD" date (large if it can't be read).
pub fn age_days(date: &str) -> i64 {
    let parts: Vec<i64> = date.split('-').filter_map(|p| p.parse().ok()).collect();
    if parts.len() != 3 {
        return 9999;
    }
    let target = (0..30000)
        .map(|k| today_days() - k)
        .find(|&d| civil_from_days(d) == (parts[0], parts[1] as u32, parts[2] as u32));
    target.map(|d| today_days() - d).unwrap_or(9999)
}

// ---------------------------------------------------------------- loading

pub fn load_ratings(data_dir: &Path) -> Option<RatingsFile> {
    let text = std::fs::read_to_string(ratings_path(data_dir)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Snapshot of Playgroup.gg numbers found while building this feature (Oct 2026), used
/// until the app can download fresh ones.
fn builtin_commander_stats() -> CommanderStatsFile {
    let mut c = BTreeMap::new();
    for (name, wr, games) in [
        ("Thanos, the Mad Titan", 0.41, 151),
        ("Madame Hydra", 0.25, 35),
        ("Red Hulk", 0.38, 13),
    ] {
        c.insert(
            name.to_string(),
            CommanderStat {
                win_rate: wr,
                games,
                source: "Playgroup.gg (Oct 2026 snapshot)".into(),
            },
        );
    }
    CommanderStatsFile {
        fetched: String::new(),
        commanders: c,
    }
}

pub fn load_commander_stats(data_dir: &Path) -> CommanderStatsFile {
    let mut file = std::fs::read_to_string(commander_stats_path(data_dir))
        .ok()
        .and_then(|t| serde_json::from_str::<CommanderStatsFile>(&t).ok())
        .unwrap_or_default();
    for (k, v) in builtin_commander_stats().commanders {
        file.commanders.entry(k).or_insert(v);
    }
    file
}

// ---------------------------------------------------------------- 17Lands

fn get_text(url: &str) -> Result<String, String> {
    ureq::get(url)
        .header("User-Agent", "MSHDeckbuilder/0.5 (personal deck builder)")
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .with_config()
        .limit(32 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| e.to_string())
}

fn num(v: &serde_json::Value) -> Option<f32> {
    v.as_f64().map(|x| x as f32)
}

/// Reads 17Lands' card_ratings JSON (a list of cards with `ever_drawn_win_rate` etc.).
pub fn parse_17lands(json: &str) -> Result<BTreeMap<String, Rating>, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let list = v
        .as_array()
        .ok_or("17Lands returned something other than a card list")?;
    let mut out = BTreeMap::new();
    for card in list {
        let Some(name) = card["name"].as_str() else {
            continue;
        };
        // Double-faced cards are listed as "Front // Back"; the app uses the front name.
        let name = name.split(" // ").next().unwrap_or(name).to_string();
        let r = Rating {
            gih_wr: num(&card["ever_drawn_win_rate"]),
            games: card["ever_drawn_game_count"].as_u64().unwrap_or(0) as u32,
            alsa: num(&card["avg_seen"]),
            iwd: num(&card["drawn_improvement_win_rate"]),
        };
        out.insert(name, r);
    }
    if out.is_empty() {
        return Err("17Lands returned no cards".into());
    }
    Ok(out)
}

/// Downloads the latest 17Lands ratings for the set and saves them. Returns the card count.
pub fn fetch_17lands(data_dir: &Path) -> Result<usize, String> {
    let url = std::env::var("MSH_17LANDS_URL").unwrap_or_else(|_| {
        format!(
            "https://www.17lands.com/card_ratings/data?expansion=MSH&format=PremierDraft&start_date={}&end_date={}",
            SET_RELEASE,
            today()
        )
    });
    let cards = parse_17lands(&get_text(&url)?)?;
    let n = cards.len();
    let file = RatingsFile {
        fetched: today(),
        source: "17lands.com Premier Draft".into(),
        cards,
    };
    let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    std::fs::write(ratings_path(data_dir), json).map_err(|e| e.to_string())?;
    Ok(n)
}

// ---------------------------------------------------------------- Playgroup.gg

fn slug(name: &str) -> String {
    normalize(name).replace(' ', "-")
}

/// Pulls "41% win rate across 151 recorded Playgroup games" (or "winning 41% of them")
/// out of a commander's public page.
pub fn parse_playgroup_html(html: &str) -> Option<(f32, u32)> {
    let text = html.replace(['\n', '\t'], " ");
    if let Some(i) = text.find("% win rate across ") {
        let before: String = text[..i]
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        let pct: f32 = before.chars().rev().collect::<String>().parse().ok()?;
        let after = &text[i + "% win rate across ".len()..];
        let games: u32 = after
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == ',')
            .filter(|c| *c != ',')
            .collect::<String>()
            .parse()
            .ok()?;
        return Some((pct / 100.0, games));
    }
    None
}

/// Looks through any JSON for an object about `name` that has win-rate / game-count fields.
pub fn find_stats_in_json(v: &serde_json::Value, name: &str) -> Option<(f32, u32)> {
    let target = normalize(name);
    match v {
        serde_json::Value::Object(map) => {
            let named = map
                .values()
                .any(|x| x.as_str().is_some_and(|s| normalize(s) == target));
            if named {
                let mut wr = None;
                let mut games = None;
                let mut wins = None;
                for (k, x) in map {
                    let key = k.to_lowercase().replace(['_', '-'], "");
                    if key.contains("winrate") {
                        wr = x
                            .as_f64()
                            .map(|f| if f > 1.0 { f / 100.0 } else { f } as f32);
                    } else if key == "games"
                        || key.contains("gamecount")
                        || key.contains("totalgames")
                        || key == "gamesplayed"
                    {
                        games = x.as_u64().map(|g| g as u32);
                    } else if key == "wins" {
                        wins = x.as_u64();
                    }
                }
                if let (Some(w), Some(g)) = (wins, games) {
                    if wr.is_none() && g > 0 {
                        wr = Some(w as f32 / g as f32);
                    }
                }
                if let (Some(w), Some(g)) = (wr, games) {
                    return Some((w, g));
                }
            }
            map.values().find_map(|x| find_stats_in_json(x, name))
        }
        serde_json::Value::Array(a) => a.iter().find_map(|x| find_stats_in_json(x, name)),
        _ => None,
    }
}

/// Known commander page IDs on Playgroup.gg (pages are /commanders/{id}-{slug}).
const KNOWN_PAGES: [(&str, u32); 3] = [
    ("Thanos, the Mad Titan", 3896),
    ("Madame Hydra", 3934),
    ("Red Hulk", 3986),
];

/// Tries Playgroup.gg's public API, then the public commander pages, for each name.
/// Saves whatever it finds; returns how many commanders got fresh numbers.
pub fn fetch_playgroup(data_dir: &Path, names: &[String]) -> Result<usize, String> {
    let base =
        std::env::var("MSH_PLAYGROUP_BASE").unwrap_or_else(|_| "https://playgroup.gg".into());
    let api = format!("{}/api/public/v1", base);
    // Learn query parameter names from the published API description, if reachable.
    let mut params: Vec<String> = vec!["search".into(), "q".into(), "name".into(), "query".into()];
    if let Ok(spec) = get_text(&format!("{}/openapi.yaml", api)) {
        for line in spec.lines() {
            let l = line.trim().trim_start_matches("- ");
            if let Some(p) = l.strip_prefix("name: ") {
                let p = p.trim().trim_matches('"').to_string();
                if !params.contains(&p) && p.len() < 20 && !p.contains(' ') {
                    params.insert(0, p);
                }
            }
        }
    }
    let mut file = std::fs::read_to_string(commander_stats_path(data_dir))
        .ok()
        .and_then(|t| serde_json::from_str::<CommanderStatsFile>(&t).ok())
        .unwrap_or_default();
    let mut found = 0;
    let mut working: Option<String> = None; // URL pattern that worked, reused for the rest
    for name in names {
        let s = slug(name);
        let enc = name
            .replace(' ', "%20")
            .replace(',', "%2C")
            .replace('\'', "%27");
        let mut patterns: Vec<String> = Vec::new();
        if let Some(w) = &working {
            patterns.push(w.clone());
        }
        for p in params.iter().take(6) {
            patterns.push(format!("{}/commanders?{}={{name}}", api, p));
        }
        patterns.push(format!("{}/commanders/{{slug}}", api));
        patterns.push(format!("{}/commanders/{{slug}}", base));
        if let Some((_, id)) = KNOWN_PAGES.iter().find(|(n, _)| n == name) {
            patterns.push(format!("{}/commanders/{}-{{slug}}", base, id));
        }
        for pat in patterns {
            let url = pat.replace("{name}", &enc).replace("{slug}", &s);
            let Ok(body) = get_text(&url) else { continue };
            std::thread::sleep(std::time::Duration::from_millis(250));
            let stats = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| find_stats_in_json(&v, name))
                .or_else(|| parse_playgroup_html(&body));
            if let Some((wr, games)) = stats {
                file.commanders.insert(
                    name.clone(),
                    CommanderStat {
                        win_rate: wr,
                        games,
                        source: format!("Playgroup.gg ({})", today()),
                    },
                );
                found += 1;
                working = Some(pat);
                break;
            }
        }
    }
    file.fetched = today();
    let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    std::fs::write(commander_stats_path(data_dir), json).map_err(|e| e.to_string())?;
    if found == 0 {
        return Err(
            "Playgroup.gg didn't return commander stats (offline, or its site changed)".into(),
        );
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_17lands_json() {
        let json = r#"[
            {"name": "Red Hulk", "ever_drawn_win_rate": 0.583, "ever_drawn_game_count": 4210, "avg_seen": 3.1, "drawn_improvement_win_rate": 0.031},
            {"name": "Bruce Banner // The Incredible Hulk", "ever_drawn_win_rate": 0.66, "ever_drawn_game_count": 900, "avg_seen": null, "drawn_improvement_win_rate": null},
            {"name": "Unplayed", "ever_drawn_win_rate": null, "ever_drawn_game_count": 0}
        ]"#;
        let r = parse_17lands(json).unwrap();
        assert_eq!(r["Red Hulk"].games, 4210);
        assert!((r["Red Hulk"].gih_wr.unwrap() - 0.583).abs() < 1e-4);
        assert!(r.contains_key("Bruce Banner"));
        assert!(r["Unplayed"].gih_wr.is_none());
    }

    #[test]
    fn parses_playgroup_text_and_json() {
        let html = "<p>Thanos wins 41% win rate across 151 recorded Playgroup games (62 wins)</p>";
        assert_eq!(parse_playgroup_html(html), Some((0.41, 151)));
        let json: serde_json::Value = serde_json::from_str(
            r#"{"data":[{"name":"Other","winRate":0.3,"games":10},{"name":"Red Hulk","win_rate":38.0,"total_games":13}]}"#,
        )
        .unwrap();
        assert_eq!(find_stats_in_json(&json, "Red Hulk"), Some((0.38, 13)));
    }

    #[test]
    fn dates_round_trip() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(age_days(&today()), 0);
    }
}
