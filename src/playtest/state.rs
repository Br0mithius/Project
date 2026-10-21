//! The game: players and their zones, permanents, mana, triggers, casting and the
//! state-based actions (CR 704) that keep it legal.

use super::compile::*;
use crate::abilities::{Effect, Whose};
use crate::analysis::Rng;
use std::sync::OnceLock;

/// A card in a player's deck list: (whose list, index).
pub type CardRef = (u8, u16);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tok {
    /// A real card.
    None,
    Creature,
    Treasure,
    Food,
    Clue,
    Map,
    Blood,
    Jace,
    Army,
    Germ,
    Other,
}

#[derive(Debug, Clone)]
pub struct Perm {
    pub id: u32,
    pub owner: u8,
    pub ctrl: u8,
    pub card: Option<CardRef>,
    pub tok: Tok,
    pub types: u16,
    pub subtypes: Vec<String>,
    pub colors: u8,
    pub base_p: i16,
    pub base_t: i16,
    /// +1/+1 counters minus -1/-1 counters.
    pub counters: i16,
    pub temp_p: i16,
    pub temp_t: i16,
    pub damage: i16,
    pub deathtouched: bool,
    pub loyalty: i16,
    /// Plan, lore, charge and other counters.
    pub other_counters: i16,
    pub shield: u8,
    pub stun: u8,
    pub kw: u32,
    pub temp_kw: u32,
    pub tapped: bool,
    pub sick: bool,
    pub attached: Option<u32>,
    /// 1 = commander, 2 = second commander (The Prismatic Piper / partner).
    pub commander: u8,
    pub powered: bool,
    pub born: u32,
    /// Goaded by this player (until their next turn).
    pub goad: Option<u8>,
    /// "Can't attack or block" (from an effect or an Aura), worked out by `recalc`.
    pub locked: bool,
    /// Locked by a one-off effect ("target creature can't attack or block for as long as...").
    pub lock_effect: bool,
    pub crewed: bool,
    pub prepared: bool,
    pub loyalty_used: bool,
    /// Returns to this player at end of turn (threaten effects).
    pub threaten_from: Option<u8>,
    /// Cards exiled "until ~ leaves the battlefield".
    pub holding: Vec<(u8, CardRef)>,
    pub activations: u8,
    /// Once-each-turn triggers that already fired this turn (bit per ability).
    pub fired: u32,
    /// Mana value (of the card it is, or copies; 0 for other tokens).
    pub mv: u8,
    /// In combat this turn: declared as an attacker / blocking.
    pub attacking: bool,
    pub blocking: bool,
    /// Dealt damage this turn.
    pub dealt_damage: bool,
    // Values after static effects (`Game::recalc`).
    pub p: i16,
    pub t: i16,
    pub eff_kw: u32,
    pub ward: u8,
    pub protection: u8,
}

impl Perm {
    pub fn is(&self, t: u16) -> bool {
        self.types & t != 0
    }

    pub fn is_creature(&self) -> bool {
        self.is(T_CREATURE) || self.crewed
    }

    pub fn has(&self, kw: u32) -> bool {
        self.eff_kw & kw != 0
    }

    pub fn is_token(&self) -> bool {
        self.card.is_none() || self.tok != Tok::None
    }
}

#[derive(Debug, Clone)]
pub struct Player<'a> {
    pub cards: &'a [SimCard],
    pub life: i32,
    pub poison: u8,
    pub library: Vec<u16>,
    pub hand: Vec<u16>,
    pub graveyard: Vec<u16>,
    pub exile: Vec<u16>,
    /// Cards exiled on an Adventure (castable as the creature later).
    pub adventure: Vec<u16>,
    /// Graveyard cards that may be cast again (flashback-like effects).
    pub recast: Vec<u16>,
    /// Command zone: (card, which commander slot).
    pub command: Vec<(u16, u8)>,
    pub casts: [u8; 3],
    /// Combat damage taken from each player's commanders: [player][slot].
    pub cmd_damage: [[i32; 3]; 8],
    pub alive: bool,
    pub lands_played: u8,
    pub spells_this_turn: u8,
    pub drawn_this_turn: u8,
    pub attacked_this_turn: bool,
    pub gained_life_turn: bool,
    pub lost_life_turn: bool,
    pub enduring_story: bool,
    /// An opponent controls this player's next turn (MSH Plans).
    pub hijacked: bool,
    /// Mana from rituals and the like, usable this step (any colour).
    pub pool: u8,
    pub extra_combats: u8,
    /// How much each opponent has attacked this player recently (for retaliation).
    pub grudge: [f32; 8],
    /// The last instant or sorcery cast (for copy effects): (card, back face).
    pub last_spell: Option<(CardRef, bool)>,
    pub mulligans: u8,
}

#[derive(Debug, Clone)]
pub struct Pending {
    pub ctrl: u8,
    pub source: Option<u32>,
    pub card: CardRef,
    pub back: bool,
    pub ability: u16,
    /// Snapshot of the source for leaves-the-battlefield triggers ("its power").
    pub snapshot: Option<Perm>,
    /// The permanent the event was about (the creature that died, entered...).
    pub subject: Option<u32>,
    pub teamwork: bool,
    pub gift: bool,
    pub from_graveyard: bool,
    pub kicked: bool,
    /// Cast for its overload cost: "target" means "each".
    pub overloaded: bool,
}

/// Context for resolving a spell's own abilities.
#[derive(Debug, Clone, Copy, Default)]
pub struct CastFlags {
    pub teamwork: bool,
    pub gift: bool,
    pub from_graveyard: bool,
}

pub struct Game<'a> {
    pub players: Vec<Player<'a>>,
    pub perms: Vec<Perm>,
    pub next_id: u32,
    pub round: u32,
    pub active: usize,
    pub rng: Rng,
    pub start_life: i32,
    pub died_this_turn: u8,
    pub pending: Vec<Pending>,
    pub fog: bool,
    pub in_combat: bool,
    /// Guards against endless trigger loops.
    pub resolved_this_action: u32,
    pub log: Option<Vec<String>>,
    /// The permanent the AI has in mind for its next targeted spell (protection, tricks,
    /// removal aimed at an attacker).
    pub focus: Option<u32>,
    /// What the resolving ability chose or made last ("it", "those creatures" refer to it).
    pub last_targets: Vec<u32>,
    /// Who controlled the first thing the resolving ability targeted ("its controller").
    pub last_ctrl: Option<u8>,
    /// The resolving ability has chosen targets ("it" is what it chose, if anything).
    pub targeted: bool,
    /// Who is targeting, for "becomes the target of a spell or ability an opponent controls".
    pub targeter: u8,
    /// Bumped whenever the board changes (`recalc`); the AI caches values per tick.
    pub tick: u64,
    /// Per permanent index: (tick, id, value).
    pub vcache: std::cell::RefCell<Vec<(u64, u32, f32)>>,
    /// Per player: (tick, threat).
    pub tcache: std::cell::RefCell<[(u64, f32); 8]>,
    /// Per (me, opponent): (tick, weight).
    pub wcache: std::cell::RefCell<[[(u64, f32); 8]; 8]>,
}

pub const NO_CARD: u16 = u16::MAX;

/// Creature types the league's cards care about (what a changeling counts as).
pub const CHANGELING_TYPES: &[&str] = &[
    "hero", "villain", "robot", "human", "dwarf", "elf", "goblin", "hobbit", "halfling", "wizard",
    "warrior", "soldier", "zombie", "dragon", "vampire", "beast", "spider", "mutant", "avenger",
    "inhuman", "army", "orc", "wolf", "cat", "bird", "knight", "rogue", "spy",
];

fn token_abilities(tok: Tok) -> &'static [SimAbility] {
    static TABLE: OnceLock<Vec<(Tok, Vec<SimAbility>)>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        use crate::abilities::{Amount, Target};
        let act = |mana: u8, tap: bool, effects: Vec<Effect>, text: &str| SimAbility {
            trig: Trig::Activated {
                mana,
                tap,
                sac_self: true,
                sac_other: false,
                discard: text.contains("discard"),
                life: 0,
                sorcery: false,
            },
            cond: Cond::Always,
            optional: false,
            once: false,
            modal: false,
            modes: Vec::new(),
            mode_texts: Vec::new(),
            mode_restr: Vec::new(),
            effects,
            text: text.to_string(),
            until_eot: false,
            counter_goal: None,
            filter: Filter::default(),
            once_per_turn: false,
            second_draw: false,
            card_restr: Default::default(),
        };
        let it = Target {
            kind: crate::abilities::Kind::Creature,
            whose: Whose::Yours,
            mass: false,
            other: false,
            restricted: false,
            restr: Default::default(),
            count: 1,
        };
        let loyalty = |n: i8, effects: Vec<Effect>| SimAbility {
            trig: Trig::Loyalty(n),
            cond: Cond::Always,
            optional: false,
            once: false,
            modal: false,
            modes: Vec::new(),
            mode_texts: Vec::new(),
            mode_restr: Vec::new(),
            effects,
            text: String::new(),
            until_eot: false,
            counter_goal: None,
            filter: Filter::default(),
            once_per_turn: false,
            second_draw: false,
            card_restr: Default::default(),
        };
        vec![
            (
                Tok::Food,
                vec![act(
                    2,
                    true,
                    vec![Effect::GainLife(Amount::fixed(3))],
                    "{2}, {t}, sacrifice this: you gain 3 life",
                )],
            ),
            (
                Tok::Clue,
                vec![act(
                    2,
                    false,
                    vec![Effect::Draw {
                        amount: Amount::fixed(1),
                        you: true,
                    }],
                    "{2}, sacrifice this: draw a card",
                )],
            ),
            (
                Tok::Map,
                vec![act(
                    1,
                    true,
                    vec![Effect::PlusCounters {
                        amount: Amount::fixed(1),
                        to: it,
                    }],
                    "{1}, {t}, sacrifice this: target creature you control explores",
                )],
            ),
            (
                Tok::Blood,
                vec![act(
                    1,
                    true,
                    vec![Effect::Loot],
                    "{1}, {t}, discard a card, sacrifice this: draw a card",
                )],
            ),
            (
                Tok::Jace,
                vec![
                    loyalty(-1, vec![Effect::Scry(Amount::fixed(1))]),
                    loyalty(
                        -3,
                        vec![Effect::Draw {
                            amount: Amount::fixed(1),
                            you: true,
                        }],
                    ),
                ],
            ),
        ]
    });
    table
        .iter()
        .find(|(t, _)| *t == tok)
        .map(|(_, a)| a.as_slice())
        .unwrap_or(&[])
}

impl<'a> Game<'a> {
    pub fn card(&self, r: CardRef) -> &'a SimCard {
        let cards: &'a [SimCard] = self.players[r.0 as usize].cards;
        &cards[r.1 as usize]
    }

    pub fn note(&mut self, f: impl FnOnce() -> String) {
        if let Some(l) = &mut self.log {
            l.push(f());
        }
    }

    /// The abilities a permanent has.
    pub fn abilities_of(&self, perm: &Perm) -> &'a [SimAbility] {
        match (perm.tok, perm.card) {
            (Tok::None | Tok::Creature | Tok::Other, Some(r)) => &self.card(r).abilities,
            (t, _) => token_abilities(t),
        }
    }

    pub fn idx(&self, id: u32) -> Option<usize> {
        self.perms.iter().position(|p| p.id == id)
    }

    pub fn opponents(&self, p: usize) -> Vec<usize> {
        (0..self.players.len())
            .filter(|&o| o != p && self.players[o].alive)
            .collect()
    }

    pub fn chance(&mut self, p: f32) -> bool {
        (self.rng.next() % 10_000) as f32 / 10_000.0 < p
    }

    pub fn rand(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.rng.next() % n as u64) as usize
        }
    }

    // ------------------------------------------------------------ statics

    /// Re-applies static effects: anthems, Equipment and Aura bonuses, conditional
    /// self-buffs, locks; then state-based actions.
    pub fn recalc(&mut self) {
        self.tick += 1;
        for x in self.perms.iter_mut() {
            x.p = x.base_p + x.counters + x.temp_p;
            x.t = x.base_t + x.counters + x.temp_t;
            x.eff_kw = x.kw | x.temp_kw;
            x.locked = x.lock_effect;
            if let Some(r) = x.card {
                let c = &self.players[r.0 as usize].cards[r.1 as usize];
                x.ward = c.ward;
                x.protection = c.protection;
            }
        }
        // Collect static effects first (they read the board), then apply.
        let mut mods: Vec<(usize, i16, i16, u32, bool)> = Vec::new();
        for si in 0..self.perms.len() {
            let src = &self.perms[si];
            if src.card.is_none_or(|r| !self.card(r).has_static)
                || matches!(
                    src.tok,
                    Tok::Treasure | Tok::Food | Tok::Clue | Tok::Map | Tok::Blood | Tok::Jace
                )
            {
                continue;
            }
            for a in self.abilities_of(src) {
                if a.trig != Trig::Static || !self.cond_holds(&a.cond, src.ctrl as usize, Some(si))
                {
                    continue;
                }
                let attached_text =
                    a.text.starts_with("equipped") || a.text.starts_with("enchanted");
                for e in &a.effects {
                    let (dp, dt, kw, lock, target) = match e {
                        Effect::Pump {
                            power,
                            toughness,
                            to,
                            until_eot: false,
                        } => (*power as i16, *toughness as i16, 0, false, Some(to)),
                        Effect::Shrink {
                            amount,
                            to,
                            until_eot: false,
                        } => (-amount.n as i16, -amount.n as i16, 0, false, Some(to)),
                        Effect::Grant {
                            keywords,
                            to,
                            until_eot: false,
                        } => (0, 0, keyword_bits(keywords), false, Some(to)),
                        Effect::Lock {
                            target,
                            lasting: true,
                        } if attached_text => (0, 0, 0, true, Some(target)),
                        _ => continue,
                    };
                    let Some(to) = target else { continue };
                    if attached_text {
                        if let Some(t) = src.attached.and_then(|id| self.idx(id)) {
                            mods.push((t, dp, dt, kw, lock));
                        }
                        continue;
                    }
                    if !to.mass {
                        if to.kind == crate::abilities::Kind::It || a.text.starts_with('~') {
                            mods.push((si, dp, dt, kw, lock));
                        }
                        continue;
                    }
                    let filt = &a.filter;
                    let r = &to.restr;
                    let d = self.dyn_values(r, None, src.ctrl as usize, Some(src.id));
                    for (ti, t) in self.perms.iter().enumerate() {
                        if r.any_types != 0 {
                            if !(t.is(r.any_types)
                                || r.any_types & T_CREATURE != 0 && t.is_creature())
                            {
                                continue;
                            }
                        } else if !t.is_creature() && filt.types & !T_CREATURE == 0 {
                            continue;
                        }
                        if (to.other || filt.other) && ti == si {
                            continue;
                        }
                        let mine = t.ctrl == src.ctrl;
                        let ok_side = match to.whose {
                            Whose::Yours => mine,
                            Whose::Theirs => !mine,
                            Whose::Any => true,
                        };
                        if !ok_side {
                            continue;
                        }
                        if !r.is_empty() {
                            if !self.perm_ok(r, t, &d) {
                                continue;
                            }
                        } else if let Some(sub) = &filt.subtype {
                            if !t.subtypes.iter().any(|s| s == sub) {
                                continue;
                            }
                        }
                        mods.push((ti, dp, dt, kw, lock));
                    }
                }
            }
        }
        for (i, dp, dt, kw, lock) in mods {
            let x = &mut self.perms[i];
            x.p += dp;
            x.t += dt;
            x.eff_kw |= kw;
            if lock {
                x.eff_kw |= CANT_BLOCK;
                x.locked = true;
            }
        }
        // Enduring story (Storied): three or more artifacts, legendaries and/or Sagas.
        for p in 0..self.players.len() {
            if !self.players[p].enduring_story {
                let n = self
                    .perms
                    .iter()
                    .filter(|x| x.ctrl as usize == p && x.is(T_ARTIFACT | T_LEGENDARY | T_SAGA))
                    .count();
                if n >= 3 {
                    self.players[p].enduring_story = true;
                }
            }
        }
    }

    /// Checks a condition for player `p` (and the source permanent, if any).
    pub fn cond_holds(&self, c: &Cond, p: usize, src: Option<usize>) -> bool {
        let pl = &self.players[p];
        let mine = || self.perms.iter().filter(move |x| x.ctrl as usize == p);
        match c {
            Cond::Always => true,
            Cond::Equipped => src.is_some_and(|s| {
                let id = self.perms[s].id;
                self.perms
                    .iter()
                    .any(|x| x.is(T_EQUIPMENT) && x.attached == Some(id))
            }),
            Cond::Teamwork | Cond::Gift | Cond::CastFromGraveyard | Cond::Kicked => false,
            Cond::Not(inner) => !self.cond_holds(inner, p, src),
            Cond::Storied => pl.enduring_story,
            Cond::AttackedThisTurn => pl.attacked_this_turn,
            Cond::CreatureDiedThisTurn => self.died_this_turn > 0,
            Cond::Graveyard(n, creatures) => {
                let k = if *creatures {
                    pl.graveyard
                        .iter()
                        .filter(|&&c| pl.cards[c as usize].is(T_CREATURE))
                        .count()
                } else {
                    pl.graveyard.len()
                };
                k >= *n as usize
            }
            Cond::ControlPower(n) => mine().any(|x| x.is_creature() && x.p >= *n as i16),
            Cond::ControlToughness(n) => mine().any(|x| x.is_creature() && x.t >= *n as i16),
            Cond::ControlLands(n) => mine().filter(|x| x.is(T_LAND)).count() >= *n as usize,
            Cond::Control(what, another) => mine().enumerate().any(|(_, x)| {
                let other_ok = !*another || src.is_none_or(|s| self.perms[s].id != x.id);
                other_ok
                    && (x.subtypes.iter().any(|s| s == what)
                        || match what.as_str() {
                            "creature" => x.is_creature(),
                            "artifact" => x.is(T_ARTIFACT),
                            "enchantment" => x.is(T_ENCHANTMENT),
                            "planeswalker" => x.is(T_PLANESWALKER),
                            "legendary" => x.is(T_LEGENDARY),
                            "basic" => x.is(T_BASIC),
                            _ => false,
                        })
            }),
            Cond::NoPlaneswalker => !mine().any(|x| x.is(T_PLANESWALKER)),
            Cond::HasPlaneswalker => mine().any(|x| x.is(T_PLANESWALKER)),
            Cond::LifeAtLeast(n) => pl.life >= *n,
            Cond::LifeAboveStart => pl.life > self.start_life,
            Cond::OpponentLostLife => self
                .players
                .iter()
                .enumerate()
                .any(|(i, o)| i != p && o.lost_life_turn),
            Cond::DrawnThisTurn(n) => pl.drawn_this_turn >= *n,
            Cond::GainedLife => pl.gained_life_turn,
            Cond::MyTurn => self.active == p,
            Cond::NotPrepared => src.is_some_and(|s| !self.perms[s].prepared),
            Cond::NotToken => src.is_none_or(|s| !self.perms[s].is_token()),
            Cond::NoSpellsThisTurn => pl.spells_this_turn == 0,
            Cond::CountersAtLeast(n) => src.is_some_and(|s| self.perms[s].counters >= *n),
            Cond::PowerAtMost(n) => src.is_some_and(|s| self.perms[s].p <= *n),
            Cond::HasShield => src.is_some_and(|s| self.perms[s].shield > 0),
            Cond::Treasures(n) => mine().filter(|x| x.tok == Tok::Treasure).count() >= *n as usize,
            // Can't be checked: decided once per check, deterministically from the round.
            Cond::Chance(f) => {
                ((self.round as usize * 7 + p * 3 + src.unwrap_or(0)) % 100) as f32 / 100.0 < *f
            }
        }
    }

    // ------------------------------------------------------------ mana

    /// Untapped mana sources: (perm index, colours, amount, sacrificed when used).
    pub fn sources(&self, p: usize) -> Vec<(usize, u8, u8, bool)> {
        let mut out = Vec::new();
        for (i, x) in self.perms.iter().enumerate() {
            if x.ctrl as usize != p || x.tapped {
                continue;
            }
            if x.tok == Tok::Treasure {
                out.push((i, 0b11111, 1, true));
                continue;
            }
            let Some(r) = x.card else { continue };
            let c = self.card(r);
            let land = x.is(T_LAND);
            if land || c.mana_ability && !(x.is_creature() && x.sick && !x.has(HASTE)) {
                out.push((
                    i,
                    if c.produces == 0 && land {
                        0
                    } else {
                        c.produces
                    },
                    c.mana_amount.max(1),
                    false,
                ));
            }
        }
        // Cheapest-to-lose first: single-colour lands before rainbow sources, Treasures last.
        out.sort_by_key(|s| (s.3, s.1.count_ones(), s.2));
        out
    }

    pub fn mana_available(&self, p: usize) -> u32 {
        self.sources(p).iter().map(|s| s.2 as u32).sum::<u32>() + self.players[p].pool as u32
    }

    /// Plans paying `cost` from player `p`'s sources; returns the sources to use.
    fn plan_payment(&self, p: usize, cost: &Mana) -> Option<(Vec<usize>, u8)> {
        let srcs = self.sources(p);
        let mut used = vec![false; srcs.len()];
        let mut pips = cost.pips;
        // Most constrained colour first.
        let mut order: Vec<usize> = (0..5).filter(|&c| pips[c] > 0).collect();
        order.sort_by_key(|&c| srcs.iter().filter(|s| s.1 & (1 << c) != 0).count());
        let mut extra_generic = 0u32; // mana left over from multi-mana sources
        for c in order {
            while pips[c] > 0 {
                let pick = srcs
                    .iter()
                    .enumerate()
                    .filter(|(i, s)| !used[*i] && s.1 & (1 << c) != 0)
                    .min_by_key(|(_, s)| (s.3, s.1.count_ones()))
                    .map(|(i, _)| i);
                match pick {
                    Some(i) => {
                        used[i] = true;
                        pips[c] -= 1;
                        extra_generic += srcs[i].2 as u32 - 1;
                    }
                    None => return None,
                }
            }
        }
        // {C}: only colourless sources; hybrid: either colour.
        let wanted: Vec<u8> = std::iter::repeat_n(COLORLESS, cost.colorless as usize)
            .chain(cost.hybrid.iter().copied().filter(|&h| h != 0))
            .collect();
        for mask in wanted {
            let pick = srcs
                .iter()
                .enumerate()
                .filter(|(i, s)| !used[*i] && s.1 & mask != 0)
                .min_by_key(|(_, s)| (s.3, s.1.count_ones()))
                .map(|(i, _)| i);
            match pick {
                Some(i) => {
                    used[i] = true;
                    extra_generic += srcs[i].2 as u32 - 1;
                }
                None => return None,
            }
        }
        let mut need = (cost.generic as u32).saturating_sub(extra_generic);
        let mut pool = self.players[p].pool;
        while need > 0 && pool > 0 {
            pool -= 1;
            need -= 1;
        }
        for (i, s) in srcs.iter().enumerate() {
            if need == 0 {
                break;
            }
            if !used[i] {
                used[i] = true;
                need = need.saturating_sub(s.2 as u32);
            }
        }
        if need > 0 {
            return None;
        }
        Some((
            srcs.iter()
                .enumerate()
                .filter(|(i, _)| used[*i])
                .map(|(_, s)| s.0)
                .collect(),
            self.players[p].pool - pool,
        ))
    }

    pub fn can_pay(&self, p: usize, cost: &Mana) -> bool {
        self.plan_payment(p, cost).is_some()
    }

    /// Pays a cost (taps sources, sacrifices Treasures). False if it can't be paid.
    pub fn pay(&mut self, p: usize, cost: &Mana) -> bool {
        let Some((idxs, from_pool)) = self.plan_payment(p, cost) else {
            return false;
        };
        self.players[p].pool -= from_pool;
        let mut sac = Vec::new();
        for i in idxs {
            if self.perms[i].tok == Tok::Treasure {
                sac.push(self.perms[i].id);
            } else {
                self.perms[i].tapped = true;
            }
        }
        for id in sac {
            if let Some(i) = self.idx(id) {
                self.leave(i, Zone::Graveyard, true);
            }
        }
        true
    }

    // ------------------------------------------------------------ zones

    pub fn draw(&mut self, p: usize, n: u32) {
        for _ in 0..n {
            match self.players[p].library.pop() {
                Some(c) => {
                    self.players[p].hand.push(c);
                    self.players[p].drawn_this_turn += 1;
                    self.emit(Ev::Draw, p, None);
                }
                None => {
                    self.players[p].alive = false;
                    return;
                }
            }
        }
    }

    pub fn gain_life(&mut self, p: usize, n: i32) {
        if n <= 0 || !self.players[p].alive {
            return;
        }
        if self.no_lifegain() {
            return;
        }
        self.players[p].life += n;
        self.players[p].gained_life_turn = true;
        self.emit(Ev::GainLife, p, None);
    }

    fn no_lifegain(&self) -> bool {
        self.perms.iter().any(|x| {
            self.abilities_of(x)
                .iter()
                .any(|a| a.trig == Trig::Static && a.text.contains("players can't gain life"))
        })
    }

    pub fn lose_life(&mut self, p: usize, n: i32) {
        if n <= 0 || !self.players[p].alive {
            return;
        }
        self.players[p].life -= n;
        self.players[p].lost_life_turn = true;
        self.emit(Ev::LoseLife, p, None);
    }

    /// Damage to a player from `source_ctrl` (combat or not).
    pub fn damage_player(&mut self, from: usize, to: usize, n: i32, toxic: u8) {
        if n <= 0 {
            return;
        }
        self.lose_life(to, n);
        if toxic > 0 {
            self.players[to].poison = self.players[to].poison.saturating_add(toxic);
        }
        if from != to {
            self.players[to].grudge[from] += n as f32;
        }
    }

    /// Moves a permanent off the battlefield. `sba` = by state-based actions (no replacement
    /// choice for commanders is needed: they always go to the command zone).
    pub fn leave(&mut self, i: usize, to: Zone, _sba: bool) {
        let perm = self.perms.remove(i);
        let id = perm.id;
        // Attachments: Auras fall off (to the graveyard), Equipment stays unattached.
        let mut fall = Vec::new();
        for x in self.perms.iter_mut() {
            if x.attached == Some(id) {
                x.attached = None;
                if x.is(T_AURA) {
                    fall.push(x.id);
                }
            }
        }
        // What it was attached to.
        // Cards held "until ~ leaves" come back.
        for (owner, r) in &perm.holding {
            let pl = &mut self.players[*owner as usize];
            if let Some(pos) = pl.exile.iter().position(|&c| c == r.1) {
                pl.exile.remove(pos);
                self.enter_card(r.0 as usize, r.1, *owner as usize, false);
            }
        }
        let creature = perm.is_creature();
        let to_gy = matches!(to, Zone::Graveyard);
        if creature && to_gy {
            self.died_this_turn += 1;
        }
        // Dies / leaves triggers of the permanent itself.
        if let Some(r) = perm.card {
            if to_gy && perm.tok == Tok::None {
                let abilities = &self.card(r).abilities;
                for (ai, a) in abilities.iter().enumerate() {
                    if a.trig == Trig::Dies {
                        self.pending.push(Pending {
                            ctrl: perm.ctrl,
                            source: None,
                            card: r,
                            back: false,
                            ability: ai as u16,
                            snapshot: Some(perm.clone()),
                            subject: None,
                            teamwork: false,
                            gift: false,
                            from_graveyard: false,
                            kicked: false,
                            overloaded: false,
                        });
                    }
                }
            }
        }
        if creature && to_gy {
            self.emit_about(Ev::Death, perm.ctrl as usize, &perm);
        }
        if matches!(to, Zone::Hand) {
            self.emit_about(Ev::Bounced, perm.ctrl as usize, &perm);
        }
        // The card goes to its zone.
        if let (Some(r), Tok::None) = (perm.card, perm.tok) {
            let owner = perm.owner as usize;
            if r.0 as usize == owner {
                let pl = &mut self.players[owner];
                if perm.commander > 0
                    && matches!(to, Zone::Graveyard | Zone::Exile | Zone::Library(_))
                {
                    pl.command.push((r.1, perm.commander));
                } else {
                    match to {
                        Zone::Graveyard => pl.graveyard.push(r.1),
                        Zone::Exile => pl.exile.push(r.1),
                        Zone::Hand => pl.hand.push(r.1),
                        Zone::Library(top) => {
                            if top {
                                pl.library.push(r.1)
                            } else {
                                pl.library.insert(0, r.1)
                            }
                        }
                    }
                }
            }
        }
        for fid in fall {
            if let Some(fi) = self.idx(fid) {
                self.leave(fi, Zone::Graveyard, true);
            }
        }
    }

    /// Destroys a permanent (indestructible and shield counters stop it). True if it left.
    pub fn destroy(&mut self, i: usize) -> bool {
        let x = &mut self.perms[i];
        if x.has(INDESTRUCTIBLE) {
            return false;
        }
        if x.shield > 0 {
            x.shield -= 1;
            x.damage = 0;
            return false;
        }
        self.leave(i, Zone::Graveyard, false);
        true
    }

    pub fn sacrifice(&mut self, i: usize) {
        let p = self.perms[i].ctrl as usize;
        let subject = self.perms[i].clone();
        self.leave(i, Zone::Graveyard, false);
        self.emit_about(Ev::Sacrifice, p, &subject);
    }

    /// Damage to a permanent from a source with these keywords.
    pub fn damage_perm(&mut self, i: usize, n: i16, deathtouch: bool) {
        if n <= 0 {
            return;
        }
        let x = &mut self.perms[i];
        if x.is(T_PLANESWALKER) && !x.is_creature() {
            x.loyalty -= n;
            return;
        }
        if x.shield > 0 {
            x.shield -= 1;
            return;
        }
        x.damage += n;
        if deathtouch {
            x.deathtouched = true;
        }
        let snap = x.clone();
        self.emit_about(Ev::Damaged, snap.ctrl as usize, &snap);
    }

    /// Creates a new permanent and returns its index.
    pub fn new_perm(
        &mut self,
        ctrl: usize,
        owner: usize,
        card: Option<CardRef>,
        tok: Tok,
    ) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        let mv = card.map(|r| self.card(r).mv).unwrap_or(0);
        let (types, subtypes, colors, p, t, kw) = match card {
            Some(r) if tok == Tok::None || tok == Tok::Creature || tok == Tok::Other => {
                let c = self.card(r);
                // Changelings are every creature type.
                let subtypes = if c.changeling {
                    CHANGELING_TYPES.iter().map(|s| s.to_string()).collect()
                } else {
                    c.subtypes.clone()
                };
                (c.types, subtypes, c.colors, c.power, c.toughness, c.kw)
            }
            _ => (T_ARTIFACT, Vec::new(), 0, 0, 0, 0),
        };
        let mut perm = Perm {
            id,
            owner: owner as u8,
            ctrl: ctrl as u8,
            card,
            tok,
            types,
            subtypes,
            colors,
            base_p: p,
            base_t: t,
            counters: 0,
            temp_p: 0,
            temp_t: 0,
            damage: 0,
            deathtouched: false,
            loyalty: 0,
            other_counters: 0,
            shield: 0,
            stun: 0,
            kw,
            temp_kw: 0,
            tapped: false,
            sick: true,
            attached: None,
            commander: 0,
            powered: false,
            born: self.round,
            goad: None,
            locked: false,
            lock_effect: false,
            crewed: false,
            prepared: false,
            loyalty_used: false,
            threaten_from: None,
            holding: Vec::new(),
            activations: 0,
            fired: 0,
            mv,
            attacking: false,
            blocking: false,
            dealt_damage: false,
            p,
            t,
            eff_kw: kw,
            ward: 0,
            protection: 0,
        };
        match tok {
            Tok::Jace => {
                perm.types = T_PLANESWALKER | T_TOKEN;
                perm.subtypes = vec!["jace".into()];
            }
            Tok::Army => {
                perm.types = T_CREATURE | T_TOKEN;
                perm.subtypes = vec!["army".into()];
            }
            Tok::Germ => {
                perm.types = T_CREATURE | T_TOKEN;
            }
            Tok::Creature if card.is_none() => perm.types = T_CREATURE | T_TOKEN,
            Tok::Creature | Tok::Other => perm.types |= T_TOKEN,
            Tok::None => {}
            _ => perm.types = T_ARTIFACT | T_TOKEN,
        }
        self.perms.push(perm);
        self.perms.len() - 1
    }

    /// Puts card `idx` from player `list`'s deck onto the battlefield under `ctrl`. Returns
    /// its index afterwards (None if it didn't survive arriving).
    pub fn enter_card(
        &mut self,
        list: usize,
        idx: u16,
        ctrl: usize,
        _commander: bool,
    ) -> Option<usize> {
        let r = (list as u8, idx);
        let i = self.new_perm(ctrl, list, Some(r), Tok::None);
        let id = self.perms[i].id;
        let c = self.card(r);
        if c.is(T_LAND) && c.enters_tapped {
            self.perms[i].tapped = true;
        }
        if c.is(T_PLANESWALKER) {
            self.perms[i].loyalty = c.loyalty;
        }
        self.after_enter(i);
        self.idx(id)
    }

    /// Enters-the-battlefield bookkeeping: triggers, Sagas, living weapon, Auras, legend rule.
    pub fn after_enter(&mut self, i: usize) {
        let id = self.perms[i].id;
        let ctrl = self.perms[i].ctrl as usize;
        let snap = self.perms[i].clone();
        if let Some(r) = snap.card.filter(|_| snap.tok == Tok::None) {
            let c = self.card(r);
            // Legend rule: keep the newer one.
            if c.is(T_LEGENDARY) {
                if let Some(old) = self.perms.iter().position(|x| {
                    x.id != id && x.ctrl as usize == ctrl && x.card == Some(r) && x.tok == Tok::None
                }) {
                    self.leave(old, Zone::Graveyard, true);
                }
            }
            if c.living_weapon {
                let g = self.new_perm(ctrl, ctrl, None, Tok::Germ);
                self.perms[g].sick = true;
                if let Some(e) = self.idx(id) {
                    self.perms[e].attached = Some(self.perms[g].id);
                }
            }
            for (ai, a) in c.abilities.iter().enumerate() {
                let fire = a.trig == Trig::Enters || (a.trig == Trig::Chapter(1) && c.is(T_SAGA));
                if fire {
                    self.pending.push(Pending {
                        ctrl: ctrl as u8,
                        source: Some(id),
                        card: r,
                        back: false,
                        ability: ai as u16,
                        snapshot: None,
                        subject: None,
                        teamwork: false,
                        gift: false,
                        from_graveyard: false,
                        kicked: false,
                        overloaded: false,
                    });
                }
            }
            if c.is(T_SAGA) {
                if let Some(e) = self.idx(id) {
                    self.perms[e].other_counters = 1;
                }
            }
        }
        if snap.is(T_LAND) {
            self.emit(Ev::Landfall, ctrl, Some(id));
        }
        self.emit_about(Ev::OtherEnters, ctrl, &snap);
        if snap.is_token() {
            self.emit(Ev::Tokens, ctrl, Some(id));
        }
        self.recalc();
    }

    // ------------------------------------------------------------ triggers

    /// An event about player `p` (drawing, gaining life, landfall...).
    pub fn emit(&mut self, ev: Ev, p: usize, subject: Option<u32>) {
        let snap = subject
            .and_then(|id| self.idx(id))
            .map(|i| self.perms[i].clone());
        match snap {
            Some(s) => self.emit_about(ev, p, &s),
            None => self.emit_inner(ev, p, None),
        }
    }

    pub fn emit_about(&mut self, ev: Ev, p: usize, subject: &Perm) {
        self.emit_inner(ev, p, Some(subject));
    }

    fn emit_inner(&mut self, ev: Ev, p: usize, subject: Option<&Perm>) {
        let mut add = Vec::new();
        let mut fired = Vec::new();
        let bit = ev_bit(ev);
        for (si, src) in self.perms.iter().enumerate() {
            // Only cards that listen for this kind of event (tokens without a card don't).
            let Some(r) = src.card else { continue };
            if self.card(r).ev_mask & bit == 0
                || src.tok != Tok::None && src.tok != Tok::Other && src.tok != Tok::Creature
            {
                continue;
            }
            for (ai, a) in self.abilities_of(src).iter().enumerate() {
                let Trig::Event(kind, f) = &a.trig else {
                    continue;
                };
                if *kind != ev {
                    continue;
                }
                if a.once_per_turn && ai < 32 && src.fired & (1 << ai) != 0 {
                    continue;
                }
                if a.second_draw && ev == Ev::Draw && self.players[p].drawn_this_turn != 2 {
                    continue;
                }
                let mine = src.ctrl as usize == p;
                if let Some(y) = f.yours {
                    if y != mine {
                        continue;
                    }
                }
                if let Some(s) = subject {
                    // Self-referential events ("whenever ~ is dealt damage", "becomes the target").
                    let about_self =
                        a.text.contains("whenever ~") && !a.text.contains("whenever ~ or");
                    if matches!(
                        ev,
                        Ev::Damaged
                            | Ev::Targeted
                            | Ev::TeamworkTapped
                            | Ev::Tapped
                            | Ev::Untapped
                            | Ev::Blocks
                            | Ev::Bounced
                    ) && about_self
                        && s.id != src.id
                    {
                        continue;
                    }
                    // "Whenever equipped creature ...": only the creature this is attached to.
                    let attached_text = a.text.starts_with("whenever equipped creature")
                        || a.text.starts_with("whenever enchanted creature");
                    if (attached_text
                        || a.text.contains("equipped creature") && ev != Ev::EachAttacker)
                        && src.attached != Some(s.id)
                    {
                        continue;
                    }
                    // "Becomes the target of a spell or ability an opponent controls".
                    if ev == Ev::Targeted
                        && trigger_part(&a.text).contains("an opponent controls")
                        && self.targeter == src.ctrl
                    {
                        continue;
                    }
                    // "... attacks alone".
                    if ev == Ev::EachAttacker
                        && a.text.contains(" alone")
                        && self.perms.iter().filter(|x| x.attacking).count() != 1
                    {
                        continue;
                    }
                    if a.text.contains("with deathtouch") && !s.has(DEATHTOUCH) {
                        continue;
                    }
                    if f.other && s.id == src.id {
                        continue;
                    }
                    if ev == Ev::OtherEnters && s.id == src.id {
                        continue;
                    }
                    if f.types != 0
                        && s.types & f.types == 0
                        && !(f.types & T_CREATURE != 0 && s.is_creature())
                    {
                        continue;
                    }
                    if f.not_types != 0 && s.types & f.not_types != 0 {
                        continue;
                    }
                    if (f.nontoken && s.is_token()) || (f.token && !s.is_token()) {
                        continue;
                    }
                    if !f.restr.is_empty() {
                        let d = self.dyn_values(&f.restr, None, src.ctrl as usize, Some(src.id));
                        if !self.perm_ok(&f.restr, s, &d) {
                            continue;
                        }
                    } else if let Some(sub) = &f.subtype {
                        if !s.subtypes.iter().any(|x| x == sub) {
                            continue;
                        }
                    }
                }
                if a.once_per_turn && ai < 32 {
                    fired.push((si, ai));
                }
                add.push(Pending {
                    ctrl: src.ctrl,
                    source: Some(src.id),
                    card: r,
                    back: false,
                    ability: ai as u16,
                    snapshot: if src.tok != Tok::None {
                        Some(src.clone())
                    } else {
                        None
                    },
                    subject: subject.map(|s| s.id),
                    teamwork: false,
                    gift: false,
                    from_graveyard: false,
                    kicked: false,
                    overloaded: false,
                });
            }
        }
        for (si, ai) in fired {
            self.perms[si].fired |= 1 << ai;
        }
        self.pending.extend(add);
    }

    /// Fires the spell-cast triggers: "whenever you cast", prowess, extort.
    pub fn on_cast(&mut self, p: usize, card: &SimCard) {
        self.players[p].spells_this_turn += 1;
        // Build a stand-in permanent to match filters against the spell.
        let probe = Perm {
            types: card.types,
            subtypes: card.subtypes.clone(),
            colors: card.colors,
            mv: card.mv,
            ..self.probe()
        };
        self.emit_about(Ev::Cast, p, &probe);
        if !card.is(T_CREATURE) {
            for x in self
                .perms
                .iter_mut()
                .filter(|x| x.ctrl as usize == p && x.has(PROWESS))
            {
                x.temp_p += 1;
                x.temp_t += 1;
            }
        }
        // Extort: pay {1} per extort permanent to drain each opponent 1.
        let extorts = self
            .perms
            .iter()
            .filter(|x| x.ctrl as usize == p && x.card.is_some_and(|r| self.card(r).extort))
            .count();
        for _ in 0..extorts {
            if self.can_pay(p, &Mana::generic(1)) && self.mana_available(p) > 2 {
                self.pay(p, &Mana::generic(1));
                let mut total = 0;
                for o in self.opponents(p) {
                    self.lose_life(o, 1);
                    total += 1;
                }
                self.gain_life(p, total);
            }
        }
        self.recalc();
    }

    pub fn probe(&self) -> Perm {
        Perm {
            id: u32::MAX,
            owner: 0,
            ctrl: 0,
            card: None,
            tok: Tok::Other,
            types: 0,
            subtypes: Vec::new(),
            colors: 0,
            base_p: 0,
            base_t: 0,
            counters: 0,
            temp_p: 0,
            temp_t: 0,
            damage: 0,
            deathtouched: false,
            loyalty: 0,
            other_counters: 0,
            shield: 0,
            stun: 0,
            kw: 0,
            temp_kw: 0,
            tapped: false,
            sick: false,
            attached: None,
            commander: 0,
            powered: false,
            born: 0,
            goad: None,
            locked: false,
            lock_effect: false,
            crewed: false,
            prepared: false,
            loyalty_used: false,
            threaten_from: None,
            holding: Vec::new(),
            activations: 0,
            fired: 0,
            mv: 0,
            attacking: false,
            blocking: false,
            dealt_damage: false,
            p: 0,
            t: 0,
            eff_kw: 0,
            ward: 0,
            protection: 0,
        }
    }

    // ------------------------------------------------------------ state-based actions

    /// CR 704: players at 0 life, 10 poison or 21 commander damage lose; creatures with
    /// lethal damage or 0 toughness die; planeswalkers with no loyalty go; Auras with
    /// nothing to enchant fall off.
    pub fn sba(&mut self) {
        loop {
            self.recalc();
            let mut changed = false;
            for p in 0..self.players.len() {
                let pl = &self.players[p];
                if pl.alive
                    && (pl.life <= 0
                        || pl.poison >= 10
                        || pl.cmd_damage.iter().any(|d| d.iter().any(|&x| x >= 21)))
                {
                    self.players[p].alive = false;
                    changed = true;
                }
                if !self.players[p].alive {
                    // A player who leaves takes their permanents with them (CR 800.4a).
                    let before = self.perms.len();
                    self.perms
                        .retain(|x| x.owner as usize != p && x.ctrl as usize != p);
                    changed |= self.perms.len() != before;
                }
            }
            let dead = self.perms.iter().position(|x| {
                (x.is_creature()
                    && (x.t <= 0
                        || (!x.has(INDESTRUCTIBLE)
                            && (x.damage >= x.t || x.deathtouched && x.damage > 0))))
                    || (x.is(T_PLANESWALKER) && !x.is_creature() && x.loyalty <= 0)
                    || (x.is(T_AURA)
                        && x.attached
                            .is_none_or(|a| !self.perms.iter().any(|y| y.id == a)))
            });
            if let Some(i) = dead {
                let x = &self.perms[i];
                if x.is_creature() && x.t > 0 && x.shield > 0 && !x.deathtouched {
                    let x = &mut self.perms[i];
                    x.shield -= 1;
                    x.damage = 0;
                } else {
                    self.leave(i, Zone::Graveyard, true);
                }
                changed = true;
            }
            if !changed {
                break;
            }
        }
    }

    pub fn alive_count(&self) -> usize {
        self.players.iter().filter(|p| p.alive).count()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Zone {
    Graveyard,
    Exile,
    Hand,
    /// true = top.
    Library(bool),
}
