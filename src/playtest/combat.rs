//! Combat (CR 506-511): attacks on players and planeswalkers, legal blocks, first-strike
//! and regular damage steps, trample, deathtouch, lifelink, toxic, commander damage.

use super::ai;
use super::compile::*;
use super::state::*;

/// What an attacker is attacking.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Def {
    Player(usize),
    Walker(u32),
}

#[derive(Debug, Clone)]
pub struct Attack {
    pub attacker: u32,
    pub def: Def,
    pub blockers: Vec<u32>,
    pub blocked: bool,
}

impl<'a> Game<'a> {
    pub fn can_attack(&self, i: usize) -> bool {
        let x = &self.perms[i];
        // "~ can attack as though it didn't have defender" (while its condition holds),
        // "creatures you control can attack as though they didn't have defender" (Ghalta).
        let defender = x.has(DEFENDER)
            && !self.static_holds(i, |t| {
                t.contains("~ can attack as though it didn't have defender")
            })
            && self
                .rules
                .get(x.ctrl as usize)
                .is_none_or(|r| r & R_DEFENDERS_ATTACK == 0);
        // "~ can't attack unless you control two or more other Wolves".
        let barred = self
            .statics_of(i)
            .any(|(a, ok)| a.text.contains("~ can't attack unless") && !ok);
        x.is_creature()
            && !x.tapped
            && !(x.sick && !x.has(HASTE))
            && !defender
            && !x.locked
            && !barred
    }

    /// Permanent `i`'s own static abilities, each with whether its condition holds.
    pub fn statics_of(&self, i: usize) -> impl Iterator<Item = (&'a SimAbility, bool)> + '_ {
        let x = &self.perms[i];
        let p = x.ctrl as usize;
        self.abilities_of(x)
            .iter()
            .filter(|a| a.trig == Trig::Static)
            .map(move |a| (a, self.cond_holds(&a.cond, p, Some(i))))
    }

    /// Does permanent `i` have a static ability with this wording, whose condition holds?
    pub fn static_holds(&self, i: usize, f: impl Fn(&str) -> bool) -> bool {
        self.statics_of(i).any(|(a, ok)| ok && f(&a.text))
    }

    /// "Attacks each combat if able" (or goaded).
    pub fn must_attack(&self, i: usize) -> bool {
        self.perms[i].goad.is_some()
            || self.static_holds(i, |t| t.contains("attacks each combat if able"))
    }

    /// "~ can't be blocked by Walls / Humans / creatures with power 2 or less".
    fn evades(&self, a: usize, b: usize) -> bool {
        let bl = &self.perms[b];
        self.statics_of(a).any(|(ab, ok)| {
            let Some(what) = ok
                .then(|| ab.text.split("~ can't be blocked by ").nth(1))
                .flatten()
            else {
                return false;
            };
            let what = what.trim_end_matches('.');
            if let Some(n) = what
                .strip_prefix("creatures with power ")
                .and_then(|r| r.split_whitespace().next())
                .and_then(|d| d.parse::<i16>().ok())
            {
                return what.contains("or less") && bl.p <= n
                    || what.contains("or greater") && bl.p >= n;
            }
            what.split_whitespace()
                .find_map(crate::restrict::subtype_word)
                .is_some_and(|id| {
                    let s = crate::restrict::subtype_name(id);
                    bl.subtypes.iter().any(|x| x == s)
                })
        })
    }

    /// Can blocker `b` block attacker `a` (evasion, protection, "can't block")?
    pub fn can_block(&self, b: usize, a: usize) -> bool {
        let (bl, at) = (&self.perms[b], &self.perms[a]);
        if !bl.is_creature() || bl.tapped || bl.locked || bl.has(CANT_BLOCK) {
            return false;
        }
        if at.has(UNBLOCKABLE) {
            return false;
        }
        if at.has(FLYING) && !bl.has(FLYING | REACH) {
            return false;
        }
        if at.has(FEAR) && !(bl.is(T_ARTIFACT) || bl.colors & 0b100 != 0) {
            return false;
        }
        if at.has(INTIMIDATE) && !(bl.is(T_ARTIFACT) || bl.colors & at.colors != 0) {
            return false;
        }
        if at.has(SKULK) && bl.p > at.p {
            return false;
        }
        if at.protection & bl.colors != 0 {
            return false;
        }
        !self.evades(a, b)
    }

    /// Mana per creature attacking player `d` (Propaganda; Dáin with an enduring story).
    pub fn attack_tax(&self, d: usize) -> u32 {
        if self.rules.get(d).is_none_or(|r| r & R_ATTACK_TAX == 0) {
            return 0;
        }
        self.perms
            .iter()
            .filter(|x| x.ctrl as usize == d && self.perm_rules(x) & R_ATTACK_TAX != 0)
            .filter_map(|x| x.card.and_then(|r| self.card(r).attack_tax.as_ref()))
            .filter(|(_, cond)| self.cond_holds(cond, d, None))
            .map(|(n, _)| *n as u32)
            .sum()
    }

    pub fn def_player(&self, d: Def) -> Option<usize> {
        match d {
            Def::Player(p) => Some(p),
            Def::Walker(id) => self.idx(id).map(|i| self.perms[i].ctrl as usize),
        }
    }

    /// One combat phase for the active player `a`.
    pub fn combat(&mut self, a: usize) {
        ai::crew_for_combat(self, a);
        let mut declared = ai::declare_attacks(self, a);
        // "Creatures can't attack you unless their controller pays {2} for each": what
        // can't be paid for stays home.
        if self.rule_anywhere(R_ATTACK_TAX) {
            let mut keep = Vec::new();
            for (id, def) in declared {
                let tax = match def {
                    Def::Player(d) => self.attack_tax(d),
                    Def::Walker(_) => 0,
                };
                if tax == 0 || self.pay(a, &Mana::generic(tax)) {
                    keep.push((id, def));
                }
            }
            declared = keep;
        }
        if declared.is_empty() {
            return;
        }
        self.in_combat = true;
        let mut attacks: Vec<Attack> = Vec::new();
        for (id, def) in declared {
            let Some(i) = self.idx(id) else { continue };
            if !self.can_attack(i) {
                continue;
            }
            if !self.perms[i].has(VIGILANCE) {
                self.perms[i].tapped = true;
                self.emit(Ev::Tapped, a, Some(id));
            }
            let Some(i) = self.idx(id) else { continue };
            if let Some(d) = self.def_player(def) {
                let pw = self.perms[i].p.max(1) as f32;
                self.players[d].grudge[a] += pw;
            }
            self.perms[i].attacking = true;
            attacks.push(Attack {
                attacker: id,
                def,
                blockers: Vec::new(),
                blocked: false,
            });
        }
        if attacks.is_empty() {
            self.in_combat = false;
            self.end_combat();
            return;
        }
        self.players[a].attacked_this_turn = true;
        let bonus = self.players[a].attack_bonus;
        for at in &attacks {
            // Jace: attackers of this player (or their planeswalkers) get -5/-0.
            let weaken = self
                .def_player(at.def)
                .map_or(0, |d| self.players[d].attacked_power);
            if let Some(i) = self.idx(at.attacker) {
                let x = &mut self.perms[i];
                x.attacking = true;
                x.attacked_turn = true;
                x.temp_p += weaken;
                // Garruk: our attackers get +2/+2 and trample.
                if let Some((p, t, kw)) = bonus {
                    x.temp_p += p;
                    x.temp_t += t;
                    x.temp_kw |= kw;
                }
            }
        }
        self.recalc();
        self.note(|| format!("P{} attacks with {} creature(s)", a, attacks.len()));
        // "Whenever ~ attacks" and "whenever you attack" triggers.
        for at in &attacks {
            let Some(i) = self.idx(at.attacker) else {
                continue;
            };
            let x = self.perms[i].clone();
            for (k, ab) in self.abilities_of(&x).iter().enumerate() {
                if ab.trig == Trig::Attacks {
                    // "Attacks for the first time each turn".
                    if ab.once_per_turn && k < 32 {
                        if x.fired & (1 << k) != 0 {
                            continue;
                        }
                        self.perms[i].fired |= 1 << k;
                    }
                    self.pending.push(Pending {
                        ctrl: x.ctrl,
                        source: Some(x.id),
                        card: x.card.unwrap_or((x.ctrl, NO_CARD)),
                        back: false,
                        ability: k as u16,
                        snapshot: (x.card.is_none() || x.tok != Tok::None).then(|| x.clone()),
                        subject: Some(x.id),
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
        // "Whenever a creature you control attacks alone": about that creature.
        let lone = (attacks.len() == 1).then(|| attacks[0].attacker);
        self.emit(Ev::OthersAttack, a, lone);
        self.emit(Ev::OthersAttack, a, None);
        // "Whenever a creature you control attacks": once per attacker, about it.
        for at in &attacks {
            if let Some(i) = self.idx(at.attacker) {
                let x = self.perms[i].clone();
                self.emit_about(Ev::EachAttacker, a, &x);
            }
        }
        self.run_pending();
        self.sba();
        attacks.retain(|at| self.idx(at.attacker).is_some());
        // Defenders: instant-speed answers before blocks (removal, flash blockers, Fog).
        let mut defenders: Vec<usize> = attacks
            .iter()
            .filter_map(|at| self.def_player(at.def))
            .collect();
        defenders.sort();
        defenders.dedup();
        for &d in &defenders {
            if self.players[d].alive {
                ai::before_blocks(self, d, &attacks);
            }
        }
        attacks.retain(|at| self.idx(at.attacker).is_some());
        // "End the turn" (Time Stop) in answer to the attack: no blocks, no damage.
        if self.end_turn {
            self.in_combat = false;
            self.end_combat();
            return;
        }
        // Blocks.
        for &d in &defenders {
            if !self.players[d].alive {
                continue;
            }
            let mine: Vec<Attack> = attacks
                .iter()
                .filter(|at| self.def_player(at.def) == Some(d))
                .cloned()
                .collect();
            let blocks = ai::declare_blocks(self, d, &mine);
            for (b, at_id) in blocks {
                let (Some(bi), Some(ai_)) = (self.idx(b), self.idx(at_id)) else {
                    continue;
                };
                if self.perms[bi].ctrl as usize != d || !self.can_block(bi, ai_) {
                    continue;
                }
                if attacks.iter().any(|x| x.blockers.contains(&b)) {
                    continue;
                }
                if let Some(at) = attacks.iter_mut().find(|x| x.attacker == at_id) {
                    at.blockers.push(b);
                }
            }
        }
        // "Whenever ~ blocks" and "whenever a creature blocks".
        let blockers: Vec<u32> = attacks
            .iter()
            .flat_map(|at| at.blockers.iter().copied())
            .collect();
        for &b in &blockers {
            if let Some(bi) = self.idx(b) {
                self.perms[bi].blocking = true;
            }
        }
        for b in blockers {
            if let Some(bi) = self.idx(b) {
                let snap = self.perms[bi].clone();
                self.emit_about(Ev::Blocks, snap.ctrl as usize, &snap);
            }
        }
        let nblocks: usize = attacks.iter().map(|at| at.blockers.len()).sum();
        if nblocks > 0 {
            self.note(|| format!("AI blocks with {} creature(s)", nblocks));
        }
        // Menace needs two or more blockers.
        for at in attacks.iter_mut() {
            let menace = self
                .idx(at.attacker)
                .is_some_and(|i| self.perms[i].has(MENACE));
            if menace && at.blockers.len() == 1 {
                at.blockers.clear();
            }
            at.blocked = !at.blockers.is_empty();
        }
        // Flanking: blockers without it get -1/-1.
        for at in &attacks {
            let flank = self
                .idx(at.attacker)
                .is_some_and(|i| self.perms[i].has(FLANKING));
            if flank {
                for b in &at.blockers {
                    if let Some(bi) = self.idx(*b) {
                        if !self.perms[bi].has(FLANKING) {
                            self.perms[bi].temp_p -= 1;
                            self.perms[bi].temp_t -= 1;
                        }
                    }
                }
            }
        }
        self.recalc();
        self.sba();
        // Combat tricks: the attacker first, then each defender.
        ai::combat_tricks(self, a, &attacks);
        for &d in &defenders {
            if self.players[d].alive {
                ai::combat_tricks(self, d, &attacks);
            }
        }
        self.recalc();
        if !self.fog {
            self.damage_step(a, &attacks, true);
            self.damage_step(a, &attacks, false);
        }
        self.in_combat = false;
        self.end_combat();
        self.run_pending();
        self.sba();
    }

    /// Creatures stop attacking and blocking when combat ends.
    pub fn end_combat(&mut self) {
        for x in self.perms.iter_mut() {
            x.attacking = false;
            x.blocking = false;
        }
        self.recalc();
    }

    /// One combat damage step (`first` = the first-strike step).
    pub(crate) fn damage_step(&mut self, a: usize, attacks: &[Attack], first: bool) {
        let strikes = |x: &Perm| {
            let fs = x.has(FIRST_STRIKE | DOUBLE_STRIKE);
            if first {
                fs
            } else {
                !x.has(FIRST_STRIKE) || x.has(DOUBLE_STRIKE)
            }
        };
        let any_first = attacks.iter().any(|at| {
            self.idx(at.attacker)
                .is_some_and(|i| self.perms[i].has(FIRST_STRIKE | DOUBLE_STRIKE))
                || at.blockers.iter().any(|b| {
                    self.idx(*b)
                        .is_some_and(|i| self.perms[i].has(FIRST_STRIKE | DOUBLE_STRIKE))
                })
        });
        if first && !any_first {
            return;
        }
        // (target perm or player, amount, source id)
        let mut to_perms: Vec<(u32, i16, u32)> = Vec::new();
        let mut to_players: Vec<(usize, i32, u32)> = Vec::new();
        let mut to_walkers: Vec<(u32, i16)> = Vec::new();
        for at in attacks {
            let Some(ai_) = self.idx(at.attacker) else {
                continue;
            };
            let x = self.perms[ai_].clone();
            let blockers: Vec<usize> = at.blockers.iter().filter_map(|b| self.idx(*b)).collect();
            let power = self.combat_power(&x);
            if strikes(&x) && power > 0 {
                if !at.blocked {
                    match at.def {
                        Def::Player(p) => to_players.push((p, power as i32, x.id)),
                        Def::Walker(w) => to_walkers.push((w, power)),
                    }
                } else {
                    let mut left = power;
                    for (k, &bi) in blockers.iter().enumerate() {
                        let b = &self.perms[bi];
                        let lethal = if x.has(DEATHTOUCH) {
                            1
                        } else {
                            (b.t - b.damage).max(0)
                        };
                        let last = k + 1 == blockers.len();
                        let give = if last && !x.has(TRAMPLE) {
                            left
                        } else {
                            left.min(lethal)
                        };
                        if give > 0 {
                            to_perms.push((b.id, give, x.id));
                        }
                        left -= give;
                    }
                    if x.has(TRAMPLE) && left > 0 {
                        match at.def {
                            Def::Player(p) => to_players.push((p, left as i32, x.id)),
                            Def::Walker(w) => to_walkers.push((w, left)),
                        }
                    }
                }
            }
            for &bi in &blockers {
                let b = &self.perms[bi];
                let power = self.combat_power(b);
                if strikes(b) && power > 0 {
                    to_perms.push((x.id, power, b.id));
                }
            }
        }
        // Apply (all at once: sources were read before any damage).
        let src_info: std::collections::HashMap<u32, (u8, u32, u8, u8, u8)> = self
            .perms
            .iter()
            .map(|x| {
                let toxic = x.card.map(|r| self.card(r).toxic).unwrap_or(0);
                (x.id, (x.ctrl, x.eff_kw, x.colors, x.commander, toxic))
            })
            .collect();
        let mut lifelink: Vec<(usize, i32)> = Vec::new();
        for (target, n, src) in to_perms {
            let Some(&(ctrl, kw, colors, _, _)) = src_info.get(&src) else {
                continue;
            };
            let Some(ti) = self.idx(target) else { continue };
            if self.perms[ti].protection & colors != 0 {
                continue;
            }
            self.perms[ti].hurt_by = src;
            if let Some(si) = self.idx(src) {
                self.perms[si].ever_hit = true;
            }
            let Some(ti) = self.idx(target) else { continue };
            self.damage_perm(ti, n, kw & DEATHTOUCH != 0);
            if let Some(si) = self.idx(src) {
                self.perms[si].dealt_damage = true;
            }
            if kw & LIFELINK != 0 {
                lifelink.push((ctrl as usize, n as i32));
            }
        }
        for (w, n) in to_walkers {
            if let Some(wi) = self.idx(w) {
                self.perms[wi].loyalty -= n;
            }
        }
        let mut dealt: Vec<u32> = Vec::new();
        for (p, n, src) in to_players {
            let Some(&(ctrl, kw, _, cmd, toxic)) = src_info.get(&src) else {
                continue;
            };
            let n = if self.doubles_from(ctrl as usize, "damage", Some(src)) {
                n * 2
            } else {
                n
            };
            self.damage_player(a, p, n, toxic);
            if let Some(si) = self.idx(src) {
                self.perms[si].dealt_damage = true;
            }
            if cmd > 0 {
                self.players[p].cmd_damage[a][cmd as usize % 3] += n;
            }
            if kw & LIFELINK != 0 {
                lifelink.push((ctrl as usize, n));
            }
            if let Some(si) = self.idx(src) {
                self.perms[si].hit_turn |= 1 << p.min(7);
                self.perms[si].ever_hit = true;
            }
            dealt.push(src);
        }
        for (p, n) in lifelink {
            self.gain_life(p, n);
        }
        // "Whenever ~ deals combat damage to a player."
        for id in dealt {
            let Some(i) = self.idx(id) else { continue };
            let x = self.perms[i].clone();
            self.emit_about(Ev::CombatDamageAny, x.ctrl as usize, &x);
            for (k, ab) in self.abilities_of(&x).iter().enumerate() {
                if ab.trig == Trig::CombatDamage {
                    self.pending.push(Pending {
                        ctrl: x.ctrl,
                        source: Some(x.id),
                        card: x.card.unwrap_or((x.ctrl, NO_CARD)),
                        back: false,
                        ability: k as u16,
                        snapshot: (x.card.is_none() || x.tok != Tok::None).then(|| x.clone()),
                        subject: Some(x.id),
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
        self.sba();
        self.run_pending();
    }
}
