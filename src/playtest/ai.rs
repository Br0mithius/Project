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
use super::{CastOpts, ExtraPay, Src};
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
        let e: f32 = if a.modal {
            a.effects.iter().map(effect_value).fold(0.0, f32::max)
        } else {
            a.effects.iter().map(effect_value).sum()
        };
        let times = match &a.trig {
            Trig::Phase { .. } => 2.5,
            Trig::Event(..) => 1.5,
            Trig::Attacks | Trig::CombatDamage => 1.2,
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

fn perm_value_uncached(g: &Game, i: usize) -> f32 {
    let x = &g.perms[i];
    let abilities = g.abilities_of(x);
    let mut v = match x.tok {
        Tok::Treasure => return 0.8,
        Tok::Clue => return 1.0,
        Tok::Food | Tok::Map => return 0.5,
        Tok::Blood => return 0.4,
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
    v += engine_value(abilities) * if x.locked { 0.5 } else { 1.0 };
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

/// How threatening a player is to the rest of the table.
pub fn threat(g: &Game, o: usize) -> f32 {
    if !g.players[o].alive {
        return 0.0;
    }
    if o < 8 {
        let (t, v) = g.tcache.borrow()[o];
        if t == g.tick {
            return v;
        }
    }
    let v = threat_uncached(g, o);
    if o < 8 {
        g.tcache.borrow_mut()[o] = (g.tick, v);
    }
    v
}

fn threat_uncached(g: &Game, o: usize) -> f32 {
    let board: f32 = (0..g.perms.len())
        .filter(|&i| g.perms[i].ctrl as usize == o && !g.perms[i].is(T_LAND))
        .map(|i| perm_value(g, i))
        .sum();
    board + g.players[o].hand.len() as f32 * 0.4 + g.players[o].life as f32 * 0.05
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
    let opps = g.opponents(me);
    let max = opps.iter().map(|&q| threat(g, q)).fold(0.1, f32::max);
    let low = if g.players[o].life <= 10 { 0.4 } else { 0.0 };
    1.0 + 0.7 * threat(g, o) / max + (g.players[me].grudge[o] / 15.0).min(0.5) + low
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
                weight(g, me, owner)
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

pub fn blink_choice(g: &Game, me: usize, not: Option<u32>) -> Option<usize> {
    (0..g.perms.len())
        .filter(|&i| {
            let x = &g.perms[i];
            x.ctrl as usize == me
                && x.tok == Tok::None
                && x.commander == 0
                && x.card.is_some()
                && Some(x.id) != not
        })
        .map(|i| (arrival_value(g.card(g.perms[i].card.unwrap())), i))
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

pub fn aura_target(g: &Game, p: usize, idx: u16) -> Option<usize> {
    let c = &g.players[p].cards[idx as usize];
    let creatures: Vec<usize> = (0..g.perms.len())
        .filter(|&i| {
            g.perms[i].is_creature()
                && !(g.perms[i].ctrl as usize != p && g.perms[i].has(HEXPROOF | SHROUD))
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
fn card_worth(g: &Game, p: usize, c: &SimCard) -> f32 {
    if c.is(T_LAND) {
        return if lands_total(g, p) >= 6 { 0.3 } else { 2.5 };
    }
    let lands = lands_in_play(g, p) as f32;
    c.value - 0.4 * (c.mv as f32 - lands - 2.0).max(0.0)
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

/// Look at the top N, take the best, the rest go to the bottom.
pub fn dig(g: &mut Game, p: usize, n: usize, lands_ok: bool) {
    let n = n.min(g.players[p].library.len());
    if n == 0 {
        return;
    }
    let len = g.players[p].library.len();
    let top: Vec<u16> = g.players[p].library.drain(len - n..).collect();
    let best = (0..top.len()).max_by(|&a, &b| {
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
    let want = |c: &SimCard| {
        if text.contains("creature card") {
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

fn keepable(g: &Game, p: usize) -> bool {
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
    (2..=5).contains(&lands) && (early >= 1 || lands >= 3)
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
        if keepable(g, p) || attempt == 2 {
            // Bottom one card per mulligan after the free one.
            let bottom = attempt.saturating_sub(1);
            for _ in 0..bottom {
                if let Some(h) = discard_choice(g, p) {
                    let c = g.players[p].hand.remove(h);
                    g.players[p].library.insert(0, c);
                }
            }
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

/// Cards worth casting only in response (counters, Fogs, protection, combat tricks).
fn is_reactive_only(c: &SimCard) -> bool {
    c.is(T_INSTANT)
        && effects_of(c, false).all(|e| {
            matches!(e, Effect::CounterSpell | Effect::Fog)
                || matches!(e, Effect::Pump { until_eot: true, to, .. } | Effect::Grant { until_eot: true, to, .. } if !to.mass)
        })
}

/// What an effect would do for player `p` right now (card points).
fn effect_now(
    g: &Game,
    p: usize,
    e: &Effect,
    text: &str,
    src: Option<u32>,
    card_restr: Restr,
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
    };
    let d = g.dyn_values(&card_restr, Some(&ctx), p, src);
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
            cands
                .iter()
                .filter(|&&i| g.perms[i].ctrl as usize != p && kill(&g.perms[i]))
                .map(|&i| perm_value(g, i) * weight(g, p, g.perms[i].ctrl as usize) * 0.8)
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
        Effect::Edict { .. } => g
            .opponents(p)
            .iter()
            .filter_map(|&o| sacrifice_choice_r(g, o, T_CREATURE, &card_restr))
            .map(|i| perm_value(g, i) * 0.8)
            .sum(),
        Effect::CounterSpell | Effect::Fog => 0.0,
        Effect::Draw { amount, .. } => {
            1.2 * amount.n.max(1) as f32
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
        Effect::Blink => blink_choice(g, p, src.filter(|_| text.contains("target")))
            .map(|i| arrival_value(g.card(g.perms[i].card.unwrap())) * 0.8)
            .unwrap_or(0.0),
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
        // Worth it before our attack, when an opponent has a blocker to stop.
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
                    .map(|e| effect_now(g, p, e, text, src, cr))
                    .sum()
            })
            .collect();
        v.sort_by(|a, b| b.partial_cmp(a).unwrap());
        v.iter().take(if two { 2 } else { 1 }).sum()
    } else {
        a.effects
            .iter()
            .map(|e| effect_now(g, p, e, &a.text, src, a.card_restr))
            .sum()
    }
}

/// The modes a modal ability picks: the best one, or two (`used`: a bit per mode already
/// chosen, for "choose one that hasn't been chosen").
pub fn choose_modes(g: &Game, ctx: &Ctx, ab: &SimAbility, used: u32) -> Vec<usize> {
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
    let chosen: Vec<usize> = v
        .into_iter()
        .take(if two { 2 } else { 1 })
        .map(|(_, m)| m)
        .collect();
    chosen
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
    let gain: f32 = ab
        .effects
        .iter()
        .skip(pc.first as usize)
        .map(|e| effect_now(g, p, e, &ab.text, ctx.src, ab.card_restr))
        .sum();
    let src = ctx.src.unwrap_or(u32::MAX);
    let pl = &g.players[p];
    let price = match &pc.cost {
        ResCost::Free | ResCost::SacrificeSelf | ResCost::ExileSelf | ResCost::ExileNamed(_) => {
            return gain >= 0.0;
        }
        ResCost::Sacrifice { types, other } => {
            match fodder(g, p, *types, if *other { src } else { u32::MAX }) {
                Some(i) => perm_value(g, i) * 0.8,
                None => return false,
            }
        }
        ResCost::SacrificeOrDiscard(types) => match fodder(g, p, *types, src) {
            Some(i) => perm_value(g, i) * 0.8,
            None if !pl.hand.is_empty() => 1.0,
            None => return false,
        },
        ResCost::Mana(m) => {
            if !g.can_pay(p, m) {
                return false;
            }
            m.total() as f32 * 0.3
        }
        ResCost::Life(n) => {
            if pl.life <= *n as i32 + 5 {
                return false;
            }
            *n as f32 * if pl.life < 15 { 0.3 } else { 0.1 }
        }
        ResCost::Discard => {
            if pl.hand.is_empty() {
                return false;
            }
            discard_choice(g, p)
                .map(|h| card_worth(g, p, &pl.cards[pl.hand[h] as usize]) * 0.5)
                .unwrap_or(1.0)
        }
        ResCost::RemoveCounter { plus } => {
            let Some(x) = ctx.src.and_then(|id| g.idx(id)).map(|i| &g.perms[i]) else {
                return false;
            };
            if *plus {
                // Not the last counter keeping it alive.
                if x.counters < 1 || x.t <= 1 {
                    return false;
                }
                0.8
            } else {
                if x.other_counters < 1 {
                    return false;
                }
                0.0
            }
        }
    };
    gain > price
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
        if c.mana_ability && !c.is(T_LAND) {
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
            if aura_target(g, p, idx).is_none() {
                return 0.0;
            }
            if c.harmful_aura {
                if let Some(t) = aura_target(g, p, idx) {
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
                v += ability_now(g, p, a, None);
            }
        }
        if commander {
            v += 1.5;
        }
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
                v += ability_now(g, p, a, None);
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
}

/// Lands: play the one that fixes what the hand needs; tapped lands on quiet turns.
fn choose_land(g: &Game, p: usize) -> Option<usize> {
    let pl = &g.players[p];
    let missing = missing_colors(g, p);
    let mana = g.mana_available(p) as u8;
    let cheapest_spell = pl
        .hand
        .iter()
        .map(|&c| &pl.cards[c as usize])
        .filter(|c| !c.is(T_LAND))
        .map(|c| c.mv)
        .min()
        .unwrap_or(99);
    (0..pl.hand.len())
        .filter(|&h| pl.cards[pl.hand[h] as usize].is(T_LAND))
        .max_by_key(|&h| {
            let c = &pl.cards[pl.hand[h] as usize];
            let fixes = (c.produces & missing).count_ones() as i32 * 10;
            let tapped_pen = if c.enters_tapped && cheapest_spell <= mana + 1 {
                -6
            } else {
                0
            };
            fixes + tapped_pen + c.produces.count_ones() as i32
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
    if (c.convoke || c.improvise) && post {
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
            if !g.can_pay(p, &cost) {
                // Try without the extras.
                let plain = CastOpts {
                    back,
                    ..Default::default()
                };
                match g.cast_cost(p, src, plain) {
                    Some(c2) if g.can_pay(p, &c2) => {
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
            out.push((v, cost.total(), Act::Cast(src, opts)));
        };
    for (h, &idx) in pl.hand.iter().enumerate() {
        let c = &pl.cards[idx as usize];
        if c.is(T_LAND) {
            continue;
        }
        push_cast(&mut out, Src::Hand(h), idx, false, false);
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
                    if g.can_pay(p, &cost) {
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
            let Some(cost) = g.activation_cost(p, i, k) else {
                continue;
            };
            if !g.can_pay(p, &cost) {
                continue;
            }
            let mut v = ability_now(g, p, a, Some(x.id));
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
                    ..
                } => {
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
                Trig::PowerUp(_) => {
                    // Cheapest the turn it enters.
                    if x.born == g.round {
                        v += 0.5;
                    }
                }
                _ => {}
            }
            out.push((
                v - 0.1 * cost.total() as f32,
                cost.total(),
                Act::Activate(x.id, k),
            ));
        }
        if let Some((v, k)) = best_loyalty {
            out.push((v.max(0.3), 0, Act::Activate(x.id, k)));
        }
        // Equip: move gear to the best attacker.
        if x.is(T_EQUIPMENT) && (!post || x.attached.is_none()) {
            if let (Some(r), Some(t)) = (x.card, equip_target(g, p, x.id)) {
                let tid = g.perms[t].id;
                let cost = g.equip_cost(p, x.id, tid).unwrap_or(Mana::generic(2));
                let life = g.card(r).equip_life as i32;
                if x.attached != Some(tid)
                    && x.attached.is_none_or(|a| g.idx(a).is_none())
                    && g.can_pay(p, &cost)
                    && (life == 0 || g.players[p].life > life + 5)
                {
                    let bonus = g
                        .card(r)
                        .abilities
                        .iter()
                        .filter(|a| a.trig == Trig::Static || a.text.contains("equipped"))
                        .flat_map(|a| a.effects.iter())
                        .map(effect_value)
                        .sum::<f32>();
                    out.push((1.0 + bonus, cost.total(), Act::Equip(x.id, tid)));
                }
            }
        }
    }
    out
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
    let c = &g.players[p].cards[idx as usize];
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

fn plan(options: &[(f32, u32, Act)], budget: u32) -> Option<(usize, f32)> {
    plan_where(options, budget, &|_| true)
}

/// The best set of actions for the mana (a small knapsack); returns the most valuable of
/// them that `now` allows doing at this point, and the whole set's value.
fn plan_where(
    options: &[(f32, u32, Act)],
    budget: u32,
    now: &dyn Fn(usize) -> bool,
) -> Option<(usize, f32)> {
    let items: Vec<usize> = (0..options.len()).filter(|&i| options[i].0 > 0.3).collect();
    if items.is_empty() {
        return None;
    }
    let b = budget.min(30) as usize;
    // dp[m] = (value, chosen items as a bitmask over `items`, up to 64 items)
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
    // Do the chosen action that's most urgent: removal / ramp first, then the biggest.
    let first = items
        .iter()
        .enumerate()
        .take(64)
        .filter(|(k, _)| mask & (1 << k) != 0)
        .map(|(_, &i)| i)
        .filter(|&i| now(i))
        .max_by(|&a, &b| options[a].0.partial_cmp(&options[b].0).unwrap())?;
    Some((first, total))
}

fn do_act(g: &mut Game, p: usize, act: Act) -> bool {
    match act {
        Act::Cast(src, opts) => g.cast(p, src, opts),
        Act::Activate(id, k) => g.activate(p, id, k),
        Act::ActivateGy(gk, k) => g.activate_from_graveyard(p, gk, k),
        Act::Equip(e, t) => g.equip(p, e, t),
        Act::Cycle(h) => g.cycle(p, h),
    }
}

/// Mana to keep open for counterspells and instant-speed answers, if that's worth more
/// than what it would cost to tap out.
fn holdup(g: &Game, p: usize, opts: &[(f32, u32, Act)], avail: u32) -> u32 {
    let pl = &g.players[p];
    let best = pl
        .hand
        .iter()
        .map(|&c| &pl.cards[c as usize])
        .filter(|c| c.is(T_INSTANT) || has_flash(g, p, c))
        .map(|c| (reactive_value(g, p, c), c.cost.total()))
        .filter(|(v, _)| *v >= 1.5)
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let Some((value, cost)) = best else { return 0 };
    if cost > avail {
        return 0;
    }
    let full = plan(opts, avail).map(|x| x.1).unwrap_or(0.0);
    let held = plan(opts, avail - cost).map(|x| x.1).unwrap_or(0.0);
    if held + value * 0.6 >= full { cost } else { 0 }
}

/// A main phase: land, then the best plan for the mana, re-planned after each action.
pub fn main_phase(g: &mut Game, p: usize, post: bool) {
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
        let opts: Vec<(f32, u32, Act)> = options(g, p, post)
            .into_iter()
            .filter(|o| !failed.contains(&format!("{:?}", o.2)))
            .collect();
        if opts.is_empty() {
            return;
        }
        let avail = g.mana_available(p);
        if g.log.is_some() && std::env::var("MSH_SIM_DEBUG").is_ok() {
            let list: Vec<String> = opts
                .iter()
                .map(|o| {
                    let what = match o.2 {
                        Act::Cast(Src::Hand(h), _) => g.players[p].cards
                            [g.players[p].hand[h] as usize]
                            .name
                            .clone(),
                        other => format!("{:?}", other).chars().take(30).collect(),
                    };
                    format!("{} v{:.1} c{}", what, o.0, o.1)
                })
                .collect();
            g.note(|| format!("   P{} options ({} mana): {}", p, avail, list.join(" | ")));
        }
        let reserve = holdup(g, p, &opts, avail);
        if reserve > 0 {
            g.note(|| format!("AI P{} holds up {} mana", p, reserve));
        }
        // The whole turn is planned together; before combat, only its pre-combat parts happen.
        let Some((i, _)) = plan_where(&opts, avail.saturating_sub(reserve), &|i| {
            post || !waits_for_combat(g, p, &opts[i].2)
        }) else {
            return;
        };
        let act = opts[i].2;
        let ok = do_act(g, p, act);
        if g.log.is_some() && std::env::var("MSH_SIM_DEBUG").is_ok() {
            g.note(|| format!("   P{} did {:?}: {}", p, act, ok));
        }
        if !ok {
            failed.push(format!("{:?}", act));
        }
    }
}

// ------------------------------------------------------------ instant speed

/// Casts an instant from hand aimed at `focus` (if any). True if it was cast.
fn cast_instant(g: &mut Game, p: usize, h: usize, focus: Option<u32>) -> bool {
    let name = g.players[p].cards[g.players[p].hand[h] as usize]
        .name
        .clone();
    g.note(|| format!("AI P{} instant-speed: {}", p, name));
    g.focus = focus;
    let ok = g.cast(p, Src::Hand(h), CastOpts::default());
    g.focus = None;
    ok
}

fn hand_find(g: &Game, p: usize, f: impl Fn(&SimCard) -> bool) -> Option<usize> {
    let pl = &g.players[p];
    (0..pl.hand.len()).find(|&h| {
        let c = &pl.cards[pl.hand[h] as usize];
        f(c) && g
            .cast_cost(p, Src::Hand(h), CastOpts::default())
            .is_some_and(|m| g.can_pay(p, &m))
    })
}

/// How bad it would be for `o` if the spell resolves.
fn spell_threat(g: &Game, o: usize, caster: usize, idx: u16, back: bool) -> f32 {
    let c = &g.players[caster].cards[idx as usize];
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

/// Opponents may counter a spell `caster` just cast. True if it was countered.
pub fn respond_to_spell(g: &mut Game, caster: usize, idx: u16, back: bool) -> bool {
    let n = g.players.len();
    let spell = &g.players[caster].cards[idx as usize];
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
        let threshold = if counters_in_hand > 1 { 3.5 } else { 4.5 };
        if spell_threat(g, o, caster, idx, back) < threshold {
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
        if !g.pay(o, &cost) {
            continue;
        }
        g.players[o].hand.remove(h);
        g.players[o].graveyard.push(counter_idx);
        g.on_cast(o, counter);
        // Other effects on the counterspell (draw a card, etc.).
        for (k2, a) in counter.abilities.iter().enumerate() {
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
        g.note(|| format!("AI P{} counters it", o));
        // "Unless its controller pays {N}".
        if let Some(i) = text.find("pays {") {
            let n: u32 = text[i + 6..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .unwrap_or(2);
            if g.pay(caster, &Mana::generic(n)) {
                return false;
            }
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
    cast_instant(g, owner, h, Some(id))
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
            cast_instant(g, d, h, None);
            return;
        }
    }
    // Instant removal on the biggest attacker coming at us.
    if let Some(&(id, pw)) = incoming.iter().max_by_key(|x| x.1) {
        if pw >= 4 || pw as i32 * 3 >= life {
            if let Some(h) = hand_find(g, d, is_instant_removal) {
                cast_instant(g, d, h, Some(id));
            }
        }
    }
    // Flash creatures as surprise blockers.
    if let Some(h) = hand_find(g, d, |c| c.is(T_CREATURE) && has_flash(g, d, c)) {
        g.cast(d, Src::Hand(h), CastOpts::default());
    }
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
            if !g
                .cast_cost(p, Src::Hand(h), CastOpts::default())
                .is_some_and(|m| g.can_pay(p, &m))
            {
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
                if !matches!(a.trig, Trig::Activated { sorcery: false, .. }) {
                    continue;
                }
                let Some(cost) = g.activation_cost(p, i, k) else {
                    continue;
                };
                if !g.can_pay(p, &cost) {
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
            x.ctrl as usize == a && x.is(T_VEHICLE) && !x.crewed && !x.tapped && x.base_p >= 3
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

fn dmg_of(g: &Game, x: &Perm) -> i32 {
    g.combat_power(x).max(0) as i32 * if x.has(DOUBLE_STRIKE) { 2 } else { 1 }
}

/// How `d` blocks: kill-and-survive first, good trades, double blocks on big threats,
/// chump blocks only to stay alive. Careful about tricks when the attacker has mana up.
pub fn declare_blocks(g: &Game, d: usize, attacks: &[Attack]) -> Vec<(u32, u32)> {
    let mut used: Vec<u32> = Vec::new();
    let mut out = Vec::new();
    let mut atts: Vec<usize> = attacks.iter().filter_map(|at| g.idx(at.attacker)).collect();
    atts.sort_by(|&x, &y| perm_value(g, y).partial_cmp(&perm_value(g, x)).unwrap());
    let blockers: Vec<usize> = (0..g.perms.len())
        .filter(|&i| g.perms[i].ctrl as usize == d && g.perms[i].is_creature())
        .collect();
    let attacker_ctrl = atts.first().map(|&i| g.perms[i].ctrl as usize).unwrap_or(0);
    let wary = g.mana_available(attacker_ctrl) >= 2 && !g.players[attacker_ctrl].hand.is_empty();
    let mut unblocked: Vec<usize> = Vec::new();
    for &a in &atts {
        let at = &g.perms[a];
        let menace = at.has(MENACE);
        let can: Vec<usize> = blockers
            .iter()
            .copied()
            .filter(|&b| !used.contains(&g.perms[b].id) && g.can_block(b, a))
            .collect();
        // 1. Kill it and live (with a margin if they might have a trick).
        let safe = can
            .iter()
            .copied()
            .filter(|&b| {
                let (ad, bd) = fight(at, &g.perms[b]);
                let margin_ok =
                    !wary || g.perms[b].t - at.p >= 2 || g.perms[b].has(FIRST_STRIKE | DEATHTOUCH);
                ad && !bd && margin_ok
            })
            .min_by(|&x, &y| perm_value(g, x).partial_cmp(&perm_value(g, y)).unwrap());
        if let (Some(b), false) = (safe, menace) {
            used.push(g.perms[b].id);
            out.push((g.perms[b].id, at.id));
            continue;
        }
        // 2. A trade that favours us.
        let av = perm_value(g, a);
        let trade = can
            .iter()
            .copied()
            .filter(|&b| {
                let (ad, _) = fight(at, &g.perms[b]);
                ad && perm_value(g, b) + 0.5 <= av
            })
            .min_by(|&x, &y| perm_value(g, x).partial_cmp(&perm_value(g, y)).unwrap());
        if let (Some(b), false) = (trade, menace) {
            used.push(g.perms[b].id);
            out.push((g.perms[b].id, at.id));
            continue;
        }
        // 3. Double block a big threat (also answers menace).
        if av >= 5.0 && can.len() >= 2 {
            let mut pair: Option<(usize, usize, f32)> = None;
            for (x, &b1) in can.iter().enumerate() {
                for &b2 in &can[x + 1..] {
                    let (p1, p2) = (&g.perms[b1], &g.perms[b2]);
                    let kills =
                        p1.p + p2.p >= at.t - at.damage || p1.has(DEATHTOUCH) || p2.has(DEATHTOUCH);
                    if !kills || at.has(INDESTRUCTIBLE) {
                        continue;
                    }
                    // The attacker kills at most one (the cheaper we choose to lose).
                    let loss = perm_value(g, b1).min(perm_value(g, b2));
                    if loss < av && pair.is_none_or(|p| loss < p.2) {
                        pair = Some((b1, b2, loss));
                    }
                }
            }
            if let Some((b1, b2, _)) = pair {
                used.push(g.perms[b1].id);
                used.push(g.perms[b2].id);
                out.push((g.perms[b1].id, at.id));
                out.push((g.perms[b2].id, at.id));
                continue;
            }
        }
        unblocked.push(a);
    }
    // 4. Chump only if the unblocked damage would kill us (life, commander damage, poison).
    let pl = &g.players[d];
    let lethal = |un: &[usize]| {
        let dmg: i32 = un.iter().map(|&a| dmg_of(g, &g.perms[a])).sum();
        let poison: i32 = un
            .iter()
            .filter_map(|&a| g.perms[a].card.map(|r| g.card(r).toxic as i32))
            .sum();
        let cmd = un.iter().any(|&a| {
            let x = &g.perms[a];
            x.commander > 0
                && pl.cmd_damage[x.ctrl as usize][x.commander as usize % 3] + dmg_of(g, x) >= 21
        });
        dmg >= pl.life || pl.poison as i32 + poison >= 10 || cmd
    };
    unblocked.sort_by_key(|&a| -dmg_of(g, &g.perms[a]));
    let mut k = 0;
    while lethal(&unblocked[k..]) && k < unblocked.len() {
        let a = unblocked[k];
        let at = &g.perms[a];
        let chump = blockers
            .iter()
            .copied()
            .filter(|&b| !used.contains(&g.perms[b].id) && g.can_block(b, a))
            .min_by(|&x, &y| perm_value(g, x).partial_cmp(&perm_value(g, y)).unwrap());
        match chump {
            Some(b) if !at.has(MENACE) => {
                used.push(g.perms[b].id);
                out.push((g.perms[b].id, at.id));
            }
            _ => {}
        }
        k += 1;
    }
    out
}

/// Expected result of attacking opponent `o` with attacker `a`: positive is good for us.
fn attack_gain(g: &Game, me: usize, a: usize, o: usize, taken: &[u32]) -> f32 {
    let at = &g.perms[a];
    let av = perm_value(g, a);
    let w = weight(g, me, o);
    let blockers: Vec<usize> = (0..g.perms.len())
        .filter(|&b| {
            g.perms[b].ctrl as usize == o && !taken.contains(&g.perms[b].id) && g.can_block(b, a)
        })
        .collect();
    let menace = at.has(MENACE) && blockers.len() < 2;
    let mut worst = dmg_of(g, at) as f32 * 0.6 * w; // unblocked
    if !menace {
        for &b in &blockers {
            let (ad, bd) = fight(at, &g.perms[b]);
            let bv = perm_value(g, b);
            let outcome = match (ad, bd) {
                (true, false) => -av,                      // they eat it
                (true, true) if bv + 0.5 <= av => bv - av, // they trade up
                (false, false) => 0.0,                     // they block safely
                _ => worst,                                // blocking is bad for them
            };
            if outcome < worst {
                worst = outcome;
            }
        }
    }
    worst
}

/// Attacks for the active player: lethal swings first, then every attack that pays off,
/// keeping enough back to survive the next turn around the table.
pub fn declare_attacks(g: &Game, me: usize) -> Vec<(u32, Def)> {
    let mut ready: Vec<usize> = (0..g.perms.len())
        .filter(|&i| g.perms[i].ctrl as usize == me && g.can_attack(i))
        .collect();
    if ready.is_empty() {
        return Vec::new();
    }
    ready.sort_by(|&x, &y| g.perms[y].p.cmp(&g.perms[x].p));
    let opps = g.opponents(me);
    // Lethal: everything at one player whose blockers can't stop enough of it.
    for &o in &opps {
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
        // They'll block the biggest attackers that can be blocked.
        let mut through: i32 = 0;
        let mut nb = blockers.len();
        for &a in &ready {
            let at = &g.perms[a];
            let evasive = at.has(FLYING | UNBLOCKABLE)
                && !(0..g.perms.len()).any(|b| g.perms[b].ctrl as usize == o && g.can_block(b, a));
            if evasive || nb == 0 {
                through += dmg_of(g, at);
            } else {
                nb -= 1;
                if at.has(TRAMPLE) {
                    through += (at.p - blockers.get(nb).copied().unwrap_or(0)).max(0) as i32;
                }
            }
        }
        if through >= g.players[o].life {
            return ready
                .iter()
                .map(|&a| (g.perms[a].id, Def::Player(o)))
                .collect();
        }
    }
    let goad_ok = |a: usize, o: usize| match g.perms[a].goad {
        Some(gd) if opps.len() > 1 => gd as usize != o,
        _ => true,
    };
    // Group plan: send a group at one opponent, predict their blocks with the blocking AI,
    // drop attackers that would just die, and keep the best opponent's plan.
    let mut best_plan: Option<(f32, usize, Vec<usize>)> = None;
    // Attack taxes (Propaganda): only as many attackers as the mana pays for, and the mana
    // isn't free.
    let mana = g.mana_available(me);
    let tax_of = |o: usize| g.attack_tax(o);
    for &o in &opps {
        let w = weight(g, me, o);
        let mut group: Vec<usize> = ready.iter().copied().filter(|&a| goad_ok(a, o)).collect();
        let tax = tax_of(o);
        if tax > 0 {
            group.truncate((mana / tax) as usize);
        }
        for _ in 0..4 {
            if group.is_empty() {
                break;
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
            let blocks = declare_blocks(g, o, &attacks);
            let mut value = 0.0;
            let mut bad = Vec::new();
            for &a in &group {
                let at = &g.perms[a];
                let av = perm_value(g, a);
                let bl: Vec<usize> = blocks
                    .iter()
                    .filter(|(_, x)| *x == at.id)
                    .filter_map(|(b, _)| g.idx(*b))
                    .collect();
                if bl.is_empty() {
                    value += dmg_of(g, at) as f32 * 0.6 * w;
                    continue;
                }
                // Blocked: what dies on each side.
                let (dies, killed) = if bl.len() == 1 {
                    let (ad, bd) = fight(at, &g.perms[bl[0]]);
                    (ad, if bd { perm_value(g, bl[0]) } else { 0.0 })
                } else {
                    let total: i16 = bl.iter().map(|&b| g.perms[b].p.max(0)).sum();
                    let dt = bl.iter().any(|&b| g.perms[b].has(DEATHTOUCH));
                    let dies = !at.has(INDESTRUCTIBLE) && (dt || total >= at.t - at.damage);
                    let first = bl[0];
                    let kills = at.has(DEATHTOUCH) || at.p >= g.perms[first].t;
                    (dies, if kills { perm_value(g, first) } else { 0.0 })
                };
                let trample = if at.has(TRAMPLE) {
                    (at.p - bl.iter().map(|&b| g.perms[b].t).sum::<i16>()).max(0) as f32 * 0.6 * w
                } else {
                    0.0
                };
                let net = killed + trample - if dies { av } else { 0.0 };
                if dies && net < 0.0 && at.goad.is_none() {
                    bad.push(a);
                }
                value += net;
            }
            value -= tax as f32 * 0.35 * group.len() as f32;
            if bad.is_empty() {
                if value > 0.2 && best_plan.as_ref().is_none_or(|b| value > b.0) {
                    best_plan = Some((value, o, group.clone()));
                }
                break;
            }
            group.retain(|a| !bad.contains(a));
        }
    }
    let mut out: Vec<(u32, Def)> = Vec::new();
    let mut gains: Vec<(f32, usize)> = Vec::new();
    if let Some((v, o, group)) = &best_plan {
        let each = v / group.len().max(1) as f32;
        for &a in group {
            out.push((g.perms[a].id, Def::Player(*o)));
            gains.push((each, a));
        }
    }
    // Everyone else: one at a time, at another player or a planeswalker.
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
        for &o in &choices {
            let tax = tax_of(o);
            let v = attack_gain(g, me, a, o, &[]) - tax as f32 * 0.35;
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
        if let Some((v, d)) = best {
            if v > 0.2 || at.goad.is_some() {
                if let Def::Player(o) = d {
                    spent += tax_of(o);
                }
                out.push((at.id, d));
                gains.push((v, a));
            }
        }
    }
    // Crack-back safety: what could hit us before our next turn.
    let my_life = g.players[me].life;
    let incoming: i32 = opps
        .iter()
        .map(|&o| {
            let others = g.opponents(o).len().max(1) as f32;
            let power: i32 = g
                .perms
                .iter()
                .filter(|x| x.ctrl as usize == o && x.is_creature() && !x.has(DEFENDER))
                .map(|x| dmg_of(g, x))
                .sum();
            // They may pick us; the biggest threat to us counts in full.
            (power as f32 * (0.5 + 0.5 / others)) as i32
        })
        .max()
        .unwrap_or(0);
    let defenders = |out: &Vec<(u32, Def)>| -> i32 {
        g.perms
            .iter()
            .filter(|x| {
                x.ctrl as usize == me
                    && x.is_creature()
                    && (x.has(VIGILANCE) || !out.iter().any(|(id, _)| *id == x.id))
                    && !x.has(CANT_BLOCK)
            })
            .map(|x| x.p.max(1) as i32 * 2)
            .sum()
    };
    gains.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
    let mut k = 0;
    while incoming - defenders(&out) >= my_life * 3 / 4 && k < gains.len() {
        let id = g.perms[gains[k].1].id;
        if g.perms[gains[k].1].goad.is_none() {
            out.retain(|(x, _)| *x != id);
        }
        k += 1;
    }
    out
}

/// After blocks: pump or protect a creature to win a fight, or push lethal damage.
pub fn combat_tricks(g: &mut Game, p: usize, attacks: &[Attack]) {
    let Some(h) = hand_find(g, p, is_trick) else {
        return;
    };
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
                cast_instant(g, p, h, Some(id));
                return;
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
                    cast_instant(g, p, h, Some(id));
                    return;
                }
            }
        }
    }
}
