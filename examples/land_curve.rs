//! How the simulator values land counts: for each deck, win rate at -3..+3 lands around its
//! own count, on paired games against the simulated league. Three of the deck's mid-cost
//! spells are stood in by a plain 2/2 for {2} (the baseline, "0"); "+k" turns k of those
//! stand-ins into the deck's most-played basic, "-k" turns k basics into more stand-ins. So
//! every step compares one land against one plain cheap creature.
//!
//! `cargo run --release --example land_curve -- <data dir> <games> [deck file]...`
//! (no deck files: every simulated league deck).

use msh_deckbuilder::db::CardDb;
use msh_deckbuilder::deck::Deck;
use msh_deckbuilder::playtest::compile::{T_LAND, compile};
use msh_deckbuilder::playtest::simvalues::vanilla;
use msh_deckbuilder::playtest::{Prepared, lab::play_all};
use std::sync::atomic::{AtomicBool, AtomicUsize};

const BASICS: [&str; 5] = ["Plains", "Island", "Swamp", "Mountain", "Forest"];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(&args[0]);
    let games: u32 = args[1].parse().expect("games");
    let mut db = CardDb::load(&dir.join("cards.json")).unwrap();
    db.attach_ratings(&dir);
    let rules = msh_deckbuilder::league::app_rules(&dir, None);
    let gauntlet = msh_deckbuilder::ranking::TestBed::new(&db, &rules).gauntlet;
    let mut decks: Vec<(String, Prepared, Vec<usize>)> = Vec::new();
    if args.len() > 2 {
        for f in &args[2..] {
            let d = Deck::load(std::path::Path::new(f), &db).expect("deck");
            decks.push((
                f.rsplit('/').next().unwrap().to_string(),
                Prepared::new(&d, &db),
                (0..gauntlet.len()).collect(),
            ));
        }
    } else {
        for (i, g) in gauntlet.iter().enumerate() {
            let others = (0..gauntlet.len()).filter(|j| *j != i).collect();
            decks.push((g.commanders_names().join(" + "), g.with_swaps(&[]), others));
        }
    }
    let filler = compile(&vanilla(2));
    let mut total = [0.0f64; 7];
    let mut total_var = [0.0f64; 7];
    println!(
        "{:<34} lands  avgMV   -3     -2     -1      0     +1     +2     +3   (win-rate pts vs 0; ± about the last column)",
        "deck"
    );
    for (name, p, opp_idx) in &decks {
        let opp: Vec<Prepared> = opp_idx
            .iter()
            .map(|&j| gauntlet[j].with_swaps(&[]))
            .collect();
        // The deck's spells by distance from 3 mana, and its most-played basic.
        let mut spells: Vec<u16> = Vec::new();
        let (mut lands, mut mv_sum, mut n_spells) = (0, 0u32, 0u32);
        for i in 0..p.library_len() {
            let idx = p.library_card(i);
            let c = &p.cards[idx as usize];
            if c.is(T_LAND) {
                lands += 1;
            } else {
                mv_sum += c.mv as u32;
                n_spells += 1;
                if !spells.contains(&idx) {
                    spells.push(idx);
                }
            }
        }
        spells.sort_by_key(|&i| (p.cards[i as usize].mv as i32 - 3).abs());
        let basic = BASICS
            .iter()
            .filter_map(|b| p.card_index(b).map(|i| (p.copies(i), i, *b)))
            .max();
        let Some((copies, bidx, bname)) = basic else {
            println!("{:<34} no basics, skipped", name);
            continue;
        };
        if spells.len() < 3 || copies < 3 {
            println!("{:<34} too few spells or basics, skipped", name);
            continue;
        }
        let land = compile(db.get(bname).unwrap());
        let s = [spells[0], spells[1], spells[2]];
        // Variant k (index k+3): the three stand-in slots hold max(k,0) basics; -k basics become stand-ins.
        let variants: Vec<Prepared> = (-3i32..=3)
            .map(|k| {
                let mut sw = Vec::new();
                for (j, &slot) in s.iter().enumerate() {
                    sw.push((
                        slot,
                        if (j as i32) < k {
                            land.clone()
                        } else {
                            filler.clone()
                        },
                    ));
                }
                for _ in 0..(-k).max(0) {
                    sw.push((bidx, filler.clone()));
                }
                p.with_swaps(&sw)
            })
            .collect();
        let refs: Vec<&Prepared> = variants.iter().collect();
        let o = play_all(
            &refs,
            &opp,
            &rules,
            games,
            0x1A2D,
            &AtomicUsize::new(0),
            &AtomicBool::new(false),
        );
        let mut line = format!(
            "{:<34} {:>4}  {:>5.2}",
            &name[..name.len().min(34)],
            lands,
            mv_sum as f32 / n_spells.max(1) as f32
        );
        let mut se = 0.0;
        for k in 0..7 {
            let d = o.diff(k, 3);
            line += &format!(" {:>+6.1}", d.gain * 100.0);
            total[k] += d.gain as f64 * 100.0;
            total_var[k] += (d.stderr as f64 * 100.0).powi(2);
            se = d.stderr * 100.0;
        }
        println!("{}  ±{:.1}", line, se);
    }
    let n = decks.len().max(1) as f64;
    let mut line = format!("{:<34} {:>4}  {:>5}", "AVERAGE", "", "");
    for k in 0..7 {
        line += &format!(" {:>+6.1}", total[k] / n);
    }
    println!("{}  ±{:.1}", line, total_var[6].sqrt() / n);
}
