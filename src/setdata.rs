//! Keeps the card database current with every set in the league rules, using Scryfall (the
//! standard Magic card database) on the user's PC: Marvel Super Heroes, The Hobbit,
//! Reality Fracture, Nauctis: The Sunken Realms (once released) and Foundations (the
//! Public Pool starter kit). New cards are added; cards already in the database are kept.

use crate::card::Card;
use std::path::Path;

/// Sets named in the Season 2 rules, matched against Scryfall's set names.
pub const LEAGUE_SETS: [(&str, &str); 5] = [
    ("Marvel Super Heroes", "MSH"),
    ("The Hobbit", "HOB"),
    ("Reality Fracture", "FRA"),
    ("Nauctis", ""), // code unknown until release; looked up by name
    ("Foundations", "FDN"),
];

fn get_json(url: &str) -> Result<serde_json::Value, String> {
    let body = ureq::get(url)
        .header("User-Agent", "MSHDeckbuilder/0.5 (personal deck builder)")
        .header("Accept", "application/json")
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .with_config()
        .limit(64 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&body).map_err(|e| e.to_string())
}

fn api() -> String {
    std::env::var("MSH_SCRYFALL_API").unwrap_or_else(|_| "https://api.scryfall.com".into())
}

/// Scryfall set codes for the league sets (Nauctis found by name once it exists).
pub fn resolve_codes() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let sets = get_json(&format!("{}/sets", api())).ok();
    for (name, code) in LEAGUE_SETS {
        let found = sets.as_ref().and_then(|v| {
            v["data"].as_array()?.iter().find_map(|s| {
                let n = s["name"].as_str()?;
                let main = s["set_type"]
                    .as_str()
                    .is_none_or(|t| t == "expansion" || t == "core");
                (n.to_lowercase().contains(&name.to_lowercase()) && main)
                    .then(|| s["code"].as_str().map(|c| c.to_uppercase()))
                    .flatten()
            })
        });
        match (found, code) {
            (Some(c), _) => out.push((name.to_string(), c)),
            (None, c) if !c.is_empty() => out.push((name.to_string(), c.to_string())),
            _ => {}
        }
    }
    out
}

fn s(v: &serde_json::Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

/// Turns one Scryfall card object into the app's card format (front face, with the back
/// face's text appended for double-faced cards).
pub fn card_from_scryfall(v: &serde_json::Value) -> Option<Card> {
    let faces = v["card_faces"].as_array();
    let front = faces.and_then(|f| f.first()).unwrap_or(v);
    let name = s(&front["name"]);
    if name.is_empty() {
        return None;
    }
    let pick = |key: &str| {
        let top = s(&v[key]);
        if top.is_empty() { s(&front[key]) } else { top }
    };
    let mut text = s(&front["oracle_text"]);
    if text.is_empty() {
        text = s(&v["oracle_text"]);
    }
    if let Some(back) = faces.and_then(|f| f.get(1)) {
        let pt = match (back["power"].as_str(), back["toughness"].as_str()) {
            (Some(p), Some(t)) => format!(" {}/{}", p, t),
            _ => String::new(),
        };
        text += &format!(
            "\n\n// Back face: {} {}\n{}{}\n{}",
            s(&back["name"]),
            s(&back["mana_cost"]),
            s(&back["type_line"]),
            pt,
            s(&back["oracle_text"])
        );
    }
    let mana_cost = if faces.is_some() {
        s(&front["mana_cost"])
    } else {
        pick("mana_cost")
    };
    let type_line = if faces.is_some() {
        s(&front["type_line"])
    } else {
        pick("type_line")
    };
    Some(Card {
        name,
        mana_cost,
        cmc: v["cmc"].as_f64().unwrap_or(0.0) as f32,
        type_line,
        oracle_text: text,
        power: front["power"].as_str().map(String::from),
        toughness: front["toughness"].as_str().map(String::from),
        rarity: s(&v["rarity"]),
        color_identity: v["color_identity"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|c| c.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        set: s(&v["set"]).to_uppercase(),
        collector_number: v["collector_number"].as_str().map(String::from),
        confidence: "verified".into(),
        note: "Rules text from Scryfall (scryfall.com).".into(),
        ..Default::default()
    })
}

/// Downloads every league set from Scryfall and adds cards the database doesn't have.
/// Returns (cards added, sets checked, set names not released yet).
pub fn update_cards(data_dir: &Path) -> Result<(usize, Vec<String>, Vec<String>), String> {
    let path = data_dir.join("cards.json");
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut cards: Vec<Card> = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let mut have: std::collections::HashSet<String> = cards
        .iter()
        .map(|c| crate::db::normalize(&c.name))
        .collect();
    let codes = resolve_codes();
    let mut checked = Vec::new();
    let mut added = 0;
    let mut errors = Vec::new();
    for (name, code) in &codes {
        let mut url = Some(format!(
            "{}/cards/search?q=e%3A{}&unique=cards&order=set",
            api(),
            code.to_lowercase()
        ));
        let mut ok = false;
        while let Some(u) = url.take() {
            match get_json(&u) {
                Ok(page) => {
                    ok = true;
                    for v in page["data"].as_array().into_iter().flatten() {
                        if let Some(c) = card_from_scryfall(v) {
                            let key = crate::db::normalize(&c.name);
                            if !c.type_line.contains("Basic Land") && have.insert(key) {
                                cards.push(c);
                                added += 1;
                            }
                        }
                    }
                    if page["has_more"].as_bool() == Some(true) {
                        url = page["next_page"].as_str().map(String::from);
                    }
                }
                Err(e) => errors.push(format!("{}: {}", name, e)),
            }
            std::thread::sleep(std::time::Duration::from_millis(120));
        }
        if ok {
            checked.push(format!("{} ({})", name, code));
        }
    }
    if checked.is_empty() {
        return Err(format!("Couldn't reach Scryfall ({})", errors.join("; ")));
    }
    if added > 0 {
        let json = serde_json::to_string_pretty(&cards).map_err(|e| e.to_string())?;
        std::fs::write(&path, json + "\n").map_err(|e| e.to_string())?;
    }
    let missing: Vec<String> = LEAGUE_SETS
        .iter()
        .filter(|(n, _)| !codes.iter().any(|(m, _)| m == n))
        .map(|(n, _)| n.to_string())
        .collect();
    Ok((added, checked, missing))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_scryfall_cards() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"name":"Red Hulk","mana_cost":"{4}{R}{R}","cmc":6.0,"type_line":"Legendary Creature — Gamma Berserker Villain",
                "oracle_text":"Reach, trample","power":"6","toughness":"7","rarity":"uncommon","color_identity":["R"],
                "set":"msh","collector_number":"149"}"#,
        )
        .unwrap();
        let c = card_from_scryfall(&v).unwrap();
        assert_eq!(
            (c.name.as_str(), c.set.as_str(), c.mana_value()),
            ("Red Hulk", "MSH", 6)
        );
        let dfc: serde_json::Value = serde_json::from_str(
            r#"{"name":"Bruce Banner // The Incredible Hulk","cmc":1.0,"rarity":"mythic","color_identity":["G","R","U"],
                "set":"msh","collector_number":"49","card_faces":[
                {"name":"Bruce Banner","mana_cost":"{U}","type_line":"Legendary Creature — Human Scientist Hero","oracle_text":"Draw.","power":"1","toughness":"1"},
                {"name":"The Incredible Hulk","mana_cost":"{2}{R}{R}{G}{G}","type_line":"Legendary Creature — Gamma Berserker Hero","oracle_text":"Reach","power":"8","toughness":"8"}]}"#,
        )
        .unwrap();
        let b = card_from_scryfall(&dfc).unwrap();
        assert_eq!(b.name, "Bruce Banner");
        assert_eq!(b.mana_cost, "{U}");
        assert!(b.oracle_text.contains("// Back face: The Incredible Hulk"));
        assert_eq!(b.identity().to_string(), "URG");
    }
}
