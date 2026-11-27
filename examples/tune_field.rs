//! Tunes opponent decks with the deck optimiser, from every league card (one copy each),
//! against the peer field plus any reference decks - so stand-in opponents are as well
//! built as the user's own deck.
//!
//! `cargo run --release --example tune_field -- <data dir> <out dir> [--rounds N]
//! [--ref <deck file>]... (--table | <deck file>...)`
//! `--table`: tunes every opponent in `<data dir>/table.json` with their trade picks and
//! seen cards kept in (`--free`: in to start with, but the tuning may cut them), writes the tuned lists back to table.json (and to the out dir).
//! Otherwise each deck file is tuned and written to the out dir under the same name.
use msh_deckbuilder::db::{CardDb, Collection, Rules};
use msh_deckbuilder::deck::Deck;
use msh_deckbuilder::lab::{Effort, Progress};
use msh_deckbuilder::optimize::{Opponents, optimize};
use msh_deckbuilder::playtest::Prepared;
use msh_deckbuilder::table::Table;
use std::path::{Path, PathBuf};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = PathBuf::from(&a[0]);
    let out = PathBuf::from(&a[1]);
    let _ = std::fs::create_dir_all(&out);
    let mut db = CardDb::load(&dir.join("cards.json")).expect("cards.json");
    // The measured card values and ratings, as the app loads them.
    db.attach_ratings(&dir);
    let mut rounds = 3usize;
    let mut refs: Vec<Deck> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    let mut table_mode = false;
    // `--free`: trade picks and seen cards start in the deck but may be tuned out (a
    // player doesn't play every card they picked up).
    let mut free = false;
    // `--only <player>`: just that opponent; with `--commander <name>` they changed
    // commander: their deck is rebuilt around it (strong build, picks in) before tuning.
    let mut only: Option<String> = None;
    let mut new_cmd: Option<String> = None;
    // `--archetype <name>` (with `--only`): the deck is rebuilt as that archetype ("Voltron"
    // for an equipment deck) and its theme cards (Equipment and Auras) are kept in.
    let mut archetype: Option<String> = None;
    // `--rebuild`: the deck is rebuilt from the pool first (its commander's own archetype
    // unless `--archetype`). `--exclude-set FRA`: the pool leaves that set out (cards
    // nobody has yet).
    let mut rebuild = false;
    let mut exclude: Option<String> = None;
    let mut i = 2;
    while i < a.len() {
        match a[i].as_str() {
            "--rounds" => {
                rounds = a[i + 1].parse().expect("rounds");
                i += 1;
            }
            "--ref" => {
                refs.push(Deck::load(Path::new(&a[i + 1]), &db).expect("reference deck"));
                i += 1;
            }
            "--table" => table_mode = true,
            "--free" => free = true,
            "--only" => {
                only = Some(a[i + 1].clone());
                i += 1;
            }
            "--rebuild" => rebuild = true,
            "--exclude-set" => {
                exclude = Some(a[i + 1].to_uppercase());
                i += 1;
            }
            "--archetype" => {
                archetype = Some(a[i + 1].clone());
                i += 1;
            }
            "--commander" => {
                new_cmd = Some(a[i + 1].clone());
                i += 1;
            }
            f => files.push(f.to_string()),
        }
        i += 1;
    }
    let all = Collection::new(
        PathBuf::new(),
        db.cards
            .iter()
            .filter(|c| {
                exclude
                    .as_ref()
                    .is_none_or(|x| !c.set.eq_ignore_ascii_case(x))
            })
            .map(|c| (c.name.clone(), 1))
            .collect(),
    );
    let field = |skip: &str| -> Vec<Prepared> {
        msh_deckbuilder::table::peer_decks(&db)
            .iter()
            .filter(|d| d.commander.as_deref() != Some(skip))
            .chain(refs.iter())
            .map(|d| Prepared::new(d, &db))
            .collect()
    };
    let tune = |deck: &Deck, keep: Vec<String>| -> Deck {
        let cmd = db
            .get(deck.commander.as_deref().expect("commander"))
            .expect("commander card");
        let rules = Rules {
            lab_keep: keep,
            ..Rules::default()
        };
        let opp = Opponents {
            table: Vec::new(),
            league: field(&cmd.name),
            table_weight: 0.0,
        };
        let r = optimize(
            cmd,
            deck,
            &db,
            &all,
            &rules,
            &opp,
            Effort::QUICK,
            rounds,
            &Progress::new(),
        )
        .expect("not cancelled");
        let gain = r.final_gain.as_ref().map(|g| g.score);
        let better = gain.as_ref().is_some_and(|g| g.gain > 0.0 && g.z() >= 1.0);
        println!(
            "{}: {} ({} steps, {:.0} s){}",
            cmd.name,
            match &gain {
                Some(g) => format!("{:+.1} ± {:.1} pts", g.gain * 100.0, g.stderr * 100.0),
                None => "no change".to_string(),
            },
            r.steps.len(),
            r.seconds,
            if better { "" } else { " - kept as it was" }
        );
        if better { r.best } else { deck.clone() }
    };
    if table_mode {
        let mut table = Table::load(&dir);
        table.load_seen(&dir);
        for k in 0..table.opponents.len() {
            if only
                .as_ref()
                .is_some_and(|w| !w.eq_ignore_ascii_case(&table.opponents[k].player))
            {
                continue;
            }
            if rebuild || new_cmd.is_some() || archetype.is_some() {
                let c = new_cmd
                    .clone()
                    .unwrap_or_else(|| table.opponents[k].commander.clone());
                let d = match &archetype {
                    Some(q) => {
                        let arch = msh_deckbuilder::archetypes::find(q)
                            .into_iter()
                            .next()
                            .expect("archetype");
                        let cmd = db.get(&c).expect("commander");
                        let mut d = msh_deckbuilder::archetypes::autobuild_as(
                            arch,
                            cmd,
                            &db,
                            &all,
                            &Rules::default(),
                        )
                        .0;
                        d.commander = Some(cmd.name.clone());
                        d
                    }
                    None => {
                        let cmd = db.get(&c).expect("commander");
                        let mut d = msh_deckbuilder::archetypes::autobuild(
                            cmd,
                            &db,
                            &all,
                            &Rules::default(),
                            None,
                            None,
                        )
                        .0;
                        d.commander = Some(cmd.name.clone());
                        d
                    }
                };
                table.opponents[k].commander = d.commander.clone().unwrap_or_default();
                table.opponents[k].decklist = d.to_text();
            }
            let o = table.opponents[k].clone();
            let Ok(deck) = table.deck_of(&o, &db) else {
                continue;
            };
            let mut keep: Vec<String> = table
                .picks_of(&o.player)
                .iter()
                .map(|p| p.card.clone())
                .collect();
            keep.extend(
                table
                    .seen
                    .get(&o.player.trim().to_lowercase())
                    .cloned()
                    .unwrap_or_default(),
            );
            // An equipment deck plays its Equipment: at least eight pieces, the best ones
            // in place of the weakest other spells (never a trade pick).
            let mut deck = deck;
            if archetype
                .as_ref()
                .is_some_and(|q| q.eq_ignore_ascii_case("voltron"))
            {
                let is_gear =
                    |n: &str| db.get(n).is_some_and(|c| c.type_line.contains("Equipment"));
                let rules = msh_deckbuilder::league::app_rules(&dir, None);
                while deck.cards.keys().filter(|n| is_gear(n)).count() < 8 {
                    let Some(add) =
                        msh_deckbuilder::analysis::suggestions(&deck, &db, &all, &rules)
                            .into_iter()
                            .find(|s| is_gear(&s.card.name))
                            .map(|s| s.card.name.clone())
                    else {
                        break;
                    };
                    let Some(cut) = msh_deckbuilder::analysis::weakest(&deck, &db, &rules)
                        .into_iter()
                        .find(|w| !is_gear(&w.card.name) && !keep.contains(&w.card.name))
                        .map(|w| w.card.name.clone())
                    else {
                        break;
                    };
                    deck.remove(&cut, 1);
                    deck.add(&add, 1);
                }
            }
            let mut locked = if free { Vec::new() } else { keep.clone() };
            if archetype.is_some() {
                locked.extend(
                    deck.cards
                        .keys()
                        .filter(|n| {
                            db.get(n).is_some_and(|c| {
                                c.type_line.contains("Equipment") || c.type_line.contains("Aura")
                            })
                        })
                        .cloned(),
                );
            }
            let tuned = tune(&deck, locked);
            let picks_in = keep.iter().filter(|k| tuned.count_of(k) > 0).count();
            println!(
                "  {}: {} of {} picks and seen cards in the deck",
                o.player,
                picks_in,
                keep.len()
            );
            let _ = std::fs::write(out.join(format!("{}.txt", o.player)), tuned.to_text());
            table.opponents[k].decklist = tuned.to_text();
        }
        table.save(&dir).expect("save table.json");
    }
    for f in &files {
        let deck = Deck::load(Path::new(f), &db).expect("deck file");
        let tuned = tune(&deck, Vec::new());
        let name = Path::new(f).file_name().expect("file name");
        let _ = std::fs::write(out.join(name), tuned.to_text());
    }
}
