//! Fingerprints the source files the season simulation depends on (`SIM_FINGERPRINT`), so
//! `opponents::cache_key` changes with the code and a stale `gauntlet_cache.json` is rebuilt.

use std::path::Path;

/// Files that can't change a simulated season (user interface, the playtest AI).
const SKIP: &[&str] = &["app.rs", "ui.rs", "main.rs", "lab.rs"];

fn main() {
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir("src")
        .map(|d| {
            d.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "rs"))
                .filter(|p| {
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    !SKIP.contains(&name)
                })
                .collect()
        })
        .unwrap_or_default();
    // Card abilities are read through the playtest compiler (restrict.rs).
    files.push(Path::new("src/playtest/compile.rs").to_path_buf());
    files.sort();
    let mut h: u64 = 0xcbf29ce484222325;
    for f in &files {
        // Line endings don't count, so a Windows checkout hashes the same.
        let text = std::fs::read_to_string(f)
            .unwrap_or_default()
            .replace('\r', "");
        let label = f.to_string_lossy().replace('\\', "/");
        for b in label.bytes().chain(text.bytes()) {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    // Any change under src (new files too) re-runs this.
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rustc-env=SIM_FINGERPRINT={:016x}", h);
}
