use super::*;
use std::sync::OnceLock;

fn db() -> &'static CardDb {
    static DB: OnceLock<CardDb> = OnceLock::new();
    DB.get_or_init(|| {
        CardDb::load(&std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data/cards.json"))
            .unwrap()
    })
}

fn rules_at_session(n: u32) -> Rules {
    let mut r = Rules::default();
    league::apply_session(&mut r, n).unwrap();
    r
}

/// One season through Session 6 (drafts, singleton start), shared by several tests.
fn six() -> &'static Season {
    static S: OnceLock<Season> = OnceLock::new();
    S.get_or_init(|| simulate(db(), &rules_at_session(6), 7, None, &Pins::default()))
}

/// The rarity in a log entry: "Name (rarity)" or "Name (rarity, 8 VP, ...)".
fn rarity_in(entry: &str) -> &str {
    let open = entry.rfind(" (").unwrap();
    entry[open + 2..].split([',', ')']).next().unwrap()
}

fn name_in(entry: &str) -> &str {
    &entry[..entry.rfind(" (").unwrap()]
}

fn price(r: &str) -> u32 {
    match r {
        "mythic" => 8,
        "rare" => 4,
        "uncommon" => 2,
        _ => 1,
    }
}

#[test]
fn deterministic_for_a_seed() {
    let r = rules_at_session(3);
    let a = simulate(db(), &r, 11, None, &Pins::default());
    let b = simulate(db(), &r, 11, None, &Pins::default());
    assert_eq!(a, b);
    let c = simulate(db(), &r, 12, None, &Pins::default());
    assert_ne!(a.opponents, c.opponents);
    // Session 2's drafted pack is usable at Session 3: about a pack's worth of picks each.
    assert!(a.opponents.iter().all(|o| o.log[2].picks.len() >= 10));
}

#[test]
fn trades_follow_the_vp_rules() {
    let s = six();
    assert_eq!(s.opponents.len(), 7);
    for o in &s.opponents {
        let mut bank = 0u32;
        for l in &o.log {
            // At least 10 VP traded in every session (a note says so if they ran out).
            assert!(
                l.traded_vp >= MIN_TRADE_VP || !l.notes.is_empty(),
                "{} session {}",
                o.name,
                l.session
            );
            let traded: Vec<&str> = l.traded.iter().map(|t| rarity_in(t)).collect();
            let claims: Vec<&str> = l.claimed.iter().map(|t| rarity_in(t)).collect();
            let count = |v: &[&str], r: &str| v.iter().filter(|x| **x == r).count();
            // Rarity floor: each rare claim takes a traded rare, each mythic claim a traded
            // mythic or two traded rares - all from this same round.
            let (tm, tr) = (count(&traded, "mythic"), count(&traded, "rare"));
            let (cm, cr) = (count(&claims, "mythic"), count(&claims, "rare"));
            assert!(
                tr >= cr + 2 * cm.saturating_sub(tm),
                "{} session {}: claims {:?} with trade-ins {:?}",
                o.name,
                l.session,
                l.claimed,
                l.traded
            );
            // One copy per card, a few claims a round.
            let mut names: Vec<&str> = l.claimed.iter().map(|c| name_in(c)).collect();
            let n = names.len();
            names.sort();
            names.dedup();
            assert_eq!(names.len(), n);
            assert!(n <= MAX_CLAIMS);
            // VP: trade-ins at face value, claims at 2x, staples 5k; never overspent.
            let traded_vp: u32 = traded.iter().map(|r| price(r)).sum();
            assert_eq!(traded_vp, l.traded_vp);
            let staples: u32 = l
                .staples
                .iter()
                .map(|s| {
                    s.rsplit('(')
                        .next()
                        .unwrap()
                        .trim_end_matches(" VP)")
                        .parse::<u32>()
                        .unwrap()
                })
                .sum();
            let spent = claims.iter().map(|r| 2 * price(r)).sum::<u32>() + staples;
            assert_eq!(
                bank + traded_vp,
                spent + l.vp,
                "{} session {}",
                o.name,
                l.session
            );
            bank = l.vp;
        }
        assert_eq!(bank, o.vp);
        // The k-th staple costs 5k.
        let costs: Vec<u32> = o
            .log
            .iter()
            .flat_map(|l| l.staples.iter())
            .map(|s| {
                s.rsplit('(')
                    .next()
                    .unwrap()
                    .trim_end_matches(" VP)")
                    .parse::<u32>()
                    .unwrap()
            })
            .collect();
        assert_eq!(
            costs,
            (1..=costs.len() as u32)
                .map(staple_cost)
                .collect::<Vec<_>>()
        );
        assert_eq!(costs.len() as u32, o.staples);
    }
}

#[test]
fn the_pool_is_shared() {
    // Session 1: the pool is Foundations, one copy each, and trade-ins only arrive after
    // the round - so no card can be claimed by two players.
    let s = simulate(db(), &rules_at_session(1), 3, None, &Pins::default());
    let mut all: Vec<&str> = s
        .opponents
        .iter()
        .flat_map(|o| o.log[0].claimed.iter())
        .map(|c| name_in(c))
        .collect();
    let n = all.len();
    assert!(n > 0, "nobody claimed anything");
    all.sort();
    all.dedup();
    assert_eq!(all.len(), n, "a card was claimed twice");
    for name in &all {
        assert_eq!(db().get(name).unwrap().set, POOL_START_SET);
    }
    // An imported pool replaces the simulated one for the last session's claims.
    let real = Collection::new(
        "real.txt".into(),
        db().cards
            .iter()
            .filter(|c| c.set == "FRA" && c.rarity == "uncommon" && !c.is_land())
            .map(|c| (c.name.clone(), 1))
            .collect(),
    );
    let s = simulate(db(), &rules_at_session(2), 3, Some(&real), &Pins::default());
    assert!(s.real_pool);
    let mut claimed: Vec<&str> = s
        .opponents
        .iter()
        .flat_map(|o| o.log[1].claimed.iter())
        .map(|c| name_in(c))
        .collect();
    assert!(!claimed.is_empty());
    for c in &claimed {
        assert_eq!(real.owned(c), 1, "{} isn't in the imported pool", c);
    }
    let n = claimed.len();
    claimed.sort();
    claimed.dedup();
    assert_eq!(claimed.len(), n);
}

#[test]
fn decks_are_legal_and_singleton_from_session_6() {
    let s = six();
    let rules = rules_at_session(6);
    for o in &s.opponents {
        let deck = o.deck().expect("every opponent has a deck");
        let coll = Collection::new("pool.txt".into(), o.pool.clone());
        let errors: Vec<String> = deck
            .validate(db(), &coll, &rules)
            .into_iter()
            .filter(|p| p.severity == crate::deck::Severity::Error)
            .map(|p| p.message)
            .collect();
        assert!(errors.is_empty(), "{}: {:?}", o.name, errors);
        assert_eq!(deck.total_count(&rules), 60);
        let id = deck.identity(db()).unwrap();
        for (n, c) in &deck.cards {
            let card = db().get(n).unwrap();
            assert!(card.identity().is_subset_of(id), "{} in {}", n, o.name);
            assert!(card.is_basic() || *c == 1, "{}x {} under singleton", c, n);
        }
    }
}

#[test]
fn pinned_commander_and_decklist() {
    let cmd = db()
        .cards
        .iter()
        .find(|c| c.set == "MSH" && c.can_be_commander() && c.rarity == "mythic")
        .unwrap();
    let mut pins = Pins::default();
    pins.players.insert(
        "hobbit player 2".into(),
        Pin {
            commander: Some(cmd.name.clone()),
            deck: None,
        },
    );
    pins.players.insert(
        "Mixed player".into(),
        Pin {
            commander: None,
            deck: Some(format!(
                "Commander\n1 {}\n\nDeck\n30 Island\n29 Mountain\n",
                cmd.name
            )),
        },
    );
    let s = simulate(db(), &rules_at_session(3), 5, None, &pins);
    let o = s
        .opponents
        .iter()
        .find(|o| o.name == "Hobbit player 2")
        .unwrap();
    assert_eq!(o.commander.as_deref(), Some(cmd.name.as_str()));
    assert_eq!(o.pinned, "commander");
    assert!(
        o.log
            .iter()
            .all(|l| l.commander.as_deref() == Some(cmd.name.as_str()))
    );
    let m = s
        .opponents
        .iter()
        .find(|o| o.name == "Mixed player")
        .unwrap();
    assert_eq!(m.pinned, "decklist");
    // The simulation plays the decklist's commander all along (drafts, trades).
    assert!(
        m.log
            .iter()
            .all(|l| l.commander.as_deref() == Some(cmd.name.as_str()))
    );
    assert_eq!(m.cards.get("Island"), Some(&30));
    assert_eq!(m.deck().unwrap().main_count(), 59);
}

#[test]
fn remembered_quality_matches_the_analysis() {
    let o = &six().opponents[0];
    let coll = Collection::new("pool.txt".into(), o.pool.clone());
    let rules = rules_at_session(6);
    let mut memo = Memo::default();
    let cmds: Vec<&Card> = coll
        .counts
        .keys()
        .filter_map(|n| db().get(n))
        .filter(|c| c.can_be_commander())
        .collect();
    assert!(!cmds.is_empty());
    for c in cmds {
        let colors = analysis::best_colors(c, db(), &coll, &rules);
        let want = analysis::pool_quality_for(c, colors, db(), &coll, &rules);
        let got = quality(c, db(), &coll, &rules, &mut memo);
        assert!((want - got).abs() < 1e-4, "{}: {} vs {}", c.name, want, got);
    }
}

#[test]
fn claim_floors() {
    let get = |r: &str| {
        db().cards
            .iter()
            .find(|c| c.set == "MSH" && c.rarity == r)
            .unwrap()
    };
    let (m, r, u, c) = (get("mythic"), get("rare"), get("uncommon"), get("common"));
    // A mythic needs a mythic or two rares traded in this round.
    assert!(plan_claim(m, 100, &[], &[u, u, c], 0).is_none());
    assert!(plan_claim(m, 100, &[], &[r, u], 0).is_none());
    let (commit, floor, _) = plan_claim(m, 100, &[], &[u, r, r], 0).unwrap();
    assert_eq!((floor, commit.len()), (2, 2));
    let (commit, floor, mark) = plan_claim(m, 0, &[(m, false)], &[u, r, u, c], 0).unwrap();
    assert_eq!((floor, mark), (0, vec![0]));
    assert_eq!(
        commit,
        vec![0, 1, 2],
        "8 VP traded + 8 more for the 16 VP price"
    );
    assert!(plan_claim(m, 0, &[(m, false)], &[u, u, c], 0).is_none());
    // A rare already covering another claim doesn't count twice.
    assert!(plan_claim(r, 100, &[(r, true)], &[u], 0).is_none());
    // Price: 2x VP from the bank and this round's trades.
    assert!(plan_claim(u, 3, &[], &[], 0).is_none());
    assert!(plan_claim(u, 4, &[], &[], 0).is_some());
    assert!(plan_claim(u, 4, &[], &[], 1).is_none());
}
