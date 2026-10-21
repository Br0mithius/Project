//! Prints each card's rules text next to how the app reads it, for reviewing many cards:
//! `cargo run --release --example review_cards -- <data dir> <file with one card name per line>`
use msh_deckbuilder::db::CardDb;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(&args[1]);
    let db = CardDb::load(&dir.join("cards.json")).unwrap();
    let names = std::fs::read_to_string(&args[2]).unwrap();
    for name in names.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let Some(c) = db.get(name) else {
            println!("### {} - NOT IN DATABASE\n", name);
            continue;
        };
        println!(
            "### {} | {} | {} | {}",
            c.name,
            c.mana_cost,
            c.type_line,
            c.power_toughness().unwrap_or_default()
        );
        println!("TEXT: {}", c.oracle_text.replace('\n', " / "));
        let p = c.parsed();
        if !p.keywords.is_empty() {
            println!("  keywords: {}", p.keywords.join(", "));
        }
        let show = |p: &msh_deckbuilder::abilities::Parsed, tag: &str| {
            for a in &p.abilities {
                let mut flags = Vec::new();
                if a.optional {
                    flags.push("may");
                }
                if a.conditional {
                    flags.push("if");
                }
                if a.once {
                    flags.push("once");
                }
                if a.modal {
                    flags.push("modal");
                }
                println!(
                    "  {}{:?} [{}] -> {:?}",
                    tag,
                    a.when,
                    flags.join(","),
                    a.effects
                );
            }
        };
        show(p, "");
        if let Some(b) = &p.back {
            show(b, "back: ");
        }
        let roles: Vec<&str> = c.roles().iter().map(|r| r.label()).collect();
        print!("  roles: {}", roles.join(", "));
        if let Some(r) = p.removal() {
            print!(" | removal {:.1} ({})", r.points, r.label);
        }
        if c.is_creature() {
            let v = msh_deckbuilder::value::creature_value(c);
            print!(" | value {:.0}% of fair", v.efficiency * 100.0);
        }
        println!("\n");
    }
}
