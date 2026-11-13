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
        Trig::Phase {
            phase,
            any_player,
            opponents_only,
        } => (
            format!(
                "beginning of {:?}{}",
                phase,
                if *opponents_only {
                    " (each opponent's)"
                } else if *any_player {
                    " (each player's)"
                } else {
                    ""
                }
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

/// Text the simulator reads straight off an ability (`text.contains("...")` in its sources):
/// a static the parser leaves without effects can still be played this way.
fn sim_literals() -> Vec<String> {
    let srcs = [
        include_str!("../src/playtest/combat.rs"),
        include_str!("../src/playtest/state.rs"),
        include_str!("../src/playtest/mod.rs"),
        include_str!("../src/playtest/ai.rs"),
        include_str!("../src/playtest/resolve.rs"),
        include_str!("../src/playtest/compile.rs"),
        include_str!("../src/playtest/targets.rs"),
        include_str!("../src/restrict.rs"),
    ];
    let mut out = Vec::new();
    for s in srcs {
        for pat in [
            "contains(\"",
            "starts_with(\"",
            "split(\"",
            "find(\"",
            "strip_prefix(\"",
        ] {
            for part in s.split(pat).skip(1) {
                if let Some(lit) = part.split('"').next() {
                    // Short words ("flying", "draw") would match anything.
                    if lit.len() >= 12 && !lit.contains('\\') {
                        out.push(lit.to_string());
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

type Row = (String, String, String, String, &'static str, String);

/// Per card and ability: the text, how it is parsed and how the simulator plays it.
fn write_tsv(db: &CardDb, path: &str, groups: Option<&str>) {
    use std::fmt::Write as _;
    let lits = sim_literals();
    let group: BTreeMap<String, String> = groups
        .and_then(|g| std::fs::read_to_string(g).ok())
        .map(|t| {
            t.lines()
                .filter_map(|l| l.split_once('\t'))
                .map(|(n, g)| (n.to_string(), g.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let rank = |h: &str| match h {
        "none" => 2,
        "approx" => 1,
        _ => 0,
    };
    let mut tsv = String::from(
        "card\tset\tgroup\tcard_status\tability\ttrigger\ttext\tparsed_as\thandling\tnotes\n",
    );
    let mut totals: BTreeMap<(String, &str), u32> = BTreeMap::new();
    for card in &db.cards {
        let sim = compile::compile(card);
        let parsed = card.parsed();
        let mut rows: Vec<Row> = Vec::new();
        let mut special = Vec::new();
        if sim.rules != 0 {
            special.push(format!("rules bits {:#x}", sim.rules));
        }
        if sim.attack_tax.is_some() {
            special.push("attack tax".into());
        }
        if sim.opp_tax.is_some() {
            special.push("opponent spell tax".into());
        }
        if sim.cost_reduction.is_some() {
            special.push("cost reduction".into());
        }
        if sim.attached_sets.is_some() {
            special.push("attached sets".into());
        }
        if sim.hexproof_from != 0 {
            special.push("hexproof from".into());
        }
        let back = sim.back.iter().flat_map(|b| b.abilities.iter());
        for (k, a) in sim.abilities.iter().chain(back).enumerate() {
            let (trig, _) = trig_name(&a.trig);
            let mut how = "exact";
            let mut notes: Vec<String> = Vec::new();
            if let Trig::Event(Ev::Other, _) = a.trig {
                how = "none";
                notes.push("trigger event never fires".into());
            }
            if matches!(a.cond, Cond::Chance(_)) {
                if rank(how) < 1 {
                    how = "approx";
                }
                notes.push("condition decided by chance".into());
            }
            for e in &a.effects {
                let (h, note) = effect_handling(e);
                let h = match h {
                    "exact" => "exact",
                    "approximated" => "approx",
                    _ => "none",
                };
                if rank(h) > rank(how) {
                    how = h;
                }
                if h != "exact" {
                    let d = format!("{:?}", e);
                    let d = match e {
                        msh_deckbuilder::abilities::Effect::Rule(l) => format!("Rule \"{}\"", l),
                        _ => d.split([' ', '(', '{']).next().unwrap_or(&d).to_string(),
                    };
                    notes.push(format!("{}: {}", d, note));
                }
            }
            let compiled = (a.text.contains("enters tapped")
                && (sim.enters_tapped || sim.enters_tapped_unless.is_some()))
                || (a.text.contains("can't block") && sim.kw & compile::CANT_BLOCK != 0)
                || (a.text.contains("can't be blocked") && sim.kw & compile::UNBLOCKABLE != 0)
                // (A spell face's additional cost: compiled into its cast cost.)
                || a.text.starts_with("as an additional cost to cast ~");
            if a.effects.is_empty() && compiled {
                notes.push("compiled into the card's static fields".into());
            } else if a.effects.is_empty() && a.text.starts_with("as ~ enters, choose") {
                how = "approx";
                notes.push("the choice is made where it's used (best type / colour)".into());
            } else if a.effects.is_empty() && matches!(a.trig, Trig::Static) {
                let hit: Vec<&String> = lits
                    .iter()
                    .filter(|l| a.text.contains(l.as_str()))
                    .collect();
                if let Some(l) = hit.first() {
                    how = "approx";
                    notes.push(format!(
                        "no parsed effect; simulator reads the text (\"{}\")",
                        l
                    ));
                } else if !special.is_empty() {
                    how = "approx";
                    notes.push(format!(
                        "no parsed effect; compiled into {}",
                        special.join(", ")
                    ));
                } else {
                    how = "none";
                    notes.push("no parsed effect and nothing in the simulator reads it".into());
                }
            } else if a.effects.is_empty() {
                how = "none";
                notes.push("no parsed effect".into());
            }
            let parsed_as = a
                .effects
                .iter()
                .map(|e| e.describe())
                .collect::<Vec<_>>()
                .join("; ");
            rows.push((
                format!("{}", k + 1),
                trig,
                a.text.clone(),
                parsed_as,
                how,
                notes.join(" | "),
            ));
        }
        for kw in &parsed.keywords {
            let played = KEYWORDS_PLAYED.iter().any(|w| {
                kw == *w
                    || kw.starts_with(&format!("{} ", w))
                    || kw.starts_with(&format!("{}—", w))
                    || kw.starts_with(&format!("{} {{", w))
            }) || kw.contains("cycling");
            let how = if !played {
                "none"
            } else if kw.starts_with("sneak") {
                "approx"
            } else {
                "exact"
            };
            rows.push((
                "kw".into(),
                "keyword".into(),
                kw.clone(),
                kw.clone(),
                how,
                String::new(),
            ));
        }
        for u in &parsed.unparsed {
            rows.push((
                "?".into(),
                "unparsed".into(),
                u.clone(),
                "(not understood)".into(),
                "none",
                "the parser skips this text".into(),
            ));
        }
        let status = rows
            .iter()
            .map(|r| r.4)
            .max_by_key(|h| rank(h))
            .unwrap_or("exact");
        let g = group
            .get(&card.name)
            .map(String::as_str)
            .unwrap_or("e_other");
        *totals.entry((g.to_string(), status)).or_default() += 1;
        *totals.entry(("all".to_string(), status)).or_default() += 1;
        for r in rows {
            let _ = writeln!(
                tsv,
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                card.name,
                card.set,
                g,
                status,
                r.0,
                r.1,
                r.2.replace(['\t', '\n'], " "),
                r.3.replace(['\t', '\n'], " "),
                r.4,
                r.5.replace(['\t', '\n'], " ")
            );
        }
    }
    std::fs::write(path, tsv).expect("write tsv");
    println!("\n== Cards by worst handling (TSV: {})", path);
    for ((g, h), n) in totals {
        println!("  {:<14} {:<7} {}", g, h, n);
    }
}
