//! Resolving abilities: every effect the ability parser reads (`abilities::Effect`) does
//! its thing here, with targets picked by the AI (`ai`).

use super::ai;
use super::compile::*;
use super::state::*;
use crate::abilities::{Amount, Effect, Kind, Target, Whose};

/// Where an ability is resolving from.
#[derive(Clone)]
pub struct Ctx<'a> {
    pub ctrl: usize,
    /// The source permanent (it may have left the battlefield since).
    pub src: Option<u32>,
    /// Last known information about the source.
    pub snap: Option<Perm>,
    pub card: Option<&'a SimCard>,
    /// What the triggering event was about (the creature that died, entered...).
    pub subject: Option<u32>,
    pub text: &'a str,
    pub until_eot: bool,
    /// Cast with overload: "target" means "each" (of the opponents' things).
    pub overload: bool,
}

/// Labels of `Effect::Rule` the simulator acts on; the rest change nothing in its games.
pub const RULES_MODELLED: &[&str] = &[
    "you control an opponent's next turn",
    "stops all life gain",
    "gives poison counters",
    "returns itself to hand",
    "weakens a creature's attack",
    "weakens attackers",
    "heals himself",
    "can't stay in a graveyard",
    "exiles cards instead of the graveyard",
    "returns the exiled card",
    "returns an exiled card",
    "recycles a graveyard card",
    "removes counters",
    "moves its counters",
    "gives a permanent away",
    "turns a permanent into a creature",
    "sets base power/toughness",
    "adds to noncombat damage",
    "everyone draws an extra card",
    "exiles itself",
    "mills opponents",
    "reshuffles the graveyard",
    "returns to the top of the library",
    "untaps your creatures",
    "steals the enchanted permanent",
    "fetches a card from outside the game",
];

impl<'a> Game<'a> {
    pub fn src_idx(&self, ctx: &Ctx) -> Option<usize> {
        ctx.src.and_then(|id| self.idx(id))
    }

    fn src_perm(&self, ctx: &Ctx) -> Option<Perm> {
        self.src_idx(ctx)
            .map(|i| self.perms[i].clone())
            .or_else(|| ctx.snap.clone())
    }

    /// Doubling effects (CR 614) a player controls: "token", "counter" or "damage".
    pub fn doubles(&self, p: usize, kind: &str) -> bool {
        self.perms.iter().any(|x| {
            x.ctrl as usize == p
                && self.abilities_of(x).iter().any(|a| {
                    a.effects
                        .iter()
                        .any(|e| matches!(e, Effect::Doubler(k) if k.contains(kind)))
                })
        })
    }

    /// X and "that many" values, counted from the board where the text says how.
    pub fn x_value(&self, ctx: &Ctx, a: &Amount) -> i32 {
        if !a.x {
            return a.n;
        }
        let t = ctx.text;
        let me = ctx.ctrl;
        let mine = |f: &dyn Fn(&Perm) -> bool| {
            self.perms
                .iter()
                .filter(|x| x.ctrl as usize == me && f(x))
                .count() as i32
        };
        let after = t.split("number of").nth(1).unwrap_or("");
        let n = if !after.is_empty() {
            if after.contains("creatures you control") || after.contains("creature you control") {
                mine(&|x| x.is_creature())
            } else if after.contains("artifacts you control") {
                mine(&|x| x.is(T_ARTIFACT))
            } else if after.contains("lands you control")
                || after.contains("islands")
                || after.contains("mountains")
                || after.contains("forests")
                || after.contains("plains")
                || after.contains("swamps")
            {
                mine(&|x| x.is(T_LAND))
            } else if after.contains("cards in your hand") {
                self.players[me].hand.len() as i32
            } else if after.contains("cards in your graveyard")
                || after.contains("creature cards in your graveyard")
            {
                self.players[me].graveyard.len() as i32
            } else if after.contains("opponents") {
                self.opponents(me).len() as i32
            } else if after.contains("equipment") {
                let id = ctx.src.unwrap_or(u32::MAX);
                self.perms
                    .iter()
                    .filter(|x| x.is(T_EQUIPMENT) && x.attached == Some(id))
                    .count() as i32
            } else if after.contains("counters") {
                self.src_perm(ctx)
                    .map(|x| x.counters as i32 + x.other_counters as i32)
                    .unwrap_or(1)
            } else {
                a.n
            }
        } else if t.contains("its power")
            || t.contains("that creature's power")
            || t.contains("equal to the power")
        {
            ctx.subject
                .and_then(|id| self.idx(id))
                .map(|i| self.perms[i].p as i32)
                .or_else(|| self.src_perm(ctx).map(|x| x.p as i32))
                .unwrap_or(a.n)
        } else if t.contains("mana value") {
            ctx.subject
                .and_then(|id| self.idx(id))
                .and_then(|i| self.perms[i].card)
                .map(|r| self.card(r).mv as i32)
                .unwrap_or(a.n)
        } else {
            a.n
        };
        n.max(0)
    }

    /// Can this permanent be hit by an effect of `kind`?
    fn kind_ok(x: &Perm, kind: Kind) -> bool {
        match kind {
            Kind::Creature => x.is_creature(),
            Kind::CreatureOrPlaneswalker | Kind::AnyTarget => {
                x.is_creature() || x.is(T_PLANESWALKER)
            }
            Kind::NonlandPermanent => !x.is(T_LAND),
            Kind::Permanent => true,
            Kind::Artifact => x.is(T_ARTIFACT),
            Kind::Enchantment => x.is(T_ENCHANTMENT),
            Kind::ArtifactOrEnchantment => x.is(T_ARTIFACT | T_ENCHANTMENT),
            Kind::Planeswalker => x.is(T_PLANESWALKER),
            Kind::Land => x.is(T_LAND),
            Kind::It | Kind::Player | Kind::Spell | Kind::Other => false,
        }
    }

    /// Permanents an effect could hit. `harmful`: for "any" targets, the AI points harmful
    /// effects at opponents and helpful ones at itself.
    pub fn candidates(&self, ctx: &Ctx, t: &Target, harmful: bool) -> Vec<usize> {
        let me = ctx.ctrl;
        let src_colors = ctx.card.map(|c| c.colors).unwrap_or(0);
        let src_id = ctx.src;
        let text = ctx.text;
        // "equipped creature" / "enchanted creature": what the source is attached to.
        if (text.contains("equipped creature") || text.contains("enchanted creature")) && !t.mass {
            if let Some(a) = self.src_perm(ctx).and_then(|s| s.attached) {
                return self.idx(a).into_iter().collect();
            }
        }
        if t.kind == Kind::It {
            return self.src_idx(ctx).into_iter().collect();
        }
        let another = t.other || text.contains("another");
        let mut out = Vec::new();
        for (i, x) in self.perms.iter().enumerate() {
            if !Self::kind_ok(x, t.kind) {
                // "Other" kinds that name a creature type ("target Hero")
                if !(t.kind == Kind::Other && x.is_creature()) {
                    continue;
                }
            }
            let mine = x.ctrl as usize == me;
            let side = match t.whose {
                Whose::Yours => mine,
                Whose::Theirs => !mine,
                Whose::Any => {
                    if t.mass {
                        true
                    } else if harmful {
                        !mine
                    } else {
                        mine
                    }
                }
            };
            if !side {
                continue;
            }
            if another && Some(x.id) == src_id && !t.mass {
                continue;
            }
            if t.restricted && !t.mass {
                let mv = x.card.map(|r| self.card(r).mv).unwrap_or(0);
                if x.p > 3 && mv > 3 {
                    continue;
                }
            }
            if !t.mass {
                if x.has(SHROUD) || (!mine && x.has(HEXPROOF)) {
                    continue;
                }
                if x.protection & src_colors != 0 && !mine {
                    continue;
                }
                if !mine && x.ward > 0 && !self.can_pay(me, &Mana::generic(x.ward as u32)) {
                    continue;
                }
            }
            out.push(i);
        }
        out
    }

    /// Picks targets and handles targeting side effects (ward is paid, "becomes the target"
    /// triggers, the owner may respond with protection). Returns perm ids still valid.
    fn choose(&mut self, ctx: &Ctx, t: &Target, harmful: bool) -> Vec<u32> {
        let over = Target {
            mass: true,
            whose: if harmful { Whose::Theirs } else { t.whose },
            ..*t
        };
        let t = if ctx.overload && !t.mass { &over } else { t };
        let cands = self.candidates(ctx, t, harmful);
        if t.mass {
            return cands.iter().map(|&i| self.perms[i].id).collect();
        }
        let n = t.count.max(1) as usize;
        let picks = if harmful {
            ai::choose_victims(self, ctx.ctrl, &cands, n)
        } else {
            ai::choose_friends(self, ctx.ctrl, &cands, n)
        };
        let ids: Vec<u32> = picks.iter().map(|&i| self.perms[i].id).collect();
        let mut out = Vec::new();
        for id in ids {
            let Some(i) = self.idx(id) else { continue };
            let owner = self.perms[i].ctrl as usize;
            if owner != ctx.ctrl {
                let ward = self.perms[i].ward;
                if ward > 0 && !self.pay(ctx.ctrl, &Mana::generic(ward as u32)) {
                    continue; // countered by ward
                }
                if harmful && ai::protect(self, owner, id) {
                    // The owner answered with protection; check the target is still legal.
                    match self.idx(id) {
                        Some(j) if !self.perms[j].has(HEXPROOF | SHROUD) => {}
                        _ => continue,
                    }
                }
            }
            let snap = self.perms[self.idx(id).unwrap_or(i).min(self.perms.len() - 1)].clone();
            if snap.id == id {
                self.emit_about(Ev::Targeted, snap.ctrl as usize, &snap);
            }
            out.push(id);
        }
        out
    }

    /// Resolves one pending trigger or spell ability.
    pub fn resolve_pending(&mut self, pend: Pending) {
        let me = pend.ctrl as usize;
        if !self.players.get(me).is_some_and(|p| p.alive) {
            return;
        }
        let abilities: &'a [SimAbility] = if pend.card.1 == NO_CARD {
            match &pend.snapshot {
                Some(s) => self.abilities_of(s),
                None => match pend.source.and_then(|id| self.idx(id)) {
                    Some(i) => self.abilities_of(&self.perms[i].clone()),
                    None => return,
                },
            }
        } else {
            let c = self.card(pend.card);
            if pend.back {
                match &c.back {
                    Some(b) => &b.abilities,
                    None => return,
                }
            } else {
                match (pend.source.and_then(|id| self.idx(id)), &pend.snapshot) {
                    // Tokens copying a card use the card's abilities; item tokens their own.
                    (Some(i), _) if self.perms[i].card.is_none() => {
                        self.abilities_of(&self.perms[i].clone())
                    }
                    _ => &c.abilities,
                }
            }
        };
        let Some(ab) = abilities.get(pend.ability as usize) else {
            return;
        };
        let src_idx = pend.source.and_then(|id| self.idx(id));
        let eval = |c: &Cond| match c {
            Cond::Teamwork => pend.teamwork,
            Cond::Gift => pend.gift,
            Cond::CastFromGraveyard => pend.from_graveyard,
            Cond::Kicked => pend.kicked,
            c => self.cond_holds(c, me, src_idx),
        };
        let holds = match &ab.cond {
            Cond::Not(inner) => !eval(inner),
            c => eval(c),
        };
        if !holds {
            return;
        }
        let card = (pend.card.1 != NO_CARD).then(|| self.card(pend.card));
        let ctx = Ctx {
            ctrl: me,
            src: pend.source,
            snap: pend.snapshot.clone(),
            card,
            subject: pend.subject,
            text: &ab.text,
            until_eot: ab.until_eot,
            overload: pend.overloaded,
        };
        if ab.optional && !ai::wants_optional(self, &ctx, ab) {
            return;
        }
        let picks: Vec<usize> = if ab.modal {
            ai::choose_modes(self, &ctx, ab)
        } else {
            (0..ab.effects.len()).collect()
        };
        self.note(|| {
            format!(
                "P{} resolves {}: {}",
                me,
                card.map(|c| c.name.as_str()).unwrap_or("token"),
                ab.text
            )
        });
        for k in picks {
            if !self.players[me].alive {
                return;
            }
            self.apply(&ctx, &ab.effects[k]);
        }
        // "When you do, ..." right after this ability (not the counter-goal kind).
        if let Some(next) = abilities.get(pend.ability as usize + 1) {
            if next.trig == Trig::Reflexive && next.counter_goal.is_none() {
                self.pending.insert(
                    0,
                    Pending {
                        ability: pend.ability + 1,
                        ..pend.clone()
                    },
                );
            }
        }
    }

    /// Runs triggered abilities until none are waiting (bounded, against loops).
    pub fn run_pending(&mut self) {
        let mut guard = 0;
        while !self.pending.is_empty() {
            guard += 1;
            if guard > 60 || self.alive_count() <= 1 {
                self.pending.clear();
                break;
            }
            // Active player's triggers first (APNAP), then in the order they happened.
            let a = self.active as u8;
            let k = self.pending.iter().position(|p| p.ctrl == a).unwrap_or(0);
            let pend = self.pending.remove(k);
            self.resolve_pending(pend);
            self.sba();
        }
    }

    fn add_counters(&mut self, i: usize, n: i32) {
        let p = self.perms[i].ctrl as usize;
        let n = if self.doubles(p, "counter") { n * 2 } else { n };
        self.perms[i].counters += n as i16;
        let snap = self.perms[i].clone();
        self.emit_about(Ev::Counters, p, &snap);
    }

    /// Plan, lore and other counters; fires "when the Nth counter is put" abilities.
    pub fn add_other_counters(&mut self, i: usize, n: i16) {
        let before = self.perms[i].other_counters;
        self.perms[i].other_counters += n;
        let after = self.perms[i].other_counters;
        let x = self.perms[i].clone();
        if let Some(r) = x.card {
            let c = self.card(r);
            for (ai, a) in c.abilities.iter().enumerate() {
                if a.trig != Trig::Reflexive {
                    continue;
                }
                if let Some(goal) = a.counter_goal {
                    if before < goal && after >= goal {
                        // "When the Nth counter is put on ~, sacrifice it. When you do, ..."
                        let sac = c.abilities.iter().any(|b| {
                            b.text.contains("sacrifice it") || b.text.contains("sacrifice ~")
                        });
                        self.pending.push(Pending {
                            ctrl: x.ctrl,
                            source: Some(x.id),
                            card: r,
                            back: false,
                            ability: ai as u16,
                            snapshot: Some(x.clone()),
                            subject: None,
                            teamwork: false,
                            gift: false,
                            from_graveyard: false,
                            kicked: false,
                            overloaded: false,
                        });
                        if sac {
                            if let Some(j) = self.idx(x.id) {
                                self.sacrifice(j);
                            }
                        }
                    }
                }
            }
            // Sagas: the chapter abilities.
            if c.is(T_SAGA) {
                for (ai, a) in c.abilities.iter().enumerate() {
                    if let Trig::Chapter(ch) = a.trig {
                        if (ch as i16) > before && (ch as i16) <= after {
                            self.pending.push(Pending {
                                ctrl: x.ctrl,
                                source: Some(x.id),
                                card: r,
                                back: false,
                                ability: ai as u16,
                                snapshot: Some(x.clone()),
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
        }
        self.emit_about(Ev::Counters, x.ctrl as usize, &x);
    }

    pub fn make_token(
        &mut self,
        ctrl: usize,
        p: i32,
        t: i32,
        kw: u32,
        subtypes: Vec<String>,
    ) -> usize {
        let i = self.new_perm(ctrl, ctrl, None, Tok::Creature);
        let x = &mut self.perms[i];
        x.base_p = p as i16;
        x.base_t = t as i16;
        x.kw = kw;
        x.subtypes = subtypes;
        i
    }

    pub fn make_item(&mut self, ctrl: usize, kind: &str) {
        let tok = match kind {
            "treasure" | "powerstone" | "gold" => Tok::Treasure,
            "food" => Tok::Food,
            "clue" => Tok::Clue,
            "map" => Tok::Map,
            "blood" => Tok::Blood,
            _ => Tok::Other,
        };
        let i = self.new_perm(ctrl, ctrl, None, tok);
        self.perms[i].sick = false;
        self.after_enter(i);
    }

    /// Discards a card the player chooses (the worst one).
    pub fn discard(&mut self, p: usize) {
        if let Some(h) = ai::discard_choice(self, p) {
            let c = self.players[p].hand.remove(h);
            self.players[p].graveyard.push(c);
            self.emit(Ev::Discard, p, None);
        }
    }

    pub fn mill(&mut self, p: usize, n: i32) {
        for _ in 0..n {
            if let Some(c) = self.players[p].library.pop() {
                self.players[p].graveyard.push(c);
            }
        }
    }

    /// Fetches a land from the library (one adding a colour the hand needs).
    fn fetch_land(&mut self, p: usize, to_battlefield: bool, tapped: bool, basic_only: bool) {
        let Some(k) = ai::land_to_fetch(self, p, basic_only) else {
            return;
        };
        let c = self.players[p].library.remove(k);
        let n = self.players[p].library.len();
        if n > 1 {
            // Searching shuffles.
            let mut lib = std::mem::take(&mut self.players[p].library);
            self.rng.shuffle(&mut lib);
            self.players[p].library = lib;
        }
        if to_battlefield {
            if let Some(i) = self.enter_card(p, c, p, false) {
                if tapped {
                    self.perms[i].tapped = true;
                }
            }
        } else {
            self.players[p].hand.push(c);
        }
    }

    /// Deals noncombat damage from a source to a permanent.
    fn hurt(&mut self, ctx: &Ctx, id: u32, n: i32) {
        let Some(i) = self.idx(id) else { return };
        let src = self.src_perm(ctx);
        let deathtouch = src
            .as_ref()
            .is_some_and(|s| s.has(DEATHTOUCH) && s.is_creature());
        let n = if self.doubles(ctx.ctrl, "damage") {
            n * 2
        } else {
            n
        };
        if self.perms[i].protection & ctx.card.map(|c| c.colors).unwrap_or(0) != 0 {
            return;
        }
        self.damage_perm(i, n as i16, deathtouch);
        if src.is_some_and(|s| s.has(LIFELINK) && s.is_creature()) {
            self.gain_life(ctx.ctrl, n);
        }
    }

    fn hurt_player(&mut self, ctx: &Ctx, o: usize, n: i32) {
        let n = if self.doubles(ctx.ctrl, "damage") {
            n * 2
        } else {
            n
        };
        self.damage_player(ctx.ctrl, o, n, 0);
        if n > 0 && o != ctx.ctrl {
            self.emit(Ev::NoncombatDamage, o, None);
        }
        if self
            .src_perm(ctx)
            .is_some_and(|s| s.has(LIFELINK) && s.is_creature())
        {
            self.gain_life(ctx.ctrl, n);
        }
    }

    /// Applies one effect.
    pub fn apply(&mut self, ctx: &Ctx, e: &Effect) {
        let me = ctx.ctrl;
        let text = ctx.text;
        match e {
            Effect::Destroy(t) => {
                let harmful = t.whose != Whose::Yours;
                for id in self.choose(ctx, t, harmful) {
                    if let Some(i) = self.idx(id) {
                        self.destroy(i);
                    }
                }
            }
            Effect::Exile { target, until } => {
                let harmful = target.whose != Whose::Yours;
                for id in self.choose(ctx, target, harmful) {
                    let Some(i) = self.idx(id) else { continue };
                    let x = self.perms[i].clone();
                    self.leave(i, Zone::Exile, false);
                    if *until && !target.mass {
                        if let (Some(r), Some(s)) = (x.card, self.src_idx(ctx)) {
                            if x.tok == Tok::None && x.commander == 0 {
                                self.perms[s].holding.push((x.owner, r));
                            }
                        }
                    }
                }
            }
            Effect::Damage { amount, to } => {
                let n = self.x_value(ctx, amount);
                if n <= 0 {
                    return;
                }
                let each_opp = text.contains("each opponent") || text.contains("each player");
                if to.kind == Kind::Player || (to.mass && each_opp) {
                    let targets: Vec<usize> = if to.mass || each_opp {
                        if text.contains("each player") {
                            (0..self.players.len())
                                .filter(|&o| self.players[o].alive)
                                .collect()
                        } else {
                            self.opponents(me)
                        }
                    } else {
                        ai::choose_opponent(self, me, n).into_iter().collect()
                    };
                    for o in targets {
                        self.hurt_player(ctx, o, n);
                    }
                    if to.kind == Kind::Player {
                        return;
                    }
                }
                if to.mass {
                    for id in self.choose(ctx, to, true) {
                        self.hurt(ctx, id, n);
                    }
                    return;
                }
                if to.kind == Kind::AnyTarget || to.whose == Whose::Theirs || to.whose == Whose::Any
                {
                    match ai::damage_target(self, ctx, to, n) {
                        ai::Hit::Player(o) => self.hurt_player(ctx, o, n),
                        ai::Hit::Perm(id) => {
                            let t1 = Target { count: 1, ..*to };
                            // Go through `choose` for ward/protection/targeted triggers.
                            if self
                                .candidates(ctx, &t1, true)
                                .iter()
                                .any(|&i| self.perms[i].id == id)
                            {
                                self.emit_targeted(id);
                                self.hurt(ctx, id, n);
                            }
                        }
                        ai::Hit::None => {}
                    }
                } else {
                    for id in self.choose(ctx, to, false) {
                        self.hurt(ctx, id, n);
                    }
                }
            }
            Effect::Fight(t) => {
                let mine = self
                    .src_idx(ctx)
                    .filter(|&i| self.perms[i].is_creature())
                    .or_else(|| ai::best_fighter(self, me));
                let Some(mi) = mine else { return };
                let my_id = self.perms[mi].id;
                let targets = self.choose(ctx, t, true);
                for id in targets {
                    let (Some(a), Some(b)) = (self.idx(my_id), self.idx(id)) else {
                        continue;
                    };
                    let (pa, da) = (self.perms[a].p, self.perms[a].has(DEATHTOUCH));
                    let (pb, db) = (self.perms[b].p, self.perms[b].has(DEATHTOUCH));
                    self.damage_perm(b, pa, da);
                    if !text.contains("deals damage equal to its power") || text.contains("fight") {
                        if let Some(a) = self.idx(my_id) {
                            self.damage_perm(a, pb, db);
                        }
                    }
                }
            }
            Effect::Shrink {
                amount,
                to,
                until_eot,
            } => {
                let n = self.x_value(ctx, amount) as i16;
                let harmful = to.whose != Whose::Yours;
                for id in self.choose(ctx, to, harmful) {
                    if let Some(i) = self.idx(id) {
                        if *until_eot {
                            self.perms[i].temp_p -= n;
                            self.perms[i].temp_t -= n;
                        } else {
                            self.perms[i].counters -= n;
                        }
                    }
                }
            }
            Effect::Bounce(t) => {
                let harmful = t.whose != Whose::Yours;
                for id in self.choose(ctx, t, harmful) {
                    if let Some(i) = self.idx(id) {
                        self.leave(i, Zone::Hand, false);
                    }
                }
            }
            Effect::Lock { target, lasting } => {
                // Auras and Equipment do this through their static ability.
                if text.starts_with("enchanted") || text.starts_with("equipped") {
                    return;
                }
                for id in self.choose(ctx, target, true) {
                    let Some(i) = self.idx(id) else { continue };
                    let x = &mut self.perms[i];
                    if *lasting {
                        x.lock_effect = true;
                    } else {
                        x.tapped = true;
                        if text.contains("doesn't untap") || text.contains("don't untap") {
                            x.stun = x.stun.max(1);
                        }
                    }
                }
            }
            Effect::Edict { each_opponent } => {
                let victims: Vec<usize> = if *each_opponent {
                    self.opponents(me)
                } else {
                    ai::choose_opponent(self, me, 0).into_iter().collect()
                };
                for o in victims {
                    let types = if text.contains("creature or planeswalker") {
                        T_CREATURE | T_PLANESWALKER
                    } else if text.contains("nonland permanent") || text.contains("a permanent") {
                        0xFFFF & !T_LAND
                    } else {
                        T_CREATURE
                    };
                    if let Some(i) = ai::sacrifice_choice(self, o, types) {
                        self.sacrifice(i);
                    }
                }
            }
            // Countering happens when the spell is cast (see `ai::respond_to_spell`).
            Effect::CounterSpell => {}
            Effect::Draw { amount, you } => {
                let n = self.x_value(ctx, amount).max(0) as u32;
                if *you || !text.contains("target player") && !text.contains("each player") {
                    self.draw(me, n);
                } else if text.contains("each player") {
                    for p in 0..self.players.len() {
                        if self.players[p].alive {
                            self.draw(p, n);
                        }
                    }
                } else {
                    self.draw(me, n);
                }
            }
            Effect::Loot => {
                self.draw(me, 1);
                let before = self.players[me].graveyard.len();
                self.discard(me);
                // Recruit: a nonland card discarded makes a 1/1 Human Soldier.
                if text.contains("recruit") {
                    if let Some(&c) = self.players[me].graveyard.get(before) {
                        if !self.players[me].cards[c as usize].is(T_LAND) {
                            let i = self.make_token(
                                me,
                                1,
                                1,
                                0,
                                vec!["human".into(), "soldier".into()],
                            );
                            self.after_enter(i);
                        }
                    }
                }
            }
            Effect::Discard { opponents } => {
                if *opponents {
                    let victims: Vec<usize> =
                        if text.contains("each opponent") || text.contains("each player") {
                            self.opponents(me)
                        } else {
                            ai::choose_opponent(self, me, 0).into_iter().collect()
                        };
                    for o in victims {
                        if text.contains("reveal") || text.contains("you choose") {
                            // You pick their best nonland card.
                            if let Some(h) = ai::best_card_in_hand(self, o) {
                                let c = self.players[o].hand.remove(h);
                                self.players[o].graveyard.push(c);
                                self.emit(Ev::Discard, o, None);
                            }
                        } else {
                            self.discard(o);
                        }
                    }
                } else {
                    self.discard(me);
                }
            }
            Effect::Mill { amount } => {
                let n = self.x_value(ctx, amount);
                let theirs = text.contains("opponent")
                    || text.contains("target player")
                    || text.contains("each player");
                if theirs {
                    let victims: Vec<usize> = if text.contains("each") {
                        self.opponents(me)
                    } else {
                        ai::choose_opponent(self, me, 0).into_iter().collect()
                    };
                    for o in victims {
                        self.mill(o, n);
                    }
                } else {
                    self.mill(me, n);
                }
            }
            Effect::Scry(n) => {
                let k = self.x_value(ctx, n).max(1) as usize;
                ai::scry(self, me, k, text.contains("surveil"));
            }
            Effect::Tokens {
                amount,
                power,
                toughness,
                keywords,
            } => {
                let mut n = self.x_value(ctx, amount).max(0);
                if self.doubles(me, "token") {
                    n *= 2;
                }
                let kw = keyword_bits(keywords);
                let subs = token_subtypes(text);
                for _ in 0..n.min(20) {
                    let i = self.make_token(me, *power, *toughness, kw, subs.clone());
                    if text.contains("tapped and attacking")
                        || text.contains("tapped")
                            && text.contains("token")
                            && text.contains("enters tapped")
                    {
                        self.perms[i].tapped = true;
                    }
                    self.after_enter(i);
                }
            }
            Effect::ItemTokens { amount, kind } => {
                let mut n = self.x_value(ctx, amount).max(0);
                if self.doubles(me, "token") {
                    n *= 2;
                }
                // "Gift a Treasure": the opponent gets the gift; its bonus is checked as a condition.
                let to = if text.contains("gift") {
                    ai::choose_opponent(self, me, 0).unwrap_or(me)
                } else {
                    me
                };
                for _ in 0..n.min(10) {
                    self.make_item(to, kind);
                }
            }
            Effect::PlusCounters { amount, to } => {
                let n = self.x_value(ctx, amount);
                if to.mass {
                    for id in self.choose(ctx, to, false) {
                        if let Some(i) = self.idx(id) {
                            self.add_counters(i, n);
                        }
                    }
                } else {
                    let ids = if to.kind == Kind::It {
                        self.src_idx(ctx)
                            .map(|i| self.perms[i].id)
                            .into_iter()
                            .collect()
                    } else {
                        self.choose(ctx, to, false)
                    };
                    for id in ids {
                        if let Some(i) = self.idx(id) {
                            self.add_counters(i, n);
                        }
                    }
                }
            }
            Effect::Pump {
                power,
                toughness,
                to,
                until_eot,
            } => {
                let harmful = *power < 0 && to.whose != Whose::Yours;
                let ids = if to.kind == Kind::It {
                    self.src_idx(ctx)
                        .map(|i| self.perms[i].id)
                        .into_iter()
                        .collect()
                } else {
                    self.choose(ctx, to, harmful)
                };
                for id in ids {
                    let Some(i) = self.idx(id) else { continue };
                    if *until_eot || ctx.until_eot {
                        self.perms[i].temp_p += *power as i16;
                        self.perms[i].temp_t += *toughness as i16;
                    } else {
                        // A one-off lasting buff: like counters.
                        self.perms[i].base_p += *power as i16;
                        self.perms[i].base_t += *toughness as i16;
                    }
                }
            }
            Effect::Grant {
                keywords,
                to,
                until_eot,
            } => {
                let bits = keyword_bits(keywords);
                let ids = if to.kind == Kind::It {
                    self.src_idx(ctx)
                        .map(|i| self.perms[i].id)
                        .into_iter()
                        .collect()
                } else {
                    self.choose(ctx, to, false)
                };
                for id in ids {
                    let Some(i) = self.idx(id) else { continue };
                    if *until_eot || ctx.until_eot {
                        self.perms[i].temp_kw |= bits;
                    } else {
                        self.perms[i].kw |= bits;
                    }
                }
            }
            Effect::GainLife(n) => {
                let k = self.x_value(ctx, n);
                if text.contains("each player gains") {
                    for p in 0..self.players.len() {
                        self.gain_life(p, k);
                    }
                } else {
                    self.gain_life(me, k);
                }
            }
            Effect::Drain { amount, each } => {
                let n = self.x_value(ctx, amount);
                let victims: Vec<usize> = if *each {
                    self.opponents(me)
                } else {
                    ai::choose_opponent(self, me, n).into_iter().collect()
                };
                let mut total = 0;
                for o in victims {
                    if text.contains("damage") {
                        self.hurt_player(ctx, o, n);
                    } else {
                        self.lose_life(o, n);
                    }
                    total += n;
                }
                if text.contains("you gain")
                    && (text.contains("that much")
                        || text.contains("equal to")
                        || text.contains("life lost"))
                {
                    self.gain_life(me, total);
                }
            }
            Effect::AddMana(n) => {
                // Mana abilities of permanents are mana sources; this is one-off mana (rituals,
                // triggered "add" effects).
                let k = self.x_value(ctx, n).max(1);
                self.players[me].pool = self.players[me].pool.saturating_add(k as u8);
            }
            Effect::FetchLand { battlefield } => {
                let basic = text.contains("basic");
                self.fetch_land(
                    me,
                    *battlefield,
                    text.contains("tapped") || !text.contains("untapped"),
                    basic,
                );
            }
            Effect::Reanimate => {
                let any = text.contains("a graveyard")
                    || text.contains("any graveyard")
                    || text.contains("all graveyards");
                if let Some((o, k)) = ai::reanimate_choice(self, me, any) {
                    let c = self.players[o].graveyard.remove(k);
                    self.emit(Ev::LeavesGraveyard, o, None);
                    if let Some(i) = self.enter_card(o, c, me, false) {
                        if text.contains("tapped") {
                            self.perms[i].tapped = true;
                        }
                    }
                }
            }
            Effect::Regrowth => {
                let creature = text.contains("creature card");
                let pick = if text.contains("return that card") || text.contains("return it") {
                    // The card that just went there (e.g. the creature that died).
                    self.players[me].graveyard.len().checked_sub(1)
                } else {
                    ai::regrowth_choice(self, me, creature)
                };
                if let Some(k) = pick {
                    let c = self.players[me].graveyard.remove(k);
                    self.players[me].hand.push(c);
                    self.emit(Ev::LeavesGraveyard, me, None);
                }
            }
            Effect::Untap(t) => {
                let ids: Vec<u32> = if t.kind == Kind::It {
                    self.src_idx(ctx)
                        .map(|i| self.perms[i].id)
                        .into_iter()
                        .collect()
                } else {
                    let t2 = Target {
                        whose: Whose::Yours,
                        ..*t
                    };
                    let cands = self.candidates(ctx, &t2, false);
                    ai::untap_choice(self, me, &cands)
                        .map(|i| self.perms[i].id)
                        .into_iter()
                        .collect()
                };
                for id in ids {
                    if let Some(i) = self.idx(id) {
                        self.perms[i].tapped = false;
                    }
                }
            }
            Effect::GainControl(t) => {
                for id in self.choose(ctx, t, true) {
                    let Some(i) = self.idx(id) else { continue };
                    let old = self.perms[i].ctrl;
                    let x = &mut self.perms[i];
                    x.ctrl = me as u8;
                    if ctx.until_eot || text.contains("until end of turn") {
                        x.threaten_from = Some(old);
                        x.tapped = false;
                        x.temp_kw |= HASTE;
                    } else {
                        x.sick = true;
                    }
                }
            }
            Effect::Copy if text.contains("becomes a copy") => {
                // The permanent itself takes on the best target's size and keywords.
                let Some(s) = self.src_idx(ctx) else { return };
                let any = !text.contains("you control");
                if let Some(t) = ai::best_to_copy(self, me, any).filter(|&t| t != s) {
                    let (bp, bt, kw, types) = {
                        let x = &self.perms[t];
                        (x.base_p, x.base_t, x.kw, x.types)
                    };
                    let src = &mut self.perms[s];
                    src.base_p = bp;
                    src.base_t = bt;
                    src.kw |= kw;
                    src.types |= types & (T_CREATURE | T_ARTIFACT);
                }
            }
            Effect::Copy => {
                if ctx.card.is_some_and(|c| !c.is_permanent())
                    || text.contains("copy target") && text.contains("spell")
                {
                    // Copy the last instant or sorcery its controller cast.
                    if let Some(last) = self.players[me].last_spell.take() {
                        self.players[me].last_spell = Some(last);
                        let c = self.card(last.0);
                        let abilities = if last.1 {
                            c.back.as_ref().map(|b| &b.abilities)
                        } else {
                            Some(&c.abilities)
                        };
                        for (ai, a) in abilities.into_iter().flatten().enumerate() {
                            if a.trig == Trig::Spell && !a.effects.contains(&Effect::Copy) {
                                self.pending.push(Pending {
                                    ctrl: me as u8,
                                    source: None,
                                    card: last.0,
                                    back: last.1,
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
                    }
                } else if let Some(i) = ai::best_to_copy(
                    self,
                    me,
                    text.contains("target creature") && !text.contains("you control"),
                ) {
                    let x = self.perms[i].clone();
                    if let Some(r) = x.card {
                        let t = self.new_perm(me, me, Some(r), Tok::Other);
                        self.perms[t].base_p = x.base_p;
                        self.perms[t].base_t = x.base_t;
                        let id = self.perms[t].id;
                        self.after_enter(t);
                        // Its enters abilities trigger too.
                        for (ai, a) in self.card(r).abilities.iter().enumerate() {
                            if a.trig == Trig::Enters {
                                self.pending.push(Pending {
                                    ctrl: me as u8,
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
                    }
                }
            }
            Effect::Empower(n) => {
                let k = self.x_value(ctx, n) as i16;
                let jace = self
                    .perms
                    .iter()
                    .position(|x| x.ctrl as usize == me && x.tok == Tok::Jace);
                let i = match jace {
                    Some(i) => i,
                    None => {
                        let i = self.new_perm(me, me, None, Tok::Jace);
                        self.perms[i].sick = false;
                        self.after_enter(i);
                        self.perms
                            .iter()
                            .position(|x| x.ctrl as usize == me && x.tok == Tok::Jace)
                            .unwrap_or(i)
                    }
                };
                self.perms[i].loyalty += k;
            }
            Effect::Amass(n) => {
                let k = self.x_value(ctx, n);
                let army = self
                    .perms
                    .iter()
                    .position(|x| x.ctrl as usize == me && x.tok == Tok::Army);
                let i = match army {
                    Some(i) => i,
                    None => {
                        let i = self.new_perm(me, me, None, Tok::Army);
                        let id = self.perms[i].id;
                        self.after_enter(i);
                        self.idx(id).unwrap_or(0)
                    }
                };
                if i < self.perms.len() {
                    self.add_counters(i, k);
                }
            }
            Effect::Goad => {
                let t = Target {
                    kind: Kind::Creature,
                    whose: Whose::Theirs,
                    mass: text.contains("each creature"),
                    other: false,
                    restricted: false,
                    count: 1,
                };
                for id in self.choose(ctx, &t, true) {
                    if let Some(i) = self.idx(id) {
                        self.perms[i].goad = Some(me as u8);
                    }
                }
            }
            Effect::Proliferate => {
                // By id: plan counters can fire abilities that change the board.
                let ids: Vec<u32> = self
                    .perms
                    .iter()
                    .filter(|x| x.ctrl as usize == me)
                    .map(|x| x.id)
                    .collect();
                for id in ids {
                    let Some(i) = self.idx(id) else { continue };
                    let x = &mut self.perms[i];
                    if x.counters > 0 {
                        x.counters += 1;
                    }
                    if x.is(T_PLANESWALKER) && x.loyalty > 0 {
                        x.loyalty += 1;
                    }
                    if x.other_counters > 0 && !x.is(T_SAGA) {
                        self.add_other_counters(i, 1);
                    }
                }
                for o in self.opponents(me) {
                    if self.players[o].poison > 0 {
                        self.players[o].poison += 1;
                    }
                }
            }
            Effect::ExtraCombat => {
                if self.active == me {
                    self.players[me].extra_combats = (self.players[me].extra_combats + 1).min(3);
                }
            }
            Effect::Blink => {
                let target = if text.contains("~") && !text.contains("target") {
                    self.src_idx(ctx)
                } else {
                    ai::blink_choice(self, me)
                };
                let Some(i) = target else { return };
                let x = self.perms[i].clone();
                let (Some(r), Tok::None) = (x.card, x.tok) else {
                    self.leave(i, Zone::Exile, false);
                    return;
                };
                if x.commander > 0 {
                    return;
                }
                self.leave(i, Zone::Exile, false);
                let owner = x.owner as usize;
                if let Some(k) = self.players[owner].exile.iter().rposition(|&c| c == r.1) {
                    self.players[owner].exile.remove(k);
                    if let Some(j) = self.enter_card(owner, r.1, x.ctrl as usize, false) {
                        self.perms[j].sick = x.sick;
                    }
                }
            }
            Effect::Tutor => {
                if let Some(k) = ai::tutor_choice(self, me, text) {
                    let c = self.players[me].library.remove(k);
                    let mut lib = std::mem::take(&mut self.players[me].library);
                    self.rng.shuffle(&mut lib);
                    self.players[me].library = lib;
                    if text.contains("onto the battlefield")
                        && self.players[me].cards[c as usize].is_permanent()
                    {
                        self.enter_card(me, c, me, false);
                    } else if text.contains("on top") {
                        self.players[me].library.push(c);
                    } else {
                        self.players[me].hand.push(c);
                    }
                }
            }
            Effect::FreeSpell => {
                let limit = text
                    .split("mana value ")
                    .nth(1)
                    .and_then(|r| r.split_whitespace().next())
                    .and_then(|w| w.parse::<u8>().ok())
                    .unwrap_or(4);
                if text.contains("top card") || text.contains("exile the top") {
                    // Impulse: reveal the top card and cast it if it's a spell.
                    if let Some(&c) = self.players[me].library.last() {
                        let sc = &self.players[me].cards[c as usize];
                        if !sc.is(T_LAND) {
                            self.players[me].library.pop();
                            self.players[me].hand.push(c);
                            let h = self.players[me].hand.len() - 1;
                            if sc.mv <= limit.max(6) {
                                self.cast(
                                    me,
                                    super::Src::Hand(h),
                                    super::CastOpts {
                                        free: true,
                                        ..Default::default()
                                    },
                                );
                            }
                        }
                    }
                } else if let Some(h) = ai::free_spell_choice(self, me, limit) {
                    self.cast(
                        me,
                        super::Src::Hand(h),
                        super::CastOpts {
                            free: true,
                            ..Default::default()
                        },
                    );
                }
            }
            Effect::Prepared => {
                if let Some(i) = self.src_idx(ctx) {
                    self.perms[i].prepared = true;
                } else if let Some(i) = ai::best_preparable(self, me) {
                    self.perms[i].prepared = true;
                }
            }
            Effect::Attach => {
                let src = self.src_idx(ctx);
                let equipment = src
                    .filter(|&i| self.perms[i].is(T_EQUIPMENT | T_AURA))
                    .or_else(|| ai::unattached_equipment(self, me));
                if let Some(e) = equipment {
                    let eid = self.perms[e].id;
                    if let Some(c) = ai::equip_target(self, me, eid) {
                        let cid = self.perms[c].id;
                        if let Some(e) = self.idx(eid) {
                            self.perms[e].attached = Some(cid);
                        }
                    }
                }
            }
            // Doublers work while the permanent is in play (`doubles`).
            Effect::Doubler(_) => {}
            Effect::Fog => self.fog = true,
            Effect::Dig => {
                let n = text
                    .split("top ")
                    .nth(1)
                    .and_then(|r| r.split_whitespace().next())
                    .and_then(|w| match w {
                        "two" => Some(2),
                        "three" => Some(3),
                        "four" => Some(4),
                        "five" => Some(5),
                        "six" => Some(6),
                        "seven" => Some(7),
                        _ => w.parse().ok(),
                    })
                    .unwrap_or(3usize);
                ai::dig(self, me, n, text.contains("land"));
            }
            Effect::OtherCounters(kind) => {
                let target_self = !text.contains("target");
                let i = if target_self {
                    self.src_idx(ctx)
                } else {
                    let t = Target {
                        kind: Kind::Creature,
                        whose: if kind == "stun" {
                            Whose::Theirs
                        } else {
                            Whose::Yours
                        },
                        mass: false,
                        other: false,
                        restricted: false,
                        count: 1,
                    };
                    let harmful = kind == "stun";
                    self.choose(ctx, &t, harmful)
                        .first()
                        .and_then(|&id| self.idx(id))
                };
                let Some(i) = i else { return };
                match kind.as_str() {
                    "loyalty" => self.perms[i].loyalty += 1,
                    "shield" => self.perms[i].shield += 1,
                    "stun" => self.perms[i].stun += 1,
                    k if KEYWORDS.iter().any(|(w, _)| *w == k) => {
                        self.perms[i].kw |= keyword_bits(&[k.to_string()]);
                    }
                    _ => self.add_other_counters(i, 1),
                }
            }
            Effect::Recast => {
                let pick = if text.contains("~") && ctx.card.is_some() {
                    None
                } else {
                    ai::recast_choice(self, me)
                };
                if let Some(k) = pick {
                    let c = self.players[me].graveyard[k];
                    if !self.players[me].recast.contains(&c) {
                        self.players[me].recast.push(c);
                    }
                }
            }
            Effect::Rule(label) => self.apply_rule(ctx, label),
        }
    }

    pub fn emit_targeted(&mut self, id: u32) {
        if let Some(i) = self.idx(id) {
            let snap = self.perms[i].clone();
            self.emit_about(Ev::Targeted, snap.ctrl as usize, &snap);
        }
    }

    /// The special rules (`Effect::Rule`) that change a simplified game; the rest are
    /// named on their cards but don't move the simulation (see `RULES_MODELLED`).
    fn apply_rule(&mut self, ctx: &Ctx, label: &str) {
        let me = ctx.ctrl;
        let text = ctx.text;
        match label {
            "you control an opponent's next turn" => {
                if let Some(o) = ai::choose_opponent(self, me, 0) {
                    self.players[o].hijacked = true;
                }
            }
            "gives poison counters" => {
                if let Some(o) = ai::choose_opponent(self, me, 0) {
                    self.players[o].poison = self.players[o].poison.saturating_add(1);
                }
            }
            "returns itself to hand" => {
                if let Some(i) = self.src_idx(ctx) {
                    self.leave(i, Zone::Hand, false);
                }
            }
            "weakens a creature's attack" | "weakens attackers" => {
                let n: i16 = text
                    .split("gets -")
                    .nth(1)
                    .and_then(|r| r.split('/').next())
                    .and_then(|d| d.parse().ok())
                    .unwrap_or(2);
                let t = Target {
                    kind: Kind::Creature,
                    whose: Whose::Theirs,
                    mass: label == "weakens attackers",
                    other: false,
                    restricted: false,
                    count: 1,
                };
                for id in self.choose(ctx, &t, true) {
                    if let Some(i) = self.idx(id) {
                        self.perms[i].temp_p -= n;
                    }
                }
            }
            "heals himself" => {
                if let Some(i) = self.src_idx(ctx) {
                    self.perms[i].damage = 0;
                }
            }
            "returns the exiled card" | "returns an exiled card" => {
                if let Some(c) = self.players[me].exile.pop() {
                    self.players[me].hand.push(c);
                }
            }
            "recycles a graveyard card" => {
                if let Some(c) = self.players[me].graveyard.pop() {
                    self.players[me].library.insert(0, c);
                }
            }
            "removes counters" => {
                if let Some(i) = ai::choose_victims(
                    self,
                    me,
                    &self.candidates(
                        ctx,
                        &Target {
                            kind: Kind::Permanent,
                            whose: Whose::Theirs,
                            mass: false,
                            other: false,
                            restricted: false,
                            count: 1,
                        },
                        true,
                    ),
                    1,
                )
                .first()
                .copied()
                {
                    let x = &mut self.perms[i];
                    x.counters = x.counters.min(0);
                    x.loyalty = (x.loyalty - 2).max(0);
                }
            }
            "gives a permanent away" => {
                // Gives away its worst permanent to an opponent.
                if let (Some(i), Some(o)) = (
                    ai::sacrifice_choice(self, me, 0xFFFF & !T_LAND),
                    ai::choose_opponent(self, me, 0),
                ) {
                    self.perms[i].ctrl = o as u8;
                }
            }
            "turns a permanent into a creature" | "sets base power/toughness" => {
                if let Some(a) = self
                    .src_perm(ctx)
                    .and_then(|s| s.attached)
                    .and_then(|id| self.idx(id))
                {
                    let x = &mut self.perms[a];
                    x.types |= T_CREATURE;
                    if x.base_t <= 0 {
                        x.base_p = 4;
                        x.base_t = 4;
                    }
                }
            }
            "adds to noncombat damage" => {}
            "everyone draws an extra card" => {
                for p in 0..self.players.len() {
                    if self.players[p].alive {
                        self.draw(p, 1);
                    }
                }
            }
            "exiles itself" => {
                if let Some(i) = self.src_idx(ctx) {
                    self.leave(i, Zone::Exile, false);
                }
            }
            "mills opponents" => {
                for o in self.opponents(me) {
                    self.mill(o, 3);
                }
            }
            "reshuffles the graveyard" => {
                let gy = std::mem::take(&mut self.players[me].graveyard);
                self.players[me].library.extend(gy);
                let mut lib = std::mem::take(&mut self.players[me].library);
                self.rng.shuffle(&mut lib);
                self.players[me].library = lib;
            }
            "returns to the top of the library" => {
                if let Some(i) = self.src_idx(ctx) {
                    self.leave(i, Zone::Library(true), false);
                }
            }
            "untaps your creatures" => {
                for x in self
                    .perms
                    .iter_mut()
                    .filter(|x| x.ctrl as usize == me && x.is_creature())
                {
                    x.tapped = false;
                }
            }
            "steals the enchanted permanent" => {
                if let Some(a) = self
                    .src_perm(ctx)
                    .and_then(|s| s.attached)
                    .and_then(|id| self.idx(id))
                {
                    self.perms[a].ctrl = me as u8;
                }
            }
            // A wish: close enough to drawing the right card.
            "fetches a card from outside the game" => self.draw(me, 1),
            // "can't stay in a graveyard", "exiles cards instead of the graveyard": the cards
            // simply don't come back; the simulator's recursion only uses what's left there.
            "can't stay in a graveyard" | "exiles cards instead of the graveyard" => {}
            "stops all life gain" => {} // static, checked in `gain_life`
            _ => {}
        }
    }
}

fn token_subtypes(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(i) = text.find("creature token") {
        let before: Vec<&str> = text[..i].split_whitespace().collect();
        for w in before.iter().rev().take(4) {
            let w = w.trim_matches(|c: char| !c.is_alphabetic());
            if [
                "white",
                "blue",
                "black",
                "red",
                "green",
                "colorless",
                "artifact",
                "legendary",
                "a",
                "an",
                "two",
                "three",
                "x",
            ]
            .contains(&w)
                || w.contains('/')
                || w.is_empty()
            {
                break;
            }
            out.push(w.to_string());
        }
    }
    out
}
