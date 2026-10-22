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
pub mod audit;
pub mod combat;
pub mod compile;
pub mod lab;
pub mod probe;
pub mod resolve;
#[cfg(test)]
mod restrict_tests;
pub mod state;
pub mod targets;
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
    /// The commanders' names (for logs).
    pub fn commanders_names(&self) -> Vec<String> {
        self.commanders
            .iter()
            .map(|(i, _)| self.cards[*i as usize].name.clone())
            .collect()
    }

    /// Cards in the library at the start (the deck without its commanders).
    pub fn library_size(&self) -> usize {
        self.library.len()
    }

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
    /// Index into the cards in exile the player may play (`Player::playable`).
    Exile(usize),
}

/// How an additional cost to cast a spell is paid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExtraPay {
    /// Sacrifice this permanent (by id).
    Sacrifice(u32),
    Discard,
    Mana(Mana),
    /// Beheld: nothing to pay.
    Nothing,
}

/// `a` plus `b` (generic, coloured, colourless and hybrid symbols).
pub fn add_mana(mut a: Mana, b: &Mana) -> Mana {
    a.generic = a.generic.saturating_add(b.generic);
    a.colorless = a.colorless.saturating_add(b.colorless);
    for i in 0..5 {
        a.pips[i] = a.pips[i].saturating_add(b.pips[i]);
    }
    for &h in b.hybrid.iter().filter(|&&h| h != 0) {
        match a.hybrid.iter().position(|&s| s == 0) {
            Some(slot) => a.hybrid[slot] = h,
            None => a.generic = a.generic.saturating_add(1),
        }
    }
    a
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
        Game::new_audited(decks, rules, rng, false, false)
    }

    /// A game with a log (`log`) and/or statistics (`stats`) from the start, so mulligans
    /// are recorded too. A log comes with statistics (they name who eliminated whom).
    pub fn new_audited(
        decks: &[&'a Prepared],
        rules: &Rules,
        rng: Rng,
        log: bool,
        stats: bool,
    ) -> Game<'a> {
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
                lost_life_last_turn: false,
                scried_turn: false,
                loyalty_turn: false,
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
                px: Default::default(),
                extra_turns: 0,
                no_powerups: false,
                creature_spells_turn: 0,
                noncreature_spells_turn: 0,
                gy_types_used: 0,
                playable: Vec::new(),
                toughness_damage_turn: false,
                emblems: Vec::new(),
                equips_turn: 0,
                turn_reduction: Vec::new(),
                free_next: None,
                attacked_power: 0,
                attack_bonus: None,
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
            dead: Vec::new(),
            last_excess: 0,
            focus_card: None,
            pending: Vec::new(),
            fog: false,
            in_combat: false,
            resolved_this_action: 0,
            log: None,
            focus: None,
            last_targets: Vec::new(),
            last_ctrl: None,
            targeted: false,
            actor: 0,
            tick: 1,
            vcache: Default::default(),
            tcache: std::cell::RefCell::new([(0, 0.0, 0.0); 8]),
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
            ext: Default::default(),
            rules: [0; 8],
            end_turn: false,
            stats: (log || stats).then(|| Box::new(audit::Stats::new(decks.len()))),
            cause: Default::default(),
            leave_why: Default::default(),
            effect_kind: "other",
            dmg_flag: false,
            quiet_enter: false,
        };
        if log {
            g.log = Some(Vec::new());
        }
        if let Some(s) = g.stats.as_mut() {
            for (p, d) in decks.iter().enumerate() {
                s.players[p].commander = d.commanders_names().join(" + ");
            }
        }
        for p in 0..g.players.len() {
            g.players[p].px.chosen_type = g.most_common_type(p);
            g.players[p].px.identity = decks[p]
                .commanders
                .iter()
                .fold(0, |m, &(c, _)| m | decks[p].cards[c as usize].identity);
        }
        for p in 0..g.players.len() {
            let mut lib = std::mem::take(&mut g.players[p].library);
            g.rng.shuffle(&mut lib);
            g.players[p].library = lib;
            ai::mulligan(&mut g, p);
            // "If ~ is in your opening hand, you may begin the game with it on the
            // battlefield" (Quicksilver, Brash Blur).
            let openers: Vec<u16> = g.players[p]
                .hand
                .iter()
                .copied()
                .filter(|&c| {
                    g.players[p].cards[c as usize].abilities.iter().any(|a| {
                        a.text
                            .contains("is in your opening hand, you may begin the game with")
                    })
                })
                .collect();
            for c in openers {
                if let Some(h) = g.players[p].hand.iter().position(|&x| x == c) {
                    g.players[p].hand.remove(h);
                    if let Some(i) = g.enter_card(p, c, p, false) {
                        g.perms[i].sick = false;
                    }
                }
            }
            g.pending.clear();
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
            Src::Exile(k) => (pl.playable.get(k)?.card.1, false),
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

    /// Whose card list the spell cast from `src` is in: the caster's own, or an opponent's
    /// for a stolen card in exile (Tinybones' stash).
    pub fn spell_list(&self, p: usize, src: Src) -> usize {
        match src {
            Src::Exile(k) => self.players[p]
                .playable
                .get(k)
                .map_or(p, |x| x.card.0 as usize),
            _ => p,
        }
    }

    /// The permanent type Muldrotha would use to cast card `c` from the graveyard this
    /// turn (a type bit not used yet), if she lets `p` cast it.
    pub fn graveyard_type_free(&self, p: usize, c: &SimCard) -> Option<u16> {
        if self.active != p
            || self.rules[p % 8] & R_PERMANENTS_FROM_GY == 0
            || c.is(T_LAND)
            || !c.is_permanent()
        {
            return None;
        }
        let used = self.players[p].gy_types_used;
        [T_CREATURE, T_ARTIFACT, T_ENCHANTMENT, T_PLANESWALKER]
            .into_iter()
            .find(|&t| c.is(t) && used & t == 0)
    }

    /// What casting from `src` costs right now (tax, reductions, extras), if it's allowed.
    pub fn cast_cost(&self, p: usize, src: Src, opts: CastOpts) -> Option<Mana> {
        self.cast_plan(p, src, opts).map(|c| c.0)
    }

    /// How an additional cost ("sacrifice a creature or pay {3}{B}") would be paid by `p`
    /// casting card `c`; None if it can't be. `base` is the mana cost without it.
    pub fn extra_payment(
        &self,
        p: usize,
        e: &ExtraCost,
        base: &Mana,
        from_hand: bool,
    ) -> Option<ExtraPay> {
        let others_in_hand = self.players[p]
            .hand
            .len()
            .saturating_sub(usize::from(from_hand));
        Some(match e {
            ExtraCost::Sacrifice(t) => {
                ExtraPay::Sacrifice(self.perms[ai::sac_for_cost(self, p, *t)?].id)
            }
            ExtraCost::Discard => {
                if others_in_hand == 0 {
                    return None;
                }
                ExtraPay::Discard
            }
            ExtraCost::SacrificeOrPay(t, m) => match ai::fodder(self, p, *t, u32::MAX) {
                Some(i) => ExtraPay::Sacrifice(self.perms[i].id),
                None => ExtraPay::Mana(*m),
            },
            ExtraCost::DiscardOrPay(m) => {
                if self.can_pay(p, &add_mana(*base, m)) || others_in_hand == 0 {
                    ExtraPay::Mana(*m)
                } else {
                    ExtraPay::Discard
                }
            }
            ExtraCost::BeholdOrPay(sub, m) => {
                let pl = &self.players[p];
                let in_hand = pl
                    .hand
                    .iter()
                    .filter(|&&h| pl.cards[h as usize].has_subtype(sub))
                    .count();
                let controls = self
                    .perms
                    .iter()
                    .any(|x| x.ctrl as usize == p && x.subtypes.iter().any(|s| s == sub));
                if controls || in_hand > 0 {
                    ExtraPay::Nothing
                } else {
                    ExtraPay::Mana(*m)
                }
            }
        })
    }

    /// The cost of a cast, the X it pays for, and how its additional cost is paid.
    pub fn cast_plan(
        &self,
        p: usize,
        src: Src,
        opts: CastOpts,
    ) -> Option<(Mana, Option<i32>, Option<ExtraPay>)> {
        let (idx, back) = self.spell_card(p, src, opts)?;
        let list = self.spell_list(p, src);
        let c = &self.players[list].cards[idx as usize];
        if c.is(T_LAND) && !back {
            return None;
        }
        // "You can't cast this spell unless there are seven or more cards in your graveyard".
        if let (Some(cond), false) = (&c.cast_cond, back) {
            if !self.cond_holds(cond, p, None) {
                return None;
            }
        }
        // "Your opponents can't cast spells during your turn" (Jennifer Walters).
        if self.active != p && self.rules[self.active % 8] & R_OPP_NO_CAST_MY_TURN != 0 {
            return None;
        }
        // An instant or sorcery needs a legal choice for each target it requires (CR 601.2c).
        let spell_abilities: Option<&[SimAbility]> = match (&c.back, back) {
            (Some(b), true) if b.types & (T_INSTANT | T_SORCERY) != 0 => Some(&b.abilities),
            (_, false) if !c.is_permanent() => Some(&c.abilities),
            _ => None,
        };
        if spell_abilities.is_some_and(|a| !self.targets_exist(p, c, a)) {
            return None;
        }
        // Stashed cards: only during your turn (and not once their owner has left the game).
        if let Src::Exile(k) = src {
            if self.players[p].playable[k].own_turn && self.active != p || !self.players[list].alive
            {
                return None;
            }
        }
        let muldrotha = matches!(src, Src::Graveyard(_))
            && !self.players[p].recast.contains(&idx)
            && c.flashback.is_none()
            && self.graveyard_type_free(p, c).is_some();
        if let Src::Graveyard(_) = src {
            if !self.players[p].recast.contains(&idx) && c.flashback.is_none() && !muldrotha {
                return None;
            }
            // "Flashback—{1}{U/R}{U/R}, Discard a card".
            if c.flashback_discard && self.players[p].hand.is_empty() {
                return None;
            }
        }
        let (mut cost, types, subtypes) = if back {
            let b = c.back.as_ref()?;
            if matches!(src, Src::Hand(_)) && b.kind == BackKind::Prepare
                || b.kind == BackKind::Transform
            {
                return None;
            }
            (b.cost, b.types, &c.subtypes)
        } else if let (Src::Graveyard(_), Some(fb), false, false) = (
            src,
            c.flashback,
            self.players[p].recast.contains(&idx),
            muldrotha,
        ) {
            (fb, c.types, &c.subtypes)
        } else if opts.overload && c.overload.is_some() {
            (c.overload.unwrap(), c.types, &c.subtypes)
        } else if opts.sneak && c.sneak.is_some() && self.players[p].attacked_this_turn {
            (c.sneak.unwrap(), c.types, &c.subtypes)
        } else {
            (c.cost, c.types, &c.subtypes)
        };
        let mut x = None;
        if cost.x {
            // X spells: spend about half the remaining mana on X.
            let n = (self.mana_available(p).saturating_sub(cost.total()) / 2).max(1);
            cost = cost.plus_generic(n);
            x = Some(n as i32);
        }
        if let Src::Command(k) = src {
            let slot = self.players[p].command[k].1 as usize;
            cost = cost.plus_generic(2 * self.players[p].casts[slot.min(2)] as u32);
        }
        // "You may cast spells from your hand without paying their mana costs" (Omniscience;
        // Omnipresence up to the number of creatures you control). X spells are cast paying
        // for the X instead.
        let free_cast = matches!(src, Src::Hand(_))
            && !cost.x
            && (self.rules[p % 8] & R_FREE_FROM_HAND != 0
                || self.rules[p % 8] & R_FREE_BY_CREATURES != 0 && {
                    let mv = if back { cost.total() } else { c.mv as u32 };
                    let creatures = self
                        .perms
                        .iter()
                        .filter(|x| x.ctrl as usize == p && x.is_creature())
                        .count() as u32;
                    mv <= creatures
                });
        if opts.free {
            return Some((Mana::default(), x, None));
        }
        // "Noncreature spells your opponents cast cost {1} more" (Thalia, the Survivor).
        let mut tax = 0u32;
        if self.rule_anywhere(R_SPELL_TAX) {
            for y in self.perms.iter().filter(|y| y.ctrl as usize != p) {
                let Some(r) = y.card else { continue };
                if y.no_abilities || y.blank {
                    continue;
                }
                if let Some((n, f)) = &self.card(r).opp_tax {
                    let type_ok = f.types == 0 || types & f.types != 0;
                    let not_ok = f.not_types == 0 || types & f.not_types == 0;
                    if type_ok && not_ok {
                        tax += *n as u32;
                    }
                }
            }
        }
        // "This spell costs {X} less to cast, where X is the greatest power among creatures
        // you control" (the Ghaltas).
        if let (Some(s), false) = (c.self_less, back) {
            let mine = self
                .perms
                .iter()
                .filter(|x| x.ctrl as usize == p && x.is_creature());
            let n = match s {
                SelfLess::GreatestToughness => mine.map(|x| x.t.max(0) as u32).max().unwrap_or(0),
                SelfLess::GreatestPower => mine.map(|x| x.p.max(0) as u32).max().unwrap_or(0),
                SelfLess::TotalPower => mine.map(|x| x.p.max(0) as u32).sum(),
                SelfLess::TotalPowerFlying => mine
                    .filter(|x| x.has(FLYING))
                    .map(|x| x.p.max(0) as u32)
                    .sum(),
            };
            cost = cost.minus_generic(n);
        }
        // "This spell costs {3} less to cast if ...", "{1} less for each ...", affinity.
        if !back {
            let mut n = 0u32;
            for r in &c.self_reduce {
                n += match r {
                    SelfReduce::If(k, cond) => {
                        if self.cond_holds(cond, p, None) {
                            *k as u32
                        } else {
                            0
                        }
                    }
                    SelfReduce::Each(k, phrase) => {
                        *k as u32 * self.count_of(p, None, phrase).unwrap_or(0).max(0) as u32
                    }
                };
            }
            cost = cost.minus_generic(n);
        }
        // "Spells you cast cost {N} less" (and the like).
        let mut less = 0;
        for x in self.perms.iter().filter(|x| x.ctrl as usize == p) {
            let Some(r) = x.card else { continue };
            let card = self.card(r);
            // "The first creature spell you cast each turn" (Radagast).
            if card.reduction_first && self.players[p].creature_spells_turn > 0 {
                continue;
            }
            if let Some((n, f)) = &card.cost_reduction {
                // "{X} less, where X is ~'s power" (The Scarlet Witch).
                let n = if card.reduction_by_power {
                    x.p.max(0) as u32
                } else {
                    *n as u32
                };
                let type_ok = f.types == 0 || types & f.types != 0;
                let not_ok = f.not_types == 0 || types & f.not_types == 0;
                // "Kithkin spells and Soldier spells", "Instant and sorcery spells".
                let sub_ok = if f.restr.is_empty() {
                    f.subtype
                        .as_ref()
                        .is_none_or(|s| subtypes.iter().any(|t| t == s))
                } else {
                    let d = self.dyn_values(&f.restr, None, p, Some(x.id));
                    Game::card_ok(&f.restr, c, &d)
                };
                if type_ok && not_ok && sub_ok {
                    less += n;
                }
            }
        }
        // Armor Wars II.
        for &(n, t) in &self.players[p].turn_reduction {
            if types & t != 0 {
                less += n as u32;
            }
        }
        if opts.convoke && (c.convoke || self.improvises(p, c)) {
            less += self.convoke_tappers(p, c).len() as u32;
        }
        cost = cost.minus_generic(less).plus_generic(tax);
        if free_cast || self.next_spell_free(p, c, back) {
            cost = Mana::default();
        }
        // Stashed cards: "mana of any type can be spent to cast those spells"; Inside
        // Information: life equal to its mana value instead of its mana cost.
        if let Src::Exile(k) = src {
            let x = &self.players[p].playable[k];
            if x.own_turn {
                cost = Mana::generic(cost.total());
            }
            if x.pay_life {
                if self.players[p].life <= c.mv as i32 + 5 {
                    return None;
                }
                cost = Mana::default();
            }
        }
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
        // "As an additional cost to cast this spell, ...".
        let extra = if back {
            c.back.as_ref().and_then(|b| b.extra_cost.as_ref())
        } else {
            c.extra_cost.as_ref()
        };
        let mut pay = None;
        if let Some(e) = extra {
            let how = self.extra_payment(p, e, &cost, matches!(src, Src::Hand(_)))?;
            if let ExtraPay::Mana(m) = &how {
                cost = add_mana(cost, m);
            }
            pay = Some(how);
        }
        Some((cost, x, pay))
    }

    /// Does every permanent a spell's abilities must target ("destroy target creature",
    /// "deals 2 damage to target attacking or blocking creature") have a legal choice for
    /// player `p`? ("Up to one target" and modal spells may choose none / another mode.)
    pub fn targets_exist(&self, p: usize, c: &SimCard, abilities: &[SimAbility]) -> bool {
        use crate::abilities::{Effect as E, Kind as K, Whose as W};
        for a in abilities
            .iter()
            .filter(|a| a.trig == Trig::Spell && !a.modal)
        {
            if !a.text.contains("target") || a.text.contains("up to") {
                continue;
            }
            // The first target the ability names.
            let first = a.effects.iter().find_map(|e| match e {
                E::Destroy(t) | E::Bounce(t) | E::GainControl(t) | E::Fight(t) => Some((t, true)),
                E::Exile { target, .. } | E::Lock { target, .. } => Some((target, true)),
                E::Damage { to, .. } | E::Shrink { to, .. } => Some((to, true)),
                E::PlusCounters { to, .. } | E::Pump { to, .. } | E::Grant { to, .. } => {
                    Some((to, false))
                }
                E::Untap(t) => Some((t, false)),
                _ => None,
            });
            let Some((t, harmful)) = first else { continue };
            if t.mass
                || t.restr.has(crate::restrict::PREV)
                || !matches!(
                    t.kind,
                    K::Creature
                        | K::CreatureOrPlaneswalker
                        | K::NonlandPermanent
                        | K::Permanent
                        | K::Artifact
                        | K::Enchantment
                        | K::ArtifactOrEnchantment
                        | K::Planeswalker
                        | K::Land
                )
            {
                continue;
            }
            let ctx = resolve::Ctx {
                ctrl: p,
                src: None,
                snap: None,
                card: Some(c),
                card_ref: None,
                subject: None,
                text: &a.text,
                until_eot: a.until_eot,
                overload: false,
                card_restr: a.card_restr,
                x: None,
                paid: None,
                teamwork: false,
                about: None,
            };
            let any = !self.candidates(&ctx, t, harmful).is_empty()
                || t.whose == W::Any && !self.candidates(&ctx, t, !harmful).is_empty();
            if !any {
                return false;
            }
        }
        true
    }

    /// Creatures (convoke) or artifacts (improvise) that could be tapped to help pay.
    fn convoke_tappers(&self, p: usize, c: &SimCard) -> Vec<u32> {
        let improvise = self.improvises(p, c);
        self.perms
            .iter()
            .filter(|x| {
                x.ctrl as usize == p
                    && !x.tapped
                    && ((c.convoke && x.is_creature())
                        || (improvise && x.is(T_ARTIFACT) && !x.is_creature()))
                    && x.tok != Tok::Treasure
            })
            .map(|x| x.id)
            .collect()
    }

    /// World War Hulk: is this the next red or green creature spell, cast free?
    pub fn next_spell_free(&self, p: usize, c: &SimCard, back: bool) -> bool {
        !back
            && self.players[p]
                .free_next
                .is_some_and(|(colors, types)| c.colors & colors != 0 && c.types & types != 0)
    }

    /// Does card `c` have improvise for `p` (its own, or Ironheart's for noncreature
    /// spells)?
    pub fn improvises(&self, p: usize, c: &SimCard) -> bool {
        c.improvise || !c.is(T_CREATURE) && self.rules[p % 8] & R_IMPROVISE != 0
    }

    pub fn can_cast(&self, p: usize, src: Src, opts: CastOpts) -> bool {
        let why = match self.spell_card(p, src, opts) {
            Some((idx, _)) => Purpose::Spell {
                card: (self.spell_list(p, src) as u8, idx),
                from_hand: matches!(src, Src::Hand(_)),
            },
            None => return false,
        };
        self.cast_cost(p, src, opts)
            .is_some_and(|m| self.can_pay_for(p, &m, why))
    }

    /// Casts a spell: pays, moves the card, fires cast triggers, lets opponents respond,
    /// then resolves it. False if it couldn't be cast.
    pub fn cast(&mut self, p: usize, src: Src, opts: CastOpts) -> bool {
        let Some((idx, _)) = self.spell_card(p, src, opts) else {
            return false;
        };
        let list = self.spell_list(p, src);
        let outer = std::mem::replace(
            &mut self.cause,
            audit::Cause::Card((list as u8, idx), p as u8),
        );
        let ok = self.cast_inner(p, src, opts);
        self.cause = outer;
        ok
    }

    fn cast_inner(&mut self, p: usize, src: Src, opts: CastOpts) -> bool {
        let Some((idx, back)) = self.spell_card(p, src, opts) else {
            return false;
        };
        let Some((cost, x, extra)) = self.cast_plan(p, src, opts) else {
            return false;
        };
        let list = self.spell_list(p, src);
        let c: &'a SimCard = &self.players[list].cards[idx as usize];
        // "This spell costs {3} less to cast if it targets a tapped creature": cast for less,
        // it must target one (CR 601.2f).
        let must_target = if back || opts.free {
            0
        } else {
            c.self_reduce
                .iter()
                .find_map(|r| match r {
                    SelfReduce::If(_, cond @ Cond::TappedCreatureToTarget)
                        if self.cond_holds(cond, p, None) =>
                    {
                        Some(1)
                    }
                    SelfReduce::If(_, cond @ Cond::AttackingCreatureToTarget(nontoken))
                        if self.cond_holds(cond, p, None) =>
                    {
                        Some(if *nontoken { 3 } else { 2 })
                    }
                    _ => None,
                })
                .unwrap_or(0)
        };
        // Muldrotha: a permanent type used up for this turn.
        let gy_type = match src {
            Src::Graveyard(_)
                if !self.players[p].recast.contains(&idx) && c.flashback.is_none() =>
            {
                self.graveyard_type_free(p, c)
            }
            _ => None,
        };
        let by_flashback = matches!(src, Src::Graveyard(_))
            && !self.players[p].recast.contains(&idx)
            && gy_type.is_none();
        // X paid (for "enters with X counters" and the spell's own X).
        let x_paid = x;
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
        let treasures = |g: &Game| {
            g.perms
                .iter()
                .filter(|x| x.ctrl as usize == p && x.tok == Tok::Treasure)
                .count()
        };
        let treasures_before = treasures(self);
        // A stolen card cast from exile is in its owner's card list, not the caster's.
        let why = Purpose::Spell {
            card: (self.spell_list(p, src) as u8, idx),
            from_hand: matches!(src, Src::Hand(_)),
        };
        // Convoke / improvise: tap helpers first (the cost already counts them).
        if opts.convoke && (c.convoke || self.improvises(p, c)) && !opts.free {
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
            if !self.pay_for(p, &base, why) {
                return false;
            }
        } else if !self.pay_for(p, &cost, why) {
            return false;
        }
        // "If mana from a Treasure was spent to cast it" (Smaug).
        self.ext.treasure_spent = treasures(self) < treasures_before;
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
        if let Src::Exile(k) = src {
            if self.players[p].playable[k].pay_life {
                self.lose_life(p, c.mv as i32);
            }
        }
        if self.next_spell_free(p, c, back) {
            self.players[p].free_next = None;
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
                if let Some(t) = gy_type {
                    self.players[p].gy_types_used |= t;
                }
                self.emit(Ev::LeavesGraveyard, p, None);
            }
            Src::Exile(k) => {
                let r = self.players[p].playable.remove(k).card;
                let owner = &mut self.players[r.0 as usize];
                if let Some(e) = owner.exile.iter().position(|&c| c == r.1) {
                    owner.exile.remove(e);
                }
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
        // Additional costs: the sacrifice or discard (mana was paid with the rest).
        if !opts.free {
            match extra {
                Some(ExtraPay::Sacrifice(id)) => {
                    if let Some(i) = self.idx(id) {
                        self.sacrifice(i);
                    }
                }
                Some(ExtraPay::Discard) => self.discard(p),
                _ => {}
            }
            if by_flashback && c.flashback_discard {
                self.discard(p);
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
            spell_targets: if back {
                c.back.as_ref().map_or(0, |b| b.spell_targets)
            } else {
                c.spell_targets
            },
            ..c.clone()
        };
        let outer = (self.casting, self.cur_x);
        self.casting = Some(((list as u8, idx), back));
        self.cur_x = x_paid;
        if spell_types & (T_INSTANT | T_SORCERY) != 0 {
            self.players[p].inst_sorc_this_turn =
                self.players[p].inst_sorc_this_turn.saturating_add(1);
        }
        self.on_cast(p, &probe_card);
        if self.auditing() {
            self.audit_cast(p, src, (list, idx), back, &cost, x, opts, commander_slot);
        }
        // Cast triggers go on the stack above the spell and resolve first, every player's
        // (APNAP) - before the spell resolves or the permanent enters (CR 603.3).
        self.run_pending();
        self.sba();
        // Opponents may counter it.
        // (Sphinx of the Final Word, Samut: instants and sorceries can't be countered.)
        let uncounterable = c.cant_be_countered
            || spell_types & (T_INSTANT | T_SORCERY) != 0
                && self.rules[p % 8] & R_SPELLS_UNCOUNTERABLE != 0;
        if !uncounterable && ai::respond_to_spell(self, p, (list as u8, idx), back) {
            let to = if commander_slot > 0 {
                "command zone"
            } else {
                "graveyard"
            };
            self.note(|| format!("  {} is countered (-> {})", c.name, to));
            match src {
                Src::Prepared(_) => {}
                _ if commander_slot > 0 => self.players[p].command.push((idx, commander_slot)),
                _ => self.players[list].graveyard.push(idx),
            }
            self.run_pending();
            (self.casting, self.cur_x) = outer;
            return true;
        }
        let permanent = !back && c.is_permanent();
        if permanent {
            self.entering_x = x_paid;
            self.entering_kicked = kicked;
            self.ext.entering_cast = true;
            let entered = self.enter_card(list, idx, p, commander_slot > 0);
            self.ext.entering_cast = false;
            self.ext.last_cast_perm = entered.map(|i| self.perms[i].id);
            let Some(i) = entered else {
                self.run_pending();
                self.sba();
                (self.casting, self.cur_x) = outer;
                return true;
            };
            let id = self.perms[i].id;
            self.perms[i].commander = commander_slot;
            // ("Enters with X +1/+1 counters", "a divinity counter": see `enter_card`.)
            for pend in self.pending.iter_mut().filter(|x| x.source == Some(id)) {
                pend.teamwork = teamwork;
                pend.gift = gift;
                pend.kicked = kicked;
                pend.x = x;
            }
            // Auras attach as they enter.
            if c.is(T_AURA) {
                if let Some(t) = ai::aura_target(self, p, (list as u8, idx)) {
                    let tid = self.perms[t].id;
                    // An opponent's creature with ward: unpaid, the Aura spell is countered
                    // (it has nothing to enchant and goes to the graveyard).
                    let warded = self.perms[t].ctrl as usize != p && !self.pay_ward(p, t);
                    if warded {
                        self.note(|| "  (the target's ward isn't paid: it's countered)".into());
                    } else if let Some(j) = self.idx(id) {
                        self.perms[j].attached = Some(tid);
                    }
                }
            }
            if opts.offspring && c.offspring.is_some() {
                let t = self.new_perm(p, p, Some((list as u8, idx)), Tok::Other);
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
                    self.copy_spell(p, ((list as u8, idx), back));
                }
                if goggles.is_some() {
                    self.copy_spell(p, ((list as u8, idx), back));
                }
            }
            for (k, a) in abilities.iter().enumerate() {
                if a.trig == Trig::Spell
                    || (back && a.trig != Trig::Static && !matches!(a.trig, Trig::Activated { .. }))
                {
                    self.pending.push(Pending {
                        ctrl: p as u8,
                        source: None,
                        card: (list as u8, idx),
                        back,
                        ability: k as u16,
                        snapshot: None,
                        subject: None,
                        teamwork,
                        gift,
                        from_graveyard,
                        kicked,
                        overloaded,
                        x,
                    });
                }
            }
            self.ext.must_target = must_target;
            self.run_pending();
            self.ext.must_target = 0;
            if c.is(T_INSTANT | T_SORCERY) || back {
                self.players[p].last_spell = Some(((list as u8, idx), back));
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
                _ => self.players[list].graveyard.push(idx),
            }
        }
        self.run_pending();
        self.sba();
        (self.casting, self.cur_x) = outer;
        true
    }

    /// The fodder an activated "sacrifice a ..." cost would use.
    fn sac_fodder(&self, p: usize, src_id: u32, text: &str) -> Option<usize> {
        // "Sacrifice an Equipment attached to ~" (Ronin): only that Equipment will do.
        if text.contains("sacrifice an equipment attached to") {
            return self.perms.iter().position(|x| {
                x.ctrl as usize == p && x.is(T_EQUIPMENT) && x.attached == Some(src_id)
            });
        }
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
        self.activation_plan(p, i, k).map(|c| c.0)
    }

    /// Permanents `p` would tap for a "tap N untapped artifacts / Elves you control" cost.
    fn tap_helpers(&self, p: usize, src_id: u32, e: &ActCost) -> Vec<u32> {
        let mut c: Vec<&Perm> = self
            .perms
            .iter()
            .filter(|x| {
                x.ctrl as usize == p
                    && x.id != src_id
                    && !x.tapped
                    && (x.is(e.tap_types) || e.tap_types & T_CREATURE != 0 && x.is_creature())
                    && e.tap_subtype
                        .as_ref()
                        .is_none_or(|s| x.subtypes.iter().any(|t| t == s))
            })
            .collect();
        // Spare the mana makers and the biggest attackers.
        c.sort_by_key(|x| (x.card.is_some_and(|r| self.card(r).mana_ability), x.p + x.t));
        c.iter().take(e.tap_others as usize).map(|x| x.id).collect()
    }

    /// Graveyard cards a cost would exile ("exile another creature card from your graveyard",
    /// "black cards with fifteen or more black mana symbols"), not counting graveyard slot
    /// `skip` (the card using the ability). None if there aren't enough.
    fn gy_cost(&self, p: usize, e: &ActCost, skip: Option<usize>) -> Option<Vec<usize>> {
        let pl = &self.players[p];
        let ok = |k: &usize| Some(*k) != skip;
        if e.black_pips > 0 {
            let mut out = Vec::new();
            let mut pips = 0u32;
            for k in (0..pl.graveyard.len()).filter(ok) {
                let b = pl.cards[pl.graveyard[k] as usize].cost.pips[2] as u32;
                if b > 0 {
                    out.push(k);
                    pips += b;
                }
                if pips >= e.black_pips as u32 {
                    return Some(out);
                }
            }
            return None;
        }
        if e.exile_cards == 0 {
            return Some(Vec::new());
        }
        let mut cands: Vec<usize> = (0..pl.graveyard.len())
            .filter(ok)
            .filter(|&k| !e.exile_creatures || pl.cards[pl.graveyard[k] as usize].is(T_CREATURE))
            .collect();
        // The least useful cards go.
        cands.sort_by(|&a, &b| {
            let v = |k: usize| pl.cards[pl.graveyard[k] as usize].value;
            v(a).partial_cmp(&v(b)).unwrap()
        });
        (cands.len() >= e.exile_cards as usize)
            .then(|| cands.into_iter().take(e.exile_cards as usize).collect())
    }

    /// Exiles graveyard cards at these slots.
    fn exile_from_graveyard(&mut self, p: usize, mut slots: Vec<usize>) {
        if slots.is_empty() {
            return;
        }
        slots.sort_unstable();
        self.ext.exiled_for_cost.clear();
        for k in slots.into_iter().rev() {
            if k < self.players[p].graveyard.len() {
                let c = self.players[p].graveyard.remove(k);
                self.players[p].exile.push(c);
                self.ext.exiled_for_cost.push(c);
            }
        }
        self.emit(Ev::LeavesGraveyard, p, None);
    }

    /// The X to pay for an ability with {X} in its cost, given the rest of its mana.
    fn choose_x(&self, p: usize, i: usize, ab: &SimAbility, fixed: u32, xs: u8) -> Option<i32> {
        let spare = self.mana_available(p).saturating_sub(fixed);
        let most = (spare / xs.max(1) as u32).min(12) as i32;
        if ab.text.contains("mana value x") {
            // Steel Hellkite: the mana value that destroys the most of the players it hit.
            let hit = self.perms[i].hit_turn;
            let mut best: Option<(f32, i32)> = None;
            for v in 0..=most {
                let gain: f32 = (0..self.perms.len())
                    .filter(|&i| {
                        let y = &self.perms[i];
                        !y.is(T_LAND)
                            && ai::perm_mv(self, y) as i32 == v
                            && hit & (1 << y.ctrl.min(7)) != 0
                    })
                    .map(|i| ai::perm_value(self, i))
                    .sum();
                if gain > 0.0 && best.is_none_or(|b| gain > b.0) {
                    best = Some((gain, v));
                }
            }
            return best.map(|b| b.1);
        }
        (most >= 1).then_some(most)
    }

    /// The mana an activated ability needs now and the X it pays (None if it can't be
    /// activated: a cost can't be paid, it's used up, or its "activate only if" fails).
    pub fn activation_plan(&self, p: usize, i: usize, k: usize) -> Option<(Mana, Option<i32>)> {
        let x = &self.perms[i];
        if x.ctrl as usize != p {
            return None;
        }
        let ab = self.abilities_of(x).get(k)?;
        if ab.from_graveyard {
            return None; // used from the graveyard only
        }
        // Sorcerous Spyglass naming this card.
        if self.rule_anywhere(R_NAMES_CARD) && self.named_shut(x) {
            return None;
        }
        match &ab.trig {
            Trig::Activated {
                mana,
                cost,
                tap,
                sac_other,
                discard,
                life,
                extra,
                ..
            } => {
                let adds_mana = ab
                    .effects
                    .iter()
                    .any(|e| matches!(e, crate::abilities::Effect::AddMana(_)));
                if adds_mana && *tap && *mana == 0 && extra.tap_others == 0 {
                    return None; // mana abilities are used as mana sources
                }
                // Filters ("{1}, {T}: Add one mana of any color", Baxter Building's "{4},
                // {T}: Add four mana"): nothing gained by activating them on their own.
                if adds_mana
                    && *mana > 0
                    && ab
                        .effects
                        .iter()
                        .all(|e| matches!(e, crate::abilities::Effect::AddMana(_)))
                {
                    return None;
                }
                // Abilities that work from the graveyard or the hand, not the battlefield.
                if extra.from_graveyard || extra.from_hand || ab.graveyard_only() {
                    return None;
                }
                let sick = x.is_creature()
                    && x.sick
                    && !x.has(HASTE)
                    && self.rules[p % 8] & R_ABILITIES_HASTE == 0;
                if *tap && (x.tapped || sick) {
                    return None;
                }
                if *sac_other && self.sac_fodder(p, x.id, &ab.text).is_none() {
                    return None;
                }
                if *discard && self.discard_for_cost(p, &ab.text).is_none() {
                    return None;
                }
                if *life as i32 >= self.players[p].life {
                    return None;
                }
                if ab.once && x.powered {
                    return None;
                }
                // "Harness ~": once harnessed, it stays so.
                if x.powered && ab.text.contains(": harness ~") {
                    return None;
                }
                if ab.once_per_turn && k < 32 && x.fired & (1 << k) != 0 {
                    return None;
                }
                if x.activations >= 3 {
                    return None; // keep loops in check
                }
                if extra.remove > 0 {
                    let have = if extra.remove_plus {
                        x.counters
                    } else {
                        x.other_counters
                    };
                    if have < extra.remove as i16 {
                        return None;
                    }
                }
                if extra.tap_others > 0
                    && self.tap_helpers(p, x.id, extra).len() < extra.tap_others as usize
                {
                    return None;
                }
                self.gy_cost(p, extra, None)?;
                // "Activate only if ..." (Boast: only if it attacked this turn).
                if ab.text.contains("activate only if") && !self.cond_holds(&ab.cond, p, Some(i)) {
                    return None;
                }
                // "Remove any number of +1/+1 counters": all of them, X = how many.
                let mut x_paid = extra.remove_all.then_some(if extra.remove_plus {
                    x.counters as i32
                } else {
                    x.other_counters as i32
                });
                // Coloured symbols must be paid with mana of their colours (CR 602.2b).
                let mut cost = *cost;
                if extra.x > 0 {
                    let v = self.choose_x(p, i, ab, *mana as u32, extra.x)?;
                    cost = cost.plus_generic(v as u32 * extra.x as u32);
                    x_paid = Some(v);
                }
                Some((cost, x_paid))
            }
            Trig::PowerUp(full) => {
                // (Kang's extra turn: "power-up abilities can't be activated".)
                if x.powered || self.players[p].no_powerups {
                    return None;
                }
                // The turn it entered, the cost is reduced by the permanent's mana cost:
                // its coloured pips come off matching pips, the rest off generic.
                let mut cost = *full;
                // "If it entered this turn" - this turn, not this round.
                if x.entered_turn {
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
                // "Power-up abilities of other creatures you control cost {3} less" (Hulk).
                let less: u32 = self
                    .perms
                    .iter()
                    .filter(|y| y.ctrl as usize == p && y.id != x.id)
                    .filter_map(|y| y.card)
                    .map(|r| self.card(r).power_up_less as u32)
                    .sum();
                let mut cost = cost.minus_generic(less);
                // "Power-up — {X}{U}{U}: put X +1/+1 counters": X is chosen and paid (CR
                // 107.3), and at least 1 (nothing is gained with X = 0).
                if cost.x {
                    cost.x = false;
                    let v = self.choose_x(p, i, ab, cost.total(), 1)?;
                    return Some((cost.plus_generic(v as u32), Some(v)));
                }
                Some((cost, None))
            }
            Trig::Loyalty(n) => {
                if x.loyalty_used || x.loyalty + (*n as i16) < 0 {
                    return None;
                }
                // "[0]: ... Activate only if there are twenty-five or more loyalty counters
                // among Jaces you control."
                if ab.text.contains("activate only if") && !self.cond_holds(&ab.cond, p, Some(i)) {
                    return None;
                }
                Some((Mana::default(), None))
            }
            _ => None,
        }
    }

    /// Are permanent `x`'s activated abilities shut off by a Sorcerous Spyglass naming it?
    pub fn named_shut(&self, x: &Perm) -> bool {
        let Some(name) = x.card.map(|r| &self.card(r).name) else {
            return false;
        };
        self.perms.iter().any(|y| {
            y.named.is_some_and(|r| self.card(r).name == *name)
                && self.perm_rules(y) & R_NAMES_CARD != 0
        })
    }

    /// Activates ability `k` of permanent `id` (paying all its costs). False if it couldn't.
    pub fn activate(&mut self, p: usize, id: u32, k: usize) -> bool {
        let Some(i) = self.idx(id) else { return false };
        let cause = match (self.perms[i].card, self.perms[i].tok) {
            (Some(r), Tok::None | Tok::Creature | Tok::Other) => audit::Cause::Card(r, p as u8),
            _ => audit::Cause::Token(p as u8),
        };
        let outer = std::mem::replace(&mut self.cause, cause);
        let ok = self.activate_inner(p, id, k);
        self.cause = outer;
        ok
    }

    fn activate_inner(&mut self, p: usize, id: u32, k: usize) -> bool {
        let Some(i) = self.idx(id) else { return false };
        let Some((cost, x_paid)) = self.activation_plan(p, i, k) else {
            return false;
        };
        // A {T} cost taps the source itself: it can't also tap for mana to pay the rest
        // ("{4}, {T}: Draw a card" on Baxter Building needs four other mana).
        let taps = matches!(
            self.abilities_of(&self.perms[i]).get(k).map(|a| &a.trig),
            Some(Trig::Activated { tap: true, .. })
        );
        let was_tapped = self.perms[i].tapped;
        if taps {
            self.perms[i].tapped = true;
        }
        if !self.pay_for(p, &cost, Purpose::Ability { src: id }) {
            if let Some(i) = self.idx(id) {
                self.perms[i].tapped = was_tapped;
            }
            return false;
        }
        if taps {
            // (Tapped below, as the cost is paid.)
            if let Some(i) = self.idx(id) {
                self.perms[i].tapped = was_tapped;
            }
        }
        let Some(i) = self.idx(id) else { return false };
        let x = self.perms[i].clone();
        if self.log.is_some() {
            let d = self.perm_desc(&x);
            let text = self.abilities_of(&x)[k].text.clone();
            let left = self.mana_available(p);
            let paid = if cost.total() > 0 {
                format!(" (paid {}, {} mana left)", audit::fmt_mana(&cost), left)
            } else {
                String::new()
            };
            self.note(|| format!("P{} activates {}: {}{}", p, d, text, paid));
        }
        let ab = &self.abilities_of(&x)[k];
        let mut snapshot = None;
        match &ab.trig {
            Trig::Activated {
                tap,
                sac_self,
                sac_other,
                discard,
                life,
                extra,
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
                // (Harnessing an Infinity Stone turns on its "∞" ability: `Cond::Harnessed`.)
                if ab.once || ab.text.contains(": harness ~") {
                    self.perms[i].powered = true;
                }
                if ab.once_per_turn && k < 32 {
                    self.perms[i].fired |= 1 << k;
                }
                if extra.remove > 0 {
                    let y = &mut self.perms[i];
                    let n = match x_paid {
                        Some(n) if extra.remove_all => n as i16,
                        _ => extra.remove as i16,
                    };
                    if extra.remove_plus {
                        y.counters -= n;
                    } else {
                        y.other_counters -= n;
                    }
                }
                match extra.add_counter {
                    1 => self.perms[i].stun += 1,
                    2 => self.add_other_counters(i, 1),
                    _ => {}
                }
                for t in self.tap_helpers(p, id, extra) {
                    if let Some(j) = self.idx(t) {
                        self.perms[j].tapped = true;
                    }
                }
                if let Some(slots) = self.gy_cost(p, extra, None) {
                    if !slots.is_empty() {
                        self.exile_from_graveyard(p, slots);
                    }
                }
                if *life > 0 {
                    self.lose_life(p, *life as i32);
                }
                if *discard {
                    match self.discard_for_cost(p, &ab.text) {
                        Some(Some(h)) => {
                            let c = self.players[p].hand.remove(h);
                            self.put_discarded(p, c);
                        }
                        _ => self.discard(p),
                    }
                }
                if *sac_other {
                    if let Some(f) = self.sac_fodder(p, id, &ab.text) {
                        self.sacrifice(f);
                    }
                }
                if *sac_self || extra.exile_self || extra.return_self {
                    snapshot = Some(x.clone());
                    if let Some(i) = self.idx(id) {
                        if *sac_self {
                            self.sacrifice(i);
                        } else if extra.exile_self {
                            self.leave(i, Zone::Exile, false);
                        } else {
                            self.leave(i, Zone::Hand, false);
                        }
                    }
                }
            }
            Trig::PowerUp(_) => self.perms[i].powered = true,
            Trig::Loyalty(n) => {
                self.perms[i].loyalty += *n as i16;
                self.perms[i].loyalty_used = true;
                self.players[p].loyalty_turn = true;
                self.ext.last_loyalty_cost = (-*n).max(0) as u8;
                self.emit(Ev::LoyaltyActivated, p, Some(id));
            }
            _ => return false,
        }
        let card = x.card.unwrap_or((p as u8, NO_CARD));
        let snapshot =
            snapshot.or_else(|| (x.card.is_none() || x.tok != Tok::None).then(|| x.clone()));
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
            x: x_paid,
        });
        self.run_pending();
        self.sba();
        true
    }

    /// What activating ability `k` of the card in graveyard slot `gk` costs ("{4}{B}{B},
    /// Exile this card from your graveyard: ..."); None if it can't be activated.
    pub fn graveyard_activation_cost(&self, p: usize, gk: usize, k: usize) -> Option<Mana> {
        let pl = &self.players[p];
        let c = &pl.cards[*pl.graveyard.get(gk)? as usize];
        let ab = c.abilities.get(k)?;
        let Trig::Activated {
            cost,
            tap: false,
            life,
            discard,
            sac_other,
            extra,
            ..
        } = &ab.trig
        else {
            return None;
        };
        // "Exile ~ from your graveyard: ...", "{1}{B}: Return ~ from your graveyard to the
        // battlefield tapped".
        if !(extra.from_graveyard || ab.graveyard_only()) {
            return None;
        }
        if ab.text.contains("only as a sorcery") && self.active != p {
            return None;
        }
        if *life as i32 >= pl.life || *discard && pl.hand.is_empty() {
            return None;
        }
        if *sac_other && self.sac_fodder(p, u32::MAX, &ab.text).is_none() {
            return None;
        }
        if ab.text.contains("activate only if") && !self.cond_holds(&ab.cond, p, None) {
            return None;
        }
        self.gy_cost(p, extra, Some(gk))?;
        Some(*cost)
    }

    /// Activates an ability of a card in the graveyard (paying its costs).
    pub fn activate_from_graveyard(&mut self, p: usize, gk: usize, k: usize) -> bool {
        let Some(&idx) = self.players[p].graveyard.get(gk) else {
            return false;
        };
        let outer = std::mem::replace(&mut self.cause, audit::Cause::Card((p as u8, idx), p as u8));
        let ok = self.activate_from_graveyard_inner(p, gk, k);
        self.cause = outer;
        ok
    }

    fn activate_from_graveyard_inner(&mut self, p: usize, gk: usize, k: usize) -> bool {
        let Some(cost) = self.graveyard_activation_cost(p, gk, k) else {
            return false;
        };
        if !self.pay(p, &cost) {
            return false;
        }
        let idx = self.players[p].graveyard[gk];
        let c: &'a SimCard = &self.players[p].cards[idx as usize];
        let ab = &c.abilities[k];
        let Trig::Activated {
            life,
            discard,
            sac_other,
            extra,
            ..
        } = &ab.trig
        else {
            return false;
        };
        if *life > 0 {
            self.lose_life(p, *life as i32);
        }
        if *sac_other {
            if let Some(f) = self.sac_fodder(p, u32::MAX, &ab.text) {
                self.sacrifice(f);
            }
        }
        if *discard {
            self.discard(p);
        }
        // The card may have moved in the graveyard (a sacrifice or discard adds cards on top).
        let Some(gk) = self.players[p].graveyard.iter().rposition(|&c| c == idx) else {
            return false;
        };
        let mut slots = self.gy_cost(p, extra, Some(gk)).unwrap_or_default();
        if extra.from_graveyard {
            slots.push(gk);
        }
        self.exile_from_graveyard(p, slots);
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
            x: None,
        });
        self.run_pending();
        self.sba();
        true
    }

    /// The mana for ability `k` of the card in hand slot `h` when it works from the hand
    /// ("{3}{G}, Discard this card: ...", "Exile ~ from your hand: ..."); None if it doesn't
    /// or can't be activated now. (Hand tutors are typecycling: see `Game::cycle`.)
    pub fn hand_activation_cost(&self, p: usize, h: usize, k: usize) -> Option<Mana> {
        let pl = &self.players[p];
        let c = &pl.cards[*pl.hand.get(h)? as usize];
        let ab = c.abilities.get(k)?;
        let Trig::Activated {
            cost,
            tap: false,
            life,
            sac_other,
            extra,
            ..
        } = &ab.trig
        else {
            return None;
        };
        if !extra.from_hand || c.cycling.is_some() {
            return None;
        }
        if ab.text.contains("only as a sorcery") && self.active != p {
            return None;
        }
        if *life as i32 >= pl.life || *sac_other && self.sac_fodder(p, u32::MAX, &ab.text).is_none()
        {
            return None;
        }
        if ab.text.contains("activate only if") && !self.cond_holds(&ab.cond, p, None) {
            return None;
        }
        Some(*cost)
    }

    /// Activates an ability of a card in hand that works from there (discarding or exiling
    /// the card as its cost).
    pub fn activate_from_hand(&mut self, p: usize, h: usize, k: usize) -> bool {
        let Some(cost) = self.hand_activation_cost(p, h, k) else {
            return false;
        };
        if !self.pay(p, &cost) {
            return false;
        }
        let idx = self.players[p].hand.remove(h);
        let c: &'a SimCard = &self.players[p].cards[idx as usize];
        let ab = &c.abilities[k];
        let outer = std::mem::replace(&mut self.cause, audit::Cause::Card((p as u8, idx), p as u8));
        self.note(|| format!("P{} activates {} from the hand", p, c.name));
        if let Trig::Activated {
            life, sac_other, ..
        } = &ab.trig
        {
            if *life > 0 {
                self.lose_life(p, *life as i32);
            }
            if *sac_other {
                if let Some(f) = self.sac_fodder(p, u32::MAX, &ab.text) {
                    self.sacrifice(f);
                }
            }
        }
        if ab.text.contains("exile ~ from your hand") {
            self.players[p].exile.push(idx);
        } else {
            self.put_discarded(p, idx);
        }
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
            from_graveyard: false,
            kicked: false,
            overloaded: false,
            x: None,
        });
        self.run_pending();
        self.sba();
        self.cause = outer;
        true
    }

    /// The card a "discard a card" cost would discard: Some(None) = any (the worst), Some(Some
    /// (slot)) = that one ("discard a legendary card with the same name as a legendary
    /// permanent you control"), None = it can't be paid.
    pub fn discard_for_cost(&self, p: usize, text: &str) -> Option<Option<usize>> {
        let pl = &self.players[p];
        if pl.hand.is_empty() {
            return None;
        }
        let cost = text.split(": ").next().unwrap_or("");
        if !cost.contains("discard a legendary card") {
            return Some(None);
        }
        let same_name = cost.contains("with the same name as a legendary permanent you control");
        (0..pl.hand.len())
            .filter(|&h| {
                let c = &pl.cards[pl.hand[h] as usize];
                c.is(T_LEGENDARY)
                    && (!same_name
                        || self.perms.iter().any(|x| {
                            x.ctrl as usize == p
                                && x.is(T_LEGENDARY)
                                && x.card.is_some_and(|r| self.card(r).name == c.name)
                        }))
            })
            .min_by(|&a, &b| {
                let va = pl.cards[pl.hand[a] as usize].value;
                let vb = pl.cards[pl.hand[b] as usize].value;
                va.partial_cmp(&vb).unwrap()
            })
            .map(Some)
    }

    /// What equipping `equip_id` to `target_id` costs `p` (mana, life): "Equip Wizard {1}"
    /// for a Wizard, "Equip worthy {1}" only for a legendary non-Villain red or white
    /// creature, the plain equip cost otherwise (None: it can't go there); Equipment tokens
    /// (an Axe) equip for {2}. Less Dwarven Mauler's {2}, Warrior's Blades' {1} per +1/+1
    /// counter, or {0} for Kili's first equip each turn.
    pub fn equip_cost(&self, p: usize, equip_id: u32, target_id: u32) -> Option<(Mana, i32)> {
        let e = self.idx(equip_id)?;
        let t = self.idx(target_id);
        let Some(r) = self.perms[e].card else {
            return matches!(self.perms[e].tok, Tok::Equip(..)).then(|| (Mana::generic(2), 0));
        };
        let c = self.card(r);
        let plain = (c.equip.unwrap_or(Mana::generic(2)), c.equip_life as i32);
        let (mut cost, life) = match (&c.equip_for, t) {
            (Some((m, what)), Some(t)) => {
                let x = &self.perms[t];
                let fits = match what.as_str() {
                    "worthy" => {
                        x.is(T_LEGENDARY)
                            && !x.subtypes.iter().any(|s| s == "villain")
                            && x.colors & 0b1001 != 0
                    }
                    w => x.subtypes.iter().any(|s| s == w),
                };
                if fits {
                    (*m, 0)
                } else {
                    (c.equip?, c.equip_life as i32)
                }
            }
            _ => plain,
        };
        if let Some(t) = t {
            let x = &self.perms[t];
            if let Some(r) = x.card.filter(|_| !x.blank && !x.no_abilities) {
                cost = cost.minus_generic(self.card(r).equip_target_less as u32);
            }
            if c.equip_less_per_counter {
                cost = cost.minus_generic(x.counters.max(0) as u32);
            }
        }
        if self.rules[p % 8] & R_FREE_FIRST_EQUIP != 0
            && self.players[p].equips_turn == 0
            && self.players[p].enduring_story
        {
            cost = Mana::default();
        }
        Some((cost, life))
    }

    pub fn equip(&mut self, p: usize, equip_id: u32, target_id: u32) -> bool {
        let Some(e) = self.idx(equip_id) else {
            return false;
        };
        if self.idx(target_id).is_none() {
            return false;
        }
        let Some((cost, life)) = self.equip_cost(p, equip_id, target_id) else {
            return false;
        };
        if self.perms[e].attached == Some(target_id)
            || life > 0 && self.players[p].life <= life
            || !self.pay_for(p, &cost, Purpose::Ability { src: equip_id })
        {
            return false;
        }
        if life > 0 {
            self.lose_life(p, life);
        }
        self.players[p].equips_turn = self.players[p].equips_turn.saturating_add(1);
        if let Some(e) = self.idx(equip_id) {
            self.perms[e].attached = Some(target_id);
        }
        self.recalc();
        if self.log.is_some() {
            let (e, t) = (self.id_desc(equip_id), self.id_desc(target_id));
            let m = audit::fmt_mana(&cost);
            self.note(|| format!("P{} equips {} to {} (paid {})", p, e, t, m));
        }
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
        // (Crewing doesn't make it able to attack: a Vehicle that arrived this turn is still
        // summoning sick - CR 302.6.)
        if let Some(v) = self.idx(vehicle_id) {
            self.perms[v].crewed = true;
        }
        self.recalc();
        if self.log.is_some() {
            let v = self.id_desc(vehicle_id);
            self.note(|| format!("P{} crews {}", p, v));
        }
        true
    }

    pub fn cycle(&mut self, p: usize, h: usize) -> bool {
        let c = &self.players[p].cards[self.players[p].hand[h] as usize];
        let Some((cost, find)) = c.cycling.clone() else {
            return false;
        };
        let mana = c.cycling_mana.unwrap_or(Mana::generic(cost as u32));
        if !self.pay(p, &mana) {
            return false;
        }
        let card = self.players[p].hand.remove(h);
        if self.log.is_some() {
            let name = self.players[p].cards[card as usize].name.clone();
            let paid = audit::fmt_mana(&mana);
            self.note(|| format!("P{} cycles {} (paid {})", p, name, paid));
        }
        self.put_discarded(p, card);
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

    /// Plays a land from the graveyard (`(true, slot)`: Mole Man, Muldrotha), from exile
    /// (`(false, playable slot)`) or from the top of the library (`(false,
    /// FROM_LIBRARY_TOP)`: Ka-Zar).
    pub fn play_land_from(&mut self, p: usize, (gy, k): (bool, usize)) {
        let c = if !gy && k == FROM_LIBRARY_TOP {
            let Some(c) = self.players[p].library.pop() else {
                return;
            };
            c
        } else if gy {
            let c = self.players[p].graveyard.remove(k);
            self.players[p].gy_types_used |= T_LAND;
            self.emit(Ev::LeavesGraveyard, p, None);
            c
        } else {
            let r = self.players[p].playable.remove(k).card;
            if let Some(e) = self.players[p].exile.iter().position(|&c| c == r.1) {
                self.players[p].exile.remove(e);
            }
            r.1
        };
        self.players[p].lands_played += 1;
        self.quiet_enter = true;
        let i = self.enter_card(p, c, p, false);
        self.quiet_enter = false;
        if self.auditing() {
            let from = if gy {
                " from the graveyard"
            } else if k == FROM_LIBRARY_TOP {
                " from the top of the library"
            } else {
                " from exile"
            };
            self.audit_land(p, c, i, from);
        }
        self.run_pending();
        self.sba();
    }

    pub fn play_land(&mut self, p: usize, h: usize) {
        let c = self.players[p].hand.remove(h);
        self.players[p].lands_played += 1;
        self.quiet_enter = true;
        let i = self.enter_card(p, c, p, false);
        self.quiet_enter = false;
        if self.auditing() {
            self.audit_land(p, c, i, "");
        }
        self.run_pending();
        self.sba();
    }

    /// Triggers of "at the beginning of" a phase for the active player `a`.
    fn phase_triggers(&mut self, a: usize, phase: Phase) {
        self.run_delayed(phase);
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
                            x: None,
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
                                from_graveyard: false,
                                kicked: false,
                                overloaded: false,
                                x: None,
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
        self.begin_turn_extra();
        self.died_this_turn = 0;
        self.dead.clear();
        for pl in self.players.iter_mut() {
            pl.lands_played = 0;
            pl.spells_this_turn = 0;
            pl.inst_sorc_this_turn = 0;
            pl.copy_next.clear();
            pl.drawn_this_turn = 0;
            pl.attacked_this_turn = false;
            pl.gained_life_turn = false;
            pl.lost_life_last_turn = pl.lost_life_turn;
            pl.lost_life_turn = false;
            pl.life_gained_turn = 0;
            pl.discarded_turn = 0;
            pl.extra_lands = 0;
            pl.scried_turn = false;
            pl.loyalty_turn = false;
            pl.creature_spells_turn = 0;
            pl.noncreature_spells_turn = 0;
            pl.gy_types_used = 0;
            pl.equips_turn = 0;
            pl.turn_reduction.clear();
            pl.free_next = None;
            pl.toughness_damage_turn = false;
            for g in pl.grudge.iter_mut() {
                *g *= 0.8;
            }
        }
        // "Until your next turn" attack modifiers end.
        self.players[a].attacked_power = 0;
        self.players[a].attack_bonus = None;
        // Goad lasts until the goading player's next turn; once-a-turn triggers reset.
        for x in self.perms.iter_mut() {
            if x.goad == Some(a as u8) {
                x.goad = None;
            }
            if x.till.is_some_and(|t| t.2 == a as u8) {
                x.till = None;
            }
            x.fired = 0;
            x.hit_turn = 0;
            x.hurt_by = 0;
            x.attacked_turn = false;
            x.entered_turn = false;
        }
        // Untap.
        // "~ doesn't untap during your untap step" (Bombur: unless an enduring story).
        let storied = self.players[a].enduring_story;
        let stays: Vec<u32> = self
            .perms
            .iter()
            .filter(|x| x.ctrl as usize == a)
            .filter(|x| {
                !x.no_abilities
                    && !x.blank
                    && x.card.is_some_and(|r| {
                        let c = self.card(r);
                        c.no_untap
                            && !(storied
                                && c.abilities.iter().any(|ab| {
                                    ab.text.contains("unless you have an enduring story")
                                }))
                    })
                    // "Enchanted creature ... doesn't untap during its controller's untap
                    // step" (Enchanted River's Grasp).
                    || self.perms.iter().any(|y| {
                        y.attached == Some(x.id)
                            && !y.no_abilities
                            && y.card.is_some_and(|r| self.card(r).attached_no_untap)
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
        if self.auditing() {
            self.audit_turn_start(a);
        }
        let hijacked = std::mem::take(&mut self.players[a].hijacked);
        self.phase_triggers(a, Phase::Upkeep);
        // (A player who lost in their upkeep takes no further actions - CR 800.4a.)
        if !first_turn && self.players[a].alive {
            self.draw(a, 1);
        }
        self.run_pending();
        self.sba();
        if self.players[a].alive {
            self.main_turn(a, hijacked);
        }
        // A player eliminated during their own turn: the turn goes on without them (end step
        // triggers of the others, damage and "until end of turn" effects wear off - CR 800.4a).
        if self.alive_count() <= 1 {
            self.end_turn = false;
            return;
        }
        // "End the turn" (Time Stop): straight to the cleanup step.
        if std::mem::take(&mut self.end_turn) {
            self.cleanup(a);
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
        self.run_pending();
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
        self.cleanup(a);
    }

    /// The active player's turn after the draw step: Sagas, main phases and combats.
    fn main_turn(&mut self, a: usize, hijacked: bool) {
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
            // Extra combats added during a combat ("after this phase, there is an additional
            // combat phase") count too (CR 505.1a); at most five in a turn (loops).
            self.players[a].extra_combats = self.players[a].extra_combats.saturating_add(1);
            let mut combats = 0;
            while self.players[a].extra_combats > 0 && combats < 5 {
                self.players[a].extra_combats -= 1;
                combats += 1;
                if self.alive_count() <= 1 || !self.players[a].alive || self.end_turn {
                    break;
                }
                self.phase_triggers(a, Phase::Combat);
                self.combat(a);
            }
            self.players[a].extra_combats = 0;
            if self.players[a].alive && self.alive_count() > 1 && !self.end_turn {
                ai::main_phase(self, a, true);
            }
        }
    }

    /// Player `a`'s turn and any extra turns after it (Kang: power-ups can't be activated
    /// during them).
    pub fn take_turns(&mut self, a: usize, first_turn: bool) {
        self.take_turn(a, first_turn);
        self.sba();
        while self.players[a].alive && self.players[a].extra_turns > 0 && self.alive_count() > 1 {
            self.players[a].extra_turns -= 1;
            self.players[a].no_powerups = true;
            self.take_turn(a, false);
            self.players[a].no_powerups = false;
            self.sba();
        }
    }

    /// The cleanup step of player `a`'s turn: hand size, damage and "this turn" effects end.
    fn cleanup(&mut self, a: usize) {
        let outer = std::mem::replace(&mut self.cause, audit::Cause::Rule("hand size"));
        while self.players[a].alive && self.players[a].hand.len() > 7 {
            self.discard(a);
        }
        self.cause = outer;
        if self.stats.is_some() {
            self.audit_turn_end(a);
        }
        for x in self.perms.iter_mut() {
            x.damage = 0;
            x.dealt_damage = false;
            x.deathtouched = false;
            x.temp_p = 0;
            x.temp_t = 0;
            x.temp_kw = 0;
            x.temp_base = None;
            x.temp_creature = false;
            x.returns_on_death = 0;
            x.crewed = false;
            if x.threaten_from.is_some() && x.ctrl as usize == a {
                x.ctrl = x.threaten_from.take().unwrap_or(x.ctrl);
            }
        }
        // Cards that could be played "this turn" / "until the end of your next turn" can't
        // any more.
        let now = (self.round, a as u8);
        for pl in self.players.iter_mut() {
            pl.pool = 0;
            pl.playable.retain(|x| x.until != now);
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

/// `Game::play_land_from` slot for the top card of the library (Ka-Zar).
pub const FROM_LIBRARY_TOP: usize = usize::MAX;

/// What `play` returns when the game is a draw: the last players left the game together
/// (CR 104.4a), so nobody wins.
pub const NO_WINNER: usize = usize::MAX;

/// Does seat `a` skip the draw of this turn (round `round`, `n` players)? Only the starting
/// player of a two-player game, on their first turn; in multiplayer games nobody does (CR
/// 103.8c).
pub fn skips_first_draw(n: usize, round: u32, a: usize) -> bool {
    n == 2 && round == 1 && a == 0
}

/// Plays one game; returns the winner's seat (`NO_WINNER` for a draw).
pub fn play_game(decks: &[&Prepared], rules: &Rules, rng: &mut Rng) -> usize {
    let seed = rng.next();
    let mut g = Game::new(decks, rules, Rng::new(seed));
    play(&mut g)
}

/// Plays a game to its end; returns the winner's seat (`NO_WINNER` for a draw).
pub fn play(g: &mut Game) -> usize {
    // A log set after `Game::new` still gets statistics (they name who eliminated whom).
    if g.log.is_some() && g.stats.is_none() {
        g.stats = Some(Box::new(audit::Stats::new(g.players.len())));
    }
    let n = g.players.len();
    for round in 1..=MAX_ROUNDS {
        g.round = round;
        for a in 0..n {
            if !g.players[a].alive {
                continue;
            }
            g.take_turns(a, skips_first_draw(n, round, a));
            let alive: Vec<usize> = (0..n).filter(|&i| g.players[i].alive).collect();
            match alive.len() {
                0 => {
                    g.audit_game_end(NO_WINNER, false, true);
                    return NO_WINNER;
                }
                1 => {
                    g.audit_game_end(alive[0], false, false);
                    return alive[0];
                }
                _ => {}
            }
        }
    }
    let w = (0..n)
        .max_by(|&x, &y| g.standing(x).partial_cmp(&g.standing(y)).unwrap())
        .unwrap_or(0);
    if g.log.is_some() {
        let st: Vec<String> = (0..n)
            .filter(|&p| g.players[p].alive)
            .map(|p| format!("P{} {:.1}", p, g.standing(p)))
            .collect();
        g.note(|| {
            format!(
                "Round limit reached: standings (life + board/2 - 3 x poison): {}",
                st.join(", ")
            )
        });
    }
    g.audit_game_end(w, true, false);
    w
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

/// Opponent decks for playtesting: one per other player in the league, from a simulation
/// of their season up to `rules.session` (their packs, drafts, rebuilds and Public Pool
/// trades - see `season`), with any real commanders or decklists pinned in
/// `data/opponents.json`. Cached per inputs (see `opponents`).
pub fn gauntlet(db: &CardDb, rules: &Rules, seed: u64) -> Vec<Prepared> {
    crate::opponents::decks(db, rules, seed)
        .iter()
        .map(|d| Prepared::new(d, db))
        .collect()
}
