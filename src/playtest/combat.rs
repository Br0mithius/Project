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
        x.is_creature() && !x.tapped && !(x.sick && !x.has(HASTE)) && !x.has(DEFENDER) && !x.locked
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
        true
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
        let declared = ai::declare_attacks(self, a);
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
            attacks.push(Attack {
                attacker: id,
                def,
                blockers: Vec::new(),
                blocked: false,
            });
        }
        if attacks.is_empty() {
            self.in_combat = false;
            return;
        }
        self.players[a].attacked_this_turn = true;
        self.note(|| format!("P{} attacks with {} creature(s)", a, attacks.len()));
        // "Whenever ~ attacks" and "whenever you attack" triggers.
        for at in &attacks {
            let Some(i) = self.idx(at.attacker) else {
                continue;
            };
            let x = self.perms[i].clone();
            for (k, ab) in self.abilities_of(&x).iter().enumerate() {
                if ab.trig == Trig::Attacks {
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
                    });
                }
            }
        }
        self.emit(Ev::OthersAttack, a, None);
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
        self.run_pending();
        self.sba();
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
            if strikes(&x) && x.p > 0 {
                if !at.blocked {
                    match at.def {
                        Def::Player(p) => to_players.push((p, x.p as i32, x.id)),
                        Def::Walker(w) => to_walkers.push((w, x.p)),
                    }
                } else {
                    let mut left = x.p;
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
                if strikes(b) && b.p > 0 {
                    to_perms.push((x.id, b.p, b.id));
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
            self.damage_perm(ti, n, kw & DEATHTOUCH != 0);
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
            let n = if self.doubles(ctrl as usize, "damage") {
                n * 2
            } else {
                n
            };
            self.damage_player(a, p, n, toxic);
            if cmd > 0 {
                self.players[p].cmd_damage[a][cmd as usize % 3] += n;
            }
            if kw & LIFELINK != 0 {
                lifelink.push((ctrl as usize, n));
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
                    });
                }
            }
        }
        self.sba();
        self.run_pending();
    }
}
