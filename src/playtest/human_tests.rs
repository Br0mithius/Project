//! The hand-played seat (`human`): scripted answers drive it through each kind of decision.

use super::combat::{Attack, Def};
use super::human::*;
use super::*;
use crate::card::Card;
use crate::db::CardDb;
use std::cell::Cell;
use std::rc::Rc;

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

fn prep(cards: &[Card]) -> Prepared {
    Prepared {
        cards: cards.iter().map(compile).collect(),
        library: Vec::new(),
        commanders: Vec::new(),
    }
}

fn game<'a>(decks: &[&'a Prepared]) -> Game<'a> {
    let mut g = Game::new(decks, &Rules::default(), Rng::new(7));
    for p in g.players.iter_mut() {
        p.hand.clear();
        p.library = vec![0; 30];
        p.command.clear();
    }
    g.round = 3;
    g.log = Some(Vec::new());
    g
}

fn put(g: &mut Game, p: usize, idx: u16) -> u32 {
    let i = g.enter_card(p, idx, p, false).unwrap();
    g.perms[i].sick = false;
    let id = g.perms[i].id;
    g.run_pending();
    g.sba();
    id
}

/// The human plays seat 0, answering with `f`; returns the asks it saw.
fn human(
    g: &mut Game,
    f: impl FnMut(&Ask, &GameView) -> Answer + 'static,
) -> Rc<std::cell::RefCell<Vec<Ask>>> {
    let io = FnIo::new(f);
    let asks = io.asks.clone();
    g.human = Some(Box::new(HumanLink::new(
        0,
        PlayOptions::default(),
        Box::new(io),
    )));
    asks
}

fn find(actions: &[ActOption], what: &str) -> Option<usize> {
    actions.iter().position(|a| a.label.contains(what))
}

fn named(g: &Game, p: usize, name: &str) -> usize {
    g.perms
        .iter()
        .filter(|x| x.ctrl as usize == p && g.perm_name(x) == name)
        .count()
}

#[test]
fn mulligans_and_bottoming_are_the_players_choice() {
    let mut d = prep(&[
        Card::basic_land("Forest", 'G'),
        card("Bear", "{1}{G}", "Creature — Bear", "", Some(("2", "2"))),
    ]);
    d.library = [vec![0u16; 30], vec![1u16; 30]].concat();
    let io = Rc::new(ScriptIo::new(vec![
        Answer::Yes(false),
        Answer::Yes(false),
        Answer::Yes(true),
        Answer::Pick(vec![0]),
    ]));
    let link = HumanLink::new(0, PlayOptions::default(), Box::new(SharedIo(io.clone())));
    let g = Game::new_human(&[&d, &d], &Rules::default(), Rng::new(3), link);
    assert_eq!(g.players[0].mulligans, 2);
    assert_eq!(
        g.players[0].hand.len(),
        6,
        "the free mulligan, then one to the bottom"
    );
    assert_eq!(g.players[0].library.len(), 54);
    let asks = io.asks.borrow();
    assert!(matches!(asks[0], Ask::Mulligan { mulligans: 0, .. }));
    assert!(matches!(asks[3], Ask::Pick { min: 1, max: 1, .. }));
}

#[test]
fn a_land_and_a_creature_paid_by_tapping_by_hand() {
    let d = prep(&[
        Card::basic_land("Forest", 'G'),
        card("Bear", "{1}{G}", "Creature — Bear", "", Some(("2", "2"))),
    ]);
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 0);
    g.players[0].hand = vec![0, 1];
    g.players[0].lands_played = 0;
    let cast = Rc::new(Cell::new(false));
    let c2 = cast.clone();
    let asks = human(&mut g, move |ask, view| match ask {
        Ask::Act { actions, .. } => {
            if let Some(k) = find(actions, "Play land") {
                Answer::Pick(vec![k])
            } else if let (Some(k), false) = (find(actions, "Cast Bear"), c2.get()) {
                c2.set(true);
                // Not paid yet: the engine asks for the mana.
                assert!(!actions[k].payable);
                assert_eq!(actions[k].missing, "{1}{G}");
                Answer::Pick(vec![k])
            } else {
                Answer::Pass
            }
        }
        Ask::Pay { .. } => {
            let s = view.sources.first().expect("an untapped Forest");
            Answer::Tap { id: s.id, color: 4 }
        }
        _ => Answer::Auto,
    });
    g.human_main(0, false);
    assert!(cast.get());
    assert_eq!(named(&g, 0, "Bear"), 1, "the Bear is cast");
    let forests: Vec<&Perm> = g
        .perms
        .iter()
        .filter(|x| x.ctrl == 0 && x.is(T_LAND))
        .collect();
    assert_eq!(forests.len(), 2);
    assert!(
        forests.iter().all(|x| x.tapped),
        "both Forests tapped by hand"
    );
    let pays = asks
        .borrow()
        .iter()
        .filter(|a| matches!(a, Ask::Pay { .. }))
        .count();
    assert_eq!(pays, 2, "one Forest tapped per Pay question");
    assert!(g.log.as_ref().unwrap().iter().any(|l| l.contains("taps")));
}

#[test]
fn tapped_mana_floats_until_the_phase_ends() {
    let d = prep(&[
        Card::basic_land("Forest", 'G'),
        card("Bear", "{1}{G}", "Creature — Bear", "", Some(("2", "2"))),
    ]);
    let mut g = game(&[&d, &d]);
    let f1 = put(&mut g, 0, 0);
    let f2 = put(&mut g, 0, 0);
    let f3 = put(&mut g, 0, 0);
    g.players[0].hand = vec![1];
    let step = Rc::new(Cell::new(0));
    let s2 = step.clone();
    human(&mut g, move |ask, _| {
        s2.set(s2.get() + 1);
        match (s2.get(), ask) {
            (1, _) => Answer::Tap { id: f1, color: 4 },
            (2, _) => Answer::Tap { id: f2, color: 4 },
            (3, _) => Answer::Tap { id: f3, color: 4 },
            (4, Ask::Act { actions, .. }) => {
                let k = find(actions, "Cast Bear").unwrap();
                assert!(actions[k].payable, "the pool pays for it");
                Answer::Pick(vec![k])
            }
            _ => Answer::Pass,
        }
    });
    g.human_main(0, false);
    assert_eq!(named(&g, 0, "Bear"), 1);
    assert!(
        g.log
            .as_ref()
            .unwrap()
            .iter()
            .any(|l| l.contains("mana pool empties ({G} unused)")),
        "the third {{G}} empties at the end of the phase"
    );
}

#[test]
fn removal_hits_the_target_the_player_picks_with_auto_tap() {
    let d0 = prep(&[
        Card::basic_land("Swamp", 'B'),
        card(
            "Kill",
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
            "{5}{R}{R}",
            "Creature — Dragon",
            "Flying",
            Some(("7", "7")),
        ),
    ]);
    let mut g = game(&[&d0, &d1]);
    for _ in 0..3 {
        put(&mut g, 0, 0);
    }
    put(&mut g, 1, 0);
    put(&mut g, 1, 1);
    g.players[0].hand = vec![1];
    let done = Rc::new(Cell::new(false));
    let d2 = done.clone();
    let asks = human(&mut g, move |ask, _| match ask {
        Ask::Act { actions, .. } if !d2.get() => {
            d2.set(true);
            Answer::AutoTap(find(actions, "Cast Kill").unwrap())
        }
        Ask::Pick {
            options, suggested, ..
        } => {
            // The bot would kill the Dragon; the player takes the Bear.
            assert!(options[suggested[0]].contains("Dragon"));
            Answer::Pick(vec![
                options.iter().position(|o| o.contains("Bear")).unwrap(),
            ])
        }
        _ => Answer::Pass,
    });
    g.human_main(0, false);
    assert_eq!(named(&g, 1, "Bear"), 0, "the Bear is destroyed");
    assert_eq!(named(&g, 1, "Dragon"), 1);
    assert!(asks.borrow().iter().any(|a| matches!(a, Ask::Pick { .. })));
}

#[test]
fn attacks_go_to_the_defender_the_player_names() {
    let d = prep(&[
        Card::basic_land("Forest", 'G'),
        card("Bear", "{1}{G}", "Creature — Bear", "", Some(("2", "2"))),
    ]);
    let mut g = game(&[&d, &d, &d]);
    let bear = put(&mut g, 0, 1);
    let life = g.players[2].life;
    human(&mut g, move |ask, _| match ask {
        Ask::Attack {
            defenders,
            attackers,
            ..
        } => {
            assert_eq!(attackers[0].0, bear);
            let k = defenders.iter().position(|d| d.starts_with("P2")).unwrap();
            Answer::Attacks(vec![(bear, k)])
        }
        _ => Answer::Pass,
    });
    g.active = 0;
    g.combat(0);
    assert_eq!(g.players[2].life, life - 2, "P2 took the Bear's 2");
    assert_eq!(g.players[1].life, life);
}

#[test]
fn blocks_are_the_players_choice() {
    let d = prep(&[
        card("Bear", "{1}{G}", "Creature — Bear", "", Some(("2", "2"))),
        card(
            "Knight",
            "{2}{W}",
            "Creature — Knight",
            "",
            Some(("4", "4")),
        ),
        card("Ogre", "{3}{R}", "Creature — Ogre", "", Some(("3", "3"))),
    ]);
    let mut g = game(&[&d, &d]);
    put(&mut g, 0, 0);
    let knight = put(&mut g, 0, 1);
    let ogre = put(&mut g, 1, 2);
    human(&mut g, move |ask, _| match ask {
        Ask::Block { blockers, .. } => {
            assert_eq!(blockers.len(), 2);
            Answer::Blocks(vec![(knight, ogre)])
        }
        _ => Answer::Pass,
    });
    let attacks = vec![Attack {
        attacker: ogre,
        def: Def::Player(0),
        blockers: Vec::new(),
        blocked: false,
    }];
    assert_eq!(g.human_blocks(0, &attacks), vec![(knight, ogre)]);
}

#[test]
fn a_may_ability_asks_the_player() {
    let d = prep(&[
        Card::basic_land("Forest", 'G'),
        card(
            "Scout",
            "{G}",
            "Creature — Elf Scout",
            "When this creature enters, you may draw a card.",
            Some(("1", "1")),
        ),
    ]);
    for (yes, drawn) in [(false, 0), (true, 1)] {
        let mut g = game(&[&d, &d]);
        let asks = human(&mut g, move |ask, _| match ask {
            Ask::YesNo { .. } => Answer::Yes(yes),
            _ => Answer::Pass,
        });
        put(&mut g, 0, 1);
        assert_eq!(g.players[0].hand.len(), drawn);
        assert_eq!(asks.borrow().len(), 1);
    }
}

#[test]
fn thanos_parity_is_the_players_choice() {
    let db = db();
    let d = prep(&[
        db.get("Thanos, the Mad Titan").unwrap().clone(),
        card("Bear", "{1}{G}", "Creature — Bear", "", Some(("2", "2"))),
        card(
            "Dragon",
            "{5}{R}{R}",
            "Creature — Dragon",
            "Flying",
            Some(("7", "7")),
        ),
    ]);
    let mut g = game(&[&d, &d]);
    let thanos = put(&mut g, 0, 0);
    put(&mut g, 1, 1);
    put(&mut g, 1, 2);
    // The bot would take the Dragon (odd); the player picks even.
    assert!(ai::best_parity(&g, 0, Some(thanos)).0);
    human(&mut g, |ask, _| match ask {
        Ask::Pick { options, .. } => {
            assert!(options[1].starts_with("Even (1 of theirs"));
            Answer::Pick(vec![1])
        }
        _ => Answer::Pass,
    });
    assert!(!g.h_parity(0, Some(thanos)).0);
}

#[test]
fn sandbox_edits_change_the_game_and_are_logged() {
    let d = prep(&[
        Card::basic_land("Forest", 'G'),
        card("Bear", "{1}{G}", "Creature — Bear", "", Some(("2", "2"))),
    ]);
    let mut g = game(&[&d, &d]);
    human(&mut g, |_, _| Answer::Pass);
    g.players[0].hand = vec![1];
    g.apply_edit(&Edit::Move {
        from: Loc::Hand(0, 0),
        to: Dest::Battlefield,
    });
    assert_eq!(named(&g, 0, "Bear"), 1);
    let bear = g.perms.iter().find(|x| x.ctrl == 0).unwrap().id;
    g.apply_edit(&Edit::Counters {
        id: bear,
        plus: 2,
        other: 0,
    });
    assert_eq!(g.perms[g.idx(bear).unwrap()].p, 4);
    g.apply_edit(&Edit::Tap { id: bear });
    assert!(g.perms[g.idx(bear).unwrap()].tapped);
    g.apply_edit(&Edit::SetLife { player: 1, life: 7 });
    assert_eq!(g.players[1].life, 7);
    g.apply_edit(&Edit::Move {
        from: Loc::Perm(bear),
        to: Dest::Graveyard,
    });
    assert!(g.idx(bear).is_none());
    assert_eq!(g.players[0].graveyard, vec![1]);
    let edits = g
        .log
        .as_ref()
        .unwrap()
        .iter()
        .filter(|l| l.contains("[manual edit]"))
        .count();
    assert_eq!(edits, 5);
}

fn test_setup(db: &CardDb, seed: u64) -> Setup {
    let deck = "Commander\n1 Thanos, the Mad Titan\n\nDeck\n8 Swamp\n8 Forest\n7 Island\n\
                1 Sol Ring\n1 Arcane Signet\n4 Vampire Nighthawk\n4 Kree Commandos\n\
                4 Ultron Drone\n4 Kree Sentinel\n4 Hero's Downfall\n4 Photon Blast Barrage\n\
                2 Massacre Wurm\n4 Celebrate the Mountain-king\n";
    let _ = db;
    Setup {
        seed,
        seat: 0,
        opts: PlayOptions::default(),
        rules: Rules::default(),
        decks: (0..4)
            .map(|p| (format!("deck {}", p), deck.to_string()))
            .collect(),
    }
}

#[test]
fn a_game_replays_exactly_from_its_answers() {
    let db = db();
    let setup = test_setup(&db, 11);
    // Played once with every question left to the bot...
    let io = Rc::new(ScriptIo::new(Vec::new()));
    let (w, answers, _) = play_with(&setup, &db, Box::new(SharedIo(io.clone())));
    let log = io.log.borrow().clone();
    assert!(!answers.is_empty());
    assert!(log.len() > 50);
    // ... and replayed from the recorded answers: the same game, line for line.
    let report = Report {
        note: String::new(),
        setup: setup.clone(),
        answers: answers.clone(),
    };
    let text = report_text(&report, None, "", &log);
    let back = parse_report(&text).expect("the replay data reads back");
    let again = replay(&back, &db);
    assert_eq!(log, again.log);
    assert!(again.crash.is_none());
    // A replay cut short stops at that question.
    let short = Report {
        answers: answers[..answers.len() / 2].to_vec(),
        ..report
    };
    let cut = replay(&short, &db);
    assert!(cut.log.len() < log.len() && cut.view.is_some());
    assert_eq!(cut.asks.len(), answers.len() / 2 + 1);
    assert!(
        cut.dump
            .as_deref()
            .is_some_and(|d| d.contains("== Battlefield"))
    );
    let _ = w;
}
