//! Hand-checked readings of what effects may hit, on real league cards, through the probe
//! (each ability fired on the standard board: every player has Bear 2/2 (mana value 2),
//! Ogre 3/3 (3), Knight 4/4 Human (4), Dragon 6/6 flier (6), Relic, Banner and basics; the
//! graveyards hold a Dragon and an Ogre; your Bear is the trigger's creature).

use super::probe::probe;
use crate::db::CardDb;

fn db() -> CardDb {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
    CardDb::load(&dir.join("cards.json")).unwrap()
}

/// The probe's lines for one card.
fn lines(db: &CardDb, name: &str) -> Vec<String> {
    let c = db.get(name).unwrap_or_else(|| panic!("{} not in db", name));
    probe(c).lines().map(|l| l.to_string()).collect()
}

/// The result lines of the ability whose text starts with `start` (or of mode `mode` of it).
fn ability(lines: &[String], start: &str, mode: Option<usize>) -> Vec<String> {
    let at = lines
        .iter()
        .position(|l| l.contains(&format!(": \"{}", start)))
        .unwrap_or_else(|| panic!("no ability \"{}\" in\n{}", start, lines.join("\n")));
    let mut out = Vec::new();
    for l in &lines[at + 1..] {
        if !l.starts_with("      ") {
            break;
        }
        let l = l.trim();
        match mode {
            Some(m) => {
                if let Some(rest) = l.strip_prefix(&format!("mode {}: ", m)) {
                    out.push(rest.to_string());
                }
            }
            None => out.push(l.trim_start_matches("-> ").to_string()),
        }
    }
    out
}

/// The "On entering" lines of a permanent.
fn entering(lines: &[String]) -> Vec<String> {
    let at = lines
        .iter()
        .position(|l| l.starts_with("On entering"))
        .expect("a permanent");
    lines[at + 1..]
        .iter()
        .take_while(|l| l.starts_with("      "))
        .map(|l| l.trim().trim_start_matches("-> ").to_string())
        .collect()
}

fn has(out: &[String], what: &str) -> bool {
    out.iter().any(|l| l.contains(what))
}

#[test]
fn restrictions_on_mass_and_modal_removal() {
    let db = db();
    // "Destroy all creatures with mana value 3 or less" / "... 4 or greater".
    let c = lines(&db, "Austere Command");
    let small = ability(&c, "choose", Some(3));
    assert!(has(&small, "Opp A's Bear left") && has(&small, "Opp A's Ogre left"));
    assert!(!has(&small, "Knight left") && !has(&small, "Dragon left"));
    let big = ability(&c, "choose", Some(4));
    assert!(has(&big, "Opp A's Knight left") && has(&big, "Opp A's Dragon left"));
    assert!(!has(&big, "Bear left") && !has(&big, "Ogre left"));
    // "Destroy all Dragon creatures" / "all non-Dragon creatures".
    let c = lines(&db, "Crux of Fate");
    let dragons = ability(&c, "choose", Some(1));
    assert!(has(&dragons, "Opp B's Dragon left") && !has(&dragons, "Bear left"));
    let others = ability(&c, "choose", Some(2));
    assert!(has(&others, "Opp B's Bear left") && !has(&others, "Dragon left"));
    // "Return all attacking creatures": nobody is attacking.
    let c = lines(&db, "Aetherize");
    assert_eq!(
        ability(&c, "return all attacking", None),
        vec!["(nothing changes)"]
    );
    // "Exile all attacking creatures target player controls": not yours, nobody attacking.
    let c = lines(&db, "Settle the Wreckage");
    assert_eq!(ability(&c, "exile all", None), vec!["(nothing changes)"]);
    // "Return all nonland permanents target player controls": one opponent's only.
    let c = lines(&db, "River's Rebuke");
    let out = ability(&c, "return all", None);
    assert!(has(&out, "Opp B's Dragon left") && !has(&out, "Your ") && !has(&out, "Opp A"));
}

#[test]
fn restrictions_on_targets() {
    let db = db();
    for (name, start, hit, spared) in [
        // "Target creature with flying".
        ("Sureshot Sower", "{3}{g}", "Dragon left", "Ogre left"),
        // "Black or green creature": the green Bear, not the red Ogre or Dragon.
        ("Essence Burn", "~ deals", "Bear left", "Ogre left"),
        // "Green or white creature": the white Knight.
        ("Deathmark", "destroy", "Knight left", "Ogre left"),
        // "Toughness 4 or greater".
        ("Surgical Precision", "choose", "Dragon left", "Ogre left"),
        // "Another target creature with power 2 or less can't be blocked".
        (
            "Goblin Smuggler",
            "{t}: another",
            "Your Bear: gains unblockable",
            "Ogre",
        ),
        // "Two +1/+1 counters on target Human you control": the Knight.
        (
            "Lake-town",
            "{2}{w}{u}",
            "Your Knight: 4/4 -> 6/6",
            "Dragon",
        ),
        // "Target Bear, Spider, or Wolf you control".
        ("Mirkwood", "{2}{b}{g}", "Your Bear: 2/2 -> 4/4", "Dragon"),
    ] {
        let c = lines(&db, name);
        let out = ability(&c, start, if start == "choose" { Some(1) } else { None });
        assert!(has(&out, hit), "{}: {:?}", name, out);
        assert!(!has(&out, spared), "{}: {:?}", name, out);
    }
    // "Target tapped creature" / "target attacking or blocking creature": none on the board.
    for (name, start) in [
        ("Deadly Riposte", "~ deals"),
        ("Elspeth's Smite", "~ deals"),
        ("Super Villain Lockup", "when ~ enters"),
        ("Pinecone Strike", "choose"),
    ] {
        let c = lines(&db, name);
        let out = ability(&c, start, (start == "choose").then_some(2));
        assert!(
            !has(&out, "left the battlefield"),
            "{} hits nothing: {:?}",
            name,
            out
        );
    }
    // "Destroy target nonbasic land": the board has basics only.
    let c = lines(&db, "Demolition Field");
    let out = ability(&c, "{2}, {t}", None);
    assert!(!has(&out, "Plains left"), "{:?}", out);
}

#[test]
fn restrictions_on_lords_and_statics() {
    let db = db();
    // "Other Angels you control get +1/+1 and have lifelink": no Angels on the board.
    assert_eq!(
        entering(&lines(&db, "Lyra Dawnbringer"))[1..],
        ["(nothing changes)"]
    );
    // "Other Bears you control get +2/+2".
    let e = entering(&lines(&db, "Beorn the Fierce"));
    assert!(has(&e, "Your Bear: 2/2 -> 4/4") && !has(&e, "Ogre"));
    // "Creatures you control of the chosen colour get +1/+0": red is the most common.
    let e = entering(&lines(&db, "Heraldic Banner"));
    assert!(has(&e, "Your Ogre") && has(&e, "Your Dragon") && !has(&e, "Your Bear"));
    // "Lands you control have hexproof".
    let e = entering(&lines(&db, "Marwyn, the Preserver"));
    assert!(has(&e, "Your Forest: gains hexproof") && !has(&e, "Your Dragon"));
    // "Attacking creatures you control get +1/+0": nobody attacks.
    let e = entering(&lines(&db, "Goblin Oriflamme"));
    assert!(!has(&e, "Your Bear"), "{:?}", e);
}

#[test]
fn restrictions_on_graveyard_and_library_cards() {
    let db = db();
    // "Creature card with mana value 2 or less": the graveyard holds a Dragon and an Ogre.
    let c = lines(&db, "Driver of the Dead");
    assert_eq!(ability(&c, "when ~ dies", None), vec!["(nothing changes)"]);
    // "Mana value 3 or less": the Ogre.
    let c = lines(&db, "Generous Revival");
    let out = ability(&c, "return target", None);
    assert!(has(&out, "You gets Ogre") && !has(&out, "Dragon"));
    // "Non-Bear creature card with mana value <= ~'s power (3)": the Ogre.
    let c = lines(&db, "Fiendish Panda");
    let out = ability(&c, "when ~ dies", None);
    assert!(has(&out, "You gets Ogre") && !has(&out, "Dragon"));
    // "Put all creature cards from all graveyards onto the battlefield".
    let c = lines(&db, "Rise of the Dark Realms");
    let out = ability(&c, "put all", None);
    assert_eq!(out.iter().filter(|l| l.contains("You gets")).count(), 6);
    // "Search your library for a Gate card": there's none.
    let c = lines(&db, "Maze's End");
    let out = ability(&c, "{3}, {t}", None);
    assert!(!has(&out, "You gets"), "{:?}", out);
    // "Return ~ from your graveyard": the probe activates it from the graveyard; only that
    // card comes back, not the best creature card there.
    let c = lines(&db, "Reassembling Skeleton");
    assert_eq!(
        ability(&c, "{1}{b}", None),
        vec!["You: graveyard -1", "You gets Reassembling Skeleton 1/1"]
    );
    // "A creature or planeswalker with the greatest mana value": the Dragon.
    let c = lines(&db, "Break Under Pressure");
    let out = ability(&c, "target opponent sacrifices", None);
    assert!(has(&out, "Dragon left") && !has(&out, "Bear left"));
}

#[test]
fn modes_keep_their_parts_together() {
    let db = db();
    // "• Destroy target creature with toughness 4 or greater. You gain 1 life."
    let c = lines(&db, "Surgical Precision");
    let m1 = ability(&c, "choose", Some(1));
    assert!(has(&m1, "life +1") && has(&m1, "left the battlefield"));
    assert!(has(&ability(&c, "choose", Some(2)), "life +2, hand +1"));
    assert!(ability(&c, "choose", Some(3)).is_empty(), "two modes only");
    // "• Put a +1/+1 counter on each creature you control. Those creatures gain vigilance."
    let c = lines(&db, "Felidar Retreat");
    let m2 = ability(&c, "whenever", Some(2));
    assert!(has(
        &m2,
        "Your Bear: 2/2 -> 3/3, +1/+1 counters +1, gains vigilance"
    ));
    // "Choose two" with a mode's own text ("target creature gets -5/-0" is one creature).
    let c = lines(&db, "Icy Reception");
    assert_eq!(ability(&c, "choose", Some(2)).len(), 1);
}

#[test]
fn each_player_includes_you() {
    let db = db();
    let c = lines(&db, "Cast Away Doubt");
    assert!(has(&ability(&c, "draw two", None), "You: life -2"));
    let c = lines(&db, "Liliana, Dreadhorde General");
    let out = ability(&c, "[-4]", None);
    assert!(has(&out, "Your Bear left") && has(&out, "Your Ogre left"));
    assert!(has(&out, "Opp A's Ogre left"));
    // Each player discards their hand, then draws seven: 3 cards out, 7 in.
    let c = lines(&db, "Dragon Mage");
    let out = ability(&c, "whenever ~ deals", None);
    assert!(
        has(&out, "You: hand +4, library -7, graveyard +3"),
        "{:?}",
        out
    );
    assert!(has(&out, "Opp B: hand +4"));
    let c = lines(&db, "Dread Summons");
    assert!(has(
        &ability(&c, "each player mills", None),
        "You: library -3"
    ));
    // "For each opponent, destroy up to one target creature that player controls".
    let c = lines(&db, "Lich's Relic");
    let out = ability(&c, "when ~ enters", None);
    assert!(has(&out, "Opp A's Dragon left") && has(&out, "Opp B's Dragon left"));
    // "Put a +1/+1 counter on each creature target player controls": yours.
    let c = lines(&db, "Twisted Fates");
    let out = ability(&c, "destroy target", None);
    assert!(has(&out, "Your Bear: 2/2 -> 3/3") && !has(&out, "Opp A's Bear"));
    // "Its controller draws a card": the destroyed creature's controller.
    let c = lines(&db, "Prophesied End");
    let out = ability(&c, "destroy target", None);
    assert!(has(&out, "Opp A: hand +1") && !has(&out, "You: hand"));
}

#[test]
fn trigger_filters() {
    let db = db();
    let label = |name: &str, start: &str| -> String {
        lines(&db, name)
            .into_iter()
            .find(|l| l.contains(&format!(": \"{}", start)))
            .unwrap_or_else(|| panic!("{}", name))
    };
    // Whole words: no "orc" in "sorcery", "another Elf" is an Elf.
    assert!(label("Archmage of Runes", "whenever").contains("subtype: None"));
    assert!(label("Dwynen's Elite", "when ~ enters").contains("Control(\"elf\", true)"));
    // "Another Dwarf or Equipment": either.
    let l = label("Kíli the Resourceful", "whenever");
    assert!(
        l.contains("types: 0") && l.contains("subtype dwarf/equipment"),
        "{}",
        l
    );
    // "You cast" is yours, "an opponent draws" is theirs.
    assert!(label("Brineborn Cutthroat", "whenever").contains("yours: Some(true)"));
    assert!(label("Gleaming Splendor", "whenever").contains("yours: Some(false)"));
    // "An attacking creature you control dies", "a Gate you control enters".
    assert!(label("Ares, God of War", "whenever").contains("Restr(attacking)"));
    assert!(label("Gateway Sneak", "whenever").contains("subtype gate"));
    assert!(label("Garruk's Uprising", "whenever").contains("power >= 4"));
    assert!(label("Mold Adder", "whenever").contains("colour blue/black"));
    // "Whenever a white creature you control attacks": about each attacker.
    let l = label("Linden, the Steadfast Queen", "whenever");
    assert!(
        l.contains("EachAttacker") && l.contains("colour white"),
        "{}",
        l
    );
}

/// A two-player game with these league cards in each deck list, nothing in play.
fn duel<'a>(d: &'a super::Prepared) -> super::Game<'a> {
    let mut g = super::Game::new(
        &[d, d],
        &crate::db::Rules::default(),
        crate::analysis::Rng::new(5),
    );
    for p in g.players.iter_mut() {
        p.hand.clear();
        p.library = vec![0; 20];
        p.graveyard.clear();
        p.command.clear();
    }
    g.pending.clear();
    g
}

fn prepared(db: &CardDb, names: &[&str]) -> super::Prepared {
    super::Prepared {
        cards: names
            .iter()
            .map(|n| super::compile::compile(db.get(n).unwrap()))
            .collect(),
        library: Vec::new(),
        commanders: Vec::new(),
    }
}

#[test]
fn attacking_creatures_in_a_game() {
    let db = db();
    // Goblin Oriflamme: "Attacking creatures you control get +1/+0."
    let d = prepared(&db, &["Goblin Oriflamme", "Bear Cub"]);
    let mut g = duel(&d);
    g.enter_card(0, 0, 0, false);
    let i = g.enter_card(0, 1, 0, false).unwrap();
    let bear = g.perms[i].id;
    g.run_pending();
    g.recalc();
    let p = |g: &super::Game| g.perms[g.idx(bear).unwrap()].p;
    assert_eq!(p(&g), 2, "not attacking");
    let i = g.idx(bear).unwrap();
    g.perms[i].attacking = true;
    g.recalc();
    assert_eq!(p(&g), 3, "attacking");
    g.end_combat();
    assert_eq!(p(&g), 2, "combat over");

    // Ares: "Whenever an attacking creature you control dies, return that card to its
    // owner's hand." Only attackers come back.
    let d = prepared(&db, &["Ares, God of War", "Bear Cub", "Bear Cub"]);
    let mut g = duel(&d);
    g.enter_card(0, 0, 0, false);
    let a = g.enter_card(0, 1, 0, false).unwrap();
    let attacker = g.perms[a].id;
    let h = g.enter_card(0, 2, 0, false).unwrap();
    let home = g.perms[h].id;
    g.run_pending();
    let a = g.idx(attacker).unwrap();
    g.perms[a].attacking = true;
    g.destroy(a);
    g.run_pending();
    let h = g.idx(home).unwrap();
    g.destroy(h);
    g.run_pending();
    assert_eq!(
        g.players[0].hand,
        vec![1],
        "the attacker came back, not the other"
    );
    assert_eq!(g.players[0].graveyard, vec![2]);
}

#[test]
fn it_and_that_creature() {
    let db = db();
    // "Put a +1/+1 counter on target creature. It gains lifelink": the same creature.
    let c = lines(&db, "Bard the Bowman");
    let out = ability(&c, "whenever", None);
    assert_eq!(
        out,
        vec!["Your Dragon: 6/6 -> 7/7, +1/+1 counters +1, gains lifelink"]
    );
    // "Whenever another creature you control enters, put a counter on that creature".
    let c = lines(&db, "Good-Fortune Unicorn");
    assert!(has(&ability(&c, "whenever", None), "Your Bear: 2/2 -> 3/3"));
    // "Create a Dwarf token, then attach ~ to it".
    let c = lines(&db, "Dwarven Shortsword");
    assert!(has(&ability(&c, "when ~ enters", None), "dwarf token 3/4"));
    // "Target Villain you control connives": the counter goes on a Villain (none here).
    let c = lines(&db, "Villainous Hideout");
    assert!(!has(&ability(&c, "{3}, {t}", None), "Villainous Hideout:"));
    // "It fights up to one target creature": a fight it survives.
    let c = lines(&db, "Mind Meanderer");
    let out = ability(&c, "when ~ enters", None);
    assert!(!has(&out, "Mind Meanderer left"), "{:?}", out);
}
