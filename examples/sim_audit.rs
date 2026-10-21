//! Writes full logs of simulated games for one commander's deck against the simulated
//! league opponents, for auditing the simulator's rules and AI:
//! `cargo run --release --example sim_audit -- <data dir> "<commander>" <colours> <out dir> [games]`.
use msh_deckbuilder::analysis::{self, Rng};
use msh_deckbuilder::card::ColorSet;
use msh_deckbuilder::db::*;
use msh_deckbuilder::playtest::{self, Prepared};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(&args[1]);
    let out = std::path::PathBuf::from(&args[4]);
    let games: u64 = args.get(5).and_then(|g| g.parse().ok()).unwrap_or(20);
    let mut db = CardDb::load(&dir.join("cards.json")).unwrap();
    db.attach_ratings(&dir);
    let coll = Collection::load(&dir.join("collection.txt")).unwrap();
    let mut rules = Rules::default();
    rules.data_dir = Some(dir.clone());
    let cmd = db.get(&args[2]).expect("commander").clone();
    let colors = ColorSet::parse(&args[3]).expect("colours");
    let (deck, _) = analysis::autobuild(&cmd, &db, &coll, &rules, Some(colors), None);
    let me = Prepared::new(&deck, &db);
    let gauntlet = playtest::gauntlet(&db, &rules, 2026);
    std::fs::create_dir_all(&out).unwrap();
    let mut decks = String::new();
    for (k, d) in std::iter::once(&me).chain(gauntlet.iter()).enumerate() {
        decks += &format!("== deck {} ==\n", k);
        for c in &d.cards {
            decks += &format!("{}\n", c.name);
        }
    }
    std::fs::write(out.join("decks.txt"), decks).unwrap();
    let n = gauntlet.len();
    for k in 0..games {
        let mut table: Vec<&Prepared> = (0..3).map(|i| &gauntlet[(k as usize + i) % n]).collect();
        let seat = k as usize % 4;
        table.insert(seat, &me);
        let mut g = playtest::state::Game::new(&table, &rules, Rng::new(1000 + k));
        g.log = Some(Vec::new());
        let winner = playtest::play(&mut g);
        let mut text = format!(
            "GAME {} | our deck ({}) in seat {} | opponents: decks {:?}\n",
            k,
            cmd.name,
            seat,
            (0..3).map(|i| 1 + (k as usize + i) % n).collect::<Vec<_>>()
        );
        for (p, d) in table.iter().enumerate() {
            let c = d.commanders_names().join(" + ");
            text += &format!("seat {}: commander {}\n", p, c);
        }
        for line in g.log.take().unwrap_or_default() {
            text += &line;
            text.push('\n');
        }
        text += &format!(
            "RESULT: winner seat {} after {} rounds; life {:?}; poison {:?}\n",
            winner,
            g.round,
            g.players.iter().map(|p| p.life).collect::<Vec<_>>(),
            g.players.iter().map(|p| p.poison).collect::<Vec<_>>()
        );
        std::fs::write(out.join(format!("game_{:02}.txt", k)), text).unwrap();
    }
}
