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

fn attack_on(attackers: &[u32], d: usize) -> Vec<Attack> {
    attackers
        .iter()
        .map(|&a| Attack {
            attacker: a,
            def: Def::Player(d),
            blockers: vec![],
            blocked: false,
        })
        .collect()
}

#[test]
fn ai_makes_free_blocks() {
    let d = prep(&[
        card("Wall", "{2}{W}", "Creature — Wall", "", Some(("1", "12"))),
        card("Brute", "{3}{R}", "Creature — Ogre", "", Some(("3", "6"))),
    ]);
    let mut g = game(&[&d, &d]);
    let brute = put(&mut g, 0, 1);
    let wall = put(&mut g, 1, 0);
    let blocks = ai::declare_blocks(&g, 1, &attack_on(&[brute], 1));
    assert_eq!(
        blocks,
        vec![(wall, brute)],
        "the 1/12 stops the 3/6 for free"
    );
}

#[test]
fn ai_blocks_the_big_hitters_first_when_the_swing_is_lethal() {
    let d = prep(&[
        card("Giant", "{5}{R}", "Creature — Giant", "", Some(("7", "7"))),
        card("Ogre", "{4}{R}", "Creature — Ogre", "", Some(("6", "6"))),
        card("Rat", "{B}", "Creature — Rat", "", Some(("1", "1"))),
        card("Bear", "{1}{G}", "Creature — Bear", "", Some(("2", "2"))),
    ]);
    let mut g = game(&[&d, &d]);
    let giant = put(&mut g, 0, 0);
    let ogre = put(&mut g, 0, 1);
    let rat1 = put(&mut g, 0, 2);
    let rat2 = put(&mut g, 0, 2);
    put(&mut g, 1, 3);
    put(&mut g, 1, 3);
    g.players[1].life = 13;
    // Eating the two 1/1s with the bears (the value blocks) would let 13 through.
    let at = attack_on(&[giant, ogre, rat1, rat2], 1);
    let blocks = ai::declare_blocks(&g, 1, &at);
    let through: i32 = [(giant, 7), (ogre, 6), (rat1, 1), (rat2, 1)]
        .iter()
        .filter(|(a, _)| !blocks.iter().any(|(_, x)| x == a))
        .map(|(_, p)| *p)
        .sum();
    assert!(
        through < 13,
        "survives: {} gets through ({:?})",
        through,
        blocks
    );
}

#[test]
fn ai_chumps_to_stay_out_of_danger() {
    let d = prep(&[
        card("Ogre", "{4}{R}", "Creature — Ogre", "", Some(("6", "6"))),
        card("Bear", "{1}{G}", "Creature — Bear", "", Some(("2", "2"))),
    ]);
    let mut g = game(&[&d, &d]);
    let ogre = put(&mut g, 0, 0);
    let bear = put(&mut g, 1, 1);
    let at = attack_on(&[ogre], 1);
    assert!(
        ai::declare_blocks(&g, 1, &at).is_empty(),
        "no chump at 40 life"
    );
    g.players[1].life = 8;
    assert_eq!(
        ai::declare_blocks(&g, 1, &at),
        vec![(bear, ogre)],
        "at 8 life the 2/2 chumps rather than going to 2"
    );
}

#[test]
fn ai_sends_only_what_lethal_needs_when_others_remain() {
    let d = prep(&[
        card("Bear", "{1}{G}", "Creature — Bear", "", Some(("3", "3"))),
        card("Giant", "{5}{R}", "Creature — Giant", "", Some(("7", "7"))),
    ]);
    let mut g = game(&[&d, &d, &d]);
    for _ in 0..4 {
        put(&mut g, 0, 0);
    }
    g.players[1].life = 3;
    for _ in 0..2 {
        put(&mut g, 2, 1);
    }
    let attacks = ai::declare_attacks(&g, 0);
    let at_p1 = attacks.iter().filter(|(_, d)| *d == Def::Player(1)).count();
    assert!(
        (1..=2).contains(&at_p1),
        "one bear kills P1, plus a spare: {:?}",
        attacks
    );
}

#[test]
fn ai_keeps_defenders_home_against_a_lethal_crack_back() {
    let d = prep(&[
        card("Bear", "{1}{G}", "Creature — Bear", "", Some(("2", "2"))),
        card("Giant", "{5}{R}", "Creature — Giant", "", Some(("6", "6"))),
    ]);
    let mut g = game(&[&d, &d, &d]);
    let bears = [put(&mut g, 0, 0), put(&mut g, 0, 0)];
    for _ in 0..4 {
        put(&mut g, 2, 1);
    }
    // P1 is open; P2's four 6/6s could swing back at our 10 life.
    g.players[0].life = 10;
    let attacks = ai::declare_attacks(&g, 0);
    assert!(
        attacks.len() < bears.len(),
        "a bear stays home: {:?}",
        attacks
    );
    g.players[0].life = 40;
    let attacks = ai::declare_attacks(&g, 0);
    assert_eq!(attacks.len(), 2, "at 40 life both attack the open player");
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
    let bears = prep(&[card(
        "Bear",
        "{1}{G}",
        "Creature — Bear",
        "",
        Some(("2", "2")),
    )]);
    let mut g = game(&[&d, &bears]);
    // (Academic Ascent's "target creature" needs a creature to target.)
    put(&mut g, 1, 0);
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
    g.perms[i].entered_turn = false;
    let later = g.activation_cost(0, i, k).unwrap().total();
    assert_eq!(
        later,
        today + 3,
        "power-up is {{3}} cheaper the turn Thanos entered"
    );
}

#[test]
fn thanos_power_up_needs_its_colours_and_picks_the_better_parity() {
    let db = db();
    let d = prep_db(
        &db,
        &[
            "Thanos, the Mad Titan",
            "Island",
            "Forest",
            "Cryptic Caves",
            "K'un-Lun Warrior",
            "Madame Masque",
            "Misty Knight, Hero for Hire",
            "Swamp",
        ],
    );
    let mut g = game(&[&d, &d]);
    // Me: Thanos (3) and K'un-Lun Warrior (2). Opponent: Madame Masque (5), Misty Knight (2).
    let thanos = put(&mut g, 0, 0);
    let kunlun = put(&mut g, 0, 4);
    let masque = put(&mut g, 1, 5);
    let misty = put(&mut g, 1, 6);
    let i = g.idx(thanos).unwrap();
    let k = g
        .abilities_of(&g.perms[i])
        .iter()
        .position(|a| matches!(a.trig, Trig::PowerUp(_)))
        .unwrap();
    // The turn he enters it costs {C}{U}{G}: black mana alone can't pay it.
    lands(&mut g, 0, 3, 'B');
    assert!(
        !g.activate(0, thanos, k),
        "needs colourless, blue and green"
    );
    put(&mut g, 0, 1);
    put(&mut g, 0, 2);
    put(&mut g, 0, 3);
    assert!(g.activate(0, thanos, k));
    g.run_pending();
    g.sba();
    // Odd kills only the opponent's Madame Masque; even would cost my Warrior for their Misty.
    assert!(g.idx(masque).is_none(), "odd-cost Madame Masque destroyed");
    assert!(g.idx(kunlun).is_some(), "my even-cost creature spared");
    assert!(g.idx(misty).is_some(), "even-cost creatures spared");
    assert!(
        g.idx(thanos).is_some(),
        "\"each other creature\" spares Thanos"
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

// ------------------------------------------------------------ costs and conditions

/// Fires ability `k` of permanent `id` as if its trigger happened.
fn fire(g: &mut Game, id: u32, k: usize) {
    let i = g.idx(id).expect("on the battlefield");
    let p = g.perms[i].ctrl;
    g.pending.push(Pending {
        ctrl: p,
        source: Some(id),
        card: g.perms[i].card.unwrap(),
        back: false,
        ability: k as u16,
        snapshot: None,
        subject: None,
        teamwork: false,
        gift: false,
        from_graveyard: false,
        kicked: false,
        overloaded: false,
        x: None,
    });
    g.run_pending();
    g.sba();
}

/// The first ability of permanent `id` that `f` picks.
fn ability_of(g: &Game, id: u32, f: impl Fn(&SimAbility) -> bool) -> usize {
    let i = g.idx(id).expect("on the battlefield");
    g.abilities_of(&g.perms[i])
        .iter()
        .position(f)
        .expect("ability")
}

fn token(g: &mut Game, p: usize, power: i32) -> u32 {
    let i = g.make_token(p, power, power, 0, vec![]);
    g.perms[i].sick = false;
    g.perms[i].id
}

#[test]
fn additional_costs_to_cast_are_paid() {
    let db = db();
    let d = prep_db(
        &db,
        &[
            "Swamp",
            "Eaten Alive",
            "Mountain",
            "Thrill of Possibility",
            "Arbiter of Woe",
        ],
    );
    // Eaten Alive: "sacrifice a creature or pay {3}{B}".
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 2, 'B');
    let foe = put(&mut g, 1, 4);
    g.players[0].hand.push(1);
    assert!(
        !g.can_cast(0, Src::Hand(0), CastOpts::default()),
        "{{B}} and {{3}}{{B}} with two Swamps and nothing to sacrifice"
    );
    let spare = token(&mut g, 0, 1);
    assert_eq!(
        g.cast_cost(0, Src::Hand(0), CastOpts::default())
            .unwrap()
            .total(),
        1,
        "a spare token pays the additional cost"
    );
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()));
    assert!(g.idx(spare).is_none(), "the token was sacrificed");
    assert!(g.idx(foe).is_none(), "and the creature exiled");
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 5, 'B');
    put(&mut g, 1, 4);
    g.players[0].hand.push(1);
    assert_eq!(
        g.cast_cost(0, Src::Hand(0), CastOpts::default())
            .unwrap()
            .total(),
        5,
        "no creature to spare: {{B}} plus {{3}}{{B}}"
    );
    // Thrill of Possibility: discard a card, draw two.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 2, 'R');
    g.players[0].hand = vec![3];
    assert!(
        !g.can_cast(0, Src::Hand(0), CastOpts::default()),
        "nothing to discard"
    );
    g.players[0].hand = vec![3, 0];
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()));
    assert_eq!(g.players[0].hand.len(), 2, "one discarded, two drawn");
    assert!(
        g.players[0].graveyard.contains(&0),
        "the Swamp was discarded"
    );
    // Arbiter of Woe: "sacrifice a creature" is required.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 6, 'B');
    g.players[0].hand.push(4);
    assert!(!g.can_cast(0, Src::Hand(0), CastOpts::default()));
    let spare = token(&mut g, 0, 1);
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()));
    assert!(g.idx(spare).is_none());
    assert!(g.perms.iter().any(|x| x.card == Some((0, 4))));
}

#[test]
fn costs_paid_as_abilities_resolve() {
    let db = db();
    let d = prep_db(
        &db,
        &[
            "Swamp",
            "Crossway Troublemakers",
            "Vampire Gourmand",
            "Killmonger, Scourge of Wakanda",
            "Lich's Relic",
            "Guiding Hydra",
            "Fishing Pole",
        ],
    );
    // Crossway Troublemakers: "you may pay 2 life. If you do, draw a card."
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 1);
    let v = put(&mut g, 0, 2);
    let i = g.idx(v).unwrap();
    g.destroy(i);
    g.run_pending();
    assert_eq!(g.players[0].life, 28, "paid 2 life");
    assert_eq!(g.players[0].hand.len(), 1, "and drew");
    // At low life it isn't worth it: no payment, no card.
    g.players[0].life = 6;
    let v = put(&mut g, 0, 2);
    let i = g.idx(v).unwrap();
    g.destroy(i);
    g.run_pending();
    assert_eq!(
        (g.players[0].life, g.players[0].hand.len()),
        (6, 1),
        "declined at 6 life"
    );
    // Killmonger: "you may sacrifice another creature. When you do, destroy ...".
    let mut g = game(&[&d, &d]);
    let foe = put(&mut g, 1, 1);
    put(&mut g, 0, 3);
    assert!(
        g.idx(foe).is_some(),
        "nothing sacrificed, nothing destroyed"
    );
    let mut g = game(&[&d, &d]);
    let foe = put(&mut g, 1, 1);
    let spare = token(&mut g, 0, 1);
    put(&mut g, 0, 3);
    assert!(g.idx(spare).is_none(), "the token was sacrificed");
    assert!(
        g.idx(foe).is_none(),
        "so the opponent's creature is destroyed"
    );
    // Lich's Relic: "you may pay {2}. When you do, destroy ...".
    let mut g = game(&[&d, &d]);
    let foe = put(&mut g, 1, 1);
    put(&mut g, 0, 4);
    assert!(g.idx(foe).is_some(), "no mana, no destruction");
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 2, 'B');
    let foe = put(&mut g, 1, 1);
    put(&mut g, 0, 4);
    assert!(g.idx(foe).is_none());
    assert!(
        g.perms.iter().filter(|x| x.is(T_LAND) && x.tapped).count() == 2,
        "{{2}} was paid"
    );
    // Guiding Hydra: "you may remove a +1/+1 counter from it. If you do, put a +1/+1
    // counter on each other creature you control."
    let mut g = game(&[&d, &d]);
    let i = g.enter_card(0, 5, 0, false).unwrap();
    g.perms[i].counters = 3;
    let hydra = g.perms[i].id;
    g.pending.clear();
    let other = put(&mut g, 0, 1);
    let k = ability_of(&g, hydra, |a| matches!(a.trig, Trig::Phase { .. }));
    fire(&mut g, hydra, k);
    assert_eq!(perm(&g, hydra).counters, 2, "the Hydra paid a counter");
    assert_eq!(perm(&g, other).counters, 1, "the other creature got one");
    // Fishing Pole: "remove a bait counter from it. If you do, create a Fish."
    let mut g = game(&[&d, &d]);
    let pole = put(&mut g, 0, 6);
    let k = ability_of(&g, pole, |a| matches!(a.trig, Trig::Event(..)));
    fire(&mut g, pole, k);
    assert!(
        !g.perms.iter().any(|x| x.tok == Tok::Creature),
        "no bait, no Fish"
    );
    let i = g.idx(pole).unwrap();
    g.perms[i].other_counters = 1;
    fire(&mut g, pole, k);
    assert!(g.perms.iter().any(|x| x.tok == Tok::Creature), "a Fish");
    assert_eq!(perm(&g, pole).other_counters, 0, "the bait counter is gone");
}

#[test]
fn activated_ability_costs_are_paid() {
    let db = db();
    let d = prep_db(
        &db,
        &[
            "Swamp",
            "Drake Hatcher",
            "Jessica Jones, Private Eye",
            "Lathril, Blade of the Elves",
            "Feldon's Cane",
            "Suspicious Shambler",
            "Gallia, Tragic Host",
            "Island",
            "Bruce Banner",
            "Myojin of Night's Reach",
        ],
    );
    let act = |a: &SimAbility| matches!(a.trig, Trig::Activated { .. });
    // Drake Hatcher: "Remove three incubation counters from it: create a Drake."
    let mut g = game(&[&d, &d]);
    let hatcher = put(&mut g, 0, 1);
    let k = ability_of(&g, hatcher, act);
    let i = g.idx(hatcher).unwrap();
    assert!(
        g.activation_cost(0, i, k).is_none(),
        "no counters to remove"
    );
    g.perms[i].other_counters = 3;
    assert!(g.activate(0, hatcher, k));
    assert_eq!(perm(&g, hatcher).other_counters, 0);
    assert!(g.perms.iter().any(|x| x.tok == Tok::Creature && x.p == 2));
    // Jessica Jones: "{T}, put a stun counter on her".
    let jess = put(&mut g, 0, 2);
    let k = ability_of(&g, jess, act);
    assert!(g.activate(0, jess, k));
    assert_eq!(perm(&g, jess).stun, 1);
    // Lathril: "{T}, tap ten untapped Elves you control".
    let lathril = put(&mut g, 0, 3);
    let k = ability_of(&g, lathril, act);
    let i = g.idx(lathril).unwrap();
    assert!(g.activation_cost(0, i, k).is_none(), "no ten Elves");
    // Feldon's Cane: "{T}, exile it".
    let cane = put(&mut g, 0, 4);
    let k = ability_of(&g, cane, act);
    assert!(g.activate(0, cane, k));
    assert!(g.idx(cane).is_none() && g.players[0].exile.contains(&4));
    // Suspicious Shambler works only from the graveyard, exiling itself.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 6, 'B');
    let shambler = put(&mut g, 0, 5);
    let k = ability_of(&g, shambler, act);
    let i = g.idx(shambler).unwrap();
    assert!(
        g.activation_cost(0, i, k).is_none(),
        "not from the battlefield"
    );
    g.leave(i, state::Zone::Graveyard, false);
    let gk = g.players[0].graveyard.iter().position(|&c| c == 5).unwrap();
    assert!(g.activate_from_graveyard(0, gk, k));
    assert!(g.players[0].exile.contains(&5), "exiled as the cost");
    assert_eq!(
        g.perms.iter().filter(|x| x.tok == Tok::Creature).count(),
        2,
        "two Zombies"
    );
    // Gallia: "{4}{B}, exile another creature card from your graveyard: return her".
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 5, 'B');
    g.players[0].graveyard = vec![6];
    assert!(
        g.graveyard_activation_cost(0, 0, 1).is_none(),
        "no other creature card to exile"
    );
    g.players[0].graveyard = vec![6, 2];
    let k = d.cards[6]
        .abilities
        .iter()
        .position(|a| matches!(a.trig, Trig::Activated { .. }))
        .unwrap();
    assert!(g.activate_from_graveyard(0, 0, k));
    let gallia = g.perms.iter().find(|x| x.card == Some((0, 6))).unwrap();
    assert!(gallia.tapped && gallia.counters == 1);
    assert_eq!(g.players[0].exile, vec![2]);
    // Bruce Banner: "{X}{X}, {T}: Draw X cards" - four lands make X = 2.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 4, 'U');
    let bruce = put(&mut g, 0, 8);
    let k = ability_of(&g, bruce, act);
    assert!(g.activate(0, bruce, k));
    assert_eq!(g.players[0].hand.len(), 2);
    // Myojin of Night's Reach: cast from hand, it has a divinity counter (and so
    // indestructible); removing it makes each opponent discard their hand.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 8, 'B');
    g.players[0].hand.push(9);
    g.players[1].hand = vec![0, 0, 0];
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()));
    let myojin = g.perms.iter().find(|x| x.card == Some((0, 9))).unwrap().id;
    assert_eq!(perm(&g, myojin).other_counters, 1);
    assert!(perm(&g, myojin).has(INDESTRUCTIBLE));
    let k = ability_of(&g, myojin, act);
    assert!(g.activate(0, myojin, k));
    assert!(g.players[1].hand.len() < 3, "the opponent discarded");
    assert!(!perm(&g, myojin).has(INDESTRUCTIBLE));
    let i = g.idx(myojin).unwrap();
    assert!(g.activation_cost(0, i, k).is_none(), "one use");
}

#[test]
fn conditions_are_checked_not_guessed() {
    let db = db();
    let d = prep_db(
        &db,
        &[
            "Mountain",
            "Hidetsugu's Second Rite",
            "Burst Lightning",
            "Island",
            "Finale of Revelation",
            "Knight of Grace",
            "Kitesail Corsair",
            "Leonin Vanguard",
            "Garna, Bloodfist of Keld",
            "Agent 13, Sharon Carter",
            "Arbiter of Woe",
        ],
    );
    let bear = prep(&[
        card("Bear", "{1}{G}", "Creature — Bear", "", Some(("2", "2"))),
        card(
            "Knight",
            "{2}{W}{W}",
            "Creature — Knight",
            "",
            Some(("4", "4")),
        ),
    ]);
    // Hidetsugu's Second Rite: only the player at exactly 10 life.
    let mut g = game(&[&d, &bear, &bear]);
    lands(&mut g, 0, 4, 'R');
    g.players[1].life = 5;
    g.players[2].life = 10;
    g.players[0].hand.push(1);
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()));
    g.sba();
    assert_eq!(g.players[1].life, 5);
    assert!(!g.players[2].alive, "10 damage to the player at 10");
    let mut g = game(&[&d, &bear]);
    lands(&mut g, 0, 4, 'R');
    g.players[0].hand.push(1);
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()));
    assert_eq!(g.players[1].life, 30, "nobody at 10 life: nothing");
    // Burst Lightning: 2 damage, or 4 if kicked.
    let mut g = game(&[&d, &bear]);
    lands(&mut g, 0, 5, 'R');
    let b = put(&mut g, 1, 0);
    let k = put(&mut g, 1, 1);
    g.players[0].hand.push(2);
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()));
    assert!(
        g.idx(b).is_none() && g.idx(k).is_some(),
        "2 damage kills the Bear"
    );
    g.players[0].hand.push(2);
    for x in g.perms.iter_mut() {
        x.tapped = false;
    }
    let kicked = CastOpts {
        kicker: true,
        ..Default::default()
    };
    assert!(g.cast(0, Src::Hand(0), kicked));
    assert!(g.idx(k).is_none(), "kicked, 4 damage kills the Knight");
    // Finale of Revelation with X = 2: draws two, not four.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 6, 'U');
    g.players[0].hand.push(4);
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()));
    assert_eq!(g.players[0].hand.len(), 2);
    // Knight of Grace: +1/+0 only while someone controls a black permanent.
    let mut g = game(&[&d, &d]);
    let knight = put(&mut g, 0, 5);
    assert_eq!(perm(&g, knight).p, 2);
    put(&mut g, 1, 10);
    g.recalc();
    assert_eq!(perm(&g, knight).p, 3, "Arbiter of Woe is black");
    // Kitesail Corsair flies only while attacking.
    let corsair = put(&mut g, 0, 6);
    assert!(!perm(&g, corsair).has(FLYING));
    let i = g.idx(corsair).unwrap();
    g.perms[i].attacking = true;
    g.recalc();
    assert!(perm(&g, corsair).has(FLYING));
    // Leonin Vanguard: only with three or more creatures.
    let mut g = game(&[&d, &d]);
    let vanguard = put(&mut g, 0, 7);
    let k = ability_of(&g, vanguard, |a| matches!(a.trig, Trig::Phase { .. }));
    fire(&mut g, vanguard, k);
    assert_eq!(g.players[0].life, 30, "one creature: nothing");
    put(&mut g, 0, 5);
    put(&mut g, 0, 5);
    fire(&mut g, vanguard, k);
    assert_eq!(g.players[0].life, 31, "three creatures: +1/+1 and 1 life");
    // Garna: draw if the creature that died was attacking, otherwise 1 damage to each
    // opponent.
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 8);
    let a = put(&mut g, 0, 5);
    let i = g.idx(a).unwrap();
    g.perms[i].attacking = true;
    g.destroy(i);
    g.run_pending();
    assert_eq!((g.players[0].hand.len(), g.players[1].life), (1, 30));
    let b = put(&mut g, 0, 5);
    let i = g.idx(b).unwrap();
    g.destroy(i);
    g.run_pending();
    assert_eq!((g.players[0].hand.len(), g.players[1].life), (1, 29));
    // Agent 13: "whenever a creature you control attacks alone".
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 9);
    let a = put(&mut g, 0, 5);
    let b = put(&mut g, 0, 5);
    for id in [a, b] {
        let i = g.idx(id).unwrap();
        g.perms[i].attacking = true;
    }
    g.emit(Ev::OthersAttack, 0, None);
    g.run_pending();
    assert!(!g.perms.iter().any(|x| x.tok == Tok::Clue), "two attackers");
    let i = g.idx(b).unwrap();
    g.perms[i].attacking = false;
    g.emit(Ev::OthersAttack, 0, Some(a));
    g.run_pending();
    assert!(
        g.perms.iter().any(|x| x.tok == Tok::Clue),
        "one attacker: a Clue"
    );
}

#[test]
fn ward_costs_life_or_a_card() {
    let db = db();
    let d0 = prep_db(&db, &["Zul Ashur, Lich Lord", "Gideon the Oathless"]);
    let d1 = prep(&[
        Card::basic_land("Swamp", 'B'),
        card(
            "Murder",
            "{1}{B}{B}",
            "Instant",
            "Destroy target creature.",
            None,
        ),
    ]);
    // Zul Ashur: "Ward—Pay 2 life".
    let mut g = game(&[&d0, &d1]);
    let zul = put(&mut g, 0, 0);
    lands(&mut g, 1, 3, 'B');
    g.players[1].hand.push(1);
    assert!(g.cast(1, Src::Hand(0), CastOpts::default()));
    assert!(g.idx(zul).is_none());
    assert_eq!(g.players[1].life, 28, "ward took 2 life");
    // Gideon: "Ward—Discard a card": with an empty hand he can't be targeted.
    let mut g = game(&[&d0, &d1]);
    let gideon = put(&mut g, 0, 1);
    lands(&mut g, 1, 3, 'B');
    g.players[1].hand.push(1);
    assert!(g.cast(1, Src::Hand(0), CastOpts::default()));
    assert!(g.idx(gideon).is_some(), "no card to discard for ward");
}

#[test]
fn self_reductions_equip_flashback_and_modes() {
    let db = db();
    let d = prep_db(
        &db,
        &[
            "Forest",
            "Ghalta the Unstoppable",
            "Island",
            "Falcon's Wing Harness",
            "My Precious",
            "Twinned Vision",
            "Swamp",
            "Vraska's Final Mercy",
            "Hulk, Gamma Goliath",
            "Viv Vision, Teen Synthezoid",
        ],
    );
    let big = prep(&[card(
        "Wurm",
        "{5}{G}",
        "Creature — Wurm",
        "",
        Some(("6", "6")),
    )]);
    // Ghalta the Unstoppable costs {X} less, X = the greatest power among your creatures.
    let mut g = game(&[&d, &big]);
    g.players[0].hand.push(1);
    let full = g
        .cast_cost(0, Src::Hand(0), CastOpts::default())
        .unwrap()
        .total();
    let i = g.enter_card(1, 0, 0, false).unwrap();
    g.perms[i].sick = false;
    let less = g
        .cast_cost(0, Src::Hand(0), CastOpts::default())
        .unwrap()
        .total();
    assert_eq!((full, less), (9, 3), "a 6/6 makes it cost 6 less");
    // Hulk: other creatures' power-ups cost {3} less ({7} -> {4}); spells don't.
    let mut g = game(&[&d, &d]);
    let viv = put(&mut g, 0, 9);
    let i = g.idx(viv).unwrap();
    g.perms[i].entered_turn = false;
    let k = ability_of(&g, viv, |a| matches!(a.trig, Trig::PowerUp(_)));
    assert_eq!(g.activation_cost(0, i, k).unwrap().total(), 7);
    put(&mut g, 0, 8);
    let i = g.idx(viv).unwrap();
    assert_eq!(g.activation_cost(0, i, k).unwrap().total(), 4);
    assert!(
        d.cards[8].cost_reduction.is_none() && d.cards[8].power_up_less == 3,
        "creature spells aren't 3 cheaper"
    );
    // Falcon's Wing Harness: Equip {2}{U} needs blue.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 3, 'G');
    let harness = put(&mut g, 0, 3);
    let w = put(&mut g, 0, 1);
    let i = g.idx(harness).unwrap();
    g.perms[i].attached = None;
    assert!(!g.equip(0, harness, w), "no blue mana");
    lands(&mut g, 0, 1, 'U');
    assert!(g.equip(0, harness, w));
    // My Precious: "Equip—{2}, Pay 2 life".
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 2, 'G');
    let ring = put(&mut g, 0, 4);
    let w = put(&mut g, 0, 1);
    assert!(g.equip(0, ring, w));
    assert_eq!(g.players[0].life, 28);
    // Twinned Vision's flashback: "{1}{U/R}{U/R}, Discard a card"; from the graveyard it
    // draws two.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 3, 'U');
    g.players[0].graveyard = vec![5];
    assert!(
        !g.can_cast(0, Src::Graveyard(0), CastOpts::default()),
        "nothing to discard"
    );
    g.players[0].hand = vec![0];
    assert!(g.cast(0, Src::Graveyard(0), CastOpts::default()));
    assert_eq!(g.players[0].hand.len(), 2, "discarded one, drew two");
    assert!(g.players[0].exile.contains(&5));
    // Vraska's Final Mercy: each mode is "you lose 2 life" and its effect.
    let mut g = game(&[&d, &big]);
    lands(&mut g, 0, 2, 'B');
    let wurm = g.enter_card(1, 0, 1, false).map(|i| g.perms[i].id).unwrap();
    g.players[0].hand.push(7);
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()));
    assert!(g.idx(wurm).is_none());
    assert_eq!(g.players[0].life, 28);
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

// ------------------------------------------------------------ card readings (league cards)

/// Resolves ability `k` of card `idx` (player 0's list) as if it triggered or was activated
/// from permanent `src` (None: from nowhere / the graveyard).
fn fire_card(g: &mut Game, idx: u16, k: usize, src: Option<u32>) {
    g.pending.push(Pending {
        ctrl: 0,
        source: src,
        card: (0, idx),
        back: false,
        ability: k as u16,
        snapshot: None,
        subject: None,
        teamwork: false,
        gift: false,
        from_graveyard: false,
        kicked: false,
        overloaded: false,
        x: None,
    });
    g.run_pending();
    g.sba();
}

/// The index of the first ability of card `idx` whose text contains `what`.
fn ability(d: &Prepared, idx: u16, what: &str) -> usize {
    d.cards[idx as usize]
        .abilities
        .iter()
        .position(|a| a.text.contains(what))
        .unwrap_or_else(|| panic!("no ability with {:?}", what))
}

#[test]
fn cards_that_return_themselves_from_the_graveyard() {
    let db = db();
    let d = prep_db(
        &db,
        &[
            "Swamp",
            "Reassembling Skeleton",
            "Hellcat, Undying Vigilante",
            "Heroes' Bane",
            "Gallia, Tragic Host",
        ],
    );
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 5, 'B');
    // Reassembling Skeleton: "{1}{B}: Return ~ from your graveyard to the battlefield
    // tapped" - from the graveyard only, and it brings back itself, not the best creature.
    let skel = put(&mut g, 0, 1);
    let i = g.idx(skel).unwrap();
    let k = ability(&d, 1, "from your graveyard");
    assert!(
        g.activation_cost(0, i, k).is_none(),
        "not activated on the battlefield"
    );
    g.destroy(i);
    g.players[0].graveyard.push(3); // a better creature card (Heroes' Bane) is there too
    let gy = g.players[0].graveyard.iter().position(|&c| c == 1).unwrap();
    assert!(g.graveyard_activation_cost(0, gy, k).is_some());
    assert!(g.activate_from_graveyard(0, gy, k));
    let back = g
        .perms
        .iter()
        .find(|x| x.card == Some((0, 1)))
        .expect("Skeleton is back");
    assert!(back.tapped, "it returns tapped");
    assert_eq!(g.players[0].graveyard, vec![3], "Heroes' Bane stays put");
    // Gallia needs another creature card to exile as part of the cost.
    g.players[0].graveyard = vec![4];
    let k = ability(&d, 4, "from your graveyard");
    assert!(
        g.graveyard_activation_cost(0, 0, k).is_none(),
        "no other creature card to exile"
    );
    // Hellcat: dies -> returns with a +1/+1 counter, no abilities but haste; the second time
    // she stays dead.
    let cat = put(&mut g, 0, 2);
    g.destroy(g.idx(cat).unwrap());
    g.run_pending();
    g.sba();
    let i = g
        .perms
        .iter()
        .position(|x| x.card == Some((0, 2)))
        .expect("Hellcat returned");
    let x = &g.perms[i];
    assert_eq!((x.p, x.t), (3, 3));
    assert!(x.no_abilities && x.has(HASTE));
    g.destroy(i);
    g.run_pending();
    g.sba();
    assert!(
        !g.perms.iter().any(|x| x.card == Some((0, 2))),
        "without her abilities she doesn't come back again"
    );
}

#[test]
fn spell_copies_and_token_copies() {
    let db = db();
    let d = prep_db(
        &db,
        &[
            "Mountain",
            "Pyromancer's Goggles",
            "Lightning Strike",
            "Electroduplicate",
            "Guttersnipe",
            "Loki Laufeyson",
        ],
    );
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 3, 'R');
    // Pyromancer's Goggles: its mana casting a red instant copies it (3 + 3 damage).
    put(&mut g, 0, 1);
    let life = g.players[1].life;
    g.players[0].hand.push(2);
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()));
    assert_eq!(g.players[1].life, life - 6, "Lightning Strike and its copy");
    // The Goggles' own ability makes no token copy of a creature.
    assert!(!g.perms.iter().any(|x| x.tok == Tok::Other));
    // Electroduplicate: a token copy of our creature, with haste, gone at the end step.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 3, 'R');
    let ogre = put(&mut g, 0, 4);
    g.players[0].hand.push(3);
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()));
    let copy = g
        .perms
        .iter()
        .find(|x| x.tok == Tok::Other && x.card == Some((0, 4)))
        .expect("a token copy of the creature");
    assert!(copy.has(HASTE) && copy.sac_at_end);
    assert_eq!(copy.p, perm(&g, ogre).p);
    // Loki: "when you next cast an instant or sorcery spell ... copy that spell".
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 3, 'R');
    let loki = put(&mut g, 0, 5);
    let k = ability(&d, 5, "next cast");
    fire_card(&mut g, 5, k, Some(loki));
    assert_eq!(g.players[0].copy_next, vec![2], "up to Loki's power");
    assert!(
        !g.perms.iter().any(|x| x.tok != Tok::None),
        "no token copy of a creature"
    );
}

#[test]
fn counters_on_entering_and_counted_power() {
    let db = db();
    let d = prep_db(
        &db,
        &[
            "Forest",
            "Heroes' Bane",
            "Guiding Hydra",
            "Crusader of Odric",
            "Tarmogoyf",
            "Blanchwood Armor",
            "Brass's Bounty",
        ],
    );
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 2, 'G');
    // "Enters with four +1/+1 counters" on a 0/0: it lives.
    let bane = put(&mut g, 0, 1);
    assert_eq!(perm(&g, bane).p, 4);
    // "Enters with X +1/+1 counters": the X paid.
    g.entering_x = Some(3);
    let hydra = put(&mut g, 0, 2);
    assert_eq!(perm(&g, hydra).p, 4, "1/0 with three counters");
    // Crusader of Odric: P/T = creatures you control (Bane, Hydra and itself).
    let crusader = put(&mut g, 0, 3);
    assert_eq!((perm(&g, crusader).p, perm(&g, crusader).t), (3, 3));
    // Tarmogoyf: card types among cards in all graveyards (a land and an instant: 2/3).
    g.players[1].graveyard = vec![0, 6];
    let goyf = put(&mut g, 0, 4);
    assert_eq!((perm(&g, goyf).p, perm(&g, goyf).t), (2, 3));
    // Blanchwood Armor: +1/+1 for each Forest you control (two).
    let ai = g.enter_card(0, 5, 0, false).unwrap();
    g.perms[ai].attached = Some(bane);
    g.sba();
    assert_eq!(perm(&g, bane).p, 6);
    // Brass's Bounty: a Treasure for each land you control.
    g.players[0].hand.push(6);
    for x in g.perms.iter_mut() {
        x.tapped = false;
    }
    lands(&mut g, 0, 5, 'G');
    let n_lands = g
        .perms
        .iter()
        .filter(|x| x.ctrl == 0 && x.is(T_LAND))
        .count();
    let k = ability(&d, 6, "for each land");
    fire_card(&mut g, 6, k, None);
    let treasures = g.perms.iter().filter(|x| x.tok == Tok::Treasure).count();
    assert_eq!(treasures, n_lands);
}

#[test]
fn end_step_sacrifice_and_untap_rules() {
    let db = db();
    let d = prep_db(
        &db,
        &["Mountain", "Ball Lightning", "Traxos, Scourge Eternal"],
    );
    let mut g = game(&[&d, &d]);
    let ball = put(&mut g, 0, 1);
    g.phase_triggers(0, Phase::End);
    assert!(
        g.idx(ball).is_none(),
        "sacrificed at the beginning of the end step"
    );
    // Traxos doesn't untap during its controller's untap step.
    let traxos = put(&mut g, 0, 2);
    let i = g.idx(traxos).unwrap();
    g.perms[i].tapped = true;
    g.take_turn(0, false);
    assert!(perm(&g, traxos).tapped);
}

#[test]
fn land_searches_and_lands_without_mana() {
    let db = db();
    let d = prep_db(&db, &["Forest", "Evolving Wilds", "Cultivate", "Island"]);
    let mut g = game(&[&d, &d]);
    g.players[0].library = vec![0, 3, 0, 3, 0];
    // Evolving Wilds makes no mana; it fetches a basic.
    let wilds = put(&mut g, 0, 1);
    assert_eq!(g.mana_available(0), 0, "no mana ability, no mana");
    let k = ability(&d, 1, "search your library");
    assert!(g.activate(0, wilds, k));
    assert_eq!(
        g.perms.iter().filter(|x| x.is(T_LAND)).count(),
        1,
        "Wilds sacrificed, a basic fetched"
    );
    // Cultivate: one basic onto the battlefield, the other into the hand.
    let hand = g.players[0].hand.len();
    let lands_before = g.perms.iter().filter(|x| x.is(T_LAND)).count();
    fire_card(&mut g, 2, 0, None);
    assert_eq!(g.players[0].hand.len(), hand + 1);
    assert_eq!(
        g.perms.iter().filter(|x| x.is(T_LAND)).count(),
        lands_before + 1
    );
}

#[test]
fn hands_life_and_the_game() {
    let db = db();
    let d = prep_db(
        &db,
        &[
            "Swamp",
            "Dragon Mage",
            "Demonic Pact",
            "Feed the Swarm",
            "Chart a Course",
            "Herald of Eternal Dawn",
        ],
    );
    let mut g = game(&[&d, &d]);
    // Dragon Mage: each player discards their hand, then draws seven.
    g.players[0].hand = vec![0, 0];
    g.players[1].hand = vec![0, 0, 0];
    let mage = put(&mut g, 0, 1);
    let k = ability(&d, 1, "discards their hand");
    fire_card(&mut g, 1, k, Some(mage));
    assert_eq!(g.players[0].hand.len(), 7);
    assert_eq!(g.players[1].hand.len(), 7);
    assert_eq!(
        g.players[1].graveyard.len(),
        3,
        "the old hand was discarded"
    );
    // Chart a Course: draw two, then discard one (no attack this turn).
    let (hand, gy) = (g.players[0].hand.len(), g.players[0].graveyard.len());
    fire_card(&mut g, 4, 0, None);
    assert_eq!(g.players[0].hand.len(), hand + 1);
    assert_eq!(g.players[0].graveyard.len(), gy + 1);
    // Feed the Swarm: you lose life equal to the destroyed permanent's mana value.
    let target = put(&mut g, 1, 1);
    let life = g.players[0].life;
    fire_card(&mut g, 3, 0, None);
    assert!(g.idx(target).is_none());
    assert_eq!(g.players[0].life, life - 7, "Dragon Mage's mana value");
    // Demonic Pact: each mode once ("deals 4 damage and you gain 4 life" is one mode); the
    // last one left loses the game.
    let pact = put(&mut g, 0, 2);
    let k = ability(&d, 2, "upkeep");
    assert_eq!(d.cards[2].abilities[k].mode_groups().len(), 4, "four modes");
    for _ in 0..3 {
        fire_card(&mut g, 2, k, Some(pact));
    }
    assert!(g.players[0].alive);
    let i = g.idx(pact).unwrap();
    assert_eq!(g.perms[i].modes_used, 0b0111, "the three good modes first");
    fire_card(&mut g, 2, k, Some(pact));
    assert!(!g.players[0].alive, "the fourth mode: you lose the game");
    // Herald of Eternal Dawn: you can't lose the game.
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 5);
    g.players[0].life = 0;
    g.sba();
    assert!(g.players[0].alive);
}

#[test]
fn trigger_timing_and_filters() {
    let db = db();
    let d = prep_db(
        &db,
        &[
            "Mountain",
            "Guttersnipe",
            "Lightning Strike",
            "Scrawling Crawler",
            "The Theorist, Jace Beleren",
            "Gardenize",
        ],
    );
    // Guttersnipe triggers on an instant (no "Orc" read in "sorcery").
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 2, 'R');
    put(&mut g, 0, 1);
    let life = g.players[1].life;
    g.players[0].hand.push(2);
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()));
    assert!(g.players[1].life <= life - 2, "Guttersnipe's 2 damage");
    // Scrawling Crawler: an opponent's draws cost them life, ours don't.
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 3);
    let (mine, theirs) = (g.players[0].life, g.players[1].life);
    g.draw(0, 1);
    g.draw(1, 1);
    g.run_pending();
    assert_eq!(g.players[0].life, mine);
    assert_eq!(g.players[1].life, theirs - 1);
    // The Theorist: "each opponent's draw step" - not on our own turn.
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 4);
    let hand = g.players[0].hand.len();
    g.phase_triggers(0, Phase::Upkeep);
    assert_eq!(g.players[0].hand.len(), hand);
    g.phase_triggers(1, Phase::Upkeep);
    assert_eq!(g.players[0].hand.len(), hand + 1);
    // Gardenize: "at the beginning of your first main phase", not the upkeep.
    let k = ability(&d, 5, "main phase");
    assert!(matches!(
        d.cards[5].abilities[k].trig,
        Trig::Phase {
            phase: Phase::Main,
            ..
        }
    ));
}

#[test]
fn cast_targets_discards_and_punishers() {
    let db = db();
    let d = prep_db(
        &db,
        &[
            "Mountain",
            "Plains",
            "Colleen Wing, Street Samurai",
            "Smaug's Fury",
            "Lightning Strike",
            "Titanbones, Towering Heart",
            "Painful Quandary",
        ],
    );
    // Colleen Wing: "whenever you cast a spell that targets a creature you control" - a pump
    // on her counts, Lightning Strike at the opponent doesn't.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 4, 'R');
    let colleen = put(&mut g, 0, 2);
    g.players[0].hand.push(3);
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()));
    g.run_pending();
    assert_eq!(perm(&g, colleen).counters, 1, "Smaug's Fury targets her");
    g.players[0].hand.push(4);
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()));
    g.run_pending();
    assert_eq!(perm(&g, colleen).counters, 1, "Lightning Strike doesn't");
    // Titanbones: "when you discard this card, you gain 3 life".
    g.players[0].hand = vec![5];
    let life = g.players[0].life;
    g.discard(0);
    g.run_pending();
    assert_eq!(g.players[0].life, life + 3);
    assert_eq!(g.players[0].graveyard.last(), Some(&5));
    // Painful Quandary: an opponent with no card to discard loses 5 life.
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 6);
    lands(&mut g, 1, 2, 'R');
    g.players[1].hand = vec![4];
    let life = g.players[1].life;
    assert!(g.cast(1, Src::Hand(0), CastOpts::default()));
    g.run_pending();
    assert_eq!(g.players[1].life, life - 5);
}

#[test]
fn resolution_counts_and_delayed_triggers() {
    let db = db();
    let d = prep_db(
        &db,
        &[
            "Swamp",
            "Plains",
            "Belladonna Took",
            "Nine-Lives Familiar",
            "The Eagles Are Coming!",
        ],
    );
    // Belladonna Took: life the first time, a card the second, counters the third.
    let mut g = game(&[&d, &d]);
    let bella = put(&mut g, 0, 2);
    let k = ability(&d, 2, "first time");
    let (life, hand) = (g.players[0].life, g.players[0].hand.len());
    fire_card(&mut g, 2, k, Some(bella));
    assert_eq!(
        (g.players[0].life, g.players[0].hand.len()),
        (life + 1, hand)
    );
    fire_card(&mut g, 2, k, Some(bella));
    assert_eq!(
        (g.players[0].life, g.players[0].hand.len()),
        (life + 1, hand + 1)
    );
    assert_eq!(perm(&g, bella).counters, 0);
    fire_card(&mut g, 2, k, Some(bella));
    assert_eq!(perm(&g, bella).counters, 1);
    assert_eq!(g.players[0].life, life + 1);
    // Nine-Lives Familiar: cast, it has eight revival counters; when it dies it comes back
    // at the next end step with seven.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 3, 'B');
    g.players[0].hand.push(3);
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()));
    let cat = g.perms.iter().position(|x| x.card == Some((0, 3))).unwrap();
    assert_eq!(g.perms[cat].other_counters, 8);
    g.destroy(cat);
    g.run_pending();
    g.sba();
    assert!(!g.perms.iter().any(|x| x.card == Some((0, 3))), "not yet");
    g.run_delayed(Phase::End);
    g.run_pending();
    let back = g
        .perms
        .iter()
        .find(|x| x.card == Some((0, 3)))
        .expect("back at the end step");
    assert_eq!(back.other_counters, 7);
    // The Eagles Are Coming!: the creature goes back to the hand now, a 4/4 flier arrives at
    // the next upkeep.
    let mut g = game(&[&d, &d]);
    let bella = put(&mut g, 0, 2);
    fire_card(&mut g, 4, 0, None);
    assert!(g.idx(bella).is_none());
    assert!(g.players[0].hand.contains(&2));
    assert!(!g.perms.iter().any(|x| x.tok != Tok::None));
    g.run_delayed(Phase::Upkeep);
    g.run_pending();
    assert!(
        g.perms
            .iter()
            .any(|x| x.tok != Tok::None && x.ctrl == 0 && x.p == 4 && x.has(FLYING))
    );
}

#[test]
fn life_thresholds_sacrifices_and_amass() {
    let db = db();
    let d = prep_db(
        &db,
        &[
            "Mountain",
            "Swamp",
            "Hidetsugu's Second Rite",
            "Liliana, Dreadhorde General",
            "Chief Warg's Company",
            "Azog, Moria's Ruin",
            "Tribute to Hunger",
            "Maelstrom Pulse",
        ],
    );
    // Hidetsugu's Second Rite: only at exactly 10 life.
    let mut g = game(&[&d, &d]);
    g.players[1].life = 11;
    fire_card(&mut g, 2, 0, None);
    assert_eq!(g.players[1].life, 11);
    g.players[1].life = 10;
    fire_card(&mut g, 2, 0, None);
    assert!(g.players[1].life <= 0);
    // Liliana -9: the opponent keeps one permanent of each type.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 1, 3, 'B');
    put(&mut g, 1, 4);
    put(&mut g, 1, 4);
    let lili = put(&mut g, 0, 3);
    let k = ability(&d, 3, "each permanent type");
    fire_card(&mut g, 3, k, Some(lili));
    let theirs = |g: &Game, t: u16| g.perms.iter().filter(|x| x.ctrl == 1 && x.is(t)).count();
    assert_eq!((theirs(&g, T_CREATURE), theirs(&g, T_LAND)), (1, 1));
    // Azog: the destroyed creature's controller amasses Goblins equal to its power; no card
    // for us (we didn't control it).
    let mut g = game(&[&d, &d]);
    let chief = put(&mut g, 1, 4);
    let hand = g.players[0].hand.len();
    put(&mut g, 0, 5);
    assert!(g.idx(chief).is_none());
    let army = g
        .perms
        .iter()
        .find(|x| x.ctrl == 1 && x.subtypes.iter().any(|s| s == "army"))
        .expect("their Army");
    assert_eq!(army.p, 5);
    assert_eq!(g.players[0].hand.len(), hand);
    // Tribute to Hunger: life equal to the sacrificed creature's toughness.
    let mut g = game(&[&d, &d]);
    let chief = put(&mut g, 1, 4);
    let life = g.players[0].life;
    fire_card(&mut g, 6, 0, None);
    assert!(g.idx(chief).is_none());
    assert_eq!(g.players[0].life, life + 3);
    // Maelstrom Pulse: everything with the same name goes too; the land stays.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 1, 1, 'B');
    put(&mut g, 1, 4);
    put(&mut g, 1, 4);
    fire_card(&mut g, 7, 0, None);
    assert_eq!((theirs(&g, T_CREATURE), theirs(&g, T_LAND)), (0, 1));
}

#[test]
fn spending_restrictions_cast_conditions_and_odd_costs() {
    let db = db();
    let d = prep_db(
        &db,
        &[
            "Forest",
            "Shang-Chi, Master of Kung Fu",
            "Swamp",
            "Proft, Sinister Mastermind",
            "Mjölnir, Hammer of Thor",
            "Colleen Wing, Street Samurai",
            "Key to the Side-Door",
            "Chief Warg's Company",
        ],
    );
    // Shang-Chi's mana: only for abilities of creature sources.
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 1);
    let chief = put(&mut g, 0, 7);
    let spell = Purpose::Spell {
        card: (0, 3),
        from_hand: true,
    };
    assert!(!g.can_pay_for(0, &Mana::generic(2), spell));
    assert!(g.can_pay_for(0, &Mana::generic(2), Purpose::Ability { src: chief }));
    // Proft, Sinister Mastermind: can't be cast without seven cards in the graveyard.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 3, 'B');
    g.players[0].hand = vec![3];
    assert!(!g.can_cast(0, Src::Hand(0), CastOpts::default()));
    g.players[0].graveyard = vec![0; 7];
    assert!(g.can_cast(0, Src::Hand(0), CastOpts::default()));
    // Mjölnir: "equip worthy {1}" - a legendary non-Villain red or white creature only.
    let mut g = game(&[&d, &d]);
    let mj = put(&mut g, 0, 4);
    let colleen = put(&mut g, 0, 5);
    let chief = put(&mut g, 0, 7);
    assert_eq!(g.equip_cost(0, mj, colleen), Some((Mana::generic(1), 0)));
    assert_eq!(g.equip_cost(0, mj, chief), None);
    // Key to the Side-Door: discard a legendary card with the same name as a legendary
    // permanent you control.
    let k = ability(&d, 6, "discard a legendary card");
    let text = &d.cards[6].abilities[k].text;
    g.players[0].hand = vec![5];
    assert_eq!(g.discard_for_cost(0, text), Some(Some(0)));
    g.players[0].hand = vec![3];
    assert_eq!(
        g.discard_for_cost(0, text),
        None,
        "no Proft on the battlefield"
    );
}

#[test]
fn attack_restrictions_and_granted_ward() {
    let db = db();
    let d = prep_db(&db, &["Forest", "Chief Warg's Company", "Super Strength"]);
    // Chief Warg's Company can't attack until there are two other Wolves (its upkeep tokens).
    let mut g = game(&[&d, &d]);
    let chief = put(&mut g, 0, 1);
    assert!(!g.can_attack(g.idx(chief).unwrap()));
    g.phase_triggers(0, Phase::Upkeep);
    assert!(!g.can_attack(g.idx(chief).unwrap()), "one Wolf");
    g.phase_triggers(0, Phase::Upkeep);
    assert!(g.can_attack(g.idx(chief).unwrap()), "two Wolves");
    // Super Strength: enchanted creature gets +4/+4 and has ward {1}.
    let ss = g.enter_card(0, 2, 0, false).unwrap();
    g.perms[ss].attached = Some(chief);
    g.sba();
    assert_eq!(perm(&g, chief).p, 9);
    assert_eq!(perm(&g, chief).ward, 1);
}

// ------------------------------------------------------------ mechanisms: transform, turns,
// other zones, cost modifiers, rule-changing statics, characteristics

fn untap_all(g: &mut Game) {
    for x in g.perms.iter_mut() {
        x.tapped = false;
    }
}

#[test]
fn transforming_cards_turn_into_their_back_face() {
    let db = db();
    let d = prep_db(
        &db,
        &[
            "Mountain",
            "Bruce Banner",
            "King T'Challa",
            "Jennifer Walters",
            "Lightning Strike",
            "Forest",
            "Plains",
            "Island",
        ],
    );
    let mut g = game(&[&d, &d]);
    // ({2}{R}{R}{G}{G} and {4}{W}{U}: the colours are paid as such.)
    lands(&mut g, 0, 4, 'R');
    lands(&mut g, 0, 2, 'G');
    let bruce = put(&mut g, 0, 1);
    // Only the front face is cast.
    g.players[0].hand.push(1);
    let back = CastOpts {
        back: true,
        ..Default::default()
    };
    assert!(g.cast_cost(0, Src::Hand(0), back).is_none());
    g.players[0].hand.clear();
    let k = ability(&d, 1, "transform");
    assert!(g.activate(0, bruce, k));
    let x = perm(&g, bruce);
    assert!(x.transformed);
    assert_eq!((x.p, x.t), (8, 8), "The Incredible Hulk is an 8/8");
    assert!(x.has(TRAMPLE) && x.has(REACH));
    // The Hulk's own abilities: enrage puts a +1/+1 counter on him.
    let i = g.idx(bruce).unwrap();
    g.damage_perm(i, 1, false);
    g.run_pending();
    assert_eq!(perm(&g, bruce).counters, 1);
    // Black Panther: double strike, and all damage to him is prevented.
    untap_all(&mut g);
    lands(&mut g, 0, 1, 'W');
    lands(&mut g, 0, 1, 'U');
    let tc = put(&mut g, 0, 2);
    let k = ability(&d, 2, "transform");
    assert!(g.activate(0, tc, k));
    let i = g.idx(tc).unwrap();
    assert!(g.perms[i].has(DOUBLE_STRIKE));
    g.damage_perm(i, 5, false);
    g.sba();
    assert_eq!(
        perm(&g, tc).damage,
        0,
        "damage to Black Panther is prevented"
    );
    // Jennifer Walters: her controller's opponents can't cast spells during their turn.
    put(&mut g, 0, 3);
    lands(&mut g, 1, 2, 'R');
    g.players[1].hand.push(4);
    g.active = 0;
    assert!(g.cast_cost(1, Src::Hand(0), CastOpts::default()).is_none());
    g.active = 1;
    assert!(g.cast_cost(1, Src::Hand(0), CastOpts::default()).is_some());
}

#[test]
fn extra_turns_and_ending_the_turn() {
    let db = db();
    let mut cards: Vec<Card> = ["Island", "Kang the Conqueror", "Time Stop"]
        .iter()
        .map(|n| db.get(n).unwrap().clone())
        .collect();
    cards.push(card(
        "Giant",
        "{4}{G}",
        "Creature — Giant",
        "",
        Some(("5", "5")),
    ));
    let d = prep(&cards);
    // Kang's power-up: an extra turn after this one, in which power-ups can't be activated.
    let mut g = game(&[&d, &d]);
    let kang = put(&mut g, 0, 1);
    let k = ability(&d, 1, "extra turn");
    fire_card(&mut g, 1, k, Some(kang));
    assert_eq!(g.players[0].extra_turns, 1);
    let library = g.players[0].library.len();
    g.take_turns(0, false);
    assert_eq!(
        g.players[0].library.len(),
        library - 2,
        "two turns, two draws"
    );
    assert_eq!(g.players[0].extra_turns, 0);
    let i = g.idx(kang).unwrap();
    g.perms[i].powered = false;
    g.players[0].no_powerups = true;
    assert!(g.activation_cost(0, i, k).is_none());
    // Time Stop in answer to a lethal attack: the turn ends, no damage is dealt.
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 3);
    lands(&mut g, 1, 6, 'U');
    g.players[1].hand.push(2);
    g.players[1].life = 4;
    g.active = 0;
    g.combat(0);
    assert!(g.end_turn, "Time Stop ended the turn");
    assert_eq!(g.players[1].life, 4);
    assert!(g.players[1].hand.is_empty());
}

#[test]
fn free_casts_and_casting_from_other_zones() {
    let db = db();
    let d = prep_db(
        &db,
        &[
            "Island",
            "Omniscience",
            "Omnipresence",
            "Tolarian Terror",
            "Helpful Hunter",
            "Muldrotha, the Gravetide",
            "Gleaming Barrier",
            "Mole Man, Moloid Master",
            "Thor, God of Thunder",
            "Lightning Strike",
            "Tinybones, Bauble Burglar",
            "Mountain",
            "Jessica Jones, Private Eye",
            "Plains",
        ],
    );
    // Omniscience: spells from hand are free.
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 1);
    g.players[0].hand.push(3);
    assert_eq!(
        g.cast_cost(0, Src::Hand(0), CastOpts::default())
            .unwrap()
            .total(),
        0
    );
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()));
    assert!(g.perms.iter().any(|x| x.card == Some((0, 3))));
    // Omnipresence: free up to the number of creatures you control.
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 2);
    put(&mut g, 0, 6);
    put(&mut g, 0, 6);
    g.players[0].hand = vec![4, 3];
    let cost = |g: &Game, h: usize| {
        g.cast_cost(0, Src::Hand(h), CastOpts::default())
            .unwrap()
            .total()
    };
    assert_eq!(cost(&g, 0), 0, "Helpful Hunter (MV 2) with two creatures");
    assert_eq!(cost(&g, 1), 7, "Tolarian Terror (MV 7) isn't free");
    // Muldrotha: one permanent of each type from the graveyard on your turn.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 8, 'W');
    put(&mut g, 0, 5);
    g.active = 0;
    g.players[0].graveyard = vec![4, 4, 6];
    assert!(g.cast(0, Src::Graveyard(0), CastOpts::default()));
    assert!(
        g.cast_cost(0, Src::Graveyard(0), CastOpts::default())
            .is_none(),
        "a second creature can't come from the graveyard this turn"
    );
    assert!(
        g.cast(0, Src::Graveyard(1), CastOpts::default()),
        "Gleaming Barrier is an artifact too: it uses the artifact type"
    );
    g.players[0].graveyard = vec![4];
    g.players[0].gy_types_used = 0;
    g.active = 1;
    assert!(
        g.cast_cost(0, Src::Graveyard(0), CastOpts::default())
            .is_none()
    );
    // Mole Man: a land from the graveyard.
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 7);
    g.active = 0;
    g.players[0].graveyard = vec![0];
    ai::main_phase(&mut g, 0, false);
    assert!(g.players[0].graveyard.is_empty());
    assert!(g.perms.iter().any(|x| x.card == Some((0, 0))));
    // Thor: an instant from the graveyard to exile, playable until the end of the next turn.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 2, 'R');
    g.players[0].graveyard = vec![9];
    g.active = 0;
    put(&mut g, 0, 8);
    assert_eq!(g.players[0].playable.len(), 1);
    assert!(g.players[0].graveyard.is_empty());
    assert!(g.cast(0, Src::Exile(0), CastOpts::default()));
    assert!(g.players[0].playable.is_empty());
    assert!(g.players[0].graveyard.contains(&9));
    // Tinybones: an opponent's discarded card is stashed, castable on our turn with any
    // mana; it stays its owner's card.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 2, 'U');
    put(&mut g, 0, 10);
    g.players[1].hand = vec![9];
    g.discard(1);
    assert!(g.players[1].graveyard.is_empty());
    assert_eq!(g.players[0].playable.len(), 1);
    g.active = 1;
    assert!(g.cast_cost(0, Src::Exile(0), CastOpts::default()).is_none());
    g.active = 0;
    assert_eq!(
        g.cast_cost(0, Src::Exile(0), CastOpts::default()),
        Some(Mana::generic(2)),
        "any type of mana: Islands pay for Lightning Strike"
    );
    assert!(g.cast(0, Src::Exile(0), CastOpts::default()));
    assert!(g.players[1].graveyard.contains(&9), "back to its owner");
    // Jessica Jones: exile the top X (her power) cards, playable this turn only.
    let mut g = game(&[&d, &d]);
    g.active = 0;
    let jj = put(&mut g, 0, 12);
    let k = ability(&d, 12, "exile the top");
    assert!(g.activate(0, jj, k));
    assert_eq!(g.players[0].playable.len(), 2);
    assert_eq!(perm(&g, jj).stun, 1);
    g.cleanup(0);
    assert!(g.players[0].playable.is_empty(), "only this turn");
}

#[test]
fn cost_taxes_and_reductions() {
    let db = db();
    let mut cards: Vec<Card> = [
        "Island",
        "Thalia, the Survivor",
        "Lightning Strike",
        "Tolarian Terror",
        "The Scarlet Witch",
        "Arcane Epiphany",
        "Radagast of Rhosgobel",
        "Cantankerous Keepers",
        "Propaganda",
    ]
    .iter()
    .map(|n| db.get(n).unwrap().clone())
    .collect();
    cards.push(card("Elf", "{G}", "Creature — Elf", "", Some(("1", "1"))));
    let d = prep(&cards);
    let cost = |g: &Game, p: usize| {
        g.cast_cost(p, Src::Hand(0), CastOpts::default())
            .unwrap()
            .total()
    };
    // Thalia, the Survivor: opponents' noncreature spells cost {1} more.
    let mut g = game(&[&d, &d]);
    put(&mut g, 1, 1);
    g.players[0].hand = vec![2];
    assert_eq!(cost(&g, 0), 3);
    g.players[0].hand = vec![9];
    assert_eq!(cost(&g, 0), 1, "creature spells aren't taxed");
    g.players[1].hand = vec![2];
    assert_eq!(cost(&g, 1), 2, "not her controller's spells");
    // Tolarian Terror: {1} less per instant and sorcery card in your graveyard.
    let mut g = game(&[&d, &d]);
    g.players[0].hand = vec![3];
    g.players[0].graveyard = vec![2, 2, 2, 9];
    assert_eq!(cost(&g, 0), 4);
    // The Scarlet Witch: instants and sorceries of mana value 4+ cost {X} less (her power).
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 4);
    g.players[0].hand = vec![5];
    assert_eq!(cost(&g, 0), 3, "Arcane Epiphany 5 - 2");
    g.players[0].hand = vec![2];
    assert_eq!(cost(&g, 0), 2, "Lightning Strike is under mana value 4");
    // Radagast: the first creature spell each turn costs {2} less (and he's a Wizard, so
    // Arcane Epiphany costs {1} less).
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 6);
    g.players[0].hand = vec![3];
    assert_eq!(cost(&g, 0), 5);
    g.players[0].creature_spells_turn = 1;
    assert_eq!(cost(&g, 0), 7);
    g.players[0].hand = vec![5];
    assert_eq!(cost(&g, 0), 4);
    // Cantankerous Keepers: affinity for Elves.
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 9);
    put(&mut g, 0, 9);
    g.players[0].hand = vec![7];
    assert_eq!(cost(&g, 0), 4);
    // Propaganda: each attacker costs {2}; what can't be paid for doesn't attack.
    let mut g = game(&[&d, &d]);
    put(&mut g, 1, 8);
    put(&mut g, 0, 9);
    put(&mut g, 0, 9);
    lands(&mut g, 0, 2, 'U');
    g.players[1].life = 1;
    g.active = 0;
    g.combat(0);
    assert!(g.players[1].life <= 0, "one Elf paid for and connected");
    let attacked = g.perms.iter().filter(|x| x.attacked_turn).count();
    assert_eq!(attacked, 1, "only one attacker could be paid for");
    assert!(
        g.perms
            .iter()
            .filter(|x| x.is(T_LAND) && x.ctrl == 0)
            .all(|x| x.tapped)
    );
}

#[test]
fn rule_changing_statics() {
    let db = db();
    let d = prep_db(
        &db,
        &[
            "Island",
            "Karn, Argent Defender",
            "Helpful Hunter",
            "Ghalta the Immovable",
            "Gleaming Barrier",
            "Loot, the Anomaly",
            "Sorcerous Spyglass",
            "Bruce Banner",
        ],
    );
    // Karn: creatures entering trigger nothing.
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 1);
    put(&mut g, 0, 2);
    assert!(g.players[0].hand.is_empty(), "Helpful Hunter drew nothing");
    // Ghalta: defenders attack and toughness assigns the damage.
    let mut g = game(&[&d, &d]);
    let wall = put(&mut g, 0, 4);
    let i = g.idx(wall).unwrap();
    assert!(!g.can_attack(i));
    put(&mut g, 0, 3);
    let i = g.idx(wall).unwrap();
    assert!(g.can_attack(i));
    assert_eq!(g.combat_power(&g.perms[i].clone()), 4);
    // Loot: negative power deals damage as though it were positive.
    let loot = put(&mut g, 1, 5);
    let i = g.idx(loot).unwrap();
    assert_eq!(g.perms[i].p, -2);
    assert_eq!(g.combat_power(&g.perms[i].clone()), 2);
    // Sorcerous Spyglass names Bruce Banner: his activated abilities stop.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 1, 6, 'U');
    let bruce = put(&mut g, 1, 7);
    let i = g.idx(bruce).unwrap();
    let k = ability_of(&g, bruce, |a| a.text.contains("transform"));
    assert!(g.activation_cost(1, i, k).is_some());
    let glass = put(&mut g, 0, 6);
    assert_eq!(perm(&g, glass).named, Some((1, 7)));
    let i = g.idx(bruce).unwrap();
    assert!(g.activation_cost(1, i, k).is_none());
}

#[test]
fn characteristic_changes_and_odd_effects() {
    let db = db();
    let mut cards: Vec<Card> = [
        "Island",
        "Eaten by Piranhas",
        "Puppet Crafting",
        "Sower of Chaos",
        "Knight of Grace",
        "Bilbo's Deadly Slice",
        "The Sentry, Golden Guardian",
        "Wishclaw Talisman",
        "Undying Malice",
        "Gandalf, Wandering Wizard",
        "Ajani Resolute",
        "Garruk, Veiled Butcher",
        "Chandra, Chill of Compliance",
    ]
    .iter()
    .map(|n| db.get(n).unwrap().clone())
    .collect();
    cards.push(card(
        "Drake",
        "{2}{U}",
        "Creature — Drake",
        "Flying",
        Some(("3", "3")),
    ));
    cards.push(card("Relic", "{1}", "Artifact", "", None));
    let d = prep(&cards);
    const DRAKE: u16 = 13;
    const RELIC: u16 = 14;
    // Eaten by Piranhas: a 1/1 with no abilities that can still attack and block.
    let mut g = game(&[&d, &d]);
    let drake = put(&mut g, 1, DRAKE);
    let i = g.enter_card(0, 1, 0, false).unwrap();
    g.perms[i].attached = Some(drake);
    g.recalc();
    let x = perm(&g, drake);
    assert_eq!((x.p, x.t), (1, 1));
    assert!(!x.has(FLYING) && x.blank && !x.locked);
    // Puppet Crafting: an artifact becomes a 5/5 creature.
    let relic = put(&mut g, 0, RELIC);
    let i = g.enter_card(0, 2, 0, false).unwrap();
    g.perms[i].attached = Some(relic);
    g.recalc();
    let x = perm(&g, relic);
    assert!(x.is_creature() && (x.p, x.t) == (5, 5));
    // Sower of Chaos: target creature can't block this turn.
    let mut g = game(&[&d, &d]);
    g.active = 0;
    let sower = put(&mut g, 0, 3);
    let blocker = put(&mut g, 1, DRAKE);
    let k = ability(&d, 3, "can't block");
    fire_card(&mut g, 3, k, Some(sower));
    let (b, a) = (g.idx(blocker).unwrap(), g.idx(sower).unwrap());
    assert!(!g.can_block(b, a));
    // Knight of Grace: hexproof from black.
    let mut g = game(&[&d, &d]);
    let knight = put(&mut g, 1, 4);
    fire_card(&mut g, 5, 0, None);
    assert!(g.idx(knight).is_some(), "black removal can't target it");
    let drake = put(&mut g, 1, DRAKE);
    fire_card(&mut g, 5, 0, None);
    assert!(g.idx(drake).is_none());
    // The Sentry: an opponent gets The Void, a 5/5 flying indestructible token.
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 6);
    let void = g
        .perms
        .iter()
        .find(|x| x.ctrl == 1 && x.is_token())
        .unwrap();
    assert_eq!((void.p, void.t), (5, 5));
    assert!(void.has(FLYING) && void.has(INDESTRUCTIBLE));
    // Wishclaw Talisman: after the search, an opponent gains control of it.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 1, 'U');
    g.active = 0;
    let claw = put(&mut g, 0, 7);
    let k = ability(&d, 7, "search");
    assert!(g.activate(0, claw, k));
    assert_eq!(perm(&g, claw).ctrl, 1);
    // Undying Malice: the creature comes back tapped with a +1/+1 counter.
    let mut g = game(&[&d, &d]);
    let drake = put(&mut g, 0, DRAKE);
    fire_card(&mut g, 8, 0, None);
    let i = g.idx(drake).unwrap();
    g.destroy(i);
    g.run_pending();
    let back = g
        .perms
        .iter()
        .find(|x| x.card == Some((0, DRAKE)))
        .expect("returned");
    assert!(back.tapped && back.counters == 1);
    // Gandalf, Wandering Wizard: shuffled into his owner's library, three cards drawn.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 6, 'U');
    let gandalf = put(&mut g, 0, 9);
    let library = g.players[0].library.len();
    let k = ability(&d, 9, "shuffles");
    assert!(g.activate(0, gandalf, k));
    assert!(g.idx(gandalf).is_none());
    assert_eq!(g.players[0].hand.len(), 3);
    assert_eq!(g.players[0].library.len(), library - 2);
    // Ajani Resolute's emblem: creatures you control get +2/+2, new ones too.
    let mut g = game(&[&d, &d]);
    let ajani = put(&mut g, 0, 10);
    let k = ability(&d, 10, "emblem");
    fire_card(&mut g, 10, k, Some(ajani));
    let drake = put(&mut g, 0, DRAKE);
    assert_eq!(perm(&g, drake).p, 5);
    assert_eq!(g.players[0].emblems.len(), 1);
    // Chandra's emblem: whenever you cast a spell, draw a card.
    let chandra = put(&mut g, 0, 12);
    let k = ability(&d, 12, "emblem");
    fire_card(&mut g, 12, k, Some(chandra));
    lands(&mut g, 0, 1, 'U');
    g.players[0].hand = vec![RELIC];
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()));
    assert_eq!(g.players[0].hand.len(), 1, "the emblem drew a card");
    // Garruk: -4/-1 until Garruk's controller's next turn.
    let mut g = game(&[&d, &d]);
    let garruk = put(&mut g, 0, 11);
    let drake = put(&mut g, 1, DRAKE);
    let k = ability(&d, 11, "-4/-1");
    fire_card(&mut g, 11, k, Some(garruk));
    assert_eq!(perm(&g, drake).p, -1);
    g.cleanup(0);
    g.recalc();
    assert_eq!(perm(&g, drake).p, -1, "still during the opponent's turn");
    // (Garruk goes, so the turn doesn't use him again.)
    let i = g.idx(garruk).unwrap();
    g.leave(i, Zone::Graveyard, false);
    g.take_turn(0, false);
    assert_eq!(perm(&g, drake).p, 3, "gone at our next turn");
}

#[test]
fn delayed_returns_and_aura_hosts() {
    let db = db();
    let d = prep_db(
        &db,
        &[
            "Island",
            "Helpful Hunter",
            "S.H.I.E.L.D. Flying Car",
            "Puppet Crafting",
            "Gleaming Barrier",
            "Forest",
        ],
    );
    // S.H.I.E.L.D. Flying Car: the creature comes back at the beginning of the next end
    // step (and enters again: Helpful Hunter draws again).
    let mut g = game(&[&d, &d]);
    g.active = 0;
    put(&mut g, 0, 1);
    let hand = g.players[0].hand.len();
    put(&mut g, 0, 2);
    assert!(
        !g.perms.iter().any(|x| x.card == Some((0, 1))),
        "exiled until the end step"
    );
    assert_eq!(g.ext.delayed.len(), 1);
    g.take_turn(0, false);
    assert!(g.perms.iter().any(|x| x.card == Some((0, 1))));
    assert!(g.players[0].hand.len() >= hand + 1);
    // Puppet Crafting goes on one of our non-creature artifacts, not a creature.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 2, 'G');
    put(&mut g, 0, 1);
    let relic = put(&mut g, 0, 4);
    let i = g.idx(relic).unwrap();
    g.perms[i].types &= !T_CREATURE;
    g.players[0].hand = vec![3];
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()));
    let x = perm(&g, relic);
    assert!(x.is_creature() && (x.p, x.t) == (5, 5));
}

#[test]
fn mechanism_cards_play_whole_games() {
    let db = db();
    let names = [
        "Bruce Banner",
        "Jennifer Walters",
        "King T'Challa",
        "Monica Rambeau",
        "Tony Stark",
        "Kang the Conqueror",
        "Time Stop",
        "Omniscience",
        "Omnipresence",
        "Muldrotha, the Gravetide",
        "Thor, God of Thunder",
        "Tinybones, Bauble Burglar",
        "Mole Man, Moloid Master",
        "Thalia, the Survivor",
        "Propaganda",
        "Tolarian Terror",
        "The Scarlet Witch",
        "Radagast of Rhosgobel",
        "Herald of Eternal Dawn",
        "Karn, Argent Defender",
        "Ghalta the Immovable",
        "Loot, the Anomaly",
        "Sorcerous Spyglass",
        "Eaten by Piranhas",
        "Perfected Theory",
        "Puppet Crafting",
        "The Sentry, Golden Guardian",
        "Wishclaw Talisman",
        "Undying Malice",
        "Fblthp, Impossibly Lost",
        "Gandalf, Wandering Wizard",
        "Frenzied Goblin",
        "Sower of Chaos",
        "Ajani Resolute",
        "Chandra, Torch of Defiance",
        "Garruk, Veiled Butcher",
        "Jessica Jones, Private Eye",
        "Inside Information",
        "Soulstone Sanctuary",
        "S.H.I.E.L.D. Flying Car",
    ];
    let mut deck = crate::deck::Deck::new("mechanisms");
    for n in names {
        deck.add(n, 1);
    }
    for land in ["Plains", "Island", "Swamp", "Mountain", "Forest"] {
        deck.add(land, 8);
    }
    let me = Prepared::new(&deck, &db);
    assert_eq!(me.library.len(), names.len() + 40);
    let opp = league_decks(&db);
    let rules = Rules::default();
    let mut rng = Rng::new(21);
    for k in 0..40 {
        let mut table: Vec<&Prepared> = vec![&opp[0], &opp[1], &opp[2]];
        table.insert(k % 4, &me);
        let w = play_game(&table, &rules, &mut rng);
        assert!(w < 4 || w == super::NO_WINNER);
    }
}

#[test]
fn free_spells_from_the_top_of_libraries() {
    let db = db();
    let d = prep_db(&db, &["Island", "Etali, Primal Storm", "Helpful Hunter"]);
    // Etali: the top card of each player's library, cast free: the opponent's creature is
    // ours to control, still theirs to own; a land just stays exiled.
    let mut g = game(&[&d, &d]);
    let etali = put(&mut g, 0, 1);
    g.players[1].library.push(2);
    let k = ability(&d, 1, "each player's library");
    fire_card(&mut g, 1, k, Some(etali));
    let hunter = g
        .perms
        .iter()
        .find(|x| x.card == Some((1, 2)))
        .expect("cast from Opp's library");
    assert_eq!((hunter.ctrl, hunter.owner), (0, 1));
    assert_eq!(g.players[0].exile.len(), 1, "our Island stays exiled");
}

#[test]
fn copies_go_on_the_right_permanent() {
    let db = db();
    let mut cards: Vec<Card> = ["Island", "Secret Invasion", "Shuri, Wakandan Inventor"]
        .iter()
        .map(|n| db.get(n).unwrap().clone())
        .collect();
    cards.push(card(
        "Drake",
        "{2}{U}",
        "Creature — Drake",
        "Flying",
        Some(("2", "2")),
    ));
    cards.push(card(
        "Giant",
        "{4}{G}",
        "Creature — Giant",
        "Trample",
        Some(("5", "5")),
    ));
    cards.push(card("Relic", "{1}", "Artifact", "", None));
    cards.push(card(
        "Golem",
        "{3}",
        "Artifact Creature — Golem",
        "",
        Some(("3", "3")),
    ));
    let d = prep(&cards);
    // Secret Invasion: the opponent's Giant is exiled and our enchanted Drake becomes a copy
    // of it (not the Aura).
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 3, 'U');
    let drake = put(&mut g, 0, 3);
    let giant = put(&mut g, 1, 4);
    g.players[0].hand = vec![1];
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()));
    assert!(g.idx(giant).is_none(), "exiled until the Aura leaves");
    let x = perm(&g, drake);
    assert_eq!((x.p, x.t), (5, 5));
    assert!(x.has(TRAMPLE));
    // Shuri: our least valuable artifact becomes a copy of the best other one this turn.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 1, 'U');
    let shuri = put(&mut g, 0, 2);
    let relic = put(&mut g, 0, 5);
    put(&mut g, 0, 6);
    let k = ability(&d, 2, "becomes a copy");
    assert!(g.activate(0, shuri, k));
    let x = perm(&g, relic);
    assert!(x.is_creature() && (x.p, x.t) == (3, 3));
    g.cleanup(0);
    g.recalc();
    assert!(!perm(&g, relic).is_creature(), "until end of turn");
}

#[test]
fn more_rule_statics_and_turn_cost_changes() {
    let db = db();
    let mut cards: Vec<Card> = [
        "Island",
        "Authority of the Consuls",
        "Yoshimaru, Beloved Companion",
        "Head of the Hunt",
        "World War Hulk",
        "Armor Wars",
        "Lightning Strike",
    ]
    .iter()
    .map(|n| db.get(n).unwrap().clone())
    .collect();
    cards.push(card(
        "Giant",
        "{4}{G}",
        "Creature — Giant",
        "",
        Some(("5", "5")),
    ));
    cards.push(card("Relic", "{3}", "Artifact", "", None));
    let d = prep(&cards);
    const GIANT: u16 = 7;
    const RELIC: u16 = 8;
    let mut g = game(&[&d, &d]);
    // Authority of the Consuls: an opponent's creature enters tapped, ours doesn't.
    put(&mut g, 0, 1);
    let theirs = put(&mut g, 1, GIANT);
    let ours = put(&mut g, 0, GIANT);
    assert!(perm(&g, theirs).tapped && !perm(&g, ours).tapped);
    // Yoshimaru: one more +1/+1 counter.
    put(&mut g, 0, 2);
    let i = g.idx(ours).unwrap();
    g.add_counters(i, 1);
    assert_eq!(perm(&g, ours).counters, 2);
    // Head of the Hunt: an opponent's creature that would die is exiled.
    put(&mut g, 0, 3);
    let i = g.idx(theirs).unwrap();
    g.destroy(i);
    assert!(g.players[1].graveyard.is_empty());
    assert_eq!(g.players[1].exile, vec![GIANT]);
    // World War Hulk I: the next red or green creature spell this turn is free.
    let mut g = game(&[&d, &d]);
    g.active = 0;
    let hulk = put(&mut g, 0, 4);
    let k = ability(&d, 4, "without paying");
    fire_card(&mut g, 4, k, Some(hulk));
    g.players[0].hand = vec![GIANT, GIANT];
    let cost = |g: &Game, h: usize| {
        g.cast_cost(0, Src::Hand(h), CastOpts::default())
            .unwrap()
            .total()
    };
    assert_eq!(cost(&g, 0), 0);
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()));
    assert_eq!(cost(&g, 0), 5, "only the next one");
    // Armor Wars II: artifact spells cost {1} less this turn.
    let saga = put(&mut g, 0, 5);
    let k = ability(&d, 5, "this turn cost");
    fire_card(&mut g, 5, k, Some(saga));
    g.players[0].hand = vec![RELIC, 6];
    assert_eq!(cost(&g, 0), 2);
    assert_eq!(cost(&g, 1), 2, "not other spells");
    g.take_turn(1, false);
    g.players[0].hand = vec![RELIC];
    assert_eq!(cost(&g, 0), 3, "only that turn");
}

#[test]
fn attack_modifiers_until_your_next_turn() {
    let db = db();
    let mut cards: Vec<Card> = ["Island", "Jace, Reality Sculptor", "Garruk, Curse Breaker"]
        .iter()
        .map(|n| db.get(n).unwrap().clone())
        .collect();
    cards.push(card(
        "Giant",
        "{4}{G}",
        "Creature — Giant",
        "",
        Some(("5", "5")),
    ));
    let d = prep(&cards);
    // Jace: creatures attacking us get -5/-0 until our next turn.
    let mut g = game(&[&d, &d]);
    let jace = put(&mut g, 0, 1);
    let k = ability(&d, 1, "attacks you");
    fire_card(&mut g, 1, k, Some(jace));
    put(&mut g, 1, 3);
    g.players[0].life = 4;
    g.active = 1;
    g.combat(1);
    assert_eq!(g.players[0].life, 4, "the Giant hits for 0");
    // Garruk: our attackers get +2/+2 and trample until our next turn.
    let mut g = game(&[&d, &d]);
    let garruk = put(&mut g, 0, 2);
    let k = ability(&d, 2, "one or more creatures attack");
    fire_card(&mut g, 2, k, Some(garruk));
    put(&mut g, 0, 3);
    g.players[1].life = 7;
    g.active = 0;
    g.combat(0);
    assert!(g.players[1].life <= 0, "a 7/7 trampler");
}

#[test]
fn ramos_counters_pay_for_spells_once_each_turn() {
    let db = db();
    let d = prep_db(&db, &["Ramos, Dragon Engine"]);
    let mut g = game(&[&d, &d]);
    let ramos = put(&mut g, 0, 0);
    let ten = Mana::parse("{W}{W}{U}{U}{B}{B}{R}{R}{G}{G}");
    assert!(!g.can_pay(0, &ten), "no counters, no mana");
    let i = g.idx(ramos).unwrap();
    g.perms[i].counters = 6;
    g.perms[i].tapped = true;
    assert!(
        g.can_pay(0, &ten),
        "five counters make ten mana, even while tapped"
    );
    assert!(g.pay(0, &ten));
    assert_eq!(perm(&g, ramos).counters, 1, "five counters removed");
    let i = g.idx(ramos).unwrap();
    g.perms[i].counters = 6;
    assert!(!g.can_pay(0, &Mana::parse("{W}")), "only once each turn");
}

#[test]
fn ronin_sacrifices_only_its_own_equipment() {
    let db = db();
    let mut cards = vec![db.get("Ronin, Shadow Stalker").unwrap().clone()];
    cards.push(card(
        "Bear",
        "{1}{G}",
        "Creature — Bear",
        "",
        Some(("2", "2")),
    ));
    cards.push(card(
        "Blade",
        "{1}",
        "Artifact — Equipment",
        "Equipped creature gets +1/+0.\nEquip {1}",
        None,
    ));
    let d0 = prep(&cards);
    let d1 = prep(&[card(
        "Dragon",
        "{4}{R}{R}",
        "Creature — Dragon",
        "Flying",
        Some(("4", "4")),
    )]);
    let mut g = game(&[&d0, &d1]);
    let ronin = put(&mut g, 0, 0);
    let bear = put(&mut g, 0, 1);
    let dragon = put(&mut g, 1, 0);
    let i = g.idx(ronin).unwrap();
    let k = g
        .abilities_of(&g.perms[i])
        .iter()
        .position(|a| matches!(a.trig, Trig::Activated { tap: true, .. }))
        .unwrap();
    assert!(
        !g.activate(0, ronin, k),
        "no Equipment attached: the -4/-4 can't be paid for with the Bear"
    );
    assert!(g.idx(bear).is_some(), "the Bear isn't sacrificed");
    let blade = put(&mut g, 0, 2);
    let b = g.idx(blade).unwrap();
    g.perms[b].attached = Some(ronin);
    assert!(g.activate(0, ronin, k));
    g.run_pending();
    g.sba();
    assert!(
        g.idx(blade).is_none(),
        "the attached Equipment is sacrificed"
    );
    assert!(g.idx(bear).is_some(), "the Bear survives");
    assert!(g.idx(dragon).is_none(), "the 4/4 gets -4/-4");
}

// ------------------------------------------------------------ rules audit regressions

/// Real cards by name; "Plains", "Island", "Swamp", "Mountain" and "Forest" are basics.
fn deck(db: &CardDb, names: &[&str]) -> Prepared {
    let basics = [
        ("Plains", 'W'),
        ("Island", 'U'),
        ("Swamp", 'B'),
        ("Mountain", 'R'),
        ("Forest", 'G'),
    ];
    let cards: Vec<Card> = names
        .iter()
        .map(|n| match basics.iter().find(|b| b.0 == *n) {
            Some(b) => Card::basic_land(b.0, b.1),
            None => db
                .get(n)
                .unwrap_or_else(|| panic!("{} not in db", n))
                .clone(),
        })
        .collect();
    prep(&cards)
}

/// The index of the first ability of permanent `id` whose text contains `what`.
fn ab_with(g: &Game, id: u32, what: &str) -> usize {
    let x = perm(g, id);
    g.abilities_of(x)
        .iter()
        .position(|a| a.text.contains(what))
        .unwrap_or_else(|| panic!("no ability with {:?}", what))
}

fn tapped_lands(g: &Game, p: usize) -> usize {
    g.perms
        .iter()
        .filter(|x| x.ctrl as usize == p && x.is(T_LAND) && x.tapped)
        .count()
}

#[test]
fn whenever_you_attack_triggers_once_per_combat() {
    let db = db();
    // Misty Mountains Raider: "whenever you attack, amass Goblins 2" - one attack, one Army
    // with two counters (it was amassed twice).
    let d = deck(&db, &["Misty Mountains Raider", "Agent 13, Sharon Carter"]);
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 0);
    g.combat(0);
    assert!(g.players[0].attacked_this_turn);
    let army: Vec<&Perm> = g.perms.iter().filter(|x| x.tok == Tok::Army).collect();
    assert_eq!(army.len(), 1);
    assert_eq!(army[0].counters, 2, "amassed 2 once");
    // Agent 13: "whenever a creature you control attacks alone, investigate" - one Clue.
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 1);
    g.combat(0);
    assert!(g.players[0].attacked_this_turn);
    let clues = g.perms.iter().filter(|x| x.tok == Tok::Clue).count();
    assert_eq!(clues, 1);
}

#[test]
fn activated_abilities_cost_their_colours() {
    let db = db();
    // The Mind Stone: "{5}{W}, {T}: Harness ~" needs white mana, and the Stone taps for
    // the cost, so its own {W} can't pay for it.
    let d = deck(&db, &["The Mind Stone", "Mountain", "Plains"]);
    let mut g = game(&[&d, &d]);
    let ms = put(&mut g, 0, 0);
    for _ in 0..6 {
        put(&mut g, 0, 1);
    }
    let k = ab_with(&g, ms, "harness");
    let cost = g.activation_cost(0, g.idx(ms).unwrap(), k).unwrap();
    assert_eq!((cost.generic, cost.pips), (5, [1, 0, 0, 0, 0]));
    assert!(!g.activate(0, ms, k), "six Mountains and the Stone itself");
    assert_eq!(tapped_lands(&g, 0), 0, "nothing was paid");
    assert!(!perm(&g, ms).tapped);
    put(&mut g, 0, 2);
    assert!(g.activate(0, ms, k));
    assert!(perm(&g, ms).powered && perm(&g, ms).tapped);
    assert_eq!(tapped_lands(&g, 0), 6, "{{5}}{{W}} from the lands");
}

#[test]
fn filter_lands_command_tower_and_conditional_lands() {
    let db = db();
    let mut d = deck(
        &db,
        &[
            "Command Tower",
            "Surveillance Room",
            "Baxter Building",
            "Gleaming Bastion",
            "Forest",
            "Misty Knight, Hero for Hire",
        ],
    );
    d.commanders = vec![(5, 1)];
    let mut g = game(&[&d, &d]);
    // Command Tower: the commander's colours (Misty Knight: red) only.
    let t = put(&mut g, 0, 0);
    assert!(g.can_pay(0, &Mana::parse("{R}")));
    assert!(!g.can_pay(0, &Mana::parse("{U}")));
    let i = g.idx(t).unwrap();
    g.perms[i].tapped = true;
    // Filters: {C} alone; "{1}, {T}: Add one mana of any color" isn't free fixing.
    let s = put(&mut g, 0, 1);
    assert!(g.can_pay(0, &Mana::parse("{1}")));
    assert!(!g.can_pay(0, &Mana::parse("{B}")));
    let i = g.idx(s).unwrap();
    g.perms[i].tapped = true;
    let b = put(&mut g, 0, 2);
    assert!(g.can_pay(0, &Mana::parse("{C}")));
    assert!(!g.can_pay(0, &Mana::parse("{G}")));
    let i = g.idx(b).unwrap();
    g.perms[i].tapped = true;
    // Gleaming Bastion: {W}/{U} the turn it entered or with a basic land.
    let gb = put(&mut g, 0, 3);
    assert!(g.can_pay(0, &Mana::parse("{W}")), "entered this turn");
    let i = g.idx(gb).unwrap();
    g.perms[i].entered_turn = false;
    assert!(
        !g.can_pay(0, &Mana::parse("{W}")),
        "a later turn, no basic land"
    );
    assert!(g.can_pay(0, &Mana::parse("{C}")));
    put(&mut g, 0, 4);
    assert!(g.can_pay(0, &Mana::parse("{W}")), "with a Forest");
    // A colourless commander: Command Tower makes nothing.
    let mut d2 = deck(&db, &["Command Tower", "Ultron Drone"]);
    d2.commanders = vec![(1, 1)];
    let mut g = game(&[&d2, &d2]);
    put(&mut g, 0, 0);
    assert!(!g.can_pay(0, &Mana::generic(1)));
}

#[test]
fn nobody_skips_the_first_draw_in_multiplayer() {
    assert!(!skips_first_draw(4, 1, 0));
    assert!(!skips_first_draw(3, 1, 0));
    assert!(skips_first_draw(2, 1, 0), "the two-player starting player");
    assert!(!skips_first_draw(2, 1, 1));
    assert!(!skips_first_draw(2, 2, 0));
}

#[test]
fn bifur_doubles_dwarf_triggers_with_an_enduring_story() {
    let db = db();
    let d = deck(&db, &["Bifur, Melodic Rider", "Ultron Drone"]);
    let counters = |g: &Game| -> i16 {
        g.perms
            .iter()
            .filter(|x| x.ctrl == 0)
            .map(|x| x.counters)
            .sum()
    };
    // Without an enduring story: Bifur's "when ~ enters, put a +1/+1 counter on target
    // creature" once.
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 1);
    put(&mut g, 0, 0);
    assert!(!g.players[0].enduring_story);
    assert_eq!(counters(&g), 1);
    // Three artifacts: an enduring story, and the Dwarf's trigger happens twice.
    let mut g = game(&[&d, &d]);
    for _ in 0..3 {
        put(&mut g, 0, 1);
    }
    assert!(g.players[0].enduring_story);
    put(&mut g, 0, 0);
    assert_eq!(counters(&g), 2);
}

#[test]
fn dwalins_hone_counters_grant_power() {
    let db = db();
    let d = deck(&db, &["Dwalin, Weaponmaster", "Captain America's Shield"]);
    let mut g = game(&[&d, &d]);
    let shield = put(&mut g, 0, 1);
    let dwalin = put(&mut g, 0, 0);
    assert_eq!(perm(&g, shield).hone, 1, "a hone counter on each Equipment");
    let i = g.idx(shield).unwrap();
    g.perms[i].attached = Some(dwalin);
    g.recalc();
    let x = perm(&g, dwalin);
    assert_eq!((x.p, x.t), (3, 9), "2/1 +1/+0 (hone) +0/+8 (Shield)");
}

#[test]
fn an_additional_combat_phase_happens() {
    let db = db();
    // Aurelia: "whenever ~ attacks for the first time each turn, ... after this phase,
    // there is an additional combat phase" - she hits twice.
    let d = deck(&db, &["Aurelia, the Warleader", "Mountain"]);
    let mut g = game(&[&d, &d]);
    g.players[0].library = vec![1; 30];
    put(&mut g, 0, 0);
    let life = g.players[1].life;
    g.take_turn(0, false);
    assert_eq!(life - g.players[1].life, 6);
}

#[test]
fn counts_read_from_the_game_not_typical_values() {
    let db = db();
    let d = deck(
        &db,
        &[
            "Ramos, Dragon Engine",
            "Ultron Drone",
            "Lightning Strike",
            "Misty Knight, Hero for Hire",
            "Mountain",
            "Stature, Size Shifter",
            "Island",
        ],
    );
    // Ramos: a counter per colour of the spell (colourless: none).
    let mut g = game(&[&d, &d]);
    let r = put(&mut g, 0, 0);
    g.on_cast(0, &d.cards[1]);
    g.run_pending();
    assert_eq!(perm(&g, r).counters, 0);
    g.on_cast(0, &d.cards[2]);
    g.run_pending();
    assert_eq!(perm(&g, r).counters, 1);
    // Misty Knight: "draw a card for each card you've discarded this turn" - one.
    let mut g = game(&[&d, &d]);
    let mk = put(&mut g, 0, 3);
    lands(&mut g, 0, 2, 'R');
    g.players[0].hand = vec![4];
    let k = ab_with(&g, mk, "discard a card");
    assert!(g.activate(0, mk, k));
    assert_eq!(g.players[0].hand.len(), 1);
    // Stature: power-up {X}{U}{U} pays X and puts X counters.
    let mut g = game(&[&d, &d]);
    let st = put(&mut g, 0, 5);
    let i = g.idx(st).unwrap();
    g.perms[i].entered_turn = false;
    lands(&mut g, 0, 2, 'U');
    let k = ab_with(&g, st, "put x");
    assert!(g.activation_cost(0, i, k).is_none(), "X = 0 does nothing");
    lands(&mut g, 0, 2, 'U');
    assert!(g.activate(0, st, k));
    assert_eq!(perm(&g, st).counters, 2);
    assert_eq!(tapped_lands(&g, 0), 4);
}

#[test]
fn balin_deals_the_cards_discarded() {
    let db = db();
    let d = deck(&db, &["Balin, Loremaster", "Ultron Drone", "Mountain"]);
    for (hand, dealt) in [(vec![2u16, 2, 2], 3), (vec![2u16], 0)] {
        let mut g = game(&[&d, &d]);
        for _ in 0..2 {
            put(&mut g, 0, 1);
        }
        g.players[0].hand = hand;
        let life = g.players[1].life;
        put(&mut g, 0, 0);
        assert!(g.players[0].enduring_story);
        assert_eq!(life - g.players[1].life, dealt);
    }
}

#[test]
fn connive_counts_only_nonland_discards() {
    let db = db();
    let d = deck(
        &db,
        &[
            "A.I.M. Scientists",
            "Island",
            "Ultron Drone",
            "Trickster's Stratagem",
        ],
    );
    // Draws an Island and discards it: no counter.
    let mut g = game(&[&d, &d]);
    g.players[0].library = vec![1; 10];
    let a = put(&mut g, 0, 0);
    assert_eq!(perm(&g, a).counters, 0);
    // Draws a Drone and discards it: a counter.
    let mut g = game(&[&d, &d]);
    g.players[0].library = vec![2; 10];
    let a = put(&mut g, 0, 0);
    assert_eq!(perm(&g, a).counters, 1);
    // "Up to one target creature you control connives" with no creature: no loot.
    let mut g = game(&[&d, &d]);
    let foe = put(&mut g, 1, 2);
    lands(&mut g, 0, 4, 'U');
    g.players[0].hand = vec![3];
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()));
    assert!(g.idx(foe).is_none());
    assert_eq!(
        (g.players[0].hand.len(), g.players[0].library.len()),
        (0, 30)
    );
}

#[test]
fn magnificent_end_cast_for_less_hits_a_tapped_creature() {
    let db = db();
    let d = deck(
        &db,
        &[
            "Magnificent End",
            "Plains",
            "Ultron Drone",
            "Ramos, Dragon Engine",
        ],
    );
    let mut g = game(&[&d, &d]);
    let drone = put(&mut g, 1, 2);
    let ramos = put(&mut g, 1, 3);
    let i = g.idx(drone).unwrap();
    g.perms[i].tapped = true;
    lands(&mut g, 0, 2, 'W');
    g.players[0].hand = vec![0];
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()), "{{1}}{{W}}");
    assert!(g.idx(drone).is_none(), "the tapped creature");
    assert!(g.idx(ramos).is_some(), "not the untapped one");
}

#[test]
fn doc_samson_adds_one_counter_of_each_kind() {
    let db = db();
    let d = deck(&db, &["Doc Samson, Super Psychiatrist", "Ultron Drone"]);
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 0);
    let drone = put(&mut g, 0, 1);
    let i = g.idx(drone).unwrap();
    g.add_counters(i, 2);
    assert_eq!(perm(&g, drone).counters, 3);
    let i = g.idx(drone).unwrap();
    g.add_other_counters(i, 1);
    assert_eq!(perm(&g, drone).other_counters, 2);
    // Not on an opponent's permanent.
    let theirs = put(&mut g, 1, 1);
    let i = g.idx(theirs).unwrap();
    g.add_counters(i, 2);
    assert_eq!(perm(&g, theirs).counters, 2);
}

#[test]
fn robot_domination_sees_creature_cards_from_anywhere() {
    let db = db();
    let d = deck(&db, &["Robot Domination", "Ultron Drone", "Island"]);
    let mut g = game(&[&d, &d]);
    let rd = put(&mut g, 0, 0);
    let life = g.players[0].life;
    // A creature card discarded: draw, lose 1 life, a plan counter.
    g.put_discarded(0, 1);
    g.run_pending();
    assert_eq!(g.players[0].hand.len(), 1);
    assert_eq!(g.players[0].life, life - 1);
    assert_eq!(perm(&g, rd).other_counters, 1);
    // Milled: a land does nothing, a creature card does.
    g.players[0].library.push(2);
    g.mill(0, 1);
    g.run_pending();
    assert_eq!(perm(&g, rd).other_counters, 1);
    g.players[0].library.push(1);
    g.mill(0, 1);
    g.run_pending();
    assert_eq!(perm(&g, rd).other_counters, 2);
}

#[test]
fn the_masters_of_evil_tutors_a_plan_from_the_hand() {
    let db = db();
    let d = deck(
        &db,
        &[
            "The Masters of Evil",
            "Robot Domination",
            "Ultron Drone",
            "Swamp",
            "Mountain",
        ],
    );
    assert_eq!(d.cards[0].cycling, Some((2, "plan".to_string())));
    // On the battlefield the ability can't be used.
    let mut g = game(&[&d, &d]);
    let m = put(&mut g, 0, 0);
    lands(&mut g, 0, 3, 'B');
    let k = ab_with(&g, m, "search your library");
    assert!(g.activation_cost(0, g.idx(m).unwrap(), k).is_none());
    // From the hand: {1}{B}, the card itself, a Plan.
    let mut g = game(&[&d, &d]);
    g.players[0].library = vec![2, 1, 2, 2];
    g.players[0].hand = vec![0];
    lands(&mut g, 0, 2, 'R');
    assert!(!g.cycle(0, 0), "no black mana");
    lands(&mut g, 0, 1, 'B');
    assert!(g.cycle(0, 0));
    assert_eq!(g.players[0].hand, vec![1]);
    assert_eq!(g.players[0].graveyard, vec![0]);
}

#[test]
fn captain_americas_shield_taps_a_defending_players_creature() {
    let db = db();
    let d = deck(
        &db,
        &["Captain America's Shield", "Ultron Drone", "Red Hulk"],
    );
    let mut g = game(&[&d, &d, &d]);
    let att = put(&mut g, 0, 1);
    let shield = put(&mut g, 0, 0);
    let i = g.idx(shield).unwrap();
    g.perms[i].attached = Some(att);
    let small = put(&mut g, 1, 1);
    let hulk = put(&mut g, 2, 2);
    let i = g.idx(att).unwrap();
    g.perms[i].attacking = true;
    g.ext.attack_def = vec![(att, 1)];
    let snap = perm(&g, att).clone();
    g.emit_about(Ev::EachAttacker, 0, &snap);
    g.run_pending();
    assert!(perm(&g, small).tapped, "the defending player's creature");
    assert!(!perm(&g, hulk).tapped, "not another player's");
}

#[test]
fn desolation_of_smaugs_mana_is_only_for_dragons() {
    let db = db();
    let d = deck(&db, &["Desolation of Smaug", "Mountain", "Ultron Drone"]);
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 4, 'R');
    g.players[0].hand = vec![0, 2];
    assert!(g.cast(0, Src::Hand(0), CastOpts::default()));
    assert_eq!(
        g.players[0].pool, 0,
        "no mana left for anything but a Dragon"
    );
    assert_eq!(g.players[0].hand, vec![2]);
}

#[test]
fn spells_need_their_targets() {
    let db = db();
    let d = deck(&db, &["Helicarrier Strike", "Plains", "Ultron Drone"]);
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 1, 'W');
    g.players[0].hand = vec![0];
    let foe = put(&mut g, 1, 2);
    assert!(
        !g.can_cast(0, Src::Hand(0), CastOpts::default()),
        "no attacking or blocking creature"
    );
    let i = g.idx(foe).unwrap();
    g.perms[i].attacking = true;
    assert!(g.can_cast(0, Src::Hand(0), CastOpts::default()));
}

#[test]
fn tokens_keep_their_quoted_abilities() {
    let db = db();
    let d = deck(
        &db,
        &[
            "Redcap Gutter-Dweller",
            "Ka-Zar of the Savage Land",
            "Forest",
        ],
    );
    let mut g = game(&[&d, &d]);
    // "Create two 1/1 black Rat creature tokens with 'This creature can't block.'"
    put(&mut g, 0, 0);
    let rats: Vec<&Perm> = g.perms.iter().filter(|x| x.tok == Tok::Creature).collect();
    assert_eq!(rats.len(), 2);
    assert!(rats.iter().all(|x| x.has(CANT_BLOCK)));
    // Zabu: legendary, and grows on landfall.
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 1);
    let zabu = |g: &Game| -> Vec<u32> {
        g.perms
            .iter()
            .filter(|x| x.tok == Tok::Creature && x.is(T_LEGENDARY))
            .map(|x| x.id)
            .collect()
    };
    let z = zabu(&g);
    assert_eq!(z.len(), 1);
    put(&mut g, 0, 2);
    assert_eq!(perm(&g, z[0]).counters, 1, "landfall");
    // A second Ka-Zar: the legend rule keeps one Zabu.
    put(&mut g, 0, 1);
    assert_eq!(zabu(&g).len(), 1);
}

#[test]
fn hawkeyes_arrows_cost_mana() {
    let db = db();
    let d = deck(&db, &["Hawkeye, Master Marksman", "Mountain"]);
    let mut g = game(&[&d, &d]);
    let h = put(&mut g, 0, 0);
    let life = g.players[1].life;
    let i = g.idx(h).unwrap();
    g.perms[i].tapped = true;
    g.emit(Ev::Tapped, 0, Some(h));
    g.run_pending();
    assert_eq!(g.players[1].life, life, "no mana: no arrows");
    lands(&mut g, 0, 1, 'R');
    g.emit(Ev::Tapped, 0, Some(h));
    g.run_pending();
    assert_eq!(tapped_lands(&g, 0), 1, "{{1}} paid for an arrow");
}

#[test]
fn enrage_triggers_once_per_combat_damage_step() {
    let db = db();
    let d = deck(&db, &["Red Hulk", "Ultron Drone"]);
    let mut g = game(&[&d, &d]);
    let hulk = put(&mut g, 0, 0);
    let a = put(&mut g, 1, 1);
    let b = put(&mut g, 1, 1);
    let at = vec![Attack {
        attacker: hulk,
        def: Def::Player(1),
        blockers: vec![a, b],
        blocked: true,
    }];
    g.damage_step(0, &at, false);
    g.run_pending();
    assert_eq!(
        perm(&g, hulk).counters,
        1,
        "dealt damage once (two blockers)"
    );
}

#[test]
fn the_master_of_lake_town_mills_whoever_lost_life() {
    let db = db();
    let d = deck(&db, &["The Master of Lake-town", "Island"]);
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 0);
    g.lose_life(1, 2);
    g.run_pending();
    assert_eq!(g.players[1].library.len(), 28, "that player, that many");
    assert_eq!(g.players[0].library.len(), 30);
}

#[test]
fn combat_damage_bookkeeping() {
    let d = prep(&[
        card(
            "Leech",
            "{1}{B}",
            "Creature — Bat",
            "Lifelink",
            Some(("3", "1")),
        ),
        card("Ox", "{3}{W}", "Creature — Ox", "", Some(("2", "6"))),
        card(
            "Sage",
            "{3}{U}",
            "Legendary Planeswalker — Sage",
            "+1: Draw a card.",
            None,
        ),
    ]);
    // Lifelink: damage a shield counter prevents gains nothing.
    let mut g = game(&[&d, &d, &d]);
    let leech = put(&mut g, 0, 0);
    let ox = put(&mut g, 1, 1);
    let i = g.idx(ox).unwrap();
    g.perms[i].shield = 1;
    let life = g.players[0].life;
    let at = vec![Attack {
        attacker: leech,
        def: Def::Player(1),
        blockers: vec![ox],
        blocked: true,
    }];
    g.damage_step(0, &at, false);
    assert_eq!(g.players[0].life, life, "prevented: no life");
    // A planeswalker hit by a lifelinker: loyalty down, life up.
    let mut g = game(&[&d, &d, &d]);
    let leech = put(&mut g, 0, 0);
    let sage = put(&mut g, 1, 2);
    let (life, loyalty) = (g.players[0].life, perm(&g, sage).loyalty);
    let at = vec![Attack {
        attacker: leech,
        def: Def::Walker(sage),
        blockers: vec![],
        blocked: false,
    }];
    g.damage_step(0, &at, false);
    assert_eq!(g.players[0].life, life + 3);
    assert_eq!(perm(&g, sage).loyalty, loyalty - 3);
    // Commander damage counts for the commander's owner, whoever controls it.
    let mut g = game(&[&d, &d, &d]);
    let cmd = put(&mut g, 0, 1);
    let i = g.idx(cmd).unwrap();
    g.perms[i].commander = 1;
    g.perms[i].ctrl = 1;
    let at = vec![Attack {
        attacker: cmd,
        def: Def::Player(2),
        blockers: vec![],
        blocked: false,
    }];
    g.damage_step(1, &at, false);
    assert_eq!(g.players[2].cmd_damage[0][1], 2, "P0's commander");
    assert_eq!(g.players[2].cmd_damage[1][1], 0);
}

#[test]
fn infinity_stone_and_graveyard_hate() {
    let db = db();
    let d = deck(
        &db,
        &[
            "The Mind Stone",
            "Plains",
            "Angel of Finality",
            "Ultron Drone",
        ],
    );
    // The Mind Stone's "∞" end step flicker needs it harnessed, and never takes a land.
    let mut g = game(&[&d, &d]);
    let ms = put(&mut g, 0, 0);
    let land = put(&mut g, 0, 1);
    let angel = put(&mut g, 0, 2);
    g.players[1].graveyard = vec![3, 3];
    g.phase_triggers(0, Phase::End);
    assert!(g.idx(angel).is_some(), "not harnessed: nothing happens");
    let i = g.idx(ms).unwrap();
    g.perms[i].powered = true;
    g.phase_triggers(0, Phase::End);
    assert!(g.idx(land).is_some(), "a land isn't a nonland permanent");
    assert!(g.idx(angel).is_none(), "the Angel was flickered");
    // Angel of Finality: "exile target player's graveyard".
    assert!(g.players[1].graveyard.is_empty());
}

#[test]
fn enters_tapped_doesnt_untap_and_vehicles() {
    let db = db();
    let d = deck(
        &db,
        &[
            "Whiplash, Vengeful Engineer",
            "Enchanted River's Grasp",
            "Ultron Drone",
            "Great Gilded Boat",
        ],
    );
    let mut g = game(&[&d, &d]);
    // "Whiplash enters tapped."
    let i = g.enter_card(0, 0, 0, false).unwrap();
    assert!(g.perms[i].tapped);
    // Enchanted River's Grasp: the creature loses its abilities and stays tapped.
    let drone = put(&mut g, 1, 2);
    let i = g.enter_card(0, 1, 0, false).unwrap();
    g.perms[i].attached = Some(drone);
    g.run_pending();
    g.sba();
    let x = perm(&g, drone);
    assert!(x.tapped);
    assert!(x.blank && g.abilities_of(x).is_empty());
    assert!(!x.locked, "it can still block");
    // A Vehicle that arrived this turn, crewed, still can't attack.
    let boat = g.enter_card(0, 3, 0, false).unwrap();
    let boat = g.perms[boat].id;
    put(&mut g, 0, 2);
    put(&mut g, 0, 2);
    assert!(g.crew(0, boat));
    let i = g.idx(boat).unwrap();
    assert!(g.perms[i].is_creature() && !g.can_attack(i));
}

// ------------------------------------------------------------ casting and mana decisions

fn dump_log(g: &Game) {
    for l in g.log.iter().flatten() {
        eprintln!("{}", l);
    }
}

#[test]
fn ai_plays_the_tapped_land_on_a_quiet_turn() {
    let db = db();
    let d = prep(&[
        Card::basic_land("Swamp", 'B'),
        db.get("Bloodfell Caves").unwrap().clone(),
        card("Ogre", "{3}{B}", "Creature — Ogre", "", Some(("4", "4"))),
        card("Bear", "{2}{B}", "Creature — Bear", "", Some(("3", "3"))),
        card("Elf", "{2}{G}", "Creature — Elf", "", Some(("3", "3"))),
    ]);
    // Two lands out, a 4-drop and an off-colour 3-drop in hand: an untapped land's mana
    // would go unused.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 2, 'B');
    g.players[0].hand = vec![0, 1, 2, 4];
    ai::main_phase(&mut g, 0, false);
    assert!(
        g.perms.iter().any(|x| x.card == Some((0, 1))),
        "Bloodfell Caves played on the quiet turn"
    );
    // With a 3-drop to cast, the untapped land comes down and the Bear is cast.
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 2, 'B');
    g.players[0].hand = vec![0, 1, 3];
    ai::main_phase(&mut g, 0, true);
    assert!(g.perms.iter().any(|x| x.card == Some((0, 3))), "Bear cast");
    assert!(g.players[0].hand.contains(&1), "the tapped land waits");
}

#[test]
fn ai_casts_with_its_mana_creature_instead_of_attacking() {
    let db = db();
    let d = prep(&[
        Card::basic_land("Forest", 'G'),
        Card::basic_land("Swamp", 'B'),
        db.get("Undercover Skrull").unwrap().clone(),
        card("Bear", "{2}{B}", "Creature — Bear", "", Some(("3", "3"))),
    ]);
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 1, 'G');
    lands(&mut g, 0, 1, 'B');
    put(&mut g, 0, 2);
    g.players[0].hand = vec![3];
    // Before combat: the 3-drop needs the Skrull's mana, so it's cast now (the Skrull
    // would otherwise attack and the Bear stay in hand).
    ai::main_phase(&mut g, 0, false);
    assert!(
        g.perms.iter().any(|x| x.card == Some((0, 3))),
        "Bear cast before combat with the Skrull's mana"
    );
}

#[test]
fn ai_pays_big_costs_from_big_sources_and_floats_the_rest() {
    let db = db();
    let d = prep(&[
        Card::basic_land("Swamp", 'B'),
        db.get("Doc Samson, Super Psychiatrist").unwrap().clone(),
        card(
            "Imp",
            "{1}",
            "Artifact Creature — Imp",
            "",
            Some(("1", "1")),
        ),
    ]);
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 1, 'B');
    let doc = put(&mut g, 0, 1);
    // {1}: the Swamp pays, not Doc Samson's five.
    assert!(g.pay(0, &Mana::generic(1)));
    assert!(!perm(&g, doc).tapped, "Doc Samson isn't tapped for {{1}}");
    // {2} needs Doc Samson (X = his power): what's left over floats in the pool.
    let made = perm(&g, doc).p as u8;
    assert!(made >= 3);
    assert!(g.pay(0, &Mana::generic(2)));
    assert_eq!(g.players[0].pool, made - 2, "surplus mana floats");
    assert!(g.can_pay(0, &Mana::generic(made as u32 - 2)));
}

#[test]
fn ai_uses_thanos_power_up_the_turn_he_enters() {
    let db = db();
    let d = prep(&[
        db.get("Thanos, the Mad Titan").unwrap().clone(),
        Card::basic_land("Mountain", 'R'),
        Card::basic_land("Plains", 'W'),
        Card::basic_land("Swamp", 'B'),
        Card::basic_land("Island", 'U'),
        Card::basic_land("Forest", 'G'),
        Card::basic_land("Wastes", 'C'),
        card("Troll", "{2}{G}", "Creature — Troll", "", Some(("4", "4"))),
        card(
            "Golem",
            "{3}",
            "Artifact Creature — Golem",
            "",
            Some(("4", "4")),
        ),
    ]);
    let mut g = game(&[&d, &d]);
    for k in 1..=6 {
        put(&mut g, 0, k);
    }
    g.players[0].command = vec![(0, 0)];
    // A 3-drop competes for the mana left after Thanos.
    g.players[0].hand = vec![8];
    let masque = put(&mut g, 1, 7);
    let masque2 = put(&mut g, 1, 7);
    g.log = Some(Vec::new());
    ai::main_phase(&mut g, 0, false);
    let thanos = g.perms.iter().find(|x| x.card == Some((0, 0)));
    if thanos.is_none_or(|t| !t.powered) {
        dump_log(&g);
    }
    assert!(thanos.is_some_and(|t| t.powered), "cast and powered up");
    assert!(g.idx(masque).is_none() && g.idx(masque2).is_none());
}

#[test]
fn ai_does_not_fire_the_parity_wipe_at_a_net_loss() {
    let db = db();
    let d = prep(&[
        db.get("Thanos, the Mad Titan").unwrap().clone(),
        Card::basic_land("Wastes", 'C'),
        Card::basic_land("Plains", 'W'),
        Card::basic_land("Island", 'U'),
        Card::basic_land("Swamp", 'B'),
        Card::basic_land("Mountain", 'R'),
        Card::basic_land("Forest", 'G'),
        card("Rat", "{B}", "Creature — Rat", "", Some(("1", "1"))),
        card("Troll", "{2}{G}", "Creature — Troll", "", Some(("4", "4"))),
    ]);
    let mut g = Game::new(&[&d, &d, &d, &d], &Rules::default(), Rng::new(7));
    for p in g.players.iter_mut() {
        p.hand.clear();
        p.library = vec![0; 30];
        p.command.clear();
    }
    g.round = 6;
    for k in 1..=6 {
        put(&mut g, 0, k);
    }
    let thanos = put(&mut g, 0, 0);
    let i = g.idx(thanos).unwrap();
    g.perms[i].born = 1;
    g.perms[i].entered_turn = false;
    // Odd kills my Troll for one opponent's Rat; even kills nothing (and the power-up still
    // puts two counters on Thanos).
    let troll = put(&mut g, 0, 8);
    let rat = put(&mut g, 1, 7);
    ai::main_phase(&mut g, 0, false);
    assert!(g.idx(troll).is_some(), "no wipe of my 4/4 for a 1/1");
    assert!(g.idx(rat).is_some());
}

#[test]
fn ai_does_not_cast_demonic_pact_without_a_way_out() {
    let db = db();
    let d = prep(&[
        Card::basic_land("Swamp", 'B'),
        db.get("Demonic Pact").unwrap().clone(),
    ]);
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 5, 'B');
    g.players[0].hand = vec![1];
    ai::main_phase(&mut g, 0, true);
    assert_eq!(g.players[0].hand, vec![1], "Demonic Pact stays in hand");
}

#[test]
fn filter_lands_turn_other_mana_into_any_colour() {
    let db = db();
    let d = prep(&[
        db.get("Surveillance Room").unwrap().clone(),
        Card::basic_land("Mountain", 'R'),
    ]);
    let mut g = game(&[&d, &d]);
    let room = put(&mut g, 0, 0);
    assert!(
        !g.can_pay(0, &Mana::parse("{B}")),
        "alone it only makes {{C}}"
    );
    assert!(g.can_pay(0, &Mana::generic(1)));
    let mountain = put(&mut g, 0, 1);
    // The Mountain's {R} pays the filter's {1}: one {B}, and nothing else.
    assert!(g.can_pay(0, &Mana::parse("{B}")));
    assert!(!g.can_pay(0, &Mana::parse("{1}{B}")));
    assert!(g.pay(0, &Mana::parse("{B}")));
    assert!(perm(&g, room).tapped && perm(&g, mountain).tapped);
}

#[test]
fn ai_uses_abilities_that_work_from_the_hand() {
    let db = db();
    let d = prep(&[
        Card::basic_land("Forest", 'G'),
        db.get("Sureshot Sower").unwrap().clone(),
        card(
            "Drake",
            "{3}{U}",
            "Creature — Drake",
            "Flying",
            Some(("4", "4")),
        ),
    ]);
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 4, 'G');
    let drake = put(&mut g, 1, 2);
    g.players[0].hand = vec![1];
    ai::main_phase(&mut g, 0, false);
    assert!(g.idx(drake).is_none(), "{{3}}{{G}}, discard Sureshot Sower");
    assert!(g.players[0].graveyard.contains(&1));
}
