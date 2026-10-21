//! Resolving abilities: every effect the ability parser reads (`abilities::Effect`) does
//! its thing here, with targets picked by the AI (`ai`).

use super::ai;
use super::compile::*;
use super::state::*;
use crate::abilities::{Amount, Effect, Kind, Target, Whose};
use crate::restrict;

/// Where an ability is resolving from.
#[derive(Clone)]
pub struct Ctx<'a> {
    pub ctrl: usize,
    /// The source permanent (it may have left the battlefield since).
    pub src: Option<u32>,
    /// Last known information about the source.
    pub snap: Option<Perm>,
    pub card: Option<&'a SimCard>,
    /// Which card that is (to find it in its owner's graveyard).
    pub card_ref: Option<CardRef>,
    /// What the triggering event was about (the creature that died, entered...).
    pub subject: Option<u32>,
    pub text: &'a str,
    pub until_eot: bool,
    /// Cast with overload: "target" means "each" (of the opponents' things).
    pub overload: bool,
    /// What a returned, found or sacrificed card must be (`SimAbility::card_restr`).
    pub card_restr: crate::restrict::Restr,
    /// The X paid for the spell or ability.
    pub x: Option<i32>,
    /// What was sacrificed to pay for it ("damage equal to that creature's power").
    pub paid: Option<Perm>,
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
    "sacrifices itself",
    "taps itself",
    "you lose the game",
    "discards your hand",
    "each player discards their hand",
    "you lose life equal to it",
    "its controller gains life equal to its power",
    "doubles counters",
    "exiles a graveyard",
    "exiles a graveyard card",
    "extra land drop",
    "mills opponents",
    "reshuffles the graveyard",
    "returns to the top of the library",
    "untaps your creatures",
    "steals the enchanted permanent",
    "fetches a card from outside the game",
    "damage to you",
    "transforms",
    "extra turn",
    "ends the turn",
    "can't block this turn",
    "shuffles itself into its library",
    "gives itself to an opponent",
    "assigns combat damage by toughness",
    "returns when it dies",
    "stashes the discarded card",
    "an opponent creates a creature token",
    "you get an emblem",
    "gets +N/+N until your next turn",
    "becomes a creature",
    // and "you lose N life" (see `rule_modelled`)
];

/// Is this `Effect::Rule` label one the simulator acts on?
pub fn rule_modelled(label: &str) -> bool {
    RULES_MODELLED.contains(&label) || label.starts_with("you lose ") && label.ends_with(" life")
}

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
        self.doubles_from(p, kind, None)
    }

    /// The same, for damage from source `src`: "double all damage equipped creature would
    /// deal" (Mjolnir) only doubles what the equipped creature deals.
    pub fn doubles_from(&self, p: usize, kind: &str, src: Option<u32>) -> bool {
        self.perms.iter().any(|x| {
            x.ctrl as usize == p
                && self.abilities_of(x).iter().any(|a| {
                    a.effects
                        .iter()
                        .any(|e| matches!(e, Effect::Doubler(k) if k.contains(kind)))
                        && (!a.text.contains("equipped creature")
                            || src.is_some() && x.attached == src)
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
        // The sacrificed creature: "damage equal to that creature's power".
        if let Some(s) = &ctx.paid {
            if t.contains("that creature's power") || t.contains("sacrificed creature's power") {
                return s.p.max(0) as i32;
            }
        }
        // "Draw a card for each graveyard with seven or more cards in it".
        if t.contains("for each graveyard with") {
            let need = t
                .split("for each graveyard with ")
                .nth(1)
                .and_then(|r| r.split_whitespace().next())
                .and_then(|w| match w {
                    "seven" => Some(7),
                    "ten" => Some(10),
                    d => d.parse().ok(),
                })
                .unwrap_or(7);
            return self
                .players
                .iter()
                .filter(|p| p.alive && p.graveyard.len() >= need)
                .count() as i32;
        }
        // X paid for the spell or ability ("draw X cards").
        if let Some(x) = ctx.x {
            if !t.contains("where x is") && !t.contains("for each") {
                return x.max(0);
            }
        }
        let subject_or_src = |f: &dyn Fn(&Perm) -> i32| -> Option<i32> {
            ctx.subject
                .and_then(|id| self.idx(id))
                .map(|i| f(&self.perms[i]))
                .or_else(|| self.src_perm(ctx).map(|x| f(&x)))
        };
        // What X is: "where X is ...", "equal to ...", or a count "for each ...".
        let measure = t
            .split("where x is ")
            .nth(1)
            .or_else(|| t.split(" x is ").nth(1))
            .or_else(|| t.split(" equal to ").nth(1));
        if let Some(m) = measure {
            let m = m.trim_start_matches("the ");
            let n = if m.starts_with("number of ") {
                self.count_of(me, ctx.src, m)
            } else if m.starts_with("your life total") {
                Some(self.players[me].life)
            } else if m.starts_with("amount of life you gained this turn") {
                Some(self.players[me].life_gained_turn)
            } else if m.starts_with("amount of mana spent to cast that spell") {
                self.casting.map(|(r, _)| self.card(r).mv as i32)
            } else if m.starts_with("difference") {
                // "If you have fewer than ten cards in hand, draw cards equal to the difference".
                Some((10 - self.players[me].hand.len() as i32).max(0))
            } else if m.starts_with("sacrificed creature's power") {
                self.last_left.as_ref().map(|x| x.p as i32)
            } else if m.starts_with("sacrificed creature's toughness") {
                self.last_left.as_ref().map(|x| x.t as i32)
            } else if m.starts_with("greatest power among") {
                let other = m.contains("other");
                Some(
                    self.perms
                        .iter()
                        .filter(|x| x.ctrl as usize == me && x.is_creature())
                        .filter(|x| !(other && Some(x.id) == ctx.src))
                        .map(|x| x.p as i32)
                        .max()
                        .unwrap_or(0),
                )
            } else if m.starts_with("greatest mana value among") {
                Some(
                    self.perms
                        .iter()
                        .filter(|x| x.ctrl as usize == me && x.card.is_some())
                        .filter(|x| type_ok(x, m))
                        .map(|x| self.card(x.card.unwrap()).mv as i32)
                        .max()
                        .unwrap_or(0),
                )
            } else if m.starts_with("~'s power") || m.starts_with("his power") {
                self.src_perm(ctx).map(|x| x.p as i32)
            } else if m.starts_with("~'s toughness") || m.starts_with("his toughness") {
                self.src_perm(ctx).map(|x| x.t as i32)
            } else if m.starts_with("its toughness") || m.starts_with("that creature's toughness") {
                subject_or_src(&|x| x.t as i32)
            } else if m.starts_with("its power")
                || m.starts_with("that creature's power")
                || m.starts_with("the power")
                || m.starts_with("power of")
            {
                subject_or_src(&|x| x.p as i32)
            } else {
                None
            };
            if let Some(n) = n {
                return n.max(0);
            }
        }
        if let Some(rest) = t.split("for each ").nth(1) {
            if let Some(k) = self.count_of(me, ctx.src, rest) {
                // The amount per thing counted ("+2/+0 for each", "a Treasure for each").
                let per = (a.n / 3).max(1);
                return (per * k).max(0);
            }
        }
        let after = t.split("number of").nth(1).unwrap_or("");
        let n = if !after.is_empty() {
            if let Some(k) = self.count_of(me, ctx.src, after.trim()) {
                k
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
            subject_or_src(&|x| x.p as i32).unwrap_or(a.n)
        } else if t.contains("mana value") {
            ctx.subject
                .and_then(|id| self.idx(id))
                .and_then(|i| self.perms[i].card)
                .map(|r| self.card(r).mv as i32)
                .unwrap_or(a.n)
        } else if let Some(x) = self.cur_x.filter(|_| has_x(t)) {
            // The X paid for the spell.
            x
        } else {
            a.n
        };
        n.max(0)
    }

    /// Counts what a "for each ..." / "the number of ..." phrase names, for player `me`
    /// (`src`: the permanent the text belongs to). None if the phrase isn't understood.
    pub fn count_of(&self, me: usize, src: Option<u32>, phrase: &str) -> Option<i32> {
        let p = phrase
            .split(['.', ',', ';', '"', '('])
            .next()
            .unwrap_or("")
            .trim()
            .trim_start_matches("the ")
            .trim_start_matches("number of ")
            .trim();
        let p = p.split(" plus ").next().unwrap_or(p).trim();
        let p = p.split(" and its ").next().unwrap_or(p).trim();
        let pl = &self.players[me];
        let alive = |o: usize| self.players[o].alive;
        // Card types named as whole words ("an Island" isn't a land word).
        let type_words = |w: &str| -> u16 {
            let mut bits = 0;
            for word in w.split(|c: char| !c.is_alphanumeric()) {
                let word = word.strip_suffix('s').unwrap_or(word);
                bits |= match word {
                    "creature" => T_CREATURE,
                    "artifact" => T_ARTIFACT,
                    "enchantment" => T_ENCHANTMENT,
                    "planeswalker" => T_PLANESWALKER,
                    "land" => T_LAND,
                    "instant" => T_INSTANT,
                    "sorcery" | "sorcerie" => T_SORCERY,
                    "permanent" => !(T_INSTANT | T_SORCERY),
                    _ => 0,
                };
            }
            bits
        };
        let cards_matching = |cards: &[u16], list: &[SimCard], types: u16| -> i32 {
            cards
                .iter()
                .filter(|&&c| types == 0 || list[c as usize].types & types != 0)
                .count() as i32
        };
        // Zones other than the battlefield.
        if p.starts_with("opponent") {
            return Some(self.opponents(me).len() as i32);
        }
        if p.starts_with("graveyard with seven or more cards") {
            return Some(
                (0..self.players.len())
                    .filter(|&o| alive(o) && self.players[o].graveyard.len() >= 7)
                    .count() as i32,
            );
        }
        if p.contains("discarded this way") {
            return Some(self.last_discarded);
        }
        if p.contains("destroyed this way") {
            return Some(self.last_destroyed);
        }
        if p.contains("cards drawn this way") {
            return Some(self.last_drawn);
        }
        if p.contains("you've discarded this turn") || p.contains("you discarded this turn") {
            return Some(pl.discarded_turn as i32);
        }
        if p.starts_with("player") {
            return Some((0..self.players.len()).filter(|&o| alive(o)).count() as i32);
        }
        if p.contains("card type") && p.contains("graveyard") {
            let mut seen = 0u16;
            for o in (0..self.players.len()).filter(|&o| alive(o)) {
                for &c in &self.players[o].graveyard {
                    seen |= self.players[o].cards[c as usize].types
                        & (T_CREATURE
                            | T_ARTIFACT
                            | T_ENCHANTMENT
                            | T_PLANESWALKER
                            | T_LAND
                            | T_INSTANT
                            | T_SORCERY);
                }
            }
            return Some(seen.count_ones() as i32);
        }
        if p.contains("in your hand") {
            return Some(cards_matching(&pl.hand, pl.cards, type_words(p)));
        }
        if p.contains("in your graveyard") {
            return Some(cards_matching(&pl.graveyard, pl.cards, type_words(p)));
        }
        if p.contains("opponents' graveyards") || p.contains("all graveyards") {
            let all = p.contains("all graveyards");
            return Some(
                (0..self.players.len())
                    .filter(|&o| alive(o) && (all || o != me))
                    .map(|o| {
                        let q = &self.players[o];
                        cards_matching(&q.graveyard, q.cards, type_words(p))
                    })
                    .sum(),
            );
        }
        let src_perm = src.and_then(|id| self.idx(id)).map(|i| &self.perms[i]);
        if p.contains("equipment attached") || p.contains("aura and equipment attached") {
            let id = src_perm.and_then(|s| s.attached.filter(|_| s.is(T_EQUIPMENT | T_AURA)));
            let host = id.or(src);
            return Some(
                self.perms
                    .iter()
                    .filter(|x| x.is(T_EQUIPMENT) && x.attached.is_some() && x.attached == host)
                    .count() as i32,
            );
        }
        if p.contains("+1/+1 counter on ~") || p.contains("counter on it") {
            return Some(src_perm.map_or(0, |x| x.counters.max(0) as i32));
        }
        let mine = |x: &&Perm| x.ctrl as usize == me;
        if p.contains("basic land type") {
            let mut n = 0;
            for t in ["plains", "island", "swamp", "mountain", "forest"] {
                if self
                    .perms
                    .iter()
                    .filter(mine)
                    .any(|x| x.is(T_LAND) && x.subtypes.iter().any(|s| s == t))
                {
                    n += 1;
                }
            }
            return Some(n);
        }
        if p.contains("different power") {
            let mut seen: Vec<i16> = self
                .perms
                .iter()
                .filter(mine)
                .filter(|x| x.is_creature())
                .map(|x| x.p)
                .collect();
            seen.sort();
            seen.dedup();
            return Some(seen.len() as i32);
        }
        if p.contains("different mana value") {
            let mut seen: Vec<u8> = self
                .perms
                .iter()
                .filter(mine)
                .filter(|x| !x.is(T_LAND) && x.card.is_some())
                .map(|x| self.card(x.card.unwrap()).mv)
                .collect();
            seen.sort();
            seen.dedup();
            return Some(seen.len() as i32);
        }
        if p.starts_with("color among") {
            let colors = self
                .perms
                .iter()
                .filter(mine)
                .filter(|x| Some(x.id) != src && x.is(T_ARTIFACT))
                .fold(0u8, |m, x| m | x.colors);
            return Some((colors & 0b11111).count_ones() as i32);
        }
        // The battlefield: "[other] [legendary] <type or subtype> [you control | your
        // opponents control | on the battlefield] [with ...]".
        let side = if p.contains("your opponents control")
            || p.contains("opponents control")
            || p.contains("you don't control")
        {
            Some(false)
        } else if p.contains("you control") || p.starts_with("attacking ") {
            Some(true)
        } else if p.contains("on the battlefield") {
            None
        } else {
            return None;
        };
        let other = p.starts_with("other ") || p.starts_with("another ");
        let noun_part = p
            .split(" named ")
            .next()
            .unwrap_or(p)
            .split(" you control")
            .next()
            .unwrap_or(p)
            .split(" your opponents control")
            .next()
            .unwrap_or(p)
            .split(" opponents control")
            .next()
            .unwrap_or(p)
            .split(" on the battlefield")
            .next()
            .unwrap_or(p)
            .split(" you don't control")
            .next()
            .unwrap_or(p);
        let mut words: Vec<&str> = noun_part.split_whitespace().collect();
        words.retain(|w| {
            ![
                "other",
                "another",
                "attacking",
                "tapped",
                "untapped",
                "nontoken",
                "legendary",
                "basic",
                "a",
                "an",
            ]
            .contains(w)
        });
        // "Named ~": copies of this card.
        let named = p.contains(" named ~");
        let src_card = src_perm.and_then(|x| x.card);
        // "Of that type" / "of the chosen type": the type most of them share.
        if p.contains("of that type") || p.contains("of the chosen type") {
            let mine: Vec<&Perm> = self
                .perms
                .iter()
                .filter(|x| x.ctrl as usize == me && x.is_creature())
                .collect();
            let best = mine
                .iter()
                .flat_map(|x| x.subtypes.iter())
                .map(|s| mine.iter().filter(|x| x.subtypes.contains(s)).count())
                .max()
                .unwrap_or(0);
            return Some(best as i32);
        }
        let legendary = p.contains("legendary");
        let basic = p.contains("basic ");
        let attacking = p.contains("attacking");
        let with_counter = p.contains("with a +1/+1 counter");
        let types = type_words(&words.join(" "));
        // A creature type or a land type ("Forests", "Elves", "Gates", "Merfolk"), or a name.
        let sub: Option<String> = if types == 0 {
            let w = words.join(" ");
            let w = w.split(" with ").next().unwrap_or(&w).trim().to_string();
            Some(if w.ends_with("ves") {
                format!("{}f", &w[..w.len() - 3])
            } else if w.ends_with("ies") {
                format!("{}y", &w[..w.len() - 3])
            } else if w.ends_with('s') && !w.ends_with("ss") {
                w[..w.len() - 1].to_string()
            } else {
                w
            })
        } else {
            None
        };
        let n = self
            .perms
            .iter()
            .filter(|x| side.is_none_or(|s| (x.ctrl as usize == me) == s))
            .filter(|x| !(other && Some(x.id) == src))
            .filter(|x| {
                !named
                    || x.card.is_some()
                        && x.card.map(|r| &self.card(r).name)
                            == src_card.map(|r| &self.card(r).name)
            })
            .filter(|x| !legendary || x.is(T_LEGENDARY))
            .filter(|x| !basic || x.is(T_BASIC))
            .filter(|x| !attacking || x.attacking)
            .filter(|x| !with_counter || x.counters > 0)
            .filter(|x| match &sub {
                None => x.types & types != 0 || (types & T_CREATURE != 0 && x.is_creature()),
                Some(s) => {
                    x.subtypes.iter().any(|t| t == s)
                        || x.card
                            .is_some_and(|r| self.card(r).name.to_lowercase() == *s)
                }
            })
            .count() as i32;
        Some(n)
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
        let another = t.other || text.contains("another") && !t.mass;
        let r = &t.restr;
        let d = self.dyn_values(r, Some(ctx), me, src_id);
        // "... target player controls": one player's things (an opponent's, to harm).
        let one_player = (r.has(restrict::ONE_PLAYER) && t.mass).then(|| {
            if harmful {
                ai::choose_opponent(self, me, 0).unwrap_or(me)
            } else {
                me
            }
        });
        let mut out = Vec::new();
        for (i, x) in self.perms.iter().enumerate() {
            if t.kind == Kind::Artifact && text.contains("target equipment") && !x.is(T_EQUIPMENT) {
                continue;
            }
            // A list of card types ("artifact, enchantment, or creature with flying") decides
            // the kind by itself.
            let listed = r.any_types != 0
                && matches!(
                    t.kind,
                    Kind::Creature | Kind::Permanent | Kind::NonlandPermanent | Kind::Other
                );
            if listed {
                if !(x.is(r.any_types) || r.any_types & T_CREATURE != 0 && x.is_creature()) {
                    continue;
                }
            } else if !Self::kind_ok(x, t.kind) {
                // "Other" kinds that name a creature type ("target Hero")
                if !(t.kind == Kind::Other && x.is_creature()) {
                    continue;
                }
            }
            let mine = x.ctrl as usize == me;
            let side = match (one_player, t.whose) {
                (Some(o), _) => x.ctrl as usize == o,
                (None, Whose::Yours) => mine,
                (None, Whose::Theirs) => !mine,
                (None, Whose::Any) => {
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
            // "Put a +1/+1 counter on each other creature you control": not the source.
            let mass_other = t.other || text.contains("each other ") || text.contains("all other ");
            if Some(x.id) == src_id && (another || t.mass && mass_other) {
                continue;
            }
            // A spell's "create a Cadet, then put a counter on each other Wizard token":
            // other than what it just made.
            if t.other && src_id.is_none() && self.last_targets.contains(&x.id) {
                continue;
            }
            if !r.is_empty() {
                if !self.perm_ok(r, x, &d) {
                    continue;
                }
            } else if t.restricted && !t.mass {
                // A narrowing the reader doesn't model: spare the biggest threats.
                if x.p > 3 && x.mv > 3 {
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
                // "Hexproof from black", "hexproof from instants" (Knight of Grace, Elenda).
                if !mine
                    && (x.hexproof_from & src_colors & 0b11111 != 0
                        || x.hexproof_from & (1 << 5) != 0
                            && ctx.card.is_some_and(|c| c.is(T_INSTANT)))
                {
                    continue;
                }
                if !mine && !self.ward_payable(me, x) {
                    continue;
                }
            }
            out.push(i);
        }
        self.keep_greatest(r, &mut out);
        out
    }

    /// Can `me` pay the ward of permanent `x` (mana, life, a discard, sacrifices, poison),
    /// without paying too much for it?
    pub fn ward_payable(&self, me: usize, x: &Perm) -> bool {
        let w = x.card.map(|r| self.card(r).ward_cost).unwrap_or_default();
        let pl = &self.players[me];
        let mana_ok = x.ward == 0 || self.can_pay(me, &Mana::generic(x.ward as u32));
        let discard_ok = !pl.hand.is_empty();
        if w.discard_or_mana {
            return mana_ok || discard_ok;
        }
        let mine = self
            .perms
            .iter()
            .filter(|y| y.ctrl as usize == me && y.commander == 0)
            .count();
        mana_ok
            && (!w.discard || discard_ok)
            && (w.life == 0 || pl.life > w.life as i32 + 5)
            && (w.poison == 0 || (pl.poison as u32 + w.poison as u32) < 10)
            && (w.sacrifice == 0 || mine > w.sacrifice as usize + 2)
    }

    /// Pays the ward of permanent `i` targeted by `me`. False if it can't be paid (the
    /// spell or ability is countered).
    pub fn pay_ward(&mut self, me: usize, i: usize) -> bool {
        let x = &self.perms[i];
        let w = x.card.map(|r| self.card(r).ward_cost).unwrap_or_default();
        let mana = Mana::generic(x.ward as u32);
        if x.ward == 0 && w == WardCost::default() {
            return true;
        }
        if !self.ward_payable(me, x) {
            return false;
        }
        if w.discard_or_mana {
            if !self.pay(me, &mana) {
                self.discard(me);
            }
            return true;
        }
        if x.ward > 0 && !self.pay(me, &mana) {
            return false;
        }
        if w.life > 0 {
            self.lose_life(me, w.life as i32);
        }
        if w.discard {
            self.discard(me);
        }
        for _ in 0..w.sacrifice {
            if let Some(j) = ai::sacrifice_choice(self, me, 0xFFFF) {
                self.sacrifice(j);
            }
        }
        if w.poison > 0 {
            self.players[me].poison = self.players[me].poison.saturating_add(w.poison);
        }
        true
    }

    /// Picks targets and handles targeting side effects (ward is paid, "becomes the target"
    /// triggers, the owner may respond with protection). Returns perm ids still valid.
    fn choose(&mut self, ctx: &Ctx, t: &Target, harmful: bool) -> Vec<u32> {
        if let Some(ids) = self.prev_ids(ctx, t) {
            return ids;
        }
        // "Each of them" with nothing named before: nothing.
        if t.mass && t.restr.has(restrict::PREV) {
            return Vec::new();
        }
        let ids = self.choose_new(ctx, t, harmful);
        // "It" now means these (or nothing, if there was nothing to target).
        self.targeted = true;
        self.last_targets = ids.clone();
        ids
    }

    /// The source itself for "~", what "it" refers to, or new targets.
    fn pick(&mut self, ctx: &Ctx, t: &Target, harmful: bool) -> Vec<u32> {
        if let Some(ids) = self.prev_ids(ctx, t) {
            return ids;
        }
        if t.kind == Kind::It {
            return self
                .src_idx(ctx)
                .map(|i| self.perms[i].id)
                .into_iter()
                .collect();
        }
        self.choose(ctx, t, harmful)
    }

    fn choose_new(&mut self, ctx: &Ctx, t: &Target, harmful: bool) -> Vec<u32> {
        // "For each opponent, ... up to one target creature that player controls".
        if t.restr.has(restrict::PER_OPPONENT) && !t.mass {
            let one = Target {
                restr: restrict::Restr {
                    flags: t.restr.flags & !restrict::PER_OPPONENT,
                    ..t.restr
                },
                ..*t
            };
            let mut out = Vec::new();
            for o in self.opponents(ctx.ctrl) {
                let cands: Vec<usize> = self
                    .candidates(ctx, &one, harmful)
                    .into_iter()
                    .filter(|&i| self.perms[i].ctrl as usize == o)
                    .collect();
                let n = one.count.max(1) as usize;
                let picks = if harmful {
                    ai::choose_victims(self, ctx.ctrl, &cands, n)
                } else {
                    ai::choose_friends(self, ctx.ctrl, &cands, n)
                };
                for i in picks {
                    let id = self.perms[i].id;
                    self.actor = ctx.ctrl as u8;
                    self.emit_targeted(id);
                    out.push(id);
                }
            }
            self.last_ctrl = out
                .first()
                .and_then(|&id| self.idx(id))
                .map(|i| self.perms[i].ctrl);
            return out;
        }
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
        if let Some(&i) = picks.first() {
            self.last_ctrl = Some(self.perms[i].ctrl);
        }
        let ids: Vec<u32> = picks.iter().map(|&i| self.perms[i].id).collect();
        let mut out = Vec::new();
        for id in ids {
            let Some(i) = self.idx(id) else { continue };
            let owner = self.perms[i].ctrl as usize;
            if owner != ctx.ctrl {
                if !self.pay_ward(ctx.ctrl, i) {
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
                self.actor = ctx.ctrl as u8;
                self.emit_about(Ev::Targeted, snap.ctrl as usize, &snap);
            }
            out.push(id);
        }
        if !t.mass && !out.is_empty() {
            self.last_targets = out.clone();
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
        } else if pend.ability >= EMBLEM_BASE {
            let c = self.card(pend.card);
            match c.emblems.get((pend.ability - EMBLEM_BASE) as usize) {
                Some((_, a)) => std::slice::from_ref(a),
                None => return,
            }
        } else {
            let c = self.card(pend.card);
            // A transformed permanent's abilities are its back face's.
            let transformed = pend
                .source
                .and_then(|id| self.idx(id))
                .map(|i| &self.perms[i])
                .or(pend.snapshot.as_ref())
                .is_some_and(|x| x.transformed && x.card == Some(pend.card));
            if pend.back || transformed {
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
        let Some(ab) = abilities.get(if pend.ability >= EMBLEM_BASE {
            0
        } else {
            pend.ability as usize
        }) else {
            return;
        };
        let src_idx = pend.source.and_then(|id| self.idx(id));
        // The creature the event was about, as it is or as it last was (if it died).
        let subject = pend.subject.and_then(|id| {
            self.idx(id)
                .map(|i| self.perms[i].clone())
                .or_else(|| self.dead.iter().rev().find(|x| x.id == id).cloned())
        });
        let source = src_idx
            .map(|i| self.perms[i].clone())
            .or_else(|| pend.snapshot.clone());
        let eval = |c: &Cond| match c {
            Cond::Teamwork => pend.teamwork,
            Cond::Gift => pend.gift,
            Cond::CastFromGraveyard => pend.from_graveyard,
            Cond::Kicked => pend.kicked,
            Cond::XAtLeast(n) => pend.x.unwrap_or(0) >= *n,
            Cond::SubjectBigger => match (&subject, &source) {
                (Some(s), Some(it)) => s.p > it.p || s.t > it.t,
                _ => false,
            },
            Cond::SubjectWasAttacking => subject.as_ref().is_some_and(|s| s.attacking),
            Cond::SubjectHurtBySource => subject
                .as_ref()
                .is_some_and(|s| pend.source.is_some_and(|id| s.hurt_by == id)),
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
        let mut ctx = Ctx {
            ctrl: me,
            src: pend.source,
            snap: pend.snapshot.clone(),
            card,
            card_ref: (pend.card.1 != NO_CARD).then_some(pend.card),
            subject: pend.subject,
            text: &ab.text,
            until_eot: ab.until_eot,
            overload: pend.overloaded,
            card_restr: ab.card_restr,
            x: pend.x,
            paid: None,
        };
        // "Any opponent may sacrifice a creature. If a player does, tap ~ and ..."
        // (Desecration Demon): an opponent feeds it a spare creature to tap it down; if
        // nobody has one to spare, nothing happens and it attacks.
        if ab.text.contains("any opponent may sacrifice") {
            let payer = self
                .opponents(me)
                .into_iter()
                .find_map(|o| ai::fodder(self, o, T_CREATURE, u32::MAX));
            let Some(i) = payer else { return };
            self.sacrifice(i);
            if ab.text.contains("tap ~") {
                if let Some(s) = pend.source.and_then(|id| self.idx(id)) {
                    self.perms[s].tapped = true;
                }
            }
        }
        // A cost paid as it resolves ("you may sacrifice another creature. If you do, ..."):
        // the effects after it need the payment; the AI decides whether it's worth it.
        let mut paid_ok = true;
        if let Some(pc) = &ab.pay {
            let s = pend.source.and_then(|id| self.idx(id));
            let allowed = pc.cond.as_ref().is_none_or(|c| self.cond_holds(c, me, s));
            let want = allowed && (!pc.optional || ai::wants_to_pay(self, &ctx, ab, pc));
            // "You may pay {X}. When you do, put X +1/+1 counters on ~": the X it can spare.
            if let (true, ResCost::Mana(m)) = (want, &pc.cost) {
                if m.x {
                    let x = self.mana_available(me).saturating_sub(m.total()).min(6) as i32;
                    ctx.x = Some(x);
                }
            }
            let cost = match (&pc.cost, ctx.x) {
                (ResCost::Mana(m), Some(x)) if m.x => ResCost::Mana(m.plus_generic(x as u32)),
                (c, _) => c.clone(),
            };
            paid_ok = want
                && ctx.x != Some(0)
                && match self.pay_res_cost(&ctx, &cost) {
                    Some(snap) => {
                        ctx.paid = snap;
                        true
                    }
                    None => false,
                };
        } else if ab.optional && !ai::wants_optional(self, &ctx, ab) {
            return;
        }
        self.last_targets.clear();
        self.last_ctrl = None;
        self.targeted = false;
        // (effect, the text and card restriction it resolves with): a mode reads its own.
        let picks: Vec<(usize, &str, crate::restrict::Restr)> = if ab.modal {
            let groups = ab.mode_groups();
            // "Choose one that hasn't been chosen" (Demonic Pact): each mode once.
            let fresh = ab.text.contains("hasn't been chosen");
            let used = if fresh {
                pend.source
                    .and_then(|id| self.idx(id))
                    .map_or(0, |i| self.perms[i].modes_used)
            } else {
                0
            };
            let modes = ai::choose_modes(self, &ctx, ab, used);
            if fresh {
                if let Some(i) = pend.source.and_then(|id| self.idx(id)) {
                    for &m in &modes {
                        if m < 32 {
                            self.perms[i].modes_used |= 1 << m;
                        }
                    }
                }
            }
            modes
                .into_iter()
                .flat_map(|m| {
                    let (t, r) = ab.mode_text(m);
                    groups[m].clone().map(move |k| (k, t, r))
                })
                .collect()
        } else {
            (0..ab.effects.len())
                .filter(|&k| match &ab.pay {
                    Some(pc) => k < pc.skip_from as usize || paid_ok && k >= pc.first as usize,
                    None => true,
                })
                .map(|k| (k, ab.text.as_str(), ab.card_restr))
                .collect()
        };
        self.note(|| {
            format!(
                "P{} resolves {}: {}",
                me,
                card.map(|c| c.name.as_str()).unwrap_or("token"),
                ab.text
            )
        });
        self.reborn = None;
        self.last_countered.clear();
        // "Reveal the top card of your library. If it's an artifact card, draw a card."
        self.focus_card = ab
            .gate
            .as_ref()
            .filter(|g| g.top)
            .and_then(|_| self.players[me].library.last().map(|&c| (me as u8, c)));
        for (k, text, card_restr) in picks {
            if !self.players[me].alive {
                return;
            }
            if let Some(gt) = &ab.gate {
                let ok = match &gt.cond {
                    Some(c) => {
                        let s = pend.source.and_then(|id| self.idx(id));
                        self.cond_holds(c, me, s)
                    }
                    None => self.focus_is(gt),
                };
                if (gt.from as usize..gt.to as usize).contains(&k) && !ok {
                    continue;
                }
            }
            let inner = Ctx {
                text,
                card_restr,
                ..ctx.clone()
            };
            self.apply(&inner, &ab.effects[k]);
            // Back from the graveyard by its own ability: the rest is about the new permanent.
            if let Some(id) = self.reborn.take() {
                ctx.src = Some(id);
                ctx.snap = None;
            }
        }
        // Echo copies the last ability of a creature of ours.
        if ab.trig != Trig::Spell
            && !ab.effects.contains(&Effect::Copy)
            && pend
                .snapshot
                .as_ref()
                .or(pend
                    .source
                    .and_then(|id| self.idx(id))
                    .map(|i| &self.perms[i]))
                .is_some_and(|x| x.is_creature())
        {
            self.players[me].last_ability = Some(pend.clone());
        }
        // "If ~ is a Scout, it becomes a Human Faerie Detective" (Kellan): its new types
        // (and size) count for the next "if ~ is a Detective".
        if ab.text.contains("if ~ is a ") {
            if let (Some(rest), Some(i)) = (
                ab.text.split("it becomes a ").nth(1),
                pend.source.and_then(|id| self.idx(id)),
            ) {
                let words: Vec<&str> = rest
                    .split([' ', '.'])
                    .take_while(|w| !w.is_empty() && *w != "and")
                    .collect();
                let x = &mut self.perms[i];
                for w in &words {
                    if let Some((p, t)) = w.split_once('/') {
                        if let (Ok(p), Ok(t)) = (p.parse(), t.parse()) {
                            x.base_p = p;
                            x.base_t = t;
                        }
                    }
                }
                x.subtypes = words
                    .iter()
                    .filter(|w| !w.contains('/'))
                    .map(|w| w.to_string())
                    .collect();
                self.recalc();
            }
        }
        // "When you do, ..." right after this ability (not the counter-goal kind).
        if let Some(next) = abilities.get(pend.ability as usize + 1) {
            if next.trig == Trig::Reflexive && next.counter_goal.is_none() && paid_ok {
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

    /// "If target player has exactly 10 life, ~ deals 10 damage to that player": Some(the
    /// opponent at exactly that life, if any) for such texts.
    fn exact_life_target(&self, me: usize, text: &str) -> Option<Option<usize>> {
        let life = text
            .split("player has exactly ")
            .nth(1)?
            .split_whitespace()
            .next()?
            .parse::<i32>()
            .ok()?;
        Some(
            self.opponents(me)
                .into_iter()
                .find(|&o| self.players[o].life == life),
        )
    }

    /// Is the card the ability dealt with of the kind a gate asks for?
    fn focus_is(&self, gt: &Gate) -> bool {
        let Some(r) = self.focus_card else {
            return false;
        };
        let c = self.card(r);
        c.types & gt.types == gt.types && gt.subtype.as_ref().is_none_or(|s| c.has_subtype(s))
    }

    /// Pays a cost while an ability resolves ("you may sacrifice another creature. If you
    /// do"). None if it can't be paid; otherwise what was sacrificed, if anything.
    pub fn pay_res_cost(&mut self, ctx: &Ctx, cost: &ResCost) -> Option<Option<Perm>> {
        let me = ctx.ctrl;
        let src = ctx.src.unwrap_or(u32::MAX);
        match cost {
            ResCost::Free => Some(None),
            ResCost::Sacrifice { types, other } => {
                let i = ai::fodder(self, me, *types, if *other { src } else { u32::MAX })?;
                let snap = self.perms[i].clone();
                self.sacrifice(i);
                Some(Some(snap))
            }
            ResCost::SacrificeOrDiscard(types) => {
                if let Some(i) = ai::fodder(self, me, *types, src) {
                    let snap = self.perms[i].clone();
                    self.sacrifice(i);
                    Some(Some(snap))
                } else if !self.players[me].hand.is_empty() {
                    self.discard(me);
                    Some(None)
                } else {
                    None
                }
            }
            ResCost::SacrificeSelf => {
                let i = self.src_idx(ctx)?;
                let snap = self.perms[i].clone();
                self.sacrifice(i);
                Some(Some(snap))
            }
            ResCost::Mana(m) => self.pay(me, m).then_some(None),
            ResCost::Life(n) => {
                if self.players[me].life <= *n as i32 {
                    return None;
                }
                self.lose_life(me, *n as i32);
                Some(None)
            }
            ResCost::Discard => {
                if self.players[me].hand.is_empty() {
                    return None;
                }
                self.discard(me);
                Some(None)
            }
            ResCost::RemoveCounter { plus } => {
                let i = self.src_idx(ctx)?;
                let x = &mut self.perms[i];
                if *plus && x.counters > 0 {
                    x.counters -= 1;
                } else if !*plus && x.other_counters > 0 {
                    x.other_counters -= 1;
                } else {
                    return None;
                }
                Some(None)
            }
            ResCost::ExileSelf => {
                let i = self.src_idx(ctx)?;
                self.leave(i, Zone::Exile, false);
                Some(None)
            }
            ResCost::ExileNamed(n) => {
                let name = &ctx.card?.name;
                let pl = &self.players[me];
                let same: Vec<usize> = (0..pl.graveyard.len())
                    .filter(|&k| pl.cards[pl.graveyard[k] as usize].name == *name)
                    .collect();
                if same.len() < *n as usize {
                    return None;
                }
                for &k in same.iter().take(*n as usize).rev() {
                    let c = self.players[me].graveyard.remove(k);
                    self.players[me].exile.push(c);
                }
                Some(None)
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
        let id = self.perms[i].id;
        if !self.last_countered.contains(&id) {
            self.last_countered.push(id);
        }
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
                            x: None,
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
                                x: None,
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
        let n = if self.doubles_from(ctx.ctrl, "damage", ctx.src) {
            n * 2
        } else {
            n
        };
        if self.perms[i].protection & ctx.card.map(|c| c.colors).unwrap_or(0) != 0 {
            return;
        }
        if ctx.text.contains("would die this turn, exile it instead") {
            self.perms[i].finality = true;
        }
        // "If excess damage was dealt this way": beyond what was lethal.
        let x = &mut self.perms[i];
        let lethal = if x.is(T_PLANESWALKER) && !x.is_creature() {
            x.loyalty
        } else if deathtouch {
            1
        } else {
            x.t - x.damage
        };
        self.last_excess = n - lethal.max(0) as i32;
        x.hurt_by = ctx.src.unwrap_or(0);
        self.damage_perm(i, n as i16, deathtouch);
        if let Some(s) = self.src_idx(ctx) {
            self.perms[s].dealt_damage = true;
        }
        if src.is_some_and(|s| s.has(LIFELINK) && s.is_creature()) {
            self.gain_life(ctx.ctrl, n);
        }
    }

    fn hurt_player(&mut self, ctx: &Ctx, o: usize, n: i32) {
        let n = if self.doubles_from(ctx.ctrl, "damage", ctx.src) {
            n * 2
        } else {
            n
        };
        self.damage_player(ctx.ctrl, o, n, 0);
        if n > 0 && o != ctx.ctrl {
            self.actor = ctx.ctrl as u8;
            self.emit(Ev::NoncombatDamage, o, None);
        }
        if self
            .src_perm(ctx)
            .is_some_and(|s| s.has(LIFELINK) && s.is_creature())
        {
            self.gain_life(ctx.ctrl, n);
        }
    }

    /// The card a "return ~" / "return it" / "return that card" means, when the text names
    /// one: `Some(Some((player, graveyard index)))` if it's in a graveyard, `Some(None)` if
    /// it isn't (nothing returns), `None` if the text lets the player choose.
    fn returned_card(&self, ctx: &Ctx) -> Option<Option<(usize, usize)>> {
        let t = ctx.text;
        let itself = self.returns_itself(ctx);
        let that = t.contains("return it ")
            || t.contains("return it.")
            || t.contains("return that card")
            || t.contains("return that creature");
        if itself {
            let r = ctx
                .snap
                .as_ref()
                .and_then(|s| s.card)
                .or_else(|| self.src_idx(ctx).and_then(|i| self.perms[i].card));
            let Some(r) = r else {
                // Activated from the graveyard (no permanent): the card of that name there.
                let c = ctx.card?;
                let me = ctx.ctrl;
                let pl = &self.players[me];
                return Some(
                    pl.graveyard
                        .iter()
                        .rposition(|&g| pl.cards[g as usize].name == c.name)
                        .map(|k| (me, k)),
                );
            };
            let o = r.0 as usize;
            return Some(
                self.players
                    .get(o)
                    .and_then(|p| p.graveyard.iter().rposition(|&c| c == r.1))
                    .map(|k| (o, k)),
            );
        }
        if that && !t.contains("exile") {
            // The creature that just died went to the top of its owner's graveyard.
            let me = ctx.ctrl;
            let pl = &self.players[me];
            let creature = t.contains("creature");
            return Some(
                pl.graveyard
                    .iter()
                    .rposition(|&c| {
                        let card = &pl.cards[c as usize];
                        if creature {
                            card.is(T_CREATURE)
                        } else {
                            card.is_permanent()
                        }
                    })
                    .filter(|&k| k + 2 >= pl.graveyard.len())
                    .map(|k| (me, k)),
            );
        }
        None
    }

    /// Applies one effect.
    pub fn apply(&mut self, ctx: &Ctx, e: &Effect) {
        let me = ctx.ctrl;
        let text = ctx.text;
        match e {
            Effect::Destroy(_) if text.contains("odd or even") => {
                // Thanos: pick the parity that hurts the table most and spares the most of
                // our own; "each other creature" never includes the source.
                let (odd, _) = ai::best_parity(self, me, ctx.src);
                let ids: Vec<u32> = self
                    .perms
                    .iter()
                    .filter(|x| x.is_creature() && Some(x.id) != ctx.src)
                    .filter(|x| (ai::perm_mv(self, x) % 2 == 1) == odd)
                    .map(|x| x.id)
                    .collect();
                self.note(|| format!("P{} chooses {}", me, if odd { "odd" } else { "even" }));
                for id in ids {
                    if let Some(i) = self.idx(id) {
                        self.destroy(i);
                    }
                }
            }
            // "Destroy each nonland permanent with mana value X whose controller was dealt
            // combat damage by ~ this turn" (Steel Hellkite), with the X paid.
            Effect::Destroy(_) if text.contains("mana value x") && ctx.x.is_some() => {
                let x = ctx.x.unwrap_or(0);
                let hit = self.src_perm(ctx).map(|s| s.hit_turn).unwrap_or(0);
                let only_hit = text.contains("dealt combat damage by");
                let ids: Vec<u32> = self
                    .perms
                    .iter()
                    .filter(|y| !y.is(T_LAND) && ai::perm_mv(self, y) as i32 == x)
                    .filter(|y| !only_hit || hit & (1 << y.ctrl.min(7)) != 0)
                    .map(|y| y.id)
                    .collect();
                for id in ids {
                    if let Some(i) = self.idx(id) {
                        self.destroy(i);
                    }
                }
            }
            Effect::Destroy(t) => {
                let harmful = t.whose != Whose::Yours;
                let mut n = 0;
                for id in self.choose(ctx, t, harmful) {
                    if let Some(i) = self.idx(id) {
                        self.focus_card = self.perms[i].card.filter(|_| !t.mass);
                        if self.destroy(i) {
                            n += 1;
                        }
                    }
                }
                self.last_destroyed = n;
            }
            Effect::Exile { target, until } => {
                let harmful = target.whose != Whose::Yours;
                // "Its owner shuffles it into their library" (Living Library, Chaos Warp).
                let shuffle = text.contains("shuffles it into") || text.contains("shuffle it into");
                for id in self.choose(ctx, target, harmful) {
                    let Some(i) = self.idx(id) else { continue };
                    let x = self.perms[i].clone();
                    if shuffle {
                        self.leave(i, Zone::Library(false), false);
                        let o = x.owner as usize;
                        let mut lib = std::mem::take(&mut self.players[o].library);
                        self.rng.shuffle(&mut lib);
                        self.players[o].library = lib;
                        continue;
                    }
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
                self.last_excess = 0;
                let n = self.x_value(ctx, amount);
                if n <= 0 {
                    return;
                }
                if let Some(o) = self.exact_life_target(me, text) {
                    if let Some(o) = o {
                        self.hurt_player(ctx, o, n);
                    }
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
                // "... deals damage equal to its power to that permanent / to it".
                if let Some(ids) = self.prev_ids(ctx, to) {
                    for id in ids {
                        self.hurt(ctx, id, n);
                    }
                    return;
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
                                let paid = match self.idx(id) {
                                    Some(i) if self.perms[i].ctrl as usize != me => {
                                        self.pay_ward(me, i)
                                    }
                                    _ => true,
                                };
                                if paid {
                                    self.actor = ctx.ctrl as u8;
                                    self.emit_targeted(id);
                                    self.hurt(ctx, id, n);
                                }
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
                // Pumps earlier in the ability count ("gets +2/+2, then fights").
                self.recalc();
                // "Another target creature you control fights ...": not the source.
                let other_fighter = text.contains("another target creature you control fight")
                    || text.contains("target creature you control fight");
                let mine = if other_fighter {
                    ai::best_fighter_except(self, me, ctx.src)
                } else {
                    self.src_idx(ctx)
                        .filter(|&i| self.perms[i].is_creature())
                        .or_else(|| ai::best_fighter(self, me))
                };
                let Some(mi) = mine else { return };
                let my_id = self.perms[mi].id;
                // "Up to one target": only a fight it wins.
                let targets = if text.contains("fights up to one") {
                    let (power, tough) =
                        (self.perms[mi].p, self.perms[mi].t - self.perms[mi].damage);
                    let mine_value = ai::perm_value(self, mi);
                    let kills: Vec<usize> = self
                        .candidates(ctx, t, true)
                        .into_iter()
                        .filter(|&i| {
                            let x = &self.perms[i];
                            x.t - x.damage <= power && !x.has(INDESTRUCTIBLE)
                        })
                        .collect();
                    // Rather a fight it survives; a trade only for something worth as much.
                    let safe: Vec<usize> = kills
                        .iter()
                        .copied()
                        .filter(|&i| self.perms[i].p < tough)
                        .collect();
                    let trades: Vec<usize> = kills
                        .iter()
                        .copied()
                        .filter(|&i| ai::perm_value(self, i) >= mine_value)
                        .collect();
                    let pool = if safe.is_empty() { trades } else { safe };
                    ai::choose_victims(self, me, &pool, 1)
                        .into_iter()
                        .map(|i| self.perms[i].id)
                        .collect()
                } else {
                    self.choose(ctx, t, true)
                };
                for id in targets {
                    let (Some(a), Some(b)) = (self.idx(my_id), self.idx(id)) else {
                        continue;
                    };
                    let (pa, da) = (self.perms[a].p, self.perms[a].has(DEATHTOUCH));
                    let (pb, db) = (self.perms[b].p, self.perms[b].has(DEATHTOUCH));
                    self.perms[b].hurt_by = my_id;
                    self.damage_perm(b, pa, da);
                    if !text.contains("deals damage equal to its power") || text.contains("fight") {
                        if let Some(a) = self.idx(my_id) {
                            self.perms[a].hurt_by = id;
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
                // "Gets -3/-1": the power part can differ.
                let np = text
                    .split(" -")
                    .nth(1)
                    .and_then(|r| r.split('/').next())
                    .and_then(|d| d.parse::<i16>().ok())
                    .filter(|_| text.contains(&format!("/-{}", n)))
                    .unwrap_or(n);
                let harmful = to.whose != Whose::Yours;
                for id in self.choose(ctx, to, harmful) {
                    if let Some(i) = self.idx(id) {
                        if *until_eot {
                            self.perms[i].temp_p -= np;
                            self.perms[i].temp_t -= n;
                        } else {
                            self.perms[i].counters -= n;
                        }
                    }
                }
            }
            Effect::Bounce(t) => {
                let harmful = t.whose != Whose::Yours;
                // "Its owner puts it on their choice of the top or bottom of their library":
                // the owner takes the top, to draw it again.
                let to = if text.contains("top or bottom") || text.contains("from the top") {
                    Zone::Library(true)
                } else {
                    Zone::Hand
                };
                for id in self.choose(ctx, t, harmful) {
                    if let Some(i) = self.idx(id) {
                        self.leave(i, to, false);
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
                let mut victims: Vec<usize> = if *each_opponent {
                    self.opponents(me)
                } else {
                    ai::choose_opponent(self, me, 0).into_iter().collect()
                };
                // "Each player sacrifices": you too.
                if text.contains("each player sacrifices") {
                    victims.insert(0, me);
                }
                // "Sacrifices two creatures".
                let n = text
                    .split("sacrifices ")
                    .nth(1)
                    .and_then(|r| r.split_whitespace().next())
                    .map(|w| match w {
                        "two" => 2,
                        "three" => 3,
                        _ => 1,
                    })
                    .unwrap_or(1);
                for o in victims {
                    let types = if text.contains("creature or planeswalker") {
                        T_CREATURE | T_PLANESWALKER
                    } else if text.contains("nonland permanent") || text.contains("a permanent") {
                        0xFFFF & !T_LAND
                    } else {
                        T_CREATURE
                    };
                    for _ in 0..n {
                        if let Some(i) = ai::sacrifice_choice_r(self, o, types, &ctx.card_restr) {
                            self.sacrifice(i);
                        }
                    }
                }
            }
            // Countering happens when the spell is cast (see `ai::respond_to_spell`).
            Effect::CounterSpell => {}
            Effect::Draw { you: true, .. } | Effect::Loot if draw_then_discard(text).is_some() => {
                // "Draw two cards, then discard a card", "draw three cards. Then discard two
                // cards unless ...": both halves (the reader keeps only the net card count).
                let (d, k) = draw_then_discard(text).unwrap_or((1, 1));
                let d = if d < 0 {
                    self.x_value(ctx, &Amount { n: 3, x: true })
                } else {
                    d
                };
                self.draw(me, d.max(0) as u32);
                self.last_drawn = d;
                let skip = text.contains("unless you attacked this turn")
                    && self.players[me].attacked_this_turn;
                if !skip {
                    for _ in 0..k {
                        self.discard(me);
                    }
                }
            }
            Effect::Draw { amount, you } => {
                let n = self.x_value(ctx, amount).max(0) as u32;
                self.last_drawn = n as i32;
                // "Destroy target creature. Its controller draws a card": the player whose
                // permanent the ability just hit.
                let theirs = [
                    "its controller draws",
                    "that player draws",
                    "their controller draws",
                ]
                .iter()
                .any(|w| text.contains(w));
                if !*you && theirs && self.last_ctrl.is_some_and(|o| o as usize != me) {
                    let o = self.last_ctrl.unwrap_or(0) as usize;
                    if self.players[o].alive {
                        self.draw(o, n);
                    }
                } else if !*you
                    && (text.contains("each opponent draws")
                        || text.contains("target opponent draws"))
                {
                    // "If you do, each opponent draws a card" (Armor Wars).
                    let who: Vec<usize> = if text.contains("each opponent draws") {
                        self.opponents(me)
                    } else {
                        ai::choose_opponent(self, me, 0).into_iter().collect()
                    };
                    for o in who {
                        self.draw(o, n);
                    }
                } else if text.contains("each player may discard their hand and draw") {
                    // "Each player may discard their hand and draw seven cards" (the parser
                    // reads "draw" there as yours).
                    for p in 0..self.players.len() {
                        if self.players[p].alive {
                            self.draw(p, n);
                        }
                    }
                } else if !*you
                    && text.contains("that player draws")
                    && text.starts_with("at the beginning of each player's")
                {
                    // Dictate of Kruphix: the player whose draw step it is.
                    let a = self.active;
                    self.draw(a, n);
                } else if *you || !text.contains("target player") && !text.contains("each player") {
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
            // "Each player discards their hand, then draws seven": the hand rule did it.
            Effect::Discard { .. }
                if text.contains("each player discards their hand")
                    || text.contains("each player may discard their hand") => {}
            Effect::Discard { opponents } => {
                if *opponents {
                    let mut victims: Vec<usize> =
                        if text.contains("each opponent") || text.contains("each player") {
                            self.opponents(me)
                        } else {
                            ai::choose_opponent(self, me, 0).into_iter().collect()
                        };
                    // "Each player discards ...": you too.
                    if text.contains("each player discards") {
                        victims.insert(0, me);
                    }
                    let whole_hand =
                        text.contains("discards their hand") || text.contains("discard their hand");
                    // "Discards two cards".
                    let times = if text.contains("discards two") {
                        2
                    } else if text.contains("discards three") {
                        3
                    } else {
                        1
                    };
                    for o in victims {
                        if whole_hand {
                            let hand = std::mem::take(&mut self.players[o].hand);
                            let had = !hand.is_empty();
                            self.players[o].graveyard.extend(hand);
                            if had {
                                self.emit(Ev::Discard, o, None);
                            }
                        } else if times > 1 {
                            for _ in 0..times.min(self.players[o].hand.len()) {
                                self.discard(o);
                            }
                        } else if text.contains("reveal") || text.contains("you choose") {
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
                    let victims: Vec<usize> = if text.contains("each player mills") {
                        (0..self.players.len())
                            .filter(|&o| self.players[o].alive)
                            .collect()
                    } else if text.contains("each opponent mills") {
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
            // Recruit's Human Soldier comes from the discard (`Loot`), not on its own.
            Effect::Tokens { .. } if text.contains("recruit") && !text.contains("create") => {}
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
                let mut made = Vec::new();
                for _ in 0..n.min(20) {
                    let i = self.make_token(me, *power, *toughness, kw, subs.clone());
                    made.push(self.perms[i].id);
                    if text.contains("tapped and attacking")
                        || text.contains("tapped")
                            && text.contains("token")
                            && text.contains("enters tapped")
                    {
                        self.perms[i].tapped = true;
                    }
                    if text.contains("tapped and attacking") && self.in_combat {
                        self.perms[i].attacking = true;
                    }
                    self.after_enter(i);
                }
                // "Create two Goblins. They gain haste."
                if !made.is_empty() {
                    self.last_targets = made;
                }
            }
            Effect::ItemTokens { amount, kind } if kind == "copy" => {
                // "For each creature ..., create a token that's a copy of that creature": one
                // copy of each.
                let n = if text.contains("for each") {
                    1
                } else {
                    self.x_value(ctx, amount).max(1)
                };
                self.token_copies(ctx, n);
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
                    for id in self.pick(ctx, to, false) {
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
                let ids = self.pick(ctx, to, harmful);
                // "+X/+0 where X is ...", "+1/+1 for each ..." (the reader's typical 3 per
                // unit becomes the real count).
                let (mut power, mut toughness) = (*power, *toughness);
                let x = || self.x_value(ctx, &Amount { n: 3, x: true });
                if text.contains("+x/") || text.contains("/+x") {
                    let k = x();
                    if text.contains("+x/") {
                        power = k;
                    }
                    if text.contains("/+x") {
                        toughness = k;
                    }
                } else if let Some(k) = text
                    .split("for each ")
                    .nth(1)
                    .and_then(|r| self.count_of(me, ctx.src, r))
                {
                    power = power / 3 * k;
                    toughness = toughness / 3 * k;
                }
                let double = text.contains("double target creature's power")
                    || text.contains("double its power");
                for id in ids {
                    let Some(i) = self.idx(id) else { continue };
                    let (power, toughness) = if double {
                        // "Double its power (and toughness)": +its current power.
                        let x = &self.perms[i];
                        (
                            x.p.max(0) as i32,
                            if text.contains("power and toughness") {
                                x.t.max(0) as i32
                            } else {
                                0
                            },
                        )
                    } else {
                        (power, toughness)
                    };
                    if *until_eot || ctx.until_eot {
                        self.perms[i].temp_p += power as i16;
                        self.perms[i].temp_t += toughness as i16;
                    } else {
                        // A one-off lasting buff: like counters.
                        self.perms[i].base_p += power as i16;
                        self.perms[i].base_t += toughness as i16;
                    }
                }
            }
            Effect::Grant {
                keywords,
                to,
                until_eot,
            } => {
                // "Gains your choice of trample, hexproof, or haste": one of them.
                let bits = if text.contains("your choice of") {
                    keywords
                        .first()
                        .map_or(0, |k| keyword_bits(std::slice::from_ref(k)))
                } else {
                    keyword_bits(keywords)
                };
                let ids = self.pick(ctx, to, false);
                for id in ids {
                    let Some(i) = self.idx(id) else { continue };
                    if *until_eot || ctx.until_eot {
                        self.perms[i].temp_kw |= bits;
                    } else {
                        self.perms[i].kw |= bits;
                    }
                }
            }
            // "You gain life equal to the life lost this way": the drain gives it.
            Effect::GainLife(_)
                if text.contains("life lost this way")
                    || text.contains("equal to the life lost") => {}
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
                let mut victims: Vec<usize> = if *each {
                    self.opponents(me)
                } else if let Some(o) = self.exact_life_target(me, text) {
                    o.into_iter().collect()
                } else {
                    ai::choose_opponent(self, me, n).into_iter().collect()
                };
                // "Deals 2 damage to each player", "each player loses 1 life": you too.
                if *each
                    && (text.contains("damage to each player")
                        || text.contains("each player loses"))
                {
                    victims.insert(0, me);
                }
                let mut total = 0;
                for o in victims {
                    // "... loses 3 life unless that player sacrifices a nonland permanent or
                    // discards a card": they pay with something they can spare.
                    if text.contains(" unless that player") {
                        let types = if text.contains("nonland permanent") {
                            0xFFFF & !T_LAND
                        } else {
                            T_CREATURE
                        };
                        if text.contains("sacrifices") {
                            if let Some(i) = ai::fodder(self, o, types, u32::MAX) {
                                self.sacrifice(i);
                                continue;
                            }
                        }
                        if text.contains("discards a card") && !self.players[o].hand.is_empty() {
                            self.discard(o);
                            continue;
                        }
                    }
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
                // "Up to two basic land cards", "up to X", "two basic land cards".
                let n = if text.contains("up to two") || text.contains("for two ") {
                    2
                } else if text.contains("up to three") || text.contains("for three ") {
                    3
                } else if text.contains("up to x ") || text.contains("for x ") {
                    self.x_value(ctx, &Amount { n: 1, x: true }).clamp(0, 5)
                } else {
                    1
                };
                // Cultivate: "put one onto the battlefield tapped and the other into your hand".
                let split = text.contains("and the other into your hand");
                let tapped = text.contains("battlefield tapped") || text.contains("tapped and");
                for k in 0..n {
                    self.fetch_land(me, *battlefield && (!split || k == 0), tapped, basic);
                }
            }
            Effect::Reanimate => {
                let any = text.contains("a graveyard")
                    || text.contains("any graveyard")
                    || text.contains("all graveyards");
                let d = self.dyn_values(&ctx.card_restr, Some(ctx), me, ctx.src);
                // "Return all creature cards with mana value 2 or less": every one that fits.
                let all = text.contains("return all")
                    || text.contains("return each")
                    || text.contains("put all");
                self.last_targets.clear();
                // "Return ~ / this card from your graveyard", "when ~ dies, return it": the
                // card itself; "whenever a creature you control dies, return that card": the
                // card that just died. Only that card, and only if it's there.
                let fixed = self.returned_card(ctx);
                for _ in 0..if all { 20 } else { 1 } {
                    let pick = match fixed {
                        Some(f) => f,
                        None => ai::reanimate_choice(self, me, any, &ctx.card_restr, &d),
                    };
                    let Some((o, k)) = pick else {
                        break;
                    };
                    let itself = fixed.is_some() && self.returns_itself(ctx);
                    if itself && self.reborn_blocked(ctx) {
                        break;
                    }
                    // An Aura comes back attached to a creature it can enchant ("target
                    // creature you control with power 1 or less"), or not at all.
                    let host = if self.players[o].cards[self.players[o].graveyard[k] as usize]
                        .is(T_AURA)
                    {
                        let small = text.contains("power 1 or less");
                        let pick = self
                            .perms
                            .iter()
                            .filter(|x| x.ctrl as usize == me && x.is_creature())
                            .filter(|x| !small || x.p <= 1)
                            .max_by_key(|x| x.p + x.t)
                            .map(|x| x.id);
                        if pick.is_none() {
                            break;
                        }
                        pick
                    } else {
                        None
                    };
                    let c = self.players[o].graveyard.remove(k);
                    self.emit(Ev::LeavesGraveyard, o, None);
                    if itself {
                        let name = &self.players[o].cards[c as usize].name;
                        self.note(|| format!("  {} returns from the graveyard", name));
                    }
                    if let Some(i) = self.enter_card(o, c, me, false) {
                        if text.contains("tapped") {
                            self.perms[i].tapped = true;
                        }
                        if host.is_some() {
                            self.perms[i].attached = host;
                        }
                        // "With an additional +1/+1 counter", "with a finality counter".
                        self.reborn_details(ctx, i);
                        let id = self.perms[i].id;
                        self.last_targets.push(id);
                        // Back by its own ability: the rest of it ("she gains haste") is
                        // about the returned permanent.
                        if itself {
                            self.reborn = Some(id);
                            self.recalc();
                        }
                    }
                }
            }
            Effect::Regrowth => {
                let creature = text.contains("creature card");
                let itself = self.returns_itself(ctx);
                let pick = if itself {
                    // "Return ~ from your graveyard to your hand": only the card itself.
                    match self.returned_card(ctx) {
                        Some(Some((o, k))) if o == me && !self.reborn_blocked(ctx) => Some(k),
                        _ => None,
                    }
                } else if text.contains("return that card") || text.contains("return it") {
                    // The card that just went there (e.g. the creature that died).
                    self.players[me].graveyard.len().checked_sub(1)
                } else {
                    let d = self.dyn_values(&ctx.card_restr, Some(ctx), me, ctx.src);
                    ai::regrowth_choice(self, me, creature, &ctx.card_restr, &d)
                };
                if let Some(k) = pick {
                    let c = self.players[me].graveyard.remove(k);
                    self.players[me].hand.push(c);
                    self.focus_card = Some((me as u8, c));
                    self.emit(Ev::LeavesGraveyard, me, None);
                    // "Up to two target creature cards".
                    if text.contains("up to two target") || text.contains("two target") {
                        let d = self.dyn_values(&ctx.card_restr, Some(ctx), me, ctx.src);
                        if let Some(k) =
                            ai::regrowth_choice(self, me, creature, &ctx.card_restr, &d)
                        {
                            let c = self.players[me].graveyard.remove(k);
                            self.players[me].hand.push(c);
                            self.emit(Ev::LeavesGraveyard, me, None);
                        }
                    }
                }
            }
            Effect::Untap(t) => {
                let ids: Vec<u32> = if t.kind == Kind::It || t.restr.has(restrict::PREV) {
                    self.pick(ctx, t, false)
                } else if text.contains("untap target") || text.contains("untap another target") {
                    // A target ("untap another target creature you control. If it's a Bear,
                    // put a counter on it"): remembered, even if it wasn't tapped.
                    let t2 = Target {
                        whose: Whose::Yours,
                        ..*t
                    };
                    let cands = self.candidates(ctx, &t2, false);
                    let pick = ai::untap_choice(self, me, &cands)
                        .or_else(|| ai::choose_friends(self, me, &cands, 1).first().copied());
                    let ids: Vec<u32> = pick.map(|i| self.perms[i].id).into_iter().collect();
                    self.targeted = true;
                    self.last_targets = ids.clone();
                    ids
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
                // "Untap target creature you control. It gets +2/+2" (Vow to Erebor).
                if t.kind != Kind::It && !ids.is_empty() {
                    self.last_targets = ids.clone();
                }
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
                if text.contains("your next instant or sorcery")
                    || text.contains("you next cast an instant or sorcery")
                {
                    // A delayed trigger: the next instant or sorcery this turn is copied
                    // (Loki: only one with mana value up to his power).
                    let limit = if text.contains("less than or equal to ~'s power") {
                        self.src_perm(ctx).map(|x| x.p.max(0) as u8).unwrap_or(0)
                    } else {
                        u8::MAX
                    };
                    self.players[me].copy_next.push(limit);
                } else if text.contains("when that mana is spent") {
                    // Pyromancer's Goggles: copies when its mana casts a red spell (`cast`).
                } else if text.contains("activated or triggered ability") {
                    // Echo: copy the last ability of a creature of ours.
                    if let Some(last) = self.players[me].last_ability.clone() {
                        self.pending.push(Pending {
                            ctrl: me as u8,
                            ..last
                        });
                    }
                } else if ctx.card.is_some_and(|c| !c.is_permanent())
                    || text.contains("copy that spell")
                    || text.contains("copy it")
                    || text.contains("copy target") && text.contains("spell")
                {
                    // "Copy it for each other instant and sorcery spell you've cast before it
                    // this turn" (Thousand-Year Storm).
                    let times = if text.contains("for each other instant and sorcery") {
                        self.players[me].inst_sorc_this_turn.saturating_sub(1) as usize
                    } else {
                        1
                    };
                    let spell = self.casting.or(self.players[me].last_spell);
                    if let Some(spell) = spell {
                        for _ in 0..times {
                            self.copy_spell(me, spell);
                        }
                    }
                }
                // Anything else ("copy those exiled cards") is read by other effects.
            }
            Effect::Empower(n) => {
                // "If excess damage was dealt this way, empower Jace X, where X is that
                // excess damage": no excess, no Jace.
                let k = if text.contains("that excess damage") {
                    self.last_excess.max(0) as i16
                } else {
                    self.x_value(ctx, n) as i16
                };
                if k <= 0 && text.contains("excess damage") {
                    return;
                }
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
                let k = if text.contains("that excess damage") {
                    self.last_excess.max(0)
                } else {
                    self.x_value(ctx, n)
                };
                if k <= 0 && text.contains("excess damage") {
                    return;
                }
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
                    self.last_targets = vec![self.perms[i].id];
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
                    restr: Default::default(),
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
                    // "Exile (another / up to one) target creature you control": not itself.
                    let src = ctx.src.filter(|_| text.contains("target"));
                    ai::blink_choice(self, me, src)
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
                // "Return it to the battlefield ... at the beginning of the next end step".
                if text.contains("beginning of the next end step") {
                    self.delayed_returns.push(r);
                    return;
                }
                if let Some(k) = self.players[owner].exile.iter().rposition(|&c| c == r.1) {
                    self.players[owner].exile.remove(k);
                    if let Some(j) = self.enter_card(owner, r.1, x.ctrl as usize, false) {
                        self.perms[j].sick = x.sick;
                    }
                }
            }
            Effect::Tutor => {
                if let Some(k) = ai::tutor_choice(self, me, text, &ctx.card_restr) {
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
                    } else if text.contains("put that card into your graveyard")
                        || text.contains("put it into your graveyard")
                    {
                        self.players[me].graveyard.push(c);
                    } else {
                        self.players[me].hand.push(c);
                    }
                }
            }
            Effect::FreeSpell
                if text.contains("onto the battlefield")
                    && (text.contains("cards of your library")
                        || text.contains("from the top of your library until")) =>
            {
                self.dig_onto_battlefield(ctx);
            }
            // "Exile the top card of each player's library, then you may cast any number of
            // spells from among those cards without paying their mana costs" (Etali), "look
            // at the top six cards ... you may cast a spell from among them" (Cosmic Cube),
            // "target opponent exiles the top five cards ... cast up to two" (Doom Reigns
            // Supreme): from those libraries, not the hand.
            Effect::FreeSpell if free_from_top_of(text).is_some() => {
                let spec = free_from_top_of(text).unwrap_or_default();
                self.free_from_top(ctx, spec);
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
                // "Create a Dwarf token, then attach this Equipment to it", "attach it to the
                // amassed Army": the creature just made.
                let to_new = [
                    " to it",
                    " to that creature",
                    " to the amassed army",
                    " to the token",
                ]
                .iter()
                .any(|w| text.contains(w));
                let made = self
                    .last_targets
                    .iter()
                    .rev()
                    .find_map(|&id| self.idx(id))
                    .filter(|_| to_new);
                // "Attach it to target Dwarf you control": only one of those.
                let named = text
                    .split(" to target ")
                    .nth(1)
                    .filter(|_| text.contains("attach") && made.is_none())
                    .map(|p| p.split(['.', ';']).next().unwrap_or(p));
                if let Some(p) = named {
                    let t = crate::abilities::target_of(p, false);
                    let t = Target {
                        whose: Whose::Yours,
                        ..t
                    };
                    let cands = self.candidates(ctx, &t, false);
                    let Some(&c) = ai::choose_friends(self, me, &cands, 1).first() else {
                        return;
                    };
                    if let Some(e) = equipment {
                        let cid = self.perms[c].id;
                        self.perms[e].attached = Some(cid);
                    }
                    return;
                }
                if let Some(e) = equipment {
                    let eid = self.perms[e].id;
                    if let Some(c) = made.or_else(|| ai::equip_target(self, me, eid)) {
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
            // Impulse draws: the cards are exiled and may be played for a while.
            Effect::Dig if impulse_of(text).is_some() => {
                let imp = impulse_of(text).unwrap_or_default();
                self.impulse(ctx, imp);
            }
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
                // "Mill four cards, then put all Elf cards from among them into your hand":
                // the milled cards, only the kind named.
                if text.contains("mill") && text.contains("from among them") {
                    let milled = text
                        .split("mill ")
                        .nth(1)
                        .and_then(|r| r.split_whitespace().next())
                        .and_then(|w| match w {
                            "two" => Some(2),
                            "three" => Some(3),
                            "four" => Some(4),
                            "five" => Some(5),
                            "six" => Some(6),
                            _ => w.parse().ok(),
                        })
                        .unwrap_or(3usize);
                    let d = self.dyn_values(&ctx.card_restr, Some(ctx), me, ctx.src);
                    let all = text.contains("put all");
                    let gy = &self.players[me].graveyard;
                    let from = gy.len().saturating_sub(milled);
                    let mut picks: Vec<usize> = (from..gy.len())
                        .filter(|&k| {
                            Game::card_ok(
                                &ctx.card_restr,
                                &self.players[me].cards[gy[k] as usize],
                                &d,
                            )
                        })
                        .collect();
                    // "You may put a permanent card from among them into your hand": the best.
                    if !all && picks.len() > 1 {
                        let value = |k: usize| self.players[me].cards[gy[k] as usize].value;
                        let best = picks.iter().copied().max_by(|&a, &b| {
                            value(a)
                                .partial_cmp(&value(b))
                                .unwrap_or(std::cmp::Ordering::Equal)
                        });
                        picks = best.into_iter().collect();
                    }
                    for k in picks.into_iter().rev() {
                        let c = self.players[me].graveyard.remove(k);
                        self.players[me].hand.push(c);
                    }
                    return;
                }
                // Two piles, an opponent picks which one you keep: about half the cards.
                let keep = if text.contains("pile") {
                    (n / 2).max(1)
                } else {
                    1
                };
                let lib_before = self.players[me].library.len();
                for _ in 0..keep {
                    ai::dig(self, me, n, text.contains("land"));
                }
                // "Put the rest / the other into your graveyard": the cards looked at go there.
                if text.contains("into your graveyard") {
                    let looked = n.min(lib_before).saturating_sub(keep);
                    for _ in 0..looked {
                        if !self.players[me].library.is_empty() {
                            let c = self.players[me].library.remove(0);
                            self.players[me].graveyard.push(c);
                        }
                    }
                }
            }
            Effect::OtherCounters(kind) => {
                // Where the counters go: "on ~", "on each Equipment you control", "on it" /
                // "on each of those creatures", "on target creature".
                let on = text
                    .find(&format!("{} counter", kind))
                    .and_then(|i| text[i..].split_once(" on "))
                    .map(|x| x.1)
                    .unwrap_or("~");
                let on = on.split(['.', ';', ',']).next().unwrap_or(on).trim();
                let on = on.trim_start_matches("each of ");
                let src_ids = || -> Vec<u32> {
                    ctx.src
                        .filter(|&id| self.idx(id).is_some())
                        .into_iter()
                        .collect()
                };
                let ids: Vec<u32> = if on.starts_with('~') || on.starts_with("this ") {
                    src_ids()
                } else if restrict::refers_back(on) {
                    let alive: Vec<u32> = self
                        .last_targets
                        .iter()
                        .copied()
                        .filter(|&id| self.idx(id).is_some())
                        .collect();
                    let about_other = text.starts_with("whenever ")
                        && !crate::playtest::compile::trigger_part(text).contains('~');
                    if !alive.is_empty() || self.targeted {
                        alive
                    } else if let Some(s) = ctx
                        .subject
                        .filter(|&s| about_other && Some(s) != ctx.src && self.idx(s).is_some())
                    {
                        vec![s]
                    } else {
                        src_ids()
                    }
                } else if on.starts_with("each ") || on.starts_with("all ") {
                    let t = crate::abilities::target_of(
                        on.trim_start_matches("each ").trim_start_matches("all "),
                        true,
                    );
                    self.choose(ctx, &t, kind == "stun")
                } else if !on.contains("target") {
                    src_ids()
                } else {
                    Vec::new()
                };
                let ids = if !ids.is_empty() || !on.contains("target") {
                    ids
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
                        restr: Default::default(),
                        count: 1,
                    };
                    let harmful = kind == "stun";
                    self.choose(ctx, &t, harmful)
                };
                for id in ids {
                    let Some(i) = self.idx(id) else { continue };
                    match kind.as_str() {
                        // Put on a returning card as it comes back (`reborn_details`).
                        "finality" => {}
                        "loyalty" => self.perms[i].loyalty += 1,
                        "shield" => self.perms[i].shield += 1,
                        "stun" => self.perms[i].stun += 1,
                        k if KEYWORDS.iter().any(|(w, _)| *w == k) => {
                            self.perms[i].kw |= keyword_bits(&[k.to_string()]);
                        }
                        _ => self.add_other_counters(i, 1),
                    }
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

    /// "Return ~ from your graveyard to ...", "when ~ dies, return it to the battlefield":
    /// the card brings back itself, not the best card there.
    fn returns_itself(&self, ctx: &Ctx) -> bool {
        let t = ctx.text;
        t.contains("return ~")
            || t.contains("return this card")
            || (t.starts_with("when ~ die")
                || t.starts_with("whenever ~ die")
                || t.starts_with("when ~ is put into"))
                && [
                    "return it",
                    "return her",
                    "return him",
                    "return them",
                    "return that card",
                ]
                .iter()
                .any(|r| t.contains(r))
    }

    /// "If it wasn't a Demon" / "if they were a creature": checked on what died; the card
    /// stays in the graveyard otherwise.
    fn reborn_blocked(&self, ctx: &Ctx) -> bool {
        let t = ctx.text;
        let Some(s) = &ctx.snap else {
            return false;
        };
        if let Some(rest) = t.split("if it wasn't a ").nth(1) {
            let what = rest.split([',', ' ']).next().unwrap_or("");
            if s.subtypes.iter().any(|x| x == what) {
                return true;
            }
        }
        (t.contains("if they were a creature") || t.contains("if it was a creature"))
            && !s.is_creature()
    }

    /// How a returned card comes back: "tapped", "with two +1/+1 counters", "with a finality
    /// counter", "it loses all abilities", "it's a Demon in addition", "they're an artifact".
    fn reborn_details(&mut self, ctx: &Ctx, i: usize) {
        let t = ctx.text;
        if t.contains("to the battlefield tapped") || t.contains("battlefield tapped") {
            self.perms[i].tapped = true;
        }
        // "with a +1/+1 counter on her", "with two +1/+1 counters on it".
        if let Some(j) = t.find(" +1/+1 counter") {
            let words: Vec<&str> = t[..j].rsplit(' ').take(3).collect();
            let n = match words.first().copied().unwrap_or("") {
                "a" | "an" | "one" | "additional" => 1,
                "two" => 2,
                "three" => 3,
                "four" => 4,
                d => d.parse().unwrap_or(0),
            };
            if n > 0 && words.contains(&"with") {
                self.add_counters(i, n);
            }
        }
        let x = &mut self.perms[i];
        if t.contains("finality counter") {
            x.finality = true;
        }
        if t.contains("loses all abilities") {
            x.no_abilities = true;
            x.kw = 0;
        }
        if let Some(rest) = t
            .split("it's a ")
            .nth(1)
            .filter(|r| r.contains("in addition"))
        {
            let what = rest.split(' ').next().unwrap_or("").to_string();
            if !what.is_empty() {
                x.subtypes.push(what);
            }
        }
        if t.contains("they're an artifact") || t.contains("it's an artifact") {
            x.types = (x.types & T_LEGENDARY) | T_ARTIFACT;
        }
    }

    /// "Reveal the top X cards of your library. You may put any number of permanent cards
    /// with mana value X or less from among them onto the battlefield. Then put the rest into
    /// your graveyard" (Genesis Wave), "reveal cards until you reveal a creature card. Put
    /// that card onto the battlefield" (Identity Echo), "look at the top twenty cards, put any
    /// number of land cards onto the battlefield tapped" (Through the Forest Gate).
    fn dig_onto_battlefield(&mut self, ctx: &Ctx) {
        let me = ctx.ctrl;
        let t = ctx.text;
        let word_n = |w: &str| -> Option<usize> {
            Some(match w {
                "a" | "one" => 1,
                "two" => 2,
                "three" => 3,
                "four" => 4,
                "five" => 5,
                "six" => 6,
                "seven" => 7,
                "eight" => 8,
                "nine" => 9,
                "ten" => 10,
                "thirteen" => 13,
                "twenty" => 20,
                "x" => self.cur_x.unwrap_or(3).max(0) as usize,
                d => d.parse().ok()?,
            })
        };
        let types = if t.contains("land card") {
            T_LAND
        } else if t.contains("creature or planeswalker card") {
            T_CREATURE | T_PLANESWALKER
        } else if t.contains("creature card") {
            T_CREATURE
        } else {
            !(T_INSTANT | T_SORCERY)
        };
        // A mana value limit: "mana value x or less", "less than or equal to the number of
        // lands you control".
        let limit: u8 = if t.contains("mana value x or less") {
            self.cur_x.unwrap_or(3).max(0) as u8
        } else if t.contains("less than or equal to the number of lands you control") {
            self.count_of(me, ctx.src, "lands you control").unwrap_or(0) as u8
        } else if let Some(n) = t
            .split("mana value ")
            .nth(1)
            .and_then(|r| r.split_whitespace().next())
            .and_then(|w| w.parse::<u8>().ok())
        {
            n
        } else {
            u8::MAX
        };
        let fits = |g: &Self, c: u16| {
            let sc = &g.players[me].cards[c as usize];
            sc.types & types != 0 && sc.mv <= limit
        };
        let until = t.contains("until you reveal");
        let n = if until {
            self.players[me].library.len()
        } else {
            t.split("top ")
                .nth(1)
                .and_then(|r| r.split_whitespace().next())
                .and_then(word_n)
                .unwrap_or(3)
        };
        let n = n.min(self.players[me].library.len());
        let k = self.players[me].library.len() - n;
        let revealed: Vec<u16> = if until {
            // Up to and including the first card that fits.
            let lib = &self.players[me].library;
            let stop = (0..lib.len()).rev().find(|&j| fits(self, lib[j]));
            match stop {
                Some(j) => self.players[me].library.split_off(j),
                None => std::mem::take(&mut self.players[me].library),
            }
        } else {
            self.players[me].library.split_off(k)
        };
        let all = t.contains("any number of");
        let random = t.contains("random");
        let mut picks: Vec<u16> = revealed
            .iter()
            .copied()
            .filter(|&c| fits(self, c))
            .collect();
        if !all && picks.len() > 1 {
            let keep = if random {
                self.rand(picks.len())
            } else {
                (0..picks.len())
                    .max_by(|&a, &b| {
                        let va = self.players[me].cards[picks[a] as usize].value;
                        let vb = self.players[me].cards[picks[b] as usize].value;
                        va.partial_cmp(&vb).unwrap()
                    })
                    .unwrap_or(0)
            };
            picks = vec![picks[keep]];
        }
        let mut rest: Vec<u16> = revealed;
        for &c in &picks {
            if let Some(j) = rest.iter().position(|&x| x == c) {
                rest.remove(j);
            }
        }
        let tapped = t.contains("battlefield tapped");
        for c in picks {
            if let Some(i) = self.enter_card(me, c, me, false) {
                if tapped {
                    self.perms[i].tapped = true;
                }
            }
        }
        if t.contains("into your graveyard") {
            self.players[me].graveyard.extend(rest);
        } else if t.contains("on the bottom") {
            for c in rest {
                self.players[me].library.insert(0, c);
            }
        } else {
            self.players[me].library.extend(rest);
        }
        if t.contains("shuffle") {
            let mut lib = std::mem::take(&mut self.players[me].library);
            self.rng.shuffle(&mut lib);
            self.players[me].library = lib;
        }
    }

    /// Puts a copy of a spell's effects on the stack (CR 707.10).
    pub fn copy_spell(&mut self, me: usize, spell: (CardRef, bool)) {
        let c = self.card(spell.0);
        let abilities = if spell.1 {
            c.back.as_ref().map(|b| &b.abilities)
        } else {
            Some(&c.abilities)
        };
        self.note(|| format!("  P{} copies {}", me, c.name));
        for (ai, a) in abilities.into_iter().flatten().enumerate() {
            if a.trig == Trig::Spell && !a.effects.contains(&Effect::Copy) {
                self.pending.push(Pending {
                    ctrl: me as u8,
                    source: None,
                    card: spell.0,
                    back: spell.1,
                    ability: ai as u16,
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

    /// "Create a token that's a copy of ...": of target creature, of the card itself, of
    /// each creature a player controls; "except it has haste and 'at the beginning of the
    /// end step, sacrifice this token'", "except it isn't legendary".
    fn token_copies(&mut self, ctx: &Ctx, n: i32) {
        let me = ctx.ctrl;
        let text = ctx.text;
        let mut models: Vec<(CardRef, Option<Perm>)> = Vec::new();
        let card_of = |x: &Perm| x.card.map(|r| (r, Some(x.clone())));
        if text.contains("for each nontoken creature you control") {
            models.extend(
                self.perms
                    .iter()
                    .filter(|x| x.ctrl as usize == me && x.is_creature() && !x.is_token())
                    .filter_map(card_of),
            );
        } else if text.contains("for each creature target player controls") {
            // The player whose creatures are worth the most (yourself included).
            let worth = |o: usize| -> f32 {
                (0..self.perms.len())
                    .filter(|&i| self.perms[i].ctrl as usize == o && self.perms[i].is_creature())
                    .map(|i| ai::perm_value(self, i))
                    .sum()
            };
            let best = (0..self.players.len())
                .filter(|&o| self.players[o].alive)
                .max_by(|&a, &b| worth(a).partial_cmp(&worth(b)).unwrap());
            if let Some(o) = best {
                models.extend(
                    self.perms
                        .iter()
                        .filter(|x| x.ctrl as usize == o && x.is_creature())
                        .filter_map(card_of),
                );
            }
        } else if text.contains("from a graveyard") && text.contains("copy of it") {
            // Abyssal Harvester: a creature card put into a graveyard this turn, exiled.
            if self.died_this_turn > 0 {
                let found = (0..self.players.len()).rev().find_map(|o| {
                    let pl = &self.players[o];
                    pl.graveyard
                        .iter()
                        .rposition(|&c| pl.cards[c as usize].is(T_CREATURE))
                        .map(|k| (o, k))
                });
                if let Some((o, k)) = found {
                    let c = self.players[o].graveyard.remove(k);
                    self.players[o].exile.push(c);
                    models.push(((o as u8, c), None));
                }
            }
        } else if text.contains("copy of target")
            || text.contains("copy of another target")
            || text.contains("copies of target")
        {
            let yours = text.contains("you control");
            let pick = if text.contains("nonland permanent") {
                (0..self.perms.len())
                    .filter(|&i| {
                        let x = &self.perms[i];
                        x.ctrl as usize == me
                            && !x.is(T_LAND)
                            && x.card.is_some()
                            && Some(x.id) != ctx.src
                    })
                    .max_by(|&a, &b| {
                        ai::perm_value(self, a)
                            .partial_cmp(&ai::perm_value(self, b))
                            .unwrap()
                    })
            } else {
                ai::best_to_copy(self, me, !yours)
            };
            if let Some(i) = pick {
                models.extend(card_of(&self.perms[i].clone()));
            }
        } else {
            // "A copy of it / of ~ / of them": the card itself.
            match (self.src_perm(ctx), ctx.card_ref) {
                (Some(x), _) if x.card.is_some() => models.extend(card_of(&x)),
                (_, Some(r)) => models.push((r, None)),
                _ => {}
            }
        }
        let haste = text.contains("except it has haste") || text.contains("with haste");
        let sac_at_end =
            text.contains("at the beginning of the end step") && text.contains("sacrifice");
        let not_legendary = text.contains("isn't legendary") || text.contains("aren't legendary");
        let nightmare = text.contains("nightmare in addition");
        let n = if self.doubles(me, "token") { n * 2 } else { n };
        let mut made = Vec::new();
        for (r, model) in &models {
            for _ in 0..n.min(10) {
                let t = self.new_perm(me, me, Some(*r), Tok::Other);
                let x = &mut self.perms[t];
                if let Some(m) = model {
                    // Copiable values: printed P/T and keywords, not counters or damage.
                    x.base_p = m.base_p;
                    x.base_t = m.base_t;
                    x.kw = m.kw;
                }
                if haste {
                    x.kw |= HASTE;
                }
                if not_legendary {
                    x.types &= !T_LEGENDARY;
                }
                if nightmare {
                    x.subtypes.push("nightmare".into());
                }
                x.sac_at_end = sac_at_end;
                let id = x.id;
                made.push(id);
                self.after_enter(t);
                // Its own enters abilities trigger too.
                for (ai, a) in self.card(*r).abilities.iter().enumerate() {
                    if a.trig == Trig::Enters {
                        self.pending.push(Pending {
                            ctrl: me as u8,
                            source: Some(id),
                            card: *r,
                            back: false,
                            ability: ai as u16,
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
        // "Then exile all other Nightmare tokens you control."
        if nightmare && !made.is_empty() {
            let others: Vec<u32> = self
                .perms
                .iter()
                .filter(|x| {
                    x.ctrl as usize == me
                        && x.is_token()
                        && x.subtypes.iter().any(|s| s == "nightmare")
                        && !made.contains(&x.id)
                })
                .map(|x| x.id)
                .collect();
            for id in others {
                if let Some(i) = self.idx(id) {
                    self.leave(i, Zone::Exile, false);
                }
            }
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
                // "Gets two poison counters".
                let n = match text
                    .split(" poison counter")
                    .next()
                    .and_then(|h| h.rsplit(' ').next())
                {
                    Some("two") => 2,
                    Some("three") => 3,
                    _ => 1,
                };
                if let Some(o) = ai::choose_opponent(self, me, 0) {
                    self.players[o].poison = self.players[o].poison.saturating_add(n);
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
                    // "Target creature gets -5/-0" is one creature.
                    mass: label == "weakens attackers" && !text.contains("target creature"),
                    other: false,
                    restricted: false,
                    restr: Default::default(),
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
                            restr: Default::default(),
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
            "sets base power/toughness"
                if base_pt_of(text).is_some()
                    || text.contains("equal to ~'s power and toughness") =>
            {
                self.set_base_pt(ctx);
            }
            "transforms" => {
                if let Some(i) = self.src_idx(ctx) {
                    self.transform(i);
                }
            }
            "extra turn" => {
                self.players[me].extra_turns = self.players[me].extra_turns.saturating_add(1);
            }
            // Time Stop: the stack is exiled and the rest of the turn is skipped (combat
            // damage included).
            "ends the turn" => {
                self.end_turn = true;
                self.fog = true;
                self.pending.clear();
            }
            "can't block this turn" => {
                let t = Target {
                    kind: Kind::Creature,
                    whose: Whose::Theirs,
                    mass: false,
                    other: false,
                    restricted: false,
                    restr: Default::default(),
                    count: 1,
                };
                for id in self.choose(ctx, &t, true) {
                    if let Some(i) = self.idx(id) {
                        self.perms[i].temp_kw |= CANT_BLOCK;
                    }
                }
                self.recalc();
            }
            // "Fblthp's owner shuffles him into their library".
            "shuffles itself into its library" => {
                if let Some(i) = self.src_idx(ctx) {
                    let owner = self.perms[i].owner as usize;
                    self.leave(i, Zone::Library(false), false);
                    let mut lib = std::mem::take(&mut self.players[owner].library);
                    self.rng.shuffle(&mut lib);
                    self.players[owner].library = lib;
                }
            }
            // Wishclaw Talisman: the least threatening opponent gets it.
            "gives itself to an opponent" => {
                let to = self.opponents(me).into_iter().min_by(|&a, &b| {
                    ai::threat(self, a)
                        .partial_cmp(&ai::threat(self, b))
                        .unwrap()
                });
                if let (Some(i), Some(o)) = (self.src_idx(ctx), to) {
                    self.perms[i].ctrl = o as u8;
                    self.recalc();
                }
            }
            // Tinybones: the discarded card goes to exile with a stash counter, playable by
            // us on our turns.
            "you get an emblem" => {
                // The emblem abilities of the loyalty ability resolving.
                let Some(r) = ctx.card_ref else { return };
                let Some(k) = self.card(r).abilities.iter().position(|a| a.text == text) else {
                    return;
                };
                let ks: Vec<u16> = self
                    .card(r)
                    .emblems
                    .iter()
                    .enumerate()
                    .filter(|(_, (from, _))| *from as usize == k)
                    .map(|(j, _)| j as u16)
                    .collect();
                for j in ks {
                    self.players[me].emblems.push((r, j));
                }
                self.recalc();
            }
            // "Up to one target creature gets -4/-1 until your next turn" (Garruk).
            "gets +N/+N until your next turn" => {
                let Some(pt) = text.split(" gets ").nth(1) else {
                    return;
                };
                let pt = pt.split(' ').next().unwrap_or("");
                let Some((p, t)) = pt.split_once('/') else {
                    return;
                };
                let num = |s: &str| s.trim_start_matches('+').parse::<i16>().ok();
                let (Some(dp), Some(dt)) = (num(p), num(t)) else {
                    return;
                };
                let harmful = dp + dt < 0;
                let t = Target {
                    kind: Kind::Creature,
                    whose: if harmful { Whose::Theirs } else { Whose::Yours },
                    mass: false,
                    other: false,
                    restricted: false,
                    restr: Default::default(),
                    count: 1,
                };
                for id in self.choose(ctx, &t, harmful) {
                    if let Some(i) = self.idx(id) {
                        self.perms[i].till = Some((dp, dt, me as u8));
                    }
                }
                self.recalc();
            }
            // (The card was exiled as it was discarded: see `Game::emit`.)
            "stashes the discarded card" => {}
            // The Sentry: The Void, a 5/5 flying indestructible token, for the opponent it
            // helps least.
            "an opponent creates a creature token" => {
                let Some(o) = self.opponents(me).into_iter().min_by(|&a, &b| {
                    ai::threat(self, a)
                        .partial_cmp(&ai::threat(self, b))
                        .unwrap()
                }) else {
                    return;
                };
                let rest = text.split("creates ").nth(1).unwrap_or("");
                let (p, t) = rest
                    .split_whitespace()
                    .find_map(|w| {
                        let (p, t) = w.split_once('/')?;
                        Some((p.parse().ok()?, t.trim_end_matches(',').parse().ok()?))
                    })
                    .unwrap_or((1, 1));
                let with = rest.split(" with ").nth(1).unwrap_or("");
                let with = with.split('"').next().unwrap_or("");
                let kw = KEYWORDS
                    .iter()
                    .filter(|(k, _)| with.contains(k))
                    .fold(0, |m, (_, b)| m | b);
                let i = self.make_token(o, p, t, kw, token_subtypes(text));
                self.after_enter(i);
            }
            // The permanent itself becomes a creature (for good, or until end of turn).
            "becomes a creature" => {
                let Some(i) = self.src_idx(ctx) else { return };
                if self.perms[i].is_creature() && text.contains("isn't a creature") {
                    return;
                }
                let rest = text.split(" become").nth(1).unwrap_or("");
                let mut pt = rest
                    .split_whitespace()
                    .find_map(|w| {
                        let (p, t) = w.split_once('/')?;
                        Some((p.parse::<i16>().ok()?, t.parse::<i16>().ok()?))
                    })
                    .unwrap_or((0, 0));
                // "Power and toughness are each equal to the number of lands you control":
                // counted now. ("+1/+1 for each artifact" is the ability's own pump.)
                if let Some(what) = rest.split("equal to the number of ").nth(1) {
                    let n = self.count_of(me, ctx.src, what).unwrap_or(0) as i16;
                    pt = (n, n);
                }
                let with = rest.split(" with ").nth(1).unwrap_or("");
                let with = with.split('"').next().unwrap_or("");
                let kw = KEYWORDS
                    .iter()
                    .filter(|(k, _)| with.contains(k))
                    .fold(0, |m, (_, b)| m | b);
                let x = &mut self.perms[i];
                // An Equipment that becomes a creature falls off (CR 301.5c).
                if x.is(T_EQUIPMENT) {
                    x.attached = None;
                }
                if text.contains("until end of turn") {
                    x.temp_creature = true;
                    x.temp_base = Some(pt);
                    x.temp_kw |= kw;
                } else {
                    x.types |= T_CREATURE;
                    x.base_p = pt.0;
                    x.base_t = pt.1;
                    x.kw |= kw;
                }
                self.recalc();
            }
            "assigns combat damage by toughness" => {
                self.players[me].toughness_damage_turn = true;
            }
            "returns when it dies" => {
                let t = Target {
                    kind: Kind::Creature,
                    whose: Whose::Yours,
                    mass: false,
                    other: false,
                    restricted: false,
                    restr: Default::default(),
                    count: 1,
                };
                for id in self.choose(ctx, &t, false) {
                    if let Some(i) = self.idx(id) {
                        self.perms[i].returns_on_death = RETURNS
                            | if text.contains("with a +1/+1 counter") {
                                RETURNS_COUNTER
                            } else {
                                0
                            }
                            | if text.contains("create a treasure") {
                                RETURNS_TREASURE
                            } else {
                                0
                            };
                    }
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
            "you lose the game" => {
                if !self.cant_lose(me) {
                    self.players[me].alive = false;
                }
            }
            "discards your hand" | "each player discards their hand" => {
                let who: Vec<usize> = if label.starts_with("each") {
                    (0..self.players.len())
                        .filter(|&o| self.players[o].alive)
                        .collect()
                } else {
                    vec![me]
                };
                for o in who {
                    let hand = std::mem::take(&mut self.players[o].hand);
                    if o == me {
                        self.last_discarded = hand.len() as i32;
                    }
                    for c in hand {
                        self.players[o].graveyard.push(c);
                        self.emit(Ev::Discard, o, None);
                    }
                }
            }
            "you lose life equal to it" => {
                // "That permanent's mana value": what the spell just removed.
                let n = self.last_left.as_ref().map_or(0, |x| {
                    if text.contains("mana value") {
                        x.card.map_or(0, |r| self.card(r).mv as i32)
                    } else if text.contains("toughness") {
                        x.t as i32
                    } else {
                        x.p as i32
                    }
                });
                self.lose_life(me, n);
            }
            "its controller gains life equal to its power" => {
                if let Some(x) = self.last_left.clone() {
                    self.gain_life(x.ctrl as usize, x.p.max(0) as i32);
                }
            }
            l if l.starts_with("its controller gets a ") => {
                // The token goes to whoever controlled what was just destroyed or exiled.
                let pt: Vec<i32> = l
                    .trim_start_matches("its controller gets a ")
                    .trim_end_matches(" token")
                    .split('/')
                    .filter_map(|d| d.parse().ok())
                    .collect();
                if let (Some(x), [p, t]) = (self.last_left.clone(), pt.as_slice()) {
                    let o = x.ctrl as usize;
                    if self.players.get(o).is_some_and(|pl| pl.alive) {
                        let i = self.make_token(o, *p, *t, 0, token_subtypes(text));
                        self.after_enter(i);
                    }
                }
            }
            "exiles a graveyard" | "exiles a graveyard card" => {
                // The opponent whose graveyard matters most (the most creature cards).
                let creatures = |g: &Self, o: usize| {
                    g.players[o]
                        .graveyard
                        .iter()
                        .filter(|&&c| g.players[o].cards[c as usize].is(T_CREATURE))
                        .count()
                };
                let Some(o) = self
                    .opponents(me)
                    .into_iter()
                    .max_by_key(|&o| (creatures(self, o), self.players[o].graveyard.len()))
                else {
                    return;
                };
                if label == "exiles a graveyard" {
                    let gy = std::mem::take(&mut self.players[o].graveyard);
                    self.players[o].exile.extend(gy);
                } else {
                    let n = if text.contains("up to two") { 2 } else { 1 };
                    for _ in 0..n {
                        let pl = &self.players[o];
                        let best = (0..pl.graveyard.len()).max_by(|&a, &b| {
                            let va = pl.cards[pl.graveyard[a] as usize].value;
                            let vb = pl.cards[pl.graveyard[b] as usize].value;
                            va.partial_cmp(&vb).unwrap()
                        });
                        if let Some(k) = best {
                            let c = self.players[o].graveyard.remove(k);
                            self.players[o].exile.push(c);
                        }
                    }
                }
                self.emit(Ev::LeavesGraveyard, o, None);
            }
            "extra land drop" => {
                self.players[me].extra_lands = self.players[me].extra_lands.saturating_add(1);
            }
            "doubles counters" => {
                // "On ~" / "on this creature": itself; "on each of those creatures": the ones
                // that just got counters; "up to two target creatures": our best two.
                let ids: Vec<u32> = if text.contains("counters on ~") {
                    self.src_idx(ctx)
                        .map(|i| self.perms[i].id)
                        .into_iter()
                        .collect()
                } else if text.contains("those creatures") {
                    self.last_countered.clone()
                } else {
                    let mut mine: Vec<&Perm> = self
                        .perms
                        .iter()
                        .filter(|x| x.ctrl as usize == me && x.counters > 0)
                        .collect();
                    mine.sort_by_key(|x| -x.counters);
                    mine.iter().take(2).map(|x| x.id).collect()
                };
                for id in ids {
                    if let Some(i) = self.idx(id) {
                        let n = self.perms[i].counters.max(0) as i32;
                        if n > 0 {
                            self.add_counters(i, n);
                        }
                    }
                }
            }
            "taps itself" => {
                if let Some(i) = self.src_idx(ctx).filter(|&i| !self.perms[i].tapped) {
                    self.perms[i].tapped = true;
                    let id = self.perms[i].id;
                    self.emit(Ev::Tapped, me, Some(id));
                }
            }
            "sacrifices itself" => {
                if let Some(i) = self.src_idx(ctx) {
                    self.sacrifice(i);
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
                // "Put ~ from your graveyard on top of your library": only from there.
                if text.contains("from your graveyard") {
                    let r = ctx
                        .snap
                        .as_ref()
                        .and_then(|s| s.card)
                        .or_else(|| self.src_idx(ctx).and_then(|i| self.perms[i].card))
                        .or(ctx.card_ref);
                    let on_battlefield = self.src_idx(ctx).is_some();
                    if let (Some(r), false) = (r, on_battlefield) {
                        let o = r.0 as usize;
                        if let Some(k) = self.players[o].graveyard.iter().rposition(|&c| c == r.1) {
                            let c = self.players[o].graveyard.remove(k);
                            self.players[o].library.push(c);
                        }
                    }
                } else if let Some(i) = self.src_idx(ctx) {
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
            // Pain lands, Ancient Tomb, Sulfuric Vortex hitting its own controller.
            "damage to you" => {
                let n = text
                    .split(" damage to you")
                    .next()
                    .and_then(|h| h.rsplit(' ').next())
                    .and_then(|d| d.parse().ok())
                    .unwrap_or(1);
                self.lose_life(me, n);
            }
            l if l.starts_with("you lose ") => {
                if let Some(n) = l.split_whitespace().nth(2).and_then(|d| d.parse().ok()) {
                    self.lose_life(me, n);
                }
            }
            _ => {}
        }
    }
}

/// An impulse draw ("exile the top two cards of your library. Choose one of them. Until the
/// end of your next turn, you may play that card").
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Impulse {
    /// How many cards (0: X).
    pub n: usize,
    pub choose_one: bool,
    /// "Until the end of your next turn".
    pub next_turn: bool,
    /// "For as long as they remain exiled".
    pub lasting: bool,
    /// A card from your graveyard (Thor), not the library.
    pub from_graveyard: bool,
    /// The top of an opponent's library (Inside Information).
    pub opponent: bool,
    pub pay_life: bool,
}

/// Reads an impulse draw (None for other "look at the top" texts).
pub fn impulse_of(text: &str) -> Option<Impulse> {
    let plays = text.contains("you may play")
        || text.contains("you may cast that card")
        || text.contains("you may cast it this turn");
    if !plays || !text.contains("exile") || text.contains("without paying") {
        return None;
    }
    let from_graveyard = text.contains("card from your graveyard");
    let n = if from_graveyard {
        1
    } else {
        let w = text
            .split("exile the top ")
            .nth(1)
            .or_else(|| text.split("look at the top ").nth(1))?
            .split_whitespace()
            .next()?;
        match w {
            "card" => 1,
            "two" => 2,
            "three" => 3,
            "four" => 4,
            "five" => 5,
            "x" => 0,
            d => d.parse().ok()?,
        }
    };
    Some(Impulse {
        n,
        choose_one: text.contains("choose one"),
        next_turn: text.contains("end of your next turn"),
        lasting: text.contains("for as long as"),
        from_graveyard,
        opponent: text.contains("opponent's library"),
        pay_life: text.contains("pay life equal to its mana value"),
    })
}

/// Free spells from the top of libraries: whose, how many cards, how many spells.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FreeTop {
    /// 0: yours, 1: a target opponent's, 2: each player's.
    pub whose: u8,
    pub n: usize,
    pub spells: usize,
    /// "With mana value less than or equal to the greatest power among attacking creatures
    /// you control".
    pub attacking_power: bool,
    /// The rest go to the bottom of the library (otherwise they stay exiled).
    pub rest_bottom: bool,
}

pub fn free_from_top_of(text: &str) -> Option<FreeTop> {
    if !text.contains("without paying")
        || !(text.contains("from among them")
            || text.contains("from among those cards")
            || text.contains("from among the exiled cards"))
    {
        return None;
    }
    let w = text.split("the top ").nth(1)?.split_whitespace().next()?;
    let n = match w {
        "card" => 1,
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        d => d.parse().ok()?,
    };
    Some(FreeTop {
        whose: if text.contains("each player's library") {
            2
        } else if text.contains("target opponent exiles the top")
            || text.contains("target opponent's library")
        {
            1
        } else {
            0
        },
        n,
        spells: if text.contains("any number of spells") {
            usize::MAX
        } else if text.contains("up to two spells") {
            2
        } else if text.contains("up to three") {
            3
        } else {
            1
        },
        attacking_power: text.contains("greatest power among attacking creatures"),
        rest_bottom: text.contains("rest on the bottom"),
    })
}

/// "Has base power and toughness 4/5", "base power and toughness become 6/6".
fn base_pt_of(text: &str) -> Option<(i16, i16)> {
    let rest = text.split("base power and toughness ").nth(1)?;
    let rest = rest
        .trim_start_matches("become ")
        .trim_start_matches("becomes ");
    let pt = rest.split([' ', '.', ',']).next()?;
    let (p, t) = pt.split_once('/')?;
    Some((p.parse().ok()?, t.parse().ok()?))
}

impl<'a> Game<'a> {
    /// Casts the best spells from the top of the libraries `spec` names, free; the rest
    /// stay exiled or go to the bottom.
    fn free_from_top(&mut self, ctx: &Ctx, spec: FreeTop) {
        let me = ctx.ctrl;
        let from: Vec<usize> = match spec.whose {
            0 => vec![me],
            1 => ai::choose_opponent(self, me, 0).into_iter().collect(),
            _ => (0..self.players.len())
                .filter(|&o| self.players[o].alive)
                .collect(),
        };
        let limit = if spec.attacking_power {
            self.perms
                .iter()
                .filter(|x| x.ctrl as usize == me && x.attacking)
                .map(|x| x.p.max(0) as u8)
                .max()
                .unwrap_or(0)
        } else {
            u8::MAX
        };
        let mut seen: Vec<CardRef> = Vec::new();
        for o in from {
            for _ in 0..spec.n {
                if let Some(c) = self.players[o].library.pop() {
                    self.players[o].exile.push(c);
                    seen.push((o as u8, c));
                }
            }
        }
        let mut picks: Vec<CardRef> = seen
            .iter()
            .copied()
            .filter(|&r| {
                let c = self.card(r);
                !c.is(T_LAND) && c.mv <= limit
            })
            .collect();
        picks.sort_by(|&a, &b| self.card(b).value.partial_cmp(&self.card(a).value).unwrap());
        picks.truncate(spec.spells);
        for r in &picks {
            self.players[me].playable.push(Playable {
                card: *r,
                until: (self.round, self.active as u8),
                own_turn: false,
                pay_life: false,
            });
            let k = self.players[me].playable.len() - 1;
            self.cast(
                me,
                super::Src::Exile(k),
                super::CastOpts {
                    free: true,
                    ..Default::default()
                },
            );
            self.players[me].playable.retain(|x| x.card != *r);
        }
        if spec.rest_bottom {
            for r in seen.into_iter().filter(|r| !picks.contains(r)) {
                let pl = &mut self.players[r.0 as usize];
                if let Some(e) = pl.exile.iter().rposition(|&c| c == r.1) {
                    pl.exile.remove(e);
                    pl.library.insert(0, r.1);
                }
            }
        }
    }

    /// When a card made playable now stops being playable: the end of this turn, of
    /// `p`'s next turn, or never.
    pub fn play_until(&self, p: usize, next_turn: bool, lasting: bool) -> (u32, u8) {
        if lasting {
            (u32::MAX, p as u8)
        } else if !next_turn {
            (self.round, self.active as u8)
        } else if self.active == p || p < self.active {
            (self.round + 1, p as u8)
        } else {
            (self.round, p as u8)
        }
    }

    /// Exiles the impulse draw's cards and makes them playable (the best one, if only one
    /// of them may be played).
    fn impulse(&mut self, ctx: &Ctx, imp: Impulse) {
        let me = ctx.ctrl;
        let until = self.play_until(me, imp.next_turn, imp.lasting);
        if imp.from_graveyard {
            // "Exile target Equipment, instant, or sorcery card from your graveyard" (Thor),
            // "exile that card from your graveyard" (Moonstone: the card just discarded).
            let pl = &self.players[me];
            let d = self.dyn_values(&ctx.card_restr, Some(ctx), me, ctx.src);
            let that = ctx.text.contains("exile that card");
            let best = (0..pl.graveyard.len())
                .filter(|&k| !that || k + 1 == pl.graveyard.len())
                .filter(|&k| {
                    let c = &pl.cards[pl.graveyard[k] as usize];
                    Game::card_ok(&ctx.card_restr, c, &d)
                })
                .max_by(|&a, &b| {
                    let v = |k: usize| pl.cards[pl.graveyard[k] as usize].value;
                    v(a).partial_cmp(&v(b)).unwrap()
                });
            if let Some(k) = best {
                let c = self.players[me].graveyard.remove(k);
                self.players[me].exile.push(c);
                self.players[me].playable.push(Playable {
                    card: (me as u8, c),
                    until,
                    own_turn: false,
                    pay_life: false,
                });
                self.emit(Ev::LeavesGraveyard, me, None);
            }
            return;
        }
        let from = if imp.opponent {
            match ai::choose_opponent(self, me, 0) {
                Some(o) => o,
                None => return,
            }
        } else {
            me
        };
        let n = if imp.n == 0 {
            // X paid for it, or "where X is ~'s power".
            match ctx.x.or(self.cur_x) {
                Some(x) if !ctx.text.contains("where x is") => x.max(0) as usize,
                _ => self.x_value(ctx, &Amount { n: 3, x: true }).max(0) as usize,
            }
        } else {
            imp.n
        };
        let mut cards = Vec::new();
        for _ in 0..n {
            match self.players[from].library.pop() {
                Some(c) => {
                    self.players[from].exile.push(c);
                    cards.push(c);
                }
                None => break,
            }
        }
        if imp.choose_one && cards.len() > 1 {
            let pl = &self.players[from];
            let best = cards.iter().copied().max_by(|&a, &b| {
                let v = |c: u16| pl.cards[c as usize].value;
                v(a).partial_cmp(&v(b)).unwrap()
            });
            cards = best.into_iter().collect();
        }
        if let Some(&c) = cards.first() {
            self.focus_card = Some((from as u8, c));
        }
        for c in cards {
            self.players[me].playable.push(Playable {
                card: (from as u8, c),
                until,
                own_turn: false,
                pay_life: imp.pay_life,
            });
        }
    }

    /// A transforming double-faced permanent turns into its back face (CR 712): its
    /// characteristics are the back's; counters, damage and attachments stay.
    pub fn transform(&mut self, i: usize) {
        let x = &self.perms[i];
        let back = x
            .card
            .and_then(|r| self.card(r).back.as_ref())
            .filter(|b| b.kind == BackKind::Transform && !x.transformed);
        let Some(b) = back else { return };
        let mut colors = 0;
        for (k, &n) in b.cost.pips.iter().enumerate() {
            if n > 0 {
                colors |= 1 << k;
            }
        }
        let ctrl = x.ctrl;
        let x = &mut self.perms[i];
        x.transformed = true;
        x.base_p = b.power;
        x.base_t = b.toughness;
        x.kw = b.kw;
        x.types = b.types | (x.types & T_TOKEN);
        x.subtypes = b.subtypes.clone();
        if colors != 0 {
            x.colors = colors;
        }
        self.note(|| format!("P{} transforms it into {}", ctrl, b.name));
        self.recalc();
    }

    /// "Target creature has base power and toughness 1/1 until end of turn", "~'s base power
    /// and toughness become 4/2 until end of turn", "its base power and toughness become equal
    /// to Galion's power and toughness".
    fn set_base_pt(&mut self, ctx: &Ctx) {
        let text = ctx.text;
        let pt = match base_pt_of(text) {
            Some(pt) => pt,
            None => match self.src_perm(ctx) {
                Some(s) => (s.p, s.t),
                None => return,
            },
        };
        let ids: Vec<u32> = if text.contains("target creature") {
            // Small (1/1, 0/0) is for an opponent's creature; big for ours, the one it
            // grows the most.
            let harmful = pt.0 + pt.1 <= 2;
            let t = Target {
                kind: Kind::Creature,
                whose: if harmful { Whose::Theirs } else { Whose::Yours },
                mass: false,
                other: text.contains("other target creature"),
                restricted: false,
                restr: Default::default(),
                count: 1,
            };
            if harmful {
                self.choose(ctx, &t, true)
            } else {
                let grows = |x: &Perm| {
                    let (bp, bt) = x.temp_base.unwrap_or((x.base_p, x.base_t));
                    (pt.0 + pt.1) - (bp + bt)
                };
                let best = self
                    .candidates(ctx, &t, false)
                    .into_iter()
                    .filter(|&i| grows(&self.perms[i]) > 0)
                    .max_by_key(|&i| grows(&self.perms[i]))
                    .map(|i| self.perms[i].id);
                self.last_targets = best.into_iter().collect();
                self.targeted = true;
                best.into_iter().collect()
            }
        } else {
            self.src_idx(ctx)
                .map(|i| self.perms[i].id)
                .into_iter()
                .collect()
        };
        for id in ids {
            let Some(i) = self.idx(id) else { continue };
            let x = &mut self.perms[i];
            if text.contains("until end of turn") || ctx.until_eot {
                x.temp_base = Some(pt);
            } else {
                x.base_p = pt.0;
                x.base_t = pt.1;
            }
        }
        self.recalc();
    }
}

/// Does the text use an X ("draw X cards", "up to X target")?
fn has_x(t: &str) -> bool {
    t.split(|c: char| !c.is_alphanumeric()).any(|w| w == "x")
}

/// "Among artifacts you control": the permanent has the named card type (any if none).
fn type_ok(x: &Perm, phrase: &str) -> bool {
    let mut any = false;
    for (word, bit) in [
        ("creature", T_CREATURE),
        ("artifact", T_ARTIFACT),
        ("enchantment", T_ENCHANTMENT),
        ("permanent", 0xFFFF),
    ] {
        if phrase.contains(word) {
            any = true;
            if x.is(bit) {
                return true;
            }
        }
    }
    !any
}

/// "Draw two cards, then discard a card" -> (2, 1); X draws are -1.
fn draw_then_discard(text: &str) -> Option<(i32, i32)> {
    let word = |w: &str| -> Option<i32> {
        Some(match w {
            "a" | "one" => 1,
            "two" => 2,
            "three" => 3,
            "four" => 4,
            "five" => 5,
            "x" => -1,
            d => d.parse().ok()?,
        })
    };
    let i = text.find("draw")?;
    let rest = &text[i..];
    let (j, key) = ["then discard", "if you do, discard"]
        .iter()
        .find_map(|k| rest.find(k).map(|j| (j, k.len())))?;
    let (draw_part, disc_part) = (&rest[..j], &rest[j + key..]);
    if draw_part.contains('.') && !draw_part.trim_end().ends_with('.') {
        return None;
    }
    let mut words = draw_part.split_whitespace();
    words.next(); // "draw" / "draws"
    let d = if draw_part.contains("cards equal to") || draw_part.contains("where x is") {
        -1
    } else {
        word(words.next()?)?
    };
    let k = word(disc_part.split_whitespace().next()?.trim_start_matches('s'))?;
    (d != 1 || k != 1 || draw_part.contains("cards equal to")).then_some((d, k))
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
