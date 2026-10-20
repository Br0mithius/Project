//! Where the app keeps its files, plus the card database baked into the program so a lone
//! .exe still works.

use std::path::{Path, PathBuf};

const DEFAULT_CARDS: &str = include_str!("../data/cards.json");
const DEFAULT_COLLECTION: &str = include_str!("../data/collection.txt");

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
        return local;
    }
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("data")));
    if let Some(d) = &beside_exe {
        if d.join("cards.json").exists() {
            refresh_card_database(d);
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
    if !target.join("collection.txt").exists() {
        let _ = std::fs::write(target.join("collection.txt"), DEFAULT_COLLECTION);
    }
    target
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
