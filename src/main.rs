//! MSH Deckbuilder - windowed app.
// No console window behind the app on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = msh_deckbuilder::paths::data_dir(&args);
    if let Err(e) = msh_deckbuilder::gui::run(dir) {
        eprintln!("{}", e);
        std::process::exit(1);
    }
}
