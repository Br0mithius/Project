//! Reports how much card text the ability parser understands:
//! `cargo run --release --example parse_report -- data [--all]`.
use msh_deckbuilder::{abilities, db::CardDb};

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "data".into()));
    let all = std::env::args().any(|a| a == "--all");
    let db = CardDb::load(&dir.join("cards.json")).unwrap();
    // `--card "Name"` (repeatable) prints how those cards were read.
    let args: Vec<String> = std::env::args().collect();
    let names: Vec<&String> = args
        .windows(2)
        .filter(|w| w[0] == "--card")
        .map(|w| &w[1])
        .collect();
    // `--roles` shows where the parser's roles differ from the old text guesses.
    if std::env::args().any(|a| a == "--roles") {
        let mut diff: std::collections::BTreeMap<String, (Vec<String>, Vec<String>)> =
            Default::default();
        for c in &db.cards {
            let (new, old) = (c.roles(), c.legacy_roles());
            for r in &new {
                if !old.contains(r) {
                    diff.entry(format!("{:?}", r))
                        .or_default()
                        .0
                        .push(c.name.clone());
                }
            }
            for r in &old {
                if !new.contains(r) {
                    diff.entry(format!("{:?}", r))
                        .or_default()
                        .1
                        .push(c.name.clone());
                }
            }
        }
        for (role, (added, dropped)) in diff {
            println!(
                "{}: +{} {:?}",
                role,
                added.len(),
                added.iter().take(12).collect::<Vec<_>>()
            );
            println!(
                "{}: -{} {:?}",
                role,
                dropped.len(),
                dropped.iter().take(12).collect::<Vec<_>>()
            );
        }
        return;
    }
    // `--synergy "Commander"` lists the cards with the most synergy for that commander.
    if let Some(i) = args.iter().position(|a| a == "--synergy") {
        let cmd = db.get(&args[i + 1]).expect("commander");
        let mut rows: Vec<(f32, String)> = db
            .cards
            .iter()
            .filter(|c| c.identity().is_subset_of(cmd.identity()) && !c.is_land())
            .map(|c| {
                let (s, why) = msh_deckbuilder::analysis::synergy(cmd, c);
                (s, format!("{:>4.1}  {:<34} {}", s, c.name, why.join("; ")))
            })
            .filter(|r| r.0 > 0.0)
            .collect();
        rows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        println!("{} cards with synergy for {}", rows.len(), cmd.name);
        for (_, r) in rows.iter().take(14) {
            println!("{}", r);
        }
        return;
    }
    // `--removal` lists every card's removal grade.
    if std::env::args().any(|a| a == "--removal") {
        let mut rows: Vec<(f32, String)> = db
            .cards
            .iter()
            .filter_map(|c| {
                let p = abilities::parse(c);
                p.removal().map(|r| {
                    (
                        r.points,
                        format!(
                            "{:>4.1}  {:<30} {:<36} {}{}{}",
                            r.points,
                            r.label,
                            c.name,
                            if r.mass { "MASS " } else { "" },
                            if r.repeatable { "REPEAT " } else { "" },
                            if p.sweeper() { "SWEEPER" } else { "" }
                        ),
                    )
                })
            })
            .collect();
        rows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        for (_, r) in rows {
            println!("{}", r);
        }
        return;
    }
    if !names.is_empty() {
        for n in names {
            let Some(c) = db.get(n) else {
                println!("?? {}", n);
                continue;
            };
            let p = abilities::parse(c);
            println!("== {} [{}] {}", c.name, c.type_line, c.mana_cost);
            if !p.keywords.is_empty() {
                println!("   keywords: {}", p.keywords.join(", "));
            }
            for a in &p.abilities {
                let flags = [
                    (a.optional, "may"),
                    (a.conditional, "if"),
                    (a.once, "once"),
                    (a.modal, "modal"),
                ]
                .iter()
                .filter(|f| f.0)
                .map(|f| f.1)
                .collect::<Vec<_>>()
                .join(",");
                println!("   {:?} [{}] rate {:.2}", a.when, flags, a.rate());
                for e in &a.effects {
                    println!("      {:?}", e);
                }
            }
            for u in &p.unparsed {
                println!("   UNREAD: {}", u);
            }
            if let Some(r) = p.removal() {
                println!(
                    "   => removal {:.1} ({}){}{}",
                    r.points,
                    r.label,
                    if r.mass { " mass" } else { "" },
                    if r.repeatable { " repeatable" } else { "" }
                );
            }
            if let Some(b) = &p.back {
                println!(
                    "   back face: {} abilities {:?}",
                    b.abilities.len(),
                    b.abilities.iter().map(|a| &a.effects).collect::<Vec<_>>()
                );
            }
        }
        return;
    }
    let (mut lines, mut missed) = (0, 0);
    let mut by_set: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
    for c in &db.cards {
        let p = abilities::parse(c);
        let n = p.abilities.len() + p.unparsed.len() + usize::from(!p.keywords.is_empty());
        lines += n;
        missed += p.unparsed.len();
        let e = by_set.entry(c.set.clone()).or_default();
        e.0 += n;
        e.1 += p.unparsed.len();
        if all || !p.unparsed.is_empty() {
            for u in &p.unparsed {
                println!("[{}] {}: {}", c.set, c.name, u);
            }
        }
    }
    for (set, (n, m)) in by_set {
        println!(
            "{:<7} {:>5} lines, {:>4} not understood ({:.1}% understood)",
            set,
            n,
            m,
            100.0 * (1.0 - m as f32 / n.max(1) as f32)
        );
    }
    println!(
        "TOTAL   {:>5} lines, {:>4} not understood ({:.1}% understood)",
        lines,
        missed,
        100.0 * (1.0 - missed as f32 / lines as f32)
    );
}
