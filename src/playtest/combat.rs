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
        let mut declared = if self.log.is_some() {
            let mut why = Vec::new();
            let d = ai::declare_attacks_why(self, a, Some(&mut why));
            for w in why {
                self.note(|| format!("AI P{} attack: {}", a, w));
            }
            d
        } else {
            ai::declare_attacks(self, a)
        };
        // "Creatures can't attack you unless their controller pays {2} for each": what
        // can't be paid for stays home.
        if self.rule_anywhere(R_ATTACK_TAX) {
            let mut keep = Vec::new();
            let n = declared.len();
            for (id, def) in declared {
                let tax = match def {
                    Def::Player(d) => self.attack_tax(d),
                    Def::Walker(_) => 0,
                };
                if tax == 0 || self.pay(a, &Mana::generic(tax)) {
                    keep.push((id, def));
                }
            }
            let unpaid = n - keep.len();
            if unpaid > 0 {
                self.note(|| format!("P{} {} attacker(s) stay home: attack tax unpaid", a, unpaid));
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
        self.ext.attack_def = attacks
            .iter()
            .filter_map(|at| self.def_player(at.def).map(|d| (at.attacker, d as u8)))
            .collect();
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
        if self.auditing() {
            self.audit_attacks(a, &attacks);
        }
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
        // "Whenever you attack" once per combat (CR 508.1m); "whenever a creature you control
        // attacks alone" (`Cond::AttacksAlone`) is about the lone attacker.
        let lone = (attacks.len() == 1).then(|| attacks[0].attacker);
        self.emit(Ev::OthersAttack, a, lone);
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
            let blocks = if self.log.is_some() {
                let mut why = Vec::new();
                let b = ai::declare_blocks_why(self, d, &mine, Some(&mut why));
                for w in why {
                    self.note(|| format!("AI P{} block: {}", d, w));
                }
                b
            } else {
                ai::declare_blocks(self, d, &mine)
            };
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
        // Menace needs two or more blockers: a lone blocker's block was never legal (it
        // doesn't block, nor trigger "whenever ~ blocks").
        for at in attacks.iter_mut() {
            let menace = self
                .idx(at.attacker)
                .is_some_and(|i| self.perms[i].has(MENACE));
            if menace && at.blockers.len() == 1 {
                if self.log.is_some() {
                    let (x, b) = (self.id_desc(at.attacker), self.id_desc(at.blockers[0]));
                    self.note(|| format!("  menace: {} can't be blocked by {} alone", x, b));
                }
                at.blockers.clear();
            }
            at.blocked = !at.blockers.is_empty();
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
        if self.auditing() {
            self.audit_blocks(&attacks);
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
        } else {
            self.note(|| "  combat damage is prevented (fog)".into());
        }
        self.in_combat = false;
        self.end_combat();
        self.run_pending();
        self.sba();
    }

    /// Creatures stop attacking and blocking when combat ends.
    pub fn end_combat(&mut self) {
        self.ext.attack_def.clear();
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
        let mut to_players: Vec<(usize, i32, u32, bool)> = Vec::new();
        let mut to_walkers: Vec<(u32, i16, u32)> = Vec::new();
        for at in attacks {
            let Some(ai_) = self.idx(at.attacker) else {
                continue;
            };
            let x = self.perms[ai_].clone();
            let mut blockers: Vec<usize> =
                at.blockers.iter().filter_map(|b| self.idx(*b)).collect();
            if blockers.len() > 1 {
                // The attacking player orders the blockers for damage (CR 510.1c).
                blockers = ai::damage_order(self, ai_, &blockers);
            }
            let power = self.combat_power(&x);
            if strikes(&x) && power > 0 {
                if !at.blocked {
                    match at.def {
                        Def::Player(p) => to_players.push((p, power as i32, x.id, false)),
                        Def::Walker(w) => to_walkers.push((w, power, x.id)),
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
                            Def::Player(p) => to_players.push((p, left as i32, x.id, true)),
                            Def::Walker(w) => to_walkers.push((w, left, x.id)),
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
        let src_info: std::collections::HashMap<u32, (u8, u32, u8, u8, u8, u8)> = self
            .perms
            .iter()
            .map(|x| {
                let toxic = x.card.map(|r| self.card(r).toxic).unwrap_or(0);
                (
                    x.id,
                    (x.ctrl, x.eff_kw, x.colors, x.commander, toxic, x.owner),
                )
            })
            .collect();
        // Damage doublers (Gratuitous Violence, Twinflame Tyrant) double combat damage to
        // creatures and planeswalkers too.
        let doubled = |g: &Self, ctrl: u8, src: u32, n: i16| {
            if g.doubles_from(ctrl as usize, "damage", Some(src)) {
                n.saturating_mul(2)
            } else {
                n
            }
        };
        let mut lifelink: Vec<(usize, i32, u32)> = Vec::new();
        let outer_cause = self.cause;
        // All of this step's damage is dealt at once: one "is dealt damage" event per
        // permanent (Red Hulk's Enrage triggers once, not once per blocker).
        self.ext.defer_damaged = Some(Vec::new());
        for (target, n, src) in to_perms {
            let Some(&(ctrl, kw, colors, _, _, _)) = src_info.get(&src) else {
                continue;
            };
            let n = doubled(self, ctrl, src, n);
            let Some(ti) = self.idx(target) else { continue };
            if self.perms[ti].protection & colors != 0 {
                if self.log.is_some() {
                    let (s, t) = (self.id_desc(src), self.id_desc(target));
                    self.note(|| format!("  {} -> {}: {} damage prevented (protection)", s, t, n));
                }
                continue;
            }
            if self.log.is_some() {
                let (s, t) = (self.id_desc(src), self.id_desc(target));
                let dt = if kw & DEATHTOUCH != 0 {
                    " (deathtouch)"
                } else {
                    ""
                };
                let ll = if kw & LIFELINK != 0 {
                    " (lifelink)"
                } else {
                    ""
                };
                let step = if first { " [first strike]" } else { "" };
                self.note(|| {
                    format!(
                        "  {} deals {} combat damage to {}{}{}{}",
                        s, n, t, dt, ll, step
                    )
                });
            }
            self.perms[ti].hurt_by = src;
            if let Some(si) = self.idx(src) {
                self.perms[si].ever_hit = true;
            }
            let Some(ti) = self.idx(target) else { continue };
            // Prevented damage (shield counters, "prevent all damage") gives no life.
            let dealt = self.damage_perm(ti, n, kw & DEATHTOUCH != 0);
            if let Some(si) = self.idx(src) {
                self.perms[si].dealt_damage = true;
            }
            if kw & LIFELINK != 0 && dealt > 0 {
                lifelink.push((ctrl as usize, dealt as i32, src));
            }
        }
        for (w, n, src) in to_walkers {
            let Some(&(ctrl, kw, colors, _, _, _)) = src_info.get(&src) else {
                continue;
            };
            let n = doubled(self, ctrl, src, n);
            let Some(wi) = self.idx(w) else { continue };
            if self.perms[wi].protection & colors != 0 {
                continue;
            }
            let dealt = self.damage_perm(wi, n, false);
            if self.log.is_some() {
                let d = self.id_desc(w);
                self.note(|| format!("  {} combat damage to planeswalker {}", dealt, d));
            }
            if let Some(si) = self.idx(src) {
                self.perms[si].dealt_damage = true;
            }
            if kw & LIFELINK != 0 && dealt > 0 {
                lifelink.push((ctrl as usize, dealt as i32, src));
            }
        }
        for id in self.ext.defer_damaged.take().unwrap_or_default() {
            if let Some(i) = self.idx(id) {
                let snap = self.perms[i].clone();
                self.emit_about(Ev::Damaged, snap.ctrl as usize, &snap);
            }
        }
        let mut dealt: Vec<u32> = Vec::new();
        for (p, n, src, trample) in to_players {
            let Some(&(ctrl, kw, _, cmd, toxic, owner)) = src_info.get(&src) else {
                continue;
            };
            let n = if self.doubles_from(ctrl as usize, "damage", Some(src)) {
                n * 2
            } else {
                n
            };
            if trample && self.log.is_some() {
                let d = self.id_desc(src);
                self.note(|| format!("  {} tramples over for {}", d, n));
            }
            if let Some(s) = self.stats.as_mut() {
                s.players[a].combat_to[p % 8] += n;
            }
            self.cause = super::audit::Cause::Combat(src, ctrl);
            self.damage_player(a, p, n, toxic);
            self.cause = outer_cause;
            if let Some(si) = self.idx(src) {
                self.perms[si].dealt_damage = true;
            }
            // Commander damage is counted per commander: whoever controls it now, it's its
            // owner's (CR 903.10a).
            if cmd > 0 {
                let o = owner as usize % 8;
                self.players[p].cmd_damage[o][cmd as usize % 3] += n;
                let total = self.players[p].cmd_damage[o][cmd as usize % 3];
                self.note(|| {
                    format!(
                        "  commander damage to P{} from P{}'s commander: {} total",
                        p, o, total
                    )
                });
            }
            if kw & LIFELINK != 0 {
                lifelink.push((ctrl as usize, n, src));
            }
            if let Some(si) = self.idx(src) {
                self.perms[si].hit_turn |= 1 << p.min(7);
                self.perms[si].ever_hit = true;
            }
            dealt.push(src);
        }
        for (p, n, src) in lifelink {
            self.cause = super::audit::Cause::Combat(src, p as u8);
            self.gain_life(p, n);
            self.cause = outer_cause;
        }
        // "Whenever ~ deals combat damage to a player."
        let from = self.pending.len();
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
        // "Whenever one or more Heroes you control deal combat damage to a player": once for
        // the damage step, not once per creature.
        let mut seen: Vec<(Option<u32>, (u8, u16), u16)> = Vec::new();
        let mut k = from;
        while k < self.pending.len() {
            let pend = &self.pending[k];
            let key = (pend.source, pend.card, pend.ability);
            let batch = self
                .pending_ability(pend)
                .is_some_and(|a| trigger_part(&a.text).contains("one or more"));
            if batch && seen.contains(&key) {
                self.pending.remove(k);
                continue;
            }
            if batch {
                seen.push(key);
            }
            k += 1;
        }
        self.sba();
        self.run_pending();
    }
}
