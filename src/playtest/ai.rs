//! The players: how each seat decides what to do. The aim is a careful, above-average
//! Commander player: it judges who's the biggest threat at the table, plans its mana each
//! turn, keeps up counterspells and instant-speed answers when they're worth more than
//! tapping out, points removal at what matters, attacks where it pays off without leaving
//! itself dead on the crack-back, blocks for value (and chumps only to survive), and uses
//! tricks, Fogs and flash creatures at the right moments.

use super::combat::{Attack, Def};
use super::compile::*;
use super::resolve::Ctx;
use super::state::*;
use super::{CastOpts, ExtraPay, FROM_LIBRARY_TOP, Src};
use crate::abilities::{Effect, Kind, Whose};
use crate::restrict::{Dyn, Restr};

// ------------------------------------------------------------ values

fn kw_value(kw: u32, p: f32) -> f32 {
    let mut v = 0.0;
    let has = |b: u32| kw & b != 0;
    if has(FLYING) {
        v += 0.5 + 0.35 * p;
    }
    if has(TRAMPLE) {
        v += 0.12 * p;
    }
    if has(MENACE) {
        v += 0.3 + 0.2 * p;
    }
    if has(UNBLOCKABLE) {
        v += 0.5 + 0.45 * p;
    }
    if has(DOUBLE_STRIKE) {
        v += 0.8 * p;
    } else if has(FIRST_STRIKE) {
        v += 0.3 + 0.2 * p;
    }
    if has(DEATHTOUCH) {
        v += 1.0;
    }
    if has(LIFELINK) {
        v += 0.3 * p;
    }
    if has(HEXPROOF | SHROUD) {
        v += 0.8;
    }
    if has(INDESTRUCTIBLE) {
        v += 1.5;
    }
    if has(VIGILANCE) {
        v += 0.3;
    }
    if has(REACH) {
        v += 0.3;
    }
    if has(FEAR | INTIMIDATE | SKULK) {
        v += 0.3 + 0.25 * p;
    }
    if has(DEFENDER) {
        v -= 0.6 * p;
    }
    v
}

/// What a permanent's abilities keep doing for its controller (beyond arrival effects).
fn engine_value(abilities: &[SimAbility]) -> f32 {
    let mut v = 0.0;
    for a in abilities {
        // Used from the hand ("{3}{G}, Discard this card: ..."): not something the
        // permanent does.
        if matches!(&a.trig, Trig::Activated { extra, .. } if extra.from_hand) {
            continue;
        }
        // "Choose one that hasn't been chosen": every mode happens in turn - including
        // "you lose the game" (Demonic Pact).
        let e: f32 = if a.modal && a.text.contains("hasn't been chosen") {
            if a.effects
                .iter()
                .any(|e| matches!(e, Effect::Rule(r) if r.contains("lose the game")))
            {
                -15.0
            } else {
                a.effects.iter().map(effect_value).sum()
            }
        } else if a.modal {
            let best = a.effects.iter().map(effect_value).fold(0.0, f32::max);
            // Hawkeye's Trick Arrows: each mode is paid for ({1}), at half a point a mana.
            match paid_modes(a) {
                Some((n, _)) => (best - 0.5 * n as f32).max(0.0),
                None => best,
            }
        } else {
            a.effects.iter().map(effect_value).sum()
        };
        let times = match &a.trig {
            Trig::Phase { .. } => 2.5,
            Trig::Event(..) => 1.5,
            Trig::Attacks | Trig::CombatDamage => 1.2,
            // Paid by removing +1/+1 counters it has to build up first (Ramos' five for
            // ten mana): an occasional burst that also shrinks it, not a repeatable
            // ability (as the deck builder rates it, `value::ability_value`). Measured: at
            // the usual activated rate Ramos' mana counted 8 points (two 4/4s' worth): game
            // logs showed it cast ahead of the commander and first in line for the table's
            // removal, and Ramos played 1.4 win-rate points worse than the same Dragon
            // without the mana ability (0.35 with this rate).
            Trig::Activated { extra, .. } if extra.remove_plus && extra.remove >= 3 => 0.35,
            Trig::Activated {
                sac_self: false,
                mana,
                ..
            } => (1.6 - 0.15 * *mana as f32).max(0.4),
            Trig::Activated { .. } => 0.7,
            Trig::Loyalty(_) => 0.8,
            Trig::Static => 1.5,
            Trig::PowerUp(_) => 0.6,
            _ => 0.0,
        };
        v += e
            * times
            * if matches!(a.cond, Cond::Always) {
                1.0
            } else {
                0.6
            };
    }
    v
}

/// What a permanent is worth to its controller right now (cached until the board changes).
pub fn perm_value(g: &Game, i: usize) -> f32 {
    let id = g.perms[i].id;
    if let Some(&(t, cid, v)) = g.vcache.borrow().get(i) {
        if t == g.tick && cid == id {
            return v;
        }
    }
    let v = perm_value_uncached(g, i);
    let mut c = g.vcache.borrow_mut();
    if c.len() <= i {
        c.resize(i + 1, (0, 0, 0.0));
    }
    c[i] = (g.tick, id, v);
    v
}

/// `perm_value` without filling the cache (for logs and statistics: they must not change
/// what the AI later reads from it).
pub fn perm_value_peek(g: &Game, i: usize) -> f32 {
    if let Some(&(t, cid, v)) = g.vcache.borrow().get(i) {
        if t == g.tick && cid == g.perms[i].id {
            return v;
        }
    }
    perm_value_uncached(g, i)
}

/// `threat` without filling caches.
pub fn threat_peek(g: &Game, o: usize) -> f32 {
    threat_parts_peek(g, o).0
}

/// (`threat`, `win_score`) without filling caches.
fn threat_parts_peek(g: &Game, o: usize) -> (f32, f32) {
    if !g.players[o].alive {
        return (0.0, f32::MIN);
    }
    if o < 8 {
        let (t, v, s) = g.tcache.borrow()[o];
        if t == g.tick {
            return (v, s);
        }
    }
    threat_parts_uncached(g, o, perm_value_peek)
}

/// `weight` without filling caches.
pub fn weight_peek(g: &Game, me: usize, o: usize) -> f32 {
    if o == me || !g.players[o].alive {
        return 0.0;
    }
    if me < 8 && o < 8 {
        let (t, v) = g.wcache.borrow()[me][o];
        if t == g.tick {
            return v;
        }
    }
    let max = g
        .opponents(me)
        .iter()
        .map(|&q| win_score_peek(g, q))
        .fold(f32::MIN, f32::max);
    weight_from(g, me, o, win_score_peek(g, o) - max)
}

/// `weight`, given how `o`'s win score compares with the likeliest opponent's.
fn weight_from(g: &Game, me: usize, o: usize, behind: f32) -> f32 {
    let low = if g.players[o].life <= 10 { 0.4 } else { 0.0 };
    // The odds `o` wins, against those of the likeliest winner among our opponents: early on
    // they're all about even, and one who merely developed first isn't singled out.
    1.0 + behind.exp() + (g.players[me].grudge[o] / 15.0).min(0.5) + low
}

fn perm_value_uncached(g: &Game, i: usize) -> f32 {
    let x = &g.perms[i];
    let abilities = g.abilities_of(x);
    let mut v = match x.tok {
        Tok::Treasure => return 0.8,
        Tok::Clue => return 1.0,
        Tok::Food | Tok::Map => return 0.5,
        Tok::Blood => return 0.4,
        Tok::Heartwood => return 1.5,
        Tok::Lotus => return 1.5,
        Tok::Equip(p, t) => return 0.5 + 0.8 * p as f32 + 0.4 * t as f32,
        Tok::Jace => return 1.0 + x.loyalty as f32 * 0.5,
        _ => 0.0,
    };
    if x.is(T_LAND) && !x.is_creature() {
        let lands = g
            .perms
            .iter()
            .filter(|y| y.ctrl == x.ctrl && y.is(T_LAND))
            .count();
        return if lands <= 4 { 1.8 } else { 1.0 };
    }
    if x.is_creature() {
        let p = x.p.max(0) as f32;
        v += p + 0.6 * x.t.max(0) as f32 + kw_value(x.eff_kw, p);
        if x.locked {
            v *= 0.25;
        }
    }
    if x.is(T_PLANESWALKER) {
        v += 2.0 + x.loyalty.max(0) as f32 * 0.6;
    }
    let engine = if x.powered {
        // A used power-up does nothing more.
        abilities
            .iter()
            .filter(|a| !matches!(a.trig, Trig::PowerUp(_)))
            .map(|a| engine_value(std::slice::from_ref(a)))
            .sum()
    } else {
        engine_value(abilities)
    };
    v += engine * if x.locked { 0.5 } else { 1.0 };
    // A Saga: the chapters still to come.
    if x.is(T_SAGA) {
        v += saga_value(g, x.ctrl as usize, abilities, x.other_counters.max(1));
    }
    if x.card.is_some_and(|r| g.card(r).mana_ability) && !x.is(T_LAND) {
        v += 1.5;
    }
    if x.commander > 0 {
        v += 2.5;
    }
    // Equipment and Auras are worth what they give.
    if x.is(T_EQUIPMENT) && x.attached.is_none() {
        v += 0.5;
    }
    v.max(0.2)
}

/// How threatening a player is to the rest of the table: its board's value, hand and life.
pub fn threat(g: &Game, o: usize) -> f32 {
    threat_parts(g, o).0
}

/// How likely player `o` is to win, as a score (log-odds, up to a constant): life, cards in
/// hand, creature power, lands and the board's value, weighted as fitted (conditional logit)
/// to who went on to win from the start of rounds 3-9 of simulated league games. It names
/// the eventual winner more often than the board value alone from round 5 on.
pub fn win_score(g: &Game, o: usize) -> f32 {
    threat_parts(g, o).1
}

/// `win_score` without filling caches.
pub fn win_score_peek(g: &Game, o: usize) -> f32 {
    threat_parts_peek(g, o).1
}

/// (`threat`, `win_score`), cached until the board changes.
fn threat_parts(g: &Game, o: usize) -> (f32, f32) {
    if !g.players[o].alive {
        return (0.0, f32::MIN);
    }
    if o < 8 {
        let (t, v, s) = g.tcache.borrow()[o];
        if t == g.tick {
            return (v, s);
        }
    }
    let parts = threat_parts_uncached(g, o, perm_value);
    if o < 8 {
        g.tcache.borrow_mut()[o] = (g.tick, parts.0, parts.1);
    }
    parts
}

fn threat_parts_uncached(g: &Game, o: usize, value: fn(&Game, usize) -> f32) -> (f32, f32) {
    let pl = &g.players[o];
    let (mut board, mut power, mut lands) = (0.0f32, 0i32, 0i32);
    for (i, x) in g.perms.iter().enumerate() {
        if x.ctrl as usize != o {
            continue;
        }
        if x.is(T_LAND) {
            lands += 1;
        } else {
            board += value(g, i);
        }
        if x.is_creature() {
            power += x.p.max(0) as i32;
        }
    }
    let threat = board + pl.hand.len() as f32 * 0.4 + pl.life as f32 * 0.05;
    let score = 0.0893 * pl.life as f32
        + 0.0779 * pl.hand.len() as f32
        + 0.0391 * power as f32
        + 0.1328 * lands as f32
        + 0.0363 * board;
    (threat, score)
}

/// How much `me` wants to hurt opponent `o`: the biggest threat, whoever attacked it, and
/// whoever is close to dead.
/// Mana value of a permanent for "odd or even" checks (tokens are 0; token copies keep the
/// copied card's).
pub fn perm_mv(g: &Game, x: &Perm) -> u8 {
    x.card.map(|r| g.card(r).mv).unwrap_or(0)
}

/// Thanos's power-up: "Choose odd or even. Destroy each other creature with mana value of the
/// chosen quality." The parity (true = odd) that's best for `p` - opponents' creatures lost,
/// weighted by how threatening their controller is, minus `p`'s own - and its net value.
pub fn best_parity(g: &Game, p: usize, src: Option<u32>) -> (bool, f32) {
    let net = |odd: bool| -> f32 {
        g.perms
            .iter()
            .enumerate()
            .filter(|(_, x)| x.is_creature() && Some(x.id) != src && !x.has(INDESTRUCTIBLE))
            .filter(|(_, x)| (perm_mv(g, x) % 2 == 1) == odd)
            .map(|(i, x)| {
                let o = x.ctrl as usize;
                if o == p {
                    -perm_value(g, i)
                } else {
                    perm_value(g, i) * weight(g, p, o) * 0.8
                }
            })
            .sum()
    };
    let (odd, even) = (net(true), net(false));
    if odd >= even {
        (true, odd)
    } else {
        (false, even)
    }
}

/// The nets `best_parity` weighs (odd, even), read without filling caches (for logs).
pub fn parity_values_peek(g: &Game, p: usize, src: Option<u32>) -> (f32, f32) {
    let net = |odd: bool| -> f32 {
        g.perms
            .iter()
            .enumerate()
            .filter(|(_, x)| x.is_creature() && Some(x.id) != src && !x.has(INDESTRUCTIBLE))
            .filter(|(_, x)| (perm_mv(g, x) % 2 == 1) == odd)
            .map(|(i, x)| {
                let o = x.ctrl as usize;
                if o == p {
                    -perm_value_peek(g, i)
                } else {
                    perm_value_peek(g, i) * weight_peek(g, p, o) * 0.8
                }
            })
            .sum()
    };
    (net(true), net(false))
}

pub fn weight(g: &Game, me: usize, o: usize) -> f32 {
    if o == me || !g.players[o].alive {
        return 0.0;
    }
    if me < 8 && o < 8 {
        let (t, v) = g.wcache.borrow()[me][o];
        if t == g.tick {
            return v;
        }
    }
    let v = weight_uncached(g, me, o);
    if me < 8 && o < 8 {
        g.wcache.borrow_mut()[me][o] = (g.tick, v);
    }
    v
}

fn weight_uncached(g: &Game, me: usize, o: usize) -> f32 {
    let max = g
        .opponents(me)
        .iter()
        .map(|&q| win_score(g, q))
        .fold(f32::MIN, f32::max);
    let w = weight_from(g, me, o, win_score(g, o) - max);
    // `lab focus`: the table ganging up on our seat.
    match g.ext.hated {
        Some((seat, k)) if seat == o && seat != me => w * k,
        _ => w,
    }
}

fn archenemy(g: &Game, me: usize) -> Option<usize> {
    g.opponents(me)
        .into_iter()
        .max_by(|&a, &b| weight(g, me, a).partial_cmp(&weight(g, me, b)).unwrap())
}

pub fn choose_opponent(g: &Game, me: usize, n: i32) -> Option<usize> {
    let opps = g.opponents(me);
    if n > 0 {
        if let Some(&o) = opps
            .iter()
            .filter(|&&o| g.players[o].life <= n)
            .min_by_key(|&&o| g.players[o].life)
        {
            return Some(o);
        }
    }
    archenemy(g, me)
}

// ------------------------------------------------------------ targets

pub fn choose_victims(g: &Game, me: usize, cands: &[usize], n: usize) -> Vec<usize> {
    let mut v: Vec<(f32, usize)> = cands
        .iter()
        .map(|&i| {
            let owner = g.perms[i].ctrl as usize;
            let focus = if g.focus == Some(g.perms[i].id) {
                100.0
            } else {
                0.0
            };
            let w = if owner == me {
                -1.0
            } else {
                // As removal is valued: less on a commander or a player about to die.
                weight(g, me, owner) * removal_discount(g, me, i)
            };
            (focus + perm_value(g, i) * w, i)
        })
        .collect();
    v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    v.into_iter()
        .filter(|(s, _)| *s > 0.0)
        .take(n)
        .map(|(_, i)| i)
        .collect()
}

pub fn choose_friends(g: &Game, me: usize, cands: &[usize], n: usize) -> Vec<usize> {
    let mut v: Vec<(f32, usize)> = cands
        .iter()
        .map(|&i| {
            let x = &g.perms[i];
            let focus = if g.focus == Some(x.id) { 100.0 } else { 0.0 };
            let mine = if x.ctrl as usize == me { 0.0 } else { -50.0 };
            let ready = if x.is_creature() && !x.tapped {
                0.5
            } else {
                0.0
            };
            (focus + mine + perm_value(g, i) + ready, i)
        })
        .collect();
    v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    v.into_iter().take(n).map(|(_, i)| i).collect()
}

pub enum Hit {
    Player(usize),
    Perm(u32),
    None,
}

/// Where to point N damage: a lethal burn to a player, the best creature or planeswalker it
/// kills, or the biggest threat's face.
pub fn damage_target(g: &Game, ctx: &Ctx, to: &crate::abilities::Target, n: i32) -> Hit {
    let me = ctx.ctrl;
    let face_ok = to.kind == Kind::AnyTarget || to.kind == Kind::Player;
    if face_ok {
        if let Some(o) = g
            .opponents(me)
            .into_iter()
            .find(|&o| g.players[o].life <= n)
        {
            return Hit::Player(o);
        }
    }
    let t1 = crate::abilities::Target { count: 1, ..*to };
    let cands = g.candidates(ctx, &t1, true);
    let best = cands
        .iter()
        .filter(|&&i| {
            let x = &g.perms[i];
            x.ctrl as usize != me
                && ((x.is(T_PLANESWALKER) && !x.is_creature())
                    || (!x.has(INDESTRUCTIBLE) && x.t - x.damage <= n as i16))
                // (Already dealt lethal damage, by an earlier part of the same spell.)
                && !(x.is_creature() && x.damage >= x.t)
        })
        .map(|&i| {
            (
                perm_value(g, i) * weight(g, me, g.perms[i].ctrl as usize),
                i,
            )
        })
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    match best {
        Some((v, i)) if v >= n as f32 * 0.6 || !face_ok => Hit::Perm(g.perms[i].id),
        _ if face_ok => archenemy(g, me).map(Hit::Player).unwrap_or(Hit::None),
        Some((_, i)) => Hit::Perm(g.perms[i].id),
        None => Hit::None,
    }
}

/// "Target creature can't block this turn" during `p`'s combat, before blocks: the
/// defender's creature whose block costs `p` the most (`blocker_to_stop`, as the attack
/// planner predicts it) and what keeping it out of the fight is worth (None outside that
/// window, or when no predicted block matters).
pub fn net_target(g: &Game, p: usize) -> Option<(u32, f32)> {
    if !g.in_combat || g.active != p || g.ext.attack_def.is_empty() {
        return None;
    }
    // Blocks are declared once: a creature already blocking means it's too late.
    if g.perms.iter().any(|x| x.blocking) {
        return None;
    }
    let id = blocker_to_stop(g, p)?;
    let b = g.idx(id)?;
    let d = g.perms[b].ctrl as usize;
    // What that block costs our attack (as `group_value` counts damage and trades).
    let attacks: Vec<Attack> = g
        .ext
        .attack_def
        .iter()
        .filter(|x| x.1 as usize == d && g.idx(x.0).is_some())
        .map(|x| Attack {
            attacker: x.0,
            def: Def::Player(d),
            blockers: Vec::new(),
            blocked: false,
        })
        .collect();
    let blocks = declare_blocks(g, d, &attacks);
    let w = weight(g, p, d);
    let outcome = |a: usize, bl: &[usize]| -> f32 {
        let at = &g.perms[a];
        if bl.is_empty() || at.has(MENACE) && bl.len() == 1 {
            return (dmg_of(g, at) + g.players[d].attacked_power as i32).max(0) as f32 * 0.6 * w;
        }
        let c = clash(g, a, bl);
        let killed: f32 = c.dead.iter().map(|&x| perm_value(g, x)).sum();
        killed + c.through as f32 * 0.6 * w - if c.att_dies { perm_value(g, a) } else { 0.0 }
    };
    let (_, att) = blocks.iter().find(|x| x.0 == id)?;
    let a = g.idx(*att)?;
    let bl: Vec<usize> = blocks
        .iter()
        .filter(|x| x.1 == *att)
        .filter_map(|x| g.idx(x.0))
        .collect();
    let rest: Vec<usize> = bl.iter().copied().filter(|&x| x != b).collect();
    Some((id, (outcome(a, &rest) - outcome(a, &bl)).max(0.0)))
}

/// The biggest creature `me` controls other than `not`.
pub fn best_fighter_except(g: &Game, me: usize, not: Option<u32>) -> Option<usize> {
    (0..g.perms.len())
        .filter(|&i| {
            g.perms[i].ctrl as usize == me && g.perms[i].is_creature() && Some(g.perms[i].id) != not
        })
        .max_by_key(|&i| g.perms[i].p * 2 + g.perms[i].t)
}

pub fn best_fighter(g: &Game, me: usize) -> Option<usize> {
    (0..g.perms.len())
        .filter(|&i| g.perms[i].ctrl as usize == me && g.perms[i].is_creature())
        .max_by_key(|&i| g.perms[i].p * 2 + g.perms[i].t)
}

pub fn sacrifice_choice(g: &Game, o: usize, types: u16) -> Option<usize> {
    sacrifice_choice_r(g, o, types, &Restr::default())
}

/// The permanent player `o` sacrifices when it must also meet `r` ("a creature with the
/// greatest mana value among creatures they control").
pub fn sacrifice_choice_r(g: &Game, o: usize, types: u16, r: &Restr) -> Option<usize> {
    let d = g.dyn_values(r, None, o, None);
    let mut cands: Vec<usize> = (0..g.perms.len())
        .filter(|&i| {
            g.perms[i].ctrl as usize == o
                && (g.perms[i].types & types != 0
                    || (types & T_CREATURE != 0 && g.perms[i].is_creature()))
                && g.perm_ok(r, &g.perms[i], &d)
        })
        .collect();
    g.keep_greatest(r, &mut cands);
    cands
        .into_iter()
        .min_by(|&a, &b| perm_value(g, a).partial_cmp(&perm_value(g, b)).unwrap())
}

/// The opponent a "target opponent sacrifices a creature" should hit: the one whose
/// creature it takes is worth most to us (their least valuable one, weighted by how much we
/// want to hurt them); none when nobody has a creature.
pub fn edict_victim(g: &Game, me: usize, r: &Restr) -> Option<usize> {
    g.opponents(me)
        .into_iter()
        .filter_map(|o| {
            sacrifice_choice_r(g, o, T_CREATURE, r)
                .map(|i| (perm_value(g, i) * weight(g, me, o), o))
        })
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
        .map(|x| x.1)
        .or_else(|| choose_opponent(g, me, 0))
}

pub fn fodder(g: &Game, p: usize, types: u16, src_id: u32) -> Option<usize> {
    (0..g.perms.len())
        .filter(|&i| {
            let x = &g.perms[i];
            x.ctrl as usize == p
                && x.id != src_id
                && x.commander == 0
                && (x.types & types != 0 || (types & T_CREATURE != 0 && x.is_creature()))
        })
        .filter(|&i| perm_value(g, i) < 3.0 || g.perms[i].is_token())
        .min_by(|&a, &b| perm_value(g, a).partial_cmp(&perm_value(g, b)).unwrap())
}

/// What `p` sacrifices to pay a cost that must be paid ("as an additional cost to cast this
/// spell, sacrifice a creature"): spare fodder, else the least valuable one (never the
/// commander).
pub fn sac_for_cost(g: &Game, p: usize, types: u16) -> Option<usize> {
    fodder(g, p, types, u32::MAX).or_else(|| {
        (0..g.perms.len())
            .filter(|&i| {
                let x = &g.perms[i];
                x.ctrl as usize == p
                    && x.commander == 0
                    && (x.types & types != 0 || (types & T_CREATURE != 0 && x.is_creature()))
            })
            .min_by(|&a, &b| perm_value(g, a).partial_cmp(&perm_value(g, b)).unwrap())
    })
}

pub fn untap_choice(g: &Game, me: usize, cands: &[usize]) -> Option<usize> {
    let tapped: Vec<usize> = cands
        .iter()
        .copied()
        .filter(|&i| g.perms[i].tapped && g.perms[i].ctrl as usize == me)
        .collect();
    tapped
        .iter()
        .copied()
        .find(|&i| g.perms[i].is(T_LAND))
        .or_else(|| {
            tapped
                .iter()
                .copied()
                .max_by(|&a, &b| perm_value(g, a).partial_cmp(&perm_value(g, b)).unwrap())
        })
}

pub fn best_to_copy(g: &Game, me: usize, any: bool) -> Option<usize> {
    (0..g.perms.len())
        .filter(|&i| {
            g.perms[i].is_creature()
                && g.perms[i].card.is_some()
                && (any || g.perms[i].ctrl as usize == me)
        })
        .max_by(|&a, &b| perm_value(g, a).partial_cmp(&perm_value(g, b)).unwrap())
}

fn arrival_value(c: &SimCard) -> f32 {
    c.abilities
        .iter()
        .filter(|a| a.trig == Trig::Enters)
        .flat_map(|a| a.effects.iter())
        .map(effect_value)
        .sum()
}

pub fn blink_choice(g: &Game, me: usize, not: Option<u32>, types: u16) -> Option<usize> {
    blink_pick(g, me, not, types, false)
}

/// `blink_choice` for a blink that returns the card at once: a commander may go too (its
/// owner leaves it in exile, and it comes back as a commander).
pub fn blink_choice_now(g: &Game, me: usize, not: Option<u32>, types: u16) -> Option<usize> {
    blink_pick(g, me, not, types, true)
}

/// What blinking permanent `i` is worth: its arrival again, and - for a once-only power-up
/// already used (Thanos) - that power-up back, less the counters it loses.
fn blink_gain(g: &Game, i: usize) -> f32 {
    let x = &g.perms[i];
    let c = g.card(x.card.unwrap());
    let mut v = arrival_value(c);
    if x.powered {
        // Paid again at full cost on a later turn: about half of what it would do now.
        v += c
            .abilities
            .iter()
            .filter(|a| matches!(a.trig, Trig::PowerUp(_)))
            .map(|a| ability_now(g, x.ctrl as usize, a, Some(x.id)).max(0.0) * 0.5)
            .sum::<f32>();
    }
    v - 0.8 * x.counters.max(0) as f32 - 0.3 * x.other_counters.max(0) as f32
}

fn blink_pick(g: &Game, me: usize, not: Option<u32>, types: u16, cmdr: bool) -> Option<usize> {
    (0..g.perms.len())
        .filter(|&i| {
            let x = &g.perms[i];
            x.ctrl as usize == me
                && x.tok == Tok::None
                && (x.commander == 0 || cmdr && x.owner as usize == me)
                && x.card.is_some()
                && Some(x.id) != not
                && (x.is(types) || types & T_CREATURE != 0 && x.is_creature())
        })
        .map(|i| (blink_gain(g, i), i))
        .filter(|(v, _)| *v > 0.0)
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
        .map(|(_, i)| i)
}

pub fn best_preparable(g: &Game, me: usize) -> Option<usize> {
    (0..g.perms.len()).find(|&i| {
        let x = &g.perms[i];
        x.ctrl as usize == me
            && !x.prepared
            && x.card.is_some_and(|r| {
                g.card(r)
                    .back
                    .as_ref()
                    .is_some_and(|b| b.kind == BackKind::Prepare)
            })
    })
}

pub fn unattached_equipment(g: &Game, me: usize) -> Option<usize> {
    (0..g.perms.len()).find(|&i| {
        g.perms[i].ctrl as usize == me
            && g.perms[i].is(T_EQUIPMENT)
            && g.perms[i].attached.is_none()
    })
}

/// The creature an Equipment should go on: evasive, big, ready to attack, not already
/// carrying gear.
pub fn equip_target(g: &Game, me: usize, equip_id: u32) -> Option<usize> {
    (0..g.perms.len())
        .filter(|&i| {
            let x = &g.perms[i];
            x.ctrl as usize == me && x.is_creature() && !x.has(DEFENDER) && !x.locked
        })
        .map(|i| {
            let x = &g.perms[i];
            let geared = g
                .perms
                .iter()
                .filter(|e| e.is(T_EQUIPMENT) && e.attached == Some(x.id) && e.id != equip_id)
                .count() as f32;
            let evasive = if x.has(FLYING | UNBLOCKABLE | MENACE | TRAMPLE) {
                2.0
            } else {
                0.0
            };
            let lifelink_bonus = if x.has(DOUBLE_STRIKE) { 2.0 } else { 0.0 };
            (
                x.p as f32 + evasive + lifelink_bonus
                    - geared * 1.5
                    - if x.sick { 0.5 } else { 0.0 },
                i,
            )
        })
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
        .map(|(_, i)| i)
}

pub fn aura_target(g: &Game, p: usize, card: CardRef) -> Option<usize> {
    let c = g.card(card);
    // "Enchanted permanent is a Construct creature with base power and toughness 5/5"
    // (Puppet Crafting): one of our artifacts or enchantments that isn't a creature yet.
    if c.attached_sets.is_some_and(|s| s.creature && !s.blank) {
        return (0..g.perms.len())
            .filter(|&i| {
                let x = &g.perms[i];
                x.ctrl as usize == p
                    && x.is(T_ARTIFACT | T_ENCHANTMENT)
                    && !x.is(T_AURA)
                    && !x.is_creature()
            })
            .min_by(|&a, &b| perm_value(g, a).partial_cmp(&perm_value(g, b)).unwrap());
    }
    let creatures: Vec<usize> = (0..g.perms.len())
        .filter(|&i| {
            let x = &g.perms[i];
            x.is_creature()
                && !(x.ctrl as usize != p
                    && (x.has(HEXPROOF | SHROUD) || x.hexproof_from & c.colors != 0))
        })
        .collect();
    if c.harmful_aura {
        choose_victims(
            g,
            p,
            &creatures
                .into_iter()
                .filter(|&i| g.perms[i].ctrl as usize != p)
                .collect::<Vec<_>>(),
            1,
        )
        .first()
        .copied()
    } else {
        choose_friends(
            g,
            p,
            &creatures
                .into_iter()
                .filter(|&i| g.perms[i].ctrl as usize == p)
                .collect::<Vec<_>>(),
            1,
        )
        .first()
        .copied()
    }
}

// ------------------------------------------------------------ hand, library, graveyard

fn lands_total(g: &Game, p: usize) -> usize {
    let pl = &g.players[p];
    g.perms
        .iter()
        .filter(|x| x.ctrl as usize == p && x.is(T_LAND))
        .count()
        + pl.hand
            .iter()
            .filter(|&&c| pl.cards[c as usize].is(T_LAND))
            .count()
}

fn lands_in_play(g: &Game, p: usize) -> usize {
    g.perms
        .iter()
        .filter(|x| x.ctrl as usize == p && x.is(T_LAND))
        .count()
}

/// How much a card in hand (or library) is worth to its owner now.
pub fn card_worth(g: &Game, p: usize, c: &SimCard) -> f32 {
    if c.is(T_LAND) {
        // A land is worth more while the hand holds spells the lands can't cast yet.
        let inplay = lands_in_play(g, p);
        if inplay >= 7 {
            return 0.3;
        }
        let pl = &g.players[p];
        let waiting = pl
            .hand
            .iter()
            .map(|&k| &pl.cards[k as usize])
            .filter(|x| !x.is(T_LAND) && x.mv as usize > inplay)
            .count() as f32;
        return if lands_total(g, p) >= 7 && waiting == 0.0 {
            0.3
        } else {
            (2.5 + 0.8 * waiting).min(5.0)
        };
    }
    // A spell that draws two or more is a card that makes cards.
    if c.abilities.iter().any(|a| {
        a.trig == Trig::Spell
            && a.effects
                .iter()
                .any(|e| matches!(e, Effect::Draw { amount, you: true } if amount.n >= 2))
    }) {
        return c.value.max(5.0) - (c.mv as f32 - lands_in_play(g, p) as f32 - 1.0).max(0.0);
    }
    // Colours its lands can't make, and mana it's far from, make a card worth less.
    let (mut lands, mut have) = (0.0f32, 0u8);
    for x in g
        .perms
        .iter()
        .filter(|x| x.ctrl as usize == p && x.is(T_LAND))
    {
        lands += 1.0;
        have |= x.card.map_or(0, |r| g.card(r).produces);
    }
    let offcolour = lands >= 3.0 && pip_mask(&c.cost) & !have != 0;
    c.value * if offcolour { 0.6 } else { 1.0 } - (c.mv as f32 - lands - 1.0).max(0.0)
}

pub fn discard_choice(g: &Game, p: usize) -> Option<usize> {
    let pl = &g.players[p];
    (0..pl.hand.len()).min_by(|&a, &b| {
        let va = card_worth(g, p, &pl.cards[pl.hand[a] as usize]);
        let vb = card_worth(g, p, &pl.cards[pl.hand[b] as usize]);
        va.partial_cmp(&vb).unwrap()
    })
}

pub fn best_card_in_hand(g: &Game, o: usize) -> Option<usize> {
    let pl = &g.players[o];
    (0..pl.hand.len())
        .filter(|&h| !pl.cards[pl.hand[h] as usize].is(T_LAND))
        .max_by(|&a, &b| {
            pl.cards[pl.hand[a] as usize]
                .value
                .partial_cmp(&pl.cards[pl.hand[b] as usize].value)
                .unwrap()
        })
}

/// Colours the player's hand needs that its mana can't make yet.
fn missing_colors(g: &Game, p: usize) -> u8 {
    let pl = &g.players[p];
    let have = g.sources(p).iter().fold(0u8, |m, s| m | s.1)
        | g.perms
            .iter()
            .filter(|x| x.ctrl as usize == p && x.is(T_LAND))
            .filter_map(|x| x.card)
            .fold(0u8, |m, r| m | g.card(r).produces);
    let mut need = 0u8;
    for &c in pl.hand.iter().chain(pl.command.iter().map(|(c, _)| c)) {
        for (i, &n) in pl.cards[c as usize].cost.pips.iter().enumerate() {
            if n > 0 {
                need |= 1 << i;
            }
        }
    }
    // Power-ups still to use ({C}{W}{U}{B}{R}{G}: Thanos), on the battlefield or on the
    // commander waiting in the command zone.
    let pu = |c: &SimCard| {
        c.abilities.iter().fold(0u8, |m, a| match &a.trig {
            Trig::PowerUp(full) => {
                m | pip_mask(full) | if full.colorless > 0 { COLORLESS } else { 0 }
            }
            _ => m,
        })
    };
    for x in g
        .perms
        .iter()
        .filter(|x| x.ctrl as usize == p && !x.powered)
    {
        if let Some(r) = x.card {
            need |= pu(g.card(r));
        }
    }
    for &(c, _) in &pl.command {
        need |= pu(&pl.cards[c as usize]);
    }
    need & !have
}

pub fn land_to_fetch(g: &Game, p: usize, basic_only: bool) -> Option<usize> {
    let pl = &g.players[p];
    let missing = missing_colors(g, p);
    (0..pl.library.len())
        .filter(|&k| {
            let c = &pl.cards[pl.library[k] as usize];
            c.is(T_LAND) && (!basic_only || c.is(T_BASIC))
        })
        .max_by_key(|&k| {
            let c = &pl.cards[pl.library[k] as usize];
            (
                (c.produces & missing).count_ones() * 4 + c.produces.count_ones(),
                !c.enters_tapped,
            )
        })
}

/// Scry / surveil: keep what the next turns need, send the rest away.
pub fn scry(g: &mut Game, p: usize, k: usize, surveil: bool) {
    let lands_now = lands_total(g, p);
    let n = g.players[p].library.len();
    let k = k.min(n);
    let mut keep = Vec::new();
    let mut away = Vec::new();
    for _ in 0..k {
        let c = g.players[p].library.pop().unwrap();
        let card = &g.players[p].cards[c as usize];
        let good = if card.is(T_LAND) {
            lands_now
                + keep
                    .iter()
                    .filter(|&&x: &&u16| g.players[p].cards[x as usize].is(T_LAND))
                    .count()
                < 6
        } else {
            card.mv as usize <= lands_now + 2
        };
        if good { keep.push(c) } else { away.push(c) }
    }
    for c in keep.into_iter().rev() {
        g.players[p].library.push(c);
    }
    for c in away {
        if surveil {
            g.players[p].graveyard.push(c);
        } else {
            g.players[p].library.insert(0, c);
        }
    }
    g.players[p].scried_turn = true;
    g.emit(Ev::Scry, p, None);
}

/// Look at the top N, take the best (of the kind `r` allows: "you may reveal a Hero card
/// from among them"), the rest go to the bottom.
pub fn dig(g: &mut Game, p: usize, n: usize, lands_ok: bool, r: &Restr) {
    let n = n.min(g.players[p].library.len());
    if n == 0 {
        return;
    }
    let len = g.players[p].library.len();
    let top: Vec<u16> = g.players[p].library.drain(len - n..).collect();
    let d = g.dyn_values(r, None, p, None);
    let best = (0..top.len())
        .filter(|&k| Game::card_ok(r, &g.players[p].cards[top[k] as usize], &d))
        .max_by(|&a, &b| {
            let ca = &g.players[p].cards[top[a] as usize];
            let cb = &g.players[p].cards[top[b] as usize];
            let va = if ca.is(T_LAND) && !lands_ok {
                -1.0
            } else {
                card_worth(g, p, ca)
            };
            let vb = if cb.is(T_LAND) && !lands_ok {
                -1.0
            } else {
                card_worth(g, p, cb)
            };
            va.partial_cmp(&vb).unwrap()
        });
    for (i, c) in top.into_iter().enumerate() {
        if Some(i) == best {
            g.players[p].hand.push(c);
        } else {
            g.players[p].library.insert(0, c);
        }
    }
}

pub fn tutor_choice(g: &Game, p: usize, text: &str, r: &Restr) -> Option<usize> {
    let pl = &g.players[p];
    // "Search your library for a Plan card": a card of that subtype only.
    let sub = text
        .split("search your library for a")
        .nth(1)
        .map(|s| s.trim_start_matches('n').trim_start())
        .and_then(|s| s.split(" card").next())
        .filter(|w| !w.is_empty() && !w.contains(' ') && !w.starts_with("card"))
        .filter(|w| {
            ![
                "creature",
                "land",
                "artifact",
                "equipment",
                "enchantment",
                "instant",
                "sorcery",
                "planeswalker",
                "basic",
                "legendary",
                "permanent",
                "nonland",
            ]
            .contains(w)
        });
    let want = |c: &SimCard| {
        if let Some(s) = sub {
            c.has_subtype(s)
        } else if text.contains("creature card") {
            c.is(T_CREATURE)
        } else if text.contains("land card") || text.contains("basic land") {
            c.is(T_LAND)
        } else if text.contains("artifact card") {
            c.is(T_ARTIFACT)
        } else if text.contains("equipment card") {
            c.is(T_EQUIPMENT)
        } else if text.contains("enchantment card") {
            c.is(T_ENCHANTMENT)
        } else if text.contains("instant or sorcery") {
            c.is(T_INSTANT | T_SORCERY)
        } else {
            true
        }
    };
    if (text.contains("land card") || text.contains("basic land")) && !text.contains("creature") {
        return land_to_fetch(g, p, text.contains("basic"));
    }
    let d = g.dyn_values(r, None, p, None);
    (0..pl.library.len())
        .filter(|&k| want(&pl.cards[pl.library[k] as usize]))
        .filter(|&k| Game::card_ok(r, &pl.cards[pl.library[k] as usize], &d))
        .max_by(|&a, &b| {
            card_worth(g, p, &pl.cards[pl.library[a] as usize])
                .partial_cmp(&card_worth(g, p, &pl.cards[pl.library[b] as usize]))
                .unwrap()
        })
}

pub fn reanimate_choice(
    g: &Game,
    me: usize,
    any: bool,
    r: &Restr,
    d: &Dyn,
) -> Option<(usize, usize)> {
    let mut best: Option<(f32, usize, usize)> = None;
    // "Return target creature card" unless the text names other cards ("permanent card").
    let creature = r.any_types == 0 && r.not_types & T_LAND == 0;
    for o in 0..g.players.len() {
        if !any && o != me {
            continue;
        }
        let pl = &g.players[o];
        for (k, &c) in pl.graveyard.iter().enumerate() {
            let card = &pl.cards[c as usize];
            let fits = if creature {
                card.is(T_CREATURE)
            } else {
                card.is_permanent()
            };
            if fits && Game::card_ok(r, card, d) && best.is_none_or(|b| card.value > b.0) {
                best = Some((card.value, o, k));
            }
        }
    }
    best.map(|(_, o, k)| (o, k))
}

pub fn regrowth_choice(g: &Game, me: usize, creature: bool, r: &Restr, d: &Dyn) -> Option<usize> {
    let pl = &g.players[me];
    (0..pl.graveyard.len())
        .filter(|&k| !creature || pl.cards[pl.graveyard[k] as usize].is(T_CREATURE))
        .filter(|&k| Game::card_ok(r, &pl.cards[pl.graveyard[k] as usize], d))
        .max_by(|&a, &b| {
            pl.cards[pl.graveyard[a] as usize]
                .value
                .partial_cmp(&pl.cards[pl.graveyard[b] as usize].value)
                .unwrap()
        })
}

pub fn recast_choice(g: &Game, me: usize) -> Option<usize> {
    let pl = &g.players[me];
    (0..pl.graveyard.len())
        .filter(|&k| pl.cards[pl.graveyard[k] as usize].is(T_INSTANT | T_SORCERY))
        .max_by(|&a, &b| {
            pl.cards[pl.graveyard[a] as usize]
                .value
                .partial_cmp(&pl.cards[pl.graveyard[b] as usize].value)
                .unwrap()
        })
}

pub fn free_spell_choice(g: &Game, me: usize, limit: u8) -> Option<usize> {
    let pl = &g.players[me];
    (0..pl.hand.len())
        .filter(|&h| {
            let c = &pl.cards[pl.hand[h] as usize];
            !c.is(T_LAND) && c.mv <= limit && !is_reactive_only(c)
        })
        .max_by(|&a, &b| {
            pl.cards[pl.hand[a] as usize]
                .value
                .partial_cmp(&pl.cards[pl.hand[b] as usize].value)
                .unwrap()
        })
}

// ------------------------------------------------------------ mulligan

/// A keepable hand: 2-5 lands and an early play its lands can cast (or 3+ lands). The
/// first mulligan is free, so the first hand also needs an untapped land and, with only two
/// lands, a castable early play.
fn keepable(g: &Game, p: usize, attempt: u8) -> bool {
    let pl = &g.players[p];
    let hand: Vec<&SimCard> = pl.hand.iter().map(|&c| &pl.cards[c as usize]).collect();
    let lands: Vec<&&SimCard> = hand.iter().filter(|c| c.is(T_LAND)).collect();
    let n = lands.len();
    let colours = lands.iter().fold(0u8, |m, c| m | c.produces);
    let untapped = lands.iter().filter(|c| !c.enters_tapped).count();
    let castable = |c: &SimCard| pip_mask(&c.cost) & !colours == 0;
    let early = hand
        .iter()
        .filter(|c| !c.is(T_LAND) && c.mv as usize <= n.max(2) + 1 && castable(c))
        .count();
    if !(2..=5).contains(&n) || early == 0 && n < 3 {
        return false;
    }
    attempt > 0 || untapped >= 1 && (early >= 1 || n >= 4)
}

/// London mulligan (CR 103.5), with the first mulligan free as in multiplayer Commander.
pub fn mulligan(g: &mut Game, p: usize) {
    for attempt in 0..3u8 {
        if attempt > 0 {
            let hand = std::mem::take(&mut g.players[p].hand);
            g.players[p].library.extend(hand);
            let mut lib = std::mem::take(&mut g.players[p].library);
            g.rng.shuffle(&mut lib);
            g.players[p].library = lib;
        }
        for _ in 0..7 {
            if let Some(c) = g.players[p].library.pop() {
                g.players[p].hand.push(c);
            }
        }
        g.players[p].mulligans = attempt;
        let keep = keepable(g, p, attempt);
        if g.log.is_some() {
            let pl = &g.players[p];
            let lands = pl
                .hand
                .iter()
                .filter(|&&c| pl.cards[c as usize].is(T_LAND))
                .count();
            let early = pl
                .hand
                .iter()
                .filter(|&&c| {
                    let card = &pl.cards[c as usize];
                    !card.is(T_LAND) && card.mv as usize <= lands.max(2) + 1
                })
                .count();
            let names: Vec<String> = pl
                .hand
                .iter()
                .map(|&c| pl.cards[c as usize].name.clone())
                .collect();
            let verdict = if keep {
                "keepable"
            } else if attempt == 2 {
                "not keepable, kept anyway (third hand)"
            } else {
                "not keepable (needs 2-5 lands and an early play they can cast, or 3+ lands; \
                 the free first hand also an untapped land): mulligan"
            };
            g.note(|| {
                format!(
                    "P{} opening hand #{}: {} lands, {} early plays -> {} | {}",
                    p,
                    attempt + 1,
                    lands,
                    early,
                    verdict,
                    names.join("; ")
                )
            });
        }
        if keep || attempt == 2 {
            // Bottom one card per mulligan after the free one.
            let bottom = attempt.saturating_sub(1);
            for _ in 0..bottom {
                if let Some(h) = discard_choice(g, p) {
                    let c = g.players[p].hand.remove(h);
                    if g.log.is_some() {
                        let name = g.players[p].cards[c as usize].name.clone();
                        g.note(|| format!("P{} puts {} on the bottom", p, name));
                    }
                    g.players[p].library.insert(0, c);
                }
            }
            let kept = g.players[p].hand.len() as u8;
            if let Some(s) = g.stats.as_mut() {
                s.players[p].mulligans = attempt;
                s.players[p].kept = kept;
            }
            g.note(|| format!("P{} keeps {} cards after {} mulligan(s)", p, kept, attempt));
            return;
        }
    }
}

// ------------------------------------------------------------ spell analysis

fn spell_abilities(c: &SimCard, back: bool) -> &[SimAbility] {
    if back {
        c.back
            .as_ref()
            .map(|b| b.abilities.as_slice())
            .unwrap_or(&[])
    } else {
        &c.abilities
    }
}

fn effects_of(c: &SimCard, back: bool) -> impl Iterator<Item = &Effect> {
    spell_abilities(c, back)
        .iter()
        .filter(|a| a.trig == Trig::Spell)
        .flat_map(|a| a.effects.iter())
}

fn is_counter(c: &SimCard) -> bool {
    c.is(T_INSTANT) && effects_of(c, false).any(|e| *e == Effect::CounterSpell)
}

fn is_fog(c: &SimCard) -> bool {
    // (Time Stop ends the turn: no combat damage either.)
    c.is(T_INSTANT)
        && effects_of(c, false)
            .any(|e| *e == Effect::Fog || matches!(e, Effect::Rule(l) if l == "ends the turn"))
}

/// Can `p` cast card `c` at instant speed right now: flash, or a rule that gives it (High
/// Fae Trickster, Captain Mar-Vell after an opponent's spell, Radagast's first creature).
pub fn has_flash(g: &Game, p: usize, c: &SimCard) -> bool {
    if c.kw & FLASH != 0 {
        return true;
    }
    let r = g.rules[p % 8];
    r & R_FLASH_SPELLS != 0
        || r & R_FLASH_AFTER_OPPONENT != 0 && g.cond_holds(&Cond::OpponentCastSpell, p, None)
        || r & R_FIRST_CREATURE_FLASH != 0
            && c.is(T_CREATURE)
            && g.players[p].creature_spells_turn == 0
}

fn is_protection(c: &SimCard) -> bool {
    c.is(T_INSTANT)
        && effects_of(c, false).any(|e| match e {
            Effect::Grant { keywords, to, .. } => {
                !to.mass
                    && to.whose != Whose::Theirs
                    && keywords.iter().any(|k| {
                        k == "hexproof"
                            || k == "indestructible"
                            || k.starts_with("protection")
                            || k == "shroud"
                    })
            }
            _ => false,
        })
}

fn is_trick(c: &SimCard) -> bool {
    c.is(T_INSTANT)
        && effects_of(c, false).any(|e| match e {
            Effect::Pump {
                to,
                until_eot: true,
                power,
                ..
            } => !to.mass && *power > 0,
            Effect::Grant {
                to,
                until_eot: true,
                keywords,
            } => {
                !to.mass
                    && keywords.iter().any(|k| {
                        k == "first strike"
                            || k == "deathtouch"
                            || k == "indestructible"
                            || k == "double strike"
                    })
            }
            _ => false,
        })
}

fn is_instant_removal(c: &SimCard) -> bool {
    c.is(T_INSTANT)
        && effects_of(c, false).any(|e| match e {
            Effect::Destroy(t) | Effect::Exile { target: t, .. } | Effect::Bounce(t) => {
                t.hits_their_creatures() && !t.mass
            }
            Effect::Damage { to, .. } | Effect::Shrink { to, .. } => {
                to.hits_their_creatures() && !to.mass
            }
            Effect::Fight(_) => true,
            _ => false,
        })
}

/// A spell whose point is to remove one creature or permanent of an opponent.
fn is_spot_removal(c: &SimCard) -> bool {
    c.is(T_INSTANT | T_SORCERY)
        && effects_of(c, false).any(|e| match e {
            Effect::Destroy(t) | Effect::Exile { target: t, .. } => {
                t.hits_their_creatures() && !t.mass
            }
            Effect::Damage { to, .. } => to.hits_their_creatures() && !to.mass,
            _ => false,
        })
        && !copies_x_times(c)
}

/// Cards worth casting only in response (counters, Fogs, protection, combat tricks).
fn is_reactive_only(c: &SimCard) -> bool {
    c.is(T_INSTANT)
        && effects_of(c, false).all(|e| {
            matches!(e, Effect::CounterSpell | Effect::Fog)
                || matches!(e, Effect::Pump { until_eot: true, to, .. } | Effect::Grant { until_eot: true, to, .. } if !to.mass)
        })
}

#[cfg(test)]
thread_local! {
    /// Tests: seats (a bit each) playing without the removal target floor.
    pub static FLOOR_OFF: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn floor_on(p: usize) -> bool {
    FLOOR_OFF.with(|f| f.get() & (1 << p) == 0)
}

#[cfg(not(test))]
fn floor_on(_: usize) -> bool {
    true
}

/// When the target floor gives way: a card that would otherwise rot - a full hand (the
/// next draw discards), the late game - or when `p` is fighting for its life.
fn floor_relaxed(g: &Game, p: usize) -> bool {
    g.players[p].hand.len() >= 7 || g.round >= 10 || g.players[p].life <= 8
}

/// Removal's target floor: is permanent `i` (an opponent's) worth one of `p`'s removal
/// cards? A commander or planeswalker; a big body (4+ power, or worth a vanilla 4/4); an
/// evasive attacker; an engine (a permanent whose abilities keep paying off); a creature
/// that threatens a real chunk of our life; or the table's biggest threat. Anything else
/// (a 1/1 token, a vanilla 2/2) isn't: the card waits in hand for a target worth it.
/// When the card would rot (`floor_relaxed`), anything but small fry is fair game.
///
/// It applies to removal spells in hand (which can wait), not to "when ~ enters" triggers
/// (now or never). Measured (one seat with the floor against three without, 2 x 20,000
/// paired games, a Thanos deck with five removal spells): +0.06 ± 0.03 win-rate points -
/// the old valuation already held most removal for bigger targets; the floor stops the
/// rest (a removal spell on a 2/2 token at the end of a turn, or on a small attacker).
/// (Removal itself is valued at 0.6 of the victim's worth: see `removal_discount`.)
pub(crate) fn worth_removing(g: &Game, p: usize, i: usize) -> bool {
    if !floor_on(p) {
        return true;
    }
    let x = &g.perms[i];
    let v = perm_value(g, i);
    if floor_relaxed(g, p) {
        return v >= 2.5 || x.commander > 0;
    }
    if x.commander > 0 || x.is(T_PLANESWALKER) || v >= 6.0 {
        return true;
    }
    let abilities = g.abilities_of(x);
    if x.is_creature() {
        let power = x.p.max(0) as i32 * if x.has(DOUBLE_STRIKE) { 2 } else { 1 };
        let evasive = x.has(FLYING | MENACE | UNBLOCKABLE | FEAR | INTIMIDATE | SKULK);
        let life = g.players[p].life;
        if power >= 4
            || evasive && power >= 2
            || engine_value(abilities) >= 2.0
            || power >= 3 && power * 5 >= life
        {
            return true;
        }
    } else if engine_value(abilities) >= 1.5 {
        return true;
    }
    // The table's biggest threat: the most valuable permanent any opponent has.
    v >= 4.0
        && !g.perms.iter().enumerate().any(|(j, y)| {
            j != i
                && y.ctrl as usize != p
                && !y.is(T_LAND)
                && g.players[y.ctrl as usize].alive
                && perm_value(g, j) > v
        })
}

/// Removing permanent `i` is worth less when it's a commander (it comes back, for more
/// mana) or its controller is about to die anyway.
fn removal_discount(g: &Game, p: usize, i: usize) -> f32 {
    let x = &g.perms[i];
    let o = x.ctrl as usize;
    let mut f = 1.0;
    if x.commander > 0 {
        // It comes back, but for two more mana each time: a real tempo gain still.
        f *= 0.85;
    }
    // Our creatures that can attack now, against their life.
    let clock: i32 = g
        .perms
        .iter()
        .filter(|y| y.ctrl as usize == p && y.is_creature() && !y.tapped && !y.sick)
        .map(|y| y.p.max(0) as i32)
        .sum();
    if g.players[o].life <= 3 || g.active == p && !g.in_combat && clock >= g.players[o].life * 2 {
        f *= 0.5;
    }
    // A one-for-one in a four-player game: the creature removed was hurting one of three
    // opponents' victims, and the card and mana could have gone on our own board instead.
    // Every seat plays this, so it is tuned as a table-wide equilibrium (a Thanos deck with
    // Hero's Downfall, 30,000-40,000 paired games per test, one seat changed against a
    // table at the current setting): from 0.375, 0.6 for one seat gained +0.32 win-rate
    // points; with the table at 0.6, 0.75 gained +0.2 more (and halving the hold-back in
    // `options` +0.17); with the table at 0.75 and half the hold-back, no single seat
    // gains from changing either (0.6, 1.0, full or quarter hold-back: all within
    // ±0.15). An older test that changed every seat at once read higher values as worse,
    // because the opponents' removal got better too.
    f * 0.75
}

/// What an effect would do for player `p` right now (card points).
pub(crate) fn effect_now(
    g: &Game,
    p: usize,
    e: &Effect,
    text: &str,
    src: Option<u32>,
    card_restr: Restr,
) -> f32 {
    effect_now_with(g, p, e, text, src, card_restr, false)
}

/// `effect_now`; with `floor`, one-for-one removal counts only targets worth a removal card
/// (`worth_removing`).
fn effect_now_with(
    g: &Game,
    p: usize,
    e: &Effect,
    text: &str,
    src: Option<u32>,
    card_restr: Restr,
    floor: bool,
) -> f32 {
    let ctx = Ctx {
        ctrl: p,
        src,
        snap: None,
        card: None,
        card_ref: None,
        subject: None,
        text,
        until_eot: false,
        overload: false,
        card_restr,
        x: None,
        paid: None,
        teamwork: false,
        about: None,
    };
    let d = g.dyn_values(&card_restr, Some(&ctx), p, src);
    // Removal (not theft or locks): see `removal_discount`.
    let removal = matches!(
        e,
        Effect::Destroy(_)
            | Effect::Exile { .. }
            | Effect::Damage { .. }
            | Effect::Shrink { .. }
            | Effect::Bounce(_)
            | Effect::Fight(_)
    );
    let opp_value = |t: &crate::abilities::Target, kill: &dyn Fn(&Perm) -> bool| -> f32 {
        let cands = g.candidates(&ctx, t, true);
        if t.mass {
            cands
                .iter()
                .filter(|&&i| kill(&g.perms[i]))
                .map(|&i| {
                    let o = g.perms[i].ctrl as usize;
                    if o == p {
                        -perm_value(g, i)
                    } else {
                        perm_value(g, i) * weight(g, p, o) * 0.8
                    }
                })
                .sum()
        } else {
            // A removal spell only counts a target worth the card (`worth_removing`).
            cands
                .iter()
                .filter(|&&i| {
                    g.perms[i].ctrl as usize != p
                        && kill(&g.perms[i])
                        && !(removal && floor && !worth_removing(g, p, i))
                })
                .map(|&i| {
                    perm_value(g, i)
                        * weight(g, p, g.perms[i].ctrl as usize)
                        * 0.8
                        * if removal {
                            removal_discount(g, p, i)
                        } else {
                            1.0
                        }
                })
                .fold(0.0, f32::max)
        }
    };
    match e {
        Effect::Destroy(_) if text.contains("odd or even") => best_parity(g, p, src).1,
        Effect::Destroy(t) => opp_value(t, &|x| !x.has(INDESTRUCTIBLE)),
        Effect::Exile { target, .. } => opp_value(target, &|_| true),
        Effect::Damage { amount, to } => {
            let n = amount.n.max(1) as i16;
            let creatures = opp_value(to, &|x| {
                x.is_creature() && x.t - x.damage <= n && !x.has(INDESTRUCTIBLE)
                    || x.is(T_PLANESWALKER) && x.loyalty <= n
            });
            let face = if to.kind == Kind::AnyTarget
                || to.kind == Kind::Player
                || text.contains("each opponent")
            {
                n as f32
                    * 0.35
                    * if text.contains("each opponent") {
                        g.opponents(p).len() as f32
                    } else {
                        1.0
                    }
            } else {
                0.0
            };
            creatures.max(face) + if to.mass { face } else { 0.0 }
        }
        Effect::Shrink { amount, to, .. } => {
            let n = amount.n as i16;
            opp_value(to, &|x| x.t <= n)
        }
        Effect::Bounce(t) => opp_value(t, &|x| x.is_token() || x.card.is_some()) * 0.4,
        Effect::Lock { target, lasting } => {
            opp_value(target, &|x| x.is_creature()) * if *lasting { 0.8 } else { 0.2 }
        }
        Effect::Fight(t) => {
            let mine = best_fighter(g, p).map(|i| g.perms[i].p).unwrap_or(0);
            if mine == 0 {
                0.0
            } else {
                opp_value(t, &|x| x.t <= mine)
            }
        }
        Effect::Edict { each_opponent } => {
            let hit = |o: usize| {
                sacrifice_choice_r(g, o, T_CREATURE, &card_restr)
                    .map_or(0.0, |i| perm_value(g, i) * weight(g, p, o) * 0.8)
            };
            let theirs = if *each_opponent {
                g.opponents(p).into_iter().map(hit).sum()
            } else {
                // "Target opponent sacrifices": the one it hurts most.
                edict_victim(g, p, &card_restr).map_or(0.0, |o| hit(o))
            };
            let mine = if text.contains("each player sacrifices") {
                sacrifice_choice_r(g, p, T_CREATURE, &card_restr).map_or(0.0, |i| perm_value(g, i))
            } else {
                0.0
            };
            theirs - mine
        }
        Effect::CounterSpell | Effect::Fog => 0.0,
        Effect::Draw { amount, you: true } => draw_value(g, p, amount.n.max(1)),
        Effect::Draw { amount, .. } => {
            2.5 * amount.n.max(1) as f32
                + if g.players[p].hand.len() <= 1 {
                    0.6
                } else {
                    0.0
                }
        }
        Effect::Tokens {
            amount,
            power,
            toughness,
            ..
        } => amount.n.max(1) as f32 * (0.4 + (*power + *toughness).max(0) as f32 * 0.5),
        Effect::PlusCounters { amount, to } => {
            let mine = g
                .perms
                .iter()
                .filter(|x| x.ctrl as usize == p && x.is_creature())
                .count() as f32;
            if to.kind == Kind::It {
                amount.n as f32 * 0.8
            } else if mine == 0.0 {
                0.0
            } else {
                amount.n as f32 * if to.mass { 0.6 * mine } else { 0.8 }
            }
        }
        Effect::Pump {
            power,
            toughness,
            to,
            until_eot,
        } => {
            let mine = g
                .perms
                .iter()
                .filter(|x| x.ctrl as usize == p && x.is_creature())
                .count() as f32;
            let base = (*power + *toughness).max(0) as f32;
            if *until_eot && !to.mass {
                0.0 // a combat trick: used in combat
            } else if to.mass {
                base * 0.4 * mine
            } else {
                base * 0.5
            }
        }
        Effect::GainLife(n) => n.n as f32 * if g.players[p].life < 12 { 0.3 } else { 0.08 },
        Effect::Drain { amount, each } => {
            amount.n as f32
                * if *each {
                    0.8 * g.opponents(p).len() as f32
                } else {
                    0.6
                }
        }
        Effect::FetchLand { battlefield } => {
            if *battlefield {
                if g.round <= 6 { 2.5 } else { 1.0 }
            } else if lands_total(g, p) < 5 {
                1.2
            } else {
                0.3
            }
        }
        Effect::Reanimate => reanimate_choice(g, p, text.contains("a graveyard"), &card_restr, &d)
            .map(|(o, k)| g.players[o].cards[g.players[o].graveyard[k] as usize].value)
            .unwrap_or(0.0),
        Effect::Regrowth => regrowth_choice(g, p, text.contains("creature card"), &card_restr, &d)
            .map(|k| g.players[p].cards[g.players[p].graveyard[k] as usize].value * 0.6)
            .unwrap_or(0.0),
        Effect::GainControl(t) => {
            opp_value(t, &|_| true)
                * if text.contains("until end of turn") {
                    0.4
                } else {
                    1.6
                }
        }
        Effect::AddMana(n) => n.n as f32 * 0.3,
        Effect::Blink => {
            let not = src.filter(|_| text.contains("target"));
            if text.contains("at the beginning of the next end step") {
                blink_choice(g, p, not, blink_types(text))
            } else {
                blink_choice_now(g, p, not, blink_types(text))
            }
            .map(|i| blink_gain(g, i) * 0.8)
            .unwrap_or(0.0)
        }
        Effect::Copy => best_to_copy(g, p, true)
            .map(|i| perm_value(g, i) * 0.7)
            .unwrap_or(0.5),
        Effect::Goad => {
            opp_value(
                &crate::abilities::Target {
                    kind: Kind::Creature,
                    whose: Whose::Theirs,
                    mass: false,
                    other: false,
                    restricted: false,
                    restr: Default::default(),
                    count: 1,
                },
                &|x| x.is_creature(),
            ) * 0.3
        }
        Effect::Untap(_) => 0.4,
        // Nothing to find, nothing gained.
        Effect::Tutor if tutor_choice(g, p, text, &card_restr).is_none() => 0.0,
        Effect::Rule(l) => rule_now(g, p, l, src),
        Effect::Attach => {
            if equip_target(g, p, 0).is_some() {
                0.8
            } else {
                0.0
            }
        }
        _ => effect_value(e),
    }
}

/// Drawing `n` cards now: 2.5 a card while the library is deep and the hand has room. Cards
/// past a full hand are mostly discarded (a little filtering), a thin library makes each
/// card dearer to lose than to draw, and drawing more than it holds loses the game.
fn draw_value(g: &Game, p: usize, n: i32) -> f32 {
    let pl = &g.players[p];
    let lib = pl.library.len() as i32;
    if n > lib {
        return -30.0;
    }
    // Library left after the draw: full value from 15 cards, nothing at 3 or fewer.
    let deep = ((lib - n - 3) as f32 / 12.0).clamp(0.0, 1.0);
    // Room in hand (a card or two get played before the end of the turn).
    let room = (9 - pl.hand.len() as i32).max(0);
    let kept = n.min(room) as f32;
    let spill = (n - room).max(0) as f32;
    let v = (2.5 * kept + 0.5 * spill) * deep;
    v + if pl.hand.len() <= 1 { 0.6 * deep } else { 0.0 } - if deep == 0.0 { 1.0 } else { 0.0 }
}

/// What a special rule (`Effect::Rule`) the simulator plays is worth to `p` now.
fn rule_now(g: &Game, p: usize, label: &str, src: Option<u32>) -> f32 {
    let me = src.and_then(|id| g.idx(id));
    match label {
        // The back face's body and abilities against what it is now.
        "transforms" => {
            let Some(i) = me else { return 0.0 };
            let x = &g.perms[i];
            let Some(b) = x
                .card
                .and_then(|r| g.card(r).back.as_ref())
                .filter(|_| !x.transformed)
            else {
                return 0.0;
            };
            let now = x.p.max(0) as f32
                + 0.6 * x.t.max(0) as f32
                + kw_value(x.eff_kw, x.p.max(0) as f32)
                + engine_value(g.abilities_of(x));
            let p2 = (b.power + x.counters).max(0) as f32;
            let then = p2
                + 0.6 * (b.toughness + x.counters).max(0) as f32
                + kw_value(b.kw, p2)
                + engine_value(&b.abilities);
            then - now
        }
        "extra turn" => 6.0,
        // An Infinity Stone's "∞" ability, for the rest of the game.
        "harness (turns on its infinity ability)" => me
            .filter(|&i| !g.perms[i].powered)
            .map(|i| {
                g.abilities_of(&g.perms[i])
                    .iter()
                    .filter(|a| matches!(a.cond, Cond::Harnessed))
                    .map(|a| engine_value(std::slice::from_ref(a)) / 0.6)
                    .sum()
            })
            .unwrap_or(0.0),
        // Worth it before our attack, when an opponent has a blocker to stop.
        // In our combat before blocks: what taking the defender's key blocker away gains.
        "can't block this turn" if g.in_combat => net_target(g, p).map_or(0.0, |x| x.1.min(8.0)),
        "can't block this turn" => {
            let attackers = g
                .perms
                .iter()
                .filter(|x| x.ctrl as usize == p && x.is_creature() && !x.tapped && !x.sick)
                .count();
            let blockers = g
                .perms
                .iter()
                .any(|x| x.ctrl as usize != p && x.is_creature() && !x.tapped);
            if g.active == p && !g.in_combat && attackers > 0 && blockers {
                0.8
            } else {
                0.0
            }
        }
        "shuffles itself into its library" | "gives itself to an opponent" => {
            -me.map_or(1.0, |i| perm_value(g, i) * 0.6)
        }
        "ends the turn" => 0.0,
        "spells cost less this turn" | "the next spell this turn is free" => 1.0,
        "attacks change until your next turn" => 1.5,
        // A land or artifact that becomes a creature: worth it to attack with this turn.
        "becomes a creature" => {
            let ready = me.is_some_and(|i| {
                let x = &g.perms[i];
                !x.is_creature() && !x.tapped && !x.sick
            });
            if ready && g.active == p && !g.in_combat {
                1.0
            } else {
                0.0
            }
        }
        // An ultimate: lasts the rest of the game.
        "you get an emblem" => 6.0,
        "gets +N/+N until your next turn" => 1.2,
        // The Kingpin of Crime: the extra damage from creatures tougher than they're strong.
        "assigns combat damage by toughness" => {
            let combat = g.in_combat;
            g.perms
                .iter()
                .filter(|x| x.ctrl as usize == p && x.is_creature() && (!combat || x.attacking))
                .map(|x| (x.t - x.p.max(0)).max(0) as f32 * 0.3)
                .sum()
        }
        // A 5/5 flier for an opponent.
        "an opponent creates a creature token" => -3.0,
        _ => effect_value(&Effect::Rule(label.to_string())),
    }
}

/// Value of an ability's effects for `p` now (modal: the best mode).
fn ability_now(g: &Game, p: usize, a: &SimAbility, src: Option<u32>) -> f32 {
    ability_now_with(g, p, a, src, false)
}

/// `ability_now`; with `floor`, one-for-one removal counts only targets worth a removal
/// card (`worth_removing`) - for a spell cast from the hand, which can wait for one.
fn ability_now_with(g: &Game, p: usize, a: &SimAbility, src: Option<u32>, floor: bool) -> f32 {
    if a.modal {
        let two = a.text.contains("choose two");
        let mut v: Vec<f32> = a
            .mode_groups()
            .into_iter()
            .enumerate()
            .map(|(m, r)| {
                let (text, cr) = a.mode_text(m);
                a.effects[r]
                    .iter()
                    .map(|e| effect_now_with(g, p, e, text, src, cr, floor))
                    .sum()
            })
            .collect();
        v.sort_by(|a, b| b.partial_cmp(a).unwrap());
        if any_number_of_modes(a) {
            // Every mode worth something (at least the best one).
            return v.first().copied().unwrap_or(0.0)
                + v.iter().skip(1).filter(|x| **x > 0.0).sum::<f32>();
        }
        v.iter().take(if two { 2 } else { 1 }).sum()
    } else {
        a.effects
            .iter()
            .map(|e| effect_now_with(g, p, e, &a.text, src, a.card_restr, floor))
            .sum()
    }
}

/// The modes a modal ability picks: the best one, or two (`used`: a bit per mode already
/// chosen, for "choose one that hasn't been chosen").
pub fn choose_modes(g: &Game, ctx: &Ctx, ab: &SimAbility, used: u32) -> Vec<usize> {
    // "Choose one. If this spell was cast using teamwork, choose both instead."
    if ctx.teamwork && ab.text.contains("choose both instead") {
        return (0..ab.mode_groups().len()).collect();
    }
    let two = ab.text.contains("choose two");
    let groups = ab.mode_groups();
    let mut v: Vec<(f32, usize)> = groups
        .iter()
        .enumerate()
        .filter(|(m, _)| *m >= 32 || used & (1 << m) == 0)
        .map(|(m, r)| {
            let (text, cr) = ab.mode_text(m);
            let val = ab.effects[r.clone()]
                .iter()
                .map(|e| effect_now(g, ctx.ctrl, e, text, ctx.src, cr))
                .sum();
            (val, m)
        })
        .collect();
    v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    // "Choose one or both" / "one or more" (Decoy Ploy, Pinecone Strike): the best mode and
    // every other one that does something.
    let any_number = any_number_of_modes(ab);
    let chosen: Vec<usize> = v
        .into_iter()
        .enumerate()
        .take_while(|(k, (val, _))| *k < if two { 2 } else { 1 } || any_number && *val > 0.0)
        .map(|(_, (_, m))| m)
        .collect();
    chosen
}

/// "Choose one or both" / "choose one or more": any number of the modes, at least one.
fn any_number_of_modes(a: &SimAbility) -> bool {
    [
        "choose one or both",
        "choose one or more",
        "choose any number",
    ]
    .iter()
    .any(|w| a.text.contains(w))
}

pub fn wants_optional(g: &Game, ctx: &Ctx, ab: &SimAbility) -> bool {
    let t = ab.text.as_str();
    if t.contains("you may sacrifice") {
        return fodder(g, ctx.ctrl, T_CREATURE | T_ARTIFACT, ctx.src.unwrap_or(0)).is_some();
    }
    if t.contains("you may discard") {
        return g.players[ctx.ctrl].hand.len() >= 2;
    }
    if t.contains("you may pay") {
        return g.players[ctx.ctrl].life > 10 || !t.contains("life");
    }
    ability_now(g, ctx.ctrl, ab, ctx.src) >= 0.0
}

/// Is paying a cost while an ability resolves ("you may sacrifice another creature. If you
/// do, ...") worth what it gets?
pub fn wants_to_pay(g: &Game, ctx: &Ctx, ab: &SimAbility, pc: &PayCost) -> bool {
    let p = ctx.ctrl;
    let mut gain: f32 = ab
        .effects
        .iter()
        .skip(pc.first as usize)
        .map(|e| effect_now(g, p, e, &ab.text, ctx.src, ab.card_restr))
        .sum();
    // "Create a token that's a copy of it" about the permanent that just entered (Ultron):
    // worth that permanent, plus a 2/2 body when "it becomes a 2/2 ... creature".
    if ab.text.contains("copy of it") {
        if let Some(i) = ctx
            .subject
            .filter(|&s| Some(s) != ctx.src)
            .and_then(|s| g.idx(s))
        {
            let body = if !g.perms[i].is_creature() && ab.text.contains("it becomes a 2/2") {
                2.0
            } else {
                0.0
            };
            gain = gain.max(perm_value(g, i) + body);
        }
    }
    match pay_price(g, p, ctx.src, pc) {
        Some(None) => gain >= 0.0,
        Some(Some(price)) => gain > price,
        None => false,
    }
}

/// What paying a resolving ability's cost would cost `p` (None: can't pay; Some(None):
/// costs nothing worth counting).
fn pay_price(g: &Game, p: usize, src_id: Option<u32>, pc: &PayCost) -> Option<Option<f32>> {
    let src = src_id.unwrap_or(u32::MAX);
    let pl = &g.players[p];
    let price = match &pc.cost {
        ResCost::Free | ResCost::SacrificeSelf | ResCost::ExileSelf | ResCost::ExileNamed(_) => {
            return Some(None);
        }
        ResCost::Sacrifice { types, other } => {
            perm_value(
                g,
                fodder(g, p, *types, if *other { src } else { u32::MAX })?,
            ) * 0.8
        }
        ResCost::SacrificeOrDiscard(types, nonland) => {
            match g.sac_or_discard(p, fodder(g, p, *types, src), *nonland)? {
                Ok(i) => perm_value(g, i) * 0.8,
                Err(h) => card_worth(g, p, &pl.cards[pl.hand[h] as usize]).max(1.0),
            }
        }
        ResCost::Mana(m) => {
            if !g.can_pay(p, m) {
                return None;
            }
            let flat = m.total() as f32 * 0.3;
            // In our own combat the mana may be what the post-combat plan casts.
            if g.in_combat && g.active == p {
                flat.max(post_combat_loss(g, p, m.total()))
            } else {
                flat
            }
        }
        ResCost::Life(n) => {
            if pl.life <= *n as i32 + 5 {
                return None;
            }
            *n as f32 * if pl.life < 15 { 0.3 } else { 0.1 }
        }
        ResCost::Discard => {
            if pl.hand.is_empty() {
                return None;
            }
            discard_choice(g, p)
                .map(|h| card_worth(g, p, &pl.cards[pl.hand[h] as usize]) * 0.5)
                .unwrap_or(1.0)
        }
        ResCost::RemoveCounter { plus } => {
            let x = src_id.and_then(|id| g.idx(id)).map(|i| &g.perms[i])?;
            if *plus {
                // Not the last counter keeping it alive.
                if x.counters < 1 || x.t <= 1 {
                    return None;
                }
                0.8
            } else {
                if x.other_counters < 1 {
                    return None;
                }
                0.0
            }
        }
    };
    Some(Some(price))
}

/// What casting a spell is worth to `p` right now.
fn spell_value(g: &Game, p: usize, idx: u16, back: bool, commander: bool) -> f32 {
    let c = &g.players[p].cards[idx as usize];
    if is_reactive_only(c) && !back {
        return 0.0;
    }
    let mut v = 0.0;
    let permanent = !back && c.is_permanent();
    if permanent {
        if c.is(T_CREATURE) {
            v += c.power.max(0) as f32
                + 0.6 * c.toughness.max(0) as f32
                + kw_value(c.kw, c.power.max(0) as f32);
            // Behind on board: bodies matter more.
            let mine = g
                .perms
                .iter()
                .filter(|x| x.ctrl as usize == p && x.is_creature())
                .count();
            let theirs = g
                .opponents(p)
                .iter()
                .map(|&o| {
                    g.perms
                        .iter()
                        .filter(|x| x.ctrl as usize == o && x.is_creature())
                        .count()
                })
                .max()
                .unwrap_or(0);
            if mine < theirs {
                v += 1.0;
            }
        }
        if c.is(T_PLANESWALKER) {
            v += 2.5 + c.loyalty as f32 * 0.5;
        }
        v += engine_value(&c.abilities);
        // Abilities about Equipment (Ronin's mana for Equipment, "sacrifice an Equipment
        // attached to ~") do nothing for a player without any.
        let no_gear = true
            && c.abilities.iter().any(|a| a.text.contains("equipment"))
            && !has_equipment(g, p);
        if no_gear {
            for a in c.abilities.iter().filter(|a| a.text.contains("equipment")) {
                v -= engine_value(std::slice::from_ref(a));
            }
        }
        if c.mana_ability && !c.is(T_LAND) && !(no_gear && c.spend_only.is_some()) {
            v += if g.round <= 5 { 3.0 } else { 1.2 };
        }
        if c.is(T_EQUIPMENT) {
            let has_creature = g
                .perms
                .iter()
                .any(|x| x.ctrl as usize == p && x.is_creature());
            v += if has_creature { 1.0 } else { 0.3 };
        }
        if c.is(T_AURA) {
            if aura_target(g, p, (p as u8, idx)).is_none() {
                return 0.0;
            }
            if c.harmful_aura {
                if let Some(t) = aura_target(g, p, (p as u8, idx)) {
                    v += perm_value(g, t) * 0.8;
                }
            }
        }
        for a in c.abilities.iter().filter(|a| a.trig == Trig::Enters) {
            if g.cond_holds(&a.cond, p, None)
                || matches!(
                    a.cond,
                    Cond::Teamwork | Cond::Gift | Cond::Kicked | Cond::Chance(_)
                )
            {
                let now = ability_now(g, p, a, None);
                // A cost paid as it resolves: counted only if it would be paid.
                v += match &a.pay {
                    Some(pc) => match pay_price(g, p, None, pc) {
                        Some(None) => now.max(0.0),
                        Some(Some(price)) if now > price => now - price,
                        _ => 0.0,
                    },
                    None => now,
                };
            }
        }
        // A Saga's chapters: the first happens now, the others one turn apart.
        if c.is(T_SAGA) {
            v += saga_value(g, p, &c.abilities, 0);
        }
        if commander {
            v += 1.5;
        }
    } else if !back && copies_x_times(c) {
        // "Copy it X times" (Photon Blast Barrage): one instance a target, as many as the
        // mana pays for.
        let spare = g.mana_available(p).saturating_sub(c.cost.total());
        v += barrage_targets(g, p, c)
            .iter()
            .take(spare as usize + 1)
            .sum::<f32>();
    } else {
        for a in spell_abilities(c, back)
            .iter()
            .filter(|a| a.trig == Trig::Spell)
        {
            let ok = match &a.cond {
                Cond::Teamwork => c
                    .teamwork
                    .is_some_and(|n| !teamwork_tappers(g, p, n as i16).is_empty()),
                Cond::Gift => c.gift,
                Cond::CastFromGraveyard => false,
                Cond::Kicked => c
                    .kicker
                    .is_some_and(|k| g.mana_available(p) >= c.cost.total() + k.total()),
                other => g.cond_holds(other, p, None),
            };
            if ok {
                // Removal waits for a target worth the card.
                v += ability_now_with(g, p, a, None, true);
            }
        }
        // Card advantage aside, a sorcery that does nothing now waits.
    }
    // "As an additional cost to cast this spell, sacrifice a creature / discard a card".
    let extra = if back {
        c.back.as_ref().and_then(|b| b.extra_cost.as_ref())
    } else {
        c.extra_cost.as_ref()
    };
    if let Some(e) = extra {
        match g.extra_payment(p, e, &c.cost, true) {
            Some(ExtraPay::Sacrifice(id)) => {
                v -= g.idx(id).map(|i| perm_value(g, i)).unwrap_or(0.0);
            }
            Some(ExtraPay::Discard) => v -= 1.0,
            Some(_) => {}
            None => return 0.0,
        }
    }
    v
}

/// "When you cast this spell, copy it X times."
pub fn copies_x_times(c: &SimCard) -> bool {
    c.abilities
        .iter()
        .any(|a| a.trig == Trig::Spell && a.text.contains("copy it x times"))
}

/// For a "deals N damage to target creature" spell copied X times: what each distinct
/// target it kills is worth (best first, the ones worth a mana or more).
fn barrage_targets(g: &Game, p: usize, c: &SimCard) -> Vec<f32> {
    let Some(n) = c
        .abilities
        .iter()
        .filter(|a| a.trig == Trig::Spell && !a.text.contains("copy it"))
        .flat_map(|a| a.effects.iter())
        .find_map(|e| match e {
            Effect::Damage { amount, .. } => Some(amount.n.max(1) as i16),
            _ => None,
        })
    else {
        return Vec::new();
    };
    let mut v: Vec<f32> = g
        .perms
        .iter()
        .enumerate()
        .filter(|(_, x)| {
            x.ctrl as usize != p
                && x.is_creature()
                && !x.has(INDESTRUCTIBLE | HEXPROOF | SHROUD)
                && x.t - x.damage <= n
        })
        .map(|(i, x)| {
            perm_value(g, i) * weight(g, p, x.ctrl as usize) * 0.8 * removal_discount(g, p, i)
        })
        .filter(|&v| v >= 0.35)
        .collect();
    v.sort_by(|a, b| b.partial_cmp(a).unwrap());
    v
}

/// The X for a "copy it X times" spell: one copy for each worthwhile target beyond the
/// first, no more than `spare` mana pays for.
pub fn copies_x(g: &Game, p: usize, c: &SimCard, spare: u32) -> u32 {
    (barrage_targets(g, p, c).len() as u32)
        .saturating_sub(1)
        .min(spare)
}

/// What a Saga's chapters after the `done` first ones are worth to `p`: the next one at what
/// it would do now when it's chapter I (it triggers as the Saga enters), the later ones at
/// their usual worth, less for each turn they're away.
fn saga_value(g: &Game, p: usize, abilities: &[SimAbility], done: i16) -> f32 {
    abilities
        .iter()
        .filter_map(|a| match a.trig {
            Trig::Chapter(n) if n as i16 > done => Some((n, a)),
            _ => None,
        })
        .map(|(n, a)| {
            if n == 1 {
                ability_now(g, p, a, None).max(0.0)
            } else {
                let e: f32 = a.effects.iter().map(effect_value).sum();
                e * 0.8f32.powi(n as i32 - 1 - done as i32)
            }
        })
        .sum()
}

/// Does `p` have an Equipment on the battlefield, in hand or in its library?
fn has_equipment(g: &Game, p: usize) -> bool {
    let pl = &g.players[p];
    g.perms
        .iter()
        .any(|x| x.ctrl as usize == p && x.is(T_EQUIPMENT))
        || pl
            .hand
            .iter()
            .chain(pl.library.iter())
            .any(|&k| pl.cards[k as usize].is(T_EQUIPMENT))
}

/// Value of keeping a reactive card ready, and its cost.
fn reactive_value(g: &Game, p: usize, c: &SimCard) -> f32 {
    let opp_power: i32 = g
        .opponents(p)
        .iter()
        .map(|&o| {
            g.perms
                .iter()
                .filter(|x| x.ctrl as usize == o && x.is_creature())
                .map(|x| x.p.max(0) as i32)
                .sum::<i32>()
        })
        .max()
        .unwrap_or(0);
    // Early on, opponents cast little worth an answer: developing the board matters more.
    let stage = (0.8 + 0.4 * g.round as f32).min(4.0);
    if is_counter(c) {
        stage
    } else if is_instant_removal(c) {
        if opp_power > 0 { stage.min(3.5) } else { 1.0 }
    } else if is_fog(c) {
        if opp_power * 2 >= g.players[p].life {
            4.0
        } else {
            1.0
        }
    } else if c.is(T_CREATURE) && has_flash(g, p, c) {
        c.value * 0.8
    } else if is_protection(c) {
        2.0
    } else if is_trick(c) {
        1.5
    } else {
        0.0
    }
}

// ------------------------------------------------------------ main phase

#[derive(Debug, Clone, Copy)]
enum Act {
    Cast(Src, CastOpts),
    Activate(u32, usize),
    /// An ability of a card in the graveyard: (graveyard slot, ability).
    ActivateGy(usize, usize),
    Equip(u32, u32),
    Cycle(usize),
    /// An ability of a card in hand that works from there: (hand slot, ability).
    ActivateHand(usize, usize),
}

/// Value of the best set of spells (value, mana, colours needed) castable with `budget` mana
/// of colours `have`.
fn spells_worth(spells: &[(f32, u32, u8)], budget: u32, have: u8) -> f32 {
    let b = budget.min(20) as usize;
    let mut dp = vec![0.0f32; b + 1];
    for &(v, cost, need) in spells {
        if need & !have != 0 || cost as usize > b {
            continue;
        }
        for m in (cost as usize..=b).rev() {
            dp[m] = dp[m].max(dp[m - cost as usize] + v);
        }
    }
    dp[b]
}

/// Lands: the one that fixes what the hand needs. A tapped land is held back when an untapped
/// one would get something cast this turn, and played on a turn whose extra mana wouldn't be
/// used (it costs nothing then).
fn choose_land(g: &Game, p: usize) -> Option<usize> {
    let pl = &g.players[p];
    let lands: Vec<usize> = (0..pl.hand.len())
        .filter(|&h| pl.cards[pl.hand[h] as usize].is(T_LAND))
        .collect();
    if lands.len() <= 1 {
        return lands.first().copied();
    }
    let missing = missing_colors(g, p);
    let mana = g.mana_available(p);
    let have = g.sources(p).iter().fold(0u8, |m, s| m | s.1);
    // What the hand (and command zone) could cast, with the colours it needs.
    let mut spells: Vec<(f32, u32, u8)> = Vec::new();
    for (h, &idx) in pl.hand.iter().enumerate() {
        let c = &pl.cards[idx as usize];
        if c.is(T_LAND) || is_reactive_only(c) {
            continue;
        }
        if let Some(cost) = g.cast_cost(p, Src::Hand(h), CastOpts::default()) {
            // (Instants and flash spells as the main phase values them: they wait.)
            let later = true && (c.kw & FLASH != 0 || c.is(T_INSTANT));
            spells.push((
                spell_value(g, p, idx, false, false) * if later { 0.6 } else { 1.0 },
                cost.total(),
                pip_mask(&cost),
            ));
        }
    }
    for (k, &(idx, _)) in pl.command.iter().enumerate() {
        if let Some(cost) = g.cast_cost(p, Src::Command(k), CastOpts::default()) {
            spells.push((
                spell_value(g, p, idx, false, true),
                cost.total(),
                pip_mask(&cost),
            ));
        }
    }
    let now = spells_worth(&spells, mana, have);
    let busy = lands.iter().any(|&h| {
        let c = &pl.cards[pl.hand[h] as usize];
        !c.enters_tapped
            && spells_worth(
                &spells,
                mana + c.mana_amount.max(1) as u32,
                have | c.produces,
            ) > now + 0.3
    });
    lands.into_iter().max_by_key(|&h| {
        let c = &pl.cards[pl.hand[h] as usize];
        let fixes = (c.produces & missing).count_ones() as i32 * 10;
        let tapped = match (c.enters_tapped, busy) {
            (true, true) => -6,
            (true, false) => 3,
            _ => 0,
        };
        fixes + tapped + c.produces.count_ones() as i32
    })
}

/// A land `p` may play from outside the hand: (true, graveyard slot) with Mole Man or
/// Muldrotha, (false, playable slot) from exile.
fn other_zone_land(g: &Game, p: usize) -> Option<(bool, usize)> {
    let pl = &g.players[p];
    if g.rules[p % 8] & R_LANDS_FROM_GY != 0 && g.active == p {
        let best = (0..pl.graveyard.len())
            .filter(|&k| pl.cards[pl.graveyard[k] as usize].is(T_LAND))
            .max_by_key(|&k| pl.cards[pl.graveyard[k] as usize].produces.count_ones());
        if let Some(k) = best {
            return Some((true, k));
        }
    }
    // Ka-Zar: a land on top of the library is a free card.
    if g.rules[p % 8] & R_LANDS_FROM_TOP != 0
        && g.active == p
        && pl
            .library
            .last()
            .is_some_and(|&c| pl.cards[c as usize].is(T_LAND))
    {
        return Some((false, FROM_LIBRARY_TOP));
    }
    (0..pl.playable.len())
        .find(|&k| {
            let x = &pl.playable[k];
            x.card.0 as usize == p
                && pl.cards[x.card.1 as usize].is(T_LAND)
                && (!x.own_turn || g.active == p)
        })
        .map(|k| (false, k))
}

fn cast_opts(g: &Game, p: usize, idx: u16, back: bool, post: bool) -> CastOpts {
    let c = &g.players[p].cards[idx as usize];
    let teamwork = c
        .teamwork
        .is_some_and(|n| post && !teamwork_tappers(g, p, n as i16).is_empty());
    let mut o = CastOpts {
        back,
        teamwork,
        gift: c.gift,
        ..Default::default()
    };
    if c.offspring.is_some()
        && g.mana_available(p) >= c.mv as u32 + c.offspring.unwrap_or(0) as u32 + 1
    {
        o.offspring = true;
    }
    if (c.convoke || g.improvises(p, c)) && post {
        o.convoke = true;
    }
    let mana = g.mana_available(p);
    if let Some(k) = c.kicker {
        if mana >= c.cost.total() + k.total() {
            o.kicker = true;
        }
    }
    if let Some(ov) = c.overload {
        if mana >= ov.total() {
            o.overload = true;
        }
    }
    if let Some(sn) = c.sneak {
        if post && g.players[p].attacked_this_turn && sn.total() < c.cost.total() {
            o.sneak = true;
        }
    }
    o
}

/// A power-up's cost the turn its creature entered (reduced by the creature's mana cost:
/// its coloured pips off matching pips, the rest off generic; Hulk's discount too).
fn powerup_cost_on_entry(g: &Game, p: usize, c: &SimCard, full: &Mana) -> Mana {
    let mut cost = *full;
    let own = c.cost;
    let mut spare = own.generic as u32;
    for i in 0..5 {
        let off = own.pips[i].min(cost.pips[i]);
        cost.pips[i] -= off;
        spare += (own.pips[i] - off) as u32;
    }
    let less: u32 = g
        .perms
        .iter()
        .filter(|y| y.ctrl as usize == p)
        .filter_map(|y| y.card)
        .map(|r| g.card(r).power_up_less as u32)
        .sum();
    cost.minus_generic(spare + less)
}

/// Everything `p` could do at sorcery speed, with its value and mana.
fn options(g: &Game, p: usize, post: bool) -> Vec<(f32, u32, Act)> {
    let mut out = Vec::new();
    let pl = &g.players[p];
    let push_cast =
        |out: &mut Vec<(f32, u32, Act)>, src: Src, idx: u16, back: bool, commander: bool| {
            let opts = cast_opts(g, p, idx, back, post);
            let Some(cost) = g.cast_cost(p, src, opts) else {
                return;
            };
            let why = Purpose::Spell {
                card: (p as u8, idx),
                from_hand: matches!(src, Src::Hand(_)),
            };
            if !g.can_pay_for(p, &cost, why) {
                // Try without the extras.
                let plain = CastOpts {
                    back,
                    ..Default::default()
                };
                match g.cast_cost(p, src, plain) {
                    Some(c2) if g.can_pay_for(p, &c2, why) => {
                        let v = spell_value(g, p, idx, back, commander);
                        out.push((v, c2.total(), Act::Cast(src, plain)));
                    }
                    _ => {}
                }
                return;
            }
            let mut v = spell_value(g, p, idx, back, commander);
            // Flash creatures and instants are worth more cast at instant speed (end of the
            // turn before ours, or in combat), so the main phase only uses them if there's
            // nothing better to do with the mana.
            let c = &pl.cards[idx as usize];
            if !back && (c.kw & FLASH != 0 || c.is(T_INSTANT)) {
                v *= 0.6;
            }
            // One-for-one removal is kept for a target worth it: a bigger threat may come
            // (half the old hold-back: see `removal_discount`).
            if !back && is_spot_removal(c) {
                v -= (0.75 + 0.2 * g.round as f32).min(2.75);
            }
            out.push((v, cost.total(), Act::Cast(src, opts)));
            // Cast it and use its power-up the same turn, at the reduced cost (planned as
            // one action; `plan_turn` keeps it apart from the plain cast).
            if !back && c.is(T_CREATURE) {
                for a in c.abilities.iter() {
                    let Trig::PowerUp(full) = &a.trig else {
                        continue;
                    };
                    let reduced = powerup_cost_on_entry(g, p, c, full);
                    let both = mana_sum(cost, &reduced);
                    if !g.can_pay_for(p, &both, why) {
                        continue;
                    }
                    let pu = ability_now(g, p, a, None)
                        + 0.7 * full.total().saturating_sub(reduced.total()) as f32;
                    if pu > 1.0 {
                        out.push((v + pu, both.total(), Act::Cast(src, opts)));
                    }
                }
            }
        };
    for (h, &idx) in pl.hand.iter().enumerate() {
        let c = &pl.cards[idx as usize];
        if c.is(T_LAND) {
            continue;
        }
        push_cast(&mut out, Src::Hand(h), idx, false, false);
        // Abilities that work from the hand ("{3}{G}, Discard this card: ..."): the card
        // is spent on it.
        for (k, ab) in c.abilities.iter().enumerate() {
            let Some(cost) = g.hand_activation_cost(p, h, k) else {
                continue;
            };
            if g.can_pay(p, &cost) {
                let v = ability_now(g, p, ab, None)
                    - 0.1 * cost.total() as f32
                    - 0.3 * card_worth(g, p, c).max(0.0);
                out.push((v, cost.total(), Act::ActivateHand(h, k)));
            }
        }
        if c.back
            .as_ref()
            .is_some_and(|b| b.kind != BackKind::Prepare && b.kind != BackKind::Transform)
        {
            push_cast(&mut out, Src::Hand(h), idx, true, false);
        }
        // Cycling a dead card.
        if let Some((cost, find)) = &c.cycling {
            let lands = lands_total(g, p);
            let land_kind = find == "land"
                || ["plains", "island", "swamp", "mountain", "forest"].contains(&find.as_str());
            // Cycle when the card is dead now: short on lands (land-finders), or too expensive early.
            let dead = if land_kind {
                lands < 4 && !c.is(T_LAND) || c.is(T_LAND) && lands >= 6
            } else {
                c.mv as usize > lands_in_play(g, p) + 3 && g.round <= 4
                    || c.is(T_LAND) && lands >= 6
            };
            if dead && g.can_pay(p, &Mana::generic(*cost as u32)) {
                out.push((1.2, *cost as u32, Act::Cycle(h)));
            }
        }
    }
    for (k, &(idx, _)) in pl.command.iter().enumerate() {
        push_cast(&mut out, Src::Command(k), idx, false, true);
    }
    for (k, &idx) in pl.adventure.iter().enumerate() {
        push_cast(&mut out, Src::Adventure(k), idx, false, false);
    }
    // Cards in exile we may play (Thor's card, impulse draws, stashed cards).
    for (k, x) in pl.playable.iter().enumerate() {
        let (list, idx) = (x.card.0 as usize, x.card.1);
        let c = &g.players[list].cards[idx as usize];
        if list == p {
            if c.is(T_LAND) {
                continue;
            }
            push_cast(&mut out, Src::Exile(k), idx, false, false);
        } else if !c.is(T_LAND) {
            // An opponent's card: worth about what it is.
            if let Some(cost) = g.cast_cost(p, Src::Exile(k), CastOpts::default()) {
                if g.can_pay(p, &cost) {
                    out.push((
                        c.value,
                        cost.total(),
                        Act::Cast(Src::Exile(k), CastOpts::default()),
                    ));
                }
            }
        }
    }
    for (k, &idx) in pl.graveyard.iter().enumerate() {
        let c = &pl.cards[idx as usize];
        if pl.recast.contains(&idx)
            || c.flashback.is_some()
            || g.graveyard_type_free(p, c).is_some()
        {
            push_cast(&mut out, Src::Graveyard(k), idx, false, false);
        }
        // Abilities that work from the graveyard ("{3}{B}, Exile this card from your
        // graveyard: ...", Gallia returning herself).
        for (a, ab) in pl.cards[idx as usize].abilities.iter().enumerate() {
            let Some(cost) = g.graveyard_activation_cost(p, k, a) else {
                continue;
            };
            if g.can_pay(p, &cost) {
                let v = if ab.text.contains("return ~ from your graveyard") {
                    // The card itself comes back (to the battlefield, or to hand).
                    pl.cards[idx as usize].value
                        * if ab.effects.contains(&Effect::Reanimate) {
                            1.0
                        } else {
                            0.6
                        }
                } else {
                    ability_now(g, p, ab, None)
                };
                out.push((
                    v - 0.1 * cost.total() as f32,
                    cost.total(),
                    Act::ActivateGy(k, a),
                ));
            }
        }
    }
    // Mana each untapped source makes (an ability that taps it costs that mana too).
    let srcs = g.sources(p);
    for (i, x) in g.perms.iter().enumerate() {
        if x.ctrl as usize != p {
            continue;
        }
        if x.prepared {
            if let Some(r) = x.card {
                let src = Src::Prepared(x.id);
                if let Some(cost) = g.cast_cost(
                    p,
                    src,
                    CastOpts {
                        back: true,
                        ..Default::default()
                    },
                ) {
                    let why = Purpose::Spell {
                        card: r,
                        from_hand: false,
                    };
                    if g.can_pay_for(p, &cost, why) {
                        out.push((
                            spell_value(g, p, r.1, true, false),
                            cost.total(),
                            Act::Cast(
                                src,
                                CastOpts {
                                    back: true,
                                    ..Default::default()
                                },
                            ),
                        ));
                    }
                }
            }
        }
        let abilities = g.abilities_of(x);
        // Planeswalkers: one loyalty ability, the best that keeps it alive.
        let mut best_loyalty: Option<(f32, usize)> = None;
        for (k, a) in abilities.iter().enumerate() {
            // Mana abilities are paid from when something needs the mana, never done for
            // their own sake (Ramos' counters would be thrown away).
            if mana_only(a) {
                continue;
            }
            let Some(cost) = g.activation_cost(p, i, k) else {
                continue;
            };
            if !g.can_pay_for(p, &cost, Purpose::Ability { src: x.id }) {
                continue;
            }
            // (Combat AI: a team pump until end of turn only helps the creatures that attack.)
            let mut v =
                ability_now(g, p, a, Some(x.id)) - unused_team_pump(g, p, a, Some(x.id), post);
            let mut plan_cost = cost.total();
            match &a.trig {
                Trig::Loyalty(n) => {
                    let after = x.loyalty + *n as i16;
                    v += if *n > 0 {
                        0.8 * *n as f32
                    } else if after == 0 {
                        -2.0
                    } else {
                        0.0
                    };
                    if best_loyalty.is_none_or(|b| v > b.0) {
                        best_loyalty = Some((v, k));
                    }
                    continue;
                }
                Trig::Activated {
                    sorcery: _,
                    sac_self,
                    extra,
                    tap,
                    discard,
                    ..
                } => {
                    // A card discarded to pay for it is a card lost.
                    if *discard {
                        let pl = &g.players[p];
                        v -= discard_choice(g, p)
                            .map(|h| card_worth(g, p, &pl.cards[pl.hand[h] as usize]) * 0.8)
                            .unwrap_or(0.0);
                    }
                    if *tap {
                        plan_cost += srcs
                            .iter()
                            .filter(|s| s.0 == i && !s.3)
                            .map(|s| s.2 as u32)
                            .sum::<u32>();
                    }
                    if *sac_self || extra.exile_self || extra.return_self {
                        // Clues and Food when there's mana to spare; other sacrifices when worth it.
                        v -= perm_value(g, i) * 0.5;
                    }
                    if a.text.contains("pay") && a.text.contains("life") {
                        v -= 0.5;
                    }
                    // Tapping helpers and spending counters aren't free either.
                    v -= 0.3 * extra.tap_others as f32 + 0.2 * extra.remove as f32;
                }
                Trig::PowerUp(full) => {
                    // Cheapest the turn it enters: worth the mana it saves.
                    if x.entered_turn {
                        v += 0.7 * full.total().saturating_sub(cost.total()) as f32;
                    }
                }
                _ => {}
            }
            out.push((
                v - 0.1 * cost.total() as f32,
                plan_cost,
                Act::Activate(x.id, k),
            ));
        }
        if let Some((v, k)) = best_loyalty {
            out.push((v.max(0.3), 0, Act::Activate(x.id, k)));
        }
        // Equip: move gear to the best attacker.
        if x.is(T_EQUIPMENT) && (!post || x.attached.is_none()) {
            // Where it can go and what that costs ("Equip worthy {1}" only on a worthy one).
            let fits = |t: usize| g.equip_cost(p, x.id, g.perms[t].id);
            let target = equip_target(g, p, x.id)
                .filter(|&t| fits(t).is_some())
                .or_else(|| {
                    (0..g.perms.len())
                        .filter(|&t| {
                            let y = &g.perms[t];
                            y.ctrl as usize == p && y.is_creature() && fits(t).is_some()
                        })
                        .max_by_key(|&t| g.perms[t].p)
                });
            let gear = target.and_then(|t| fits(t).map(|(m, l)| (m, l, x.card, t)));
            if let Some((cost, life, r, t)) = gear {
                let tid = g.perms[t].id;
                if x.attached != Some(tid)
                    && x.attached.is_none_or(|a| g.idx(a).is_none())
                    && g.can_pay_for(p, &cost, Purpose::Ability { src: x.id })
                    && (life == 0 || g.players[p].life > life + 5)
                {
                    let bonus = match (r, x.tok) {
                        (Some(r), _) => g
                            .card(r)
                            .abilities
                            .iter()
                            .filter(|a| a.trig == Trig::Static || a.text.contains("equipped"))
                            .flat_map(|a| a.effects.iter())
                            .map(effect_value)
                            .sum::<f32>(),
                        (None, Tok::Equip(p, t)) => p as f32 + 0.5 * t as f32,
                        _ => 0.0,
                    };
                    out.push((1.0 + bonus, cost.total(), Act::Equip(x.id, tid)));
                }
            }
        }
    }
    out
}

/// An activated ability that only makes mana.
fn mana_only(a: &SimAbility) -> bool {
    matches!(a.trig, Trig::Activated { .. })
        && !a.effects.is_empty()
        && a.effects.iter().all(|e| matches!(e, Effect::AddMana(_)))
}

/// Picks the set of actions worth the most for the mana (a small knapsack); returns the
/// best one to do first.
/// Creature spells that don't help this turn's combat wait until after it (they can't attack
/// yet, and holding them hides information and keeps mana up for tricks).
fn waits_for_combat(g: &Game, p: usize, act: &Act) -> bool {
    let Act::Cast(src, opts) = act else {
        return false;
    };
    let Some((idx, back)) = g.spell_card(p, *src, *opts) else {
        return false;
    };
    let c = &g.players[g.spell_list(p, *src)].cards[idx as usize];
    if back || !c.is(T_CREATURE) || c.kw & HASTE != 0 || matches!(src, Src::Command(_)) {
        return false;
    }
    !c.abilities.iter().any(|a| {
        a.trig == Trig::Enters && a.effects.iter().any(|e| effect_value(e) >= 2.5)
            || a.trig == Trig::Static
                && a.effects.iter().any(
                    |e| matches!(e, Effect::Pump { to, .. } | Effect::Grant { to, .. } if to.mass),
                )
    })
}

/// The best set of actions for the mana (a small knapsack): the chosen options and their
/// total value. Options in `skip` are left out.
fn plan_set(options: &[(f32, u32, Act)], budget: u32, skip: &[usize]) -> Option<(Vec<usize>, f32)> {
    let items: Vec<usize> = (0..options.len())
        .filter(|&i| options[i].0 > 0.3 && !skip.contains(&i))
        .collect();
    if items.is_empty() {
        return None;
    }
    let b = budget.min(30) as usize;
    let mut dp: Vec<(f32, u64)> = vec![(0.0, 0); b + 1];
    for (k, &i) in items.iter().enumerate().take(64) {
        let (v, cost, _) = options[i];
        let cost = cost as usize;
        if cost > b {
            continue;
        }
        for m in (cost..=b).rev() {
            let cand = dp[m - cost].0 + v;
            if cand > dp[m].0 {
                dp[m] = (cand, dp[m - cost].1 | (1 << k));
            }
        }
    }
    let (total, mask) = dp[b];
    if mask == 0 {
        return None;
    }
    let chosen = items
        .iter()
        .enumerate()
        .take(64)
        .filter(|(k, _)| mask & (1 << k) != 0)
        .map(|(_, &i)| i)
        .collect();
    Some((chosen, total))
}

/// The mana an action costs (pips included), as it would be paid now.
fn act_cost(g: &Game, p: usize, act: &Act) -> Option<Mana> {
    match *act {
        Act::Cast(src, opts) => g.cast_cost(p, src, opts),
        Act::Activate(id, k) => g.idx(id).and_then(|i| g.activation_cost(p, i, k)),
        Act::ActivateGy(gk, k) => g.graveyard_activation_cost(p, gk, k),
        Act::Equip(e, t) => g.equip_cost(p, e, t).map(|c| c.0),
        Act::ActivateHand(h, k) => g.hand_activation_cost(p, h, k),
        Act::Cycle(h) => {
            let pl = &g.players[p];
            pl.hand
                .get(h)
                .and_then(|&c| pl.cards[c as usize].cycling.as_ref())
                .map(|(n, _)| Mana::generic(*n as u32))
        }
    }
}

/// Two costs paid together.
fn mana_sum(a: Mana, b: &Mana) -> Mana {
    let mut m = a;
    m.generic = m.generic.saturating_add(b.generic);
    m.colorless = m.colorless.saturating_add(b.colorless);
    for i in 0..5 {
        m.pips[i] = m.pips[i].saturating_add(b.pips[i]);
    }
    for &h in b.hybrid.iter().filter(|&&h| h != 0) {
        match m.hybrid.iter().position(|&x| x == 0) {
            Some(s) => m.hybrid[s] = h,
            None => m.generic = m.generic.saturating_add(1),
        }
    }
    m
}

/// Colours an action's cost needs (pips; a bit per colour).
fn pip_mask(m: &Mana) -> u8 {
    (0..5)
        .filter(|&i| m.pips[i] > 0)
        .fold(0u8, |a, i| a | 1 << i)
}

fn do_act(g: &mut Game, p: usize, act: Act) -> bool {
    match act {
        Act::Cast(src, opts) => g.cast(p, src, opts),
        Act::Activate(id, k) => g.activate(p, id, k),
        Act::ActivateGy(gk, k) => g.activate_from_graveyard(p, gk, k),
        Act::Equip(e, t) => g.equip(p, e, t),
        Act::Cycle(h) => g.cycle(p, h),
        Act::ActivateHand(h, k) => g.activate_from_hand(p, h, k),
    }
}

/// Mana to keep open for counterspells and instant-speed answers, if that's worth more
/// than what it would cost to tap out.
fn holdup(g: &Game, p: usize, opts: &[(f32, u32, Act)], avail: u32) -> u32 {
    let pl = &g.players[p];
    let rmhold = false;
    let best = pl
        .hand
        .iter()
        .map(|&c| (c, &pl.cards[c as usize]))
        .filter(|(_, c)| c.is(T_INSTANT) || has_flash(g, p, c))
        .map(|(idx, c)| {
            let mut v = reactive_value(g, p, c);
            // Instant removal: what it would kill now (it's cast before blocks or at the
            // end of the turn before ours, not left to rot in hand).
            if rmhold && is_instant_removal(c) {
                v = v.max(spell_value(g, p, idx, false, false));
            }
            (v, c.cost.total())
        })
        .filter(|(v, _)| *v >= 1.5)
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let Some((value, cost)) = best else { return 0 };
    if cost > avail {
        return 0;
    }
    let full = plan_set(opts, avail, &[]).map(|x| x.1).unwrap_or(0.0);
    let held = plan_set(opts, avail - cost, &[])
        .map(|x| x.1)
        .unwrap_or(0.0);
    if held + value * 0.6 >= full { cost } else { 0 }
}

/// What an action is, for the log.
fn act_name(g: &Game, p: usize, act: &Act) -> String {
    match *act {
        Act::Cast(src, opts) => {
            let Some((idx, back)) = g.spell_card(p, src, opts) else {
                return format!("{:?}", act);
            };
            let c = &g.players[g.spell_list(p, src)].cards[idx as usize];
            let name = match (&c.back, back) {
                (Some(b), true) => b.name.clone(),
                _ => c.name.clone(),
            };
            match src {
                Src::Hand(_) => name,
                Src::Command(_) => format!("{} (cmdr)", name),
                Src::Graveyard(_) => format!("{} (gy)", name),
                Src::Exile(_) => format!("{} (exile)", name),
                Src::Adventure(_) => format!("{} (adventure)", name),
                Src::Prepared(_) => format!("{} (prepared)", name),
            }
        }
        Act::Activate(id, k) => match g.idx(id) {
            Some(i) => {
                let x = &g.perms[i];
                let text: String = g
                    .abilities_of(x)
                    .get(k)
                    .map(|a| a.text.chars().take(40).collect())
                    .unwrap_or_default();
                format!("{}: {}", g.perm_name(x), text)
            }
            None => format!("{:?}", act),
        },
        Act::ActivateGy(gk, _) => g.players[p]
            .graveyard
            .get(gk)
            .map(|&c| format!("{} (gy ability)", g.players[p].cards[c as usize].name))
            .unwrap_or_else(|| format!("{:?}", act)),
        Act::Equip(e, t) => match (g.idx(e), g.idx(t)) {
            (Some(e), Some(t)) => format!(
                "equip {} -> {}",
                g.perm_name(&g.perms[e]),
                g.perm_name(&g.perms[t])
            ),
            _ => format!("{:?}", act),
        },
        Act::Cycle(h) => g.players[p]
            .hand
            .get(h)
            .map(|&c| format!("cycle {}", g.players[p].cards[c as usize].name))
            .unwrap_or_else(|| format!("{:?}", act)),
        Act::ActivateHand(h, _) => g.players[p]
            .hand
            .get(h)
            .map(|&c| format!("{} (hand ability)", g.players[p].cards[c as usize].name))
            .unwrap_or_else(|| format!("{:?}", act)),
    }
}

/// The turn's plan: the best set of actions whose colours the mana can pay, and the one to
/// do first (the most valuable). Before combat, creature spells wait for after it - unless
/// the plan needs the mana of creatures, which would otherwise attack and not make it.
fn plan_turn(
    g: &Game,
    p: usize,
    post: bool,
    opts: &[(f32, u32, Act)],
    budget: u32,
) -> Option<(usize, f32)> {
    // A creature cast with its power-up is listed twice (with and without the power-up):
    // plan with each version and keep the better plan.
    let mut twins: Vec<(usize, usize)> = Vec::new();
    for i in 1..opts.len() {
        // (The two versions are listed one after the other.)
        if let (Act::Cast(s1, o1), Act::Cast(s2, o2)) = (opts[i - 1].2, opts[i].2) {
            if s1 == s2 && o1.back == o2.back {
                twins.push((i - 1, i));
            }
        }
    }
    let (set, total) = if twins.is_empty() {
        plan_payable(g, p, opts, budget, Vec::new())?
    } else {
        let plain = plan_payable(g, p, opts, budget, twins.iter().map(|t| t.1).collect());
        let both = plan_payable(g, p, opts, budget, twins.iter().map(|t| t.0).collect());
        match (plain, both) {
            (Some(a), Some(b)) => {
                if b.1 > a.1 {
                    b
                } else {
                    a
                }
            }
            (a, b) => a.or(b)?,
        }
    };
    let creature_mana = !post && {
        let cost: u32 = set.iter().map(|&i| opts[i].1).sum();
        let other: u32 = g
            .sources(p)
            .iter()
            .filter(|s| !g.perms[s.0].is_creature())
            .map(|s| s.2 as u32)
            .sum::<u32>()
            + g.players[p].pool as u32;
        cost > other
    };
    let first = set
        .iter()
        .copied()
        .filter(|&i| post || creature_mana || !waits_for_combat(g, p, &opts[i].2))
        .max_by(|&a, &b| opts[a].0.partial_cmp(&opts[b].0).unwrap())?;
    // Colours the rest of the plan needs: generic costs are paid from other sources.
    let mut keep = set
        .iter()
        .filter(|&&i| i != first)
        .filter_map(|&i| act_cost(g, p, &opts[i].2))
        .fold(0u8, |a, m| a | pip_mask(&m));
    // A creature cast to use its power-up at once: the power-up's colours (and {C}) stay
    // untapped while the spell is paid.
    if twins.iter().any(|t| t.1 == first) {
        if let Some(r) = twin_powerup_cost(g, p, &opts[first].2) {
            keep |= pip_mask(&r) | if r.colorless > 0 { COLORLESS } else { 0 };
        }
    }
    g.ext.reserve.set(keep);
    Some((first, total))
}

/// The reduced power-up cost of the creature a cast action puts onto the battlefield.
fn twin_powerup_cost(g: &Game, p: usize, act: &Act) -> Option<Mana> {
    let Act::Cast(src, o) = *act else {
        return None;
    };
    let (idx, back) = g.spell_card(p, src, o)?;
    if back {
        return None;
    }
    let c = &g.players[g.spell_list(p, src)].cards[idx as usize];
    c.abilities.iter().find_map(|a| match &a.trig {
        Trig::PowerUp(full) => Some(powerup_cost_on_entry(g, p, c, full)),
        _ => None,
    })
}

/// `plan_set`, dropping the action worth least for its mana while the set's colours can't
/// be paid together.
fn plan_payable(
    g: &Game,
    p: usize,
    opts: &[(f32, u32, Act)],
    budget: u32,
    mut skip: Vec<usize>,
) -> Option<(Vec<usize>, f32)> {
    let fixed = skip.len();
    loop {
        let (set, total) = plan_set(opts, budget, &skip)?;
        if set.len() < 2 || skip.len() >= fixed + 3 {
            return Some((set, total));
        }
        let costs: Option<Vec<Mana>> = set.iter().map(|&i| act_cost(g, p, &opts[i].2)).collect();
        let fits = costs.is_some_and(|c| {
            let sum = c.iter().fold(Mana::default(), |a, m| mana_sum(a, m));
            g.can_pay(p, &sum)
        });
        if fits {
            return Some((set, total));
        }
        // Drop the action worth the least for its mana and plan again.
        let worst = set
            .iter()
            .copied()
            .min_by(|&a, &b| {
                let r = |i: usize| opts[i].0 / opts[i].1.max(1) as f32;
                r(a).partial_cmp(&r(b)).unwrap()
            })
            .unwrap();
        skip.push(worst);
    }
}

/// What spending `k` of `p`'s mana now (in combat, on a trigger or a tax) costs the rest of
/// the turn: the post-combat plan's value with all the mana, minus with `k` less. (Not our
/// turn, or nothing left to cast: nothing.)
pub fn post_combat_loss(g: &Game, p: usize, k: u32) -> f32 {
    if k == 0 || g.active != p {
        return 0.0;
    }
    let avail = g.mana_available(p);
    if k > avail {
        return 1000.0;
    }
    let opts = options(g, p, true);
    let worth = |b: u32| plan_payable(g, p, &opts, b, Vec::new()).map_or(0.0, |x| x.1);
    let full = worth(avail);
    if full <= 0.0 {
        return 0.0;
    }
    (full - worth(avail - k)).max(0.0)
}

/// How many of these optional paid modes (their values, best first; `n` mana each) to pay
/// for: the count whose total beats what that mana would have cast later in the turn.
pub fn modes_worth_paying(g: &Game, p: usize, vals: &[f32], n: u32) -> usize {
    if g.active != p {
        return vals.len();
    }
    let avail = g.mana_available(p);
    let opts = options(g, p, true);
    let worth = |b: u32| plan_payable(g, p, &opts, b, Vec::new()).map_or(0.0, |x| x.1);
    let full = worth(avail);
    let mut best = (0.0f32, 0usize);
    let mut sum = 0.0;
    for (k, v) in vals.iter().enumerate() {
        let paid = n * (k as u32 + 1);
        if paid > avail {
            break;
        }
        sum += v;
        let net = sum - (full - worth(avail - paid)).max(0.0);
        if net > best.0 {
            best = (net, k + 1);
        }
    }
    best.1
}

/// A main phase: land, then the best plan for the mana, re-planned after each action.
pub fn main_phase(g: &mut Game, p: usize, post: bool) {
    main_phase_inner(g, p, post);
    g.ext.reserve.set(0);
    // Mana empties from the pool between steps and phases.
    g.players[p].pool = 0;
}

fn main_phase_inner(g: &mut Game, p: usize, post: bool) {
    let mut failed: Vec<String> = Vec::new();
    for _ in 0..30 {
        if !g.players[p].alive || g.alive_count() <= 1 {
            return;
        }
        if g.players[p].lands_played < g.land_drops(p) {
            // Lands from the graveyard (Mole Man, Muldrotha) or exile first: they're free
            // cards; the hand's lands wait.
            if let Some(k) = other_zone_land(g, p) {
                g.play_land_from(p, k);
                continue;
            }
            if let Some(h) = choose_land(g, p) {
                g.play_land(p, h);
                continue;
            }
        }
        if !post && g.stats.is_some() {
            g.audit_mana_avail(p);
        }
        let mut opts: Vec<(f32, u32, Act)> = options(g, p, post);
        if !failed.is_empty() {
            opts.retain(|o| !failed.contains(&format!("{:?}", o.2)));
        }
        if opts.is_empty() {
            return;
        }
        let avail = g.mana_available(p);
        // (Combat AI: mana for the attack taxes our planned swing pays.)
        let tax_keep = if post { 0 } else { attack_tax_reserve(g, p) };
        let reserve = holdup(g, p, &opts, avail) + tax_keep;
        if reserve > 0 {
            g.note(|| format!("AI P{} holds up {} mana", p, reserve));
        }
        // The whole turn is planned together; before combat, only its pre-combat parts happen.
        let budget = avail.saturating_sub(reserve);
        let pick = plan_turn(g, p, post, &opts, budget);
        if g.log.is_some() {
            let mut order: Vec<usize> = (0..opts.len()).collect();
            order.sort_by(|&a, &b| opts[b].0.partial_cmp(&opts[a].0).unwrap());
            let list: Vec<String> = order
                .iter()
                .take(8)
                .map(|&k| {
                    format!(
                        "{} v{:.1} c{}",
                        act_name(g, p, &opts[k].2),
                        opts[k].0,
                        opts[k].1
                    )
                })
                .collect();
            let more = opts.len().saturating_sub(8);
            let chosen = match pick {
                Some((i, total)) => {
                    format!("{} (plan worth {:.1})", act_name(g, p, &opts[i].2), total)
                }
                None if post => "nothing (no plan worth it)".into(),
                None => "nothing before combat".into(),
            };
            g.note(|| {
                format!(
                    "AI P{} {} options ({} mana{}): {}{} -> {}",
                    p,
                    if post { "post-combat" } else { "main" },
                    avail,
                    if reserve > 0 {
                        format!(", holding {}", reserve)
                    } else {
                        String::new()
                    },
                    list.join(" | "),
                    if more > 0 {
                        format!(" | +{} more", more)
                    } else {
                        String::new()
                    },
                    chosen
                )
            });
        }
        let Some((i, _)) = pick else {
            return;
        };
        let act = opts[i].2;
        let ok = do_act(g, p, act);
        g.ext.reserve.set(0);
        if !ok {
            g.note(|| format!("   P{} couldn't do {:?}", p, act));
        }
        if !ok {
            failed.push(format!("{:?}", act));
        }
    }
}

// ------------------------------------------------------------ instant speed

/// Casts an instant from hand aimed at `focus` (if any). True if it was cast.
fn cast_instant(g: &mut Game, p: usize, h: usize, focus: Option<u32>, why: &str) -> bool {
    if g.log.is_some() {
        let name = g.players[p].cards[g.players[p].hand[h] as usize]
            .name
            .clone();
        let at = focus
            .map(|id| format!(" at {}", g.id_desc(id)))
            .unwrap_or_default();
        g.note(|| format!("AI P{} instant-speed {}{} ({})", p, name, at, why));
    }
    g.focus = focus;
    let ok = g.cast(p, Src::Hand(h), CastOpts::default());
    g.focus = None;
    ok
}

fn hand_find(g: &Game, p: usize, f: impl Fn(&SimCard) -> bool) -> Option<usize> {
    let pl = &g.players[p];
    (0..pl.hand.len()).find(|&h| {
        let c = &pl.cards[pl.hand[h] as usize];
        f(c) && g.can_cast(p, Src::Hand(h), CastOpts::default())
    })
}

/// How bad it would be for `o` if the spell resolves.
fn spell_threat(g: &Game, o: usize, caster: usize, card: CardRef, back: bool) -> f32 {
    let c = g.card(card);
    let w = weight(g, o, caster);
    if !back && c.is_permanent() {
        return c.value * w * 0.8;
    }
    let mut v = 0.0;
    for e in effects_of(c, back) {
        match e {
            Effect::Destroy(t) | Effect::Exile { target: t, .. } if t.mass => {
                let mine: f32 = (0..g.perms.len())
                    .filter(|&i| g.perms[i].ctrl as usize == o && g.perms[i].is_creature())
                    .map(|i| perm_value(g, i))
                    .sum();
                v += mine;
            }
            Effect::Destroy(t) | Effect::Exile { target: t, .. } | Effect::Bounce(t)
                if t.hits_their_creatures() =>
            {
                // Would it hit my best thing?
                let best = (0..g.perms.len())
                    .filter(|&i| g.perms[i].ctrl as usize != caster)
                    .max_by(|&a, &b| perm_value(g, a).partial_cmp(&perm_value(g, b)).unwrap());
                if best.is_some_and(|i| g.perms[i].ctrl as usize == o) {
                    v += best.map(|i| perm_value(g, i)).unwrap_or(0.0);
                }
            }
            Effect::Draw { amount, .. } => v += amount.n as f32 * 0.8 * w,
            Effect::ExtraCombat | Effect::GainControl(_) | Effect::Reanimate => v += 4.0 * w,
            e => v += effect_value(e) * 0.5 * w,
        }
    }
    v
}

/// "Counter target spell unless its controller pays {N}": N (None for a hard counter).
fn soft_counter_tax(text: &str) -> Option<u32> {
    let i = text.find("unless its controller pays {")?;
    text[i + 28..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
}

/// Opponents may counter a spell `caster` just cast. True if it was countered.
/// Opponents may counter the spell `card` (whose card list it's in, index) `caster` cast.
pub fn respond_to_spell(g: &mut Game, caster: usize, card: CardRef, back: bool) -> bool {
    let n = g.players.len();
    let spell = g.card(card);
    let spell_types = if back {
        spell.back.as_ref().map(|b| b.types).unwrap_or(T_SORCERY)
    } else {
        spell.types
    };
    for k in 1..n {
        let o = (caster + k) % n;
        if !g.players[o].alive {
            continue;
        }
        let Some(h) = hand_find(g, o, |c| {
            if !is_counter(c) {
                return false;
            }
            let t = c
                .abilities
                .iter()
                .find(|a| a.effects.contains(&Effect::CounterSpell))
                .map(|a| a.text.as_str())
                .unwrap_or("");
            if t.contains("noncreature spell") && spell_types & T_CREATURE != 0 {
                return false;
            }
            if t.contains("creature spell")
                && !t.contains("noncreature")
                && spell_types & T_CREATURE == 0
            {
                return false;
            }
            if t.contains("instant or sorcery spell") && spell_types & (T_INSTANT | T_SORCERY) == 0
            {
                return false;
            }
            !t.contains("activated or triggered ability") || t.contains("spell")
        }) else {
            continue;
        };
        let counters_in_hand = g.players[o]
            .hand
            .iter()
            .filter(|&&c| is_counter(&g.players[o].cards[c as usize]))
            .count();
        let mut threshold = if counters_in_hand > 1 { 3.5 } else { 4.5 };
        // "Counter target spell unless its controller pays {N}" (N 4 with teamwork when
        // there are creatures to tap): useless against a caster who can still pay.
        let (unless, teamwork) = {
            let c = &g.players[o].cards[g.players[o].hand[h] as usize];
            let text = c
                .abilities
                .iter()
                .find(|a| a.effects.contains(&Effect::CounterSpell))
                .map(|a| a.text.as_str())
                .unwrap_or("");
            let n = soft_counter_tax(text);
            let tw = c.teamwork.filter(|_| text.contains("cast using teamwork"));
            match (n, tw) {
                (Some(_), Some(k)) if !teamwork_tappers(g, o, k as i16).is_empty() => {
                    (Some(4), Some(k))
                }
                (n, _) => (n, None),
            }
        };
        if unless.is_some_and(|n| g.mana_available(caster) >= n) {
            g.note(|| {
                format!(
                    "  AI P{} holds its counter: P{} could pay for it",
                    o, caster
                )
            });
            continue;
        }
        // A lone counter is kept for something bigger than a cheap spell.
        if counters_in_hand <= 1 && g.card(card).mv <= 2 {
            threshold += 1.0;
        }
        let threat_v = spell_threat(g, o, caster, card, back);
        if g.log.is_some() {
            let cname = g.players[o].cards[g.players[o].hand[h] as usize]
                .name
                .clone();
            let verdict = if threat_v < threshold {
                "lets it resolve"
            } else {
                "counters"
            };
            g.note(|| {
                format!(
                    "  AI P{} holds {}: spell threat {:.1} vs threshold {:.1} -> {}",
                    o, cname, threat_v, threshold, verdict
                )
            });
        }
        if threat_v < threshold {
            continue;
        }
        let counter_idx = g.players[o].hand[h];
        let counter = &g.players[o].cards[counter_idx as usize];
        let text = counter
            .abilities
            .iter()
            .find(|a| a.effects.contains(&Effect::CounterSpell))
            .map(|a| a.text.clone())
            .unwrap_or_default();
        let Some(cost) = g.cast_cost(o, Src::Hand(h), CastOpts::default()) else {
            continue;
        };
        let why = Purpose::Spell {
            card: (o as u8, counter_idx),
            from_hand: true,
        };
        if !g.pay_for(o, &cost, why) {
            continue;
        }
        // Teamwork: tap the helpers (the tax goes up to {4}).
        if let Some(k) = teamwork {
            for id in teamwork_tappers(g, o, k as i16) {
                if let Some(i) = g.idx(id) {
                    g.perms[i].tapped = true;
                    let snap = g.perms[i].clone();
                    g.emit_about(Ev::TeamworkTapped, o, &snap);
                    g.emit(Ev::Tapped, o, Some(id));
                }
            }
        }
        g.players[o].hand.remove(h);
        g.players[o].graveyard.push(counter_idx);
        // "If that spell's mana value was 2 or less, recruit" (Sound the Trumpets).
        g.ext.countered_mv = Some(if back {
            spell
                .back
                .as_ref()
                .map_or(spell.mv, |b| b.cost.total() as u8)
        } else {
            spell.mv
        });
        g.on_cast(o, counter);
        // Other effects on the counterspell (draw a card, etc.).
        for (k2, a) in counter.abilities.iter().enumerate() {
            // A modal counterspell used its counter mode.
            if a.modal && a.effects.contains(&Effect::CounterSpell) {
                continue;
            }
            if a.trig == Trig::Spell && a.effects.iter().any(|e| *e != Effect::CounterSpell) {
                g.pending.push(Pending {
                    ctrl: o as u8,
                    source: None,
                    card: (o as u8, counter_idx),
                    back: false,
                    ability: k2 as u16,
                    snapshot: None,
                    subject: None,
                    teamwork: false,
                    gift: false,
                    from_graveyard: false,
                    kicked: false,
                    overloaded: false,
                    x: None,
                });
            }
        }
        g.note(|| format!("AI P{} casts {} to counter it", o, counter.name));
        let mut paid_off = false;
        // "Unless its controller pays {N}".
        if let Some(i) = text.find("pays {") {
            let n: u32 = text[i + 6..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .unwrap_or(2);
            let n = if teamwork.is_some() { 4 } else { n };
            paid_off = g.pay(caster, &Mana::generic(n));
            g.note(|| {
                format!(
                    "  P{} {} {{{}}}",
                    caster,
                    if paid_off { "pays" } else { "can't pay" },
                    n
                )
            });
        }
        if g.stats.is_some() {
            let opps = g.opponents(o);
            let tc = threat_peek(g, caster);
            let leader = opps.iter().all(|&q| threat_peek(g, q) <= tc);
            let mv = g.ext.countered_mv.unwrap_or(0);
            if let Some(s) = g.stats.as_mut() {
                s.players[o].counters.push(super::audit::CounterUse {
                    caster: caster as u8,
                    mv,
                    permanent: !back && spell.is_permanent(),
                    caster_threat_leader: leader,
                    paid_off,
                });
            }
        }
        if paid_off {
            return false;
        }
        return true;
    }
    false
}

/// The owner of a targeted permanent answers with a protection spell if it's worth it.
pub fn protect(g: &mut Game, owner: usize, id: u32) -> bool {
    let Some(i) = g.idx(id) else { return false };
    if perm_value(g, i) < 3.5 {
        return false;
    }
    let Some(h) = hand_find(g, owner, is_protection) else {
        return false;
    };
    cast_instant(g, owner, h, Some(id), "protect a targeted permanent")
}

/// Defender's window after attacks are declared: Fog a lethal swing, kill a dangerous
/// attacker, flash in a blocker.
pub fn before_blocks(g: &mut Game, d: usize, attacks: &[Attack]) {
    let incoming: Vec<(u32, i16)> = attacks
        .iter()
        .filter(|at| g.def_player(at.def) == Some(d))
        .filter_map(|at| {
            g.idx(at.attacker).map(|i| {
                (
                    at.attacker,
                    g.perms[i].p * if g.perms[i].has(DOUBLE_STRIKE) { 2 } else { 1 },
                )
            })
        })
        .collect();
    let total: i32 = incoming.iter().map(|x| x.1.max(0) as i32).sum();
    if total == 0 {
        return;
    }
    let blockers = g
        .perms
        .iter()
        .filter(|x| x.ctrl as usize == d && x.is_creature() && !x.tapped)
        .count();
    let life = g.players[d].life;
    if total >= life || (total * 2 >= life && blockers == 0) {
        if let Some(h) = hand_find(g, d, is_fog) {
            cast_instant(g, d, h, None, "fog: the attack would be lethal or near it");
            return;
        }
    }
    // Instant removal on the attacker whose death helps most: its value plus how much
    // better our blocks get without it (a lethal swing counts for a lot). Only an attacker
    // the spell really takes out of the fight (3 damage doesn't stop a 6/6).
    let cards = removal_in_hand(g, d);
    if !cards.is_empty() {
        let mine: Vec<Attack> = attacks
            .iter()
            .filter(|at| g.def_player(at.def) == Some(d))
            .cloned()
            .collect();
        let base = plan_blocks(g, d, &mine, false, None).map_or(0.0, |x| x.0);
        let mut best: Option<(f32, usize, u32)> = None;
        for at in &mine {
            let Some(i) = g.idx(at.attacker) else {
                continue;
            };
            // Only at a target worth the card, unless the swing takes a third of our life.
            if !worth_removing(g, d, i) && total * 3 < life {
                continue;
            }
            let pl = &g.players[d];
            let Some(&h) = cards
                .iter()
                .find(|&&h| removal_kills(g, d, &pl.cards[pl.hand[h] as usize], i))
            else {
                continue;
            };
            let rest: Vec<Attack> = mine
                .iter()
                .filter(|x| x.attacker != at.attacker)
                .cloned()
                .collect();
            let without = plan_blocks(g, d, &rest, false, None).map_or(0.0, |x| x.0);
            let gain = without - base + perm_value(g, i);
            if best.is_none_or(|b| gain > b.0) {
                best = Some((gain, h, at.attacker));
            }
        }
        if let Some((gain, h, id)) = best {
            if gain >= 4.5 {
                cast_instant(g, d, h, Some(id), "removal on the attacker that hurts most");
            }
        }
    }
    // Flash creatures as surprise blockers.
    if let Some(h) = hand_find(g, d, |c| c.is(T_CREATURE) && has_flash(g, d, c)) {
        g.note(|| format!("AI P{} flashes in a surprise blocker", d));
        g.cast(d, Src::Hand(h), CastOpts::default());
    }
}

/// The instant removal `p` can cast now: hand slots, the least valuable card first (the
/// one to spend when several do the job).
fn removal_in_hand(g: &Game, p: usize) -> Vec<usize> {
    let pl = &g.players[p];
    let mut v: Vec<usize> = (0..pl.hand.len())
        .filter(|&h| {
            is_instant_removal(&pl.cards[pl.hand[h] as usize])
                && g.can_cast(p, Src::Hand(h), CastOpts::default())
        })
        .collect();
    v.sort_by(|&x, &y| {
        let c = |h: usize| pl.cards[pl.hand[h] as usize].value;
        c(x).partial_cmp(&c(y)).unwrap()
    });
    v
}

/// After blocks: instant removal where it swings a fight or stops the most damage - the
/// blocker that would kill our attacker, the attacker that would kill our blocker or hurt
/// us most (a lethal hit counts in full). Only on creatures the spell really removes, and
/// only when that's worth more than keeping the card. True if it cast one.
fn combat_removal(g: &mut Game, p: usize, attacks: &[Attack]) -> bool {
    let cards = removal_in_hand(g, p);
    if cards.is_empty() {
        return false;
    }
    // Damage coming at us now, and what it costs.
    let attacker_ctrl = attacks
        .first()
        .and_then(|at| g.idx(at.attacker))
        .map_or(p, |i| g.perms[i].ctrl as usize);
    let danger = danger_before_turn(g, p, attacker_ctrl);
    let through_me: Vec<(usize, i32)> = attacks
        .iter()
        .filter(|at| g.def_player(at.def) == Some(p))
        .filter_map(|at| {
            let a = g.idx(at.attacker)?;
            let bl: Vec<usize> = at.blockers.iter().filter_map(|b| g.idx(*b)).collect();
            Some((
                a,
                if at.blocked && bl.is_empty() {
                    0
                } else {
                    clash(g, a, &bl).through
                },
            ))
        })
        .collect();
    let life = g.players[p].life;
    let total: i32 = through_me.iter().map(|x| x.1).sum();
    let mut best: Option<(f32, usize, u32)> = None;
    let consider = |g: &Game, gain: f32, target: usize, best: &mut Option<(f32, usize, u32)>| {
        // A target below the floor only when the fight swings a lot on it (a creature of
        // ours saved, much damage kept off us).
        if gain < 6.0 && !worth_removing(g, p, target) {
            return;
        }
        let pl = &g.players[p];
        if let Some(&h) = cards
            .iter()
            .find(|&&h| removal_kills(g, p, &pl.cards[pl.hand[h] as usize], target))
        {
            if best.is_none_or(|b| gain > b.0) {
                *best = Some((gain, h, g.perms[target].id));
            }
        }
    };
    for at in attacks {
        let Some(a) = g.idx(at.attacker) else {
            continue;
        };
        let bl: Vec<usize> = at.blockers.iter().filter_map(|b| g.idx(*b)).collect();
        if g.perms[a].ctrl as usize == p {
            // Our attacker: kill a blocker that would kill it (or that it can't get past).
            if bl.is_empty() {
                continue;
            }
            let c = clash(g, a, &bl);
            for &b in &bl {
                let rest: Vec<usize> = bl.iter().copied().filter(|&x| x != b).collect();
                let c2 = clash(g, a, &rest);
                let mut gain = if c.dead.contains(&b) {
                    0.0
                } else {
                    perm_value(g, b)
                };
                if c.att_dies && !c2.att_dies {
                    gain += perm_value(g, a);
                }
                gain += c2
                    .dead
                    .iter()
                    .filter(|x| !c.dead.contains(x))
                    .map(|&x| perm_value(g, x))
                    .sum::<f32>();
                consider(g, gain, b, &mut best);
            }
        } else if g.def_player(at.def) == Some(p) {
            // An attacker on us: it dies, our blockers live, its damage stays off us.
            let c = clash(g, a, &bl);
            let mut gain = if c.att_dies { 0.0 } else { perm_value(g, a) };
            gain += c.dead.iter().map(|&b| perm_value(g, b)).sum::<f32>();
            if c.through > 0 {
                gain += life_cost(life, life - total, danger)
                    - life_cost(life, life - total + c.through, danger);
            }
            consider(g, gain, a, &mut best);
        }
    }
    match best {
        Some((gain, h, id)) if gain >= 4.0 => {
            cast_instant(g, p, h, Some(id), "removal after blocks: swings the fight")
        }
        _ => false,
    }
}

/// Would instant removal `c`, cast by `d`, take creature `i` out of the fight? Damage and
/// -N/-N must be enough for its toughness; a fight needs a creature of ours that can kill it.
fn removal_kills(g: &Game, d: usize, c: &SimCard, i: usize) -> bool {
    let x = &g.perms[i];
    let hp = (x.t - x.damage) as i32;
    let tough = x.has(INDESTRUCTIBLE);
    effects_of(c, false).any(|e| match e {
        Effect::Destroy(t) => t.hits_their_creatures() && !t.mass && !tough,
        Effect::Exile { target: t, .. } | Effect::Bounce(t) => t.hits_their_creatures() && !t.mass,
        Effect::Damage { amount, to } => {
            to.hits_their_creatures()
                && !to.mass
                && !tough
                && !x.has(NO_DAMAGE)
                && (amount.x || amount.n >= hp)
        }
        Effect::Shrink { amount, to, .. } => {
            to.hits_their_creatures() && !to.mass && (amount.x || amount.n >= x.t as i32)
        }
        Effect::Fight(_) => {
            !tough
                && !x.has(NO_DAMAGE)
                && g.perms.iter().any(|y| {
                    y.ctrl as usize == d
                        && y.is_creature()
                        && (g.combat_power(y) as i32 >= hp || y.has(DEATHTOUCH))
                })
        }
        _ => false,
    })
}

/// End of another player's turn: use mana that would otherwise go to waste.
pub fn end_of_turn(g: &mut Game, p: usize, next: bool) {
    if !next {
        return;
    }
    if g.log.is_some() && std::env::var("MSH_SIM_DEBUG").is_ok() {
        let pl = &g.players[p];
        let inst: Vec<String> = pl
            .hand
            .iter()
            .map(|&c| &pl.cards[c as usize])
            .filter(|c| c.is(T_INSTANT) || has_flash(g, p, c))
            .map(|c| c.name.clone())
            .collect();
        let mana = g.mana_available(p);
        g.note(|| {
            format!(
                "   P{} end of turn: {} mana, instant-speed cards: {}",
                p,
                mana,
                inst.join(", ")
            )
        });
    }
    for _ in 0..4 {
        // Flash creatures and instants that do something now.
        let pl = &g.players[p];
        let mut best: Option<(f32, usize)> = None;
        for h in 0..pl.hand.len() {
            let c = &pl.cards[pl.hand[h] as usize];
            let instant_speed = c.is(T_INSTANT) || has_flash(g, p, c);
            if !instant_speed || is_reactive_only(c) {
                continue;
            }
            if !g.can_cast(p, Src::Hand(h), CastOpts::default()) {
                continue;
            }
            let v = spell_value(g, p, pl.hand[h], false, false);
            if v > 1.0 && best.is_none_or(|b| v > b.0) {
                best = Some((v, h));
            }
        }
        if let Some((_, h)) = best {
            g.note(|| format!("AI P{} end-of-turn play", p));
            if g.cast(p, Src::Hand(h), CastOpts::default()) {
                continue;
            }
        }
        // Instant-speed abilities: card draw, Clues, mana sinks.
        let mut acted = false;
        for i in 0..g.perms.len() {
            if i >= g.perms.len() || g.perms[i].ctrl as usize != p {
                continue;
            }
            let x = g.perms[i].clone();
            for (k, a) in g.abilities_of(&x).iter().enumerate() {
                if !matches!(a.trig, Trig::Activated { sorcery: false, .. }) || mana_only(a) {
                    continue;
                }
                let Some(cost) = g.activation_cost(p, i, k) else {
                    continue;
                };
                if !g.can_pay_for(p, &cost, Purpose::Ability { src: x.id }) {
                    continue;
                }
                let v = ability_now(g, p, a, Some(x.id))
                    - if matches!(a.trig, Trig::Activated { sac_self: true, .. }) {
                        perm_value(g, i) * 0.5
                    } else {
                        0.0
                    };
                if v > 0.6 && g.activate(p, x.id, k) {
                    acted = true;
                    break;
                }
            }
            if acted {
                break;
            }
        }
        if !acted {
            return;
        }
    }
}

/// Creatures to tap for Teamwork: total power N from creatures that aren't needed.
pub fn teamwork_tappers(g: &Game, p: usize, n: i16) -> Vec<u32> {
    let mut c: Vec<&Perm> = g
        .perms
        .iter()
        .filter(|x| x.ctrl as usize == p && x.is_creature() && !x.tapped)
        .collect();
    c.sort_by_key(|x| (!x.sick, x.p + x.t));
    let mut out = Vec::new();
    let mut total = 0;
    for x in c {
        if total >= n {
            break;
        }
        out.push(x.id);
        total += x.p.max(0);
    }
    if total >= n { out } else { Vec::new() }
}

pub fn crew_tappers(g: &Game, p: usize, need: i16, vehicle_id: u32) -> Vec<u32> {
    let mut c: Vec<&Perm> = g
        .perms
        .iter()
        .filter(|x| {
            x.ctrl as usize == p && x.is_creature() && !x.tapped && x.id != vehicle_id && !x.crewed
        })
        .collect();
    // Summoning-sick and small creatures crew first.
    c.sort_by_key(|x| (!x.sick, x.p));
    let mut out = Vec::new();
    let mut total = 0;
    for x in c {
        if total >= need {
            break;
        }
        out.push(x.id);
        total += x.p.max(0);
    }
    if total >= need { out } else { Vec::new() }
}

pub fn crew_for_combat(g: &mut Game, a: usize) {
    let vehicles: Vec<u32> = g
        .perms
        .iter()
        .filter(|x| {
            // (A Vehicle that arrived this turn can't attack even crewed.)
            x.ctrl as usize == a
                && x.is(T_VEHICLE)
                && !x.crewed
                && !x.tapped
                && !(x.sick && !x.has(HASTE))
                && x.base_p >= 3
        })
        .map(|x| x.id)
        .collect();
    for v in vehicles {
        let Some(vi) = g.idx(v) else { continue };
        let need = g.perms[vi]
            .card
            .map(|r| g.card(r).crew.unwrap_or(2))
            .unwrap_or(2) as i16;
        let helpers = crew_tappers(g, a, need, v);
        // Crew only with creatures that would attack for less.
        let helper_power: i16 = helpers
            .iter()
            .filter_map(|id| g.idx(*id))
            .filter(|&i| !g.perms[i].sick)
            .map(|i| g.perms[i].p)
            .sum();
        if !helpers.is_empty() && helper_power < g.perms[vi].base_p {
            g.crew(a, v);
        }
    }
}

// ------------------------------------------------------------ combat

/// (attacker dies, blocker dies) if `a` attacks into `b` (no tricks).
pub(crate) fn fight(a: &Perm, b: &Perm) -> (bool, bool) {
    let a_first = a.has(FIRST_STRIKE | DOUBLE_STRIKE) && !b.has(FIRST_STRIKE | DOUBLE_STRIKE);
    let b_first = b.has(FIRST_STRIKE | DOUBLE_STRIKE) && !a.has(FIRST_STRIKE | DOUBLE_STRIKE);
    let kills = |s: &Perm, t: &Perm| {
        s.p > 0
            && !t.has(INDESTRUCTIBLE | NO_DAMAGE)
            && (s.has(DEATHTOUCH) || s.p >= t.t - t.damage)
    };
    let a_kills = kills(a, b);
    let b_kills = kills(b, a);
    let blocker_dies = a_kills && !(b_first && b_kills);
    let attacker_dies = b_kills && !(a_first && a_kills);
    (attacker_dies, blocker_dies)
}

/// Blocker `b` survives attacker `a` even after a small pump (a +2 trick), or can't be hurt.
fn safe_margin(a: &Perm, b: &Perm) -> bool {
    b.t - b.damage - a.p >= 2 || b.has(FIRST_STRIKE | DEATHTOUCH | INDESTRUCTIBLE | NO_DAMAGE)
}

fn dmg_of(g: &Game, x: &Perm) -> i32 {
    g.combat_power(x).max(0) as i32 * if x.has(DOUBLE_STRIKE) { 2 } else { 1 }
}

/// What happens when attacker `a` is blocked by `bl` (no tricks): whether the attacker dies,
/// which blockers die (the attacker assigns its damage to kill the most valuable blockers it
/// can, first strike and double strike included), and the damage that tramples over.
/// Unblocked, all of its damage goes through.
pub(crate) struct Clash {
    pub att_dies: bool,
    pub dead: Vec<usize>,
    pub through: i32,
}

pub(crate) fn clash(g: &Game, a: usize, bl: &[usize]) -> Clash {
    let at = &g.perms[a];
    let total = dmg_of(g, at);
    if bl.is_empty() {
        return Clash {
            att_dies: false,
            dead: Vec::new(),
            through: total,
        };
    }
    let first = |x: &Perm| x.has(FIRST_STRIKE | DOUBLE_STRIKE);
    let immune = |x: &Perm| x.has(INDESTRUCTIBLE | NO_DAMAGE);
    let pw = |x: &Perm| g.combat_power(x).max(0) as i32;
    let a_hp = (at.t - at.damage).max(1) as i32;
    let need = |x: &Perm| {
        if at.has(DEATHTOUCH) {
            1
        } else {
            (x.t - x.damage).max(1) as i32
        }
    };
    // Blockers that strike first may kill the attacker before it deals any damage.
    if !first(at) && !immune(at) {
        let (sum, dt) = bl
            .iter()
            .map(|&b| &g.perms[b])
            .filter(|x| first(x) && pw(x) > 0)
            .fold((0, false), |(s, d), x| (s + pw(x), d || x.has(DEATHTOUCH)));
        if sum >= a_hp || dt {
            return Clash {
                att_dies: true,
                dead: Vec::new(),
                through: 0,
            };
        }
    }
    // The attacker kills the most valuable blockers it can.
    let sorted: Vec<usize>;
    let order: &[usize] = if bl.len() > 1 {
        let mut v = bl.to_vec();
        v.sort_by(|&x, &y| perm_value(g, y).partial_cmp(&perm_value(g, x)).unwrap());
        sorted = v;
        &sorted
    } else {
        bl
    };
    let mut budget = total;
    let mut first_hit = if first(at) { pw(at) } else { 0 };
    let mut dead = Vec::new();
    let mut silenced = Vec::new();
    for &b in order {
        let x = &g.perms[b];
        let n = need(x);
        if n <= budget {
            budget -= n;
            if !immune(x) {
                dead.push(b);
                // Killed in the first-strike step: it never hits back.
                if n <= first_hit && !first(x) {
                    first_hit -= n;
                    silenced.push(b);
                }
            }
        }
    }
    let through = if at.has(TRAMPLE) {
        (total - bl.iter().map(|&b| need(&g.perms[b])).sum::<i32>()).max(0)
    } else {
        0
    };
    let (sum, dt) = bl
        .iter()
        .filter(|b| !silenced.contains(b))
        .map(|&b| &g.perms[b])
        .fold((0, false), |(s, d), x| {
            let p = pw(x) * if x.has(DOUBLE_STRIKE) { 2 } else { 1 };
            (s + p, d || (x.has(DEATHTOUCH) && p > 0))
        });
    Clash {
        att_dies: !immune(at) && (sum >= a_hp || dt),
        dead,
        through,
    }
}

/// The order in which attacker `a` deals damage to its blockers `bl`: the most valuable
/// blockers it can kill first (as `clash` predicts), then the rest.
pub fn damage_order(g: &Game, a: usize, bl: &[usize]) -> Vec<usize> {
    let mut order = clash(g, a, bl).dead;
    let mut rest: Vec<usize> = bl.iter().copied().filter(|b| !order.contains(b)).collect();
    rest.sort_by(|&x, &y| perm_value(g, y).partial_cmp(&perm_value(g, x)).unwrap());
    order.extend(rest);
    order
}

/// Damage that may come at `d` from the opponents who act between the active player `active`
/// and `d`'s next turn (most of each one's power: they may well pick us).
fn danger_before_turn(g: &Game, d: usize, active: usize) -> i32 {
    let n = g.players.len();
    let mut sum = 0;
    let mut o = (active + 1) % n;
    while o != d && o != active {
        if g.players[o].alive {
            sum += g
                .perms
                .iter()
                .filter(|x| x.ctrl as usize == o && x.is_creature() && !x.has(DEFENDER))
                .map(|x| dmg_of(g, x))
                .sum::<i32>();
        }
        o = (o + 1) % n;
    }
    sum * 4 / 5
}

/// The cost of going from `life` down to `after`: life is cheap while it's plentiful and
/// dear once the next round's damage could finish us (`danger`).
fn life_cost(life: i32, after: i32, danger: i32) -> f32 {
    if after <= 0 {
        return 1000.0 + (life - after) as f32;
    }
    let t = danger.max(3);
    // Points lost within (lo, hi].
    let lost = |lo: i32, hi: i32| (hi.min(life) - lo.max(after)).max(0) as f32;
    1.5 * lost(0, t) + 0.5 * lost(t, 2 * t) + 0.15 * lost(2 * t, i32::MAX)
}

/// A block assignment being planned: (blocker, attacker) perm indices, and the reasons.
#[derive(Clone, Default)]
struct BlockPlan {
    pairs: Vec<(usize, usize)>,
    why: Vec<String>,
}

impl BlockPlan {
    fn used(&self, b: usize) -> bool {
        self.pairs.iter().any(|p| p.0 == b)
    }
    fn blocked(&self, a: usize) -> bool {
        self.pairs.iter().any(|p| p.1 == a)
    }
}

/// One to four blockers for one attacker.
#[derive(Clone, Copy)]
struct Pick {
    b: [usize; 4],
    n: usize,
}

impl Pick {
    fn list(&self) -> &[usize] {
        &self.b[..self.n]
    }
}

/// Everything the blocking AI needs about one combat.
struct Blocking<'g, 'a> {
    g: &'g Game<'a>,
    d: usize,
    /// Attackers coming at `d`, most valuable first.
    atts: Vec<usize>,
    blockers: Vec<usize>,
    /// Which blocker can block which attacker (`atts` x `blockers`, row by row).
    able: Vec<bool>,
    /// The attacker has mana and cards: maybe a trick.
    wary: bool,
    danger: i32,
    log: bool,
}

impl Blocking<'_, '_> {
    fn name(&self, i: usize) -> String {
        self.g.perm_name(&self.g.perms[i])
    }

    fn can(&self, plan: &BlockPlan, a: usize) -> Vec<usize> {
        let nb = self.blockers.len();
        let row = self.atts.iter().position(|&x| x == a).unwrap_or(0) * nb;
        self.blockers
            .iter()
            .enumerate()
            .filter(|&(j, &b)| self.able[row + j] && !plan.used(b))
            .map(|(_, &b)| b)
            .collect()
    }

    /// What attacker `a` blocked by `bl` does: (creature value won minus lost, damage to
    /// us, poison, lethal commander damage).
    fn term(&self, a: usize, bl: &[usize]) -> (f32, i32, i32, bool) {
        let g = self.g;
        let x = &g.perms[a];
        let c = if x.has(MENACE) && bl.len() == 1 {
            clash(g, a, &[])
        } else {
            clash(g, a, bl)
        };
        let mut value = if c.att_dies { perm_value(g, a) } else { 0.0 };
        value -= c.dead.iter().map(|&b| perm_value(g, b)).sum::<f32>();
        if c.through == 0 {
            return (value, 0, 0, false);
        }
        let pl = &g.players[self.d];
        let poison = x.card.map_or(0, |r| g.card(r).toxic as i32);
        // (Commander damage is counted for the commander's owner.)
        let cmd = x.commander > 0
            && pl.cmd_damage[x.owner as usize % 8][x.commander as usize % 3] + c.through >= 21;
        (value, c.through, poison, cmd)
    }

    fn terms(&self, plan: &BlockPlan) -> Vec<(f32, i32, i32, bool)> {
        let mut bl = Vec::new();
        self.atts
            .iter()
            .map(|&a| {
                bl.clear();
                bl.extend(plan.pairs.iter().filter(|p| p.1 == a).map(|p| p.0));
                self.term(a, &bl)
            })
            .collect()
    }

    /// (creature value won minus lost, our life afterwards, lethal) from the attackers' terms.
    fn total(&self, terms: &[(f32, i32, i32, bool)]) -> (f32, i32, bool) {
        self.total_with(terms, usize::MAX, (0.0, 0, 0, false))
    }

    /// `total` with attacker `k`'s term replaced by `tk`.
    fn total_with(
        &self,
        terms: &[(f32, i32, i32, bool)],
        k: usize,
        tk: (f32, i32, i32, bool),
    ) -> (f32, i32, bool) {
        let pl = &self.g.players[self.d];
        let (mut value, mut dmg, mut poison, mut cmd) = (0.0, 0, 0, false);
        for (j, t) in terms.iter().enumerate() {
            let t = if j == k { &tk } else { t };
            value += t.0;
            dmg += t.1;
            poison += t.2;
            cmd |= t.3;
        }
        let after = pl.life - dmg;
        (
            value,
            after,
            after <= 0 || pl.poison as i32 + poison >= 10 || cmd,
        )
    }

    fn outcome(&self, plan: &BlockPlan) -> (f32, i32, bool) {
        self.total(&self.terms(plan))
    }

    fn score_of(&self, terms: &[(f32, i32, i32, bool)]) -> f32 {
        self.score_with(terms, usize::MAX, (0.0, 0, 0, false))
    }

    /// `score_of` with attacker `k`'s term replaced by `tk`.
    fn score_with(
        &self,
        terms: &[(f32, i32, i32, bool)],
        k: usize,
        tk: (f32, i32, i32, bool),
    ) -> f32 {
        let (value, after, lethal) = self.total_with(terms, k, tk);
        let life = self.g.players[self.d].life;
        value - life_cost(life, after, self.danger) - if lethal { 1000.0 } else { 0.0 }
    }

    /// Value blocks on `atts` (most valuable first): kill and survive, good trades, double
    /// blocks on big threats, free blocks.
    fn value_pass(&self, plan: &mut BlockPlan, atts: &[usize]) {
        let g = self.g;
        let wary = self.wary;
        // (attacker dies, blocker dies) when `b` alone blocks `a` (the same combat model as
        // everywhere else: combat power, double strike).
        let duel = |a: usize, b: usize| -> (bool, bool) {
            let c = clash(g, a, &[b]);
            (c.att_dies, c.dead.contains(&b))
        };
        for &a in atts {
            let at = &g.perms[a];
            let menace = at.has(MENACE);
            let can = self.can(plan, a);
            // 1. Kill it and live (with a margin if they might have a trick).
            let safe = can
                .iter()
                .copied()
                .filter(|&b| {
                    let (ad, bd) = duel(a, b);
                    let margin_ok = !wary
                        || g.perms[b].t - at.p >= 2
                        || g.perms[b].has(FIRST_STRIKE | DEATHTOUCH);
                    ad && !bd && margin_ok
                })
                .min_by(|&x, &y| perm_value(g, x).partial_cmp(&perm_value(g, y)).unwrap());
            if let (Some(b), false) = (safe, menace) {
                if self.log {
                    plan.why.push(format!(
                        "{} blocked by {}: kills it and survives",
                        self.name(a),
                        self.name(b)
                    ));
                }
                plan.pairs.push((b, a));
                continue;
            }
            // 2. A trade that favours us.
            let av = perm_value(g, a);
            let trade = can
                .iter()
                .copied()
                .filter(|&b| {
                    let (ad, _) = duel(a, b);
                    ad && perm_value(g, b) + 0.5 <= av
                })
                .min_by(|&x, &y| perm_value(g, x).partial_cmp(&perm_value(g, y)).unwrap());
            if let (Some(b), false) = (trade, menace) {
                if self.log {
                    plan.why.push(format!(
                        "{} (v{:.1}) blocked by {} (v{:.1}): a trade in our favour",
                        self.name(a),
                        av,
                        self.name(b),
                        perm_value(g, b)
                    ));
                }
                plan.pairs.push((b, a));
                continue;
            }
            // 3. Double block a big threat (also answers menace).
            if av >= 5.0 && can.len() >= 2 {
                let mut pair: Option<(usize, usize, f32)> = None;
                for (x, &b1) in can.iter().enumerate() {
                    for &b2 in &can[x + 1..] {
                        // Can they kill it at all (before the full simulation)?
                        let (x1, x2) = (&g.perms[b1], &g.perms[b2]);
                        let hit = |x: &Perm| {
                            g.combat_power(x).max(0) as i32
                                * if x.has(DOUBLE_STRIKE) { 2 } else { 1 }
                        };
                        if hit(x1) + hit(x2) < (at.t - at.damage) as i32
                            && !x1.has(DEATHTOUCH)
                            && !x2.has(DEATHTOUCH)
                        {
                            continue;
                        }
                        let c = clash(g, a, &[b1, b2]);
                        if !c.att_dies {
                            continue;
                        }
                        // The attacker kills what it can (the most valuable first); the
                        // block is worth the attacker plus the damage it stops, now and on
                        // its next swing.
                        let loss: f32 = c.dead.iter().map(|&b| perm_value(g, b)).sum();
                        let credit = dmg_of(g, at) as f32 * 1.2;
                        if loss < av + credit && pair.is_none_or(|p| loss < p.2) {
                            pair = Some((b1, b2, loss));
                        }
                    }
                }
                if let Some((b1, b2, loss)) = pair {
                    if self.log {
                        plan.why.push(format!(
                            "{} (v{:.1}) double-blocked by {} and {} (risking v{:.1})",
                            self.name(a),
                            av,
                            self.name(b1),
                            self.name(b2),
                            loss
                        ));
                    }
                    plan.pairs.push((b1, a));
                    plan.pairs.push((b2, a));
                    continue;
                }
                // 3b. No pair can: gang-block a big threat with three of the cheapest seven.
                if av >= 8.0 && can.len() >= 3 {
                    if let Some((bl, loss)) = self.gang_block(a, &can, av) {
                        if self.log {
                            plan.why.push(format!(
                                "{} (v{:.1}) gang-blocked by {}, {} and {} (risking v{:.1})",
                                self.name(a),
                                av,
                                self.name(bl[0]),
                                self.name(bl[1]),
                                self.name(bl[2]),
                                loss
                            ));
                        }
                        for b in bl {
                            plan.pairs.push((b, a));
                        }
                        continue;
                    }
                }
            }
            // 4. A free block: the blocker survives the fight (with the trick margin), so
            // the damage stays off us at no cost.
            let free = can
                .iter()
                .copied()
                .filter(|&b| !menace && dmg_of(g, at) > 0 && !duel(a, b).1)
                .filter(|&b| !wary || safe_margin(at, &g.perms[b]))
                .min_by(|&x, &y| perm_value(g, x).partial_cmp(&perm_value(g, y)).unwrap());
            if let Some(b) = free {
                if self.log {
                    plan.why.push(format!(
                        "{} ({} power) blocked by {}: a free block (the blocker survives)",
                        self.name(a),
                        dmg_of(g, at),
                        self.name(b)
                    ));
                }
                plan.pairs.push((b, a));
                continue;
            }
            if self.log {
                plan.why.push(format!(
                    "{} (v{:.1}, {} power) not blocked: {} possible blocker(s), no safe kill, good trade or free block{}",
                    self.name(a),
                    av,
                    dmg_of(g, at),
                    can.len(),
                    if menace { " (menace)" } else { "" }
                ));
            }
        }
    }

    /// Three blockers (of the seven cheapest in `can`) that kill attacker `a` (worth `av`)
    /// for the least loss, if that loss is under its value plus the damage it stops.
    fn gang_block(&self, a: usize, can: &[usize], av: f32) -> Option<([usize; 3], f32)> {
        let g = self.g;
        let at = &g.perms[a];
        let mut c3: Vec<usize> = can.to_vec();
        c3.sort_by(|&x, &y| perm_value(g, x).partial_cmp(&perm_value(g, y)).unwrap());
        c3.truncate(7);
        let hp = (at.t - at.damage) as i32;
        let credit = dmg_of(g, at) as f32 * 1.2;
        let mut best: Option<([usize; 3], f32)> = None;
        for i in 0..c3.len() {
            for j in i + 1..c3.len() {
                for k in j + 1..c3.len() {
                    let bl = [c3[i], c3[j], c3[k]];
                    // Can they kill it at all (before the full simulation)?
                    if bl.iter().map(|&b| dmg_of(g, &g.perms[b])).sum::<i32>() < hp
                        && !bl.iter().any(|&b| g.perms[b].has(DEATHTOUCH))
                    {
                        continue;
                    }
                    let c = clash(g, a, &bl);
                    if !c.att_dies {
                        continue;
                    }
                    let loss: f32 = c.dead.iter().map(|&b| perm_value(g, b)).sum();
                    if loss < av + credit && best.is_none_or(|p| loss < p.1) {
                        best = Some((bl, loss));
                    }
                }
            }
        }
        best
    }

    /// Blocks for one unblocked attacker that change the outcome: each single blocker, or
    /// the two cheapest against menace; against a trampler, also stacks of two to four
    /// chump blockers (each one soaks its toughness).
    fn options(&self, plan: &BlockPlan, a: usize) -> Vec<Pick> {
        let g = self.g;
        let mut can = self.can(plan, a);
        let menace = g.perms[a].has(MENACE);
        let mut v: Vec<Pick> = if menace {
            Vec::new()
        } else {
            can.iter().map(|&b| Pick { b: [b; 4], n: 1 }).collect()
        };
        if can.len() < 2 {
            return v;
        }
        if g.perms[a].has(TRAMPLE) {
            // Most toughness for the value first.
            can.sort_by(|&x, &y| {
                let k =
                    |i: usize| perm_value(g, i) / (g.perms[i].t - g.perms[i].damage).max(1) as f32;
                k(x).partial_cmp(&k(y)).unwrap()
            });
            for n in 2..=can.len().min(4) {
                let mut b = [can[0]; 4];
                b[..n].copy_from_slice(&can[..n]);
                v.push(Pick { b, n });
            }
            return v;
        }
        if menace {
            can.sort_by(|&x, &y| perm_value(g, x).partial_cmp(&perm_value(g, y)).unwrap());
            v.push(Pick {
                b: [can[0], can[1], can[1], can[1]],
                n: 2,
            });
        }
        v
    }

    /// Chump blocks and trades that keep us alive and out of danger: for each unblocked
    /// attacker (most damage first), the block that improves the plan's score the most.
    fn protect_pass(&self, plan: &mut BlockPlan) -> Vec<(f32, i32, i32, bool)> {
        let g = self.g;
        let mut open: Vec<usize> = self
            .atts
            .iter()
            .copied()
            .filter(|&a| !plan.blocked(a))
            .collect();
        open.sort_by_key(|&a| -dmg_of(g, &g.perms[a]));
        let mut terms = self.terms(plan);
        let mut cur = self.score_of(&terms);
        for a in open {
            let k = self.atts.iter().position(|&x| x == a).unwrap();
            let mut best: Option<(f32, Pick, (f32, i32, i32, bool))> = None;
            for bl in self.options(plan, a) {
                let t = self.term(a, bl.list());
                let s = self.score_with(&terms, k, t);
                if best.as_ref().is_none_or(|x| s > x.0) {
                    best = Some((s, bl, t));
                }
            }
            match best {
                Some((s, bl, t)) if s > cur => {
                    if self.log {
                        let (_, before, lethal) = self.outcome(plan);
                        let names: Vec<String> = bl.list().iter().map(|&b| self.name(b)).collect();
                        plan.why.push(format!(
                            "{} ({} power) blocked by {}: {} (life {} -> {}, danger {})",
                            self.name(a),
                            dmg_of(g, &g.perms[a]),
                            names.join(" and "),
                            if lethal {
                                "the unblocked damage would be lethal"
                            } else {
                                "keeps our life out of danger"
                            },
                            g.players[self.d].life,
                            before,
                            self.danger
                        ));
                    }
                    for &b in bl.list() {
                        plan.pairs.push((b, a));
                    }
                    terms[k] = t;
                    cur = s;
                }
                _ => {
                    if self.log && self.outcome(plan).2 {
                        plan.why.push(format!(
                            "{} would be lethal but can't be blocked usefully",
                            self.name(a)
                        ));
                    }
                }
            }
        }
        self.stack_on_tramplers(plan, &mut terms, &mut cur);
        terms
    }

    /// Tramplers that are already blocked but still push damage through: add one to three
    /// more chump blockers (most toughness for the value first) when that improves the score.
    fn stack_on_tramplers(
        &self,
        plan: &mut BlockPlan,
        terms: &mut [(f32, i32, i32, bool)],
        cur: &mut f32,
    ) {
        let g = self.g;
        for (k, &a) in self.atts.iter().enumerate() {
            if !g.perms[a].has(TRAMPLE) || !plan.blocked(a) || terms[k].1 == 0 {
                continue;
            }
            let mut can = self.can(plan, a);
            if can.is_empty() {
                continue;
            }
            can.sort_by(|&x, &y| {
                let k =
                    |i: usize| perm_value(g, i) / (g.perms[i].t - g.perms[i].damage).max(1) as f32;
                k(x).partial_cmp(&k(y)).unwrap()
            });
            let mut bl: Vec<usize> = plan
                .pairs
                .iter()
                .filter(|p| p.1 == a)
                .map(|p| p.0)
                .collect();
            let have = bl.len();
            let mut best: Option<(f32, usize, (f32, i32, i32, bool))> = None;
            for n in 1..=can.len().min(3) {
                bl.truncate(have);
                bl.extend_from_slice(&can[..n]);
                let t = self.term(a, &bl);
                let s = self.score_with(terms, k, t);
                if s > *cur && best.as_ref().is_none_or(|x| s > x.0) {
                    best = Some((s, n, t));
                }
            }
            if let Some((s, n, t)) = best {
                if self.log {
                    let names: Vec<String> = can[..n].iter().map(|&b| self.name(b)).collect();
                    plan.why.push(format!(
                        "{} (trample) also blocked by {}: soaks more of its damage",
                        self.name(a),
                        names.join(" and ")
                    ));
                }
                for &b in &can[..n] {
                    plan.pairs.push((b, a));
                }
                terms[k] = t;
                *cur = s;
            }
        }
    }

    /// Survival first: block the biggest hitters (with the blocker that loses least) until
    /// the rest can't put us in danger, then value blocks with what's left.
    fn survival_plan(&self) -> (BlockPlan, Vec<(f32, i32, i32, bool)>) {
        let g = self.g;
        let life = g.players[self.d].life;
        let mut plan = BlockPlan::default();
        let mut by_dmg = self.atts.clone();
        by_dmg.sort_by_key(|&a| -dmg_of(g, &g.perms[a]));
        let mut rem: i32 = by_dmg.iter().map(|&a| dmg_of(g, &g.perms[a])).sum();
        let mut rest = Vec::new();
        for a in by_dmg {
            let dmg = dmg_of(g, &g.perms[a]);
            if life - rem > self.danger.max(3) {
                rest.push(a);
                continue;
            }
            let mut best: Option<(f32, Pick, i32)> = None;
            for bl in self.options(&plan, a) {
                let c = clash(g, a, bl.list());
                let net = if c.att_dies { perm_value(g, a) } else { 0.0 }
                    - c.dead.iter().map(|&b| perm_value(g, b)).sum::<f32>();
                if best.as_ref().is_none_or(|x| net > x.0) {
                    best = Some((net, bl, c.through));
                }
            }
            match best {
                Some((_, bl, through)) => {
                    if self.log {
                        let names: Vec<String> = bl.list().iter().map(|&b| self.name(b)).collect();
                        plan.why.push(format!(
                            "survival first: {} ({} power) blocked by {}",
                            self.name(a),
                            dmg,
                            names.join(" and ")
                        ));
                    }
                    for &b in bl.list() {
                        plan.pairs.push((b, a));
                    }
                    rem -= dmg - through;
                }
                None => rest.push(a),
            }
        }
        rest.sort_by(|&x, &y| perm_value(g, y).partial_cmp(&perm_value(g, x)).unwrap());
        self.value_pass(&mut plan, &rest);
        let terms = self.protect_pass(&mut plan);
        (plan, terms)
    }
}

/// How `d` blocks: kill-and-survive first, good trades, double blocks on big threats, free
/// blocks; then chump blocks and trades when the damage would kill us or leave us in reach
/// of the next round's attacks (or, when that isn't enough, survival first: the biggest
/// hitters get blocked before value blocks). Careful about tricks when the attacker has
/// mana up.
pub fn declare_blocks(g: &Game, d: usize, attacks: &[Attack]) -> Vec<(u32, u32)> {
    declare_blocks_why(g, d, attacks, None)
}

/// `declare_blocks`, with the reasons written to `why` (for the game log).
pub fn declare_blocks_why(
    g: &Game,
    d: usize,
    attacks: &[Attack],
    why: Option<&mut Vec<String>>,
) -> Vec<(u32, u32)> {
    let Some((_, mut plan)) = plan_blocks(g, d, attacks, why.is_some(), None) else {
        return Vec::new();
    };
    if let Some(w) = why {
        w.append(&mut plan.why);
    }
    plan.pairs
        .iter()
        .map(|&(b, a)| (g.perms[b].id, g.perms[a].id))
        .collect()
}

/// The blocking AI's plan against `attacks` (None when nothing attacks), with its score.
/// `skip`: a creature of `d`'s that won't block (one the attacker plans to stop, see
/// `group_value`).
fn plan_blocks(
    g: &Game,
    d: usize,
    attacks: &[Attack],
    log: bool,
    skip: Option<usize>,
) -> Option<(f32, BlockPlan)> {
    let mut atts: Vec<usize> = attacks.iter().filter_map(|at| g.idx(at.attacker)).collect();
    if atts.is_empty() {
        return None;
    }
    atts.sort_by(|&x, &y| perm_value(g, y).partial_cmp(&perm_value(g, x)).unwrap());
    let blockers: Vec<usize> = (0..g.perms.len())
        .filter(|&i| {
            let x = &g.perms[i];
            x.ctrl as usize == d
                && x.is_creature()
                && !x.tapped
                && !x.locked
                && !x.has(CANT_BLOCK)
                && skip != Some(i)
        })
        .collect();
    let attacker_ctrl = g.perms[atts[0]].ctrl as usize;
    let wary = !g.players[attacker_ctrl].hand.is_empty() && g.mana_available(attacker_ctrl) >= 2;
    let able = atts
        .iter()
        .flat_map(|&a| blockers.iter().map(move |&b| (a, b)))
        .map(|(a, b)| g.can_block(b, a))
        .collect();
    let bk = Blocking {
        g,
        d,
        atts,
        blockers,
        able,
        wary,
        danger: danger_before_turn(g, d, attacker_ctrl),
        log,
    };
    let mut plan = BlockPlan::default();
    bk.value_pass(&mut plan, &bk.atts);
    let terms = bk.protect_pass(&mut plan);
    let mut score = bk.score_of(&terms);
    let (_, after, lethal) = bk.total(&terms);
    if lethal || after <= bk.danger.max(3) {
        let (alt, terms) = bk.survival_plan();
        let s = bk.score_of(&terms);
        if s > score + 0.5 {
            plan = alt;
            score = s;
        }
    }
    Some((score, plan))
}

/// Damage `group` gets through to `o` if `o` blocks the biggest blockable attackers with
/// its untapped creatures, one each (chump blocks; trample carries over).
fn lethal_through(g: &Game, o: usize, group: &[usize], blockers: &[i16]) -> i32 {
    let mut through: i32 = 0;
    let mut nb = blockers.len();
    for &a in group {
        let at = &g.perms[a];
        // A menace attacker takes two blockers.
        if at.has(MENACE) && nb >= 2 && !evades_all(g, o, a) {
            nb -= 2;
            if at.has(TRAMPLE) {
                let soak = blockers.get(nb).copied().unwrap_or(0)
                    + blockers.get(nb + 1).copied().unwrap_or(0);
                through += (at.p - soak).max(0) as i32;
            }
            continue;
        }
        if evades_all(g, o, a) || nb == 0 || (at.has(MENACE) && nb < 2) {
            through += (dmg_of(g, at) + g.players[o].attacked_power as i32).max(0);
        } else {
            nb -= 1;
            if at.has(TRAMPLE) {
                through += (at.p - blockers.get(nb).copied().unwrap_or(0)).max(0) as i32;
            }
        }
    }
    through
}

/// No creature of `o` can block attacker `a` (flying, unblockable, ...).
fn evades_all(g: &Game, o: usize, a: usize) -> bool {
    g.perms[a].has(FLYING | UNBLOCKABLE)
        && !(0..g.perms.len()).any(|b| g.perms[b].ctrl as usize == o && g.can_block(b, a))
}

/// How close `o` is to winning: 1 when its board could kill someone next turn (fliers and
/// unblockables in full, the rest at half), plus 0.6 when it is far ahead of the table.
fn alarm(g: &Game, o: usize) -> f32 {
    let clock: i32 = g
        .perms
        .iter()
        .filter(|x| x.ctrl as usize == o && x.is_creature() && !x.has(DEFENDER))
        .map(|x| {
            let d = dmg_of(g, x);
            if x.has(FLYING | UNBLOCKABLE) {
                d
            } else {
                d / 2
            }
        })
        .sum();
    let weakest = g
        .opponents(o)
        .iter()
        .map(|&p| g.players[p].life)
        .min()
        .unwrap_or(i32::MAX);
    let mut a = if clock >= weakest { 1.0 } else { 0.0 };
    let mut ts: Vec<f32> = (0..g.players.len())
        .filter(|&p| g.players[p].alive)
        .map(|p| threat(g, p))
        .collect();
    ts.sort_by(|x, y| y.partial_cmp(x).unwrap());
    if ts.len() >= 2 && threat(g, o) >= ts[0] && ts[0] >= 1.6 * ts[1].max(1.0) {
        a += 0.6;
    }
    a
}

/// "You may pay {N} up to three (two) times. When you do, choose up to that many" (Hawkeye's
/// Trick Arrows): (N, most modes), else None.
pub(crate) fn paid_modes(ab: &SimAbility) -> Option<(u32, usize)> {
    let rest = ab.text.split("you may pay {").nth(1)?;
    let most = if ab.text.contains("up to three times") {
        3
    } else if ab.text.contains("up to two times") {
        2
    } else {
        return None;
    };
    let n = rest
        .split('}')
        .next()
        .and_then(|d| d.parse().ok())
        .unwrap_or(1);
    Some((n, most))
}

/// A mode that stops a blocker ("target creature can't block this turn").
fn stops_a_blocker(ab: &SimAbility, r: &std::ops::Range<usize>) -> bool {
    ab.effects[r.clone()]
        .iter()
        .any(|e| matches!(e, Effect::Rule(l) if l == "can't block this turn"))
}

/// What attacking with `a` is worth through its own triggers - "whenever ~ attacks" and,
/// without vigilance, "whenever ~ becomes tapped" - with `mana` to pay for them (Hawkeye's
/// Trick Arrows: the paid modes worth their mana, as the resolver picks them, less what
/// that mana would have cast after combat - the price `modes_worth_paying` weighs).
///
/// A mode that stops a blocker (Net) isn't counted on: the resolver uses it when the attack
/// meets a blocker worth stopping (`net_target`). Measured on a Thanos deck, 20,000 paired
/// games on each of two seeds: Hawkeye played 1.2 win-rate points worse than the same 2/2
/// first strike, reach without arrows while the attack plan priced the arrows at a flat
/// half point a mana and predicted the blocks without the blocker its Net would stop; 0.5
/// worse with the arrows priced as the resolver prices them; level (+0.0) with the Net
/// left out of the plan too.
pub(crate) fn attack_trigger_value(g: &Game, a: usize, mana: u32) -> f32 {
    let x = &g.perms[a];
    let p = x.ctrl as usize;
    let mut v = 0.0;
    for (k, ab) in g.abilities_of(x).iter().enumerate() {
        let fires = match &ab.trig {
            Trig::Attacks => !(ab.once_per_turn && k < 32 && x.fired & (1 << k) != 0),
            Trig::Event(Ev::Tapped, _) => {
                !x.has(VIGILANCE)
                    && ab.text.contains("whenever ~")
                    && !ab.text.contains("whenever ~ or")
            }
            _ => false,
        };
        if !fires {
            continue;
        }
        let Some((n, most)) = paid_modes(ab).filter(|_| ab.modal) else {
            v += ability_now(g, p, ab, Some(x.id)).max(0.0);
            continue;
        };
        let mut ranked: Vec<f32> = Vec::new();
        for (m, r) in ab.mode_groups().iter().enumerate() {
            if stops_a_blocker(ab, r) {
                continue;
            }
            let (t, cr) = ab.mode_text(m);
            let mv: f32 = ab.effects[r.clone()]
                .iter()
                .map(|e| effect_now(g, p, e, t, Some(x.id), cr))
                .sum();
            if mv >= 0.5 * n as f32 {
                ranked.push(mv);
            }
        }
        ranked.sort_by(|a, b| b.partial_cmp(a).unwrap());
        let afford = ((mana / n.max(1)) as usize).min(most);
        // The modes worth their mana: what paying for `k` of them costs is the spells that
        // mana would have cast after combat (and half a point a mana at least).
        let (mut best, mut sum) = (0.0f32, 0.0f32);
        for (i, mv) in ranked.iter().take(afford).enumerate() {
            sum += mv;
            let m = n * (i as u32 + 1);
            best = best.max(sum - post_combat_loss(g, p, m).max(0.5 * m as f32));
        }
        v += best;
    }
    v
}

/// The value of sending `group` at opponent `o`, predicting `o`'s blocks with the blocking
/// AI: damage that gets through (weighted by how much we want to hurt `o`), creatures
/// killed on each side. Also the attackers that would die for less than they take
/// (`bad`), and how many blocks were predicted. `w`: how much we want to hurt `o`.
fn group_value(g: &Game, o: usize, w: f32, group: &[usize]) -> (f32, Vec<usize>, usize) {
    if group.is_empty() {
        return (0.0, Vec::new(), 0);
    }
    let attacks: Vec<Attack> = group
        .iter()
        .map(|&a| Attack {
            attacker: g.perms[a].id,
            def: Def::Player(o),
            blockers: Vec::new(),
            blocked: false,
        })
        .collect();
    // An open board blocks nothing (no need to ask the blocking AI).
    let open = !g.perms.iter().any(|x| {
        x.ctrl as usize == o && x.is_creature() && !x.tapped && !x.locked && !x.has(CANT_BLOCK)
    });
    // The attackers' own attack triggers.
    let me = g.perms[group[0]].ctrl as usize;
    let mut trig = vec![0.0; group.len()];
    if group.iter().any(|&a| {
        g.abilities_of(&g.perms[a])
            .iter()
            .any(|ab| matches!(ab.trig, Trig::Attacks | Trig::Event(Ev::Tapped, _)))
    }) {
        let mana = g.mana_available(me);
        for (k, &a) in group.iter().enumerate() {
            trig[k] = attack_trigger_value(g, a, mana);
        }
    }
    // Damage counts for how much it shortens the defender's clock (as at 20 life: x1; at
    // 10: x1.33; at 40: x0.83).
    let w = w * (1.0 + 10.0 / g.players[o].life.max(1) as f32) / 1.5;
    let blocks = if open {
        Vec::new()
    } else {
        declare_blocks(g, o, &attacks)
    };
    let mut value = 0.0;
    let mut bad = Vec::new();
    for (k, &a) in group.iter().enumerate() {
        let at = &g.perms[a];
        let mut bl: Vec<usize> = blocks
            .iter()
            .filter(|(_, x)| *x == at.id)
            .filter_map(|(b, _)| g.idx(*b))
            .collect();
        if at.has(MENACE) && bl.len() == 1 {
            bl.clear();
        }
        if bl.is_empty() {
            value += (dmg_of(g, at) + g.players[o].attacked_power as i32).max(0) as f32 * 0.6 * w
                + trig[k];
            continue;
        }
        // Blocked: what dies on each side, and what tramples over.
        let c = clash(g, a, &bl);
        let killed: f32 = c.dead.iter().map(|&b| perm_value(g, b)).sum();
        let net = killed + c.through as f32 * 0.6 * w + trig[k]
            - if c.att_dies { perm_value(g, a) } else { 0.0 };
        if c.att_dies && net < 0.0 && !g.must_attack(a) {
            bad.push(a);
        }
        value += net;
    }
    (value, bad, blocks.len())
}

/// During our combat, before blocks: the creature of a player we attack whose not blocking
/// helps our attack most (for "target creature can't block this turn", Hawkeye's Net), as
/// `group_value` predicts it. None when no blocker matters.
pub fn blocker_to_stop(g: &Game, me: usize) -> Option<u32> {
    let mut best: Option<(f32, u32)> = None;
    let mut defs: Vec<usize> = g.ext.attack_def.iter().map(|x| x.1 as usize).collect();
    defs.sort_unstable();
    defs.dedup();
    for o in defs {
        let attacks: Vec<Attack> = g
            .ext
            .attack_def
            .iter()
            .filter(|x| x.1 as usize == o && g.idx(x.0).is_some())
            .map(|x| Attack {
                attacker: x.0,
                def: Def::Player(o),
                blockers: Vec::new(),
                blocked: false,
            })
            .collect();
        let Some((base, plan)) = plan_blocks(g, o, &attacks, false, None) else {
            continue;
        };
        let mut used: Vec<usize> = plan.pairs.iter().map(|p| p.0).collect();
        used.sort_unstable();
        used.dedup();
        for b in used {
            // The defender's score drops the most without it.
            let s = plan_blocks(g, o, &attacks, false, Some(b)).map_or(base, |x| x.0);
            let gain = (base - s) * weight(g, me, o).max(0.3);
            if gain > 0.0 && best.is_none_or(|x| gain > x.0) {
                best = Some((gain, g.perms[b].id));
            }
        }
    }
    best.map(|x| x.1)
}

/// The creatures the opponents in `opps` could attack with next time around: (damage, index).
fn swings(g: &Game, opps: &[usize]) -> Vec<(i32, usize)> {
    g.perms
        .iter()
        .enumerate()
        .filter(|(_, x)| opps.contains(&(x.ctrl as usize)) && x.is_creature() && !x.has(DEFENDER))
        .map(|(i, x)| (dmg_of(g, x), i))
        .filter(|x| x.0 > 0)
        .collect()
}

/// Damage the opponents' `swings` could deal `me` before `me`'s next turn if the creatures
/// in `out` (without vigilance) are tapped: each one weighted by the chance its controller
/// picks us, minus what our untapped creatures can block (one attacker each).
fn crack_back(
    g: &Game,
    me: usize,
    swings: &[(i32, usize)],
    out: &[(u32, Def)],
    hand_def: &[bool],
) -> f32 {
    // Our defenders (untapped, staying home): whether each can block fliers.
    let mut defenders: Vec<bool> = g
        .perms
        .iter()
        .filter(|x| {
            x.ctrl as usize == me
                && x.is_creature()
                && !x.has(CANT_BLOCK)
                && !x.tapped
                && (x.has(VIGILANCE) || !out.iter().any(|(id, _)| *id == x.id))
        })
        .map(|x| x.has(FLYING | REACH))
        .collect();
    // Creatures we can still cast after combat block too.
    defenders.extend_from_slice(hand_def);
    // An opponent we hit this turn is more likely to hit back, and a player with no
    // blockers left is the table's favourite target.
    let (mut wh, mut wo) = (0.5, 0.25);
    if defenders.is_empty() {
        (wh, wo) = (0.75, 0.375);
    }
    let mut threats: Vec<(f32, usize)> = swings
        .iter()
        .map(|&(d, i)| {
            let o = g.perms[i].ctrl as usize;
            let hit = out.iter().any(|&(_, x)| g.def_player(x) == Some(o));
            (d as f32 * if hit { wh } else { wo }, i)
        })
        .collect();
    threats.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap());
    // Ground blockers first, so fliers and reach are left for the fliers.
    defenders.sort();
    let mut used = vec![false; defenders.len()];
    let mut sum = 0.0;
    for (v, i) in threats {
        let t = &g.perms[i];
        let fits = |k: usize, used: &[bool]| {
            !used[k] && !t.has(UNBLOCKABLE) && !(t.has(FLYING) && !defenders[k])
        };
        let Some(k) = (0..defenders.len()).find(|&k| fits(k, &used)) else {
            sum += v;
            continue;
        };
        // A menace attacker takes two blockers.
        if t.has(MENACE) {
            match (k + 1..defenders.len()).find(|&k2| fits(k2, &used)) {
                Some(k2) => {
                    used[k] = true;
                    used[k2] = true;
                }
                None => sum += v,
            }
            continue;
        }
        used[k] = true;
    }
    sum
}

/// What paying `tax` more mana in attack taxes (after `spent` already) costs `me`: at least
/// 0.35 a mana, or what that mana would have cast after combat.
fn tax_price(g: &Game, me: usize, spent: u32, tax: u32) -> f32 {
    let flat = tax as f32 * 0.35;
    if tax == 0 {
        return flat;
    }
    let lost = post_combat_loss(g, me, spent + tax) - post_combat_loss(g, me, spent);
    flat.max(lost)
}

/// The part of ability `a`'s value (as `ability_now` counts it) that is an until-end-of-turn
/// pump or keyword grant for all of `p`'s creatures and would go to creatures that won't
/// attack: all of it after combat, and before combat whatever the attack planner (asked now)
/// keeps home. 0 for anything else.
fn unused_team_pump(g: &Game, p: usize, a: &SimAbility, src: Option<u32>, post: bool) -> f32 {
    let team: f32 = a
        .effects
        .iter()
        .filter(|e| {
            matches!(e, Effect::Pump { until_eot: true, to, .. } | Effect::Grant { until_eot: true, to, .. } if to.mass)
        })
        .map(|e| effect_now(g, p, e, &a.text, src, a.card_restr))
        .sum();
    if team <= 0.0 || a.modal {
        return 0.0;
    }
    if post || g.active != p || g.in_combat {
        return team;
    }
    let mine = g
        .perms
        .iter()
        .filter(|x| x.ctrl as usize == p && x.is_creature())
        .count()
        .max(1);
    let attackers = declare_attacks(g, p).len().min(mine);
    team * (mine - attackers) as f32 / mine as f32
}

/// Mana to keep for attack taxes (Propaganda, Dáin) before combat: what the lethal swings
/// the attack planner finds now would cost. 0 when no opponent taxes attacks.
pub fn attack_tax_reserve(g: &Game, me: usize) -> u32 {
    if !g.rule_anywhere(R_ATTACK_TAX) {
        return 0;
    }
    let mana = g.mana_available(me);
    let mut ready: Vec<usize> = (0..g.perms.len())
        .filter(|&i| g.perms[i].ctrl as usize == me && g.can_attack(i))
        .collect();
    ready.sort_by(|&x, &y| g.perms[y].p.cmp(&g.perms[x].p));
    for o in g.opponents(me) {
        let tax = g.attack_tax(o);
        if tax == 0 {
            continue;
        }
        let blockers = blocker_toughness(g, o);
        let mut order = ready.clone();
        order.sort_by_key(|&a| !evades_all(g, o, a));
        let order = best_k(g, o, &order, (mana / tax) as usize, &blockers);
        let life = g.players[o].life;
        if let Some(k) =
            (1..=order.len()).find(|&k| lethal_through(g, o, &order[..k], &blockers) >= life)
        {
            return tax * (k + 1).min(order.len()) as u32;
        }
    }
    0
}

/// The `k` attackers of `order` (evasive first, then the biggest) that get the most damage
/// through `o`'s `blockers`: the best split between evasive and other attackers, biggest
/// first in each; in `order`'s order.
fn best_k(g: &Game, o: usize, order: &[usize], k: usize, blockers: &[i16]) -> Vec<usize> {
    if k >= order.len() {
        return order.to_vec();
    }
    let (ev, rest): (Vec<usize>, Vec<usize>) = order.iter().partition(|&&a| evades_all(g, o, a));
    let by_dmg = |v: &[usize]| {
        let mut v = v.to_vec();
        v.sort_by_key(|&a| -dmg_of(g, &g.perms[a]));
        v
    };
    let (ev, rest) = (by_dmg(&ev), by_dmg(&rest));
    let mut best: Option<(i32, Vec<usize>)> = None;
    for e in 0..=k.min(ev.len()) {
        if k - e > rest.len() {
            continue;
        }
        let set: Vec<usize> = order
            .iter()
            .copied()
            .filter(|a| ev[..e].contains(a) || rest[..k - e].contains(a))
            .collect();
        let t = lethal_through(g, o, &set, blockers);
        if best.as_ref().is_none_or(|b| t > b.0) {
            best = Some((t, set));
        }
    }
    best.map_or_else(Vec::new, |b| b.1)
}

/// The toughness of each creature `o` could block with, lowest first.
fn blocker_toughness(g: &Game, o: usize) -> Vec<i16> {
    let mut blockers: Vec<i16> = (0..g.perms.len())
        .filter(|&b| {
            g.perms[b].ctrl as usize == o
                && g.perms[b].is_creature()
                && !g.perms[b].tapped
                && !g.perms[b].has(CANT_BLOCK)
        })
        .map(|b| g.perms[b].t)
        .collect();
    blockers.sort();
    blockers
}

/// The creature cards in `me`'s hand that the mana we have now can cast after combat
/// (cheapest first): whether each can block fliers.
fn castable_blockers(g: &Game, me: usize) -> Vec<bool> {
    let pl = &g.players[me];
    let mut c: Vec<(u32, bool)> = (0..pl.hand.len())
        .filter_map(|h| {
            let card = &pl.cards[pl.hand[h] as usize];
            (card.is(T_CREATURE) && g.can_cast(me, Src::Hand(h), CastOpts::default()))
                .then(|| (card.cost.total(), card.kw & (FLYING | REACH) != 0))
        })
        .collect();
    if c.is_empty() {
        return Vec::new();
    }
    c.sort();
    let mut mana = g.mana_available(me);
    c.into_iter()
        .take_while(|&(cost, _)| {
            let ok = cost <= mana;
            mana = mana.saturating_sub(cost);
            ok
        })
        .map(|x| x.1)
        .collect()
}

/// Attacks for the active player: lethal swings first, then every attack that pays off,
/// keeping enough back to survive the next turn around the table.
pub fn declare_attacks(g: &Game, me: usize) -> Vec<(u32, Def)> {
    declare_attacks_why(g, me, None)
}

/// `declare_attacks`, with the reasons written to `why` (for the game log).
pub fn declare_attacks_why(
    g: &Game,
    me: usize,
    mut why: Option<&mut Vec<String>>,
) -> Vec<(u32, Def)> {
    let name = |i: usize| g.perm_name(&g.perms[i]);
    let mut ready: Vec<usize> = (0..g.perms.len())
        .filter(|&i| g.perms[i].ctrl as usize == me && g.can_attack(i))
        .collect();
    if ready.is_empty() {
        return Vec::new();
    }
    ready.sort_by(|&x, &y| g.perms[y].p.cmp(&g.perms[x].p));
    let mut opps = g.opponents(me);
    let mut out: Vec<(u32, Def)> = Vec::new();
    let mana = g.mana_available(me);
    // Attack tax paid by the lethal swings.
    let mut lethal_spent: u32 = 0;
    // The lethal swings' spares (sent in case of a surprise blocker).
    let mut spares: Vec<u32> = Vec::new();
    // Lethal: the attackers we can best afford to lose whose damage gets through a player's
    // blocks, plus a spare if the kill relies on blockable attackers (everything at the last
    // opponent). The rest attack or stay home as usual.
    'lethal: loop {
        for &o in &opps {
            let blockers = blocker_toughness(g, o);
            let life = g.players[o].life;
            // Evasive attackers first, then the biggest.
            let mut order = ready.clone();
            order.sort_by_key(|&a| !evades_all(g, o, a));
            // Attack tax: only the attackers the mana pays for.
            let ltax = g.attack_tax(o);
            if ltax > 0 {
                let k = (mana.saturating_sub(lethal_spent) / ltax) as usize;
                order = best_k(g, o, &order, k, &blockers);
            }
            if lethal_through(g, o, &order, &blockers) < life {
                if ltax > 0 && lethal_through(g, o, &ready, &blockers) >= life {
                    if let Some(w) = why.as_deref_mut() {
                        w.push(format!(
                            "no lethal on P{}: the attack tax ({} each) pays for only {} attacker(s)",
                            o,
                            ltax,
                            order.len()
                        ));
                    }
                }
                continue;
            }
            let take = if opps.len() == 1 {
                order.len()
            } else {
                // Kill with what we can best afford to lose: leave out the most valuable
                // attackers (commanders first) while the rest still get enough through.
                let mut by_value = order.clone();
                by_value.sort_by(|&x, &y| {
                    let k =
                        |i: usize| perm_value(g, i) + 4.0 * (g.perms[i].commander > 0) as u8 as f32;
                    k(y).partial_cmp(&k(x)).unwrap()
                });
                let mut set = order.clone();
                for a in by_value {
                    let rest: Vec<usize> = set.iter().copied().filter(|&x| x != a).collect();
                    if !rest.is_empty() && lethal_through(g, o, &rest, &blockers) >= life {
                        set = rest;
                    }
                }
                // A spare only when the kill relies on attackers that can be blocked: the
                // cheapest one left.
                if set.iter().any(|&a| !evades_all(g, o, a)) {
                    if let Some(&s) = order
                        .iter()
                        .filter(|a| !set.contains(a))
                        .min_by(|&&x, &&y| perm_value(g, x).partial_cmp(&perm_value(g, y)).unwrap())
                    {
                        set.push(s);
                        spares.push(g.perms[s].id);
                    }
                }
                // Keep `order`'s order (evasive first, then the biggest).
                order.retain(|a| set.contains(a));
                order.len()
            };
            lethal_spent += ltax * take as u32;
            if let Some(w) = why.as_deref_mut() {
                let names: Vec<String> = order[..take].iter().map(|&a| name(a)).collect();
                w.push(format!(
                    "lethal on P{}: {} damage gets through vs life {} -> [{}] attack P{}",
                    o,
                    lethal_through(g, o, &order[..take], &blockers),
                    life,
                    names.join(", "),
                    o
                ));
            }
            for &a in &order[..take] {
                out.push((g.perms[a].id, Def::Player(o)));
            }
            ready.retain(|a| !order[..take].contains(a));
            opps.retain(|&x| x != o);
            if opps.is_empty() || (ready.is_empty() && spares.is_empty()) {
                return out;
            }
            continue 'lethal;
        }
        break;
    }
    // The lethal swings' attackers never stay home, except a spare (the crack-back may keep
    // it, first of all).
    let lethal_ids: Vec<u32> = out
        .iter()
        .map(|x| x.0)
        .filter(|id| !spares.contains(id))
        .collect();
    let goad_ok = |a: usize, o: usize| match g.perms[a].goad {
        Some(gd) if opps.len() > 1 => gd as usize != o,
        _ => true,
    };
    // Group plan: send a group at one opponent, predict their blocks with the blocking AI,
    // drop attackers that would just die, and keep the best opponent's plan.
    let mut best_plan: Option<(f32, usize, Vec<usize>)> = None;
    // (opponent, attacker) pairs the group plan dropped as dying for less.
    let mut dropped: Vec<(usize, usize)> = Vec::new();
    // Attack taxes (Propaganda): only as many attackers as the mana pays for, and the mana
    // isn't free.
    let mana = mana.saturating_sub(lethal_spent);
    let tax_of = |o: usize| g.attack_tax(o);
    // How much we want to hurt each opponent: the usual target weight, plus the alarm when
    // they're about to win.
    let wt = |o: usize| weight(g, me, o) + alarm(g, o);
    let wts: Vec<f32> = opps.iter().map(|&o| wt(o)).collect();
    let w_of = |o: usize| wts[opps.iter().position(|&x| x == o).unwrap_or(0)];
    for &o in &opps {
        let w = w_of(o);
        let mut group: Vec<usize> = ready.iter().copied().filter(|&a| goad_ok(a, o)).collect();
        let tax = tax_of(o);
        if tax > 0 {
            group.truncate((mana / tax) as usize);
        }
        for it in 0..4 {
            if group.is_empty() {
                break;
            }
            let (mut value, bad, blocked) = group_value(g, o, w, &group);
            value -= tax_price(g, me, 0, tax * group.len() as u32);
            // Heads-up, the whole team at once is a candidate too, losses and all: one or two
            // good blockers shouldn't veto every attack.
            if !bad.is_empty()
                && it == 0
                && opps.len() == 1
                && value > 0.2
                && best_plan.as_ref().is_none_or(|b| value > b.0)
            {
                if let Some(wh) = why.as_deref_mut() {
                    wh.push(format!(
                        "group vs P{}: the full swing is a candidate (value {:.1})",
                        o, value
                    ));
                }
                best_plan = Some((value, o, group.clone()));
            }
            if bad.is_empty() {
                if let Some(wh) = why.as_deref_mut() {
                    let names: Vec<String> = group.iter().map(|&a| name(a)).collect();
                    wh.push(format!(
                        "group vs P{} (life {}, threat {:.1}, weight {:.2}): [{}] predicted {} block(s), plan value {:.1}{}",
                        o,
                        g.players[o].life,
                        threat_peek(g, o),
                        w,
                        names.join(", "),
                        blocked,
                        value,
                        if value > 0.2 { "" } else { " (not worth it)" }
                    ));
                }
                if value > 0.2 && best_plan.as_ref().is_none_or(|b| value > b.0) {
                    best_plan = Some((value, o, group.clone()));
                }
                break;
            }
            if let Some(wh) = why.as_deref_mut() {
                let names: Vec<String> = bad.iter().map(|&a| name(a)).collect();
                wh.push(format!(
                    "group vs P{}: drops [{}] (would die for less)",
                    o,
                    names.join(", ")
                ));
            }
            dropped.extend(bad.iter().map(|&a| (o, a)));
            group.retain(|a| !bad.contains(a));
        }
    }
    let mut gains: Vec<(f32, usize)> = spares
        .iter()
        .filter_map(|&id| g.idx(id))
        .map(|i| (f32::MIN, i))
        .collect();
    // Attackers sent at each opponent so far (by the group plan and the alone pass).
    let mut sent: Vec<(usize, Vec<usize>, f32)> = Vec::new();
    if let Some((v, o, group)) = &best_plan {
        if let Some(wh) = why.as_deref_mut() {
            wh.push(format!("chosen group plan: P{} (value {:.1})", o, v));
        }
        let each = v / group.len().max(1) as f32;
        for &a in group {
            out.push((g.perms[a].id, Def::Player(*o)));
            gains.push((each, a));
        }
        sent.push((*o, group.clone(), *v));
    }
    // Everyone else: one at a time, at another player or a planeswalker, valued with the
    // same block prediction as the group plan (the attacker joins what's already sent there).
    let mut spent: u32 = best_plan
        .as_ref()
        .map_or(0, |(_, o, group)| tax_of(*o) * group.len() as u32);
    for &a in &ready {
        if out.iter().any(|(id, _)| *id == g.perms[a].id) {
            continue;
        }
        let at = &g.perms[a];
        let choices: Vec<usize> = opps.iter().copied().filter(|&o| goad_ok(a, o)).collect();
        let mut best: Option<(f32, Def)> = None;
        let mut gains_s: Vec<String> = Vec::new();
        for &o in &choices {
            let tax = tax_of(o);
            let (base, mut with) = match sent.iter().find(|s| s.0 == o) {
                Some(s) => (s.2, s.1.clone()),
                None => (0.0, Vec::new()),
            };
            with.push(a);
            let v = if dropped.contains(&(o, a)) && !g.must_attack(a) {
                // The block prediction already said it dies for less there.
                -1.0
            } else {
                group_value(g, o, w_of(o), &with).0 - base - tax_price(g, me, spent, tax)
            };
            if why.is_some() {
                gains_s.push(format!("P{} {:.1}", o, v));
            }
            if (tax == 0 || spent + tax <= mana) && best.is_none_or(|b| v > b.0) {
                best = Some((v, Def::Player(o)));
            }
            // Their planeswalkers.
            for wk in g
                .perms
                .iter()
                .filter(|x| x.ctrl as usize == o && x.is(T_PLANESWALKER) && !x.is_creature())
            {
                let wv = 2.0 + wk.loyalty as f32 * 0.6;
                let kill = at.p >= wk.loyalty;
                let v2 = if v >= 0.0 {
                    if kill { wv } else { at.p as f32 * 0.6 }
                } else {
                    -1.0
                };
                if best.is_none_or(|b| v2 > b.0) {
                    best = Some((v2, Def::Walker(wk.id)));
                }
            }
        }
        if let Some(wh) = why.as_deref_mut() {
            let verdict = match best {
                Some((v, d)) if v > 0.2 || g.must_attack(a) => format!(
                    "attacks {}{}",
                    match d {
                        Def::Player(o) => format!("P{}", o),
                        Def::Walker(id) => g.id_desc(id),
                    },
                    if v > 0.2 { "" } else { " (must attack)" }
                ),
                _ => "stays home".into(),
            };
            wh.push(format!(
                "alone: {} gains [{}] -> {}",
                name(a),
                gains_s.join(", "),
                verdict
            ));
        }
        if let Some((v, d)) = best {
            if v > 0.2 || g.must_attack(a) {
                if let Def::Player(o) = d {
                    spent += tax_of(o);
                    match sent.iter_mut().find(|s| s.0 == o) {
                        Some(s) => {
                            s.1.push(a);
                            s.2 += v;
                        }
                        None => sent.push((o, vec![a], v)),
                    }
                }
                out.push((at.id, d));
                gains.push((v, a));
            }
        }
    }
    // Crack-back safety: what could hit us before our next turn.
    let my_life = g.players[me].life;
    let sw = swings(g, &opps);
    // Keep creatures home (least useful attackers first) until the crack-back leaves us
    // above a quarter of our life (at least 5), counting what our lifelinkers gain. When even
    // keeping everyone home can't do that, keep home just what stops a lethal swing; when
    // nothing can, keep no one home. Then the attackers left are checked again (the
    // blockers the others drew are free now).
    let committed: Vec<(u32, Def)> = out
        .iter()
        .copied()
        .filter(|(id, _)| lethal_ids.contains(id) || g.idx(*id).is_some_and(|i| g.must_attack(i)))
        .collect();
    // Our lifelink attackers' gains: in full when nothing of theirs can block it.
    let lifelink = |out: &[(u32, Def)]| -> f32 {
        out.iter()
            .filter_map(|(id, d)| g.idx(*id).map(|i| (i, *d)))
            .filter(|(i, _)| g.perms[*i].has(LIFELINK))
            .map(|(i, d)| {
                let o = g.def_player(d).unwrap_or(me);
                let open = !g.perms.iter().enumerate().any(|(b, x)| {
                    x.ctrl as usize == o && x.is_creature() && !x.tapped && g.can_block(b, i)
                });
                dmg_of(g, &g.perms[i]) as f32 * if open { 1.0 } else { 0.3 }
            })
            .sum()
    };
    // Creatures we can cast after combat will block too.
    let hand_def = castable_blockers(g, me);
    let mut floor = 5f32.max(my_life as f32 * 0.25);
    let worst =
        my_life as f32 + lifelink(&committed) - crack_back(g, me, &sw, &committed, &hand_def);
    if worst < floor {
        floor = 1.0;
    }
    if worst < 1.0 {
        // Even keeping everyone home can't stop the expected crack-back: holding back only
        // slows our race.
        floor = f32::MIN;
        if let Some(wh) = why.as_deref_mut() {
            wh.push(format!(
                "no holding back: crack-back {:.1} beats life {} anyway",
                crack_back(g, me, &sw, &committed, &hand_def),
                my_life
            ));
        }
    }
    gains.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
    let mut k = 0;
    let before_trim = out.len();
    while k < gains.len() {
        let incoming = crack_back(g, me, &sw, &out, &hand_def) - lifelink(&out);
        if my_life as f32 - incoming >= floor {
            break;
        }
        let a = gains[k].1;
        let id = g.perms[a].id;
        if !g.must_attack(a) && !lethal_ids.contains(&id) {
            if let Some(wh) = why.as_deref_mut() {
                wh.push(format!(
                    "keeps {} home: crack-back {:.1} vs life {}",
                    name(a),
                    incoming,
                    my_life
                ));
            }
            out.retain(|(x, _)| *x != id);
        }
        k += 1;
    }
    if out.len() < before_trim {
        // Some attackers stayed home, so the blockers they would have drawn are free again:
        // check the rest of each group against the new block prediction.
        loop {
            let mut changed = false;
            for &o in &opps {
                let group: Vec<usize> = out
                    .iter()
                    .filter(|(id, d)| {
                        *d == Def::Player(o) && !lethal_ids.contains(id) && !spares.contains(id)
                    })
                    .filter_map(|(id, _)| g.idx(*id))
                    .collect();
                if group.is_empty() {
                    continue;
                }
                let (_, bad, _) = group_value(g, o, w_of(o), &group);
                for a in bad {
                    let id = g.perms[a].id;
                    if let Some(wh) = why.as_deref_mut() {
                        wh.push(format!(
                            "re-check: {} would now die for less, stays home too",
                            name(a)
                        ));
                    }
                    out.retain(|(x, _)| *x != id);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }
    out
}

/// After blocks: pump or protect a creature to win a fight, or push lethal damage.
pub fn combat_tricks(g: &mut Game, p: usize, attacks: &[Attack]) {
    combat_removal(g, p, attacks);
    let pl = &g.players[p];
    let tricks: Vec<usize> = (0..pl.hand.len())
        .filter(|&h| {
            is_trick(&pl.cards[pl.hand[h] as usize])
                && g.can_cast(p, Src::Hand(h), CastOpts::default())
        })
        .collect();
    for h in tricks {
        if try_trick(g, p, h, attacks) {
            return;
        }
    }
}

/// Casts the trick in hand slot `h` if it wins a fight or pushes lethal damage.
fn try_trick(g: &mut Game, p: usize, h: usize, attacks: &[Attack]) -> bool {
    let c = &g.players[p].cards[g.players[p].hand[h] as usize];
    let (dp, dt, kw) = effects_of(c, false).fold((0i16, 0i16, 0u32), |acc, e| match e {
        Effect::Pump {
            power,
            toughness,
            until_eot: true,
            ..
        } => (acc.0 + *power as i16, acc.1 + *toughness as i16, acc.2),
        Effect::Grant {
            keywords,
            until_eot: true,
            ..
        } => (acc.0, acc.1, acc.2 | keyword_bits(keywords)),
        _ => acc,
    });
    for at in attacks {
        let Some(a) = g.idx(at.attacker) else {
            continue;
        };
        let blockers: Vec<usize> = at.blockers.iter().filter_map(|b| g.idx(*b)).collect();
        let a_mine = g.perms[a].ctrl as usize == p;
        for &b in &blockers {
            let (mine, theirs) = if a_mine {
                (a, b)
            } else if g.perms[b].ctrl as usize == p {
                (b, a)
            } else {
                continue;
            };
            let before = if a_mine {
                fight(&g.perms[mine], &g.perms[theirs])
            } else {
                let (x, y) = fight(&g.perms[theirs], &g.perms[mine]);
                (y, x)
            };
            let mut boosted = g.perms[mine].clone();
            boosted.p += dp;
            boosted.t += dt;
            boosted.eff_kw |= kw;
            let after = if a_mine {
                fight(&boosted, &g.perms[theirs])
            } else {
                let (x, y) = fight(&g.perms[theirs], &boosted);
                (y, x)
            };
            // (my creature dies, theirs dies)
            let better = (before.0 && !after.0) || (!before.1 && after.1);
            if better && perm_value(g, theirs) + perm_value(g, mine) >= 3.0 {
                let id = g.perms[mine].id;
                cast_instant(g, p, h, Some(id), "combat trick: wins the fight");
                return true;
            }
        }
        // Unblocked: pump for lethal.
        if a_mine && blockers.is_empty() && dp > 0 {
            if let Some(d) = g.def_player(at.def) {
                let total: i32 = attacks
                    .iter()
                    .filter(|x| x.blockers.is_empty() && g.def_player(x.def) == Some(d))
                    .filter_map(|x| g.idx(x.attacker))
                    .map(|i| dmg_of(g, &g.perms[i]))
                    .sum();
                if total < g.players[d].life && total + dp as i32 >= g.players[d].life {
                    let id = g.perms[a].id;
                    cast_instant(g, p, h, Some(id), "combat trick: pump for lethal");
                    return true;
                }
            }
        }
    }
    false
}
