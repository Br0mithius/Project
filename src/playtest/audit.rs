//! Audit instrumentation: readable game logs (`Game::log`) and per-game statistics
//! (`Game::stats`), both off by default so normal playtests pay nothing for them.
//!
//! Nothing here may change the game: no random numbers, no AI caches filled (values are
//! read with the `ai::*_peek` functions), no state the rules read. The fields the game keeps
//! for this (`Game::cause`, `Game::leave_why`, `Game::effect_kind`, `Game::dmg_flag`) are only
//! ever read by this module.

use super::ai;
use super::compile::*;
use super::state::*;

/// What is acting right now (for "who did it" in logs and statistics).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum Cause {
    #[default]
    None,
    /// A spell or ability of this card resolving, controlled by this player.
    Card(CardRef, u8),
    /// An ability of a token without a card (Food, Clue...), controlled by this player.
    Token(u8),
    /// Combat damage (and lifelink) from this creature, controlled by this player.
    Combat(u32, u8),
    /// A cost this player pays (life for an ability).
    Cost(u8),
    /// A game rule (the hand size limit).
    Rule(&'static str),
}

/// Why the next permanent to leave the battlefield leaves (set just before `leave`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum LeaveWhy {
    #[default]
    Unknown,
    Destroyed,
    Sacrificed,
    /// State-based actions (lethal damage, 0 toughness, no loyalty, an Aura with no host).
    Sba,
    LegendRule,
    /// An Aura whose host left.
    AuraFell,
    /// A Treasure or Lotus sacrificed for mana (not logged).
    ManaCost,
    /// Its owner left the game (not logged).
    PlayerLeft,
}

/// How a player lost life last.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LossKind {
    Combat,
    Damage,
    LifeLoss,
}

/// How a player left the game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EndCause {
    CombatDamage,
    CommanderDamage,
    Poison,
    NoncombatDamage,
    LifeLoss,
    Decking,
    /// "You lose the game" or anything else.
    Other,
}

impl EndCause {
    pub fn label(self) -> &'static str {
        match self {
            EndCause::CombatDamage => "combat damage",
            EndCause::CommanderDamage => "commander damage",
            EndCause::Poison => "poison",
            EndCause::NoncombatDamage => "noncombat damage",
            EndCause::LifeLoss => "life loss (drain/payment)",
            EndCause::Decking => "drew from an empty library",
            EndCause::Other => "other (lose-the-game effect)",
        }
    }
}

/// One of a player's own turns.
#[derive(Debug, Clone, Default)]
pub struct TurnStat {
    pub round: u32,
    /// A land was played.
    pub land: bool,
    /// No land was played though the hand held one at the end of the turn.
    pub land_in_hand: bool,
    /// Mana available after the land drop, at the start of the first main phase.
    pub mana_avail: Option<u32>,
    /// Mana paid during this turn.
    pub mana_spent: u32,
    pub spells: u32,
    pub attackers: u32,
}

/// A harmful targeted effect (removal and the like) and what it was pointed at.
#[derive(Debug, Clone, Default)]
pub struct Removal {
    pub kind: &'static str,
    /// Whose permanent was targeted.
    pub target_seat: u8,
    /// The target is the caster's own.
    pub own: bool,
    pub token: bool,
    pub creature: bool,
    /// The target was its controller's most valuable creature (or nonland permanent).
    pub best_of_ctrl: bool,
    /// Its controller had the most life / the biggest threat among the caster's opponents.
    pub ctrl_life_leader: bool,
    pub ctrl_threat_leader: bool,
    pub value: f32,
}

/// A counterspell cast.
#[derive(Debug, Clone, Default)]
pub struct CounterUse {
    pub caster: u8,
    pub mv: u8,
    pub permanent: bool,
    pub caster_threat_leader: bool,
    /// The caster paid "unless its controller pays" and the spell resolved.
    pub paid_off: bool,
}

/// A player leaving the game.
#[derive(Debug, Clone)]
pub struct Elim {
    pub seat: usize,
    pub round: u32,
    pub by: Option<usize>,
    pub cause: EndCause,
    /// Rank (0 = biggest) of the player's threat at the start of the turn among the players
    /// still in, and how many were.
    pub threat_rank: usize,
    pub alive_before: usize,
    pub hand: usize,
}

#[derive(Debug, Clone, Default)]
pub struct PlayerStats {
    pub commander: String,
    pub mulligans: u8,
    pub kept: u8,
    pub turns: Vec<TurnStat>,
    /// Own turn number (1-based) the commander was first cast in.
    pub cmd_first_turn: Option<u32>,
    pub cmd_casts: u32,
    /// Own cards cast (indices into the deck's card list).
    pub cast: Vec<u16>,
    pub spells_cast: u32,
    /// Creatures attacking (each counted per combat) and at whom.
    pub attackers: u32,
    pub at_life_leader: u32,
    pub at_threat_leader: u32,
    pub at_weakest: u32,
    pub at_walkers: u32,
    /// Attacking creatures that got blocked.
    pub attackers_blocked: u32,
    /// Blocks made (blockers), double blocks, and chump blocks (blocker dies, attacker lives).
    pub blocks: u32,
    pub chumps: u32,
    pub creatures_lost: u32,
    pub creatures_died: u32,
    pub tokens_lost: u32,
    /// Damage dealt to each player (combat) and noncombat damage or life loss caused.
    pub combat_to: [i32; 8],
    pub noncombat_to: [i32; 8],
    pub removal: Vec<Removal>,
    pub counters: Vec<CounterUse>,
    pub last_loss: Option<(Option<usize>, LossKind)>,
    pub last_poison_by: Option<usize>,
    pub decked: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub players: Vec<PlayerStats>,
    pub elims: Vec<Elim>,
    pub winner: Option<usize>,
    pub rounds: u32,
    /// The game hit `MAX_ROUNDS` and was called on standing.
    pub timeout: bool,
    /// The last players left the game together: a draw (`winner` is None).
    pub simultaneous: bool,
    /// Each player's threat at the start of the latest turn.
    pub threat: Vec<f32>,
    /// Players already recorded as out (bit per seat).
    pub out: u8,
}

impl Stats {
    pub fn new(n: usize) -> Stats {
        Stats {
            players: vec![PlayerStats::default(); n],
            threat: vec![0.0; n],
            ..Default::default()
        }
    }
}

/// `{2}{B}{B}` style.
pub fn fmt_mana(m: &Mana) -> String {
    let mut s = String::new();
    if m.x {
        s += "{X}";
    }
    if m.generic > 0 || m.total() == 0 {
        s += &format!("{{{}}}", m.generic);
    }
    for _ in 0..m.colorless {
        s += "{C}";
    }
    for (i, c) in ['W', 'U', 'B', 'R', 'G'].iter().enumerate() {
        for _ in 0..m.pips[i] {
            s += &format!("{{{}}}", c);
        }
    }
    for &h in m.hybrid.iter().filter(|&&h| h != 0) {
        let cs: String = ['W', 'U', 'B', 'R', 'G']
            .iter()
            .enumerate()
            .filter(|(i, _)| h & (1 << i) != 0)
            .map(|(_, c)| c.to_string())
            .collect::<Vec<_>>()
            .join("/");
        s += &format!("{{{}}}", cs);
    }
    s
}

/// A short name for an effect (removal statistics).
pub fn effect_kind(e: &crate::abilities::Effect) -> &'static str {
    use crate::abilities::Effect as E;
    match e {
        E::Destroy(_) => "destroy",
        E::Exile { .. } => "exile",
        E::Damage { .. } => "damage",
        E::Fight(_) => "fight",
        E::Shrink { .. } => "shrink",
        E::Bounce(_) => "bounce",
        E::Lock { .. } => "lock/tap",
        E::GainControl(_) => "steal",
        E::Goad => "goad",
        E::Edict { .. } => "edict",
        _ => "other",
    }
}

/// Effect kinds that count as removal.
pub fn is_removal(kind: &str) -> bool {
    matches!(
        kind,
        "destroy" | "exile" | "damage" | "fight" | "shrink" | "bounce" | "lock/tap" | "steal"
    )
}

impl<'a> Game<'a> {
    /// Logging or statistics are on.
    pub fn auditing(&self) -> bool {
        self.log.is_some() || self.stats.is_some()
    }

    /// The player's current own turn, if it's their turn and statistics are on.
    pub fn turn_stat(&mut self, p: usize) -> Option<&mut TurnStat> {
        if self.active != p {
            return None;
        }
        self.stats.as_mut()?.players.get_mut(p)?.turns.last_mut()
    }

    /// A permanent's name ("Goblin token", "Treasure").
    pub fn perm_name(&self, x: &Perm) -> String {
        match (x.tok, x.card) {
            (Tok::None, Some(r)) => {
                let c = self.card(r);
                match (&c.back, x.transformed) {
                    (Some(b), true) => b.name.clone(),
                    _ => c.name.clone(),
                }
            }
            (Tok::Creature | Tok::Other, Some(r)) => format!("{} token", self.card(r).name),
            (Tok::Creature | Tok::Army | Tok::Germ, None) => {
                let s = x.subtypes.first().map(|s| s.as_str()).unwrap_or("creature");
                let mut s = s.to_string();
                if let Some(f) = s.get_mut(0..1) {
                    f.make_ascii_uppercase();
                }
                format!("{} token", s)
            }
            (Tok::Equip(..), _) => "Equipment token".into(),
            (t, _) => format!("{:?}", t),
        }
    }

    /// "Goblin token 2/2 (P1)", "Sol Ring (P0)", "Jace [3] (P2)".
    pub fn perm_desc(&self, x: &Perm) -> String {
        let mut s = self.perm_name(x);
        if x.is_creature() {
            s += &format!(" {}/{}", x.p, x.t);
        } else if x.is(T_PLANESWALKER) {
            s += &format!(" [{}]", x.loyalty);
        }
        s + &format!(" (P{})", x.ctrl)
    }

    /// The permanent with this id, described, or "#id" if it's gone.
    pub fn id_desc(&self, id: u32) -> String {
        match self.idx(id) {
            Some(i) => self.perm_desc(&self.perms[i]),
            None => format!("#{}", id),
        }
    }

    /// What is acting now, described ("Murder (P0)", "combat: Thanos 7/7 (P0)").
    pub fn cause_desc(&self) -> String {
        match self.cause {
            Cause::None => "?".into(),
            Cause::Card(r, c) => {
                if r.1 == NO_CARD {
                    format!("token ability (P{})", c)
                } else {
                    format!("{} (P{})", self.card(r).name, c)
                }
            }
            Cause::Token(c) => format!("token ability (P{})", c),
            Cause::Combat(id, _) => format!("combat: {}", self.id_desc(id)),
            Cause::Cost(c) => format!("cost paid by P{}", c),
            Cause::Rule(r) => r.into(),
        }
    }

    /// " [what is acting]", or nothing outside spells, abilities and combat.
    pub fn cause_tag(&self) -> String {
        match self.cause {
            Cause::None => String::new(),
            _ => format!(" [{}]", self.cause_desc()),
        }
    }

    /// The player behind what's acting now.
    pub fn cause_player(&self) -> Option<usize> {
        match self.cause {
            Cause::None | Cause::Rule(_) => None,
            Cause::Card(_, c) | Cause::Token(c) | Cause::Combat(_, c) | Cause::Cost(c) => {
                Some(c as usize)
            }
        }
    }

    /// Compact board of a player: "Thanos 7/7*, Goblin token 1/1 x2 | Sol Ring, Jace [3]".
    fn board_summary(&self, p: usize) -> String {
        let mut creatures: Vec<(String, u32)> = Vec::new();
        let mut others: Vec<(String, u32)> = Vec::new();
        for x in self.perms.iter().filter(|x| x.ctrl as usize == p) {
            if x.is(T_LAND) && !x.is_creature() {
                continue;
            }
            let mut d = self.perm_name(x);
            if x.is_creature() {
                d += &format!(" {}/{}", x.p, x.t);
            } else if x.is(T_PLANESWALKER) {
                d += &format!(" [{}]", x.loyalty);
            }
            if x.commander > 0 {
                d += "*";
            }
            if x.tapped {
                d += "(T)";
            }
            let list = if x.is_creature() {
                &mut creatures
            } else {
                &mut others
            };
            match list.iter_mut().find(|e| e.0 == d) {
                Some(e) => e.1 += 1,
                None => list.push((d, 1)),
            }
        }
        let show = |v: &Vec<(String, u32)>| {
            v.iter()
                .map(|(d, n)| {
                    if *n > 1 {
                        format!("{} x{}", d, n)
                    } else {
                        d.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        format!("{} | {}", show(&creatures), show(&others))
    }

    /// One line about a player's resources.
    fn player_line(&self, p: usize) -> String {
        let pl = &self.players[p];
        let lands = self
            .perms
            .iter()
            .filter(|x| x.ctrl as usize == p && x.is(T_LAND))
            .count();
        let cmd: i32 = pl.cmd_damage.iter().flat_map(|d| d.iter()).sum();
        let mut s = format!(
            "P{} life {} hand {} lib {} lands {} mana {}",
            p,
            pl.life,
            pl.hand.len(),
            pl.library.len(),
            lands,
            self.mana_available(p)
        );
        if pl.poison > 0 {
            s += &format!(" poison {}", pl.poison);
        }
        if cmd > 0 {
            let parts: Vec<String> = (0..self.players.len())
                .filter_map(|q| {
                    let d: i32 = pl.cmd_damage[q].iter().sum();
                    (d > 0).then(|| format!("P{}:{}", q, d))
                })
                .collect();
            s += &format!(" cmd-dmg {}", parts.join(","));
        }
        s
    }

    /// Start of player `a`'s turn (after the untap step): log header and statistics.
    pub fn audit_turn_start(&mut self, a: usize) {
        if !self.auditing() {
            return;
        }
        let threats: Vec<f32> = (0..self.players.len())
            .map(|o| ai::threat_peek(self, o))
            .collect();
        let round = self.round;
        if let Some(s) = self.stats.as_mut() {
            s.threat = threats.clone();
            if let Some(ps) = s.players.get_mut(a) {
                ps.turns.push(TurnStat {
                    round,
                    ..Default::default()
                });
            }
        }
        if self.log.is_none() {
            return;
        }
        let pl = &self.players[a];
        let hand: Vec<String> = pl
            .hand
            .iter()
            .map(|&c| {
                let c = &pl.cards[c as usize];
                format!("{} {}", c.name, c.mv)
            })
            .collect();
        let cmd: Vec<String> = pl
            .command
            .iter()
            .map(|&(c, slot)| {
                format!(
                    "{} (tax {})",
                    pl.cards[c as usize].name,
                    2 * pl.casts[slot as usize % 3]
                )
            })
            .collect();
        let mut lines = vec![format!(
            "======== P{}'s turn {} ========",
            a,
            self.stats.as_ref().map_or(0, |s| s.players[a].turns.len())
        )];
        lines.push(format!(
            "  {} threat {:.1} | hand before the draw: {}{}",
            self.player_line(a),
            threats[a],
            hand.join("; "),
            if cmd.is_empty() {
                String::new()
            } else {
                format!(" | command zone: {}", cmd.join(", "))
            }
        ));
        lines.push(format!("  board P{}: {}", a, self.board_summary(a)));
        let n = self.players.len();
        for k in 1..n {
            let o = (a + k) % n;
            if !self.players[o].alive {
                continue;
            }
            lines.push(format!(
                "  {} threat {:.1} | board: {}",
                self.player_line(o),
                threats[o],
                self.board_summary(o)
            ));
        }
        for l in lines {
            self.note(|| l);
        }
    }

    /// End of player `a`'s turn (cleanup).
    pub fn audit_turn_end(&mut self, a: usize) {
        if self.stats.is_none() {
            return;
        }
        let pl = &self.players[a];
        let land_in_hand = pl.hand.iter().any(|&c| pl.cards[c as usize].is(T_LAND));
        if let Some(t) = self.turn_stat(a) {
            if !t.land && land_in_hand {
                t.land_in_hand = true;
            }
        }
    }

    /// Records the mana `p` has after the land drop (once per turn).
    pub fn audit_mana_avail(&mut self, p: usize) {
        if self.stats.is_none() || self.active != p {
            return;
        }
        if self
            .stats
            .as_ref()
            .and_then(|s| s.players[p].turns.last())
            .is_some_and(|t| t.mana_avail.is_some())
        {
            return;
        }
        let m = self.mana_available(p);
        if let Some(t) = self.turn_stat(p) {
            t.mana_avail = Some(m);
        }
    }

    /// A land was played (permanent index `i`, or None if it didn't stay).
    pub fn audit_land(&mut self, p: usize, card: u16, i: Option<usize>, from: &str) {
        if !self.auditing() {
            return;
        }
        if let Some(t) = self.turn_stat(p) {
            t.land = true;
        }
        if self.log.is_some() {
            let name = self.players[p].cards[card as usize].name.clone();
            let tapped = i.is_some_and(|i| self.perms[i].tapped);
            let lands = self
                .perms
                .iter()
                .filter(|x| x.ctrl as usize == p && x.is(T_LAND))
                .count();
            let mana = self.mana_available(p);
            self.note(|| {
                format!(
                    "P{} plays land {}{}{} -> {} lands, {} mana available",
                    p,
                    name,
                    from,
                    if tapped { " (tapped)" } else { " (untapped)" },
                    lands,
                    mana
                )
            });
        }
    }

    /// A permanent entered (logged unless it was just cast or played as a land).
    pub fn audit_enter(&mut self, i: usize) {
        if self.log.is_none() || self.quiet_enter || self.ext.entering_cast {
            return;
        }
        let x = &self.perms[i];
        // (Its static effects aren't applied yet: size from its base and counters.)
        let d = if x.is_creature() {
            let (bp, bt) = x.temp_base.unwrap_or((x.base_p, x.base_t));
            format!(
                "{} {}/{} (P{})",
                self.perm_name(x),
                bp + x.counters + x.temp_p,
                bt + x.counters + x.temp_t,
                x.ctrl
            )
        } else {
            self.perm_desc(x)
        };
        let (owner, ctrl) = (x.owner, x.ctrl);
        let tapped = x.tapped;
        let by = self.cause_desc();
        self.note(|| {
            format!(
                "  + {} enters{}{} [{}]",
                d,
                if tapped { " tapped" } else { "" },
                if owner != ctrl {
                    format!(" (owner P{})", owner)
                } else {
                    String::new()
                },
                by
            )
        });
    }

    /// A permanent left the battlefield.
    pub fn audit_leave(&mut self, perm: &Perm, to: Zone, why: LeaveWhy) {
        if matches!(why, LeaveWhy::ManaCost | LeaveWhy::PlayerLeft) {
            return;
        }
        let ctrl = perm.ctrl as usize;
        if let Some(s) = self.stats.as_mut() {
            if perm.is_creature() {
                let ps = &mut s.players[ctrl];
                ps.creatures_lost += 1;
                if matches!(to, Zone::Graveyard) {
                    ps.creatures_died += 1;
                }
                if perm.is_token() {
                    ps.tokens_lost += 1;
                }
            }
        }
        if self.log.is_none() {
            return;
        }
        let by = self.cause_desc();
        let what = match why {
            LeaveWhy::Destroyed => format!("destroyed by {}", by),
            LeaveWhy::Sacrificed => format!("sacrificed ({})", by),
            LeaveWhy::LegendRule => "legend rule".into(),
            LeaveWhy::AuraFell => "Aura fell off (its host left)".into(),
            LeaveWhy::ManaCost => "sacrificed for mana".into(),
            LeaveWhy::PlayerLeft => "its owner left the game".into(),
            LeaveWhy::Sba => {
                let killer = if perm.hurt_by != 0 {
                    format!(" from {}", self.id_desc(perm.hurt_by))
                } else {
                    String::new()
                };
                let combat = if self.in_combat { " in combat" } else { "" };
                if perm.is_creature() && perm.t <= 0 {
                    "0 toughness".into()
                } else if perm.is_creature() && perm.deathtouched && perm.damage > 0 {
                    format!("dies{} (deathtouch damage{})", combat, killer)
                } else if perm.is_creature() {
                    format!(
                        "dies{} (lethal damage {}/{}{})",
                        combat, perm.damage, perm.t, killer
                    )
                } else if perm.is(T_PLANESWALKER) {
                    "no loyalty left".into()
                } else if perm.is(T_AURA) {
                    "Aura with nothing to enchant".into()
                } else {
                    "state-based action".into()
                }
            }
            LeaveWhy::Unknown => match to {
                Zone::Exile => format!("exiled by {}", by),
                Zone::Hand => format!("returned to hand by {}", by),
                Zone::Library(top) => format!(
                    "put on the {} of the library by {}",
                    if top { "top" } else { "bottom" },
                    by
                ),
                Zone::Graveyard => format!("put into the graveyard by {}", by),
            },
        };
        let dest = if perm.is_token() {
            "ceases to exist"
        } else if perm.commander > 0 && !matches!(to, Zone::Hand) {
            "-> command zone"
        } else {
            match to {
                Zone::Graveyard => "-> graveyard",
                Zone::Exile => "-> exile",
                Zone::Hand => "-> hand",
                Zone::Library(_) => "-> library",
            }
        };
        let d = self.perm_desc(perm);
        self.note(|| format!("  - {} {}: {}", d, dest, what));
    }

    /// Life changed for `p` (`before` -> now).
    pub fn audit_life(&mut self, p: usize, before: i32, gain: bool) {
        let after = self.players[p].life;
        let n = (after - before).abs();
        let by = self.cause_player();
        let kind = if matches!(self.cause, Cause::Combat(..)) {
            LossKind::Combat
        } else if self.dmg_flag {
            LossKind::Damage
        } else {
            LossKind::LifeLoss
        };
        if !gain {
            if let Some(s) = self.stats.as_mut() {
                s.players[p].last_loss = Some((by, kind));
                if let Some(q) = by.filter(|&q| q != p) {
                    if kind != LossKind::Combat {
                        s.players[q].noncombat_to[p % 8] += n;
                    }
                }
            }
        }
        if self.log.is_some() {
            let what = match (gain, kind) {
                (true, _) => "gains",
                (false, LossKind::Combat) => "takes combat damage",
                (false, LossKind::Damage) => "takes damage",
                (false, LossKind::LifeLoss) => "loses life",
            };
            let src = self.cause_desc();
            self.note(|| {
                format!(
                    "  P{} {} {}: life {} -> {} [{}]",
                    p, what, n, before, after, src
                )
            });
        }
    }

    /// Records players who just left the game.
    pub fn audit_eliminations(&mut self) {
        let Some(s) = self.stats.as_ref() else {
            return;
        };
        let n = self.players.len();
        let newly: Vec<usize> = (0..n)
            .filter(|&p| !self.players[p].alive && s.out & (1 << p) == 0)
            .collect();
        if newly.is_empty() {
            return;
        }
        let alive_before = (0..n).filter(|&p| s.out & (1 << p) == 0).count();
        for p in newly {
            let s = self.stats.as_ref().unwrap();
            let pl = &self.players[p];
            let ps = &s.players[p];
            let cmd = (0..n).find(|&q| pl.cmd_damage[q].iter().any(|&d| d >= 21));
            let (cause, by) = if pl.poison >= 10 {
                (EndCause::Poison, ps.last_poison_by)
            } else if let Some(q) = cmd {
                (EndCause::CommanderDamage, Some(q))
            } else if pl.life <= 0 {
                match ps.last_loss {
                    Some((by, LossKind::Combat)) => (EndCause::CombatDamage, by),
                    Some((by, LossKind::Damage)) => (EndCause::NoncombatDamage, by),
                    Some((by, LossKind::LifeLoss)) => (EndCause::LifeLoss, by),
                    None => (EndCause::Other, None),
                }
            } else if ps.decked {
                (EndCause::Decking, None)
            } else {
                (EndCause::Other, self.cause_player())
            };
            let by = by.filter(|&q| q != p);
            let rank = (0..n)
                .filter(|&q| s.out & (1 << q) == 0 && q != p)
                .filter(|&q| s.threat[q] > s.threat[p])
                .count();
            let e = Elim {
                seat: p,
                round: self.round,
                by,
                cause,
                threat_rank: rank,
                alive_before,
                hand: pl.hand.len(),
            };
            let life = pl.life;
            let poison = pl.poison;
            let round = self.round;
            let active = self.active;
            if let Some(s) = self.stats.as_mut() {
                s.out |= 1 << p;
                s.elims.push(e.clone());
            }
            self.note(|| {
                format!(
                    "*** P{} is ELIMINATED in round {} (P{}'s turn): {}{} | life {} poison {} hand {} | threat rank {} of {} | their permanents leave the game",
                    p,
                    round,
                    active,
                    cause.label(),
                    by.map(|q| format!(" by P{}", q)).unwrap_or_default(),
                    life,
                    poison,
                    e.hand,
                    rank + 1,
                    alive_before
                )
            });
        }
    }

    /// Targets a resolving effect chose (`ids`) out of `ncands` candidates.
    pub fn audit_targets(&mut self, me: usize, ids: &[u32], harmful: bool, cands: &[usize]) {
        if !self.auditing() {
            return;
        }
        let kind = self.effect_kind;
        if self.log.is_some() {
            let src = self.cause_desc();
            if ids.is_empty() {
                self.note(|| {
                    format!(
                        "    {} {} finds no target ({} candidates)",
                        src,
                        kind,
                        cands.len()
                    )
                });
            } else {
                // The AI's scores (as `choose_victims` / `choose_friends` saw them).
                let score = |g: &Game, i: usize| -> f32 {
                    let o = g.perms[i].ctrl as usize;
                    let v = ai::perm_value_peek(g, i);
                    if !harmful {
                        v
                    } else if o == me {
                        -v
                    } else {
                        v * ai::weight_peek(g, me, o)
                    }
                };
                let chosen: Vec<String> = ids
                    .iter()
                    .filter_map(|&id| self.idx(id))
                    .map(|i| format!("{} v{:.1}", self.perm_desc(&self.perms[i]), score(self, i)))
                    .collect();
                let mut rest: Vec<(f32, usize)> = cands
                    .iter()
                    .copied()
                    .filter(|&i| i < self.perms.len() && !ids.contains(&self.perms[i].id))
                    .map(|i| (score(self, i), i))
                    .collect();
                rest.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
                let next: Vec<String> = rest
                    .iter()
                    .take(2)
                    .map(|&(v, i)| format!("{} v{:.1}", self.perm_desc(&self.perms[i]), v))
                    .collect();
                self.note(|| {
                    format!(
                        "    {} {} {}targets {}{}",
                        src,
                        kind,
                        if harmful { "" } else { "(friendly) " },
                        chosen.join(", "),
                        if next.is_empty() {
                            String::new()
                        } else {
                            format!(" | passed over: {}", next.join(", "))
                        }
                    )
                });
            }
        }
        if !harmful || self.stats.is_none() || !is_removal(kind) {
            return;
        }
        let opps = self.opponents(me);
        for &id in ids {
            let Some(i) = self.idx(id) else { continue };
            let x = &self.perms[i];
            let o = x.ctrl as usize;
            let creature = x.is_creature();
            let best = (0..self.perms.len())
                .filter(|&j| {
                    let y = &self.perms[j];
                    y.ctrl as usize == o
                        && if creature {
                            y.is_creature()
                        } else {
                            !y.is(T_LAND)
                        }
                })
                .max_by(|&a, &b| {
                    ai::perm_value_peek(self, a)
                        .partial_cmp(&ai::perm_value_peek(self, b))
                        .unwrap()
                });
            let life_leader = opps
                .iter()
                .all(|&q| self.players[q].life <= self.players[o].life);
            let t_o = ai::threat_peek(self, o);
            let threat_leader = opps.iter().all(|&q| ai::threat_peek(self, q) <= t_o);
            let r = Removal {
                kind,
                target_seat: o as u8,
                own: o == me,
                token: x.is_token(),
                creature,
                best_of_ctrl: best == Some(i),
                ctrl_life_leader: o != me && life_leader,
                ctrl_threat_leader: o != me && threat_leader,
                value: ai::perm_value_peek(self, i),
            };
            if let Some(s) = self.stats.as_mut() {
                s.players[me].removal.push(r);
            }
        }
    }

    /// Attacks declared by `a`.
    pub fn audit_attacks(&mut self, a: usize, attacks: &[super::combat::Attack]) {
        use super::combat::Def;
        if self.log.is_some() {
            let list: Vec<String> = attacks
                .iter()
                .map(|at| {
                    let to = match at.def {
                        Def::Player(p) => format!("P{}", p),
                        Def::Walker(id) => self.id_desc(id),
                    };
                    format!("{} -> {}", self.id_desc(at.attacker), to)
                })
                .collect();
            self.note(|| format!("P{} attacks with {}: {}", a, attacks.len(), list.join(", ")));
        }
        if self.stats.is_none() {
            return;
        }
        let opps = self.opponents(a);
        let max_life = opps.iter().map(|&o| self.players[o].life).max();
        let min_life = opps.iter().map(|&o| self.players[o].life).min();
        let threats: Vec<(usize, f32)> = opps
            .iter()
            .map(|&o| (o, ai::threat_peek(self, o)))
            .collect();
        let max_threat = threats.iter().map(|t| t.1).fold(f32::MIN, f32::max);
        let defs: Vec<(bool, Option<usize>)> = attacks
            .iter()
            .map(|at| (matches!(at.def, Def::Walker(_)), self.def_player(at.def)))
            .collect();
        let life: Vec<i32> = self.players.iter().map(|p| p.life).collect();
        let s = self.stats.as_mut().unwrap();
        let ps = &mut s.players[a];
        if let Some(t) = ps.turns.last_mut() {
            t.attackers += attacks.len() as u32;
        }
        for (walker, d) in defs {
            ps.attackers += 1;
            if walker {
                ps.at_walkers += 1;
            }
            let Some(d) = d else { continue };
            if Some(life[d]) == max_life {
                ps.at_life_leader += 1;
            }
            if Some(life[d]) == min_life {
                ps.at_weakest += 1;
            }
            if threats.iter().any(|&(o, t)| o == d && t >= max_threat) {
                ps.at_threat_leader += 1;
            }
        }
    }

    /// Blocks, after menace is checked.
    pub fn audit_blocks(&mut self, attacks: &[super::combat::Attack]) {
        let mut lines = Vec::new();
        for at in attacks {
            let Some(ai_) = self.idx(at.attacker) else {
                continue;
            };
            let a = self.perms[ai_].ctrl as usize;
            let d = self.def_player(at.def);
            let blockers: Vec<usize> = at.blockers.iter().filter_map(|&b| self.idx(b)).collect();
            let chump = blockers.len() == 1
                && ai::fight(&self.perms[ai_], &self.perms[blockers[0]]) == (false, true);
            if self.log.is_some() {
                if blockers.is_empty() {
                    lines.push(format!(
                        "  {} is unblocked{}",
                        self.id_desc(at.attacker),
                        d.map(|d| format!(" (P{} takes it)", d)).unwrap_or_default()
                    ));
                } else {
                    let names: Vec<String> = blockers
                        .iter()
                        .map(|&b| self.perm_desc(&self.perms[b]))
                        .collect();
                    lines.push(format!(
                        "  {} is blocked by {}{}",
                        self.id_desc(at.attacker),
                        names.join(" + "),
                        if chump { " (chump block)" } else { "" }
                    ));
                }
            }
            if !blockers.is_empty() {
                if let Some(s) = self.stats.as_mut() {
                    s.players[a].attackers_blocked += 1;
                    if let Some(d) = d {
                        s.players[d].blocks += blockers.len() as u32;
                        if chump {
                            s.players[d].chumps += 1;
                        }
                    }
                }
            }
        }
        for l in lines {
            self.note(|| l);
        }
    }

    /// A spell was cast (paid, and out of its zone).
    #[allow(clippy::too_many_arguments)]
    pub fn audit_cast(
        &mut self,
        p: usize,
        src: super::Src,
        (list, idx): (usize, u16),
        back: bool,
        cost: &Mana,
        x: Option<i32>,
        opts: super::CastOpts,
        commander_slot: u8,
    ) {
        use super::Src;
        let casts = self.players[p].casts[commander_slot as usize % 3] as u32;
        let own_turn = self.stats.as_ref().map_or(0, |s| s.players[p].turns.len()) as u32;
        let active = self.active == p;
        if let Some(s) = self.stats.as_mut() {
            let ps = &mut s.players[p];
            ps.spells_cast += 1;
            if list == p {
                ps.cast.push(idx);
            }
            if commander_slot > 0 {
                ps.cmd_casts += 1;
                if ps.cmd_first_turn.is_none() {
                    ps.cmd_first_turn = Some(own_turn.max(1));
                }
            }
            if active {
                if let Some(t) = ps.turns.last_mut() {
                    t.spells += 1;
                }
            }
        }
        if self.log.is_none() {
            return;
        }
        let c = self.card((list as u8, idx));
        let name = match (&c.back, back) {
            (Some(b), true) => format!("{} (back face: {})", c.name, b.name),
            _ => c.name.clone(),
        };
        let from = match src {
            Src::Hand(_) => String::new(),
            Src::Command(_) => format!(
                " from the command zone (commander cast #{}, tax +{})",
                casts,
                2 * casts.saturating_sub(1)
            ),
            Src::Graveyard(_) => " from the graveyard".into(),
            Src::Adventure(_) => " from adventure exile".into(),
            Src::Prepared(_) => " (prepared spell copy)".into(),
            Src::Exile(_) if list != p => format!(" from exile (P{}'s card)", list),
            Src::Exile(_) => " from exile".into(),
        };
        let mut how = Vec::new();
        for (on, label) in [
            (opts.kicker && c.kicker.is_some(), "kicked"),
            (opts.overload && c.overload.is_some(), "overloaded"),
            (opts.teamwork, "teamwork"),
            (opts.convoke, "convoke/improvise"),
            (opts.offspring && c.offspring.is_some(), "offspring"),
            (opts.sneak && c.sneak.is_some(), "sneak"),
        ] {
            if on {
                how.push(label);
            }
        }
        let paid = if opts.free {
            "free".to_string()
        } else {
            format!("{} = {}", fmt_mana(cost), cost.total())
        };
        let left = self.mana_available(p);
        let xs = x.map(|x| format!(" X={}", x)).unwrap_or_default();
        self.note(|| {
            format!(
                "P{} casts {} (MV {}){}{} for {}{}, {} mana left{}",
                p,
                name,
                c.mv,
                from,
                if how.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", how.join(", "))
                },
                paid,
                xs,
                left,
                if active { "" } else { " (instant speed)" }
            )
        });
    }

    /// The game is over.
    pub fn audit_game_end(&mut self, winner: usize, timeout: bool, simultaneous: bool) {
        if !self.auditing() {
            return;
        }
        self.audit_eliminations();
        let round = self.round;
        if let Some(s) = self.stats.as_mut() {
            s.winner = (winner != super::NO_WINNER).then_some(winner);
            s.rounds = round;
            s.timeout = timeout;
            s.simultaneous = simultaneous;
        }
        if self.log.is_some() {
            let lines: Vec<String> = (0..self.players.len())
                .filter(|&p| self.players[p].alive)
                .map(|p| {
                    format!(
                        "  {} | board: {}",
                        self.player_line(p),
                        self.board_summary(p)
                    )
                })
                .collect();
            self.note(|| {
                if simultaneous {
                    format!(
                        "GAME OVER after round {}: a draw (the last players left together)",
                        round
                    )
                } else {
                    format!(
                        "GAME OVER after round {}: P{} wins{}",
                        round,
                        winner,
                        if timeout {
                            " (round limit: called on life and board)"
                        } else {
                            ""
                        }
                    )
                }
            });
            for l in lines {
                self.note(|| l);
            }
        }
    }
}
