//! A 4-player Commander game simulator, so decks can be compared by playing them rather
//! than only by scoring their cards.
//!
//! Cards are read by the ability parser (`abilities`) and compiled into what the game needs
//! (`compile`): costs, types, keywords, triggers, conditions. The game (`state`) has every
//! zone - library, hand, graveyard, exile, command zone, battlefield - with creatures,
//! artifacts, enchantments, Auras, Equipment, Vehicles, Sagas, planeswalkers and tokens;
//! mana is paid source by source with the right colours. Every parsed effect resolves
//! (`resolve`), combat follows the combat rules (`combat`), and each seat is played by an
//! AI that plans its mana, holds up interaction, picks fights and blocks like a careful
//! player (`ai`). It's still a model: where a card's text goes beyond what the parser
//! reads, the simulator plays what was read.

pub mod ai;
pub mod combat;
pub mod compile;
pub mod probe;
pub mod resolve;
pub mod state;
#[cfg(test)]
mod tests;

use crate::analysis::Rng;
use crate::db::{CardDb, Rules};
use crate::deck::Deck;
use compile::*;
use state::*;

/// A deck read once into simulator cards, ready to play many games.
pub struct Prepared {
    pub cards: Vec<SimCard>,
    library: Vec<u16>,
    /// Command zone: (card, slot) - the commander, and The Prismatic Piper or a partner.
    commanders: Vec<(u16, u8)>,
}

impl Prepared {
    pub fn new(deck: &Deck, db: &CardDb) -> Prepared {
        let mut cards = Vec::new();
        let mut library = Vec::new();
        for (name, n) in &deck.cards {
            if let Some(c) = db.get(name) {
                cards.push(compile(c));
                for _ in 0..*n {
                    library.push((cards.len() - 1) as u16);
                }
            }
        }
        let mut commanders = Vec::new();
        for (slot, name) in [(1u8, &deck.commander), (2u8, &deck.partner)] {
            if let Some(c) = name.as_ref().and_then(|n| db.get(n)) {
                cards.push(compile(c));
                commanders.push(((cards.len() - 1) as u16, slot));
            }
        }
        Prepared {
            cards,
            library,
            commanders,
        }
    }
}

/// Where a spell is cast from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Src {
    Hand(usize),
    /// Index into the command zone list.
    Command(usize),
    /// Index into the graveyard (cards allowed to be cast from there).
    Graveyard(usize),
    /// Index into the cards exiled on an Adventure.
    Adventure(usize),
    /// A prepared creature's spell copy (CR 722).
    Prepared(u32),
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CastOpts {
    /// Cast the back face (Adventure / other double-faced spell) from hand.
    pub back: bool,
    pub free: bool,
    pub teamwork: bool,
    pub gift: bool,
    pub offspring: bool,
    pub convoke: bool,
    pub kicker: bool,
    pub overload: bool,
    /// MSH Sneak's alternative cost (after attacking this turn).
    pub sneak: bool,
}

impl<'a> Game<'a> {
    pub fn new(decks: &[&'a Prepared], rules: &Rules, rng: Rng) -> Game<'a> {
        let life = rules.starting_life.max(1) as i32;
        let players = decks
            .iter()
            .map(|d| Player {
                cards: &d.cards,
                life,
                poison: 0,
                library: d.library.clone(),
                hand: Vec::new(),
                graveyard: Vec::new(),
                exile: Vec::new(),
                adventure: Vec::new(),
                recast: Vec::new(),
                command: d.commanders.clone(),
                casts: [0; 3],
                cmd_damage: [[0; 3]; 8],
                alive: true,
                lands_played: 0,
                spells_this_turn: 0,
                drawn_this_turn: 0,
                attacked_this_turn: false,
                gained_life_turn: false,
                lost_life_turn: false,
                life_gained_turn: 0,
                discarded_turn: 0,
                extra_lands: 0,
                enduring_story: false,
                hijacked: false,
                pool: 0,
                extra_combats: 0,
                grudge: [0.0; 8],
                last_spell: None,
                copy_next: Vec::new(),
                inst_sorc_this_turn: 0,
                last_ability: None,
                mulligans: 0,
            })
            .collect();
        let mut g = Game {
            players,
            perms: Vec::new(),
            next_id: 1,
            round: 0,
            active: 0,
            rng,
            start_life: life,
            died_this_turn: 0,
            pending: Vec::new(),
            fog: false,
            in_combat: false,
            resolved_this_action: 0,
            log: None,
            focus: None,
            tick: 1,
            vcache: Default::default(),
            tcache: std::cell::RefCell::new([(0, 0.0); 8]),
            wcache: std::cell::RefCell::new([[(0, 0.0); 8]; 8]),
            reborn: None,
            entering_x: None,
            cur_x: None,
            casting: None,
            entering_kicked: false,
            last_left: None,
            last_discarded: 0,
            last_countered: Vec::new(),
            last_destroyed: 0,
            last_drawn: 0,
            last_targets: Vec::new(),
        };
        for p in 0..g.players.len() {
            let mut lib = std::mem::take(&mut g.players[p].library);
            g.rng.shuffle(&mut lib);
            g.players[p].library = lib;
            ai::mulligan(&mut g, p);
        }
        g
    }

    /// The card a cast would use, and whether it's the back face.
    pub fn spell_card(&self, p: usize, src: Src, opts: CastOpts) -> Option<(u16, bool)> {
        let pl = &self.players[p];
        Some(match src {
            Src::Hand(h) => (*pl.hand.get(h)?, opts.back),
            Src::Command(k) => (pl.command.get(k)?.0, false),
            Src::Graveyard(k) => (*pl.graveyard.get(k)?, false),
            Src::Adventure(k) => (*pl.adventure.get(k)?, false),
            Src::Prepared(id) => {
                let x = &self.perms[self.idx(id)?];
                if !x.prepared || x.ctrl as usize != p {
                    return None;
                }
                let r = x.card?;
                if r.0 as usize != p {
                    return None;
                }
                (r.1, true)
            }
        })
    }

    /// What casting from `src` costs right now (tax, reductions, extras), if it's allowed.
    pub fn cast_cost(&self, p: usize, src: Src, opts: CastOpts) -> Option<Mana> {
        let (idx, back) = self.spell_card(p, src, opts)?;
        let c = &self.players[p].cards[idx as usize];
        if c.is(T_LAND) && !back {
            return None;
        }
        if let Src::Graveyard(_) = src {
            if !self.players[p].recast.contains(&idx) && c.flashback.is_none() {
                return None;
            }
        }
        let (mut cost, types, subtypes) = if back {
            let b = c.back.as_ref()?;
            if matches!(src, Src::Hand(_)) && b.kind == BackKind::Prepare {
                return None;
            }
            (b.cost, b.types, &c.subtypes)
        } else if let (Src::Graveyard(_), Some(fb), false) =
            (src, c.flashback, self.players[p].recast.contains(&idx))
        {
            (fb, c.types, &c.subtypes)
        } else if opts.overload && c.overload.is_some() {
            (c.overload.unwrap(), c.types, &c.subtypes)
        } else if opts.sneak && c.sneak.is_some() && self.players[p].attacked_this_turn {
            (c.sneak.unwrap(), c.types, &c.subtypes)
        } else {
            (c.cost, c.types, &c.subtypes)
        };
        if cost.x {
            // X spells: spend about half the remaining mana on X.
            cost =
                cost.plus_generic((self.mana_available(p).saturating_sub(cost.total()) / 2).max(1));
        }
        if let Src::Command(k) = src {
            let slot = self.players[p].command[k].1 as usize;
            cost = cost.plus_generic(2 * self.players[p].casts[slot.min(2)] as u32);
        }
        if opts.free {
            return Some(Mana::default());
        }
        // "Spells you cast cost {N} less" (and the like).
        let mut less = 0;
        for x in self.perms.iter().filter(|x| x.ctrl as usize == p) {
            let Some(r) = x.card else { continue };
            if let Some((n, f)) = &self.card(r).cost_reduction {
                let type_ok = f.types == 0 || types & f.types != 0;
                let not_ok = f.not_types == 0 || types & f.not_types == 0;
                let sub_ok = f
                    .subtype
                    .as_ref()
                    .is_none_or(|s| subtypes.iter().any(|t| t == s));
                if type_ok && not_ok && sub_ok {
                    less += *n as u32;
                }
            }
        }
        if c.affinity {
            less += self
                .perms
                .iter()
                .filter(|x| x.ctrl as usize == p && x.is(T_ARTIFACT))
                .count() as u32;
        }
        if opts.convoke && (c.convoke || c.improvise) {
            less += self.convoke_tappers(p, c).len() as u32;
        }
        cost = cost.minus_generic(less);
        if opts.offspring {
            cost = cost.plus_generic(c.offspring.unwrap_or(0) as u32);
        }
        if opts.kicker {
            if let Some(k) = c.kicker {
                cost.generic = cost.generic.saturating_add(k.generic);
                for i in 0..5 {
                    cost.pips[i] += k.pips[i];
                }
            }
        }
        Some(cost)
    }

    /// Creatures (convoke) or artifacts (improvise) that could be tapped to help pay.
    fn convoke_tappers(&self, p: usize, c: &SimCard) -> Vec<u32> {
        self.perms
            .iter()
            .filter(|x| {
                x.ctrl as usize == p
                    && !x.tapped
                    && ((c.convoke && x.is_creature())
                        || (c.improvise && x.is(T_ARTIFACT) && !x.is_creature()))
                    && x.tok != Tok::Treasure
            })
            .map(|x| x.id)
            .collect()
    }

    pub fn can_cast(&self, p: usize, src: Src, opts: CastOpts) -> bool {
        self.cast_cost(p, src, opts)
            .is_some_and(|m| self.can_pay(p, &m))
    }

    /// Casts a spell: pays, moves the card, fires cast triggers, lets opponents respond,
    /// then resolves it. False if it couldn't be cast.
    pub fn cast(&mut self, p: usize, src: Src, opts: CastOpts) -> bool {
        let Some((idx, back)) = self.spell_card(p, src, opts) else {
            return false;
        };
        let Some(cost) = self.cast_cost(p, src, opts) else {
            return false;
        };
        let c: &'a SimCard = &self.players[p].cards[idx as usize];
        // X paid (`cast_cost` spends about half the spare mana on it).
        let x_paid = (!back && c.cost.x && !opts.free)
            .then(|| (self.mana_available(p).saturating_sub(c.cost.total()) / 2).max(1) as i32);
        // Pyromancer's Goggles: its mana casting a red instant or sorcery copies it.
        let goggles = if c.is(T_INSTANT | T_SORCERY) && !back && c.colors & (1 << 3) != 0 {
            self.perms.iter().position(|x| {
                x.ctrl as usize == p
                    && !x.tapped
                    && self.abilities_of(x).iter().any(|a| {
                        a.text
                            .contains("when that mana is spent to cast a red instant or sorcery")
                    })
            })
        } else {
            None
        }
        .map(|i| self.perms[i].id);
        // Convoke / improvise: tap helpers first (the cost already counts them).
        if opts.convoke && (c.convoke || c.improvise) && !opts.free {
            let tappers = self.convoke_tappers(p, c);
            let mut base = self
                .cast_cost(
                    p,
                    src,
                    CastOpts {
                        convoke: false,
                        ..opts
                    },
                )
                .unwrap_or(cost);
            for id in tappers {
                if base.generic == 0 {
                    break;
                }
                if let Some(i) = self.idx(id) {
                    self.perms[i].tapped = true;
                    base.generic -= 1;
                }
            }
            if !self.pay(p, &base) {
                return false;
            }
        } else if !self.pay(p, &cost) {
            return false;
        }
        if let Some(i) = goggles.and_then(|id| self.idx(id)) {
            if !self.perms[i].tapped {
                // Its {R} went into this spell instead of another source's.
                self.perms[i].tapped = true;
                self.players[p].pool = self.players[p].pool.saturating_add(1);
            }
        }
        // Teamwork: tap creatures with enough total power as an extra cost.
        let kicked = opts.kicker && c.kicker.is_some();
        let overloaded = opts.overload && c.overload.is_some();
        let mut teamwork = false;
        if opts.teamwork {
            if let Some(n) = c.teamwork {
                let helpers = ai::teamwork_tappers(self, p, n as i16);
                if !helpers.is_empty() {
                    teamwork = true;
                    for id in helpers {
                        if let Some(i) = self.idx(id) {
                            self.perms[i].tapped = true;
                            let snap = self.perms[i].clone();
                            self.emit_about(Ev::TeamworkTapped, p, &snap);
                        }
                    }
                }
            }
        }
        // The card leaves its zone.
        let mut commander_slot = 0;
        let from_graveyard = matches!(src, Src::Graveyard(_));
        match src {
            Src::Hand(h) => {
                self.players[p].hand.remove(h);
            }
            Src::Command(k) => {
                let (_, slot) = self.players[p].command.remove(k);
                commander_slot = slot;
                self.players[p].casts[slot as usize % 3] += 1;
            }
            Src::Graveyard(k) => {
                self.players[p].graveyard.remove(k);
                self.players[p].recast.retain(|&x| x != idx);
                self.emit(Ev::LeavesGraveyard, p, None);
            }
            Src::Adventure(k) => {
                self.players[p].adventure.remove(k);
            }
            Src::Prepared(id) => {
                if let Some(i) = self.idx(id) {
                    self.perms[i].prepared = false;
                }
            }
        }
        let gift = opts.gift && c.gift;
        if gift {
            if let Some(o) = ai::choose_opponent(self, p, 0) {
                if c.abilities.iter().any(|a| a.text.contains("gift a card")) {
                    self.draw(o, 1);
                } else {
                    self.make_item(o, "treasure");
                }
            }
        }
        let spell_types = if back {
            c.back.as_ref().map(|b| b.types).unwrap_or(T_SORCERY)
        } else {
            c.types
        };
        let probe_card = SimCard {
            types: spell_types,
            ..c.clone()
        };
        let outer = (self.casting, self.cur_x);
        self.casting = Some(((p as u8, idx), back));
        self.cur_x = x_paid;
        if spell_types & (T_INSTANT | T_SORCERY) != 0 {
            self.players[p].inst_sorc_this_turn =
                self.players[p].inst_sorc_this_turn.saturating_add(1);
        }
        self.on_cast(p, &probe_card);
        self.note(|| {
            format!(
                "P{} casts {}{}",
                p,
                c.name,
                if back { " (back face)" } else { "" }
            )
        });
        // Opponents may counter it.
        if !c.cant_be_countered && ai::respond_to_spell(self, p, idx, back) {
            self.note(|| format!("  {} is countered", c.name));
            match src {
                Src::Prepared(_) => {}
                _ if commander_slot > 0 => self.players[p].command.push((idx, commander_slot)),
                _ => self.players[p].graveyard.push(idx),
            }
            self.run_pending();
            (self.casting, self.cur_x) = outer;
            return true;
        }
        let permanent = !back && c.is_permanent();
        if permanent {
            self.entering_x = x_paid;
            self.entering_kicked = kicked;
            let entered = self.enter_card(p, idx, p, commander_slot > 0);
            let Some(i) = entered else {
                self.run_pending();
                self.sba();
                (self.casting, self.cur_x) = outer;
                return true;
            };
            let id = self.perms[i].id;
            self.perms[i].commander = commander_slot;
            for pend in self.pending.iter_mut().filter(|x| x.source == Some(id)) {
                pend.teamwork = teamwork;
                pend.gift = gift;
                pend.kicked = kicked;
            }
            // Auras attach as they enter.
            if c.is(T_AURA) {
                if let Some(t) = ai::aura_target(self, p, idx) {
                    let tid = self.perms[t].id;
                    if let Some(j) = self.idx(id) {
                        self.perms[j].attached = Some(tid);
                    }
                }
            }
            if opts.offspring && c.offspring.is_some() {
                let t = self.new_perm(p, p, Some((p as u8, idx)), Tok::Other);
                self.perms[t].base_p = 1;
                self.perms[t].base_t = 1;
                self.after_enter(t);
            }
        } else {
            let abilities: &'a [SimAbility] = if back {
                c.back
                    .as_ref()
                    .map(|b| b.abilities.as_slice())
                    .unwrap_or(&[])
            } else {
                &c.abilities
            };
            // "When you next cast an instant or sorcery spell this turn, copy that spell"
            // (Loki: mana value up to his power) and Pyromancer's Goggles.
            if spell_types & (T_INSTANT | T_SORCERY) != 0 {
                let mv = if back {
                    c.back.as_ref().map_or(c.mv, |b| b.cost.total() as u8)
                } else {
                    c.mv
                };
                if let Some(k) = self.players[p].copy_next.iter().position(|&l| mv <= l) {
                    self.players[p].copy_next.remove(k);
                    self.copy_spell(p, ((p as u8, idx), back));
                }
                if goggles.is_some() {
                    self.copy_spell(p, ((p as u8, idx), back));
                }
            }
            for (k, a) in abilities.iter().enumerate() {
                if a.trig == Trig::Spell
                    || (back && a.trig != Trig::Static && !matches!(a.trig, Trig::Activated { .. }))
                {
                    self.pending.push(Pending {
                        ctrl: p as u8,
                        source: None,
                        card: (p as u8, idx),
                        back,
                        ability: k as u16,
                        snapshot: None,
                        subject: None,
                        teamwork,
                        gift,
                        from_graveyard,
                        kicked,
                        overloaded,
                    });
                }
            }
            self.run_pending();
            if c.is(T_INSTANT | T_SORCERY) || back {
                self.players[p].last_spell = Some(((p as u8, idx), back));
            }
            // Where the card goes.
            let kind = c.back.as_ref().map(|b| b.kind);
            match src {
                Src::Prepared(_) => {}
                Src::Hand(_) if back && kind == Some(BackKind::Adventure) => {
                    self.players[p].adventure.push(idx)
                }
                Src::Graveyard(_) => self.players[p].exile.push(idx),
                _ if commander_slot > 0 => self.players[p].command.push((idx, commander_slot)),
                _ => self.players[p].graveyard.push(idx),
            }
        }
        self.run_pending();
        self.sba();
        (self.casting, self.cur_x) = outer;
        true
    }

    /// The fodder an activated "sacrifice a ..." cost would use.
    fn sac_fodder(&self, p: usize, src_id: u32, text: &str) -> Option<usize> {
        let types = if text.contains("sacrifice a creature")
            || text.contains("sacrifice another creature")
        {
            T_CREATURE
        } else if text.contains("sacrifice an artifact")
            || text.contains("sacrifice another artifact")
        {
            T_ARTIFACT
        } else if text.contains("sacrifice a land") {
            T_LAND
        } else {
            0xFFFF & !T_LAND
        };
        ai::fodder(self, p, types, src_id)
    }

    /// The mana an activated ability needs now (None if it can't be activated).
    pub fn activation_cost(&self, p: usize, i: usize, k: usize) -> Option<Mana> {
        let x = &self.perms[i];
        if x.ctrl as usize != p {
            return None;
        }
        let ab = self.abilities_of(x).get(k)?;
        if ab.from_graveyard {
            return None; // used from the graveyard only
        }
        match &ab.trig {
            Trig::Activated {
                mana,
                tap,
                sac_other,
                discard,
                life,
                ..
            } => {
                if ab
                    .effects
                    .iter()
                    .any(|e| matches!(e, crate::abilities::Effect::AddMana(_)))
                    && *tap
                    && *mana == 0
                {
                    return None; // mana abilities are used as mana sources
                }
                if *tap && (x.tapped || (x.is_creature() && x.sick && !x.has(HASTE))) {
                    return None;
                }
                if *sac_other && self.sac_fodder(p, x.id, &ab.text).is_none() {
                    return None;
                }
                if *discard && self.players[p].hand.is_empty() {
                    return None;
                }
                if *life as i32 >= self.players[p].life {
                    return None;
                }
                if ab.once && x.powered {
                    return None;
                }
                if x.activations >= 3 {
                    return None; // keep loops in check
                }
                Some(Mana::generic(*mana as u32))
            }
            Trig::PowerUp(full) => {
                if x.powered {
                    return None;
                }
                // The turn it entered, the cost is reduced by the permanent's mana cost:
                // its coloured pips come off matching pips, the rest off generic.
                let mut cost = *full;
                if x.born == self.round {
                    if let Some(r) = x.card {
                        let own = self.card(r).cost;
                        let mut spare = own.generic as u32;
                        for i in 0..5 {
                            let off = own.pips[i].min(cost.pips[i]);
                            cost.pips[i] -= off;
                            spare += (own.pips[i] - off) as u32;
                        }
                        cost = cost.minus_generic(spare);
                    }
                }
                Some(cost)
            }
            Trig::Loyalty(n) => {
                if x.loyalty_used || x.loyalty + (*n as i16) < 0 {
                    return None;
                }
                Some(Mana::default())
            }
            _ => None,
        }
    }

    /// Activates ability `k` of permanent `id` (paying all its costs). False if it couldn't.
    pub fn activate(&mut self, p: usize, id: u32, k: usize) -> bool {
        let Some(i) = self.idx(id) else { return false };
        let Some(cost) = self.activation_cost(p, i, k) else {
            return false;
        };
        if !self.pay(p, &cost) {
            return false;
        }
        let Some(i) = self.idx(id) else { return false };
        let x = self.perms[i].clone();
        let ab = &self.abilities_of(&x)[k];
        let mut snapshot = None;
        match &ab.trig {
            Trig::Activated {
                tap,
                sac_self,
                sac_other,
                discard,
                life,
                ..
            } => {
                if *tap {
                    self.perms[i].tapped = true;
                    self.emit(Ev::Tapped, p, Some(id));
                }
                let Some(i) = self.idx(id) else { return false };
                if self.perms[i].is_creature() {
                    self.emit(Ev::AbilityActivated, p, Some(id));
                }
                let Some(i) = self.idx(id) else { return false };
                self.perms[i].activations += 1;
                if ab.once {
                    self.perms[i].powered = true;
                }
                if *life > 0 {
                    self.lose_life(p, *life as i32);
                }
                if *discard {
                    self.discard(p);
                }
                if *sac_other {
                    if let Some(f) = self.sac_fodder(p, id, &ab.text) {
                        self.sacrifice(f);
                    }
                }
                if *sac_self {
                    snapshot = Some(x.clone());
                    if let Some(i) = self.idx(id) {
                        self.sacrifice(i);
                    }
                }
            }
            Trig::PowerUp(_) => self.perms[i].powered = true,
            Trig::Loyalty(n) => {
                self.perms[i].loyalty += *n as i16;
                self.perms[i].loyalty_used = true;
                self.emit(Ev::LoyaltyActivated, p, Some(id));
            }
            _ => return false,
        }
        let card = x.card.unwrap_or((p as u8, NO_CARD));
        let snapshot =
            snapshot.or_else(|| (x.card.is_none() || x.tok != Tok::None).then(|| x.clone()));
        self.note(|| format!("P{} activates {}", p, ab.text));
        self.pending.push(Pending {
            ctrl: p as u8,
            source: Some(id),
            card,
            back: false,
            ability: k as u16,
            snapshot,
            subject: None,
            teamwork: false,
            gift: false,
            from_graveyard: false,
            kicked: false,
            overloaded: false,
        });
        self.run_pending();
        self.sba();
        true
    }

    /// The mana ability `k` of the card at `gy` in `p`'s graveyard needs to be activated
    /// from there ("{1}{B}: Return ~ from your graveyard to the battlefield"), if it can be.
    pub fn graveyard_activation_cost(&self, p: usize, gy: usize, k: usize) -> Option<Mana> {
        let idx = *self.players[p].graveyard.get(gy)?;
        let c = &self.players[p].cards[idx as usize];
        let ab = c.abilities.get(k)?;
        let Trig::Activated {
            mana,
            sac_other,
            discard,
            life,
            ..
        } = &ab.trig
        else {
            return None;
        };
        if !ab.from_graveyard || !self.cond_holds(&ab.cond, p, None) {
            return None;
        }
        let cost = ab.text.split(": ").next().unwrap_or("");
        if *sac_other && self.sac_fodder(p, u32::MAX, &ab.text).is_none()
            || *discard && self.players[p].hand.is_empty()
            || *life as i32 >= self.players[p].life
        {
            return None;
        }
        // "Exile another creature card from your graveyard" (Gallia).
        if cost.contains("exile another creature card from your graveyard")
            && self.other_creature_card(p, gy).is_none()
        {
            return None;
        }
        if ab.text.contains("activate only as a sorcery") && self.active != p {
            return None;
        }
        let m = Mana::parse(cost);
        Some(if m.total() == 0 && *mana > 0 {
            Mana::generic(*mana as u32)
        } else {
            m
        })
    }

    /// Another creature card in `p`'s graveyard (the least valuable), not the one at `gy`.
    fn other_creature_card(&self, p: usize, gy: usize) -> Option<usize> {
        let pl = &self.players[p];
        (0..pl.graveyard.len())
            .filter(|&j| j != gy && pl.cards[pl.graveyard[j] as usize].is(T_CREATURE))
            .min_by(|&a, &b| {
                let va = pl.cards[pl.graveyard[a] as usize].value;
                let vb = pl.cards[pl.graveyard[b] as usize].value;
                va.partial_cmp(&vb).unwrap()
            })
    }

    /// Activates ability `k` of the card at `gy` in `p`'s graveyard, paying its costs.
    pub fn activate_from_graveyard(&mut self, p: usize, gy: usize, k: usize) -> bool {
        let Some(cost) = self.graveyard_activation_cost(p, gy, k) else {
            return false;
        };
        if !self.pay(p, &cost) {
            return false;
        }
        let idx = self.players[p].graveyard[gy];
        let c: &'a SimCard = &self.players[p].cards[idx as usize];
        let ab = &c.abilities[k];
        let Trig::Activated {
            sac_other,
            discard,
            life,
            ..
        } = ab.trig
        else {
            return false;
        };
        let cost_text = ab.text.split(": ").next().unwrap_or("");
        if life > 0 {
            self.lose_life(p, life as i32);
        }
        if discard {
            self.discard(p);
        }
        if sac_other {
            if let Some(f) = self.sac_fodder(p, u32::MAX, &ab.text) {
                self.sacrifice(f);
            }
        }
        if cost_text.contains("exile another creature card from your graveyard") {
            if let Some(j) = self.other_creature_card(p, gy) {
                let e = self.players[p].graveyard.remove(j);
                self.players[p].exile.push(e);
            }
        }
        // "Exile ~ from your graveyard: ..." - the card itself is the cost.
        if cost_text.contains("exile ~ from your graveyard") {
            if let Some(j) = self.players[p].graveyard.iter().position(|&x| x == idx) {
                self.players[p].graveyard.remove(j);
                self.players[p].exile.push(idx);
            }
        }
        self.note(|| format!("P{} activates {} from the graveyard", p, c.name));
        self.pending.push(Pending {
            ctrl: p as u8,
            source: None,
            card: (p as u8, idx),
            back: false,
            ability: k as u16,
            snapshot: None,
            subject: None,
            teamwork: false,
            gift: false,
            from_graveyard: true,
            kicked: false,
            overloaded: false,
        });
        self.run_pending();
        self.sba();
        true
    }

    pub fn equip(&mut self, p: usize, equip_id: u32, target_id: u32) -> bool {
        let Some(e) = self.idx(equip_id) else {
            return false;
        };
        let Some(r) = self.perms[e].card else {
            return false;
        };
        let cost = Mana::generic(self.card(r).equip.unwrap_or(2) as u32);
        if self.perms[e].attached == Some(target_id) || !self.pay(p, &cost) {
            return false;
        }
        if let Some(e) = self.idx(equip_id) {
            self.perms[e].attached = Some(target_id);
        }
        self.recalc();
        true
    }

    pub fn crew(&mut self, p: usize, vehicle_id: u32) -> bool {
        let Some(v) = self.idx(vehicle_id) else {
            return false;
        };
        let Some(r) = self.perms[v].card else {
            return false;
        };
        let need = self.card(r).crew.unwrap_or(2) as i16;
        let helpers = ai::crew_tappers(self, p, need, vehicle_id);
        if helpers.is_empty() {
            return false;
        }
        for id in helpers {
            if let Some(i) = self.idx(id) {
                self.perms[i].tapped = true;
            }
        }
        if let Some(v) = self.idx(vehicle_id) {
            self.perms[v].crewed = true;
            self.perms[v].sick = false;
        }
        self.recalc();
        true
    }

    pub fn cycle(&mut self, p: usize, h: usize) -> bool {
        let c = &self.players[p].cards[self.players[p].hand[h] as usize];
        let Some((cost, find)) = c.cycling.clone() else {
            return false;
        };
        if !self.pay(p, &Mana::generic(cost as u32)) {
            return false;
        }
        let card = self.players[p].hand.remove(h);
        self.players[p].graveyard.push(card);
        self.emit(Ev::Discard, p, None);
        if find.is_empty() {
            self.draw(p, 1);
        } else {
            // Landcycling / typecycling: search for a basic land, a land type or a creature type.
            let pl = &self.players[p];
            let k = if find == "land" {
                ai::land_to_fetch(self, p, true)
            } else {
                (0..pl.library.len()).find(|&k| {
                    let lc = &pl.cards[pl.library[k] as usize];
                    lc.has_subtype(&find) || lc.name.to_lowercase() == find
                })
            };
            if let Some(k) = k {
                let l = self.players[p].library.remove(k);
                self.players[p].hand.push(l);
            }
        }
        self.run_pending();
        true
    }

    pub fn play_land(&mut self, p: usize, h: usize) {
        let c = self.players[p].hand.remove(h);
        self.players[p].lands_played += 1;
        self.enter_card(p, c, p, false);
        self.run_pending();
        self.sba();
    }

    /// Triggers of "at the beginning of" a phase for the active player `a`.
    fn phase_triggers(&mut self, a: usize, phase: Phase) {
        let mut add = Vec::new();
        for x in &self.perms {
            for (k, ab) in self.abilities_of(x).iter().enumerate() {
                if let Trig::Phase {
                    phase: ph,
                    any_player,
                    opponents_only,
                } = ab.trig
                {
                    let mine = x.ctrl as usize == a;
                    if ph == phase
                        && (mine && !opponents_only || !mine && any_player)
                        && !ab.from_graveyard
                    {
                        add.push(Pending {
                            ctrl: x.ctrl,
                            source: Some(x.id),
                            card: x.card.unwrap_or((x.ctrl, NO_CARD)),
                            back: false,
                            ability: k as u16,
                            snapshot: (x.card.is_none() || x.tok != Tok::None).then(|| x.clone()),
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
        // Cards that act from their owner's graveyard (Flamewake Phoenix, Command the Stage).
        for (o, pl) in self.players.iter().enumerate() {
            if !pl.alive {
                continue;
            }
            let mut seen: Vec<u16> = Vec::new();
            for &idx in &pl.graveyard {
                if seen.contains(&idx) {
                    continue;
                }
                seen.push(idx);
                for (k, ab) in pl.cards[idx as usize].abilities.iter().enumerate() {
                    if let Trig::Phase {
                        phase: ph,
                        any_player,
                        ..
                    } = ab.trig
                    {
                        if ph == phase && ab.from_graveyard && (o == a || any_player) {
                            add.push(Pending {
                                ctrl: o as u8,
                                source: None,
                                card: (o as u8, idx),
                                back: false,
                                ability: k as u16,
                                snapshot: None,
                                subject: None,
                                teamwork: false,
                                gift: false,
                                from_graveyard: true,
                                kicked: false,
                                overloaded: false,
                            });
                        }
                    }
                }
            }
        }
        self.pending.extend(add);
        self.run_pending();
        self.sba();
    }

    /// One full turn for player `a`.
    pub fn take_turn(&mut self, a: usize, first_turn: bool) {
        self.active = a;
        self.died_this_turn = 0;
        for pl in self.players.iter_mut() {
            pl.lands_played = 0;
            pl.spells_this_turn = 0;
            pl.inst_sorc_this_turn = 0;
            pl.copy_next.clear();
            pl.drawn_this_turn = 0;
            pl.attacked_this_turn = false;
            pl.gained_life_turn = false;
            pl.lost_life_turn = false;
            pl.life_gained_turn = 0;
            pl.discarded_turn = 0;
            pl.extra_lands = 0;
            for g in pl.grudge.iter_mut() {
                *g *= 0.8;
            }
        }
        // Goad lasts until the goading player's next turn; once-a-turn triggers reset.
        for x in self.perms.iter_mut() {
            if x.goad == Some(a as u8) {
                x.goad = None;
            }
            x.fired = 0;
        }
        // Untap.
        // "~ doesn't untap during your untap step" (Bombur: unless an enduring story).
        let storied = self.players[a].enduring_story;
        let stays: Vec<u32> = self
            .perms
            .iter()
            .filter(|x| x.ctrl as usize == a && !x.no_abilities)
            .filter(|x| {
                x.card.is_some_and(|r| {
                    let c = self.card(r);
                    c.no_untap
                        && !(storied
                            && c.abilities
                                .iter()
                                .any(|ab| ab.text.contains("unless you have an enduring story")))
                })
            })
            .map(|x| x.id)
            .collect();
        let untapping: Vec<u32> = self
            .perms
            .iter()
            .filter(|x| x.ctrl as usize == a && x.tapped && x.stun == 0 && !stays.contains(&x.id))
            .map(|x| x.id)
            .collect();
        for x in self.perms.iter_mut().filter(|x| x.ctrl as usize == a) {
            if stays.contains(&x.id) {
                x.sick = false;
                x.loyalty_used = false;
                x.activations = 0;
                continue;
            }
            if x.stun > 0 && x.tapped {
                x.stun -= 1;
            } else {
                x.tapped = false;
            }
            x.sick = false;
            x.loyalty_used = false;
            x.activations = 0;
        }
        self.recalc();
        for id in untapping {
            self.emit(Ev::Untapped, a, Some(id));
        }
        if self.log.is_some() {
            let lands = self
                .perms
                .iter()
                .filter(|x| x.ctrl as usize == a && x.is(T_LAND))
                .count();
            let creatures: Vec<String> = self
                .perms
                .iter()
                .filter(|x| x.ctrl as usize == a && x.is_creature())
                .map(|x| format!("{}/{}", x.p, x.t))
                .collect();
            let (life, hand, round) =
                (self.players[a].life, self.players[a].hand.len(), self.round);
            let names: Vec<String> = self.players[a]
                .hand
                .iter()
                .map(|&c| {
                    let c = &self.players[a].cards[c as usize];
                    format!("{} {}", c.name, c.mv)
                })
                .collect();
            self.note(|| {
                format!(
                    "-- P{} round {}: life {}, {} lands, hand {}, creatures [{}] | hand: {}",
                    a,
                    round,
                    life,
                    lands,
                    hand,
                    creatures.join(" "),
                    names.join("; ")
                )
            });
        }
        let hijacked = std::mem::take(&mut self.players[a].hijacked);
        self.phase_triggers(a, Phase::Upkeep);
        if !first_turn {
            self.draw(a, 1);
        }
        self.run_pending();
        self.sba();
        if !self.players[a].alive {
            return;
        }
        // Sagas gain a lore counter after the draw.
        let sagas: Vec<u32> = self
            .perms
            .iter()
            .filter(|x| x.ctrl as usize == a && x.is(T_SAGA))
            .map(|x| x.id)
            .collect();
        for id in sagas {
            if let Some(i) = self.idx(id) {
                self.add_other_counters(i, 1);
                self.run_pending();
                if let Some(i) = self.idx(id) {
                    let ch = self.perms[i]
                        .card
                        .map(|r| self.card(r).chapters)
                        .unwrap_or(3) as i16;
                    if self.perms[i].other_counters >= ch {
                        self.sacrifice(i);
                    }
                }
            }
        }
        if hijacked {
            // An opponent runs this turn (MSH Plans): they play a land and nothing useful.
            if let Some(h) = self.players[a]
                .hand
                .iter()
                .position(|&c| self.players[a].cards[c as usize].is(T_LAND))
            {
                self.play_land(a, h);
            }
        } else {
            self.phase_triggers(a, Phase::Main);
            ai::main_phase(self, a, false);
            let combats = 1 + self.players[a].extra_combats as usize;
            for _ in 0..combats {
                if self.alive_count() <= 1 || !self.players[a].alive {
                    break;
                }
                self.phase_triggers(a, Phase::Combat);
                self.combat(a);
            }
            self.players[a].extra_combats = 0;
            if self.players[a].alive && self.alive_count() > 1 {
                ai::main_phase(self, a, true);
            }
        }
        if !self.players[a].alive || self.alive_count() <= 1 {
            return;
        }
        // Token copies made "with haste and 'at the beginning of the end step, sacrifice this
        // token'" go now.
        let doomed: Vec<u32> = self
            .perms
            .iter()
            .filter(|x| x.sac_at_end)
            .map(|x| x.id)
            .collect();
        for id in doomed {
            if let Some(i) = self.idx(id) {
                self.sacrifice(i);
            }
        }
        self.phase_triggers(a, Phase::End);
        // Other players use instant-speed tricks at the end of the turn - mostly the next
        // player, whose mana is about to untap.
        let n = self.players.len();
        for k in 1..n {
            let o = (a + k) % n;
            if self.players[o].alive && self.alive_count() > 1 {
                ai::end_of_turn(self, o, k == 1);
            }
        }
        // Cleanup.
        while self.players[a].hand.len() > 7 {
            self.discard(a);
        }
        for x in self.perms.iter_mut() {
            x.damage = 0;
            x.deathtouched = false;
            x.temp_p = 0;
            x.temp_t = 0;
            x.temp_kw = 0;
            x.crewed = false;
            if x.threaten_from.is_some() && x.ctrl as usize == a {
                x.ctrl = x.threaten_from.take().unwrap_or(x.ctrl);
            }
        }
        for pl in self.players.iter_mut() {
            pl.pool = 0;
        }
        self.fog = false;
        self.run_pending();
        self.sba();
    }

    /// A rough standing for games that hit the round limit.
    fn standing(&self, p: usize) -> f32 {
        if !self.players[p].alive {
            return f32::MIN;
        }
        let board: f32 = (0..self.perms.len())
            .filter(|&i| self.perms[i].ctrl as usize == p)
            .map(|i| ai::perm_value(self, i))
            .sum();
        self.players[p].life as f32 + 0.5 * board - 3.0 * self.players[p].poison as f32
    }
}

/// How the simulator plays an effect: "exact" (as the rules say, within the game model),
/// "approximated" (the main part, simplified) or "no effect" - with a short note.
pub fn effect_handling(e: &crate::abilities::Effect) -> (&'static str, &'static str) {
    use crate::abilities::Effect as E;
    match e {
        E::Destroy(_) => (
            "exact",
            "destroy; indestructible and shield counters stop it",
        ),
        E::Exile { .. } => ("exact", "exile; \"until ~ leaves\" cards come back"),
        E::Damage { .. } => (
            "exact",
            "damage to creatures, planeswalkers or players (AI picks)",
        ),
        E::Fight(_) => ("exact", "your best creature fights the target"),
        E::Shrink { .. } => ("exact", "-N/-N until end of turn or as counters"),
        E::Bounce(_) => ("exact", "returns to hand; tokens cease to exist"),
        E::Lock { .. } => (
            "approximated",
            "can't attack or block (abilities kept); short locks tap and stun",
        ),
        E::Edict { .. } => (
            "exact",
            "each opponent / target opponent sacrifices their worst",
        ),
        E::CounterSpell => (
            "exact",
            "cast in response by opponents holding it, with \"unless pays\"",
        ),
        E::Draw { .. } => (
            "exact",
            "draw (X counted from the board where the text says how)",
        ),
        E::Loot => (
            "exact",
            "draw then discard the worst card; Recruit makes its 1/1",
        ),
        E::Discard { .. } => (
            "exact",
            "discard (their worst; \"you choose\" takes their best)",
        ),
        E::Mill { .. } => ("exact", "mill yourself or opponents"),
        E::Scry(_) => ("exact", "scry / surveil keep what the next turns need"),
        E::Tokens { .. } => ("exact", "creature tokens with size, keywords and types"),
        E::ItemTokens { .. } => (
            "exact",
            "Treasure, Food, Clue, Map, Blood with their abilities; other kinds have none",
        ),
        E::PlusCounters { .. } => ("exact", "+1/+1 counters (doublers apply)"),
        E::Pump { .. } => (
            "exact",
            "until end of turn, static anthems and Equipment/Aura bonuses",
        ),
        E::Grant { .. } => ("exact", "keywords until end of turn or static"),
        E::GainLife(_) => ("exact", "life gain (\"players can't gain life\" respected)"),
        E::Drain { .. } => (
            "exact",
            "life loss or damage to each / target opponent, gain if the text says",
        ),
        E::AddMana(_) => (
            "exact",
            "mana abilities are mana sources; one-off mana goes to the pool",
        ),
        E::FetchLand { .. } => (
            "exact",
            "searches for a land of a needed colour (tapped if the text says)",
        ),
        E::Reanimate => ("exact", "best creature from your (or any) graveyard"),
        E::Regrowth => ("exact", "best card back to hand"),
        E::Untap(_) => ("exact", "untaps a land or creature"),
        E::GainControl(_) => (
            "exact",
            "steal; \"until end of turn\" returns it and grants haste",
        ),
        E::Copy => (
            "approximated",
            "token copy of a creature; spell copies repeat your last instant/sorcery; \"becomes a copy\" takes size and keywords",
        ),
        E::Empower(_) => ("exact", "Jace token with [-1] surveil 1 / [-3] draw"),
        E::Amass(_) => ("exact", "Army token with counters"),
        E::Goad => ("exact", "must attack someone else until your next turn"),
        E::Proliferate => ("exact", "your counters, loyalty, and opponents' poison"),
        E::ExtraCombat => ("exact", "another combat this turn"),
        E::Blink => ("exact", "exile and return: enters triggers fire again"),
        E::Tutor => ("exact", "best matching card to hand, top or battlefield"),
        E::FreeSpell => (
            "approximated",
            "casts the best affordable card from hand, or the top card, for free",
        ),
        E::Prepared => (
            "exact",
            "the creature becomes prepared; its spell copy can be cast",
        ),
        E::Attach => ("exact", "attaches Equipment / Auras to the best creature"),
        E::Doubler(_) => (
            "approximated",
            "doubles tokens, counters or damage (no stacking order)",
        ),
        E::Fog => ("exact", "prevents combat damage this turn"),
        E::Dig => (
            "approximated",
            "look at the top N (3 if unnamed), take the best",
        ),
        E::OtherCounters(_) => (
            "approximated",
            "shield, stun, loyalty, keyword counters exact; others (plan, lore, hone...) counted for counter goals",
        ),
        E::Recast => (
            "exact",
            "the best instant/sorcery in your graveyard may be cast again (then exiled)",
        ),
        E::Rule(label) => {
            if resolve::rule_modelled(label) {
                ("approximated", "special rule acted on")
            } else {
                (
                    "no effect",
                    "special rule named, no effect in a simplified game",
                )
            }
        }
    }
}

/// Keywords the simulator plays (all of the parser's list).
pub const KEYWORDS_PLAYED: &[&str] = &[
    "flying",
    "reach",
    "trample",
    "menace",
    "deathtouch",
    "lifelink",
    "haste",
    "first strike",
    "double strike",
    "vigilance",
    "indestructible",
    "hexproof",
    "shroud",
    "defender",
    "flash",
    "prowess",
    "ward",
    "protection",
    "storied",
    "teamwork",
    "equip",
    "crew",
    "partner",
    "convoke",
    "improvise",
    "affinity",
    "extort",
    "changeling",
    "landcycling",
    "basic landcycling",
    "cycling",
    "enchant",
    "living weapon",
    "offspring",
    "flanking",
    "fear",
    "intimidate",
    "skulk",
    "toxic",
    "gift",
    "flashback",
    "kicker",
    "overload",
    "sneak",
];

/// Rounds before a game is called on life and board.
pub const MAX_ROUNDS: u32 = 25;

/// Plays one game; returns the winner's seat.
pub fn play_game(decks: &[&Prepared], rules: &Rules, rng: &mut Rng) -> usize {
    let seed = rng.next();
    let mut g = Game::new(decks, rules, Rng::new(seed));
    play(&mut g)
}

pub fn play(g: &mut Game) -> usize {
    let n = g.players.len();
    for round in 1..=MAX_ROUNDS {
        g.round = round;
        for a in 0..n {
            if !g.players[a].alive {
                continue;
            }
            g.take_turn(a, round == 1 && a == 0);
            g.sba();
            let alive: Vec<usize> = (0..n).filter(|&i| g.players[i].alive).collect();
            match alive.len() {
                0 => return a,
                1 => return alive[0],
                _ => {}
            }
        }
    }
    (0..n)
        .max_by(|&x, &y| g.standing(x).partial_cmp(&g.standing(y)).unwrap())
        .unwrap_or(0)
}

/// Win rate of `deck` in `games` 4-player games against random picks from `gauntlet`
/// (25% = as good as the gauntlet decks). `seed` makes results repeatable, so different
/// decks are tested on the same shuffles of opponents.
pub fn win_rate(
    deck: &Deck,
    gauntlet: &[Prepared],
    db: &CardDb,
    rules: &Rules,
    games: u32,
    seed: u64,
) -> f32 {
    if gauntlet.is_empty() {
        return 0.25;
    }
    let me = Prepared::new(deck, db);
    let mut rng = Rng::new(seed);
    let seats = rules.players.clamp(2, 4) as usize;
    let mut wins = 0;
    for g in 0..games {
        // Three different opponents when there are enough (like a real pod).
        let mut picks: Vec<usize> = (0..gauntlet.len()).collect();
        rng.shuffle(&mut picks);
        let mut table: Vec<&Prepared> = (1..seats)
            .map(|i| &gauntlet[picks[(i - 1) % picks.len()]])
            .collect();
        // Rotate our seat so turn order evens out.
        let seat = g as usize % seats;
        table.insert(seat, &me);
        if play_game(&table, rules, &mut rng) == seat {
            wins += 1;
        }
    }
    wins as f32 / games.max(1) as f32
}

/// Opponent decks for playtesting: one per other player in the league, auto-built from a
/// simulated pool of the packs that player has by this point of the season (their own
/// Marvel / Hobbit mix from `rules.roster`).
pub fn gauntlet(db: &CardDb, rules: &Rules, seed: u64) -> Vec<Prepared> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::new();
    for player in &rules.roster.others {
        // A pool with no legendary creature can't make a deck; reopen (rare).
        for _ in 0..4 {
            let pool = crate::packs::simulated_pool(db, player, rules.session.max(1), &mut rng);
            let best = db
                .cards
                .iter()
                .filter(|c| {
                    c.can_be_commander() && c.name != crate::deck::PIPER && pool.owned(&c.name) > 0
                })
                .map(|c| {
                    let colors = crate::analysis::best_colors(c, db, &pool, rules);
                    (
                        crate::analysis::pool_quality_for(c, colors, db, &pool, rules),
                        c,
                    )
                })
                .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            if let Some((_, cmd)) = best {
                let deck = crate::analysis::autobuild(cmd, db, &pool, rules, None, None).0;
                out.push(Prepared::new(&deck, db));
                break;
            }
        }
    }
    out
}
