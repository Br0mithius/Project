//! Simulator tests: rules pieces with real league cards, then whole-game sanity checks.

use super::combat::{Attack, Def};
use super::*;
use crate::card::Card;
use crate::db::CardDb;

fn db() -> CardDb {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
    CardDb::load(&dir.join("cards.json")).unwrap()
}

fn card(name: &str, cost: &str, ty: &str, text: &str, pt: Option<(&str, &str)>) -> Card {
    Card {
        name: name.into(),
        mana_cost: cost.into(),
        cmc: crate::card::symbols_in(cost)
            .iter()
            .map(|s| s.parse::<f32>().unwrap_or(1.0))
            .sum(),
        type_line: ty.into(),
        oracle_text: text.into(),
        power: pt.map(|x| x.0.into()),
        toughness: pt.map(|x| x.1.into()),
        ..Default::default()
    }
}

/// A deck of these cards (each once), nothing in the library.
fn prep(cards: &[Card]) -> Prepared {
    Prepared {
        cards: cards.iter().map(compile).collect(),
        library: Vec::new(),
        commanders: Vec::new(),
    }
}

fn prep_db(db: &CardDb, names: &[&str]) -> Prepared {
    let cards: Vec<Card> = names
        .iter()
        .map(|n| {
            db.get(n)
                .unwrap_or_else(|| panic!("{} not in db", n))
                .clone()
        })
        .collect();
    prep(&cards)
}

/// An empty two-player game (no hands, no libraries) to set up by hand.
fn game<'a>(decks: &[&'a Prepared]) -> Game<'a> {
    let mut g = Game::new(decks, &Rules::default(), Rng::new(7));
    for p in g.players.iter_mut() {
        p.hand.clear();
        p.library = vec![0; 30]; // something to draw
        p.command.clear();
    }
    g.round = 3;
    g
}

/// Puts card `idx` of player `p`'s list onto the battlefield, ready to act.
fn put(g: &mut Game, p: usize, idx: u16) -> u32 {
    let i = g.enter_card(p, idx, p, false).unwrap();
    g.perms[i].sick = false;
    let id = g.perms[i].id;
    g.run_pending();
    g.sba();
    id
}

fn lands(g: &mut Game, p: usize, n: usize, color: char) {
    let list: &[SimCard] = g.players[p].cards;
    let idx = list
        .iter()
        .position(|c| c.is(T_LAND) && c.produces == crate::card::ColorSet::from_char(color).0)
        .expect("land in deck") as u16;
    for _ in 0..n {
        put(g, p, idx);
    }
}

fn perm<'g>(g: &'g Game, id: u32) -> &'g Perm {
    &g.perms[g.idx(id).expect("still on the battlefield")]
}

#[test]
fn mana_is_paid_with_the_right_colours() {
    let d = prep(&[
        Card::basic_land("Plains", 'W'),
        Card::basic_land("Island", 'U'),
    ]);
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 0);
    put(&mut g, 0, 1);
    assert!(g.can_pay(0, &Mana::parse("{W}{U}")));
    assert!(!g.can_pay(0, &Mana::parse("{U}{U}")));
    assert!(g.can_pay(0, &Mana::parse("{1}{W}")));
    assert!(!g.can_pay(0, &Mana::parse("{2}{W}")));
    g.make_item(0, "treasure");
    assert!(
        g.can_pay(0, &Mana::parse("{U}{U}")),
        "a Treasure makes any colour"
    );
    assert!(g.pay(0, &Mana::parse("{U}{U}")));
    assert!(
        !g.perms.iter().any(|x| x.tok == Tok::Treasure),
        "the Treasure was sacrificed"
    );
}

#[test]
fn combat_keywords() {
    let d = prep(&[
        card(
            "Striker",
            "{1}{W}",
            "Creature — Soldier",
            "First strike",
            Some(("2", "2")),
        ),
        card("Bear", "{1}{G}", "Creature — Bear", "", Some(("2", "2"))),
        card(
            "Viper",
            "{G}",
            "Creature — Snake",
            "Deathtouch",
            Some(("1", "1")),
        ),
        card(
            "Giant",
            "{4}{G}",
            "Creature — Giant",
            "Trample",
            Some(("5", "5")),
        ),
        card(
            "Leech",
            "{1}{B}",
            "Creature — Bat",
            "Lifelink",
            Some(("3", "1")),
        ),
    ]);
    // First strike: the 2/2 first striker kills a 2/2 blocker and survives.
    let mut g = game(&[&d, &d]);
    let s = put(&mut g, 0, 0);
    let b = put(&mut g, 1, 1);
    let at = vec![Attack {
        attacker: s,
        def: Def::Player(1),
        blockers: vec![b],
        blocked: true,
    }];
    g.damage_step(0, &at, true);
    g.damage_step(0, &at, false);
    assert!(g.idx(s).is_some() && g.idx(b).is_none());
    // Deathtouch: a 1/1 kills a 5/5.
    let v = put(&mut g, 1, 2);
    let giant = put(&mut g, 0, 3);
    let at = vec![Attack {
        attacker: giant,
        def: Def::Player(1),
        blockers: vec![v],
        blocked: true,
    }];
    g.damage_step(0, &at, false);
    assert!(g.idx(giant).is_none(), "deathtouch kills");
    let life = g.players[1].life;
    // Trample: 5/5 blocked by a 2/2 deals 3 to the player.
    let giant = put(&mut g, 0, 3);
    let b2 = put(&mut g, 1, 1);
    let at = vec![Attack {
        attacker: giant,
        def: Def::Player(1),
        blockers: vec![b2],
        blocked: true,
    }];
    g.damage_step(0, &at, false);
    assert_eq!(g.players[1].life, life - 3, "trample");
    // Lifelink: unblocked 3 power gains 3.
    let leech = put(&mut g, 0, 4);
    let mine = g.players[0].life;
    let at = vec![Attack {
        attacker: leech,
        def: Def::Player(1),
        blockers: vec![],
        blocked: false,
    }];
    g.damage_step(0, &at, false);
    assert_eq!(g.players[0].life, mine + 3);
}

#[test]
fn evasion_and_blocking_rules() {
    let d = prep(&[
        card(
            "Bird",
            "{1}{U}",
            "Creature — Bird",
            "Flying",
            Some(("2", "2")),
        ),
        card("Bear", "{1}{G}", "Creature — Bear", "", Some(("2", "2"))),
        card(
            "Spider",
            "{1}{G}",
            "Creature — Spider",
            "Reach",
            Some(("1", "3")),
        ),
        card(
            "Brute",
            "{2}{R}",
            "Creature — Ogre",
            "Menace",
            Some(("3", "3")),
        ),
    ]);
    let mut g = game(&[&d, &d]);
    let bird = put(&mut g, 0, 0);
    let bear = put(&mut g, 1, 1);
    let spider = put(&mut g, 1, 2);
    let (bi, be, sp) = (
        g.idx(bird).unwrap(),
        g.idx(bear).unwrap(),
        g.idx(spider).unwrap(),
    );
    assert!(!g.can_block(be, bi), "ground creatures can't block flyers");
    assert!(g.can_block(sp, bi), "reach blocks flyers");
    // Menace: a single blocker is dropped by the combat rules; the AI double-blocks or not at all.
    let brute = put(&mut g, 0, 3);
    let at = vec![Attack {
        attacker: brute,
        def: Def::Player(1),
        blockers: vec![],
        blocked: false,
    }];
    let blocks = ai::declare_blocks(&g, 1, &at);
    assert!(blocks.len() != 1, "menace needs two blockers: {:?}", blocks);
}

#[test]
fn ai_blocks_for_value_and_chumps_only_to_survive() {
    let d = prep(&[
        card("Ox", "{3}{W}", "Creature — Ox", "", Some(("2", "6"))),
        card("Bear", "{1}{G}", "Creature — Bear", "", Some(("2", "2"))),
        card("Giant", "{5}{R}", "Creature — Giant", "", Some(("7", "7"))),
    ]);
    let mut g = game(&[&d, &d]);
    let bear = put(&mut g, 0, 1);
    let ox = put(&mut g, 1, 0);
    let at = vec![Attack {
        attacker: bear,
        def: Def::Player(1),
        blockers: vec![],
        blocked: false,
    }];
    let blocks = ai::declare_blocks(&g, 1, &at);
    assert!(
        blocks.contains(&(ox, bear)) || blocks.is_empty(),
        "the 2/6 eats the 2/2 or holds back"
    );
    // A 7/7 at 30 life: no chump. At 6 life: chump.
    let giant = put(&mut g, 0, 2);
    let bear2 = put(&mut g, 1, 1);
    let at = vec![Attack {
        attacker: giant,
        def: Def::Player(1),
        blockers: vec![],
        blocked: false,
    }];
    let blocks = ai::declare_blocks(&g, 1, &at);
    assert!(
        !blocks.iter().any(|(b, _)| *b == bear2),
        "no chump at high life"
    );
    g.players[1].life = 6;
    let blocks = ai::declare_blocks(&g, 1, &at);
    assert!(!blocks.is_empty(), "chumps to survive");
}

#[test]
fn counterspells_stop_big_threats() {
    let db = db();
    let d0 = prep(&[
        Card::basic_land("Forest", 'G'),
        card(
            "Huge",
            "{4}{G}{G}",
            "Creature — Wurm",
            "Trample\nWhen this creature enters, draw two cards.",
            Some(("8", "8")),
        ),
    ]);
    let d1 = prep_db(&db, &["Island", "Cancel"]);
    let mut g = game(&[&d0, &d1]);
    lands(&mut g, 0, 6, 'G');
    lands(&mut g, 1, 3, 'U');
    g.players[0].hand.push(1);
    g.players[1].hand.push(1);
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()));
    assert!(
        !g.perms.iter().any(|x| x.card == Some((0, 1))),
        "the wurm was countered"
    );
    assert!(g.players[0].graveyard.contains(&1));
    assert!(g.players[1].graveyard.contains(&1), "Cancel was used");
}

#[test]
fn removal_targets_the_biggest_threat_and_respects_hexproof_and_ward() {
    let d0 = prep(&[
        Card::basic_land("Swamp", 'B'),
        card(
            "Murder",
            "{1}{B}{B}",
            "Instant",
            "Destroy target creature.",
            None,
        ),
    ]);
    let d1 = prep(&[
        card("Bear", "{1}{G}", "Creature — Bear", "", Some(("2", "2"))),
        card(
            "Dragon",
            "{4}{R}{R}",
            "Creature — Dragon",
            "Flying",
            Some(("6", "6")),
        ),
        card(
            "Shy Dragon",
            "{4}{R}{R}",
            "Creature — Dragon",
            "Flying, hexproof",
            Some(("7", "7")),
        ),
    ]);
    let mut g = game(&[&d0, &d1]);
    lands(&mut g, 0, 3, 'B');
    let bear = put(&mut g, 1, 0);
    let dragon = put(&mut g, 1, 1);
    let shy = put(&mut g, 1, 2);
    g.players[0].hand.push(1);
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()));
    assert!(g.idx(dragon).is_none(), "Murder hits the dragon");
    assert!(g.idx(shy).is_some(), "hexproof can't be targeted");
    assert!(g.idx(bear).is_some());
}

#[test]
fn equipment_and_auras() {
    let db = db();
    let d = prep(&[
        Card::basic_land("Plains", 'W'),
        card(
            "Sword",
            "{2}",
            "Artifact — Equipment",
            "Equipped creature gets +2/+2.\nEquip {2}",
            None,
        ),
        card("Bear", "{1}{G}", "Creature — Bear", "", Some(("2", "2"))),
        card(
            "Pacifism",
            "{1}{W}",
            "Enchantment — Aura",
            "Enchant creature\nEnchanted creature can't attack or block.",
            None,
        ),
    ]);
    let _ = db;
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 6, 'W');
    let sword = put(&mut g, 0, 1);
    let bear = put(&mut g, 0, 2);
    assert!(g.equip(0, sword, bear));
    assert_eq!((perm(&g, bear).p, perm(&g, bear).t), (4, 4));
    // Pacifism on the opponent's creature locks it.
    let foe = put(&mut g, 1, 2);
    g.players[0].hand.push(3);
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()));
    g.recalc();
    let fi = g.idx(foe).unwrap();
    assert!(g.perms[fi].locked, "Pacifism locks the opponent's creature");
    assert!(!g.can_attack(fi));
}

#[test]
fn commander_rules() {
    let d = prep(&[
        Card::basic_land("Mountain", 'R'),
        card(
            "Boss",
            "{1}{R}",
            "Legendary Creature — Villain",
            "Haste",
            Some(("3", "3")),
        ),
    ]);
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 6, 'R');
    g.players[0].command = vec![(1, 1)];
    let base = g
        .cast_cost(0, Src::Command(0), CastOpts::default())
        .unwrap()
        .total();
    assert!(g.cast(0, Src::Command(0), CastOpts::default()));
    let i = g.perms.iter().position(|x| x.commander == 1).unwrap();
    // Dies: back to the command zone, and the next cast costs {2} more.
    g.destroy(i);
    assert_eq!(g.players[0].command.len(), 1);
    let again = g
        .cast_cost(0, Src::Command(0), CastOpts::default())
        .unwrap()
        .total();
    assert_eq!(again, base + 2);
    // 21 commander damage from one commander kills, whatever the life total.
    g.players[1].cmd_damage[0][1] = 21;
    g.sba();
    assert!(!g.players[1].alive);
}

#[test]
fn poison_and_life_end_the_game() {
    let d = prep(&[card(
        "Bear",
        "{1}{G}",
        "Creature — Bear",
        "",
        Some(("2", "2")),
    )]);
    let mut g = game(&[&d, &d, &d]);
    g.players[1].poison = 10;
    g.players[2].life = 0;
    g.sba();
    assert!(!g.players[1].alive && !g.players[2].alive && g.players[0].alive);
}

#[test]
fn sagas_plans_and_jace() {
    let db = db();
    let d = prep_db(
        &db,
        &[
            "Island",
            "Armor Wars",
            "Academic Ascent",
            "Forest",
            "Claim the Kingdom",
            "Plains",
        ],
    );
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 6, 'U');
    lands(&mut g, 0, 2, 'W');
    // Saga: chapter I on entering; it's gone after its last chapter.
    let saga = put(&mut g, 0, 1);
    assert_eq!(perm(&g, saga).other_counters, 1);
    for _ in 0..3 {
        if let Some(i) = g.idx(saga) {
            g.add_other_counters(i, 1);
            g.run_pending();
            let ch = g.card(g.perms[i].card.unwrap()).chapters as i16;
            if g.perms[i].other_counters >= ch {
                g.sacrifice(i);
            }
        }
    }
    assert!(
        g.idx(saga).is_none(),
        "the Saga is sacrificed after its last chapter"
    );
    // Empower Jace: a Jace token with loyalty, whose abilities can be used.
    g.players[0].hand.push(2);
    let h = g.players[0].hand.len() - 1;
    assert!(g.cast(0, Src::Hand(h), CastOpts::default()));
    let jace = g
        .perms
        .iter()
        .find(|x| x.tok == Tok::Jace)
        .expect("Jace token");
    assert_eq!(jace.loyalty, 2);
    // Plans: Claim the Kingdom goes off on its fourth plan counter.
    let plan = put(&mut g, 0, 4);
    let pi = g.idx(plan).unwrap();
    g.add_other_counters(pi, 3);
    g.run_pending();
    assert!(g.idx(plan).is_some(), "three counters aren't enough");
    let pi = g.idx(plan).unwrap();
    g.add_other_counters(pi, 1);
    g.run_pending();
    assert!(
        g.idx(plan).is_none(),
        "a Plan is sacrificed when it goes off"
    );
}

#[test]
fn adventures_and_power_up() {
    let db = db();
    let d = prep_db(
        &db,
        &[
            "Island",
            "Bilbo Baggins, Burglar",
            "Mountain",
            "Plains",
            "Swamp",
            "Thanos, the Mad Titan",
        ],
    );
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 3, 'U');
    g.players[0].hand.push(1);
    // The Adventure first: the card goes on an adventure, then the creature can be cast.
    assert!(g.cast(
        0,
        Src::Hand(0),
        CastOpts {
            back: true,
            ..Default::default()
        }
    ));
    assert_eq!(g.players[0].adventure, vec![1]);
    for x in g.perms.iter_mut() {
        x.tapped = false;
    }
    assert!(g.cast(0, Src::Adventure(0), CastOpts::default()));
    assert!(g.perms.iter().any(|x| x.card == Some((0, 1))));
    // Power-up costs less the turn the creature entered.
    lands(&mut g, 0, 2, 'R');
    let thanos = put(&mut g, 0, 5);
    let i = g.idx(thanos).unwrap();
    let k = g
        .abilities_of(&g.perms[i])
        .iter()
        .position(|a| matches!(a.trig, Trig::PowerUp(_)))
        .unwrap();
    let today = g.activation_cost(0, i, k).unwrap().total();
    g.perms[i].born = 0;
    let later = g.activation_cost(0, i, k).unwrap().total();
    assert_eq!(
        later,
        today + 3,
        "power-up is {{3}} cheaper the turn Thanos entered"
    );
}

#[test]
fn mulligans_keep_playable_hands() {
    let lands_only = prep(&[
        Card::basic_land("Forest", 'G'),
        card("Bear", "{1}{G}", "Creature — Bear", "", Some(("2", "2"))),
    ]);
    let mut d = lands_only;
    d.library = [vec![0u16; 25], vec![1u16; 35]].concat();
    for seed in 0..20 {
        let g = Game::new(&[&d, &d], &Rules::default(), Rng::new(seed));
        let pl = &g.players[0];
        let lands = pl
            .hand
            .iter()
            .filter(|&&c| pl.cards[c as usize].is(T_LAND))
            .count();
        assert!(
            pl.mulligans == 2 || (2..=5).contains(&lands),
            "kept {} lands",
            lands
        );
        assert!(pl.hand.len() >= 6, "first mulligan is free");
    }
}

#[test]
fn hand_size_and_fog() {
    let d = prep(&[
        Card::basic_land("Forest", 'G'),
        card(
            "Fog",
            "{G}",
            "Instant",
            "Prevent all combat damage that would be dealt this turn.",
            None,
        ),
        card("Giant", "{4}{G}", "Creature — Giant", "", Some(("9", "9"))),
    ]);
    let mut g = game(&[&d, &d]);
    lands(&mut g, 1, 1, 'G');
    g.players[1].hand.push(1);
    g.players[1].life = 8;
    let giant = put(&mut g, 0, 2);
    let at = vec![Attack {
        attacker: giant,
        def: Def::Player(1),
        blockers: vec![],
        blocked: false,
    }];
    ai::before_blocks(&mut g, 1, &at);
    assert!(g.fog, "Fogs a lethal attack");
    // Cleanup discards down to seven.
    let mut g = game(&[&d, &d]);
    g.players[0].hand = vec![0; 10];
    g.take_turn(0, true);
    assert!(g.players[0].hand.len() <= 7);
}

// ------------------------------------------------------------ whole games

fn league_decks(db: &CardDb) -> Vec<Prepared> {
    gauntlet(db, &Rules::default(), 99)
}

#[test]
fn games_finish_and_basics_lose() {
    let db = db();
    let opp = league_decks(&db);
    assert!(opp.len() >= 3);
    let mut basics = crate::deck::Deck::new("basics");
    basics.add("Forest", 30);
    basics.add("Mountain", 30);
    let wr = win_rate(&basics, &opp, &db, &Rules::default(), 200, 3);
    assert!(wr < 0.02, "a pile of lands won {:.0}%", wr * 100.0);
}

#[test]
fn mirror_matches_are_even_and_better_decks_win_more() {
    let db = db();
    let decks = league_decks(&db);
    let rules = Rules::default();
    // A deck against three copies of itself: ~25%.
    let mut rng = Rng::new(5);
    let me = &decks[0];
    let mut wins = 0;
    let n = 800;
    for k in 0..n {
        let seat = k % 4;
        let table: Vec<&Prepared> = vec![me, me, me, me];
        if play_game(&table, &rules, &mut rng) == seat {
            wins += 1;
        }
    }
    let wr = wins as f32 / n as f32;
    assert!((0.17..=0.33).contains(&wr), "mirror win rate {:.2}", wr);
    // Half the deck replaced with lands is clearly worse.
    let mut weak = Prepared {
        cards: me.cards.clone(),
        library: me.library.clone(),
        commanders: me.commanders.clone(),
    };
    let land = weak.cards.iter().position(|c| c.is(T_LAND)).unwrap() as u16;
    for (k, c) in weak.library.iter_mut().enumerate() {
        if k % 2 == 0 {
            *c = land;
        }
    }
    // Same opponents, same shuffles: the original against the weakened copy.
    let rate = |deck: &Prepared| {
        let mut rng = Rng::new(11);
        let mut wins = 0;
        for k in 0..n {
            let seat = k % 4;
            let mut table: Vec<&Prepared> = vec![&decks[1], &decks[2], &decks[3]];
            table.insert(seat, deck);
            if play_game(&table, &rules, &mut rng) == seat {
                wins += 1;
            }
        }
        wins as f32 / n as f32
    };
    let (strong_wr, weak_wr) = (rate(me), rate(&weak));
    assert!(
        strong_wr - weak_wr >= 0.05,
        "original {:.2} vs weakened {:.2}",
        strong_wr,
        weak_wr
    );
}

#[test]
fn instant_speed_plays_at_end_of_turn_and_in_combat() {
    let d = prep(&[
        Card::basic_land("Forest", 'G'),
        card(
            "Ambusher",
            "{1}{G}",
            "Creature — Cat",
            "Flash",
            Some(("3", "2")),
        ),
        card(
            "Bolt",
            "{G}",
            "Instant",
            "Bolt deals 4 damage to target creature an opponent controls.",
            None,
        ),
        card("Giant", "{4}{G}", "Creature — Giant", "", Some(("4", "4"))),
    ]);
    // A flash creature is cast at the end of the turn before ours.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 1, 3, 'G');
    g.players[1].hand.push(1);
    ai::end_of_turn(&mut g, 1, true);
    assert!(
        g.perms
            .iter()
            .any(|x| x.ctrl == 1 && x.card == Some((1, 1))),
        "flash creature cast at end of turn"
    );
    // Instant removal on a big attacker.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 1, 1, 'G');
    g.players[1].hand.push(2);
    let giant = put(&mut g, 0, 3);
    let at = vec![Attack {
        attacker: giant,
        def: Def::Player(1),
        blockers: vec![],
        blocked: false,
    }];
    ai::before_blocks(&mut g, 1, &at);
    assert!(g.idx(giant).is_none(), "removal on the attacker");
}
