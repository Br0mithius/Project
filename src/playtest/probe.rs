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
}

#[derive(Clone)]
struct Snap {
    life: Vec<i32>,
    poison: Vec<u8>,
    zones: Vec<[usize; 4]>,
    hijacked: Vec<bool>,
    extra_combats: Vec<u8>,
    fog: bool,
    perms: Vec<PermSnap>,
}

fn perm_name(g: &Game, x: &Perm) -> String {
    match x.card {
        Some(r) if x.tok == Tok::None => g.card(r).name.clone(),
        Some(r) => format!("{} (token copy)", g.card(r).name),
        None if x.tok == Tok::Creature => {
            let sub = x.subtypes.join(" ");
            format!("{}/{} {} token", x.base_p, x.base_t, sub).replace("  ", " ")
        }
        None => format!("{:?} token", x.tok),
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
        fog: g.fog,
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
            })
            .collect(),
    }
}

fn kw_names(bits: u32) -> String {
    KEYWORDS
        .iter()
        .filter(|(_, b)| bits & b != 0)
        .map(|(n, _)| *n)
        .collect::<Vec<_>>()
        .join(", ")
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
        if !bits.is_empty() {
            out.push(format!("{}: {}", who, bits.join(", ")));
        }
    }
    if b.fog && !a.fog {
        out.push("Combat damage prevented this turn".into());
    }
    for x in &a.perms {
        if Some(x.id) == skip {
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
                if y.tapped != x.tapped {
                    bits.push(if y.tapped { "tapped" } else { "untapped" }.into());
                }
                let gained = y.kw & !x.kw;
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
        } => {
            let mut parts = Vec::new();
            if *mana > 0 {
                parts.push(format!("{} mana", mana));
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
    let src = if ab.from_graveyard {
        g.players[0].graveyard.push(PROBED);
        None
    } else if card.is_permanent() && !back {
        let i = g.enter_card(0, PROBED, 0, false);
        let id = i.map(|i| g.perms[i].id);
        if let Some(i) = i {
            g.perms[i].sick = false;
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
    let before = snap(&g);
    let snapshot = died.or_else(|| src.and_then(|id| g.idx(id)).map(|i| g.perms[i].clone()));
    match mode {
        // One mode of a modal ability, or forcing an ability past its condition.
        Some(k) if k < ab.effects.len() || ab.cond != Cond::Always => {
            let ctx = Ctx {
                ctrl: 0,
                src,
                snap: snapshot,
                card: Some(&prep.cards[PROBED as usize]),
                card_ref: Some((0, PROBED)),
                subject: Some(bear),
                text: &ab.text,
                until_eot: ab.until_eot,
                overload: false,
            };
            if k < ab.effects.len() {
                // A mode: all of its effects.
                let mode = |j: usize| ab.modes.get(j).copied().unwrap_or(j as u8) as usize;
                for j in (0..ab.effects.len()).filter(|&j| mode(j) == k) {
                    g.apply(&ctx, &ab.effects[j]);
                }
            } else {
                for e in &ab.effects {
                    g.apply(&ctx, e);
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
                subject: Some(bear),
                teamwork: false,
                gift: false,
                from_graveyard: false,
                kicked: false,
                overloaded: false,
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
        let n_modes = ab.modes.iter().map(|&m| m as usize + 1).max().unwrap_or(0);
        if ab.modal && ab.effects.len() > 1 {
            for m in 0..n_modes.max(1) {
                for line in fire(prep, k, back, Some(m)) {
                    out.push(format!("      mode {}: {}", m + 1, line));
                }
            }
        } else {
            for line in fire(prep, k, back, None) {
                out.push(format!("      -> {}", line));
            }
            if ab.cond != Cond::Always {
                for line in fire(prep, k, back, Some(usize::MAX)) {
                    out.push(format!("      if the condition holds -> {}", line));
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
         (\"it\"/\"that creature\" = your Bear). Abilities resolve as in a game; one with a condition is also forced, to show what it does when the condition holds."
            .into(),
    );
    let mut sim = Vec::new();
    if c.kw != 0 {
        sim.push(format!("keywords {}", kw_names(c.kw)));
    }
    if c.ward > 0 {
        sim.push(format!("ward {}", c.ward));
    }
    if c.produces != 0 {
        sim.push(format!(
            "makes mana (colour mask {:#08b}, {} at a time)",
            c.produces, c.mana_amount
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
    for (label, m) in [
        ("flashback", c.flashback),
        ("kicker", c.kicker),
        ("overload", c.overload),
        ("sneak", c.sneak),
    ] {
        if let Some(m) = m {
            sim.push(format!("{} {} mana", label, m.total()));
        }
    }
    for (label, on) in [
        ("convoke", c.convoke),
        ("improvise", c.improvise),
        ("affinity for artifacts", c.affinity),
        ("extort", c.extort),
    ] {
        if on {
            sim.push(label.into());
        }
    }
    for (label, v) in [
        ("equip", c.equip),
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
        sim.push(format!("spells cost {} less ({:?})", n, f));
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
                    "      itself: {}{}",
                    if m.t > 0 {
                        format!("{}/{} ", m.p, m.t)
                    } else {
                        String::new()
                    },
                    kw_names(m.kw)
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
