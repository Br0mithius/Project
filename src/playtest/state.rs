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
    /// A finality counter: if it would die, it's exiled instead.
    pub finality: bool,
    /// Lost all its abilities (Hellcat back from the dead).
    pub no_abilities: bool,
    /// A token copy sacrificed at the beginning of the end step (Electroduplicate).
    pub sac_at_end: bool,
    /// Modes already chosen ("choose one that hasn't been chosen"), a bit per mode.
    pub modes_used: u32,
    /// Once-each-turn triggers that already fired this turn (bit per ability).
    pub fired: u32,
    /// Mana value (of the card it is, or copies; 0 for other tokens).
    pub mv: u8,
    /// In combat this turn: declared as an attacker / blocking.
    pub attacking: bool,
    pub blocking: bool,
    /// Dealt damage this turn.
    pub dealt_damage: bool,
    /// Attacked this turn / entered the battlefield this turn.
    pub attacked_turn: bool,
    pub entered_turn: bool,
    /// Players it dealt combat damage to this turn (bit per player).
    pub hit_turn: u8,
    /// It has dealt combat damage at some point.
    pub ever_hit: bool,
    /// The last source that dealt damage to it this turn.
    pub hurt_by: u32,
    // Values after static effects (`Game::recalc`).
    pub p: i16,
    pub t: i16,
    pub eff_kw: u32,
    pub ward: u8,
    pub protection: u8,
    /// Hexproof from these colours (bits 0-4) or instants (bit 5), from its card.
    pub hexproof_from: u8,
    /// A transforming double-faced card showing its back face.
    pub transformed: bool,
    /// Lost all its abilities to an Aura ("enchanted creature loses all abilities"), worked
    /// out by `recalc`.
    pub blank: bool,
    /// Made a creature by an Aura ("enchanted permanent is a Construct creature"), by `recalc`.
    pub animated: bool,
    /// A creature until end of turn ("this land becomes a 0/0 Elemental creature until end of
    /// turn").
    pub temp_creature: bool,
    /// "Has base power and toughness 4/5 until end of turn".
    pub temp_base: Option<(i16, i16)>,
    /// "When this creature dies, return it to the battlefield tapped" until end of turn:
    /// `RETURNS` plus `RETURNS_COUNTER` (Undying Malice: with a +1/+1 counter) or
    /// `RETURNS_TREASURE` (Fake Your Own Death: and you create a Treasure).
    pub returns_on_death: u8,
    /// The card named as it entered (Sorcerous Spyglass).
    pub named: Option<CardRef>,
    /// "+N/+M until your next turn": (power, toughness, whose next turn ends it).
    pub till: Option<(i16, i16, u8)>,
}

impl Perm {
    pub fn is(&self, t: u16) -> bool {
        self.types & t != 0
    }

    pub fn is_creature(&self) -> bool {
        self.is(T_CREATURE) || self.crewed || self.animated || self.temp_creature
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
    /// Extra land drops this turn ("you may play an additional land this turn").
    pub extra_lands: u8,
    /// Life gained and cards discarded this turn (amounts).
    pub life_gained_turn: i32,
    pub discarded_turn: u8,
    /// Lost life during the previous turn.
    pub lost_life_last_turn: bool,
    pub scried_turn: bool,
    pub loyalty_turn: bool,
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
    /// "When you next cast an instant or sorcery spell this turn, copy that spell": one
    /// entry per copy waiting, with the highest mana value it copies.
    pub copy_next: Vec<u8>,
    /// Instants and sorceries cast this turn (Thousand-Year Storm).
    pub inst_sorc_this_turn: u8,
    /// The last activated or triggered ability of a creature this player resolved (Echo).
    pub last_ability: Option<Pending>,
    pub mulligans: u8,
    /// Extra turns to take after this one (Kang).
    pub extra_turns: u8,
    /// This is an extra turn in which power-up abilities can't be activated.
    pub no_powerups: bool,
    /// Creature and noncreature spells cast this turn.
    pub creature_spells_turn: u8,
    pub noncreature_spells_turn: u8,
    /// Permanent types already cast from the graveyard this turn (Muldrotha), and whether
    /// a land was played from there.
    pub gy_types_used: u16,
    /// Cards in exile this player may play: Thor's exiled card, impulse draws, stashed cards.
    pub playable: Vec<Playable>,
    /// "Creatures you control with toughness greater than their power assign combat damage
    /// equal to their toughness" this turn (The Kingpin of Crime).
    pub toughness_damage_turn: bool,
    /// Emblems: (the card that gave it, index into its `SimCard::emblems`).
    pub emblems: Vec<(CardRef, u16)>,
    /// Equip abilities activated this turn (Kíli: the first may cost {0}).
    pub equips_turn: u8,
}

pub const RETURNS: u8 = 1;
pub const RETURNS_COUNTER: u8 = 2;
pub const RETURNS_TREASURE: u8 = 4;

/// `Pending::ability` values from here on are emblem abilities (`SimCard::emblems`).
pub const EMBLEM_BASE: u16 = 0x4000;

/// A card in exile its holder may play: (whose list, index), until the end of the turn of
/// player `until.1` in round `until.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Playable {
    pub card: CardRef,
    pub until: (u32, u8),
    /// "Only during your turn" (stash counters).
    pub own_turn: bool,
    /// "Pay life equal to its mana value rather than pay its mana cost" (Inside Information).
    pub pay_life: bool,
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
    /// The X paid for the spell or ability.
    pub x: Option<i32>,
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
    /// Creatures that died this turn, as they last were (for "if it was attacking").
    pub dead: Vec<Perm>,
    /// Excess damage dealt by the last damaging effect ("if excess damage was dealt").
    pub last_excess: i32,
    /// The card the resolving ability last dealt with (returned, destroyed, revealed), for
    /// "if it's a Zombie card, ...".
    pub focus_card: Option<CardRef>,
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
    /// Whose spell or ability is acting: targeting ("becomes the target of a spell or
    /// ability an opponent controls") or dealing noncombat damage ("a source you control").
    pub actor: u8,
    /// Bumped whenever the board changes (`recalc`); the AI caches values per tick.
    pub tick: u64,
    /// Per permanent index: (tick, id, value).
    pub vcache: std::cell::RefCell<Vec<(u64, u32, f32)>>,
    /// Per player: (tick, threat).
    pub tcache: std::cell::RefCell<[(u64, f32); 8]>,
    /// Per (me, opponent): (tick, weight).
    pub wcache: std::cell::RefCell<[[(u64, f32); 8]; 8]>,
    /// A card that just returned to the battlefield from its owner's graveyard by its own
    /// ability: the rest of that ability ("she gains haste") is about it.
    pub reborn: Option<u32>,
    /// X paid for the permanent spell about to enter ("enters with X +1/+1 counters").
    pub entering_x: Option<i32>,
    /// X paid for the spell being cast (its own abilities and enters triggers use it).
    pub cur_x: Option<i32>,
    /// The spell being cast (for "copy that spell" cast triggers): (card, back face).
    pub casting: Option<(CardRef, bool)>,
    /// The permanent spell about to enter was kicked.
    pub entering_kicked: bool,
    /// The last permanent to leave the battlefield, as it last was ("that permanent's mana
    /// value", "its controller gains life equal to its power").
    pub last_left: Option<Perm>,
    /// Cards the last "discard your hand" discarded ("draw that many").
    pub last_discarded: i32,
    /// Permanents that got +1/+1 counters during the ability resolving ("those creatures").
    pub last_countered: Vec<u32>,
    /// Permanents the last destroy effect destroyed ("for each creature destroyed this way").
    pub last_destroyed: i32,
    /// Cards the last draw effect drew ("the number of cards drawn this way").
    pub last_drawn: i32,
    /// Rule-changing statics (`R_*`) each player has from their permanents (`recalc`).
    pub rules: [u32; 8],
    /// "End the turn" (Time Stop): the rest of the turn is skipped.
    pub end_turn: bool,
    /// Cards exiled to return to the battlefield under their owner's control at the
    /// beginning of the next end step.
    pub delayed_returns: Vec<CardRef>,
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
                extra: ActCost::default(),
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
            from_graveyard: false,
            card_restr: Default::default(),
            pay: None,
            gate: None,
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
            from_graveyard: false,
            card_restr: Default::default(),
            pay: None,
            gate: None,
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
        if perm.no_abilities || perm.blank {
            return &[];
        }
        match (perm.tok, perm.card) {
            (Tok::None | Tok::Creature | Tok::Other, Some(r)) => {
                let c = self.card(r);
                match (&c.back, perm.transformed) {
                    (Some(b), true) => &b.abilities,
                    _ => &c.abilities,
                }
            }
            (t, _) => token_abilities(t),
        }
    }

    /// The rule-changing statics (`R_*`) a permanent has.
    pub fn perm_rules(&self, perm: &Perm) -> u32 {
        if perm.no_abilities || perm.blank {
            return 0;
        }
        match (perm.tok, perm.card) {
            (Tok::None | Tok::Creature | Tok::Other, Some(r)) => {
                let c = self.card(r);
                match (&c.back, perm.transformed) {
                    (Some(b), true) => b.rules,
                    _ => c.rules,
                }
            }
            _ => 0,
        }
    }

    /// Does any player's permanent have this rule (`R_*`)?
    pub fn rule_anywhere(&self, bit: u32) -> bool {
        self.rules.iter().any(|r| r & bit != 0)
    }

    /// The combat damage creature `x` assigns: its power, or its toughness under Ghalta the
    /// Immovable / The Kingpin of Crime, or its negative power as positive (Loot).
    pub fn combat_power(&self, x: &Perm) -> i16 {
        let c = x.ctrl as usize;
        if x.t > x.p
            && (self
                .rules
                .get(c)
                .is_some_and(|r| r & R_TOUGHNESS_DAMAGE != 0)
                || self.players[c].toughness_damage_turn)
        {
            return x.t;
        }
        if x.p < 0 && self.perm_rules(x) & R_NEGATIVE_POWER != 0 {
            return -x.p;
        }
        x.p
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
            let (bp, bt) = x.temp_base.unwrap_or((x.base_p, x.base_t));
            let (tp, tt) = x.till.map_or((0, 0), |t| (t.0, t.1));
            x.p = bp + x.counters + x.temp_p + tp;
            x.t = bt + x.counters + x.temp_t + tt;
            x.eff_kw = x.kw | x.temp_kw;
            x.locked = x.lock_effect;
            x.blank = false;
            x.animated = false;
            if let Some(r) = x.card {
                let c = &self.players[r.0 as usize].cards[r.1 as usize];
                x.ward = c.ward;
                x.protection = c.protection;
                x.hexproof_from = c.hexproof_from;
            }
        }
        // Auras that make their permanent something else ("loses all abilities and is a 1/1
        // Skeleton", "is a 5/5 Construct creature"): before other statics, which a blanked
        // creature no longer has.
        let mut sets = Vec::new();
        for x in &self.perms {
            if let (Some(r), Some(a)) = (x.card, x.attached) {
                if let Some(s) = self.card(r).attached_sets.filter(|_| !x.no_abilities) {
                    sets.push((a, s));
                }
            }
        }
        for (a, s) in sets {
            let Some(i) = self.idx(a) else { continue };
            let x = &mut self.perms[i];
            x.p = s.power + x.counters + x.temp_p;
            x.t = s.toughness + x.counters + x.temp_t;
            if s.creature && !x.is(T_CREATURE) {
                x.animated = true;
            }
            if s.blank {
                x.blank = true;
                x.eff_kw = x.temp_kw;
                x.ward = 0;
                x.protection = 0;
                x.hexproof_from = 0;
            }
        }
        // Collect static effects first (they read the board), then apply.
        let mut mods: Vec<(usize, i16, i16, u32, bool)> = Vec::new();
        for si in 0..self.perms.len() {
            let src = &self.perms[si];
            // Characteristic-defining power/toughness ("~'s power and toughness are each
            // equal to the number of creatures you control").
            if let Some(cda) = src
                .card
                .and_then(|r| self.card(r).cda.as_ref())
                .filter(|_| !src.no_abilities && !src.blank)
            {
                let k = self
                    .count_of(src.ctrl as usize, Some(src.id), &cda.phrase)
                    .unwrap_or(0) as i16;
                mods.push((
                    si,
                    if cda.power { k } else { 0 },
                    if cda.toughness { k + cda.plus_t } else { 0 },
                    0,
                    false,
                ));
            }
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
                        } => {
                            // "+1/+1 for each Forest you control": the reader's typical count
                            // (3 per unit) becomes the real one.
                            let per_each = a.text.split("for each ").nth(1).and_then(|rest| {
                                self.count_of(src.ctrl as usize, Some(src.id), rest)
                            });
                            // "+2/+0 for every seven cards in your graveyard".
                            let per_each = per_each.or_else(|| {
                                let rest = a.text.split("for every ").nth(1)?;
                                let (n, what) = rest.split_once(' ')?;
                                let n = match n {
                                    "two" => 2,
                                    "three" => 3,
                                    "five" => 5,
                                    "seven" => 7,
                                    d => d.parse().ok()?,
                                };
                                // (Not scaled by the reader: three "units" per step.)
                                self.count_of(src.ctrl as usize, Some(src.id), what)
                                    .map(|k| 3 * (k / n))
                            });
                            match per_each {
                                Some(k) => (
                                    (*power * k / 3) as i16,
                                    (*toughness * k / 3) as i16,
                                    0,
                                    false,
                                    Some(to),
                                ),
                                None => (*power as i16, *toughness as i16, 0, false, Some(to)),
                            }
                        }
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
        // Emblems' static abilities ("creatures you control get +2/+2").
        for p in 0..self.players.len() {
            for &(r, k) in &self.players[p].emblems {
                let Some((_, a)) = self.card(r).emblems.get(k as usize) else {
                    continue;
                };
                if a.trig != Trig::Static {
                    continue;
                }
                for e in &a.effects {
                    let (dp, dt, kw) = match e {
                        Effect::Pump {
                            power, toughness, ..
                        } => (*power as i16, *toughness as i16, 0),
                        Effect::Grant { keywords, .. } => (0, 0, keyword_bits(keywords)),
                        _ => continue,
                    };
                    for (i, x) in self.perms.iter().enumerate() {
                        if x.ctrl as usize == p && x.is_creature() {
                            mods.push((i, dp, dt, kw, false));
                        }
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
        // Rule-changing statics, per controller.
        let mut rules = [0u32; 8];
        for x in &self.perms {
            if x.card
                .is_some_and(|r| self.card(r).rules != 0 || self.card(r).back.is_some())
            {
                rules[x.ctrl as usize % 8] |= self.perm_rules(x);
            }
        }
        self.rules = rules;
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
                            "equipment" => x.is(T_EQUIPMENT),
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
            Cond::ControlCount(n, what) => {
                mine()
                    .filter(|x| match what.as_str() {
                        "creature" => x.is_creature(),
                        "artifact" => x.is(T_ARTIFACT),
                        "enchantment" => x.is(T_ENCHANTMENT),
                        "permanent" => true,
                        w => x.subtypes.iter().any(|s| s == w),
                    })
                    .count()
                    >= *n as usize
            }
            Cond::TotalPower(n) => {
                mine()
                    .filter(|x| x.is_creature())
                    .map(|x| x.p.max(0) as i32)
                    .sum::<i32>()
                    >= *n as i32
            }
            Cond::PowerAtLeast(n) => src.is_some_and(|s| self.perms[s].p >= *n),
            Cond::CreaturesDied(n) => self.died_this_turn >= *n,
            Cond::OpponentCastSpell => self
                .players
                .iter()
                .enumerate()
                .any(|(i, o)| i != p && o.spells_this_turn > 0),
            Cond::OpponentLostLifeLastTurn => self
                .players
                .iter()
                .enumerate()
                .any(|(i, o)| i != p && o.alive && o.lost_life_last_turn),
            Cond::OpponentLifeExactly(n) => self
                .players
                .iter()
                .enumerate()
                .any(|(i, o)| i != p && o.alive && o.life == *n),
            // Need the event or the payment: checked where the ability resolves.
            Cond::XAtLeast(_)
            | Cond::SubjectBigger
            | Cond::SubjectWasAttacking
            | Cond::SubjectHurtBySource => false,
            Cond::SourceNotCreature => src.is_some_and(|s| !self.perms[s].is_creature()),
            Cond::SourceSubtype(w) => src.is_some_and(|s| self.perms[s].subtypes.contains(w)),
            Cond::JaceLoyalty(n) => {
                mine()
                    .filter(|x| x.is(T_PLANESWALKER) && x.subtypes.iter().any(|s| s == "jace"))
                    .map(|x| x.loyalty)
                    .sum::<i16>()
                    >= *n
            }
            Cond::LoyaltyActivated => pl.loyalty_turn,
            Cond::AnyPermanentColor(m) => self.perms.iter().any(|x| x.colors & m != 0),
            Cond::OtherCountersAtLeast(n) => {
                src.is_some_and(|s| self.perms[s].other_counters >= *n)
            }
            Cond::SourceAttacking => src.is_some_and(|s| self.perms[s].attacking),
            Cond::SourceHitThisTurn => src.is_some_and(|s| self.perms[s].hit_turn != 0),
            Cond::SourceNeverHit => src.is_some_and(|s| !self.perms[s].ever_hit),
            Cond::Scried => pl.scried_turn,
            Cond::AttacksAlone => mine().filter(|x| x.attacking).count() == 1,
            Cond::AttackedOrEntered(w) => {
                mine().any(|x| x.subtypes.contains(w) && (x.attacked_turn || x.entered_turn))
            }
            Cond::AttackersAtLeast(n) => mine().filter(|x| x.attacking).count() >= *n as usize,
            Cond::AttackerSubtype(w) => mine().any(|x| x.attacking && x.subtypes.contains(w)),
            Cond::AttackingPower(n) => {
                mine()
                    .filter(|x| x.attacking)
                    .map(|x| x.p.max(0) as i32)
                    .sum::<i32>()
                    >= *n as i32
            }
            Cond::CanBeholdJace => {
                mine().any(|x| x.subtypes.iter().any(|s| s == "jace"))
                    || pl
                        .hand
                        .iter()
                        .any(|&c| pl.cards[c as usize].subtypes.iter().any(|s| s == "jace"))
            }
            Cond::TappedCreatureToTarget => self
                .perms
                .iter()
                .any(|x| x.ctrl as usize != p && x.is_creature() && x.tapped),
            Cond::AttackingCreatureToTarget(nontoken) => self
                .perms
                .iter()
                .any(|x| x.ctrl as usize != p && x.attacking && !(*nontoken && x.is_token())),
            Cond::GainedLifeAtLeast(n) => pl.life_gained_turn >= *n,
            Cond::CastNoncreatureThisTurn => pl.noncreature_spells_turn > 0,
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
            // A land without a mana ability (Evolving Wilds) isn't a source.
            let land = land && (c.produces != 0 || c.mana_ability);
            if land || c.mana_ability && !(x.is_creature() && x.sick && !x.has(HASTE)) {
                let amount = match &c.mana_count {
                    Some(what) if what.contains("power") => x.p.clamp(0, 10) as u8,
                    Some(what) => self
                        .count_of(p, Some(x.id), what)
                        .map_or(c.mana_amount, |k| k.clamp(0, 10) as u8),
                    None => c.mana_amount,
                };
                if amount == 0 {
                    continue;
                }
                out.push((
                    i,
                    if c.produces == 0 && land {
                        0
                    } else {
                        c.produces
                    },
                    amount.max(1),
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
                    if !self.cant_lose(p) {
                        self.players[p].alive = false;
                    }
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
        self.players[p].life_gained_turn += n;
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
        self.last_left = Some(perm.clone());
        // A finality counter: dying exiles it instead (no dies triggers).
        let to = if perm.finality && matches!(to, Zone::Graveyard) {
            Zone::Exile
        } else {
            to
        };
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
            self.dead.push(perm.clone());
        }
        // Dies / leaves triggers of the permanent itself.
        if let Some(r) = perm.card {
            if to_gy && perm.tok == Tok::None {
                let abilities = self.abilities_of(&perm);
                for (ai, a) in abilities.iter().enumerate() {
                    if a.trig == Trig::Dies {
                        self.pending.push(Pending {
                            ctrl: perm.ctrl,
                            source: None,
                            card: r,
                            back: perm.transformed,
                            ability: ai as u16,
                            snapshot: Some(perm.clone()),
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
        // "When this creature dies, return it to the battlefield tapped under its owner's
        // control with a +1/+1 counter on it" (Undying Malice).
        if let (true, true, Some(r), Tok::None) = (
            perm.returns_on_death != 0,
            creature && to_gy,
            perm.card,
            perm.tok,
        ) {
            let owner = perm.owner as usize;
            let pl = &mut self.players[owner];
            if let Some(k) = pl.graveyard.iter().rposition(|&c| c == r.1) {
                pl.graveyard.remove(k);
                if let Some(i) = self.enter_card(r.0 as usize, r.1, owner, false) {
                    self.perms[i].tapped = true;
                    if perm.returns_on_death & RETURNS_COUNTER != 0 {
                        self.perms[i].counters += 1;
                    }
                    self.recalc();
                }
                if perm.returns_on_death & RETURNS_TREASURE != 0 {
                    self.make_item(perm.ctrl as usize, "treasure");
                }
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
        // "Prevent all damage that would be dealt to ~" (Black Panther).
        if x.has(NO_DAMAGE) {
            return;
        }
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
            finality: false,
            no_abilities: false,
            sac_at_end: false,
            modes_used: 0,
            fired: 0,
            mv,
            attacking: false,
            blocking: false,
            dealt_damage: false,
            attacked_turn: false,
            entered_turn: true,
            hit_turn: 0,
            ever_hit: false,
            hurt_by: 0,
            p,
            t,
            eff_kw: kw,
            ward: 0,
            protection: 0,
            hexproof_from: 0,
            transformed: false,
            blank: false,
            animated: false,
            temp_creature: false,
            temp_base: None,
            returns_on_death: 0,
            named: None,
            till: None,
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
        // "As this land enters, you may behold a Jace. If you don't, it enters tapped."
        let beheld = c.behold_untapped && self.cond_holds(&Cond::CanBeholdJace, ctrl, None);
        if c.is(T_LAND) && c.enters_tapped && !beheld {
            self.perms[i].tapped = true;
        }
        if let Some(cond) = &c.enters_tapped_unless {
            if !self.cond_holds(cond, ctrl, Some(i)) {
                self.perms[i].tapped = true;
            }
        }
        if c.is(T_PLANESWALKER) {
            self.perms[i].loyalty = c.loyalty;
        }
        let x = self.entering_x.take();
        let kicked = std::mem::take(&mut self.entering_kicked);
        self.after_enter(i);
        // "Enters with N (or X) +1/+1 counters": on it as it arrives, before it could die.
        for (kind, n, cond) in &c.enters_counters {
            let Some(i) = self.idx(id) else { break };
            let ok = match cond {
                Cond::Kicked => kicked,
                Cond::Teamwork | Cond::Gift | Cond::CastFromGraveyard => false,
                c => self.cond_holds(c, ctrl, Some(i)),
            };
            if !ok {
                continue;
            }
            let k = match n {
                EntersCounters::Fixed(k) => *k as i32,
                EntersCounters::X => x.unwrap_or(0),
                EntersCounters::GreatestOtherPower => self
                    .perms
                    .iter()
                    .filter(|o| o.ctrl as usize == ctrl && o.id != id && o.is_creature())
                    .map(|o| o.p as i32)
                    .max()
                    .unwrap_or(0),
            };
            if k <= 0 {
                continue;
            }
            match kind.as_str() {
                "+1/+1" => {
                    let k = if self.doubles(ctrl, "counter") {
                        k * 2
                    } else {
                        k
                    };
                    self.perms[i].counters += k as i16;
                }
                "shield" => self.perms[i].shield += k as u8,
                _ => self.perms[i].other_counters += k as i16,
            }
            self.recalc();
        }
        self.idx(id)
    }

    /// Enters-the-battlefield bookkeeping: triggers, Sagas, living weapon, Auras, legend rule.
    pub fn after_enter(&mut self, i: usize) {
        let id = self.perms[i].id;
        let ctrl = self.perms[i].ctrl as usize;
        let snap = self.perms[i].clone();
        // "Artifacts and creatures entering the battlefield don't cause abilities to
        // trigger" (Karn, Argent Defender; it counts as it enters itself).
        let hushed = snap.is(T_ARTIFACT | T_CREATURE)
            && self
                .perms
                .iter()
                .any(|x| self.perm_rules(x) & R_NO_ETB_TRIGGERS != 0);
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
                let fire = a.trig == Trig::Enters && !hushed
                    || (a.trig == Trig::Chapter(1) && c.is(T_SAGA));
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
                        x: None,
                    });
                }
            }
            if c.is(T_SAGA) {
                if let Some(e) = self.idx(id) {
                    self.perms[e].other_counters = 1;
                }
            }
            // "As ~ enters, look at an opponent's hand, then choose any card name"
            // (Sorcerous Spyglass): the opponents' most valuable permanent with activated
            // abilities.
            if c.rules & R_NAMES_CARD != 0 {
                let named = (0..self.perms.len())
                    .filter(|&j| {
                        let y = &self.perms[j];
                        y.ctrl as usize != ctrl
                            && y.card.is_some()
                            && self.abilities_of(y).iter().any(|a| {
                                matches!(
                                    a.trig,
                                    Trig::Activated { .. } | Trig::Loyalty(_) | Trig::PowerUp(_)
                                ) && !a.effects.iter().any(|e| matches!(e, Effect::AddMana(_)))
                            })
                    })
                    .max_by(|&a, &b| {
                        super::ai::perm_value(self, a)
                            .partial_cmp(&super::ai::perm_value(self, b))
                            .unwrap()
                    })
                    .and_then(|j| self.perms[j].card);
                if let Some(e) = self.idx(id) {
                    self.perms[e].named = named;
                }
            }
        }
        if snap.is(T_LAND) && !hushed {
            self.emit(Ev::Landfall, ctrl, Some(id));
        }
        if !hushed {
            self.emit_about(Ev::OtherEnters, ctrl, &snap);
            if snap.is_token() {
                self.emit(Ev::Tokens, ctrl, Some(id));
            }
        }
        self.recalc();
    }

    // ------------------------------------------------------------ triggers

    /// An event about player `p` (drawing, gaining life, landfall...).
    pub fn emit(&mut self, ev: Ev, p: usize, subject: Option<u32>) {
        if ev == Ev::Discard {
            self.players[p].discarded_turn = self.players[p].discarded_turn.saturating_add(1);
            // Tinybones: "whenever an opponent discards a card, exile it from their graveyard
            // with a stash counter on it" - the card just discarded.
            if self.rule_anywhere(R_STASH) {
                let thief = (0..self.players.len()).find(|&q| {
                    q != p
                        && self.players[q].alive
                        && self
                            .perms
                            .iter()
                            .any(|x| x.ctrl as usize == q && self.perm_rules(x) & R_STASH != 0)
                });
                if let Some(q) = thief {
                    if let Some(c) = self.players[p].graveyard.pop() {
                        self.players[p].exile.push(c);
                        self.players[q].playable.push(Playable {
                            card: (p as u8, c),
                            until: (u32::MAX, q as u8),
                            own_turn: true,
                            pay_life: false,
                        });
                    }
                }
            }
        }
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
                if *kind != ev || a.graveyard_only() {
                    continue;
                }
                if a.once_per_turn && ai < 32 && src.fired & (1 << ai) != 0 {
                    continue;
                }
                if a.second_draw && ev == Ev::Draw && self.players[p].drawn_this_turn != 2 {
                    continue;
                }
                // "Whenever an opponent casts their second spell each turn".
                if a.second_draw && ev == Ev::Cast && self.players[p].spells_this_turn != 2 {
                    continue;
                }
                let mine = src.ctrl as usize == p;
                if let Some(y) = f.yours {
                    if y != mine {
                        continue;
                    }
                }
                // "Whenever a source you control deals noncombat damage to an opponent".
                if ev == Ev::NoncombatDamage
                    && trigger_part(&a.text).contains("you control")
                    && self.actor != src.ctrl
                {
                    continue;
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
                        && self.actor == src.ctrl
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
                    x: None,
                });
            }
        }
        for (si, ai) in fired {
            self.perms[si].fired |= 1 << ai;
        }
        // Emblems ("whenever you cast a spell, draw a card").
        for (o, pl) in self.players.iter().enumerate() {
            if !pl.alive {
                continue;
            }
            for &(r, k) in &pl.emblems {
                let Some((_, a)) = self.card(r).emblems.get(k as usize) else {
                    continue;
                };
                let Trig::Event(kind, f) = &a.trig else {
                    continue;
                };
                if *kind != ev || f.yours.is_some_and(|y| y != (o == p)) {
                    continue;
                }
                add.push(Pending {
                    ctrl: o as u8,
                    source: None,
                    card: r,
                    back: false,
                    ability: EMBLEM_BASE + k,
                    snapshot: None,
                    subject: subject.map(|s| s.id),
                    teamwork: false,
                    gift: false,
                    from_graveyard: false,
                    kicked: false,
                    overloaded: false,
                    x: None,
                });
            }
        }
        // Cards that work from the graveyard ("whenever a land you control enters, you may
        // pay {1}{G}{U}. If you do, return ~ from your graveyard to your hand").
        for (o, pl) in self.players.iter().enumerate() {
            if !pl.alive {
                continue;
            }
            for &c in &pl.graveyard {
                let card = &pl.cards[c as usize];
                if (card.ev_mask | card.gy_ev_mask) & bit == 0 {
                    continue;
                }
                for (k, a) in card.abilities.iter().enumerate() {
                    let Trig::Event(kind, f) = &a.trig else {
                        continue;
                    };
                    if *kind != ev || !a.graveyard_only() || f.yours.is_some_and(|y| y != (o == p))
                    {
                        continue;
                    }
                    if let Some(s) = subject {
                        if f.types != 0 && s.types & f.types == 0
                            || f.subtype.as_ref().is_some_and(|t| !s.subtypes.contains(t))
                            || f.nontoken && s.is_token()
                        {
                            continue;
                        }
                    }
                    add.push(Pending {
                        ctrl: o as u8,
                        source: None,
                        card: (o as u8, c),
                        back: false,
                        ability: k as u16,
                        snapshot: None,
                        subject: subject.map(|s| s.id),
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
        self.pending.extend(add);
    }

    /// Fires the spell-cast triggers: "whenever you cast", prowess, extort.
    pub fn on_cast(&mut self, p: usize, card: &SimCard) {
        self.players[p].spells_this_turn += 1;
        if card.is(T_CREATURE) {
            self.players[p].creature_spells_turn += 1;
        } else {
            self.players[p].noncreature_spells_turn += 1;
        }
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
            finality: false,
            no_abilities: false,
            sac_at_end: false,
            modes_used: 0,
            fired: 0,
            mv: 0,
            attacking: false,
            blocking: false,
            dealt_damage: false,
            attacked_turn: false,
            entered_turn: false,
            hit_turn: 0,
            ever_hit: false,
            hurt_by: 0,
            p: 0,
            t: 0,
            eff_kw: 0,
            ward: 0,
            protection: 0,
            hexproof_from: 0,
            transformed: false,
            blank: false,
            animated: false,
            temp_creature: false,
            temp_base: None,
            returns_on_death: 0,
            named: None,
            till: None,
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
                    && !self.cant_lose(p)
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

    /// Lands `p` may play this turn: one, plus "you may play an additional land on each of
    /// your turns" permanents and one-off extra drops.
    pub fn land_drops(&self, p: usize) -> u8 {
        let statics = self
            .perms
            .iter()
            .enumerate()
            .filter(|(_, x)| x.ctrl as usize == p)
            .filter(|(i, x)| {
                self.abilities_of(x).iter().any(|a| {
                    a.trig == Trig::Static
                        && a.text
                            .contains("play an additional land on each of your turns")
                        && self.cond_holds(&a.cond, p, Some(*i))
                })
            })
            .count() as u8;
        1 + statics + self.players[p].extra_lands
    }

    /// "You can't lose the game" (Herald of Eternal Dawn).
    pub fn cant_lose(&self, p: usize) -> bool {
        self.perms
            .iter()
            .any(|x| x.ctrl as usize == p && self.perm_rules(x) & R_CANT_LOSE != 0)
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
