//! How the game simulator handles every card in the database:
//! `cargo run --release --example sim_coverage -- <data dir>`.
//!
//! Counts each trigger, condition, effect, keyword and special rule the parser reads,
//! and says whether the simulator plays it exactly, approximately, or not at all.
use msh_deckbuilder::db::CardDb;
use msh_deckbuilder::playtest::compile::{self, Cond, Ev, Trig};
use msh_deckbuilder::playtest::{KEYWORDS_PLAYED, effect_handling};
use std::collections::BTreeMap;

fn trig_name(t: &Trig) -> (String, &'static str) {
    match t {
        Trig::Spell => ("spell resolves".into(), "exact"),
        Trig::Enters => ("enters".into(), "exact"),
        Trig::Dies => ("dies".into(), "exact"),
        Trig::Attacks => ("attacks".into(), "exact"),
        Trig::CombatDamage => ("deals combat damage to a player".into(), "exact"),
        Trig::Phase { phase, any_player } => (
            format!(
                "beginning of {:?}{}",
                phase,
                if *any_player { " (each player's)" } else { "" }
            ),
            "exact",
        ),
        Trig::Event(e, _) => (
            format!("event: {:?}", e),
            if *e == Ev::Other {
                "no effect"
            } else {
                "exact"
            },
        ),
        Trig::Activated { .. } => ("activated ability".into(), "exact"),
        Trig::PowerUp(_) => ("power-up".into(), "exact"),
        Trig::Loyalty(_) => ("loyalty ability".into(), "exact"),
        Trig::Chapter(_) => ("Saga chapter".into(), "exact"),
        Trig::Reflexive => ("reflexive (\"when you do\")".into(), "exact"),
        Trig::Static => ("static".into(), "exact"),
    }
}

fn cond_name(c: &Cond) -> (String, &'static str) {
    let name = format!("{:?}", c);
    let name = name.split('(').next().unwrap_or(&name).to_string();
    let how = if matches!(c, Cond::Chance(_)) {
        "approximated"
    } else {
        "exact"
    };
    (name, how)
}

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
                effects.entry(name).or_insert((how, note, 0)).2 += 1;
                *totals.entry(how).or_default() += 1;
            }
        }
        for k in &card.parsed().keywords {
            let base = KEYWORDS_PLAYED
                .iter()
                .find(|w| {
                    k == *w
                        || k.starts_with(&format!("{} ", w))
                        || k.starts_with(&format!("{}—", w))
                        || k.starts_with(&format!("{} {{", w))
                })
                .copied();
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
