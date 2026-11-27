//! Where the app keeps its files, plus the card database baked into the program so a lone
//! .exe still works.

use std::path::{Path, PathBuf};

const DEFAULT_CARDS: &str = include_str!("../data/cards.json");
const DEFAULT_COLLECTION: &str = include_str!("../data/collection.txt");
/// The league's real opponents (decks rebuilt from the trade log) shipped with the app,
/// and the date they were made: a newer set replaces the data folder's `table.json` once
/// (the old one is kept as a backup).
const LEAGUE_TABLE: &str = include_str!("../league_data/table.json");
pub const LEAGUE_DATA_VERSION: &str = "2026-11-27 round 4";

/// Finds the folder holding cards.json: `--data <dir>`, `./data`, `data` next to the
/// program, or the source tree's `data` (debug builds). If none exist, creates `data`
/// next to the program from the built-in copies.
pub fn data_dir(args: &[String]) -> PathBuf {
    if let Some(i) = args.iter().position(|a| a == "--data") {
        if let Some(d) = args.get(i + 1) {
            return PathBuf::from(d);
        }
    }
    let local = PathBuf::from("data");
    if local.join("cards.json").exists() {
        ensure_collection(&local);
        seed_league_data(&local);
        return local;
    }
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("data")));
    if let Some(d) = &beside_exe {
        if d.join("cards.json").exists() {
            refresh_card_database(d);
            ensure_collection(d);
            seed_league_data(d);
            return d.clone();
        }
    }
    // Development builds (`cargo run` from elsewhere) use the source tree's data.
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
    if cfg!(debug_assertions) && source.join("cards.json").exists() {
        return source;
    }
    let target = beside_exe.unwrap_or(local);
    let _ = std::fs::create_dir_all(&target);
    let _ = std::fs::write(target.join("cards.json"), DEFAULT_CARDS);
    ensure_collection(&target);
    target
}

/// Writes the empty starter pool (with its how-to header) if there isn't one yet.
fn ensure_collection(dir: &Path) {
    if !dir.join("collection.txt").exists() {
        let _ = std::fs::write(dir.join("collection.txt"), DEFAULT_COLLECTION);
    }
}

/// A newer program version may ship more cards than the copy on disk; add any it lacks
/// without touching cards the user typed in.
fn refresh_card_database(dir: &Path) {
    let path = dir.join("cards.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(mut on_disk) = serde_json::from_str::<Vec<serde_json::Value>>(&text) else {
        return;
    };
    let Ok(built_in) = serde_json::from_str::<Vec<serde_json::Value>>(DEFAULT_CARDS) else {
        return;
    };
    let have: std::collections::HashSet<String> = on_disk
        .iter()
        .filter_map(|c| c["name"].as_str().map(String::from))
        .collect();
    let before = on_disk.len();
    on_disk.extend(
        built_in
            .into_iter()
            .filter(|c| c["name"].as_str().is_some_and(|n| !have.contains(n))),
    );
    if on_disk.len() > before {
        if let Ok(json) = serde_json::to_string_pretty(&on_disk) {
            let _ = std::fs::write(&path, json + "\n");
        }
    }
}

/// Decks live next to the data folder: <project>/decks.
pub fn decks_dir(data_dir: &Path) -> PathBuf {
    data_dir
        .parent()
        .map(|p| p.join("decks"))
        .unwrap_or_else(|| PathBuf::from("decks"))
}

/// Card images are saved once in an `images` folder next to the program (beside
/// `data` and `decks`) and reused from there.
pub fn images_dir(data_dir: &Path) -> PathBuf {
    data_dir
        .parent()
        .map(|p| p.join("images"))
        .unwrap_or_else(|| PathBuf::from("images"))
}

/// Installs the shipped league opponents into `dir/table.json` when they are newer than
/// the ones installed before (a stamp file records the version). The table it replaces is
/// kept as `table.before-update.json`. Returns true when it installed them.
pub fn seed_league_data(dir: &Path) -> bool {
    let stamp = dir.join(".league_data_version");
    if std::fs::read_to_string(&stamp).is_ok_and(|v| v.trim() == LEAGUE_DATA_VERSION) {
        return false;
    }
    let table = dir.join("table.json");
    if table.exists() {
        let _ = std::fs::copy(&table, dir.join("table.before-update.json"));
    }
    if std::fs::write(&table, LEAGUE_TABLE).is_err() {
        return false;
    }
    let _ = std::fs::write(&stamp, LEAGUE_DATA_VERSION);
    true
}

#[cfg(test)]
mod league_data_tests {
    use super::*;

    #[test]
    fn league_table_installs_once_and_keeps_a_backup() {
        let dir = std::env::temp_dir().join(format!("msh_league_seed_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("table.json"), "{\"opponents\":[]}").unwrap();
        assert!(seed_league_data(&dir));
        let t: crate::table::Table =
            serde_json::from_str(&std::fs::read_to_string(dir.join("table.json")).unwrap())
                .unwrap();
        assert!(
            t.opponents.len() >= 8,
            "the shipped table has every league opponent"
        );
        assert!(dir.join("table.before-update.json").exists());
        // A second start leaves the user's edits alone.
        std::fs::write(dir.join("table.json"), "{\"opponents\":[]}").unwrap();
        assert!(!seed_league_data(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
