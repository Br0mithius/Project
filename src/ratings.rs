//! Outside commander data, downloaded on your PC and saved in `data/`:
//! **Playgroup.gg** commander win rates from logged casual Commander games (mostly 4-player
//! pods; a 25% win rate is average). These are 100-card constructed decks, so they're a
//! weaker signal for league pools and are weighted lightly. Optional. Also the league sets'
//! release dates, used to judge how much data a set has had time to collect.
//!
//! 17Lands draft ratings were used here once; they're no longer used and any saved copy is
//! deleted (`remove_17lands_data`).

use crate::db::normalize;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Release dates of the league sets.
const SET_RELEASES: [(&str, &str); 4] = [
    ("MSH", "2026-06-26"),
    ("HOB", "2026-08-14"),
    ("FRA", "2026-10-02"),
    ("FDN", "2024-11-15"),
];

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

/// Deletes 17Lands draft ratings saved by earlier versions (they're no longer used).
pub fn remove_17lands_data(data_dir: &Path) {
    let old = data_dir.join("ratings_17lands.json");
    if old.exists() {
        let _ = std::fs::remove_file(old);
    }
}

pub fn commander_stats_path(data_dir: &Path) -> PathBuf {
    data_dir.join("commander_stats.json")
}

/// Release date of a league set ("YYYY-MM-DD"), if known.
pub fn set_release(set: &str) -> Option<&'static str> {
    SET_RELEASES
        .iter()
        .find(|(c, _)| *c == set)
        .map(|(_, d)| *d)
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

// ---------------------------------------------------------------- downloads

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
    let mut tried = 0;
    let mut working: Option<String> = None; // URL pattern that worked, reused for the rest
    for name in names {
        // Be gentle: if nothing works for the first few commanders, stop instead of
        // trying every pattern for every commander.
        if working.is_none() && tried >= 3 {
            break;
        }
        tried += 1;
        let s = slug(name);
        let enc = name
            .replace(' ', "%20")
            .replace(',', "%2C")
            .replace('\'', "%27");
        let mut patterns: Vec<String> = Vec::new();
        match &working {
            // Once an address format works, only that one is used (one request per commander).
            Some(w) => patterns.push(w.clone()),
            None => {
                for p in params.iter().take(3) {
                    patterns.push(format!("{}/commanders?{}={{name}}", api, p));
                }
                patterns.push(format!("{}/commanders/{{slug}}", api));
                patterns.push(format!("{}/commanders/{{slug}}", base));
            }
        }
        if let Some((_, id)) = KNOWN_PAGES.iter().find(|(n, _)| n == name) {
            patterns.push(format!("{}/commanders/{}-{{slug}}", base, id));
        }
        for pat in patterns {
            let url = pat.replace("{name}", &enc).replace("{slug}", &s);
            // A pause before every request, successful or not.
            std::thread::sleep(std::time::Duration::from_millis(400));
            let Ok(body) = get_text(&url) else { continue };
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
                // A commander-specific page address can't be reused for the others.
                if !KNOWN_PAGES
                    .iter()
                    .any(|(_, id)| pat.contains(&format!("/commanders/{}-", id)))
                {
                    working = Some(pat);
                }
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
