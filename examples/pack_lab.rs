//! Tests the colour packs (`colorpacks`): for each two-colour pair, commanders of exactly
//! those colours are built two ways from every league card - the builder's own auto-build
//! and the pair's pack - and both play the same games against the peer field.
//! `cargo run --release --example pack_lab -- <data dir> [games] [commanders per pair]`
use msh_deckbuilder::colorpacks;
use msh_deckbuilder::db::{CardDb, Collection, Rules};
use msh_deckbuilder::playtest::Prepared;
use msh_deckbuilder::playtest::lab::play_all;
use std::sync::atomic::{AtomicBool, AtomicUsize};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(&a[0]);
    let db = CardDb::load(&dir.join("cards.json")).expect("cards.json");
    let games: u32 = a.get(1).and_then(|g| g.parse().ok()).unwrap_or(800);
    let per: usize = a.get(2).and_then(|g| g.parse().ok()).unwrap_or(3);
    let rules = Rules::default();
    let all = Collection::new(
        std::path::PathBuf::new(),
        db.cards.iter().map(|c| (c.name.clone(), 1)).collect(),
    );
    let field = msh_deckbuilder::table::peer_field(&db, &rules);
    for pack in colorpacks::PACKS {
        let cmds: Vec<_> = db
            .cards
            .iter()
            .filter(|c| c.can_be_commander() && c.identity() == pack.color_set())
            .take(per)
            .collect();
        let mut sum = 0.0;
        let mut n = 0;
        println!("{} ({})", pack.name, pack.colors);
        for cmd in cmds {
            let Ok(base) = msh_deckbuilder::table::strong_build(&cmd.name, &db, &rules) else {
                continue;
            };
            let built = colorpacks::build(pack, cmd, &db, &all, &rules);
            if built.total_count(&rules) != base.total_count(&rules) {
                continue;
            }
            let (pb, pp) = (Prepared::new(&base, &db), Prepared::new(&built, &db));
            let o = play_all(
                &[&pb, &pp],
                &field,
                &rules,
                games,
                0x9AC4,
                &AtomicUsize::new(0),
                &AtomicBool::new(false),
            );
            let d = o.diff(1, 0);
            println!(
                "  {:+5.1} ± {:.1}  {} (auto-build {:.1}%, pack {:.1}%)",
                d.gain * 100.0,
                d.stderr * 100.0,
                cmd.name,
                o.win_rate(0) * 100.0,
                o.win_rate(1) * 100.0
            );
            sum += d.gain;
            n += 1;
        }
        if n > 0 {
            println!(
                "  average {:+.1} pts over {} commanders",
                sum / n as f32 * 100.0,
                n
            );
        }
    }
}
