//! How the game simulator handles every card in the database:
//! `cargo run --release --example sim_coverage -- <data dir>`.
//!
//! Counts each trigger, condition, effect, keyword and special rule the parser reads,
//! and says whether the simulator plays it exactly, approximately, or not at all.
use msh_deckbuilder::db::CardDb;
use msh_deckbuilder::playtest::compile::{self, Cond, Ev, Trig};
use msh_deckbuilder::playtest::coverage::{card_coverage, cond_name, keyword_base, trig_name};
use msh_deckbuilder::playtest::effect_handling;
use std::collections::BTreeMap;

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "data".into()));
    let db = CardDb::load(&dir.join("cards.json")).unwrap();
    let mut triggers: BTreeMap<String, (&str, u32)> = BTreeMap::new();
    let mut conds: BTreeMap<String, (&str, u32)> = BTreeMap::new();
    let mut effects: BTreeMap<String, (&str, &str, u32)> = BTreeMap::new();
    let mut keywords: BTreeMap<String, (&str, u32)> = BTreeMap::new();
    let mut rules: BTreeMap<String, (&str, u32)> = BTreeMap::new();
    let mut totals: BTreeMap<&str, u32> = BTreeMap::new();
    let mut unread_events: Vec<String> = Vec::new();
    let mut chance_texts: Vec<String> = Vec::new();
    for card in &db.cards {
        let c = compile::compile(card);
        let back = c.back.iter().flat_map(|b| b.abilities.iter());
        for a in c.abilities.iter().chain(back) {
            let (t, how) = trig_name(&a.trig);
            triggers.entry(t).or_insert((how, 0)).1 += 1;
            if let Trig::Event(Ev::Other, _) = a.trig {
                unread_events.push(format!("{}: {}", card.name, a.text));
            }
            if matches!(a.cond, Cond::Chance(_)) {
                chance_texts.push(format!("{}: {}", card.name, a.text));
            }
            let (cn, chow) = cond_name(&a.cond);
            conds.entry(cn).or_insert((chow, 0)).1 += 1;
            for e in &a.effects {
                let (how, note) = effect_handling(e);
                let name = match e {
                    msh_deckbuilder::abilities::Effect::Rule(l) => {
                        rules.entry(l.clone()).or_insert((how, 0)).1 += 1;
                        "Rule (see below)".to_string()
                    }
                    other => {
                        let d = format!("{:?}", other);
                        d.split([' ', '(', '{']).next().unwrap_or(&d).to_string()
                    }
                };
                let (how_row, note_row) = if name.starts_with("Rule") {
                    ("mixed", "special rules: each listed below")
                } else {
                    (how, note)
                };
                effects.entry(name).or_insert((how_row, note_row, 0)).2 += 1;
                *totals.entry(how).or_default() += 1;
            }
        }
        for k in &card.parsed().keywords {
            let base = keyword_base(k);
            let (name, how) = match base {
                Some("sneak") => ("sneak".to_string(), "approximated"),
                Some(b) => (b.to_string(), "exact"),
                None if k.contains("cycling") => {
                    ("typecycling (land / creature type)".to_string(), "exact")
                }
                None => (k.clone(), "no effect"),
            };
            keywords.entry(name).or_insert((how, 0)).1 += 1;
        }
    }
    println!("== Effects ({} total)", totals.values().sum::<u32>());
    for (k, (how, note, n)) in &effects {
        println!("  {:<16} {:>5}  {:<13} {}", k, n, how, note);
    }
    for (how, n) in &totals {
        println!("  -> {}: {}", how, n);
    }
    println!("\n== Special rules (Effect::Rule)");
    for (k, (how, n)) in &rules {
        println!("  {:>4}  {:<13} {}", n, how, k);
    }
    println!("\n== Triggers");
    for (k, (how, n)) in &triggers {
        println!("  {:>5}  {:<13} {}", n, how, k);
    }
    println!("\n== Conditions");
    for (k, (how, n)) in &conds {
        println!("  {:>5}  {:<13} {}", n, how, k);
    }
    println!("\n== Keywords");
    for (k, (how, n)) in &keywords {
        println!("  {:>5}  {:<13} {}", n, how, k);
    }
    if std::env::args().any(|a| a == "--chance") {
        println!("\n== Conditions decided by chance ({})", chance_texts.len());
        for t in &chance_texts {
            println!("  {}", t.chars().take(170).collect::<String>());
        }
    }
    if let Some(i) = std::env::args().position(|a| a == "--tsv") {
        let out = std::env::args().nth(i + 1).expect("--tsv <file>");
        let groups = std::env::args()
            .position(|a| a == "--groups")
            .and_then(|i| std::env::args().nth(i + 1));
        write_tsv(&db, &out, groups.as_deref());
    }
    if !unread_events.is_empty() {
        println!(
            "\n== Events the simulator doesn't fire ({})",
            unread_events.len()
        );
        for e in unread_events.iter().take(40) {
            println!("  {}", e.chars().take(150).collect::<String>());
        }
    }
}

/// Per card and ability: the text, how it is parsed and how the simulator plays it
/// (`playtest::coverage`).
fn write_tsv(db: &CardDb, path: &str, groups: Option<&str>) {
    use std::fmt::Write as _;
    let group: BTreeMap<String, String> = groups
        .and_then(|g| std::fs::read_to_string(g).ok())
        .map(|t| {
            t.lines()
                .filter_map(|l| l.split_once('\t'))
                .map(|(n, g)| (n.to_string(), g.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let mut tsv = String::from(
        "card\tset\tgroup\tcard_status\tability\ttrigger\ttext\tparsed_as\thandling\tnotes\n",
    );
    let mut totals: BTreeMap<(String, &str), u32> = BTreeMap::new();
    for card in &db.cards {
        let cov = card_coverage(card);
        let status = cov.status.code();
        let g = group
            .get(&card.name)
            .map(String::as_str)
            .unwrap_or("e_other");
        *totals.entry((g.to_string(), status)).or_default() += 1;
        *totals.entry(("all".to_string(), status)).or_default() += 1;
        for p in &cov.parts {
            let _ = writeln!(
                tsv,
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                card.name,
                card.set,
                g,
                status,
                p.id,
                p.trigger,
                p.text.replace(['\t', '\n'], " "),
                p.parsed_as.replace(['\t', '\n'], " "),
                p.handling.code(),
                p.notes.replace(['\t', '\n'], " ")
            );
        }
    }
    std::fs::write(path, tsv).expect("write tsv");
    println!("\n== Cards by worst handling (TSV: {})", path);
    for ((g, h), n) in totals {
        println!("  {:<14} {:<7} {}", g, h, n);
    }
}
