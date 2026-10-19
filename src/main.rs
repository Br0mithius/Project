//! MSH Deckbuilder - an interactive deck builder for a 60-card, non-singleton
//! commander deck built from a Marvel Super Heroes booster pool.

mod analysis;
mod app;
mod card;
mod db;
mod deck;
mod ui;

use app::{App, COMMANDS};
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
use std::path::PathBuf;
use ui::Style;

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

/// Finds the folder holding cards.json: --data <dir>, ./data, or the crate's data dir.
fn data_dir(args: &[String]) -> PathBuf {
    if let Some(i) = args.iter().position(|a| a == "--data") {
        if let Some(d) = args.get(i + 1) {
            return PathBuf::from(d);
        }
    }
    let local = PathBuf::from("data");
    if local.join("cards.json").exists() {
        return local;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data")
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let interactive = std::io::stdin().is_terminal();
    let color = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    let mut app = match App::new(data_dir(&args), Style { color }) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Error: {}", e);
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
        "{}\n{} cards in database, {} cards in your collection. Type `help` to start, `commanders` to see options.\n",
        app.style
            .bold("MSH Deckbuilder - 60-card commander from a Marvel Super Heroes pool"),
        app.db.cards.len(),
        app.coll.total()
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
