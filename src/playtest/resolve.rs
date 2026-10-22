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
    /// Cast using teamwork ("choose both instead", "any number ... instead").
    pub teamwork: bool,
    /// The player the triggering event was about ("that player loses 5 life").
    pub about: Option<usize>,
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
    "exiles opponents' libraries",
    "mountains make an extra red mana",
    "exchanges control",
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
    "spells cost less this turn",
    "attacks change until your next turn",
    "the next spell this turn is free",
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
        // What X is: "where X is ...", "equal to ...", or a count "for each ...". A sentence
        // read as an ability of its own ("draw X cards, where X is the number of cards
        // discarded this way. If you have an enduring story, ~ deals X damage") has its X
        // defined in the sentence it was split from.
        let measure = t
            .split("where x is ")
            .nth(1)
            .or_else(|| t.split(" x is ").nth(1))
            .or_else(|| t.split(" equal to ").nth(1))
            .or_else(|| {
                let head = trigger_part(t);
                ctx.card?
                    .abilities
                    .iter()
                    .filter(|a| a.text != t && has_x(t) && trigger_part(&a.text) == head)
                    .find_map(|a| a.text.split("where x is ").nth(1))
            });
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
                // ("Target opponent sacrifices a creature. You gain life equal to that
                // creature's toughness": the one that just left.)
                subject_or_src(&|x| x.t as i32)
                    .or_else(|| self.last_left.as_ref().map(|x| x.t as i32))
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
        // "A +1/+1 counter for each of that spell's colors" (Ramos): colourless is none.
        if p.contains("that spell's color") {
            return Some(
                self.ext
                    .cast_probe
                    .as_ref()
                    .map_or(0, |x| (x.colors & 0b11111).count_ones() as i32),
            );
        }
        if p.contains("destroyed this way") {
            return Some(self.last_destroyed);
        }
        if p.contains("returned to your hand this way") {
            return Some(self.ext.last_bounced);
        }
        // "Cards that were put into target player's graveyard from their library this turn".
        if p.contains("graveyard from their library this turn") {
            return Some(
                (0..self.players.len())
                    .filter(|&o| alive(o))
                    .map(|o| self.players[o].px.milled_turn as i32)
                    .max()
                    .unwrap_or(0),
            );
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
        // "For each fellowship counter on ~": its other counters.
        if p.ends_with(" counter on ~") || p.ends_with(" counters on ~") {
            return Some(src_perm.map_or(0, |x| x.other_counters.max(0) as i32));
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
        // "Each creature target opponent controls": the opponent with the most.
        if let Some(what) = p.strip_suffix(" target opponent controls") {
            return self
                .opponents(me)
                .into_iter()
                .filter_map(|o| self.count_of(o, None, &format!("{} you control", what)))
                .max();
        }
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
        // "equipped creature" / "enchanted creature": what the source is attached to ("target
        // creature other than enchanted creature": anything but that).
        let attached = self.src_perm(ctx).and_then(|s| s.attached);
        let other_than = text.contains("other than enchanted creature")
            || text.contains("other than equipped creature");
        // ("Whenever equipped creature attacks, tap target creature defending player
        // controls": the trigger names it, the effect doesn't.)
        let body = &text[trigger_part(text).len()..];
        if (body.contains("equipped creature") || body.contains("enchanted creature"))
            && !t.mass
            && !other_than
        {
            if let Some(a) = attached {
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
        // "Tap target creature defending player controls": the player the attacker (the one
        // the trigger is about) attacks.
        let defending = if text.contains("defending player") && !self.ext.attack_def.is_empty() {
            let ad = &self.ext.attack_def;
            ctx.subject
                .or(src_id)
                .and_then(|s| ad.iter().find(|a| a.0 == s))
                .or_else(|| ad.iter().all(|a| a.1 == ad[0].1).then(|| &ad[0]))
                .map(|a| a.1 as usize)
        } else {
            None
        };
        // A spell cast for less because it targets a tapped (attacking) creature targets one.
        let must = if src_id.is_none() && harmful && !t.mass {
            self.ext.must_target
        } else {
            0
        };
        let mut out = Vec::new();
        for (i, x) in self.perms.iter().enumerate() {
            if t.kind == Kind::Artifact && text.contains("target equipment") && !x.is(T_EQUIPMENT) {
                continue;
            }
            if defending.is_some_and(|d| x.ctrl as usize != d) {
                continue;
            }
            match must {
                1 if !x.tapped => continue,
                2 if !x.attacking => continue,
                3 if !x.attacking || x.is_token() => continue,
                _ => {}
            }
            if other_than && Some(x.id) == attached {
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
        // "Choose target creature you control. Until end of turn, double its power ... and it
        // gains trample" (World War Hulk): "it"/"its" is the chosen creature, whichever effect
        // comes first.
        if ctx.text.contains("choose target creature you control.") && !t.mass {
            let alive: Vec<u32> = self
                .last_targets
                .iter()
                .copied()
                .filter(|&id| self.idx(id).is_some())
                .collect();
            if !alive.is_empty() {
                return alive;
            }
            let mine = Target {
                kind: Kind::Creature,
                whose: Whose::Yours,
                mass: false,
                other: false,
                restricted: false,
                restr: Default::default(),
                count: 1,
            };
            return self.choose(ctx, &mine, false);
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
                if self.auditing() {
                    let ids: Vec<u32> = picks.iter().map(|&i| self.perms[i].id).collect();
                    self.audit_targets(ctx.ctrl, &ids, harmful, &cands);
                }
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
        if self.auditing() {
            self.audit_targets(ctx.ctrl, &ids, harmful, &cands);
        }
        let mut out = Vec::new();
        for id in ids {
            let Some(i) = self.idx(id) else { continue };
            let owner = self.perms[i].ctrl as usize;
            if owner != ctx.ctrl {
                if !self.pay_ward(ctx.ctrl, i) {
                    self.note(|| "    (the target's ward isn't paid: it's countered)".into());
                    self.ext.ward_countered = true;
                    return Vec::new();
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
        let cause = if pend.card.1 == NO_CARD {
            super::audit::Cause::Token(pend.ctrl)
        } else {
            super::audit::Cause::Card(pend.card, pend.ctrl)
        };
        let outer = std::mem::replace(&mut self.cause, cause);
        self.resolve_pending_inner(pend);
        self.cause = outer;
    }

    fn resolve_pending_inner(&mut self, pend: Pending) {
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
        // "Put a +1/+1 counter on him. When you do, he deals damage ..." (Red Hulk): no counter
        // can go on a creature that has left the battlefield, so nothing happens.
        if src_idx.is_none()
            && pend.source.is_some()
            && ab.text.contains(". when you do,")
            && [
                "counter on him",
                "counter on her",
                "counter on ~",
                "counter on it",
            ]
            .iter()
            .any(|w| {
                ab.text
                    .split(". when you do,")
                    .next()
                    .unwrap_or("")
                    .contains(w)
            })
        {
            return;
        }
        // The creature the event was about, as it is or as it last was (if it died).
        let subject = pend.subject.and_then(|id| {
            self.idx(id)
                .map(|i| self.perms[i].clone())
                .or_else(|| self.dead.iter().rev().find(|x| x.id == id).cloned())
        });
        let source = src_idx
            .map(|i| self.perms[i].clone())
            .or_else(|| pend.snapshot.clone());
        // "If this is the second time this ability has resolved this turn": count it.
        let nth = if ab.text.contains("time this ability has resolved this turn") {
            let key = (
                pend.source.unwrap_or(u32::MAX),
                pend.ability,
                self.ext.turn_no,
            );
            let r = &mut self.ext.resolutions;
            r.retain(|e| e.2 == key.2);
            match r.iter_mut().find(|e| (e.0, e.1, e.2) == key) {
                Some(e) => {
                    e.3 = e.3.saturating_add(1);
                    e.3
                }
                None => {
                    r.push((key.0, key.1, key.2, 1));
                    1
                }
            }
        } else {
            0
        };
        // Who a cast trigger's "that player" is (the spell's stand-in carries its caster).
        let about = pend.subject.and_then(|id| {
            if id == u32::MAX {
                self.ext.cast_probe.as_ref().map(|x| x.ctrl as usize)
            } else if let Some(o) = subject_player(id) {
                Some(o)
            } else {
                subject.as_ref().map(|x| x.ctrl as usize)
            }
        });
        let eval = |c: &Cond| match c {
            Cond::NthResolution(n) => nth == *n,
            Cond::HadOtherCounters => source.as_ref().is_some_and(|s| s.other_counters > 0),
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
            teamwork: pend.teamwork,
            about,
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
            // ("You may discard your hand. ... X is the number of cards discarded this way":
            // none.)
            if ab.text.contains("discard your hand") {
                self.last_discarded = 0;
            }
            return;
        }
        self.last_targets.clear();
        self.last_ctrl = None;
        self.targeted = false;
        // (effect, the text and card restriction it resolves with): a mode reads its own.
        let mut chosen_modes: Option<String> = None;
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
            let mut modes = ai::choose_modes(self, &ctx, ab, used);
            // "You may pay {1} up to three times. When you do, choose up to that many"
            // (Hawkeye's Trick Arrows): {1} per mode, the ones worth it; none unpaid.
            if let Some(rest) = ab.text.split("you may pay {").nth(1) {
                if ab.text.contains("up to three times") || ab.text.contains("up to two times") {
                    let n: u32 = rest
                        .split('}')
                        .next()
                        .and_then(|d| d.parse().ok())
                        .unwrap_or(1);
                    let most = if ab.text.contains("up to three times") {
                        3
                    } else {
                        2
                    };
                    let mut ranked: Vec<(f32, usize)> = groups
                        .iter()
                        .enumerate()
                        .map(|(m, r)| {
                            let (t, cr) = ab.mode_text(m);
                            let v: f32 = ab.effects[r.clone()]
                                .iter()
                                .map(|e| ai::effect_now(self, me, e, t, ctx.src, cr))
                                .sum();
                            (v, m)
                        })
                        .filter(|(v, _)| *v >= 0.5 * n as f32)
                        .collect();
                    ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
                    modes.clear();
                    // Only what beats the spells that mana would cast later this turn.
                    let vals: Vec<f32> = ranked.iter().take(most).map(|r| r.0).collect();
                    let most = ai::paid_modes(self, me, &vals, n);
                    for (_, m) in ranked.into_iter().take(most) {
                        if !self.pay(me, &Mana::generic(n)) {
                            break;
                        }
                        modes.push(m);
                    }
                    modes.sort_unstable();
                }
            }
            if self.log.is_some() {
                let texts: Vec<String> = modes
                    .iter()
                    .map(|&m| match ab.mode_texts.get(m) {
                        // (Without the ability's own text it starts with.)
                        Some(t) => {
                            let same = t
                                .char_indices()
                                .zip(ab.text.chars())
                                .take_while(|((_, a), b)| a == b)
                                .last()
                                .map_or(0, |((i, a), _)| i + a.len_utf8());
                            t[same..].trim_start_matches([' ', '—', '-']).to_string()
                        }
                        None => format!("mode {}", m + 1),
                    })
                    .collect();
                chosen_modes = Some(texts.join(" + "));
            }
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
                "P{} resolves {}: {}{}",
                me,
                card.map(|c| c.name.as_str()).unwrap_or("token"),
                ab.text,
                chosen_modes
                    .map(|m| format!(" => chosen: {}", m))
                    .unwrap_or_default()
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
        self.ext.ward_countered = false;
        for (k, text, card_restr) in picks {
            if !self.players[me].alive {
                return;
            }
            // A target's ward wasn't paid: the whole spell or ability is countered (CR
            // 702.21a).
            if std::mem::take(&mut self.ext.ward_countered) {
                return;
            }
            // "You gain 1 life if this is the first time this ability has resolved this
            // turn. If it's the second time, draw a card. ...": only that time's part.
            if nth > 0 && ordinal_of_effect(&ab.text, k).is_some_and(|o| o != nth) {
                continue;
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
            // Bifur: the Dwarf's ability triggered an additional time.
            if self.triggers_twice(&pend) {
                self.resolve_pending(pend.clone());
                self.sba();
            }
            self.resolve_pending(pend);
            self.sba();
        }
    }

    /// The ability a pending trigger or activation would resolve.
    pub(crate) fn pending_ability(&self, pend: &Pending) -> Option<&'a SimAbility> {
        if pend.card.1 == NO_CARD {
            let perm = pend.snapshot.as_ref().or_else(|| {
                pend.source
                    .and_then(|id| self.idx(id))
                    .map(|i| &self.perms[i])
            })?;
            return self.abilities_of(perm).get(pend.ability as usize);
        }
        let c = self.card(pend.card);
        if pend.ability >= EMBLEM_BASE {
            return c
                .emblems
                .get((pend.ability - EMBLEM_BASE) as usize)
                .map(|e| &e.1);
        }
        let transformed = pend
            .source
            .and_then(|id| self.idx(id))
            .map(|i| &self.perms[i])
            .or(pend.snapshot.as_ref())
            .is_some_and(|x| x.transformed && x.card == Some(pend.card));
        match (&c.back, pend.back || transformed) {
            (Some(b), true) => b.abilities.get(pend.ability as usize),
            _ => c.abilities.get(pend.ability as usize),
        }
    }

    /// "As long as you have an enduring story, if an ability of a Dwarf you control triggers,
    /// that ability triggers an additional time" (Bifur, Melodic Rider): is `pend` such a
    /// triggered ability?
    fn triggers_twice(&self, pend: &Pending) -> bool {
        let p = pend.ctrl as usize;
        if self.rules[p % 8] & R_DWARF_TRIGGERS_TWICE == 0
            || !self.players[p].enduring_story
            || pend.card.1 == NO_CARD
            || pend.ability >= EMBLEM_BASE
            || pend.from_graveyard
        {
            return false;
        }
        // The source as it is, or as it last was (a "dies" trigger looks back).
        let Some(src) = pend
            .source
            .and_then(|id| self.idx(id))
            .map(|i| &self.perms[i])
            .or(pend.snapshot.as_ref())
            .or_else(|| {
                pend.source
                    .and_then(|id| self.dead.iter().rev().find(|x| x.id == id))
            })
        else {
            return false;
        };
        if src.ctrl as usize != p || !src.subtypes.iter().any(|s| s == "dwarf") {
            return false;
        }
        let c = self.card(pend.card);
        let abilities = match (&c.back, pend.back || src.transformed) {
            (Some(b), true) => &b.abilities,
            _ => &c.abilities,
        };
        abilities.get(pend.ability as usize).is_some_and(|ab| {
            matches!(
                ab.trig,
                Trig::Enters
                    | Trig::Dies
                    | Trig::Attacks
                    | Trig::CombatDamage
                    | Trig::Phase { .. }
                    | Trig::Event(..)
                    | Trig::Chapter(_)
                    | Trig::Reflexive
            )
        })
    }

    pub(crate) fn add_counters(&mut self, i: usize, n: i32) {
        let p = self.perms[i].ctrl as usize;
        let id = self.perms[i].id;
        if !self.last_countered.contains(&id) {
            self.last_countered.push(id);
        }
        let n = if self.doubles(p, "counter") { n * 2 } else { n };
        // Yoshimaru, Doc Samson: "that many plus one".
        let n = n + self.extra_counters(p, n);
        self.perms[i].counters += n as i16;
        if n > 0 {
            self.perms[i].countered_turn = true;
        }
        let snap = self.perms[i].clone();
        self.emit_about(Ev::Counters, p, &snap);
    }

    /// Counters added on top of `n` +1/+1 counters put on a permanent player `p` controls
    /// ("that many plus one": Yoshimaru for +1/+1 counters, Doc Samson for any kind).
    pub(crate) fn extra_counters(&self, p: usize, n: i32) -> i32 {
        if n <= 0 {
            return 0;
        }
        let r = self.rules[p % 8];
        i32::from(r & R_EXTRA_COUNTER != 0) + i32::from(r & R_EXTRA_ANY_COUNTER != 0)
    }

    /// Plan, lore and other counters; fires "when the Nth counter is put" abilities.
    pub fn add_other_counters(&mut self, i: usize, n: i16) {
        // Doc Samson: "that many plus one of each of those kinds of counters".
        let n = if n > 0 && self.rules[self.perms[i].ctrl as usize % 8] & R_EXTRA_ANY_COUNTER != 0 {
            n + 1
        } else {
            n
        };
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
            "heartwood" => Tok::Heartwood,
            "lotus" => Tok::Lotus,
            // "An Equipment artifact token named Axe with 'Equipped creature gets +1/+0'".
            k if k.starts_with("equipment") => {
                let pt: Vec<i8> = k
                    .split(' ')
                    .nth(1)
                    .unwrap_or("+1/+0")
                    .split('/')
                    .filter_map(|d| d.trim_start_matches('+').parse().ok())
                    .collect();
                Tok::Equip(
                    pt.first().copied().unwrap_or(1),
                    pt.get(1).copied().unwrap_or(0),
                )
            }
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
            if self.log.is_some() {
                let name = self.players[p].cards[c as usize].name.clone();
                let tag = self.cause_tag();
                self.note(|| format!("  P{} discards {}{}", p, name, tag));
            }
            self.put_discarded(p, c);
        }
    }

    /// "Exile the top card of your library. You may cast that card. If you don't, ~ deals 2
    /// damage to each opponent" (Chandra): decided now.
    fn cast_or_else(&mut self, ctx: &Ctx) {
        let me = ctx.ctrl;
        let t = ctx.text;
        let Some(rest) = t.split("you may cast that card. if you don't, ").nth(1) else {
            return;
        };
        let n = word_number(t.split("the top ").nth(1).unwrap_or("card")).unwrap_or(1);
        let k = self.players[me].library.len().saturating_sub(n);
        let mut cards = self.players[me].library.split_off(k);
        cards.reverse(); // top first
        self.ext.impulse_cast = false;
        for c in cards {
            let castable = !self.players[me].cards[c as usize].is(T_LAND);
            self.players[me].hand.push(c);
            let h = self.players[me].hand.len() - 1;
            if castable
                && self.can_cast(me, super::Src::Hand(h), Default::default())
                && self.cast(me, super::Src::Hand(h), Default::default())
            {
                self.ext.impulse_cast = true;
            } else if let Some(h) = self.players[me].hand.iter().rposition(|&x| x == c) {
                self.players[me].hand.remove(h);
                self.players[me].exile.push(c);
            }
        }
        if !self.ext.impulse_cast {
            let dmg = rest
                .split(" damage")
                .next()
                .and_then(|h| h.rsplit(' ').next())
                .and_then(|d| d.parse::<i32>().ok())
                .unwrap_or(2);
            if rest.contains("each opponent") {
                for o in self.opponents(me) {
                    self.hurt_player(ctx, o, dmg);
                }
            }
        }
    }

    /// Casts card `r` (from any player's deck list) for player `me` without paying its mana
    /// cost, from wherever the caller took it: cast triggers fire, a permanent enters under
    /// `me` (as a token if it's a `copy`), an instant or sorcery resolves and then goes to its
    /// owner's graveyard (unless it's a copy).
    pub fn cast_foreign(&mut self, me: usize, r: CardRef, copy: bool) {
        let c = self.card(r);
        self.note(|| format!("P{} casts {} for free", me, c.name));
        self.on_cast(me, c);
        if c.is_permanent() {
            let i = if copy {
                let t = self.new_perm(me, me, Some(r), Tok::Other);
                let id = self.perms[t].id;
                self.after_enter(t);
                for (k, a) in c.abilities.iter().enumerate() {
                    if a.trig == Trig::Enters {
                        self.pending.push(Pending {
                            ctrl: me as u8,
                            source: Some(id),
                            card: r,
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
                self.idx(id)
            } else {
                self.enter_card(r.0 as usize, r.1, me, false)
            };
            // An Aura goes on the creature it suits.
            if let (Some(i), true) = (i, c.is(T_AURA)) {
                let id = self.perms[i].id;
                let cands: Vec<usize> = (0..self.perms.len())
                    .filter(|&j| self.perms[j].is_creature() && j != i)
                    .filter(|&j| (self.perms[j].ctrl as usize == me) != c.harmful_aura)
                    .collect();
                let pick = if c.harmful_aura {
                    ai::choose_victims(self, me, &cands, 1)
                } else {
                    ai::choose_friends(self, me, &cands, 1)
                };
                if let (Some(&t), Some(j)) = (pick.first(), self.idx(id)) {
                    self.perms[j].attached = Some(self.perms[t].id);
                }
            }
        } else {
            let outer = self.casting;
            self.casting = Some((r, false));
            for (k, a) in c.abilities.iter().enumerate() {
                if a.trig == Trig::Spell {
                    self.pending.push(Pending {
                        ctrl: me as u8,
                        source: None,
                        card: r,
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
            self.run_pending();
            self.casting = outer;
            if !copy {
                self.players[r.0 as usize].graveyard.push(r.1);
            }
        }
        self.run_pending();
        self.sba();
    }

    /// Is a free card from library/graveyard/exile worth casting for `me` (not a land, not a
    /// card that only answers something)?
    fn worth_free_cast(&self, me: usize, r: CardRef) -> bool {
        let c = self.card(r);
        let _ = me;
        !c.is(T_LAND)
            && c.value > 0.5
            && !c.abilities.iter().any(|a| {
                a.trig == Trig::Spell && a.effects.iter().all(|e| *e == Effect::CounterSpell)
            })
    }

    /// FreeSpell effects that cast copies of cards exiled to pay for the ability (Baron
    /// Helmut Zemo). False if the text isn't one of those (the caller handles it).
    fn free_cast_elsewhere(&mut self, ctx: &Ctx) -> bool {
        let me = ctx.ctrl;
        let t = ctx.text;
        if t.contains("copy those exiled cards") {
            // Baron Helmut Zemo: copies of the cards exiled to pay for it.
            let up_to = if t.contains("up to three") { 3 } else { 1 };
            let exiled = std::mem::take(&mut self.ext.exiled_for_cost);
            self.cast_copies(me, &exiled, u32::MAX, up_to);
            return true;
        }
        false
    }

    /// Casts copies of some of `me`'s cards (best first) within a total mana value and count.
    fn cast_copies(&mut self, me: usize, cards: &[u16], budget: u32, max: u8) {
        let mut order: Vec<u16> = cards
            .iter()
            .copied()
            .filter(|&c| self.worth_free_cast(me, (me as u8, c)))
            .collect();
        order.sort_by(|&a, &b| {
            self.card((me as u8, b))
                .value
                .partial_cmp(&self.card((me as u8, a)).value)
                .unwrap()
        });
        let mut spent = 0u32;
        let mut cast = 0u8;
        for c in order {
            let mv = self.card((me as u8, c)).mv as u32;
            if spent + mv > budget || cast >= max {
                continue;
            }
            spent += mv;
            cast += 1;
            self.cast_foreign(me, (me as u8, c), true);
        }
    }

    /// Delayed triggers ("at the beginning of the next end step, return it") for `phase`.
    pub fn run_delayed(&mut self, phase: Phase) {
        let now: Vec<Delayed> = {
            let (now, later): (Vec<Delayed>, Vec<Delayed>) = std::mem::take(&mut self.ext.delayed)
                .into_iter()
                .partition(|d| match d {
                    Delayed::Return { phase: p, .. } | Delayed::Tokens { phase: p, .. } => {
                        *p == phase
                    }
                });
            self.ext.delayed = later;
            now
        };
        for d in now {
            match d {
                Delayed::Return {
                    owner,
                    card,
                    ctrl,
                    from_graveyard,
                    other_counters,
                    ..
                } => {
                    let o = owner as usize;
                    let zone = if from_graveyard {
                        &mut self.players[o].graveyard
                    } else {
                        &mut self.players[o].exile
                    };
                    let Some(k) = zone.iter().rposition(|&c| c == card) else {
                        continue;
                    };
                    zone.remove(k);
                    if !self.players[ctrl as usize].alive {
                        continue;
                    }
                    let name = self.players[o].cards[card as usize].name.clone();
                    self.note(|| format!("  {} returns (delayed)", name));
                    if let Some(i) = self.enter_card(o, card, ctrl as usize, false) {
                        if other_counters > 0 {
                            self.perms[i].other_counters = other_counters;
                        }
                    }
                }
                Delayed::Tokens {
                    ctrl,
                    n,
                    p,
                    t,
                    kw,
                    subtypes,
                    ..
                } => {
                    if !self.players[ctrl as usize].alive {
                        continue;
                    }
                    let n = if self.doubles(ctrl as usize, "token") {
                        n as i32 * 2
                    } else {
                        n as i32
                    };
                    for _ in 0..n.min(20) {
                        let i = self.make_token(
                            ctrl as usize,
                            p as i32,
                            t as i32,
                            kw,
                            subtypes.clone(),
                        );
                        self.after_enter(i);
                    }
                }
            }
        }
        self.run_pending();
    }

    /// "Target opponent reveals their hand. You choose a nonland card from it. That player
    /// discards that card / exile that card": you take their best card. "Reveals a number of
    /// cards from their hand equal to ..." (Klaw): they show their worst N, you pick the best
    /// of those. "Exile a nonland card from their hand or the chosen creature until ~ leaves"
    /// (Cloak and Dagger): whichever is worth more.
    fn reveal_and_take(&mut self, ctx: &Ctx, o: usize) {
        let me = ctx.ctrl;
        let text = ctx.text;
        let exile = text.contains("exile that card") || text.contains("exile a nonland card");
        let pick = if text.contains("reveals a number of cards") {
            let n = 1 + self
                .count_of(me, ctx.src, "creature cards in your graveyard")
                .unwrap_or(0);
            let pl = &self.players[o];
            let mut order: Vec<usize> = (0..pl.hand.len()).collect();
            let worth = |h: usize| pl.cards[pl.hand[h] as usize].value;
            order.sort_by(|&a, &b| worth(a).partial_cmp(&worth(b)).unwrap());
            order.truncate(n.max(1) as usize);
            order
                .into_iter()
                .max_by(|&a, &b| worth(a).partial_cmp(&worth(b)).unwrap())
        } else {
            ai::best_card_in_hand(self, o)
        };
        // The chosen creature instead, if it's worth more than any card in hand.
        if text.contains("or the chosen creature") {
            let card_value = pick.map_or(0.0, |h| {
                let pl = &self.players[o];
                pl.cards[pl.hand[h] as usize].value
            });
            let t = crate::abilities::target_of("creature that player controls", false);
            let cands: Vec<usize> = self
                .candidates(ctx, &t, true)
                .into_iter()
                .filter(|&i| self.perms[i].ctrl as usize == o)
                .collect();
            if let Some(&i) = ai::choose_victims(self, me, &cands, 1).first() {
                if ai::perm_value(self, i) > card_value {
                    let x = self.perms[i].clone();
                    self.leave(i, Zone::Exile, false);
                    if let (Some(r), Some(s), Tok::None) = (x.card, self.src_idx(ctx), x.tok) {
                        if x.commander == 0 {
                            self.perms[s].holding.push((x.owner, r));
                        }
                    }
                    return;
                }
            }
        }
        let Some(h) = pick else { return };
        let c = self.players[o].hand.remove(h);
        if exile {
            self.players[o].exile.push(c);
        } else {
            self.put_discarded(o, c);
        }
    }

    /// A card player `p` just discarded goes to the graveyard: "whenever you discard"
    /// triggers, and the card's own "when you discard ~" (Titanbones).
    pub fn put_discarded(&mut self, p: usize, c: u16) {
        self.players[p].graveyard.push(c);
        self.emit(Ev::Discard, p, None);
        self.to_graveyard(p, &[c]);
        let card = &self.players[p].cards[c as usize];
        for (k, a) in card.abilities.iter().enumerate() {
            if a.text.starts_with("when you discard ~") {
                self.pending.push(Pending {
                    ctrl: p as u8,
                    source: None,
                    card: (p as u8, c),
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

    pub fn mill(&mut self, p: usize, n: i32) {
        if self.log.is_some() && n > 0 {
            let (tag, left) = (self.cause_tag(), self.players[p].library.len());
            self.note(|| format!("  P{} mills {} (library {}){}", p, n, left, tag));
        }
        let mut milled = Vec::new();
        for _ in 0..n {
            if let Some(c) = self.players[p].library.pop() {
                self.players[p].graveyard.push(c);
                self.players[p].px.milled_turn += 1;
                milled.push(c);
            }
        }
        self.to_graveyard(p, &milled);
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
        let owner = self.perms[i].ctrl as usize;
        // "If a source you control would deal noncombat damage to an opponent or a permanent
        // an opponent controls, it deals that much damage plus 1 instead" (Tomik).
        let n = n + self.damage_bonus(ctx.ctrl, owner);
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
        // Crystal Barricade: noncombat damage to its controller's other creatures.
        let (owner, target) = (self.perms[i].ctrl as usize, self.perms[i].id);
        if self.rules[owner % 8] & R_PREVENT_NONCOMBAT != 0
            && self.perms[i].is_creature()
            && self.perms.iter().any(|y| {
                y.ctrl as usize == owner
                    && y.id != target
                    && self.perm_rules(y) & R_PREVENT_NONCOMBAT != 0
            })
        {
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
        let dealt = self.damage_perm(i, n as i16, deathtouch);
        if let Some(s) = self.src_idx(ctx) {
            self.perms[s].dealt_damage = true;
        }
        // (Prevented damage isn't dealt: no life for lifelink.)
        if dealt > 0 && src.is_some_and(|s| s.has(LIFELINK) && s.is_creature()) {
            self.gain_life(ctx.ctrl, dealt as i32);
        }
    }

    /// "... deals that much damage plus 1 instead" to an opponent or their permanents.
    fn damage_bonus(&self, from: usize, to: usize) -> i32 {
        if from == to {
            return 0;
        }
        self.perms
            .iter()
            .filter(|x| x.ctrl as usize == from)
            .flat_map(|x| self.abilities_of(x).iter())
            .filter(|a| {
                a.trig == Trig::Static
                    && a.text.contains("it deals that much damage plus 1 instead")
            })
            .count() as i32
    }

    fn hurt_player(&mut self, ctx: &Ctx, o: usize, n: i32) {
        let n = n + self.damage_bonus(ctx.ctrl, o);
        let n = if self.doubles_from(ctx.ctrl, "damage", ctx.src) {
            n * 2
        } else {
            n
        };
        self.damage_player(ctx.ctrl, o, n, 0);
        if n > 0 && o != ctx.ctrl {
            self.players[o].px.noncombat_damaged_turn = true;
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
        if !self.auditing() {
            return self.apply_inner(ctx, e);
        }
        let outer = std::mem::replace(&mut self.effect_kind, super::audit::effect_kind(e));
        self.apply_inner(ctx, e);
        self.effect_kind = outer;
    }

    fn apply_inner(&mut self, ctx: &Ctx, e: &Effect) {
        let me = ctx.ctrl;
        let text = ctx.text;
        match e {
            Effect::Destroy(_) if text.contains("odd or even") => {
                // Thanos: pick the parity that hurts the table most and spares the most of
                // our own; "each other creature" never includes the source.
                let (odd, net) = ai::best_parity(self, me, ctx.src);
                if self.log.is_some() {
                    let (o, e) = ai::parity_values_peek(self, me, ctx.src);
                    self.note(|| {
                        format!(
                            "  AI P{} parity: odd nets {:.1}, even nets {:.1} -> {} ({:.1})",
                            me,
                            o,
                            e,
                            if odd { "odd" } else { "even" },
                            net
                        )
                    });
                }
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
                let mut lost_lands: Vec<usize> = Vec::new();
                for id in self.choose(ctx, t, harmful) {
                    if let Some(i) = self.idx(id) {
                        self.focus_card = self.perms[i].card.filter(|_| !t.mass);
                        let x = self.perms[i].clone();
                        if self.destroy(i) {
                            n += 1;
                            if x.is(T_LAND) {
                                lost_lands.push(x.ctrl as usize);
                            }
                            // "... and all other permanents with the same name as that
                            // permanent" (Maelstrom Pulse).
                            if text.contains("all other permanents with the same name") {
                                let name = x.card.map(|r| self.card(r).name.clone());
                                let same: Vec<u32> = self
                                    .perms
                                    .iter()
                                    .filter(|y| {
                                        name.is_some()
                                            && y.card.map(|r| self.card(r).name.clone()) == name
                                    })
                                    .map(|y| y.id)
                                    .collect();
                                for sid in same {
                                    if let Some(j) = self.idx(sid) {
                                        if self.destroy(j) {
                                            n += 1;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                self.last_destroyed = n;
                // "Destroy target land. Its controller may search their library for a basic
                // land card, put it onto the battlefield tapped" (and "that land's controller").
                if text.contains("controller may search their library for a basic land") {
                    let tapped = text.contains("onto the battlefield tapped");
                    for o in lost_lands {
                        if self.players[o].alive {
                            self.fetch_land(o, true, tapped, true);
                        }
                    }
                }
            }
            Effect::Exile { target, until } => {
                let harmful = target.whose != Whose::Yours;
                // "Its owner shuffles it into their library" (Living Library, Chaos Warp).
                let shuffle = text.contains("shuffles it into") || text.contains("shuffle it into");
                // "Exile all creatures. Each player may put any number of creature cards from
                // their hand onto the battlefield. Then put all cards exiled this way into their
                // owners' hands" (Worlds Within Worlds).
                if text.contains("put all cards exiled this way into their owners' hands") {
                    let mut exiled: Vec<(usize, u16)> = Vec::new();
                    for id in self.choose(ctx, target, harmful) {
                        let Some(i) = self.idx(id) else { continue };
                        let x = self.perms[i].clone();
                        self.leave(i, Zone::Exile, false);
                        if let (Some(r), Tok::None, 0) = (x.card, x.tok, x.commander) {
                            exiled.push((x.owner as usize, r.1));
                        }
                    }
                    let alive: Vec<usize> = (0..self.players.len())
                        .filter(|&p| self.players[p].alive)
                        .collect();
                    for p in alive {
                        let creatures: Vec<u16> = self.players[p]
                            .hand
                            .iter()
                            .copied()
                            .filter(|&c| self.players[p].cards[c as usize].is(T_CREATURE))
                            .collect();
                        for c in creatures {
                            if let Some(h) = self.players[p].hand.iter().position(|&x| x == c) {
                                self.players[p].hand.remove(h);
                                self.enter_card(p, c, p, false);
                            }
                        }
                    }
                    for (o, c) in exiled {
                        if let Some(k) = self.players[o].exile.iter().rposition(|&x| x == c) {
                            self.players[o].exile.remove(k);
                            self.players[o].hand.push(c);
                        }
                    }
                    return;
                }
                for id in self.choose(ctx, target, harmful) {
                    let Some(i) = self.idx(id) else { continue };
                    let x = self.perms[i].clone();
                    if shuffle {
                        self.leave(i, Zone::Library(false), false);
                        let o = x.owner as usize;
                        let mut lib = std::mem::take(&mut self.players[o].library);
                        self.rng.shuffle(&mut lib);
                        self.players[o].library = lib;
                        // "Then reveals the top card of their library. If it's a permanent
                        // card, they put it onto the battlefield" (Chaos Warp).
                        if text.contains("reveals the top card of their library") {
                            if let Some(&c) = self.players[o].library.last() {
                                if self.players[o].cards[c as usize].is_permanent() {
                                    self.players[o].library.pop();
                                    self.enter_card(o, c, o, false);
                                }
                            }
                        }
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
            // "Its owner may put it on the top of their library. If they do, ~ deals 2 damage
            // to them" (Clash of Elements): part of the choice (`Effect::Bounce`).
            Effect::Damage { .. } if text.contains("may put it on the top of their library") => {}
            // A bite: "target creature you control deals damage equal to (twice) its power to
            // target creature an opponent controls" - your biggest creature is the source.
            Effect::Damage { to, .. }
                if (text.contains("creature you control deals damage equal to")
                    || text.contains("that creature deals damage equal to its power"))
                    && !text.contains("planeswalker you control deals") =>
            {
                self.last_excess = 0;
                // "Put a +1/+1 counter on target creature you control. Then that creature
                // deals damage ...": the one chosen; otherwise your biggest.
                let chosen = text
                    .contains("that creature deals damage")
                    .then(|| self.last_targets.iter().find_map(|&id| self.idx(id)))
                    .flatten();
                let Some(b) = chosen.or_else(|| ai::best_fighter(self, me)) else {
                    return;
                };
                let p = self.perms[b].p.max(0) as i32;
                let n = if text.contains("equal to twice its power") {
                    2 * p
                } else {
                    p
                };
                let bctx = Ctx {
                    src: Some(self.perms[b].id),
                    snap: None,
                    ..ctx.clone()
                };
                if n <= 0 {
                    return;
                }
                if let Some(ids) = self.prev_ids(ctx, to) {
                    for id in ids {
                        self.hurt(&bctx, id, n);
                    }
                    return;
                }
                if let ai::Hit::Perm(id) = ai::damage_target(self, ctx, to, n) {
                    if self.auditing() {
                        self.audit_targets(me, &[id], true, &[]);
                    }
                    if let Some(i) = self.idx(id) {
                        if self.perms[i].ctrl as usize == me || self.pay_ward(me, i) {
                            self.actor = me as u8;
                            self.emit_targeted(id);
                            self.hurt(&bctx, id, n);
                        }
                    }
                }
            }
            Effect::Damage { amount, to } => {
                self.last_excess = 0;
                // "~ gains '{T}: ~ deals damage equal to his power to any other target' until
                // end of turn" (Iron Fist): used right away, by tapping him.
                if text.contains("gains \"{t}: ~ deals damage") {
                    match self.src_idx(ctx) {
                        Some(s)
                            if !self.perms[s].tapped
                                && !(self.perms[s].sick && !self.perms[s].has(HASTE)) =>
                        {
                            self.perms[s].tapped = true;
                        }
                        _ => return,
                    }
                }
                // "Target planeswalker you control deals damage equal to its loyalty": none,
                // no damage.
                let n = if text
                    .contains("planeswalker you control deals damage equal to its loyalty")
                {
                    match self
                        .perms
                        .iter()
                        .filter(|x| x.ctrl as usize == me && x.is(T_PLANESWALKER))
                        .map(|x| x.loyalty as i32)
                        .max()
                    {
                        Some(l) => l,
                        None => return,
                    }
                } else {
                    self.x_value(ctx, amount)
                };
                if n <= 0 {
                    return;
                }
                if let Some(o) = self.exact_life_target(me, text) {
                    if let Some(o) = o {
                        self.hurt_player(ctx, o, n);
                    }
                    return;
                }
                // ("Each creature and each opponent": the opponents' part is its own Drain.)
                let each_opp = text.contains("each opponent")
                    && !text.contains("and each opponent")
                    || text.contains("each player");
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
                        ai::burn_opponent(self, me, n).into_iter().collect()
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
                        ai::Hit::Player(o) => {
                            self.note(|| format!("    {} damage aimed at P{}", n, o));
                            self.hurt_player(ctx, o, n)
                        }
                        ai::Hit::Perm(id) => {
                            if self.auditing() {
                                self.audit_targets(me, &[id], true, &[]);
                            }
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
                // "Gets -4/-1 until your next turn" (Garruk): a temporary -4/-1 (until the end
                // of this turn here), not a -1/-1 counter.
                let temporary = *until_eot || text.contains("until your next turn");
                for id in self.choose(ctx, to, harmful) {
                    if let Some(i) = self.idx(id) {
                        if temporary {
                            self.perms[i].temp_p -= np;
                            self.perms[i].temp_t -= n;
                        } else {
                            self.perms[i].counters -= n;
                        }
                    }
                }
            }
            // "Choose target creature you own. ... Return each chosen creature to your hand.
            // At the beginning of the next upkeep, create a 4/4 Bird for each creature
            // returned this way" (The Eagles Are Coming!): the cheapest creature to replay.
            Effect::Bounce(_) if text.contains("return each chosen creature to your hand") => {
                let pick = self
                    .perms
                    .iter()
                    .enumerate()
                    .filter(|(_, x)| {
                        x.ctrl as usize == me
                            && x.owner as usize == me
                            && x.is_creature()
                            && !x.is_token()
                            && x.commander == 0
                    })
                    .map(|(i, _)| i)
                    .min_by(|&a, &b| {
                        ai::perm_value(self, a)
                            .partial_cmp(&ai::perm_value(self, b))
                            .unwrap()
                    });
                self.ext.last_bounced = 0;
                if let Some(i) = pick {
                    self.leave(i, Zone::Hand, false);
                    self.ext.last_bounced = 1;
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
                // "Its owner may put it on the top of their library. If they do, ~ deals 2
                // damage to them. If they didn't ..., on the bottom": the top (and the damage)
                // while they can spare the life.
                let top_for_damage = text.contains("may put it on the top of their library");
                let mut n = 0;
                for id in self.choose(ctx, t, harmful) {
                    if let Some(i) = self.idx(id) {
                        if top_for_damage {
                            let o = self.perms[i].owner as usize;
                            let dmg = text
                                .split(" damage to them")
                                .next()
                                .and_then(|h| h.rsplit(' ').next())
                                .and_then(|d| d.parse::<i32>().ok())
                                .unwrap_or(2);
                            let top = self.players[o].life > 3 * dmg + 4;
                            self.leave(i, Zone::Library(top), false);
                            if top {
                                self.hurt_player(ctx, o, dmg);
                            }
                        } else {
                            self.leave(i, to, false);
                        }
                        n += 1;
                    }
                }
                self.ext.last_bounced = n;
            }
            Effect::Lock { target, lasting } => {
                // Auras and Equipment do this through their static ability.
                if text.starts_with("enchanted") || text.starts_with("equipped") {
                    return;
                }
                // "Loses all abilities until end of turn" doesn't stop it attacking or blocking
                // (Flourishing Grapple): only what it's chosen for.
                let only_target = text.contains("loses all abilities until end of turn");
                for id in self.choose(ctx, target, true) {
                    let Some(i) = self.idx(id) else { continue };
                    let x = &mut self.perms[i];
                    if only_target {
                        continue;
                    }
                    // "Tap enchanted creature and remove all counters from it".
                    if text.contains("remove all counters from it") {
                        x.counters = 0;
                        x.other_counters = 0;
                        x.shield = 0;
                    }
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
            // "Each opponent chooses a permanent they control of each permanent type and
            // sacrifices the rest" (Liliana, Dreadhorde General): they keep their best of each.
            Effect::Edict { .. }
                if text.contains("of each permanent type and sacrifices the rest") =>
            {
                for o in self.opponents(me) {
                    let theirs: Vec<usize> = (0..self.perms.len())
                        .filter(|&i| self.perms[i].ctrl as usize == o)
                        .collect();
                    let mut keep: Vec<u32> = Vec::new();
                    for ty in [
                        T_ARTIFACT,
                        T_CREATURE,
                        T_ENCHANTMENT,
                        T_LAND,
                        T_PLANESWALKER,
                    ] {
                        let kept = theirs
                            .iter()
                            .copied()
                            .filter(|&i| {
                                let x = &self.perms[i];
                                if ty == T_CREATURE {
                                    x.is_creature()
                                } else {
                                    x.is(ty)
                                }
                            })
                            .max_by(|&a, &b| {
                                ai::perm_value(self, a)
                                    .partial_cmp(&ai::perm_value(self, b))
                                    .unwrap()
                            });
                        if let Some(i) = kept {
                            keep.push(self.perms[i].id);
                        }
                    }
                    let gone: Vec<u32> = theirs
                        .iter()
                        .map(|&i| self.perms[i].id)
                        .filter(|id| !keep.contains(id))
                        .collect();
                    for id in gone {
                        if let Some(i) = self.idx(id) {
                            self.sacrifice(i);
                        }
                    }
                }
            }
            Effect::Edict { each_opponent } => {
                let mut victims: Vec<usize> = if *each_opponent {
                    self.opponents(me)
                } else {
                    ai::edict_victim(self, me, &ctx.card_restr)
                        .into_iter()
                        .collect()
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
                } else if text.contains("two target players each draw") {
                    // You and the opponent it helps least.
                    self.draw(me, n);
                    if let Some(o) = self.opponents(me).into_iter().min_by(|&a, &b| {
                        ai::threat(self, a)
                            .partial_cmp(&ai::threat(self, b))
                            .unwrap()
                    }) {
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
                let connive = text.contains(" connive");
                self.ext.connive_nonland = false;
                // "Then up to one target creature you control connives" with no creature to
                // choose: nothing connives (CR 702.157).
                if connive {
                    let clause = text
                        .split(". ")
                        .find(|s| s.contains(" connive"))
                        .unwrap_or(text);
                    if clause.contains("target ")
                        && !self
                            .perms
                            .iter()
                            .any(|x| x.ctrl as usize == me && x.is_creature())
                    {
                        return;
                    }
                }
                // "If a creature you control would connive, instead you draw a card, then that
                // creature connives" (Leader).
                if connive
                    && self.controls_static(me, None, |t| {
                        t.contains("would connive, instead you draw a card")
                    })
                {
                    self.draw(me, 1);
                }
                // "Discard a card, then draw a card" (Boomerang): the discard is chosen
                // before the new card is seen.
                if !connive && text.contains("discard a card, then draw a card") {
                    self.discard(me);
                    self.draw(me, 1);
                    return;
                }
                self.draw(me, 1);
                let before = self.players[me].graveyard.len();
                self.discard(me);
                // Connive: the +1/+1 counter only for a nonland card discarded.
                if connive {
                    let pl = &self.players[me];
                    self.ext.connive_nonland = pl
                        .graveyard
                        .get(before)
                        .or(pl.exile.last().filter(|_| pl.graveyard.len() == before))
                        .is_some_and(|&c| !pl.cards[c as usize].is(T_LAND));
                }
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
                            self.reveal_and_take(ctx, o);
                        } else {
                            self.discard(o);
                        }
                    }
                } else {
                    self.discard(me);
                }
            }
            // "Whenever a player loses life, that player mills that many cards" (The Master of
            // Lake-town): the player who lost it, the life lost.
            Effect::Mill { amount } if text.contains("that player mills") => {
                let n = self.x_value(ctx, amount);
                if let Some(o) = ctx.about.filter(|&o| self.players[o].alive) {
                    self.mill(o, n);
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
            // "Look at the top three cards of your library. You may put one of those cards back
            // on top of your library. Put the rest into your graveyard" (Gutless Plunderer).
            Effect::Scry(_) if text.contains("one of those cards back on top") => {
                let n =
                    word_number(text.split("look at the top ").nth(1).unwrap_or("")).unwrap_or(3);
                let k = self.players[me].library.len().saturating_sub(n);
                let mut cards = self.players[me].library.split_off(k);
                let best = (0..cards.len()).max_by(|&a, &b| {
                    let va = ai::card_worth(self, me, &self.players[me].cards[cards[a] as usize]);
                    let vb = ai::card_worth(self, me, &self.players[me].cards[cards[b] as usize]);
                    va.partial_cmp(&vb).unwrap()
                });
                let top = best.map(|j| cards.remove(j));
                self.players[me].graveyard.extend(cards);
                self.players[me].library.extend(top);
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
                // "A number of Goblins equal to the amount of excess damage dealt this way".
                let mut n = if text.contains("tokens equal to the amount of excess damage") {
                    self.last_excess.max(0)
                } else {
                    self.x_value(ctx, amount).max(0)
                };
                let kw = keyword_bits(keywords);
                let subs = token_subtypes(text);
                // "At the beginning of the next upkeep, create a 4/4 Bird ...".
                if text.contains("at the beginning of the next upkeep, create") {
                    if n > 0 {
                        self.ext.delayed.push(Delayed::Tokens {
                            phase: Phase::Upkeep,
                            ctrl: me as u8,
                            n: n.min(20) as u8,
                            p: *power as i8,
                            t: *toughness as i8,
                            kw,
                            subtypes: subs,
                        });
                    }
                    return;
                }
                if self.doubles(me, "token") {
                    n *= 2;
                }
                let mut made = Vec::new();
                let artifact = text.contains("artifact creature token");
                // Abilities in quotes, a name ("Zabu, a legendary ... token"): the card's token.
                let quoted = ctx
                    .card_ref
                    .filter(|_| text.contains('"') || text.contains(", a legendary "))
                    .filter(|&r| self.card(r).token.is_some());
                for _ in 0..n.min(20) {
                    let i = self.make_token(me, *power, *toughness, kw, subs.clone());
                    if artifact {
                        self.perms[i].types |= T_ARTIFACT;
                    }
                    if let Some(r) = quoted {
                        let t = self
                            .card(r)
                            .token
                            .as_deref()
                            .map_or((0, 0), |t| (t.kw, t.types));
                        let x = &mut self.perms[i];
                        x.tok_src = Some(r);
                        x.kw |= t.0;
                        x.types |= t.1 & T_LEGENDARY;
                    }
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
                // "Destroy target permanent. They create a Treasure token" (Vraska): theirs.
                let to = if text.contains("gift") {
                    ai::choose_opponent(self, me, 0).unwrap_or(me)
                } else if text.contains(". they create a") {
                    self.last_left
                        .as_ref()
                        .map(|x| x.ctrl as usize)
                        .filter(|&o| o != me && self.players[o].alive)
                        .unwrap_or(me)
                } else {
                    me
                };
                // An Equipment token's bonus: "with 'Equipped creature gets +1/+2'".
                let kind = if kind == "equipment" {
                    let pt = text
                        .split("equipped creature gets ")
                        .nth(1)
                        .and_then(|r| r.split(['"', ' ', '.']).next())
                        .unwrap_or("+1/+0");
                    format!("equipment {}", pt)
                } else {
                    kind.clone()
                };
                let mut made = Vec::new();
                for _ in 0..n.min(10) {
                    self.make_item(to, &kind);
                    if let Some(x) = self.perms.last() {
                        made.push(x.id);
                    }
                }
                // "... named Sturdy Shield ... Attach it to ~" (U.S.Agent).
                if kind.starts_with("equipment") && text.contains("attach it to ~") {
                    if let (Some(&e), Some(s)) = (made.first(), ctx.src) {
                        if let (Some(i), Some(_)) = (self.idx(e), self.idx(s)) {
                            self.perms[i].attached = Some(s);
                        }
                    }
                }
            }
            // "That player exiles cards from the top of their library until they exile a
            // nonland card. You may put a +1/+1 counter on ~. If you don't, you may cast the
            // exiled nonland card ... mana of any type" (Black Widow).
            Effect::PlusCounters { .. }
                if text.contains("if you don't, you may cast the exiled nonland card") =>
            {
                let hit = self.src_perm(ctx).map_or(0, |s| s.hit_turn);
                let o = (0..self.players.len())
                    .find(|&o| hit & (1 << o.min(7)) != 0 && o != me && self.players[o].alive)
                    .or_else(|| ai::choose_opponent(self, me, 0));
                let mut cast = false;
                if let Some(o) = o {
                    while let Some(c) = self.players[o].library.pop() {
                        let r = (o as u8, c);
                        if self.card(r).is(T_LAND) {
                            self.players[o].exile.push(c);
                            continue;
                        }
                        let mv = self.card(r).mv as u32;
                        if self.worth_free_cast(me, r)
                            && self.card(r).value > 2.5
                            && self.can_pay(me, &Mana::generic(mv))
                        {
                            self.pay(me, &Mana::generic(mv));
                            self.cast_foreign(me, r, false);
                            cast = true;
                        } else {
                            self.players[o].exile.push(c);
                        }
                        break;
                    }
                }
                if !cast {
                    if let Some(i) = self.src_idx(ctx) {
                        self.add_counters(i, 1);
                    }
                }
            }
            // Connive's counter: only if a nonland card was discarded (see `Effect::Loot`).
            Effect::PlusCounters { .. }
                if text.contains(" connive")
                    && !text.contains("+1/+1 counter")
                    && !self.ext.connive_nonland => {}
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
            // "Choose another target creature. If that creature has haste and ~ doesn't, put a
            // haste counter on ~. Do the same for flying, first strike, ..." (Super-Adaptoid):
            // the keyword counters it copies (`Effect::OtherCounters`).
            Effect::Grant { .. } if text.contains("do the same for") => {}
            // "Put a trample counter on target creature": the counter gives it
            // (`Effect::OtherCounters`).
            Effect::Grant { keywords, .. }
                if keywords.len() == 1 && text.contains(&format!("{} counter on", keywords[0])) => {
            }
            Effect::OtherCounters(_) if text.contains("do the same for") => {
                let Some(s) = self.src_idx(ctx) else { return };
                let mut list: Vec<String> = text
                    .split("do the same for ")
                    .nth(1)
                    .unwrap_or("")
                    .trim_end_matches('.')
                    .replace(" and ", ", ")
                    .split(", ")
                    .map(|w| w.trim().to_string())
                    .filter(|w| !w.is_empty())
                    .collect();
                list.push("haste".into());
                let bits = keyword_bits(&list);
                let sid = self.perms[s].id;
                // The creature with the most keywords it lacks.
                let mine = self.perms[s].kw;
                let pick = (0..self.perms.len())
                    .filter(|&i| self.perms[i].is_creature() && self.perms[i].id != sid)
                    .filter(|&i| {
                        let x = &self.perms[i];
                        x.ctrl as usize == me || !x.has(HEXPROOF | SHROUD)
                    })
                    .max_by_key(|&i| (self.perms[i].eff_kw & bits & !mine).count_ones());
                if let Some(i) = pick {
                    let gained = self.perms[i].eff_kw & bits & !mine;
                    self.perms[s].kw |= gained;
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
            // "Exile the top card. You may cast that card. If you don't, ~ deals 2 damage to
            // each opponent": decided with the card (`cast_or_else`).
            Effect::Drain { .. } if text.contains("you may cast that card. if you don't") => {}
            Effect::Drain { amount, each } => {
                let n = self.x_value(ctx, amount);
                let mut victims: Vec<usize> = if *each {
                    self.opponents(me)
                } else if let Some(o) = self.exact_life_target(me, text) {
                    o.into_iter().collect()
                } else if let Some(o) = ctx
                    .about
                    .filter(|&o| o != me && text.contains("that player loses"))
                {
                    // "Whenever an opponent casts a spell, that player loses 5 life".
                    vec![o]
                } else {
                    ai::burn_opponent(self, me, n).into_iter().collect()
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
                    if text.contains(" unless that player") || text.contains(" unless they ") {
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
                        let discards =
                            text.contains("discards a card") || text.contains("discard a card");
                        if discards && !self.players[o].hand.is_empty() {
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
                // "Spend this mana only to cast Dragon spells" (Desolation of Smaug): only such
                // a spell, cast now - the mana is gone when the phase ends (CR 106.4, 106.6).
                if let Some(rule) = text
                    .split("spend this mana only to cast ")
                    .nth(1)
                    .filter(|_| !text.starts_with("{t}"))
                {
                    let what = rule
                        .split(" spell")
                        .next()
                        .unwrap_or("")
                        .trim_start_matches("a ")
                        .trim_start_matches("an ")
                        .trim();
                    let fits = |c: &SimCard| match what {
                        "creature" => c.is(T_CREATURE),
                        "artifact" => c.is(T_ARTIFACT),
                        "instant or sorcery" => c.is(T_INSTANT | T_SORCERY),
                        w => c.has_subtype(w.trim_end_matches('s')),
                    };
                    let before = self.players[me].pool;
                    self.players[me].pool = before.saturating_add(k.clamp(0, 255) as u8);
                    let pl = &self.players[me];
                    let pick = (0..pl.hand.len())
                        .filter(|&h| fits(&pl.cards[pl.hand[h] as usize]))
                        .filter(|&h| self.can_cast(me, super::Src::Hand(h), Default::default()))
                        .max_by_key(|&h| pl.cards[pl.hand[h] as usize].mv);
                    if let Some(h) = pick {
                        self.cast(me, super::Src::Hand(h), Default::default());
                    }
                    // What's left of it can't pay for anything else.
                    self.players[me].pool = self.players[me].pool.min(before);
                    return;
                }
                self.players[me].pool = self.players[me].pool.saturating_add(k as u8);
            }
            // "Draw a card, then discard a card. If you discard a land card this way, put it
            // from your graveyard onto the battlefield tapped" (Silvan Reveler).
            Effect::FetchLand { .. } if text.contains("if you discard a land card this way") => {
                if let Some(&c) = self.players[me].graveyard.last() {
                    if self.players[me].cards[c as usize].is(T_LAND) {
                        self.players[me].graveyard.pop();
                        if let Some(i) = self.enter_card(me, c, me, false) {
                            self.perms[i].tapped = text.contains("battlefield tapped");
                        }
                    }
                }
            }
            // "Return target land card from your graveyard to the battlefield" (Primal
            // Witchstalker): from the graveyard, if there is one.
            Effect::FetchLand { .. }
                if text.contains("land card from your graveyard to the battlefield") =>
            {
                let pl = &self.players[me];
                let pick = (0..pl.graveyard.len())
                    .filter(|&k| pl.cards[pl.graveyard[k] as usize].is(T_LAND))
                    .max_by_key(|&k| pl.cards[pl.graveyard[k] as usize].produces.count_ones());
                if let Some(k) = pick {
                    let c = self.players[me].graveyard.remove(k);
                    self.emit(Ev::LeavesGraveyard, me, None);
                    if let Some(i) = self.enter_card(me, c, me, false) {
                        self.perms[i].tapped |= text.contains("battlefield tapped");
                    }
                }
            }
            // "Destroy target nonbasic land an opponent controls. ... You may search your
            // library": only with something to destroy (Demolition Field).
            Effect::FetchLand { .. }
                if text.contains("destroy target nonbasic land") && self.last_destroyed == 0 => {}
            Effect::FetchLand { battlefield } => {
                let basic = text.contains("basic");
                // "Search your library for a basic land card, reveal it, then shuffle and put
                // that card on top" / "put it into your graveyard" / "exile them".
                let elsewhere = !*battlefield
                    && (text.contains("put that card on top")
                        || text.contains("put it into your graveyard")
                        || text.contains(", exile them"));
                if elsewhere {
                    let n = if text.contains("up to two") { 2 } else { 1 };
                    for _ in 0..n {
                        let before = self.players[me].hand.len();
                        self.fetch_land(me, false, false, basic);
                        if self.players[me].hand.len() > before {
                            let c = self.players[me].hand.pop().unwrap_or(0);
                            if text.contains("put that card on top") {
                                self.players[me].library.push(c);
                            } else if text.contains("put it into your graveyard") {
                                self.players[me].graveyard.push(c);
                            } else {
                                self.players[me].exile.push(c);
                            }
                        }
                    }
                    return;
                }
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
            // "Put onto the battlefield under your control all creature cards in your
            // opponents' graveyards that were put there from the battlefield this turn. They
            // are Food artifacts" (Supper for Spiders).
            Effect::Reanimate
                if text
                    .contains("graveyards that were put there from the battlefield this turn") =>
            {
                let died: Vec<CardRef> = self
                    .dead
                    .iter()
                    .filter(|x| x.tok == Tok::None && x.owner as usize != me)
                    .filter_map(|x| x.card)
                    .collect();
                for (o, c) in died {
                    let o = o as usize;
                    let Some(k) = self.players[o].graveyard.iter().rposition(|&g| g == c) else {
                        continue;
                    };
                    if !self.players[o].cards[c as usize].is(T_CREATURE) {
                        continue;
                    }
                    self.players[o].graveyard.remove(k);
                    self.emit(Ev::LeavesGraveyard, o, None);
                    if let Some(i) = self.enter_card(o, c, me, false) {
                        if text.contains("food artifacts") {
                            let x = &mut self.perms[i];
                            x.types = (x.types & T_LEGENDARY) | T_ARTIFACT;
                            x.subtypes = vec!["food".into()];
                        }
                    }
                }
                self.recalc();
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
                        // "If this spell was cast using teamwork, instead choose target
                        // creature card" (Too Evil to Stay Dead): any creature card.
                        None if ctx.teamwork
                            && ctx.card.is_some_and(|c| c.teamwork.is_some())
                            && text.starts_with("choose target") =>
                        {
                            ai::reanimate_choice(self, me, any, &Default::default(), &d)
                        }
                        None => ai::reanimate_choice(self, me, any, &ctx.card_restr, &d),
                    };
                    let Some((o, k)) = pick else {
                        break;
                    };
                    let itself = fixed.is_some() && self.returns_itself(ctx);
                    if itself && self.reborn_blocked(ctx) {
                        break;
                    }
                    // "Return it to the battlefield with one fewer revival counter on it at the
                    // beginning of the next end step" (Nine-Lives Familiar).
                    if itself && text.contains("at the beginning of the next end step") {
                        let had = ctx.snap.as_ref().map_or(0, |s| s.other_counters);
                        let fewer = text.contains("one fewer");
                        self.ext.delayed.push(Delayed::Return {
                            phase: Phase::End,
                            owner: o as u8,
                            card: self.players[o].graveyard[k],
                            ctrl: me as u8,
                            from_graveyard: true,
                            other_counters: if fewer { had - 1 } else { had },
                        });
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
                } else if text.contains("put there from anywhere this turn") {
                    // "Target permanent card in your graveyard that was put there from
                    // anywhere this turn" (Night Nurse): the best of those.
                    let fresh = self.new_in_graveyard(me);
                    let pl = &self.players[me];
                    fresh
                        .into_iter()
                        .filter(|&k| pl.cards[pl.graveyard[k] as usize].is_permanent())
                        .max_by(|&a, &b| {
                            let va = pl.cards[pl.graveyard[a] as usize].value;
                            let vb = pl.cards[pl.graveyard[b] as usize].value;
                            va.partial_cmp(&vb).unwrap()
                        })
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
                // "Return up to two target creature cards ... to your hand, then discard a card"
                // (Macabre Waltz).
                if text.contains("to your hand, then discard a card") {
                    self.discard(me);
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
                    let mut cands = self.candidates(ctx, &t2, false);
                    // "If that creature is a Bear, put a +1/+1 counter on it": a Bear if any.
                    if let Some(sub) = text
                        .split("if that creature is a ")
                        .nth(1)
                        .and_then(|r| r.split([',', ' ']).next())
                    {
                        let fits: Vec<usize> = cands
                            .iter()
                            .copied()
                            .filter(|&i| self.perms[i].subtypes.iter().any(|s| s == sub))
                            .collect();
                        if !fits.is_empty() {
                            cands = fits;
                        }
                    }
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
                // Who becomes the copy: the enchanted creature (Secret Invasion), one of our
                // artifacts (Shuri: the least valuable), or the permanent itself.
                let copier = if text.contains("enchanted creature becomes a copy")
                    || text.contains("equipped creature becomes a copy")
                {
                    self.src_perm(ctx)
                        .and_then(|s| s.attached)
                        .and_then(|id| self.idx(id))
                } else if text.contains("target artifact you control becomes a copy") {
                    (0..self.perms.len())
                        .filter(|&i| {
                            let x = &self.perms[i];
                            x.ctrl as usize == me && x.is(T_ARTIFACT) && !x.is(T_LAND)
                        })
                        .min_by(|&a, &b| {
                            ai::perm_value(self, a)
                                .partial_cmp(&ai::perm_value(self, b))
                                .unwrap()
                        })
                } else {
                    self.src_idx(ctx)
                };
                let Some(s) = copier else { return };
                let s_id = self.perms[s].id;
                // What it copies: the creature just exiled ("a copy of that creature"), our
                // best other artifact ("a second target artifact you control"), or the best
                // creature it may copy.
                let model: Option<Perm> = if text.contains("copy of that creature") {
                    self.last_left.clone()
                } else if text.contains("second target artifact you control") {
                    (0..self.perms.len())
                        .filter(|&i| {
                            let x = &self.perms[i];
                            x.ctrl as usize == me && x.is(T_ARTIFACT) && x.id != s_id
                        })
                        .max_by(|&a, &b| {
                            ai::perm_value(self, a)
                                .partial_cmp(&ai::perm_value(self, b))
                                .unwrap()
                        })
                        .map(|i| self.perms[i].clone())
                } else {
                    let any = !text.contains("you control");
                    ai::best_to_copy(self, me, any)
                        .filter(|&t| t != s)
                        .map(|t| self.perms[t].clone())
                };
                let Some(m) = model else { return };
                let eot = text.contains("until end of turn");
                let x = &mut self.perms[s];
                if eot {
                    x.temp_base = Some((m.base_p, m.base_t));
                    x.temp_kw |= m.kw;
                    if m.is_creature() {
                        x.temp_creature = true;
                    }
                } else {
                    x.base_p = m.base_p;
                    x.base_t = m.base_t;
                    x.kw |= m.kw;
                    x.types |= m.types & (T_CREATURE | T_ARTIFACT);
                }
                self.recalc();
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
                // "Destroy up to one other target creature. Its controller amasses Goblins X,
                // where X is that creature's power" (Azog): the destroyed creature's player.
                let (me, k) = if text.contains("its controller amasses") {
                    match self.last_left.as_ref().filter(|_| self.last_destroyed > 0) {
                        Some(x) => (x.ctrl as usize, x.p.max(0) as i32),
                        None => return,
                    }
                } else if text.contains("that excess damage") {
                    (me, self.last_excess.max(0))
                } else {
                    (me, self.x_value(ctx, n))
                };
                if k <= 0 && text.contains("excess damage") {
                    return;
                }
                if !self.players[me].alive {
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
                    if x.hone > 0 {
                        x.hone += 1;
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
                // "Exile two target creatures and/or lands you control, then return them".
                let times = if text.contains("exile two target") {
                    2
                } else {
                    1
                };
                let mut done: Vec<u32> = Vec::new();
                let delayed = text.contains("at the beginning of the next end step");
                for _ in 0..times {
                    let target = if text.contains("~") && !text.contains("target") {
                        self.src_idx(ctx)
                    } else {
                        // "Exile (another / up to one) target creature you control": not
                        // itself.
                        let src = ctx.src.filter(|_| text.contains("target"));
                        if delayed {
                            ai::blink_choice(self, me, src, blink_types(text))
                        } else {
                            ai::blink_choice_now(self, me, src, blink_types(text))
                        }
                        .filter(|&i| !done.contains(&self.perms[i].id))
                    };
                    let Some(i) = target else { return };
                    let x = self.perms[i].clone();
                    let (Some(r), Tok::None) = (x.card, x.tok) else {
                        self.leave(i, Zone::Exile, false);
                        return;
                    };
                    // A commander returned at once: its owner leaves it in exile (it isn't
                    // moved to the command zone) and it comes back still a commander.
                    let commander = x.commander;
                    if commander > 0 {
                        if delayed || x.owner as usize != me || !ai::blinks_commanders(me) {
                            return;
                        }
                        self.perms[i].commander = 0;
                    }
                    self.leave(i, Zone::Exile, false);
                    let owner = x.owner as usize;
                    let back_ctrl = if text.contains("under your control") {
                        me
                    } else if text.contains("under its owner's control") {
                        owner
                    } else {
                        x.ctrl as usize
                    };
                    // "Return it ... at the beginning of the next end step".
                    if text.contains("at the beginning of the next end step") {
                        self.ext.delayed.push(Delayed::Return {
                            phase: Phase::End,
                            owner: x.owner,
                            card: r.1,
                            ctrl: back_ctrl as u8,
                            from_graveyard: false,
                            other_counters: 0,
                        });
                        continue;
                    }
                    if let Some(k) = self.players[owner].exile.iter().rposition(|&c| c == r.1) {
                        self.players[owner].exile.remove(k);
                        if let Some(j) = self.enter_card(owner, r.1, back_ctrl, false) {
                            self.perms[j].sick = x.sick;
                            self.perms[j].commander = commander;
                            done.push(self.perms[j].id);
                        }
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
                    // "... put it into your hand, shuffle, then discard a card at random".
                    if text.contains("then discard a card at random") {
                        let n = self.players[me].hand.len();
                        if n > 0 {
                            let h = self.rand(n);
                            let c = self.players[me].hand.remove(h);
                            self.put_discarded(me, c);
                        }
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
            // Uldaros Theorix: "exile up to one target nonland card of each card type from
            // your graveyard. Copy those cards. You may cast any number of spells with total
            // mana value 6 or less from among the copies" - permanents become tokens.
            Effect::FreeSpell
                if text.contains("from among the copies") && text.contains("of each card type") =>
            {
                self.cast_graveyard_copies(ctx);
            }
            // Copies of the cards exiled to pay for it (Baron Helmut Zemo).
            Effect::FreeSpell if self.free_cast_elsewhere(ctx) => {}
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
            // "Whenever enchanted creature attacks or blocks, attach any number of target
            // Equipment you control to it" (Super-Soldier Serum): your Equipment moves onto
            // the creature this Aura enchants.
            Effect::Attach if text.contains("target equipment you control to it") => {
                let Some(host) = self.src_perm(ctx).and_then(|s| s.attached) else {
                    return;
                };
                let many = text.contains("any number of");
                let gear: Vec<usize> = (0..self.perms.len())
                    .filter(|&i| {
                        let x = &self.perms[i];
                        x.ctrl as usize == me && x.is(T_EQUIPMENT) && x.attached != Some(host)
                    })
                    .collect();
                for i in gear.into_iter().take(if many { usize::MAX } else { 1 }) {
                    self.perms[i].attached = Some(host);
                }
                self.recalc();
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
            // "You may cast that card. If you don't, ~ deals 2 damage to each opponent".
            Effect::Dig if text.contains("you may cast that card. if you don't, ") => {
                self.cast_or_else(ctx)
            }
            // Impulse draws: the cards are exiled and may be played for a while.
            Effect::Dig if impulse_of(text).is_some() => {
                let imp = impulse_of(text).unwrap_or_default();
                self.impulse(ctx, imp);
            }
            // (Handled with its "onto the battlefield" part: `dig_onto_battlefield`.)
            Effect::Dig
                if text.contains("until you reveal")
                    && text.contains("otherwise, put it into your hand") => {}
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
                // "You may reveal a Dwarf or Equipment card from among them and put it into
                // your hand": only such a card.
                let reveal = text
                    .split("you may reveal ")
                    .nth(1)
                    .and_then(|r| r.split(" from among").next())
                    .map(restrict::Restr::read)
                    .unwrap_or_default();
                for _ in 0..keep {
                    ai::dig(self, me, n, text.contains("land"), &reveal);
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
                    // A keyword counter goes where the keyword is new.
                    let bits = keyword_bits(std::slice::from_ref(kind));
                    let lacking: Vec<usize> = self
                        .candidates(ctx, &t, harmful)
                        .into_iter()
                        .filter(|&i| bits != 0 && !self.perms[i].has(bits))
                        .collect();
                    if !harmful && !lacking.is_empty() {
                        let pick = ai::choose_friends(self, me, &lacking, 1);
                        let ids: Vec<u32> = pick.iter().map(|&i| self.perms[i].id).collect();
                        self.targeted = true;
                        self.last_targets = ids.clone();
                        ids
                    } else {
                        self.choose(ctx, &t, harmful)
                    }
                };
                // "Put a trample counter on up to one target creature you control. It becomes
                // a Bear in addition to its other types" (Beorn).
                if let Some(sub) = text
                    .split("it becomes a ")
                    .nth(1)
                    .filter(|r| r.contains(" in addition to its other types"))
                    .and_then(|r| r.split_whitespace().next())
                {
                    for &id in &ids {
                        if let Some(i) = self.idx(id) {
                            if !self.perms[i].subtypes.iter().any(|s| s == sub) {
                                self.perms[i].subtypes.push(sub.to_string());
                            }
                        }
                    }
                }
                for id in ids {
                    let Some(i) = self.idx(id) else { continue };
                    // "Put X stun counters on it", "a hone counter on ~ for each creature
                    // target opponent controls".
                    let amt = if text.contains(&format!("put x {} counters", kind))
                        || text.contains(&format!("{} counter on ~ for each", kind))
                    {
                        self.x_value(ctx, &Amount { n: 3, x: true }).clamp(0, 30)
                    } else {
                        1
                    };
                    match kind.as_str() {
                        // Put on a returning card as it comes back (`reborn_details`).
                        "finality" => {}
                        "loyalty" => self.perms[i].loyalty += amt as i16,
                        "shield" => self.perms[i].shield += amt as u8,
                        "stun" => self.perms[i].stun += amt as u8,
                        k if KEYWORDS.iter().any(|(w, _)| *w == k) => {
                            self.perms[i].kw |= keyword_bits(&[k.to_string()]);
                        }
                        "hone" if amt > 0 => {
                            let before = self.perms[i].other_counters;
                            self.add_other_counters(i, amt as i16);
                            if let Some(i) = self.idx(id) {
                                let added = self.perms[i].other_counters - before;
                                self.perms[i].hone += added.max(0);
                            }
                            self.recalc();
                        }
                        _ if amt > 0 => self.add_other_counters(i, amt as i16),
                        _ => {}
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
        // "Reveal cards until you reveal a creature card. If its mana value is less than or
        // equal to the number of lands you control, put it onto the battlefield. Otherwise,
        // put it into your hand" (Part in Friendship): the first creature, either way.
        let otherwise_hand = until && t.contains("otherwise, put it into your hand");
        let stops = |g: &Self, c: u16| {
            if otherwise_hand {
                g.players[me].cards[c as usize].types & types != 0
            } else {
                fits(g, c)
            }
        };
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
            let stop = (0..lib.len()).rev().find(|&j| stops(self, lib[j]));
            match stop {
                Some(j) => self.players[me].library.split_off(j),
                None => std::mem::take(&mut self.players[me].library),
            }
        } else {
            self.players[me].library.split_off(k)
        };
        // "You may put a creature card ... If this spell was cast using teamwork, put any
        // number of creature cards ... instead" (Earth's Mightiest Heroes).
        let all = t.contains("any number of")
            && (ctx.teamwork || !t.contains("cast using teamwork, put any number"));
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
        // Too big for the battlefield: into the hand instead.
        if otherwise_hand && picks.is_empty() {
            if let Some(j) = rest.iter().rposition(|&c| stops(self, c)) {
                let c = rest.remove(j);
                self.players[me].hand.push(c);
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
                let n: i16 = match text
                    .split("gets -")
                    .nth(1)
                    .and_then(|r| r.split('/').next())
                {
                    // "-X/-0, where X is the number of cards in your graveyard".
                    Some("x") => self.x_value(ctx, &Amount { n: 2, x: true }) as i16,
                    Some(d) => d.parse().unwrap_or(2),
                    None => 2,
                };
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
            // In our combat, before blocks: the defender's blocker that would cost us most.
            "can't block this turn" if ai::net_target(self, me).is_some() => {
                if let Some((i, _)) = ai::net_target(self, me) {
                    self.perms[i].temp_kw |= CANT_BLOCK;
                    if self.log.is_some() {
                        let d = self.id_desc(self.perms[i].id);
                        self.note(|| format!("  {} can't block this turn", d));
                    }
                    self.recalc();
                }
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
            "attacks change until your next turn" => {
                let Some(pt) = text.split(" get").nth(1) else {
                    return;
                };
                let pt = pt.trim_start_matches('s').trim();
                let pt = pt.split(' ').next().unwrap_or("");
                let Some((p, t)) = pt.split_once('/') else {
                    return;
                };
                let num = |s: &str| s.trim_start_matches('+').parse::<i16>().ok();
                let (Some(dp), Some(dt)) = (num(p), num(t)) else {
                    return;
                };
                if text.contains("attacks you") {
                    self.players[me].attacked_power += dp;
                } else {
                    let kw = KEYWORDS
                        .iter()
                        .filter(|(k, _)| text.contains(&format!("gain {}", k)))
                        .fold(0, |m, (_, b)| m | b);
                    self.players[me].attack_bonus = Some((dp, dt, kw));
                }
            }
            "spells cost less this turn" => {
                let Some(rest) = text.split(" cost {").nth(1) else {
                    return;
                };
                let n = rest
                    .split('}')
                    .next()
                    .and_then(|d| d.parse::<u8>().ok())
                    .unwrap_or(1);
                let what = text
                    .split(" spells you cast this turn")
                    .next()
                    .unwrap_or("");
                let types = filter_of(what).types;
                let types = if types == 0 { 0xFFFF } else { types };
                self.players[me].turn_reduction.push((n, types));
            }
            "the next spell this turn is free" => {
                let what = text.split("the next ").nth(1).unwrap_or("");
                let what = what.split(" spell").next().unwrap_or("");
                let colors = what
                    .split([' ', ','])
                    .fold(0u8, |m, w| m | crate::playtest::compile::color_word(w));
                let types = filter_of(what).types;
                self.players[me].free_next = Some((
                    if colors == 0 { 0b11111 } else { colors },
                    if types == 0 { 0xFFFF } else { types },
                ));
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
                    for &c in &hand {
                        self.players[o].graveyard.push(c);
                        self.emit(Ev::Discard, o, None);
                    }
                    self.to_graveyard(o, &hand);
                }
            }
            // "You lose life equal to the number of cards drawn this way".
            "you lose life equal to it" if text.contains("cards drawn this way") => {
                let n = self.last_drawn;
                self.lose_life(me, n);
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
                    let n = gy.len();
                    self.note(|| format!("  P{}'s graveyard is exiled ({} cards)", o, n));
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
            "mountains make an extra red mana" => {
                self.players[me].px.mountain_bonus = self.ext.turn_no;
            }
            // "Exchange control of two target nonland permanents that share a card type"
            // (Burglar's Plot): their best for your least useful of the same type.
            "exchanges control" => {
                let kinds = [
                    T_CREATURE,
                    T_ARTIFACT,
                    T_ENCHANTMENT,
                    T_PLANESWALKER,
                    T_LAND,
                ];
                let mut best: Option<(f32, usize, usize)> = None;
                for &ty in &kinds {
                    if text.contains("nonland") && ty == T_LAND {
                        continue;
                    }
                    let mine = (0..self.perms.len())
                        .filter(|&i| {
                            let x = &self.perms[i];
                            x.ctrl as usize == me && x.is(ty) && x.commander == 0
                        })
                        .min_by(|&a, &b| {
                            ai::perm_value(self, a)
                                .partial_cmp(&ai::perm_value(self, b))
                                .unwrap()
                        });
                    let theirs = (0..self.perms.len())
                        .filter(|&i| {
                            let x = &self.perms[i];
                            x.ctrl as usize != me && x.is(ty) && !x.has(HEXPROOF | SHROUD)
                        })
                        .max_by(|&a, &b| {
                            ai::perm_value(self, a)
                                .partial_cmp(&ai::perm_value(self, b))
                                .unwrap()
                        });
                    if let (Some(m), Some(o)) = (mine, theirs) {
                        let gain = ai::perm_value(self, o) - ai::perm_value(self, m);
                        if gain > 0.0 && best.is_none_or(|b| gain > b.0) {
                            best = Some((gain, m, o));
                        }
                    }
                }
                if let Some((_, m, o)) = best {
                    let (cm, co) = (self.perms[m].ctrl, self.perms[o].ctrl);
                    self.perms[m].ctrl = co;
                    self.perms[o].ctrl = cm;
                    self.perms[o].sick = true;
                }
            }
            // "Exile all but the bottom card of each opponent's library".
            "exiles opponents' libraries" => {
                for o in self.opponents(me) {
                    let keep = if text.contains("all but the bottom card") {
                        1
                    } else {
                        0
                    };
                    let pl = &mut self.players[o];
                    let gone = pl.library.split_off(keep.min(pl.library.len()));
                    pl.exile.extend(gone);
                }
            }
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

/// The number a phrase starts with ("five cards" -> 5, "card" -> 1).
fn word_number(phrase: &str) -> Option<usize> {
    let w = phrase.split_whitespace().next()?;
    Some(match w {
        "card" | "a" | "one" => 1,
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        "eight" => 8,
        "nine" => 9,
        "ten" => 10,
        d => d.parse().ok()?,
    })
}

/// "... if this is the first time this ability has resolved this turn. If it's the second
/// time, ...": which time (1, 2, 3) effect `k`'s sentence is for, if it names one.
fn ordinal_of_effect(text: &str, k: usize) -> Option<u8> {
    let mut start = 0;
    loop {
        let next = text[start..]
            .find(". ")
            .map(|i| start + i + 1)
            .unwrap_or(text.len());
        if k < crate::abilities::effects_count(&text[..next]) || next >= text.len() {
            let sentence = &text[start..next];
            return [("first time", 1), ("second time", 2), ("third time", 3)]
                .iter()
                .find(|(w, _)| sentence.contains(w))
                .map(|&(_, n)| n);
        }
        start = next + 1;
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
    /// Uldaros Theorix: the best graveyard card of each card type, total mana value up to
    /// the limit, exiled and cast as free copies (permanents as tokens).
    fn cast_graveyard_copies(&mut self, ctx: &Ctx) {
        let me = ctx.ctrl;
        let total = ctx
            .text
            .split("total mana value ")
            .nth(1)
            .and_then(|r| r.split_whitespace().next())
            .and_then(|d| d.parse::<u32>().ok())
            .unwrap_or(6);
        let pl = &self.players[me];
        let mut order: Vec<usize> = (0..pl.graveyard.len())
            .filter(|&k| !pl.cards[pl.graveyard[k] as usize].is(T_LAND))
            .collect();
        order.sort_by(|&a, &b| {
            let v = |k: usize| pl.cards[pl.graveyard[k] as usize].value;
            v(b).partial_cmp(&v(a)).unwrap()
        });
        let mut types_used = 0u16;
        let mut mv = 0u32;
        let mut picks: Vec<u16> = Vec::new();
        let main = T_CREATURE | T_ARTIFACT | T_ENCHANTMENT | T_PLANESWALKER | T_INSTANT | T_SORCERY;
        for k in order {
            let c = &pl.cards[pl.graveyard[k] as usize];
            let t = c.types & main;
            if t & types_used != 0 || mv + c.mv as u32 > total {
                continue;
            }
            types_used |= t;
            mv += c.mv as u32;
            picks.push(pl.graveyard[k]);
        }
        for c in picks {
            if let Some(k) = self.players[me].graveyard.iter().rposition(|&x| x == c) {
                self.players[me].graveyard.remove(k);
                self.players[me].exile.push(c);
            }
            let r = (me as u8, c);
            if self.card(r).is_permanent() {
                let i = self.new_perm(me, me, Some(r), Tok::Other);
                self.after_enter(i);
            } else {
                self.copy_spell(me, (r, false));
            }
        }
        self.emit(Ev::LeavesGraveyard, me, None);
    }

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
        for w in before.iter().rev().take(5) {
            let w = w.trim_matches(|c: char| !c.is_alphabetic());
            // "Robot Villain artifact creature token": the types come before "artifact".
            if w == "artifact" {
                continue;
            }
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
