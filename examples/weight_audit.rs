//! Checks how trustworthy each commander-ranking part is on a real pool:
//! `cargo run --release --example weight_audit -- <data dir>`
//!
//! For every part it prints the spread between options, the noise when only the random
//! parts change (other shuffles, other simulated opponent pools), the resulting
//! reliability, and how strongly the parts agree with each other (rank correlation), so
//! parts that measure the same thing aren't counted twice.
use msh_deckbuilder::{analysis, db::*, playtest, ranking};

fn sd(v: &[f32]) -> f32 {
    let n = v.len().max(1) as f32;
    let m = v.iter().sum::<f32>() / n;
    (v.iter().map(|x| (x - m).powi(2)).sum::<f32>() / n).sqrt()
}

fn ranks(v: &[f32]) -> Vec<f32> {
    let mut idx: Vec<usize> = (0..v.len()).collect();
    idx.sort_by(|&a, &b| v[a].partial_cmp(&v[b]).unwrap());
    let mut r = vec![0.0; v.len()];
    for (pos, i) in idx.into_iter().enumerate() {
        r[i] = pos as f32;
    }
    r
}

fn spearman(a: &[f32], b: &[f32]) -> f32 {
    let (ra, rb) = (ranks(a), ranks(b));
    let n = ra.len() as f32;
    let ma = ra.iter().sum::<f32>() / n;
    let mb = rb.iter().sum::<f32>() / n;
    let cov: f32 = ra.iter().zip(&rb).map(|(x, y)| (x - ma) * (y - mb)).sum();
    cov / (sd(&ra) * sd(&rb) * n).max(1e-6)
}

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("data dir"));
    let mut db = CardDb::load(&dir.join("cards.json")).unwrap();
    db.attach_ratings(&dir);
    let coll = Collection::load(&dir.join("collection.txt")).unwrap();
    let rules = Rules::default();
    let bed = ranking::TestBed::new(&db, &rules);
    // Opponents from pools the ranking doesn't use.
    let other_pools: Vec<_> = [777, 888]
        .into_iter()
        .flat_map(|seed| playtest::gauntlet(&db, &rules, seed))
        .collect();
    let options = ranking::candidates(&db, &coll, &rules);
    let k = ranking::PARTS;
    let mut raw: Vec<Vec<f32>> = vec![Vec::new(); k];
    let mut noise: Vec<Vec<f32>> = vec![Vec::new(); k];
    for (name, piper) in &options {
        let cmd = db.get(name).unwrap();
        let e = ranking::evaluate(cmd, *piper, &db, &coll, &rules, &bed);
        let v = e.parts.values();
        for p in 0..k {
            raw[p].push(v[p]);
        }
        // Mana: the same deck, other shuffles.
        let mana: Vec<f32> = (0..4)
            .map(|_| {
                let s = analysis::simulate(&e.deck, &db, 2500);
                let on = s.commander_on_curve.map(|(_, p)| p).unwrap_or(0.6);
                (s.keepable - 0.85) * 6.0 + (on - 0.6) * 2.0
            })
            .collect();
        noise[4].push(sd(&mana));
        // Playtest: other shuffles, and opponents from other simulated pools.
        let mut play = vec![v[6]];
        for seed in [8, 9] {
            play.push(playtest::win_rate(
                &e.deck,
                &bed.gauntlet,
                &db,
                &rules,
                1000,
                seed,
            ));
        }
        play.push(playtest::win_rate(
            &e.deck,
            &other_pools,
            &db,
            &rules,
            1000,
            7,
        ));
        noise[6].push(sd(&play));
        eprint!(".");
    }
    eprintln!();
    println!("{} options", options.len());
    println!(
        "{:<12} {:>9} {:>9} {:>11}  {}",
        "part",
        "spread",
        "noise",
        "reliability",
        "(spread = SD between options; noise = SD from randomness alone)"
    );
    for p in 0..k {
        let known: Vec<f32> = raw[p].iter().copied().filter(|x| x.is_finite()).collect();
        let between = sd(&known);
        let n = if noise[p].is_empty() {
            0.0
        } else {
            noise[p].iter().sum::<f32>() / noise[p].len() as f32
        };
        let rel = if between > 0.0 {
            (1.0 - (n / between).powi(2)).max(0.0)
        } else {
            0.0
        };
        let distinct = {
            let mut d: Vec<i64> = known.iter().map(|x| (x * 100.0) as i64).collect();
            d.sort();
            d.dedup();
            d.len()
        };
        println!(
            "{:<12} {:>9.3} {:>9.3} {:>11.2}  {} distinct values, {} with data",
            ranking::Parts::LABELS[p],
            between,
            n,
            rel,
            distinct,
            known.len()
        );
    }
    println!("\nRank correlation between parts (1 = same order, 0 = unrelated):");
    print!("{:<12}", "");
    for l in ranking::Parts::LABELS {
        print!(" {:>6.6}", l);
    }
    println!();
    for a in 0..k {
        print!("{:<12}", ranking::Parts::LABELS[a]);
        for b in 0..k {
            let pairs: Vec<(f32, f32)> = raw[a]
                .iter()
                .zip(&raw[b])
                .filter(|(x, y)| x.is_finite() && y.is_finite())
                .map(|(x, y)| (*x, *y))
                .collect();
            let (x, y): (Vec<f32>, Vec<f32>) = pairs.into_iter().unzip();
            if x.len() < 3 {
                print!(" {:>6}", "-");
            } else {
                print!(" {:>+6.2}", spearman(&x, &y));
            }
        }
        println!();
    }
}
