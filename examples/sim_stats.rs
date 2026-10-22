//! Statistics over many simulated games, for auditing the simulator and its AI:
//! `cargo run --release --example sim_stats -- <data dir> "<commander>" <colours> [games] [--gauntlet] [--check]`.
//! Plays the commander's deck against the simulated league opponents (rotating seats), or
//! with `--gauntlet` tables of league opponents only (commander and colours are then ignored;
//! pass "-"). `--check` replays every game without instrumentation and stops if anything
//! differs (instrumentation must not change the game).
use msh_deckbuilder::analysis::{self, Rng};
use msh_deckbuilder::card::ColorSet;
use msh_deckbuilder::db::*;
use msh_deckbuilder::playtest::audit::{EndCause, Stats};
use msh_deckbuilder::playtest::compile::T_LAND;
use msh_deckbuilder::playtest::{self, MAX_ROUNDS, Prepared};
use std::collections::BTreeMap;

/// One finished game.
struct Game {
    /// Deck id per seat (0 = ours, k = league deck k).
    decks: Vec<usize>,
    stats: Stats,
}

fn pct(a: f64, b: f64) -> String {
    if b == 0.0 {
        "  -  ".into()
    } else {
        format!("{:5.1}%", 100.0 * a / b)
    }
}

fn avg(a: f64, b: f64) -> String {
    if b == 0.0 {
        "  -  ".into()
    } else {
        format!("{:5.2}", a / b)
    }
}

fn main() {
    let all: Vec<String> = std::env::args().collect();
    let gauntlet_only = all.iter().any(|a| a == "--gauntlet");
    let check = all.iter().any(|a| a == "--check");
    let args: Vec<&String> = all.iter().filter(|a| !a.starts_with("--")).collect();
    let dir = std::path::PathBuf::from(args[1]);
    let games: u64 = args.get(4).and_then(|g| g.parse().ok()).unwrap_or(1000);
    let mut db = CardDb::load(&dir.join("cards.json")).unwrap();
    db.attach_ratings(&dir);
    let coll = Collection::load(&dir.join("collection.txt")).unwrap();
    let mut rules = Rules::default();
    rules.data_dir = Some(dir.clone());
    let me = (!gauntlet_only).then(|| {
        let cmd = db.get(args[2]).expect("commander").clone();
        let colors = ColorSet::parse(args[3]).expect("colours");
        let (deck, _) = analysis::autobuild(&cmd, &db, &coll, &rules, Some(colors), None);
        Prepared::new(&deck, &db)
    });
    let gauntlet = playtest::gauntlet(&db, &rules, 2026);
    let n = gauntlet.len();
    let deck_of = |d: usize| -> &Prepared {
        if d == 0 {
            me.as_ref().unwrap()
        } else {
            &gauntlet[d - 1]
        }
    };
    let label = |d: usize| -> String {
        let p = deck_of(d);
        let who = if d == 0 {
            "OURS".to_string()
        } else {
            format!("L{}", d)
        };
        format!("{} {}", who, p.commanders_names().join(" + "))
    };
    // ---------------------------------------------------------------- play
    let mut played: Vec<Game> = Vec::new();
    for k in 0..games {
        // Opponents and their seats are drawn at random for each game (seeded): fixed
        // neighbours would confound the per-deck rates. Our seat rotates.
        let mut picks: Vec<usize> = (1..=n).collect();
        Rng::new(0x5EA7_0000 + k).shuffle(&mut picks);
        let decks: Vec<usize> = match &me {
            Some(_) => {
                let mut ids = picks[..3].to_vec();
                ids.insert(k as usize % 4, 0);
                ids
            }
            None => picks[..4].to_vec(),
        };
        let table: Vec<&Prepared> = decks.iter().map(|&d| deck_of(d)).collect();
        let mut g =
            playtest::state::Game::new_audited(&table, &rules, Rng::new(1000 + k), false, true);
        let w = playtest::play(&mut g);
        if check {
            let mut plain = playtest::state::Game::new(&table, &rules, Rng::new(1000 + k));
            let w2 = playtest::play(&mut plain);
            let a: Vec<i32> = g.players.iter().map(|p| p.life).collect();
            let b: Vec<i32> = plain.players.iter().map(|p| p.life).collect();
            assert!(
                w == w2 && g.round == plain.round && a == b,
                "game {} differs with statistics on",
                k
            );
        }
        let stats = *g.stats.take().unwrap();
        played.push(Game { decks, stats });
    }
    let total = played.len() as f64;
    let ours = |d: usize| d == 0;
    // Groups for the per-player sections.
    let groups: Vec<(&str, Box<dyn Fn(usize) -> bool>)> = if me.is_some() {
        vec![
            ("ours", Box::new(ours)),
            ("league", Box::new(move |d: usize| d != 0)),
        ]
    } else {
        vec![("all", Box::new(|_| true))]
    };
    println!(
        "{} games, {}{} | seeds 1000..{}",
        games,
        match &me {
            Some(m) => format!(
                "our deck {} vs the simulated league opponents (rotating seats)",
                m.commanders_names().join(" + ")
            ),
            None => "league opponents only".into(),
        },
        if check {
            " | checked: identical with statistics off"
        } else {
            ""
        },
        1000 + games - 1
    );
    println!();

    // ---------------------------------------------------------------- results
    println!("== WIN RATES ==");
    let mut seat_w = [0f64; 4];
    let mut seat_ours = [(0f64, 0f64); 4];
    for g in &played {
        // (A draw - the last players out together - is a win for nobody.)
        let w = g.stats.winner.unwrap_or(usize::MAX);
        if w < 4 {
            seat_w[w] += 1.0;
        }
        if let Some(s) = g.decks.iter().position(|&d| d == 0) {
            seat_ours[s].1 += 1.0;
            if w == s {
                seat_ours[s].0 += 1.0;
            }
        }
    }
    println!(
        "by seat (any deck): {}",
        (0..4)
            .map(|s| format!("seat {} {}", s, pct(seat_w[s], total)))
            .collect::<Vec<_>>()
            .join(" | ")
    );
    if me.is_some() {
        println!(
            "our deck by seat:   {}",
            (0..4)
                .map(|s| format!("seat {} {}", s, pct(seat_ours[s].0, seat_ours[s].1)))
                .collect::<Vec<_>>()
                .join(" | ")
        );
    }
    let mut by_deck: BTreeMap<usize, (f64, f64)> = BTreeMap::new();
    for g in &played {
        for (s, &d) in g.decks.iter().enumerate() {
            let e = by_deck.entry(d).or_default();
            e.1 += 1.0;
            if g.stats.winner == Some(s) {
                e.0 += 1.0;
            }
        }
    }
    println!("by deck (25% = par at a 4-player table):");
    for (d, (w, n)) in &by_deck {
        println!("  {:<55} {:>5} games  won {}", label(*d), n, pct(*w, *n));
    }
    println!();

    // ---------------------------------------------------------------- length / endings
    println!("== GAME LENGTH AND ENDINGS ==");
    let mut rounds: BTreeMap<u32, f64> = BTreeMap::new();
    let (mut timeouts, mut simult, mut rsum) = (0.0, 0.0, 0.0);
    let mut how: BTreeMap<String, f64> = BTreeMap::new();
    let mut all_elims: BTreeMap<EndCause, f64> = BTreeMap::new();
    let mut nelims = 0.0;
    for g in &played {
        let s = &g.stats;
        *rounds.entry(s.rounds).or_default() += 1.0;
        rsum += s.rounds as f64;
        if s.timeout {
            timeouts += 1.0;
        }
        if s.simultaneous {
            simult += 1.0;
        }
        let end = if s.timeout {
            "round limit (called on standing)".to_string()
        } else {
            s.elims
                .last()
                .map(|e| e.cause.label().to_string())
                .unwrap_or_else(|| "?".into())
        };
        *how.entry(end).or_default() += 1.0;
        for e in &s.elims {
            *all_elims.entry(e.cause).or_default() += 1.0;
            nelims += 1.0;
        }
    }
    println!(
        "rounds: avg {:.1} | hit MAX_ROUNDS ({}) {} | last players out together (a draw: nobody wins) {}",
        rsum / total,
        MAX_ROUNDS,
        pct(timeouts, total),
        pct(simult, total)
    );
    print!("distribution:");
    for (r, c) in &rounds {
        print!(" r{}:{:.1}%", r, 100.0 * c / total);
    }
    println!();
    println!("how games end (the deciding elimination):");
    for (h, c) in &how {
        println!("  {:<40} {}", h, pct(*c, total));
    }
    println!("all eliminations by cause ({} total):", nelims);
    for (c, k) in &all_elims {
        println!("  {:<40} {}", c.label(), pct(*k, nelims));
    }
    println!("  (alternate wins are not modelled: none can occur)");
    println!();

    // ---------------------------------------------------------------- per-player sections
    for (gname, inc) in &groups {
        println!("== PLAYER STATS: {} ==", gname.to_uppercase());
        let pg: Vec<(&Game, usize)> = played
            .iter()
            .flat_map(|g| (0..g.decks.len()).map(move |s| (g, s)))
            .filter(|(g, s)| inc(g.decks[*s]))
            .collect();
        let np = pg.len() as f64;
        // Mulligans.
        let mut mull = 0.0;
        let mut kept: BTreeMap<u8, f64> = BTreeMap::new();
        for (g, s) in &pg {
            let p = &g.stats.players[*s];
            if p.mulligans > 0 {
                mull += 1.0;
            }
            *kept.entry(p.kept).or_default() += 1.0;
        }
        println!(
            "mulligan rate {} | hand kept: {}",
            pct(mull, np),
            kept.iter()
                .map(|(k, c)| format!("{} cards {}", k, pct(*c, np)))
                .collect::<Vec<_>>()
                .join(", ")
        );
        // Per own-turn: lands, mana, spells, attackers.
        println!(
            "own turn:              {}",
            (1..=10).map(|t| format!("{:>6}", t)).collect::<String>()
        );
        let mut rows: Vec<(String, Vec<String>)> = Vec::new();
        let mut land = vec![(0.0, 0.0, 0.0); 10];
        let mut mana = vec![(0.0, 0.0, 0.0); 10];
        let mut spells = vec![(0.0, 0.0); 10];
        let mut att = vec![(0.0, 0.0); 10];
        for (g, s) in &pg {
            for (t, ts) in g.stats.players[*s].turns.iter().enumerate().take(10) {
                land[t].2 += 1.0;
                if ts.land {
                    land[t].0 += 1.0;
                } else if ts.land_in_hand {
                    land[t].1 += 1.0;
                }
                if let Some(a) = ts.mana_avail {
                    mana[t].0 += a as f64;
                    mana[t].1 += ts.mana_spent as f64;
                    mana[t].2 += 1.0;
                }
                spells[t].0 += ts.spells as f64;
                spells[t].1 += 1.0;
                att[t].0 += ts.attackers as f64;
                att[t].1 += 1.0;
            }
        }
        rows.push((
            "land drop made".into(),
            land.iter().map(|l| pct(l.0, l.2)).collect(),
        ));
        rows.push((
            "missed, land in hand".into(),
            land.iter().map(|l| pct(l.1, l.2)).collect(),
        ));
        rows.push((
            "missed, no land".into(),
            land.iter().map(|l| pct(l.2 - l.0 - l.1, l.2)).collect(),
        ));
        rows.push((
            "mana available".into(),
            mana.iter().map(|m| avg(m.0, m.2)).collect(),
        ));
        rows.push((
            "mana spent".into(),
            mana.iter().map(|m| avg(m.1, m.2)).collect(),
        ));
        rows.push((
            "spent / available".into(),
            mana.iter().map(|m| pct(m.1, m.0)).collect(),
        ));
        rows.push((
            "spells cast".into(),
            spells.iter().map(|m| avg(m.0, m.1)).collect(),
        ));
        rows.push((
            "attacking creatures".into(),
            att.iter().map(|m| avg(m.0, m.1)).collect(),
        ));
        rows.push((
            "players still in".into(),
            land.iter().map(|l| pct(l.2, np)).collect(),
        ));
        for (name, vals) in rows {
            println!(
                "{:<22} {}",
                name,
                vals.iter()
                    .map(|v| format!("{:>6}", v.trim()))
                    .collect::<String>()
            );
        }
        // Commander.
        let mut first: BTreeMap<u32, f64> = BTreeMap::new();
        let (mut never, mut casts) = (0.0, 0.0);
        for (g, s) in &pg {
            let p = &g.stats.players[*s];
            casts += p.cmd_casts as f64;
            match p.cmd_first_turn {
                Some(t) => *first.entry(t.min(11)).or_default() += 1.0,
                None => never += 1.0,
            }
        }
        println!(
            "commander: never cast {} | casts per game {:.2} (recasts {:.2}) | first cast on own turn: {}",
            pct(never, np),
            casts / np,
            (casts - (np - never)) / np,
            first
                .iter()
                .map(|(t, c)| format!(
                    "{}{} {}",
                    if *t == 11 { ">" } else { "" },
                    t.min(&10),
                    pct(*c, np)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        );
        // Spells, hand at elimination, library cast.
        let (mut sp, mut libc, mut libn) = (0.0, 0.0, 0.0);
        let mut hand_out = (0.0, 0.0);
        for (g, s) in &pg {
            let p = &g.stats.players[*s];
            sp += p.spells_cast as f64;
            libc += p.cast.len() as f64;
            libn += deck_of(g.decks[*s]).library_size() as f64;
            if let Some(e) = g.stats.elims.iter().find(|e| e.seat == *s) {
                hand_out.0 += e.hand as f64;
                hand_out.1 += 1.0;
            }
        }
        println!(
            "spells cast per game {:.1} | cards cast from the deck: {} of the library per game | cards in hand when eliminated: avg {}",
            sp / np,
            pct(libc, libn),
            avg(hand_out.0, hand_out.1)
        );
        // Combat.
        let mut c = [0f64; 12];
        let mut lost = (0.0, 0.0, 0.0);
        for (g, s) in &pg {
            let p = &g.stats.players[*s];
            c[0] += p.attackers as f64;
            c[1] += p.at_life_leader as f64;
            c[2] += p.at_threat_leader as f64;
            c[3] += p.at_weakest as f64;
            c[4] += p.at_walkers as f64;
            c[5] += p.attackers_blocked as f64;
            c[6] += p.blocks as f64;
            c[7] += p.chumps as f64;
            lost.0 += p.creatures_lost as f64;
            lost.1 += p.creatures_died as f64;
            lost.2 += p.tokens_lost as f64;
        }
        println!(
            "attacks: {:.1} attacking creatures per game | aimed at: life leader {} biggest threat {} lowest life {} planeswalkers {} | blocked {}",
            c[0] / np,
            pct(c[1], c[0]),
            pct(c[2], c[0]),
            pct(c[3], c[0]),
            pct(c[4], c[0]),
            pct(c[5], c[0])
        );
        println!(
            "blocks: {:.2} blockers per game ({:.2} chump blocks) | creatures lost per game {:.2} (died {:.2}, tokens {:.2})",
            c[6] / np,
            c[7] / np,
            lost.0 / np,
            lost.1 / np,
            lost.2 / np
        );
        // Damage spread.
        let (mut top_share, mut nspread) = (0.0, 0.0);
        let (mut to_winner, mut fair, mut dmg_all) = (0.0, 0.0, 0.0);
        for (g, s) in &pg {
            let p = &g.stats.players[*s];
            let dealt: Vec<f64> = (0..g.decks.len())
                .map(|o| (p.combat_to[o] + p.noncombat_to[o]) as f64)
                .collect();
            let sum: f64 = dealt.iter().sum();
            if sum > 0.0 {
                top_share += dealt.iter().cloned().fold(0.0, f64::max) / sum;
                nspread += 1.0;
                dmg_all += sum;
                if let Some(w) = g.stats.winner.filter(|&w| w != *s) {
                    to_winner += dealt[w];
                    fair += sum / (g.decks.len() - 1) as f64;
                }
            }
        }
        let combat: f64 = pg
            .iter()
            .map(|(g, s)| g.stats.players[*s].combat_to.iter().sum::<i32>() as f64)
            .sum();
        println!(
            "damage dealt per game {:.1} ({} combat) | share to the most-hit opponent {} | damage to the eventual winner vs an even split {:.2}x",
            dmg_all / np,
            pct(combat, dmg_all),
            pct(top_share, nspread),
            if fair > 0.0 { to_winner / fair } else { 0.0 }
        );
        // Removal and counters.
        let rem: Vec<_> = pg
            .iter()
            .flat_map(|(g, s)| g.stats.players[*s].removal.iter())
            .collect();
        let nr = rem.len() as f64;
        let mut kinds: BTreeMap<&str, f64> = BTreeMap::new();
        for r in &rem {
            *kinds.entry(r.kind).or_default() += 1.0;
        }
        let cnt = |f: &dyn Fn(&&msh_deckbuilder::playtest::audit::Removal) -> bool| {
            rem.iter().filter(|r| f(r)).count() as f64
        };
        println!(
            "removal (targeted harmful effects): {:.2} per game [{}] | own permanents {} | tokens {} | creatures {} | target was its controller's best {} | controller had most life {} | controller was the biggest threat {} | avg target value {:.1}",
            nr / np,
            kinds
                .iter()
                .map(|(k, c)| format!("{} {:.0}", k, c))
                .collect::<Vec<_>>()
                .join(", "),
            pct(cnt(&|r| r.own), nr),
            pct(cnt(&|r| r.token), nr),
            pct(cnt(&|r| r.creature), nr),
            pct(cnt(&|r| r.best_of_ctrl), nr),
            pct(cnt(&|r| r.ctrl_life_leader), nr),
            pct(cnt(&|r| r.ctrl_threat_leader), nr),
            rem.iter().map(|r| r.value as f64).sum::<f64>() / nr.max(1.0)
        );
        let cs: Vec<_> = pg
            .iter()
            .flat_map(|(g, s)| g.stats.players[*s].counters.iter())
            .collect();
        let nc = cs.len() as f64;
        let mut mvs: BTreeMap<u8, f64> = BTreeMap::new();
        for c in &cs {
            *mvs.entry(c.mv).or_default() += 1.0;
        }
        println!(
            "counterspells: {:.2} per game | on permanents {} | caster was the biggest threat {} | paid off ('unless pays') {} | MV: {}",
            nc / np,
            pct(cs.iter().filter(|c| c.permanent).count() as f64, nc),
            pct(
                cs.iter().filter(|c| c.caster_threat_leader).count() as f64,
                nc
            ),
            pct(cs.iter().filter(|c| c.paid_off).count() as f64, nc),
            mvs.iter()
                .map(|(m, c)| format!("{}:{:.0}", m, c))
                .collect::<Vec<_>>()
                .join(" ")
        );
        println!();
    }

    // ---------------------------------------------------------------- our cards
    if let Some(m) = &me {
        println!("== OUR DECK'S CARDS ==");
        let mut count = vec![0u32; m.cards.len()];
        let mut seen = 0.0;
        for g in &played {
            let s = g.decks.iter().position(|&d| d == 0).unwrap();
            seen += 1.0;
            let mut once = vec![false; m.cards.len()];
            for &c in &g.stats.players[s].cast {
                if (c as usize) < once.len() {
                    once[c as usize] = true;
                }
            }
            for (k, o) in once.iter().enumerate() {
                if *o {
                    count[k] += 1;
                }
            }
        }
        let mut order: Vec<usize> = (0..m.cards.len())
            .filter(|&k| !m.cards[k].is(T_LAND))
            .collect();
        order.sort_by_key(|&k| std::cmp::Reverse(count[k]));
        println!("most cast (share of games in which it was cast at least once):");
        for &k in order.iter().take(20) {
            println!("  {:<40} {}", m.cards[k].name, pct(count[k] as f64, seen));
        }
        let never: Vec<&str> = order
            .iter()
            .filter(|&&k| count[k] == 0)
            .map(|&k| m.cards[k].name.as_str())
            .collect();
        let rare: Vec<String> = order
            .iter()
            .filter(|&&k| count[k] > 0 && (count[k] as f64) < 0.05 * seen)
            .map(|&k| format!("{} {}", m.cards[k].name, pct(count[k] as f64, seen).trim()))
            .collect();
        println!("never cast (nonland): {}", never.join(", "));
        println!("cast in under 5% of games: {}", rare.join(", "));
        let least: Vec<String> = order
            .iter()
            .rev()
            .take(10)
            .map(|&k| format!("{} {}", m.cards[k].name, pct(count[k] as f64, seen).trim()))
            .collect();
        println!("least cast nonland cards: {}", least.join(", "));
        println!();
    }

    // ---------------------------------------------------------------- removal received
    println!("== REMOVAL RECEIVED (targeted harmful effects aimed at each deck's permanents) ==");
    let mut got: BTreeMap<usize, (f64, f64)> = BTreeMap::new();
    for g in &played {
        for (s, &d) in g.decks.iter().enumerate() {
            got.entry(d).or_default().1 += 1.0;
            let n = g
                .stats
                .players
                .iter()
                .flat_map(|p| p.removal.iter())
                .filter(|r| r.target_seat as usize == s)
                .count();
            got.entry(d).or_default().0 += n as f64;
        }
    }
    for (d, (n, games)) in &got {
        println!("  {:<55} {:.2} per game", label(*d), n / games);
    }
    println!();

    // ---------------------------------------------------------------- eliminations
    println!("== ELIMINATIONS ==");
    let mut matrix: BTreeMap<(usize, usize), f64> = BTreeMap::new();
    let (mut by_winner, mut by_other, mut by_none) = (0.0, 0.0, 0.0);
    let mut order_rank: Vec<(f64, f64)> = vec![(0.0, 0.0); 3];
    let mut top_out = vec![(0.0, 0.0); 3];
    let mut winner_no_kill = 0.0;
    for g in &played {
        let s = &g.stats;
        let w = s.winner;
        if !s.elims.iter().any(|e| e.by == w && w.is_some()) {
            winner_no_kill += 1.0;
        }
        for (k, e) in s.elims.iter().enumerate() {
            let victim = g.decks[e.seat];
            match e.by {
                Some(b) => {
                    *matrix.entry((g.decks[b], victim)).or_default() += 1.0;
                    if Some(b) == w {
                        by_winner += 1.0;
                    } else {
                        by_other += 1.0;
                    }
                }
                None => by_none += 1.0,
            }
            if k < 3 {
                order_rank[k].0 += e.threat_rank as f64 + 1.0;
                order_rank[k].1 += 1.0;
                top_out[k].1 += 1.0;
                if e.threat_rank == 0 {
                    top_out[k].0 += 1.0;
                }
            }
        }
    }
    let ne = by_winner + by_other + by_none;
    println!(
        "eliminations credited to: the eventual winner {} | another player {} | nobody (decking, own life payments...) {}",
        pct(by_winner, ne),
        pct(by_other, ne),
        pct(by_none, ne)
    );
    println!(
        "games the winner eliminated nobody (kingmade or outlasted): {}",
        pct(winner_no_kill, total)
    );
    for k in 0..3 {
        println!(
            "elimination #{}: avg threat rank at the start of that turn {} (1 = biggest board) | was the biggest threat {}",
            k + 1,
            avg(order_rank[k].0, order_rank[k].1),
            pct(top_out[k].0, top_out[k].1)
        );
    }
    println!("who eliminates whom (eliminator -> eliminated: count):");
    let mut decks: Vec<usize> = by_deck.keys().copied().collect();
    decks.sort();
    for &a in &decks {
        let row: Vec<String> = decks
            .iter()
            .filter_map(|&b| {
                matrix.get(&(a, b)).map(|c| {
                    format!(
                        "{}:{:.0}",
                        if b == 0 {
                            "OURS".into()
                        } else {
                            format!("L{}", b)
                        },
                        c
                    )
                })
            })
            .collect();
        if !row.is_empty() {
            println!(
                "  {:<8} -> {}",
                if a == 0 {
                    "OURS".to_string()
                } else {
                    format!("L{}", a)
                },
                row.join(" ")
            );
        }
    }
}
