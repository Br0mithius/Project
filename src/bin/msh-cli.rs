//! MSH Deckbuilder, command-line edition: the same deck builder as a typed REPL.
//! (The windowed app is the main `msh-deckbuilder` binary.)

use msh_deckbuilder::app::{App, COMMANDS};
use msh_deckbuilder::paths::data_dir;
use msh_deckbuilder::ui::Style;
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{Context, Editor, Helper};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{IsTerminal, Read, Write};

/// Tab-completes command names, then card names.
struct Completion {
    names: Vec<String>,
}

impl Completer for Completion {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let upto = &line[..pos];
        let Some(space) = upto.find(' ') else {
            let c = COMMANDS
                .iter()
                .filter(|(c, _, _)| c.starts_with(upto))
                .map(|(c, _, _)| Pair {
                    display: c.to_string(),
                    replacement: format!("{} ", c),
                })
                .collect();
            return Ok((0, c));
        };
        // Skip an optional count ("add 2 ro<TAB>").
        let mut start = space + 1;
        let rest = &upto[start..];
        if let Some((first, _)) = rest.split_once(' ') {
            if first.trim_end_matches('x').parse::<u32>().is_ok() {
                start += first.len() + 1;
            }
        }
        let frag = upto[start..].to_lowercase();
        let mut hits: Vec<&String> = self
            .names
            .iter()
            .filter(|n| n.to_lowercase().starts_with(&frag))
            .collect();
        if hits.is_empty() {
            hits = self
                .names
                .iter()
                .filter(|n| n.to_lowercase().contains(&frag))
                .collect();
        }
        Ok((
            start,
            hits.into_iter()
                .map(|n| Pair {
                    display: n.clone(),
                    replacement: n.clone(),
                })
                .collect(),
        ))
    }
}

impl Hinter for Completion {
    type Hint = String;
}
impl Highlighter for Completion {}
impl Validator for Completion {}
impl Helper for Completion {}

/// Older Windows consoles need ANSI colour codes switched on; returns false if that fails.
#[cfg(windows)]
fn enable_ansi() -> bool {
    use windows_sys::Win32::System::Console::{
        ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetStdHandle, STD_OUTPUT_HANDLE,
        SetConsoleMode,
    };
    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut mode = 0;
        GetConsoleMode(handle, &mut mode) != 0
            && SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
    }
}

#[cfg(not(windows))]
fn enable_ansi() -> bool {
    true
}

/// Keeps a double-clicked console window open long enough to read an error.
fn pause_before_exit(interactive: bool) {
    if interactive {
        println!("Press Enter to close.");
        let mut s = String::new();
        let _ = std::io::stdin().read_line(&mut s);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let interactive = std::io::stdin().is_terminal();
    let color =
        std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() && enable_ansi();
    let mut app = match App::new(data_dir(&args), Style { color }) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Error: {}", e);
            pause_before_exit(interactive);
            std::process::exit(1);
        }
    };

    // `msh-deckbuilder -c "auto red hulk" -c stats` runs commands and exits.
    let scripted: Vec<String> = args
        .windows(2)
        .filter(|w| w[0] == "-c")
        .map(|w| w[1].clone())
        .collect();
    let mut no_ask = |_: &str| -> Option<String> { None };
    if !scripted.is_empty() {
        for cmd in scripted {
            println!("{}", app.style.bold(&format!("> {}", cmd)));
            app.run(&cmd, &mut no_ask);
        }
        return;
    }

    if !interactive {
        // Piped input: run each line as a command (handy for scripts and testing).
        // Follow-up questions (from `newcard`) take the next lines of the same input.
        let mut input = String::new();
        let _ = std::io::stdin().read_to_string(&mut input);
        let queue = RefCell::new(
            input
                .lines()
                .map(|l| l.to_string())
                .collect::<VecDeque<_>>(),
        );
        loop {
            let Some(line) = queue.borrow_mut().pop_front() else {
                break;
            };
            println!("> {}", line);
            let mut ask = |p: &str| -> Option<String> {
                let answer = queue.borrow_mut().pop_front();
                println!("{}{}", p, answer.clone().unwrap_or_default());
                answer
            };
            if !app.run(&line, &mut ask) {
                break;
            }
        }
        return;
    }

    println!(
        "{}\n{} cards in database, {} cards in your collection. Type `help` to start, `commanders` to see options.\n{}\n",
        app.style
            .bold("MSH Deckbuilder - 60-card commander from a Marvel Super Heroes pool"),
        app.db.cards.len(),
        app.coll.total(),
        app.style.dim(&format!(
            "Your files: {}",
            std::fs::canonicalize(&app.data_dir)
                .unwrap_or(app.data_dir.clone())
                .display()
        ))
    );
    let mut rl: Editor<Completion, DefaultHistory> = match Editor::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Could not start the line editor: {}", e);
            std::process::exit(1);
        }
    };
    rl.set_helper(Some(Completion {
        names: app.card_names(),
    }));
    let history = app.data_dir.join(".history");
    let _ = rl.load_history(&history);
    loop {
        match rl.readline(&app.prompt()) {
            Ok(line) => {
                let _ = rl.add_history_entry(line.as_str());
                let keep_going = {
                    // `newcard` asks follow-up questions; read them with a plain prompt.
                    let mut ask = |p: &str| -> Option<String> {
                        print!("{}", p);
                        let _ = std::io::stdout().flush();
                        let mut s = String::new();
                        std::io::stdin().read_line(&mut s).ok()?;
                        Some(s.trim_end_matches(['\n', '\r']).to_string())
                    };
                    app.run(&line, &mut ask)
                };
                if app.names_changed {
                    rl.set_helper(Some(Completion {
                        names: app.card_names(),
                    }));
                    app.names_changed = false;
                }
                if !keep_going {
                    break;
                }
            }
            Err(ReadlineError::Interrupted) => println!("(Ctrl-C) type `quit` to exit"),
            Err(ReadlineError::Eof) => {
                app.run("quit", &mut no_ask);
                break;
            }
            Err(e) => {
                eprintln!("{}", e);
                break;
            }
        }
    }
    let _ = rl.save_history(&history);
}
