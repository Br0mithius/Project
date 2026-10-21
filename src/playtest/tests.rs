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
    g.perms[i].born = 0;
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
        ],
    );
    let mut g = game(&[&d, &d]);
    lands(&mut g, 0, 6, 'R');
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
    assert_eq!(g.delayed_returns.len(), 1);
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
        assert!(w < 4);
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
