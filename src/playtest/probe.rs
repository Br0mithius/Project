//! What the simulator actually does with a card: each ability is fired on the same
//! three-player board and the changes are listed, to check against the rules text.
//! Reading mistakes anywhere (parser, compiler or resolver) show up here as wrong changes.

use super::compile::*;
use super::resolve::Ctx;
use super::state::*;
use super::{Game, Prepared};
use crate::analysis::Rng;
use crate::card::Card;
use crate::db::Rules;

const WHO: [&str; 3] = ["You", "Opp A", "Opp B"];
const WHOSE: [&str; 3] = ["Your", "Opp A's", "Opp B's"];

// Indices into the shared card list.
const PROBED: u16 = 0;
const BEAR: u16 = 1;
const OGRE: u16 = 2;
const KNIGHT: u16 = 3;
const DRAGON: u16 = 4;
const RELIC: u16 = 5;
const BANNER: u16 = 6;
const LANDS: u16 = 7;

fn filler(name: &str, cost: &str, ty: &str, text: &str, pt: Option<(&str, &str)>) -> Card {
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

fn deck(card: &Card) -> Prepared {
    let mut cards = vec![
        card.clone(),
        filler("Bear", "{1}{G}", "Creature — Bear", "", Some(("2", "2"))),
        filler("Ogre", "{2}{R}", "Creature — Ogre", "", Some(("3", "3"))),
        filler(
            "Knight",
            "{2}{W}{W}",
            "Creature — Human Knight",
            "",
            Some(("4", "4")),
        ),
        filler(
            "Dragon",
            "{4}{R}{R}",
            "Creature — Dragon",
            "Flying",
            Some(("6", "6")),
        ),
        filler("Relic", "{1}", "Artifact", "", None),
        filler("Banner", "{1}{W}", "Enchantment", "", None),
    ];
    for (name, c) in [
        ("Plains", 'W'),
        ("Island", 'U'),
        ("Swamp", 'B'),
        ("Mountain", 'R'),
        ("Forest", 'G'),
    ] {
        cards.push(Card::basic_land(name, c));
    }
    Prepared {
        cards: cards.iter().map(compile).collect(),
        library: Vec::new(),
        commanders: Vec::new(),
    }
}

/// The standard board: each player has a 2/2, 3/3, 4/4 and a 6/6 flyer (mana values 2, 3,
/// 4, 6), an artifact, an enchantment, one basic of each colour, 3 cards in hand, 20 in the
/// library (basics and Ogres alternating) and 2 creature cards in the graveyard. Returns the game and your Bear's id.
fn board<'a>(decks: &[&'a Prepared]) -> (Game<'a>, u32) {
    let mut g = Game::new(decks, &Rules::default(), Rng::new(11));
    g.round = 3;
    g.active = 0;
    let mut bear = 0;
    for p in 0..g.players.len() {
        let pl = &mut g.players[p];
        pl.hand = vec![BEAR; 3];
        // Basics and Ogres alternating, so searches and digs have lands to find.
        pl.library = (0..20)
            .map(|k| {
                if k % 2 == 0 {
                    LANDS + (k / 2) % 5
                } else {
                    OGRE
                }
            })
            .collect();
        pl.graveyard = vec![DRAGON, OGRE];
        pl.command.clear();
        for idx in [BEAR, OGRE, KNIGHT, DRAGON, RELIC, BANNER]
            .into_iter()
            .chain(LANDS..LANDS + 5)
        {
            let i = g.enter_card(p, idx, p, false).unwrap();
            g.perms[i].sick = false;
            if p == 0 && idx == BEAR {
                bear = g.perms[i].id;
            }
        }
    }
    g.pending.clear();
    g.recalc();
    // The graveyards' cards were there before this turn.
    g.begin_turn_extra();
    (g, bear)
}

#[derive(Clone, PartialEq)]
struct PermSnap {
    id: u32,
    name: String,
    ctrl: u8,
    p: i16,
    t: i16,
    counters: i16,
    other: i16,
    loyalty: i16,
    tapped: bool,
    kw: u32,
    locked: bool,
    goad: bool,
    attached: Option<u32>,
    shield: u8,
    stun: u8,
    blank: bool,
    creature: bool,
    returns: bool,
    named: Option<String>,
}

#[derive(Clone)]
struct Snap {
    life: Vec<i32>,
    poison: Vec<u8>,
    zones: Vec<[usize; 4]>,
    hijacked: Vec<bool>,
    extra_combats: Vec<u8>,
    extra_turns: Vec<u8>,
    alive: Vec<bool>,
    playable: Vec<usize>,
    by_toughness: Vec<bool>,
    emblems: Vec<usize>,
    cheaper: Vec<usize>,
    attacks_changed: Vec<bool>,
    free_next: Vec<bool>,
    fog: bool,
    end_turn: bool,
    perms: Vec<PermSnap>,
}

fn perm_name(g: &Game, x: &Perm) -> String {
    match x.card {
        Some(r) if x.tok == Tok::None && x.transformed => g
            .card(r)
            .back
            .as_ref()
            .map_or_else(|| g.card(r).name.clone(), |b| b.name.clone()),
        Some(r) if x.tok == Tok::None => g.card(r).name.clone(),
        Some(r) => format!("{} (token copy)", g.card(r).name),
        None if x.tok == Tok::Creature => {
            let sub = x.subtypes.join(" ");
            format!("{}/{} {} token", x.base_p, x.base_t, sub).replace("  ", " ")
        }
        None => match x.tok {
            Tok::Equip(p, t) => format!("Equipment token ({:+}/{:+})", p, t),
            _ => format!("{:?} token", x.tok),
        },
    }
}

fn snap(g: &Game) -> Snap {
    Snap {
        life: g.players.iter().map(|p| p.life).collect(),
        poison: g.players.iter().map(|p| p.poison).collect(),
        zones: g
            .players
            .iter()
            .map(|p| {
                [
                    p.hand.len(),
                    p.library.len(),
                    p.graveyard.len(),
                    p.exile.len(),
                ]
            })
            .collect(),
        hijacked: g.players.iter().map(|p| p.hijacked).collect(),
        extra_combats: g.players.iter().map(|p| p.extra_combats).collect(),
        extra_turns: g.players.iter().map(|p| p.extra_turns).collect(),
        alive: g.players.iter().map(|p| p.alive).collect(),
        playable: g.players.iter().map(|p| p.playable.len()).collect(),
        by_toughness: g.players.iter().map(|p| p.toughness_damage_turn).collect(),
        emblems: g.players.iter().map(|p| p.emblems.len()).collect(),
        cheaper: g.players.iter().map(|p| p.turn_reduction.len()).collect(),
        attacks_changed: g
            .players
            .iter()
            .map(|p| p.attacked_power != 0 || p.attack_bonus.is_some())
            .collect(),
        free_next: g.players.iter().map(|p| p.free_next.is_some()).collect(),
        fog: g.fog,
        end_turn: g.end_turn,
        perms: g
            .perms
            .iter()
            .map(|x| PermSnap {
                id: x.id,
                name: perm_name(g, x),
                ctrl: x.ctrl,
                p: x.p,
                t: x.t,
                counters: x.counters,
                other: x.other_counters,
                loyalty: x.loyalty,
                tapped: x.tapped,
                kw: x.eff_kw,
                locked: x.locked || x.lock_effect,
                goad: x.goad.is_some(),
                attached: x.attached,
                shield: x.shield,
                stun: x.stun,
                blank: x.blank || x.no_abilities,
                creature: x.is_creature(),
                returns: x.returns_on_death != 0,
                named: x.named.map(|r| g.card(r).name.clone()),
            })
            .collect(),
    }
}

fn kw_names(bits: u32) -> String {
    KEYWORDS
        .iter()
        .chain(
            [
                ("can't block", CANT_BLOCK),
                ("damage to it prevented", NO_DAMAGE),
            ]
            .iter(),
        )
        .filter(|(_, b)| bits & b != 0)
        .map(|(n, _)| *n)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Names of rule-changing statics (`R_*`).
fn rule_names(bits: u32) -> String {
    [
        (R_CANT_LOSE, "you can't lose the game"),
        (
            R_NO_ETB_TRIGGERS,
            "artifacts and creatures entering trigger nothing",
        ),
        (R_DEFENDERS_ATTACK, "your defenders can attack"),
        (
            R_TOUGHNESS_DAMAGE,
            "your creatures deal combat damage by toughness when greater",
        ),
        (R_NEGATIVE_POWER, "negative power deals combat damage"),
        (
            R_OPP_NO_CAST_MY_TURN,
            "opponents can't cast spells during your turn",
        ),
        (R_LANDS_FROM_GY, "lands playable from your graveyard"),
        (
            R_PERMANENTS_FROM_GY,
            "one permanent of each type castable from your graveyard each turn",
        ),
        (R_FREE_FROM_HAND, "spells from your hand are free"),
        (
            R_FREE_BY_CREATURES,
            "spells from your hand with MV <= your creatures are free",
        ),
        (R_FLASH_SPELLS, "your spells have flash"),
        (
            R_FLASH_AFTER_OPPONENT,
            "your spells have flash once an opponent cast one this turn",
        ),
        (
            R_FIRST_CREATURE_FLASH,
            "your first creature spell each turn has flash",
        ),
        (R_STASH, "stashed cards playable on your turn"),
        (
            R_NAMES_CARD,
            "names an opponent's card: its activated abilities stop",
        ),
        (
            R_FREE_FIRST_EQUIP,
            "the first equip each turn is free with an enduring story",
        ),
        (
            R_SPELLS_UNCOUNTERABLE,
            "your instants and sorceries can't be countered",
        ),
        (R_IMPROVISE, "your noncreature spells have improvise"),
        (
            R_ABILITIES_HASTE,
            "your creatures' abilities ignore summoning sickness",
        ),
        (R_OPP_CREATURES_TAPPED, "opponents' creatures enter tapped"),
        (R_EXTRA_COUNTER, "one extra +1/+1 counter on your creatures"),
        (R_WALKERS_STAY, "your planeswalkers stay at 0 loyalty"),
        (
            R_OPP_DIE_EXILE,
            "opponents' creatures are exiled instead of dying",
        ),
        (
            R_PREVENT_NONCOMBAT,
            "noncombat damage to your other creatures is prevented",
        ),
    ]
    .iter()
    .filter(|(b, _)| bits & b != 0)
    .map(|(_, n)| *n)
    .collect::<Vec<_>>()
    .join("; ")
}

/// "{2}{U}{U}".
fn mana_text(m: &Mana) -> String {
    let mut s = String::new();
    if m.x {
        s.push_str("{X}");
    }
    if m.generic > 0 || m.total() == 0 {
        s.push_str(&format!("{{{}}}", m.generic));
    }
    for _ in 0..m.colorless {
        s.push_str("{C}");
    }
    for &h in m.hybrid.iter().filter(|&&h| h != 0) {
        let cs: Vec<String> = (0..5)
            .filter(|i| h & (1 << i) != 0)
            .map(|i| crate::card::COLORS[i].to_string())
            .collect();
        s.push_str(&format!("{{{}}}", cs.join("/")));
    }
    for (i, &n) in m.pips.iter().enumerate() {
        for _ in 0..n {
            s.push_str(&format!("{{{}}}", crate::card::COLORS[i]));
        }
    }
    s
}

fn signed(n: i64) -> String {
    format!("{:+}", n)
}

/// Lists what changed between two snapshots, one line per change.
fn diff(a: &Snap, b: &Snap, skip: Option<u32>) -> Vec<String> {
    let mut out = Vec::new();
    for p in 0..a.life.len() {
        let who = WHO[p];
        let mut bits = Vec::new();
        if a.life[p] != b.life[p] {
            bits.push(format!("life {}", signed((b.life[p] - a.life[p]) as i64)));
        }
        if a.poison[p] != b.poison[p] {
            bits.push(format!(
                "poison {}",
                signed(b.poison[p] as i64 - a.poison[p] as i64)
            ));
        }
        for (k, zone) in ["hand", "library", "graveyard", "exile"].iter().enumerate() {
            let d = b.zones[p][k] as i64 - a.zones[p][k] as i64;
            if d != 0 {
                bits.push(format!("{} {}", zone, signed(d)));
            }
        }
        if !a.hijacked[p] && b.hijacked[p] {
            bits.push("next turn controlled by an opponent".into());
        }
        if b.extra_combats[p] > a.extra_combats[p] {
            bits.push("extra combat".into());
        }
        if b.extra_turns[p] > a.extra_turns[p] {
            bits.push("extra turn".into());
        }
        if b.playable[p] != a.playable[p] {
            bits.push(format!(
                "cards playable from exile {}",
                signed(b.playable[p] as i64 - a.playable[p] as i64)
            ));
        }
        if b.emblems[p] > a.emblems[p] {
            bits.push("gets an emblem".into());
        }
        if b.attacks_changed[p] && !a.attacks_changed[p] {
            bits.push("attacks changed until their next turn".into());
        }
        if b.cheaper[p] > a.cheaper[p] {
            bits.push("spells cost less this turn".into());
        }
        if b.free_next[p] && !a.free_next[p] {
            bits.push("the next matching spell this turn is free".into());
        }
        if b.by_toughness[p] && !a.by_toughness[p] {
            bits.push("creatures deal combat damage by toughness this turn".into());
        }
        if a.alive[p] && !b.alive[p] {
            bits.push("loses the game".into());
        }
        if !bits.is_empty() {
            out.push(format!("{}: {}", who, bits.join(", ")));
        }
    }
    if b.end_turn && !a.end_turn {
        out.push("The turn ends".into());
    } else if b.fog && !a.fog {
        out.push("Combat damage prevented this turn".into());
    }
    for x in &a.perms {
        // (A player who lost took their permanents with them.)
        if Some(x.id) == skip || !b.alive[x.ctrl as usize] && a.alive[x.ctrl as usize] {
            continue;
        }
        match b.perms.iter().find(|y| y.id == x.id) {
            None => out.push(format!(
                "{} {} left the battlefield",
                WHOSE[x.ctrl as usize], x.name
            )),
            Some(y) if y != x => {
                let mut bits = Vec::new();
                if y.ctrl != x.ctrl {
                    bits.push(format!("now controlled by {}", WHO[y.ctrl as usize]));
                }
                if (y.p, y.t) != (x.p, x.t) {
                    bits.push(format!("{}/{} -> {}/{}", x.p, x.t, y.p, y.t));
                }
                if y.counters != x.counters {
                    bits.push(format!(
                        "+1/+1 counters {}",
                        signed((y.counters - x.counters) as i64)
                    ));
                }
                if y.other != x.other {
                    bits.push(format!(
                        "other counters {}",
                        signed((y.other - x.other) as i64)
                    ));
                }
                if y.loyalty != x.loyalty {
                    bits.push(format!(
                        "loyalty {}",
                        signed((y.loyalty - x.loyalty) as i64)
                    ));
                }
                if y.shield != x.shield {
                    bits.push("shield counter".into());
                }
                if y.stun != x.stun {
                    bits.push(format!(
                        "stun counters {}",
                        signed(y.stun as i64 - x.stun as i64)
                    ));
                }
                if y.name != x.name {
                    bits.push(format!("becomes {}", y.name));
                }
                if y.blank && !x.blank {
                    bits.push("loses all abilities".into());
                }
                if y.creature && !x.creature {
                    bits.push("becomes a creature".into());
                }
                if y.returns && !x.returns {
                    bits.push("returns to the battlefield if it dies this turn".into());
                }
                if let (Some(n), None) = (&y.named, &x.named) {
                    bits.push(format!("names {}", n));
                }
                if y.tapped != x.tapped {
                    bits.push(if y.tapped { "tapped" } else { "untapped" }.into());
                }
                // ("Can't attack or block" says it already.)
                let lock = if y.locked && !x.locked { CANT_BLOCK } else { 0 };
                let gained = y.kw & !x.kw & !lock;
                let lost = x.kw & !y.kw;
                if gained != 0 {
                    bits.push(format!("gains {}", kw_names(gained)));
                }
                if lost != 0 {
                    bits.push(format!("loses {}", kw_names(lost)));
                }
                if y.locked && !x.locked {
                    bits.push("can't attack or block".into());
                }
                if y.goad && !x.goad {
                    bits.push("goaded".into());
                }
                if !bits.is_empty() {
                    out.push(format!(
                        "{} {}: {}",
                        WHOSE[x.ctrl as usize],
                        x.name,
                        bits.join(", ")
                    ));
                }
            }
            _ => {}
        }
    }
    for y in &b.perms {
        if !a.perms.iter().any(|x| x.id == y.id) {
            let body = if y.t > 0 {
                format!(" {}/{}", y.p, y.t)
            } else {
                String::new()
            };
            let kw = if y.kw != 0 {
                format!(" with {}", kw_names(y.kw))
            } else {
                String::new()
            };
            out.push(format!(
                "{} gets {}{}{}",
                WHO[y.ctrl as usize], y.name, body, kw
            ));
        }
    }
    if out.is_empty() {
        out.push("(nothing changes)".into());
    }
    out
}

fn trig_label(t: &Trig) -> String {
    match t {
        Trig::Activated {
            mana,
            tap,
            sac_self,
            sac_other,
            discard,
            life,
            sorcery,
            extra,
        } => {
            let mut parts = Vec::new();
            if *mana > 0 {
                parts.push(format!("{} mana", mana));
            }
            if extra.x > 0 {
                parts.push(format!("{{X}} x{}", extra.x));
            }
            if extra.remove_all {
                parts.push(format!(
                    "remove any number of {} counters (X)",
                    if extra.remove_plus { "+1/+1" } else { "other" }
                ));
            } else if extra.remove > 0 {
                parts.push(format!(
                    "remove {} {} counter(s)",
                    extra.remove,
                    if extra.remove_plus { "+1/+1" } else { "other" }
                ));
            }
            if extra.add_counter > 0 {
                parts.push(
                    if extra.add_counter == 1 {
                        "stun counter on it"
                    } else {
                        "a counter on it"
                    }
                    .into(),
                );
            }
            if extra.tap_others > 0 {
                parts.push(format!(
                    "tap {} untapped {}",
                    extra.tap_others,
                    extra
                        .tap_subtype
                        .as_deref()
                        .unwrap_or(if extra.tap_types == T_ARTIFACT {
                            "artifacts"
                        } else {
                            "creatures"
                        })
                ));
            }
            if extra.exile_self {
                parts.push("exile it".into());
            }
            if extra.return_self {
                parts.push("return it to hand".into());
            }
            if extra.from_graveyard {
                parts.push("from the graveyard, exiling it".into());
            }
            if extra.from_hand {
                parts.push("from the hand".into());
            }
            if extra.exile_cards > 0 {
                parts.push(format!(
                    "exile {} other {}card(s) from the graveyard",
                    extra.exile_cards,
                    if extra.exile_creatures {
                        "creature "
                    } else {
                        ""
                    }
                ));
            }
            if extra.black_pips > 0 {
                parts.push(format!(
                    "exile black cards with {}+ black pips from the graveyard",
                    extra.black_pips
                ));
            }
            if extra.boast {
                parts.push("boast".into());
            }
            if *tap {
                parts.push("tap".into());
            }
            if *sac_self {
                parts.push("sacrifice it".into());
            }
            if *sac_other {
                parts.push("sacrifice another".into());
            }
            if *discard {
                parts.push("discard".into());
            }
            if *life > 0 {
                parts.push(format!("{} life", life));
            }
            if *sorcery {
                parts.push("sorcery speed".into());
            }
            if parts.is_empty() {
                parts.push("free".into());
            }
            format!("Activated ({})", parts.join(", "))
        }
        other => format!("{:?}", other),
    }
}

/// Fires one ability (or one mode of it) from a fresh board and lists the changes.
fn fire(prep: &Prepared, ab_index: usize, back: bool, mode: Option<usize>) -> Vec<String> {
    let decks = [prep, prep, prep];
    let (mut g, bear) = board(&decks);
    let card: &SimCard = &prep.cards[PROBED as usize];
    let ab = if back {
        &card.back.as_ref().unwrap().abilities[ab_index]
    } else {
        &card.abilities[ab_index]
    };
    // A permanent fires from the battlefield; an instant or sorcery from nowhere; an ability
    // that works from the graveyard from there; a dies trigger once the card has died.
    let mut died = None;
    // X costs are paid with X = 3.
    let x = card.cost.x.then_some(3);
    g.cur_x = x;
    g.entering_x = x.filter(|_| card.is_permanent() && !back && !ab.from_graveyard);
    // A transforming card's back face fires from the transformed permanent.
    let transformed = back
        && card
            .back
            .as_ref()
            .is_some_and(|b| b.kind == BackKind::Transform);
    let src = if ab.from_graveyard {
        g.players[0].graveyard.push(PROBED);
        None
    } else if card.is_permanent() && (!back || transformed) {
        let i = g.enter_card(0, PROBED, 0, false);
        let id = i.map(|i| g.perms[i].id);
        if let Some(i) = i {
            g.perms[i].sick = false;
            if transformed {
                g.transform(i);
            }
            if card.is(T_EQUIPMENT) || card.is(T_AURA) {
                g.perms[i].attached = Some(bear);
            }
        }
        g.pending.clear();
        g.recalc();
        // (Not "when enchanted/equipped creature dies": that's about another creature.)
        if ab.trig == Trig::Dies
            && (ab.text.starts_with("when ~") || ab.text.starts_with("whenever ~"))
        {
            // It has died: off the battlefield (as last seen) and in the graveyard.
            if let Some(i) = id.and_then(|id| g.idx(id)) {
                died = Some(g.perms.remove(i));
                g.players[0].graveyard.push(PROBED);
                g.recalc();
            }
            None
        } else {
            id
        }
    } else {
        None
    };
    // Forcing "if target player has exactly 10 life": Opp A has 10.
    if let (Some(_), Cond::OpponentLifeExactly(n)) = (mode, &ab.cond) {
        g.players[1].life = *n;
    }
    let before = snap(&g);
    let snapshot = died.or_else(|| src.and_then(|id| g.idx(id)).map(|i| g.perms[i].clone()));
    let groups = ab.mode_groups();
    // What a triggered ability's event was about ("it"/"that creature"): your Bear. Other
    // abilities have no such creature ("its power" is the card's own).
    let subject = matches!(ab.trig, Trig::Event(..)).then_some(bear);
    match mode {
        // One mode of a modal ability, or forcing an ability past its condition.
        Some(k) if k < groups.len() || ab.cond != Cond::Always || ab.pay.is_some() => {
            let (text, card_restr) = if k < groups.len() && ab.modal {
                ab.mode_text(k)
            } else {
                (ab.text.as_str(), ab.card_restr)
            };
            let ctx = Ctx {
                ctrl: 0,
                src,
                snap: snapshot,
                card: Some(&prep.cards[PROBED as usize]),
                card_ref: Some((0, PROBED)),
                subject,
                text,
                until_eot: ab.until_eot,
                overload: false,
                card_restr,
                x: None,
                paid: None,
                teamwork: false,
                about: None,
            };
            if let Some(r) = groups.get(k) {
                for e in &ab.effects[r.clone()] {
                    g.apply(&ctx, e);
                }
            } else {
                // A cost paid as it resolves is the payment, not an effect.
                let cost = ab
                    .pay
                    .as_ref()
                    .map(|p| p.skip_from as usize..p.first as usize)
                    .unwrap_or(0..0);
                for (j, e) in ab.effects.iter().enumerate() {
                    if !cost.contains(&j) {
                        g.apply(&ctx, e);
                    }
                }
            }
        }
        // The real path: conditions, "may" choices and special cases as in a game.
        _ => {
            g.pending.push(Pending {
                ctrl: 0,
                source: src,
                card: (0, PROBED),
                back,
                ability: ab_index as u16,
                snapshot,
                subject,
                teamwork: false,
                gift: false,
                from_graveyard: false,
                kicked: false,
                overloaded: false,
                x: None,
            });
        }
    }
    g.run_pending();
    g.sba();
    g.recalc();
    diff(&before, &snap(&g), None)
}

fn describe_abilities(prep: &Prepared, back: bool, out: &mut Vec<String>) {
    let card = &prep.cards[PROBED as usize];
    let list = if back {
        &card.back.as_ref().unwrap().abilities
    } else {
        &card.abilities
    };
    for (k, ab) in list.iter().enumerate() {
        let mut flags = Vec::new();
        if ab.cond != Cond::Always {
            flags.push(format!("only if {:?}", ab.cond));
        }
        if ab.optional {
            flags.push("may".into());
        }
        if ab.once {
            flags.push("once".into());
        }
        if ab.once_per_turn {
            flags.push("once each turn".into());
        }
        if ab.second_draw {
            flags.push(
                if ab.text.contains("second spell") {
                    "the second spell each turn"
                } else {
                    "the second card drawn each turn"
                }
                .into(),
            );
        }
        if let Some(p) = &ab.pay {
            flags.push(format!(
                "{}pays {:?} for effects {}..",
                if p.optional { "may " } else { "" },
                p.cost,
                p.first
            ));
        }
        let flags = if flags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", flags.join("; "))
        };
        out.push(format!(
            "  {}{}{}: \"{}\"",
            if back { "Back: " } else { "" },
            trig_label(&ab.trig),
            flags,
            ab.text
        ));
        if ab.effects.is_empty() {
            out.push("      (no effects read)".into());
            continue;
        }
        if ab.trig == Trig::Static {
            out.push("      (static: shown under \"On entering\")".into());
            continue;
        }
        if ab.modal && ab.mode_groups().len() > 1 {
            for m in 0..ab.mode_groups().len() {
                for line in fire(prep, k, back, Some(m)) {
                    out.push(format!("      mode {}: {}", m + 1, line));
                }
            }
        } else {
            for line in fire(prep, k, back, None) {
                out.push(format!("      -> {}", line));
            }
            if ab.cond != Cond::Always || ab.pay.is_some() {
                let what = if ab.cond != Cond::Always {
                    "the condition holds"
                } else {
                    "the cost is paid"
                };
                for line in fire(prep, k, back, Some(usize::MAX)) {
                    out.push(format!("      if {} -> {}", what, line));
                }
            }
        }
    }
}

/// Everything the simulator does with this card on the standard board.
pub fn probe(card: &Card) -> String {
    let prep = deck(card);
    let c = &prep.cards[PROBED as usize];
    let mut out = vec![format!(
        "{} | {} | {}{}",
        card.name,
        card.mana_cost,
        card.type_line,
        card.power_toughness()
            .map(|pt| format!(" | {}", pt))
            .unwrap_or_default()
    )];
    out.push(format!("TEXT: {}", card.oracle_text.replace('\n', " / ")));
    out.push(
        "BOARD: each player has Bear 2/2 (MV2), Ogre 3/3 (MV3), Knight 4/4 (MV4), Dragon 6/6 \
         flying (MV6), Relic (artifact), Banner (enchantment), 5 basics, 3 cards in hand, a \
         library of 10 basics and 10 Ogres, 2 creature cards (Dragon, Ogre) in the graveyard, \
         30 life. Triggers fire as if their event happened \
         (\"it\"/\"that creature\" = your Bear). Abilities resolve as in a game; one with a condition (or a cost paid as it resolves) is also forced, to show what it does when the condition holds (or the cost is paid). Activation costs are listed, not paid."
            .into(),
    );
    let mut sim = Vec::new();
    if c.kw != 0 {
        sim.push(format!("keywords {}", kw_names(c.kw)));
    }
    if c.ward > 0 || c.ward_cost != WardCost::default() {
        let w = &c.ward_cost;
        let mut parts = Vec::new();
        if c.ward > 0 {
            parts.push(format!("{{{}}}", c.ward));
        }
        if w.life > 0 {
            parts.push(format!("pay {} life", w.life));
        }
        if w.discard {
            parts.push("discard a card".into());
        }
        if w.sacrifice > 0 {
            parts.push(format!("sacrifice {} permanents", w.sacrifice));
        }
        if w.poison > 0 {
            parts.push(format!("get {} poison", w.poison));
        }
        let join = if w.discard_or_mana { " or " } else { ", " };
        sim.push(format!("ward {}", parts.join(join)));
    }
    if let Some(m) = &c.flashback {
        sim.push(format!(
            "flashback {}{}",
            mana_text(m),
            if c.flashback_discard {
                ", discard a card"
            } else {
                ""
            }
        ));
    }
    if let Some(m) = &c.kicker {
        sim.push(format!("kicker {}", mana_text(m)));
    }
    if let Some(e) = &c.extra_cost {
        sim.push(format!("additional cost {:?}", e));
    }
    if let Some(e) = c.back.as_ref().and_then(|b| b.extra_cost.as_ref()) {
        sim.push(format!("back face additional cost {:?}", e));
    }
    if let Some(s) = &c.self_less {
        sim.push(format!("costs X less, X = {:?}", s));
    }
    if c.power_up_less > 0 {
        sim.push(format!(
            "other creatures' power-ups cost {} less",
            c.power_up_less
        ));
    }
    if c.behold_untapped {
        sim.push("enters untapped if you can behold a Jace".into());
    }
    if c.produces != 0 {
        sim.push(format!(
            "makes mana (colour mask {:#08b}, {} at a time)",
            c.produces, c.mana_amount
        ));
    }
    if c.prowess > 1 {
        sim.push(format!("prowess x{}", c.prowess));
    }
    if c.changeling {
        sim.push("changeling".into());
    }
    if c.cant_be_countered {
        sim.push("can't be countered".into());
    }
    if let Some(cond) = &c.cast_cond {
        sim.push(format!("can be cast only if {:?}", cond));
    }
    if let Some(so) = &c.spend_only {
        let what = |types: u16, sub: &Option<String>| {
            let mut w = Vec::new();
            if let Some(s) = sub {
                w.push(s.clone());
            }
            if types != ANY_TYPE {
                for (n, b) in [
                    ("creature", T_CREATURE),
                    ("artifact", T_ARTIFACT),
                    ("planeswalker", T_PLANESWALKER),
                    ("equipment", T_EQUIPMENT),
                ] {
                    if types & b != 0 {
                        w.push(n.to_string());
                    }
                }
            }
            w.join(" ")
        };
        let mut parts = Vec::new();
        if so.spell_types != 0 {
            parts.push(format!(
                "{}{} spells{}",
                if so.spell_not != 0 { "non" } else { "" },
                what(
                    if so.spell_not != 0 {
                        so.spell_not
                    } else {
                        so.spell_types
                    },
                    &so.spell_sub
                ),
                if so.not_from_hand {
                    " not cast from hand"
                } else {
                    ""
                }
            ));
        }
        if so.ability_types != 0 {
            parts.push(format!(
                "abilities of {} sources",
                what(so.ability_types, &so.ability_sub)
            ));
        }
        sim.push(format!(
            "that mana only for {}{}",
            parts
                .join(" or ")
                .replace("  ", " ")
                .trim()
                .replace("of  ", "of "),
            if c.free_mask != 0 {
                " (its {C} for anything)"
            } else {
                ""
            }
        ));
    }
    if c.enters_tapped {
        sim.push("enters tapped".into());
    }
    if c.cost.x {
        sim.push("X = 3".into());
    }
    if c.protection != 0 {
        sim.push(format!("protection from colours {:#07b}", c.protection));
    }
    for (label, m) in [("overload", &c.overload), ("sneak", &c.sneak)] {
        if let Some(m) = m {
            sim.push(format!("{} {}", label, mana_text(m)));
        }
    }
    for (label, on) in [
        ("convoke", c.convoke),
        ("improvise", c.improvise),
        ("extort", c.extort),
    ] {
        if on {
            sim.push(label.into());
        }
    }
    if let Some(m) = &c.equip {
        sim.push(format!(
            "equip {}{}",
            mana_text(m),
            if c.equip_life > 0 {
                format!(", pay {} life", c.equip_life)
            } else {
                String::new()
            }
        ));
    }
    if let Some((m, what)) = &c.equip_for {
        sim.push(format!("equip {} {}", what, mana_text(m)));
    }
    for (label, v) in [
        ("crew", c.crew),
        ("teamwork", c.teamwork),
        ("offspring", c.offspring),
    ] {
        if let Some(n) = v {
            sim.push(format!("{} {}", label, n));
        }
    }
    if let Some((n, what)) = &c.cycling {
        sim.push(format!(
            "cycling {} ({})",
            n,
            if what.is_empty() { "draw" } else { what }
        ));
    }
    if let Some((n, f)) = &c.cost_reduction {
        let first = if c.reduction_first {
            "the first such spell each turn: "
        } else {
            ""
        };
        if c.reduction_by_power {
            sim.push(format!(
                "{}spells cost X less, X = its power ({:?})",
                first, f
            ));
        } else {
            sim.push(format!("{}spells cost {} less ({:?})", first, n, f));
        }
    }
    for r in &c.self_reduce {
        sim.push(match r {
            SelfReduce::If(n, cond) => format!("costs {} less if {:?}", n, cond),
            SelfReduce::Each(n, what) => format!("costs {} less for each {}", n, what),
        });
    }
    if let Some((n, f)) = &c.opp_tax {
        sim.push(format!("opponents' spells cost {} more ({:?})", n, f));
    }
    if let Some((n, cond)) = &c.attack_tax {
        sim.push(format!(
            "attacking you costs {} per creature{}",
            n,
            if *cond == Cond::Always {
                String::new()
            } else {
                format!(" while {:?}", cond)
            }
        ));
    }
    let rules = c.rules & !(R_SPELL_TAX | R_ATTACK_TAX);
    if rules != 0 {
        sim.push(format!("rules: {}", rule_names(rules)));
    }
    if c.hexproof_from != 0 {
        sim.push(format!(
            "hexproof from {}",
            (0..6)
                .filter(|i| c.hexproof_from & (1 << i) != 0)
                .map(|i| ["white", "blue", "black", "red", "green", "instants"][i])
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(a) = &c.attached_sets {
        sim.push(format!(
            "makes its permanent a {}/{}{}{}",
            a.power,
            a.toughness,
            if a.creature { " creature" } else { "" },
            if a.blank { " with no abilities" } else { "" }
        ));
    }
    if c.equip_target_less > 0 {
        sim.push(format!("equipping it costs {} less", c.equip_target_less));
    }
    if c.equip_less_per_counter {
        sim.push("equip costs 1 less per +1/+1 counter on the target".into());
    }
    if let Some(cond) = &c.enters_tapped_unless {
        sim.push(format!("enters tapped unless {:?}", cond));
    }
    if let Some(b) = c.back.as_ref().filter(|b| b.kind == BackKind::Transform) {
        let mut back = format!("transforms into {} {}/{}", b.name, b.power, b.toughness);
        if b.kw != 0 {
            back.push_str(&format!(" {}", kw_names(b.kw)));
        }
        if b.rules != 0 {
            back.push_str(&format!(" ({})", rule_names(b.rules)));
        }
        sim.push(back);
    }
    if !sim.is_empty() {
        out.push(format!("SIM: {}", sim.join("; ")));
    }
    if c.is_permanent() {
        let decks = [&prep, &prep, &prep];
        let (mut g, bear) = board(&decks);
        let before = snap(&g);
        g.entering_x = c.cost.x.then_some(3);
        g.cur_x = g.entering_x;
        if let Some(i) = g.enter_card(0, PROBED, 0, false) {
            let id = g.perms[i].id;
            if c.is(T_EQUIPMENT) || c.is(T_AURA) {
                g.perms[i].attached = Some(bear);
            }
            g.run_pending();
            g.sba();
            g.recalc();
            out.push(
                if c.is(T_EQUIPMENT) || c.is(T_AURA) {
                    "On entering (attached to your Bear):"
                } else {
                    "On entering:"
                }
                .into(),
            );
            let mut after = snap(&g);
            let mine = after.perms.iter().find(|x| x.id == id).cloned();
            if let Some(m) = &mine {
                out.push(format!(
                    "      itself: {}{}{}",
                    if m.t > 0 {
                        format!("{}/{} ", m.p, m.t)
                    } else {
                        String::new()
                    },
                    kw_names(m.kw),
                    m.named
                        .as_ref()
                        .map(|n| format!(" (names {})", n))
                        .unwrap_or_default()
                ));
            }
            after.perms.retain(|x| x.id != id);
            for line in diff(&before, &after, None) {
                out.push(format!("      -> {}", line));
            }
        } else {
            out.push("On entering: it doesn't survive".into());
        }
    }
    if c.abilities.is_empty() && c.back.is_none() {
        out.push("ABILITIES: none read".into());
    } else {
        out.push("ABILITIES:".into());
        describe_abilities(&prep, false, &mut out);
        if c.back.is_some() {
            describe_abilities(&prep, true, &mut out);
        }
    }
    out.join("\n")
}

/// Every card's probe, minus the board description, in name order: the reference the
/// snapshot test compares against.
pub fn snapshot(db: &crate::db::CardDb) -> String {
    let mut cards: Vec<&Card> = db.cards.iter().collect();
    cards.sort_by(|a, b| a.name.cmp(&b.name));
    cards.dedup_by(|a, b| a.name == b.name);
    cards
        .iter()
        .map(|c| {
            probe(c)
                .lines()
                .filter(|l| !l.starts_with("BOARD:"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
        + "\n"
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    fn by_card(text: &str) -> BTreeMap<String, String> {
        text.split("\n\n")
            .filter(|s| !s.trim().is_empty())
            .map(|s| {
                let name = s
                    .lines()
                    .next()
                    .unwrap_or("")
                    .split(" | ")
                    .next()
                    .unwrap_or("");
                (name.to_string(), s.to_string())
            })
            .collect()
    }

    /// What the simulator does with every card is pinned in `data/probe_snapshot.txt`, so
    /// a change to how any card plays can't slip in unnoticed. After checking that each
    /// listed change is right, accept them with
    /// `UPDATE_PROBE_SNAPSHOT=1 cargo test --release probe_snapshot`.
    #[test]
    fn probe_snapshot() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
        let db = crate::db::CardDb::load(&dir.join("cards.json")).unwrap();
        let now = super::snapshot(&db);
        let path = dir.join("probe_snapshot.txt");
        if std::env::var_os("UPDATE_PROBE_SNAPSHOT").is_some() {
            std::fs::write(&path, &now).unwrap();
            return;
        }
        let old = std::fs::read_to_string(&path).unwrap_or_default();
        if old == now {
            return;
        }
        let (a, b) = (by_card(&old), by_card(&now));
        let mut changed = Vec::new();
        for (name, text) in &b {
            match a.get(name) {
                Some(t) if t == text => {}
                Some(t) => changed.push(format!("--- {}\n{}\n+++ now\n{}", name, t, text)),
                None => changed.push(format!("+++ new card\n{}", text)),
            }
        }
        for name in a.keys().filter(|n| !b.contains_key(*n)) {
            changed.push(format!("--- removed: {}", name));
        }
        panic!(
            "{} card(s) now play differently. Check each, then accept with \
             UPDATE_PROBE_SNAPSHOT=1 cargo test --release probe_snapshot\n\n{}",
            changed.len(),
            changed
                .iter()
                .take(15)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n\n")
        );
    }
}
