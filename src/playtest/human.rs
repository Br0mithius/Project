//! Playing one seat by hand against the AI (the Play tab), with the same rules engine.
//!
//! The game runs on its own thread (`start`). Whenever the human seat has a decision to
//! make, the engine sends an `Ask` with a snapshot of the game (`GameView`) and waits for an
//! `Answer`. Every other seat is played by the AI as in simulated games, and with no human
//! seat (`Game::human` = None) nothing here runs, so simulations are unchanged.
//!
//! Mana is tapped by hand: the human taps lands and rocks into a mana pool (`Answer::Tap`,
//! `Answer::Filter`), which empties at the end of each step and phase; spells and abilities
//! are paid from the pool, and when it falls short the engine asks for more (`Ask::Pay`), or
//! pays the rest itself on request ("Auto-tap").
//!
//! Every answer is recorded (`HumanLink::transcript`), so a game can be replayed exactly from
//! its seed, decks and answers (`Report`, `replay`) - the way bug reports are reproduced.

use super::ai;
use super::combat::{Attack, Def};
use super::compile::*;
use super::resolve::Ctx;
use super::state::*;
use super::{CastOpts, Game, Prepared, Src};
use crate::abilities::{Kind, Target};
use crate::analysis::Rng;
use crate::db::{CardDb, Rules};
use crate::deck::Deck;
use crate::restrict::{Dyn, Restr};
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender};

/// Indices from here up in a payment plan's sources are mana in the human's pool, not
/// permanents (`Game::pool_units`).
pub const POOL_SRC: usize = usize::MAX / 2;
/// The pool's slots: W, U, B, R, G, then colourless.
pub const POOL_NAMES: [&str; 6] = ["W", "U", "B", "R", "G", "C"];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlayOptions {
    /// Stop (a "Continue" prompt) whenever a bot casts a spell.
    pub stop_on_bot_spells: bool,
    /// Stop when a bot attacks and at the end of each bot's turn.
    pub step_bot_turns: bool,
    /// Show the bots' hands (for checking what the AI does).
    pub show_bot_hands: bool,
    /// Ask at every instant-speed window, even with nothing to cast.
    pub stop_every_window: bool,
    /// Pay costs automatically from your lands (no manual tapping).
    pub auto_mana: bool,
}

/// One thing the human may do now (main phase or an instant-speed window).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ActOption {
    pub label: String,
    /// The mana it costs, as symbols ("{2}{U}"); empty when it costs none.
    pub cost: String,
    /// The pool already pays for it.
    pub payable: bool,
    /// What the pool still lacks ("{1}{G}"), when it doesn't.
    pub missing: String,
    /// The engine can pay for it from untapped sources ("Auto-tap & do it").
    pub auto_ok: bool,
    /// The card it's about (for showing it), if any.
    pub card: Option<String>,
}

/// A question for the human.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Ask {
    /// Keep this hand? (`Answer::Yes(true)` keeps.)
    Mulligan {
        hand: Vec<String>,
        mulligans: u8,
        suggest_keep: bool,
    },
    /// Main phase (`window` false) or an instant-speed window: do one of these, or pass.
    Act {
        phase: String,
        actions: Vec<ActOption>,
        window: bool,
        suggested: Option<usize>,
    },
    /// Choose between `min` and `max` of these.
    Pick {
        prompt: String,
        options: Vec<String>,
        min: usize,
        max: usize,
        suggested: Vec<usize>,
    },
    YesNo {
        prompt: String,
        suggested: bool,
    },
    Number {
        prompt: String,
        min: i32,
        max: i32,
        suggested: i32,
    },
    /// Declare attackers: each creature (id, description) may attack one defender.
    Attack {
        attackers: Vec<(u32, String)>,
        defenders: Vec<String>,
        suggested: Vec<(u32, usize)>,
    },
    /// Declare blockers: each of our creatures (id, description, attackers it can block).
    Block {
        attackers: Vec<(u32, String)>,
        blockers: Vec<(u32, String, Vec<u32>)>,
        suggested: Vec<(u32, u32)>,
    },
    /// A cost the pool can't pay yet: tap more mana, auto-tap, or cancel.
    Pay {
        prompt: String,
        cost: String,
        missing: String,
        auto_ok: bool,
    },
    /// Just a stop to look at the game.
    Continue {
        prompt: String,
    },
}

/// A sandbox correction to the game state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Edit {
    /// Move a card (or a permanent) to another zone.
    Move {
        from: Loc,
        to: Dest,
    },
    /// Tap or untap a permanent.
    Tap {
        id: u32,
    },
    SetLife {
        player: usize,
        life: i32,
    },
    /// Add (or remove, negative) +1/+1 counters and other counters.
    Counters {
        id: u32,
        plus: i16,
        other: i16,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum Loc {
    Perm(u32),
    Hand(usize, usize),
    Graveyard(usize, usize),
    Exile(usize, usize),
    LibraryTop(usize),
    Command(usize, usize),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum Dest {
    Battlefield,
    Hand,
    Graveyard,
    Exile,
    LibraryTop,
    LibraryBottom,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Answer {
    /// Let the AI decide this one (for a main phase: the AI plays the rest of it).
    Auto,
    Pick(Vec<usize>),
    /// Do action `n` of an `Ask::Act`, paying automatically.
    AutoTap(usize),
    Pass,
    Yes(bool),
    Num(i32),
    Attacks(Vec<(u32, usize)>),
    Blocks(Vec<(u32, u32)>),
    Continue,
    Cancel,
    /// Tap a mana source for mana of one colour (0-4 WUBRG, 5 colourless).
    Tap {
        id: u32,
        color: u8,
    },
    /// Use a filter ("{1}, {T}: Add one mana of any color"): its cost from the pool, these
    /// colours made.
    Filter {
        id: u32,
        colors: Vec<u8>,
    },
    Edit(Edit),
    Concede,
}

/// What the GUI sends the engine.
#[derive(Debug, Clone)]
pub enum ToEngine {
    Answer(Answer),
    /// Send back a full state dump (`FromEngine::Dump`).
    Dump,
}

/// What the engine sends the GUI.
#[derive(Debug, Clone)]
pub enum FromEngine {
    Ask {
        ask: Ask,
        view: Box<GameView>,
        /// Sandbox edits and manual tapping can be applied now.
        edits_ok: bool,
    },
    Dump(String),
    Over {
        winner: usize,
        view: Box<GameView>,
        transcript: Vec<Answer>,
    },
    Failed(String),
}

#[derive(Debug, Clone, Default)]
pub struct PermView {
    pub id: u32,
    pub name: String,
    pub owner: usize,
    pub ctrl: usize,
    pub p: i16,
    pub t: i16,
    pub creature: bool,
    pub land: bool,
    pub planeswalker: bool,
    pub tapped: bool,
    pub sick: bool,
    pub counters: i16,
    pub other_counters: i16,
    pub loyalty: i16,
    pub damage: i16,
    pub attached: Option<u32>,
    pub attacking: bool,
    pub blocking: bool,
    pub token: bool,
    pub commander: bool,
    pub keywords: String,
}

#[derive(Debug, Clone, Default)]
pub struct PlayerView {
    pub name: String,
    pub life: i32,
    pub poison: u8,
    pub alive: bool,
    /// Card names (only for the human, or every player with `show_bot_hands`).
    pub hand: Vec<String>,
    pub hand_count: usize,
    pub library: usize,
    pub graveyard: Vec<String>,
    pub exile: Vec<String>,
    /// Command zone: (name, commander tax).
    pub command: Vec<(String, u32)>,
    /// Mana floating in the engine's pool (from rituals and the like; any colour).
    pub floating: u8,
    pub commander_damage: Vec<String>,
}

/// A mana source the human can tap.
#[derive(Debug, Clone, Default)]
pub struct SourceView {
    pub id: u32,
    pub name: String,
    /// Colour bits (WUBRG = 0-4, colourless = 5).
    pub colors: u8,
    pub amount: u8,
    /// A filter ability: (mana it costs, mana of any colour it makes).
    pub filter: Option<(u32, u8)>,
}

#[derive(Debug, Clone, Default)]
pub struct GameView {
    pub round: u32,
    pub active: usize,
    pub seat: usize,
    pub phase: String,
    pub players: Vec<PlayerView>,
    pub perms: Vec<PermView>,
    /// The human's mana pool: W, U, B, R, G, C.
    pub pool: [u8; 6],
    pub sources: Vec<SourceView>,
    /// Log lines since the previous view.
    pub new_log: Vec<String>,
}

/// How the engine talks to whoever plays the human seat.
pub trait HumanIo {
    fn send(&self, msg: FromEngine);
    /// The next message; never returns if the player is gone (unwinds with `Quit`).
    fn recv(&self) -> ToEngine;
}

/// Unwinds the engine thread when the GUI closed the game (caught in `start`).
pub struct Quit;

/// The GUI's channels.
pub struct ChannelIo {
    pub tx: Sender<FromEngine>,
    pub rx: Receiver<ToEngine>,
}

impl HumanIo for ChannelIo {
    fn send(&self, msg: FromEngine) {
        let _ = self.tx.send(msg);
    }
    fn recv(&self) -> ToEngine {
        match self.rx.recv() {
            Ok(m) => m,
            Err(_) => std::panic::resume_unwind(Box::new(Quit)),
        }
    }
}

/// Scripted answers (tests and replays). When they run out: `Answer::Auto` every time, or
/// (`stop_when_done`) the game stops there with the asks so far recorded.
#[derive(Default)]
pub struct ScriptIo {
    pub answers: RefCell<VecDeque<Answer>>,
    pub asks: RefCell<Vec<Ask>>,
    pub stop_when_done: bool,
    /// The last view sent (what the game looked like at the last question).
    pub last_view: RefCell<Option<GameView>>,
    pub log: RefCell<Vec<String>>,
}

impl ScriptIo {
    pub fn new(answers: Vec<Answer>) -> ScriptIo {
        ScriptIo {
            answers: RefCell::new(answers.into()),
            ..Default::default()
        }
    }
}

impl HumanIo for ScriptIo {
    fn send(&self, msg: FromEngine) {
        if let FromEngine::Ask { ask, view, .. } = msg {
            self.log.borrow_mut().extend(view.new_log.iter().cloned());
            self.asks.borrow_mut().push(ask);
            *self.last_view.borrow_mut() = Some(*view);
        }
    }
    fn recv(&self) -> ToEngine {
        match self.answers.borrow_mut().pop_front() {
            Some(a) => ToEngine::Answer(a),
            None if self.stop_when_done => std::panic::resume_unwind(Box::new(Quit)),
            None => ToEngine::Answer(Answer::Auto),
        }
    }
}

/// The human seat of a game.
pub struct HumanLink {
    pub seat: usize,
    pub opts: PlayOptions,
    pub io: Box<dyn HumanIo>,
    /// Manual mana pool: W, U, B, R, G, C.
    pool: Cell<[u8; 6]>,
    log_sent: Cell<usize>,
    /// Pay the current action's costs automatically ("Auto-tap & cast").
    auto_pay: Cell<bool>,
    phase: RefCell<String>,
    /// Every answer the engine used, in order (replays the game with the same seed).
    pub transcript: RefCell<Vec<Answer>>,
}

impl HumanLink {
    pub fn new(seat: usize, opts: PlayOptions, io: Box<dyn HumanIo>) -> HumanLink {
        HumanLink {
            seat,
            opts,
            io,
            pool: Cell::new([0; 6]),
            log_sent: Cell::new(0),
            auto_pay: Cell::new(false),
            phase: RefCell::new("Start of game".into()),
            transcript: RefCell::new(Vec::new()),
        }
    }
}

/// What the human chose to do.
#[derive(Debug, Clone, Copy)]
enum HAct {
    Land(usize),
    LandFrom((bool, usize)),
    Cast(Src, CastOpts),
    Activate(u32, usize),
    ActivateGy(usize, usize),
    ActivateHand(usize, usize),
    Equip(u32, u32),
    Cycle(usize),
    Crew(u32),
}

/// "{2}{U}{U}" for a cost.
pub fn mana_str(m: &Mana) -> String {
    let mut s = String::new();
    if m.x {
        s += "{X}";
    }
    if m.generic > 0 {
        s += &format!("{{{}}}", m.generic);
    }
    for _ in 0..m.colorless {
        s += "{C}";
    }
    for (c, &n) in m.pips.iter().enumerate() {
        for _ in 0..n {
            s += &format!("{{{}}}", POOL_NAMES[c]);
        }
    }
    for &h in m.hybrid.iter().filter(|&&h| h != 0) {
        let names: Vec<&str> = (0..5)
            .filter(|&c| h & (1 << c) != 0)
            .map(|c| POOL_NAMES[c])
            .collect();
        s += &format!("{{{}}}", names.join("/"));
    }
    if s.is_empty() { "{0}".into() } else { s }
}

/// "{G}{G}{C}" for a pool.
pub fn pool_str(pool: &[u8; 6]) -> String {
    let mut s = String::new();
    for (c, &n) in pool.iter().enumerate() {
        for _ in 0..n {
            s += &format!("{{{}}}", POOL_NAMES[c]);
        }
    }
    if s.is_empty() { "empty".into() } else { s }
}

/// Pays as much of `cost` as `pool` (and `floating` mana of any colour, for generic) can:
/// the pool and floating mana left, and the part of the cost still unpaid.
pub fn cover(pool: [u8; 6], floating: u8, cost: &Mana) -> ([u8; 6], u8, Mana) {
    let mut pool = pool;
    let mut floating = floating;
    let mut rest = Mana { x: false, ..*cost };
    for c in 0..5 {
        let k = pool[c].min(rest.pips[c]);
        pool[c] -= k;
        rest.pips[c] -= k;
    }
    let k = pool[5].min(rest.colorless);
    pool[5] -= k;
    rest.colorless -= k;
    for h in rest.hybrid.iter_mut().filter(|h| **h != 0) {
        if let Some(c) = (0..5)
            .filter(|&c| *h & (1 << c) != 0 && pool[c] > 0)
            .max_by_key(|&c| pool[c])
        {
            pool[c] -= 1;
            *h = 0;
        }
    }
    // Generic: colourless first, then the colours there's most of, then floating mana.
    while rest.generic > 0 {
        let c = if pool[5] > 0 {
            Some(5)
        } else {
            (0..5).filter(|&c| pool[c] > 0).max_by_key(|&c| pool[c])
        };
        match c {
            Some(c) => pool[c] -= 1,
            None if floating > 0 => floating -= 1,
            None => break,
        }
        rest.generic -= 1;
    }
    (pool, floating, rest)
}

impl<'a> Game<'a> {
    pub fn is_human(&self, p: usize) -> bool {
        self.human.as_ref().is_some_and(|h| h.seat == p)
    }

    fn link(&self) -> &HumanLink {
        self.human.as_ref().expect("a human seat")
    }

    fn set_phase(&self, s: &str) {
        if let Some(h) = &self.human {
            *h.phase.borrow_mut() = s.to_string();
        }
    }

    /// Asks the human and waits. `edits_ok`: sandbox edits and manual tapping may be applied
    /// at this question (the caller handles them).
    pub(crate) fn ask(&self, ask: Ask, edits_ok: bool) -> Answer {
        let h = self.link();
        h.io.send(FromEngine::Ask {
            ask: ask.clone(),
            view: Box::new(self.view()),
            edits_ok,
        });
        loop {
            match h.io.recv() {
                ToEngine::Dump => h.io.send(FromEngine::Dump(self.dump())),
                ToEngine::Answer(a) => {
                    let side = matches!(
                        a,
                        Answer::Edit(_) | Answer::Tap { .. } | Answer::Filter { .. }
                    );
                    if side && !edits_ok {
                        continue;
                    }
                    h.transcript.borrow_mut().push(a.clone());
                    return a;
                }
            }
        }
    }

    /// Applies a sandbox edit or a mana tap given at a question. True if it was one.
    fn side_answer(&mut self, p: usize, a: &Answer) -> bool {
        match a {
            Answer::Tap { id, color } => {
                self.human_tap(p, *id, *color);
                true
            }
            Answer::Filter { id, colors } => {
                self.human_filter(p, *id, colors);
                true
            }
            Answer::Edit(e) => {
                self.apply_edit(e);
                true
            }
            _ => false,
        }
    }

    /// A stop for the human to look at the game.
    pub(crate) fn checkpoint(&mut self, prompt: String) {
        let Some(seat) = self.human.as_ref().map(|h| h.seat) else {
            return;
        };
        loop {
            let a = self.ask(
                Ask::Continue {
                    prompt: prompt.clone(),
                },
                true,
            );
            if !self.side_answer(seat, &a) {
                return;
            }
        }
    }

    // ------------------------------------------------------------ the view

    pub fn view(&self) -> GameView {
        let h = self.link();
        let seat = h.seat;
        let log = self.log.as_deref().unwrap_or(&[]);
        let from = h.log_sent.get().min(log.len());
        h.log_sent.set(log.len());
        let players = (0..self.players.len())
            .map(|p| {
                let pl = &self.players[p];
                let names = |v: &[u16]| -> Vec<String> {
                    v.iter()
                        .map(|&c| pl.cards[c as usize].name.clone())
                        .collect()
                };
                let show = p == seat || h.opts.show_bot_hands;
                PlayerView {
                    name: pl
                        .command
                        .iter()
                        .map(|&(c, _)| pl.cards[c as usize].name.clone())
                        .chain(
                            self.perms
                                .iter()
                                .filter(|x| x.owner as usize == p && x.commander > 0)
                                .map(|x| self.perm_name(x)),
                        )
                        .collect::<Vec<_>>()
                        .join(" + "),
                    life: pl.life,
                    poison: pl.poison,
                    alive: pl.alive,
                    hand: if show { names(&pl.hand) } else { Vec::new() },
                    hand_count: pl.hand.len(),
                    library: pl.library.len(),
                    graveyard: names(&pl.graveyard),
                    exile: names(&pl.exile),
                    command: pl
                        .command
                        .iter()
                        .map(|&(c, slot)| {
                            (
                                pl.cards[c as usize].name.clone(),
                                2 * pl.casts[(slot as usize).min(2)] as u32,
                            )
                        })
                        .collect(),
                    floating: pl.pool,
                    commander_damage: (0..self.players.len())
                        .flat_map(|o| {
                            pl.cmd_damage[o % 8]
                                .iter()
                                .filter(|&&d| d > 0)
                                .map(move |d| format!("{} from P{}", d, o))
                        })
                        .collect(),
                }
            })
            .collect();
        let perms = self
            .perms
            .iter()
            .map(|x| PermView {
                id: x.id,
                name: self.perm_name(x),
                owner: x.owner as usize,
                ctrl: x.ctrl as usize,
                p: x.p,
                t: x.t,
                creature: x.is_creature(),
                land: x.is(T_LAND),
                planeswalker: x.is(T_PLANESWALKER),
                tapped: x.tapped,
                sick: x.sick && x.is_creature() && !x.has(HASTE),
                counters: x.counters,
                other_counters: x.other_counters,
                loyalty: x.loyalty,
                damage: x.damage,
                attached: x.attached,
                attacking: x.attacking,
                blocking: x.blocking,
                token: x.is_token(),
                commander: x.commander > 0,
                keywords: super::probe::kw_names(x.eff_kw),
            })
            .collect();
        let sources = self
            .sources(seat)
            .into_iter()
            .filter(|s| s.0 < self.perms.len())
            .map(|s| SourceView {
                id: self.perms[s.0].id,
                name: self.perm_name(&self.perms[s.0]),
                colors: if s.1 == 0 { COLORLESS } else { s.1 },
                amount: s.2,
                filter: self.filter_of(s.0),
            })
            .collect();
        GameView {
            round: self.round,
            active: self.active,
            seat,
            phase: h.phase.borrow().clone(),
            players,
            perms,
            pool: h.pool.get(),
            sources,
            new_log: log[from..].to_vec(),
        }
    }

    /// Everything about the game, for a bug report: every zone of every player and every
    /// permanent with all its fields.
    pub fn dump(&self) -> String {
        let mut s = String::new();
        s += &format!(
            "Round {}, active P{}, tick {}, in combat {}, pending triggers {}\n",
            self.round,
            self.active,
            self.tick,
            self.in_combat,
            self.pending.len()
        );
        if let Some(h) = &self.human {
            s += &format!(
                "Human seat P{} ({}), pool {}, options {:?}\n",
                h.seat,
                h.phase.borrow(),
                pool_str(&h.pool.get()),
                h.opts
            );
        }
        for (p, pl) in self.players.iter().enumerate() {
            let names = |v: &[u16]| -> String {
                v.iter()
                    .map(|&c| pl.cards[c as usize].name.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            };
            s += &format!(
                "\n== P{} life {} poison {} alive {} floating {} lands played {} mulligans {} \
                 casts {:?} cmd damage {:?}\n",
                p,
                pl.life,
                pl.poison,
                pl.alive,
                pl.pool,
                pl.lands_played,
                pl.mulligans,
                pl.casts,
                &pl.cmd_damage[..self.players.len().min(8)]
            );
            s += &format!("  hand: {}\n", names(&pl.hand));
            s += &format!(
                "  library ({}), top first: {}\n",
                pl.library.len(),
                pl.library
                    .iter()
                    .rev()
                    .map(|&c| pl.cards[c as usize].name.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            );
            s += &format!("  graveyard: {}\n", names(&pl.graveyard));
            s += &format!("  exile: {}\n", names(&pl.exile));
            s += &format!(
                "  command: {}\n",
                pl.command
                    .iter()
                    .map(|&(c, k)| format!("{} (slot {})", pl.cards[c as usize].name, k))
                    .collect::<Vec<_>>()
                    .join("; ")
            );
            s += &format!("  adventure: {}\n", names(&pl.adventure));
            s += &format!(
                "  playable from exile: {}\n",
                pl.playable
                    .iter()
                    .map(|x| self.card(x.card).name.clone())
                    .collect::<Vec<_>>()
                    .join("; ")
            );
        }
        s += "\n== Battlefield\n";
        for x in &self.perms {
            s += &format!("  {}\n    {:?}\n", self.perm_desc(x), x);
        }
        s
    }

    // ------------------------------------------------------------ mana

    /// The human's pool as payment sources (1 mana each), for `can_pay` and payment plans.
    pub(crate) fn pool_units(&self, p: usize) -> Vec<(usize, u8, u8, bool)> {
        match &self.human {
            Some(h) if h.seat == p => {
                let pool = h.pool.get();
                let mut out = Vec::new();
                for (c, &n) in pool.iter().enumerate() {
                    let mask = if c < 5 { 1 << c } else { COLORLESS };
                    for _ in 0..n {
                        out.push((POOL_SRC + out.len(), mask, 1, false));
                    }
                }
                out
            }
            _ => Vec::new(),
        }
    }

    /// Takes the pool mana a payment plan used (`pay_planned`).
    pub(crate) fn spend_pool_units(&mut self, p: usize, idxs: &[usize]) {
        let units = self.pool_units(p);
        let Some(h) = &self.human else { return };
        let mut pool = h.pool.get();
        for &i in idxs.iter().filter(|&&i| i >= POOL_SRC) {
            if let Some(u) = units.get(i - POOL_SRC) {
                let c = if u.1 == COLORLESS {
                    5
                } else {
                    u.1.trailing_zeros() as usize
                };
                pool[c] = pool[c].saturating_sub(1);
            }
        }
        h.pool.set(pool);
    }

    fn pool_get(&self) -> [u8; 6] {
        self.human.as_ref().map_or([0; 6], |h| h.pool.get())
    }

    fn pool_set(&self, pool: [u8; 6]) {
        if let Some(h) = &self.human {
            h.pool.set(pool);
        }
    }

    /// Mana empties from the human's pool at the end of a step or phase.
    pub(crate) fn empty_pool(&mut self) {
        let Some(h) = &self.human else { return };
        let pool = h.pool.get();
        let seat = h.seat;
        if pool.iter().any(|&n| n > 0) {
            h.pool.set([0; 6]);
            self.note(|| format!("P{} mana pool empties ({} unused)", seat, pool_str(&pool)));
        }
        self.players[seat].pool = 0;
    }

    /// Tap source `id` for mana of `color`.
    fn human_tap(&mut self, p: usize, id: u32, color: u8) {
        let Some(i) = self.idx(id) else { return };
        let Some(s) = self.sources(p).into_iter().find(|s| s.0 == i) else {
            let n = self.id_desc(id);
            self.note(|| format!("  (can't tap {} for mana now)", n));
            return;
        };
        let mask = if s.1 == 0 { COLORLESS } else { s.1 };
        let bit = if color < 5 { 1 << color } else { COLORLESS };
        if mask & bit == 0 {
            let n = self.id_desc(id);
            self.note(|| {
                format!(
                    "  ({} can't make {{{}}})",
                    n,
                    POOL_NAMES[color.min(5) as usize]
                )
            });
            return;
        }
        let name = self.id_desc(id);
        self.mana_source_used(i);
        let mut pool = self.pool_get();
        pool[color.min(5) as usize] += s.2;
        self.pool_set(pool);
        self.note(|| {
            format!(
                "P{} taps {} for {}",
                p,
                name,
                std::iter::repeat_n(
                    format!("{{{}}}", POOL_NAMES[color.min(5) as usize]),
                    s.2 as usize
                )
                .collect::<String>()
            )
        });
    }

    /// A source made its mana: tapped, sacrificed (Treasure) or its counters used (Ramos).
    fn mana_source_used(&mut self, i: usize) {
        if matches!(self.perms[i].tok, Tok::Treasure | Tok::Lotus) {
            self.leave_why = super::audit::LeaveWhy::ManaCost;
            self.leave(i, Zone::Graveyard, true);
        } else if let Some((need, ..)) = self.perms[i]
            .card
            .and_then(|r| self.card(r).counter_mana)
            .filter(|_| self.perms[i].fired & COUNTER_MANA_USED == 0)
            .filter(|(need, ..)| self.perms[i].counters >= *need)
        {
            self.perms[i].counters -= need;
            self.perms[i].fired |= COUNTER_MANA_USED;
            self.recalc();
        } else {
            self.perms[i].tapped = true;
        }
    }

    /// A filter: pays its cost from the pool, makes mana of the chosen colours.
    fn human_filter(&mut self, p: usize, id: u32, colors: &[u8]) {
        let Some(i) = self.idx(id) else { return };
        let Some((fee, made)) = self.filter_of(i) else {
            return;
        };
        let (pool, floating, rest) =
            cover(self.pool_get(), self.players[p].pool, &Mana::generic(fee));
        if rest.total() > 0 {
            self.note(|| format!("  (the filter costs {{{}}}: tap that much mana first)", fee));
            return;
        }
        self.pool_set(pool);
        self.players[p].pool = floating;
        self.perms[i].tapped = true;
        let mut pool = self.pool_get();
        for k in 0..made as usize {
            let c = colors
                .get(k)
                .or(colors.first())
                .copied()
                .unwrap_or(5)
                .min(5);
            pool[c as usize] += 1;
        }
        self.pool_set(pool);
        let name = self.id_desc(id);
        let got = pool_str(&pool);
        self.note(|| format!("P{} uses {}'s filter: pool now {}", p, name, got));
    }

    /// Pays `cost` for the human: from the pool, asking for more mana when it's short.
    pub(crate) fn human_pay(&mut self, p: usize, cost: &Mana) -> bool {
        let mut auto = self.link().auto_pay.get() || self.link().opts.auto_mana;
        loop {
            let (pool, floating, rest) = cover(self.pool_get(), self.players[p].pool, cost);
            if rest.total() == 0 {
                self.pool_set(pool);
                self.players[p].pool = floating;
                return true;
            }
            if auto {
                // The pool first, the rest from untapped sources.
                let real = self.sources(p);
                if self.plan_from_sources(p, &rest, &real).is_none() {
                    return false;
                }
                self.pool_set(pool);
                self.players[p].pool = floating;
                return self.pay_planned(p, &rest);
            }
            let auto_ok = {
                let real = self.sources(p);
                self.plan_from_sources(p, &rest, &real).is_some()
            };
            let what = match self.ext.paying.get() {
                Purpose::Spell { card, .. } => format!("Pay for {}", self.card(card).name),
                Purpose::Ability { src } => format!("Pay for {}'s ability", self.id_desc(src)),
                _ => "Pay a cost".to_string(),
            };
            let a = self.ask(
                Ask::Pay {
                    prompt: what,
                    cost: mana_str(cost),
                    missing: mana_str(&rest),
                    auto_ok,
                },
                true,
            );
            match a {
                Answer::Auto | Answer::AutoTap(_) | Answer::Continue => auto = true,
                Answer::Cancel | Answer::Pass | Answer::Concede => return false,
                a => {
                    if !self.side_answer(p, &a) {
                        auto = true;
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------ mulligans

    pub(crate) fn human_mulligan(&mut self, p: usize) {
        self.set_phase("Mulligan");
        for attempt in 0..7u8 {
            if attempt > 0 {
                let hand = std::mem::take(&mut self.players[p].hand);
                self.players[p].library.extend(hand);
                let mut lib = std::mem::take(&mut self.players[p].library);
                self.rng.shuffle(&mut lib);
                self.players[p].library = lib;
            }
            for _ in 0..7 {
                if let Some(c) = self.players[p].library.pop() {
                    self.players[p].hand.push(c);
                }
            }
            self.players[p].mulligans = attempt;
            let hand: Vec<String> = self.players[p]
                .hand
                .iter()
                .map(|&c| self.players[p].cards[c as usize].name.clone())
                .collect();
            let suggest = ai::keepable(self, p, attempt) || attempt >= 2;
            let names = hand.join("; ");
            self.note(|| format!("P{} opening hand #{}: {}", p, attempt + 1, names));
            let keep = match self.ask(
                Ask::Mulligan {
                    hand,
                    mulligans: attempt,
                    suggest_keep: suggest,
                },
                false,
            ) {
                Answer::Yes(k) => k,
                _ => suggest,
            } || attempt == 6;
            if !keep {
                self.note(|| format!("P{} mulligans", p));
                continue;
            }
            // One card to the bottom per mulligan after the free one (as the AI does).
            let bottom = (attempt.saturating_sub(1) as usize).min(self.players[p].hand.len());
            if bottom > 0 {
                let mut sug = Vec::new();
                let mut left: Vec<usize> = (0..self.players[p].hand.len()).collect();
                for _ in 0..bottom {
                    // The AI's worst cards.
                    if let Some(k) = left.iter().copied().min_by(|&a, &b| {
                        let pl = &self.players[p];
                        let va = ai::card_worth(self, p, &pl.cards[pl.hand[a] as usize]);
                        let vb = ai::card_worth(self, p, &pl.cards[pl.hand[b] as usize]);
                        va.partial_cmp(&vb).unwrap_or(std::cmp::Ordering::Equal)
                    }) {
                        sug.push(k);
                        left.retain(|&x| x != k);
                    }
                }
                let options = self.players[p]
                    .hand
                    .iter()
                    .map(|&c| self.players[p].cards[c as usize].name.clone())
                    .collect();
                let pick = match self.ask(
                    Ask::Pick {
                        prompt: format!("Put {} card(s) on the bottom of your library", bottom),
                        options,
                        min: bottom,
                        max: bottom,
                        suggested: sug.clone(),
                    },
                    false,
                ) {
                    Answer::Pick(v) if v.len() == bottom => v,
                    _ => sug,
                };
                let mut pick = pick;
                pick.sort();
                pick.dedup();
                for &h in pick.iter().rev() {
                    if h < self.players[p].hand.len() {
                        let c = self.players[p].hand.remove(h);
                        let name = self.players[p].cards[c as usize].name.clone();
                        self.note(|| format!("P{} puts {} on the bottom", p, name));
                        self.players[p].library.insert(0, c);
                    }
                }
            }
            let kept = self.players[p].hand.len() as u8;
            if let Some(s) = self.stats.as_mut() {
                s.players[p].mulligans = attempt;
                s.players[p].kept = kept;
            }
            self.note(|| format!("P{} keeps {} cards after {} mulligan(s)", p, kept, attempt));
            return;
        }
    }

    // ------------------------------------------------------------ actions

    fn act_option(
        &self,
        p: usize,
        label: String,
        cost: Option<Mana>,
        why: Purpose,
        card: Option<String>,
    ) -> ActOption {
        let cost = cost.unwrap_or_default();
        let (_, _, rest) = cover(self.pool_get(), self.players[p].pool, &cost);
        ActOption {
            label,
            cost: if cost.total() == 0 && !cost.x {
                String::new()
            } else {
                mana_str(&cost)
            },
            payable: rest.total() == 0,
            missing: if rest.total() == 0 {
                String::new()
            } else {
                mana_str(&rest)
            },
            auto_ok: self.can_pay_for(p, &cost, why),
            card,
        }
    }

    /// Everything the human may do now: in a main phase (`main`) or at instant speed.
    fn human_actions(&self, p: usize, main: bool) -> Vec<(ActOption, HAct)> {
        let mut out: Vec<(ActOption, HAct)> = Vec::new();
        let pl = &self.players[p];
        let instant_ok = |c: &SimCard, back: bool| {
            main || if back {
                c.back.as_ref().is_some_and(|b| b.types & T_INSTANT != 0)
            } else {
                c.is(T_INSTANT) || ai::has_flash(self, p, c)
            }
        };
        // Lands.
        if main && pl.lands_played < self.land_drops(p) {
            for (h, &idx) in pl.hand.iter().enumerate() {
                let c = &pl.cards[idx as usize];
                if c.is(T_LAND) {
                    let label = format!("Play land: {}", c.name);
                    out.push((
                        ActOption {
                            label,
                            payable: true,
                            auto_ok: true,
                            card: Some(c.name.clone()),
                            ..Default::default()
                        },
                        HAct::Land(h),
                    ));
                }
            }
            if let Some(k) = ai::other_zone_land(self, p) {
                out.push((
                    ActOption {
                        label: "Play a land from another zone (graveyard, library top or exile)"
                            .into(),
                        payable: true,
                        auto_ok: true,
                        ..Default::default()
                    },
                    HAct::LandFrom(k),
                ));
            }
        }
        // Spells.
        let cast =
            |out: &mut Vec<(ActOption, HAct)>, src: Src, idx: u16, back: bool, from: &str| {
                let list = self.spell_list(p, src);
                let c = &self.players[list].cards[idx as usize];
                if !instant_ok(c, back) {
                    return;
                }
                let name = if back {
                    c.back
                        .as_ref()
                        .map(|b| format!("{} (other face)", b.name))
                        .unwrap_or_else(|| c.name.clone())
                } else {
                    c.name.clone()
                };
                let why = Purpose::Spell {
                    card: (list as u8, idx),
                    from_hand: matches!(src, Src::Hand(_)),
                };
                let mut variants = vec![(
                    CastOpts {
                        back,
                        ..Default::default()
                    },
                    String::new(),
                )];
                if !back {
                    let base = CastOpts::default();
                    if c.kicker.is_some() {
                        variants.push((
                            CastOpts {
                                kicker: true,
                                ..base
                            },
                            " kicked".into(),
                        ));
                    }
                    if c.overload.is_some() {
                        variants.push((
                            CastOpts {
                                overload: true,
                                ..base
                            },
                            " with overload".into(),
                        ));
                    }
                    if c.sneak.is_some() {
                        variants.push((
                            CastOpts {
                                sneak: true,
                                ..base
                            },
                            " with sneak".into(),
                        ));
                    }
                    if c.teamwork.is_some() {
                        variants.push((
                            CastOpts {
                                teamwork: true,
                                ..base
                            },
                            " with teamwork".into(),
                        ));
                    }
                    if c.gift {
                        variants.push((
                            CastOpts { gift: true, ..base },
                            " promising the gift".into(),
                        ));
                    }
                    if c.offspring.is_some() {
                        variants.push((
                            CastOpts {
                                offspring: true,
                                ..base
                            },
                            " with offspring".into(),
                        ));
                    }
                    if c.convoke {
                        variants.push((
                            CastOpts {
                                convoke: true,
                                ..base
                            },
                            " with convoke".into(),
                        ));
                    }
                }
                for (opts, how) in variants {
                    let Some(cost) = self.cast_cost(p, src, opts) else {
                        continue;
                    };
                    // The X is chosen when it's cast: list the cost without it.
                    let shown = if c.cost.x && !back {
                        Mana {
                            x: true,
                            ..self
                                .cast_cost(p, src, CastOpts { x: Some(0), ..opts })
                                .unwrap_or(cost)
                        }
                    } else {
                        cost
                    };
                    let o = self.act_option(
                        p,
                        format!("Cast {}{}{}", name, how, from),
                        Some(shown),
                        why,
                        Some(c.name.clone()),
                    );
                    if o.payable || o.auto_ok {
                        out.push((o, HAct::Cast(src, opts)));
                    }
                }
            };
        for (h, &idx) in pl.hand.iter().enumerate() {
            let c = &pl.cards[idx as usize];
            if !c.is(T_LAND) {
                cast(&mut out, Src::Hand(h), idx, false, "");
            }
            if c.back
                .as_ref()
                .is_some_and(|b| b.kind != BackKind::Prepare && b.kind != BackKind::Transform)
            {
                cast(&mut out, Src::Hand(h), idx, true, "");
            }
        }
        for (k, &(idx, _)) in pl.command.iter().enumerate() {
            cast(
                &mut out,
                Src::Command(k),
                idx,
                false,
                " from the command zone",
            );
        }
        for (k, &idx) in pl.adventure.iter().enumerate() {
            cast(
                &mut out,
                Src::Adventure(k),
                idx,
                false,
                " from its adventure",
            );
        }
        for (k, x) in pl.playable.iter().enumerate() {
            let c = self.card(x.card);
            if !c.is(T_LAND) {
                cast(&mut out, Src::Exile(k), x.card.1, false, " from exile");
            }
        }
        for (k, &idx) in pl.graveyard.iter().enumerate() {
            let c = &pl.cards[idx as usize];
            if pl.recast.contains(&idx)
                || c.flashback.is_some()
                || self.graveyard_type_free(p, c).is_some()
            {
                cast(
                    &mut out,
                    Src::Graveyard(k),
                    idx,
                    false,
                    " from your graveyard",
                );
            }
        }
        for x in self
            .perms
            .iter()
            .filter(|x| x.ctrl as usize == p && x.prepared)
        {
            if let Some(r) = x.card {
                cast(&mut out, Src::Prepared(x.id), r.1, true, " (prepared)");
            }
        }
        // Abilities of cards in hand and graveyard, cycling.
        for (h, &idx) in pl.hand.iter().enumerate() {
            let c = &pl.cards[idx as usize];
            for (k, ab) in c.abilities.iter().enumerate() {
                if let Some(cost) = self.hand_activation_cost(p, h, k) {
                    let o = self.act_option(
                        p,
                        format!("{} (from hand): {}", c.name, short(&ab.text)),
                        Some(cost),
                        Purpose::Any,
                        Some(c.name.clone()),
                    );
                    if o.payable || o.auto_ok {
                        out.push((o, HAct::ActivateHand(h, k)));
                    }
                }
            }
            if let Some((n, _)) = &c.cycling {
                let cost = c.cycling_mana.unwrap_or(Mana::generic(*n as u32));
                let o = self.act_option(
                    p,
                    format!("Cycle {}", c.name),
                    Some(cost),
                    Purpose::Any,
                    Some(c.name.clone()),
                );
                if o.payable || o.auto_ok {
                    out.push((o, HAct::Cycle(h)));
                }
            }
        }
        for (gk, &idx) in pl.graveyard.iter().enumerate() {
            let c = &pl.cards[idx as usize];
            for (k, ab) in c.abilities.iter().enumerate() {
                if let Some(cost) = self.graveyard_activation_cost(p, gk, k) {
                    let o = self.act_option(
                        p,
                        format!("{} (from graveyard): {}", c.name, short(&ab.text)),
                        Some(cost),
                        Purpose::Any,
                        Some(c.name.clone()),
                    );
                    if o.payable || o.auto_ok {
                        out.push((o, HAct::ActivateGy(gk, k)));
                    }
                }
            }
        }
        // Abilities of permanents, equip, crew.
        for (i, x) in self.perms.iter().enumerate() {
            if x.ctrl as usize != p {
                continue;
            }
            let name = self.perm_name(x);
            for (k, a) in self.abilities_of(x).iter().enumerate() {
                let sorcery_speed = match &a.trig {
                    Trig::Activated { sorcery, .. } => *sorcery,
                    Trig::PowerUp(_) | Trig::Loyalty(_) => true,
                    _ => continue,
                };
                if ai::mana_only(a) || sorcery_speed && !main {
                    continue;
                }
                let Some(cost) = self.activation_cost(p, i, k) else {
                    continue;
                };
                let o = self.act_option(
                    p,
                    format!("{}: {}", name, short(&a.text)),
                    Some(cost),
                    Purpose::Ability { src: x.id },
                    x.card.map(|r| self.card(r).name.clone()),
                );
                if o.payable || o.auto_ok {
                    out.push((o, HAct::Activate(x.id, k)));
                }
            }
            if main && x.is(T_EQUIPMENT) {
                for y in self
                    .perms
                    .iter()
                    .filter(|y| y.ctrl as usize == p && y.is_creature())
                {
                    if x.attached == Some(y.id) {
                        continue;
                    }
                    if let Some((cost, _life)) = self.equip_cost(p, x.id, y.id) {
                        let o = self.act_option(
                            p,
                            format!("Equip {} to {}", name, self.perm_name(y)),
                            Some(cost),
                            Purpose::Ability { src: x.id },
                            x.card.map(|r| self.card(r).name.clone()),
                        );
                        if o.payable || o.auto_ok {
                            out.push((o, HAct::Equip(x.id, y.id)));
                        }
                    }
                }
            }
            if !x.crewed && x.card.is_some_and(|r| self.card(r).crew.is_some()) {
                let need = x.card.and_then(|r| self.card(r).crew).unwrap_or(2) as i16;
                if !ai::crew_tappers(self, p, need, x.id).is_empty() {
                    out.push((
                        ActOption {
                            label: format!("Crew {} (taps creatures with power {})", name, need),
                            payable: true,
                            auto_ok: true,
                            ..Default::default()
                        },
                        HAct::Crew(x.id),
                    ));
                }
            }
        }
        out
    }

    /// Does what the human chose; `auto`: costs paid from untapped sources.
    fn human_do(&mut self, p: usize, act: HAct, auto: bool) -> bool {
        self.link().auto_pay.set(auto);
        let ok = match act {
            HAct::Land(h) => {
                self.play_land(p, h);
                true
            }
            HAct::LandFrom(k) => {
                self.play_land_from(p, k);
                true
            }
            HAct::Cast(src, mut opts) => {
                let x_spell = self
                    .spell_card(p, src, opts)
                    .map(|(idx, back)| {
                        !back
                            && self.players[self.spell_list(p, src)].cards[idx as usize]
                                .cost
                                .x
                    })
                    .unwrap_or(false);
                if x_spell && opts.x.is_none() {
                    let base = self
                        .cast_cost(p, src, CastOpts { x: Some(0), ..opts })
                        .map_or(0, |m| m.total());
                    let have = {
                        let (pool, fl, _) = (self.pool_get(), self.players[p].pool, ());
                        let pool_n: u32 = pool.iter().map(|&n| n as u32).sum::<u32>() + fl as u32;
                        if auto || self.link().opts.auto_mana {
                            self.mana_available(p)
                        } else {
                            pool_n.max(self.mana_available(p))
                        }
                    };
                    let max = have.saturating_sub(base) as i32;
                    let suggested = self
                        .cast_plan(p, src, opts)
                        .and_then(|(_, x, _)| x)
                        .unwrap_or(0)
                        .min(max);
                    let n = match self.ask(
                        Ask::Number {
                            prompt: "Choose X".into(),
                            min: 0,
                            max: max.max(0),
                            suggested,
                        },
                        false,
                    ) {
                        Answer::Num(n) => n.clamp(0, max.max(0)),
                        _ => suggested,
                    };
                    opts.x = Some(n as u16);
                }
                self.cast(p, src, opts)
            }
            HAct::Activate(id, k) => self.activate(p, id, k),
            HAct::ActivateGy(gk, k) => self.activate_from_graveyard(p, gk, k),
            HAct::ActivateHand(h, k) => self.activate_from_hand(p, h, k),
            HAct::Equip(e, t) => self.equip(p, e, t),
            HAct::Cycle(h) => self.cycle(p, h),
            HAct::Crew(v) => self.crew(p, v),
        };
        self.link().auto_pay.set(false);
        self.run_pending();
        self.sba();
        if !ok {
            self.note(|| "  (that couldn't be done: not paid, or no longer legal)".into());
        }
        ok
    }

    /// The human's main phase (`post`: after combat).
    pub(crate) fn human_main(&mut self, p: usize, post: bool) {
        let phase = if post { "Main phase 2" } else { "Main phase 1" };
        self.set_phase(phase);
        for _ in 0..500 {
            if !self.players[p].alive || self.alive_count() <= 1 {
                break;
            }
            let acts = self.human_actions(p, true);
            let a = self.ask(
                Ask::Act {
                    phase: phase.into(),
                    actions: acts.iter().map(|a| a.0.clone()).collect(),
                    window: false,
                    suggested: None,
                },
                true,
            );
            match a {
                Answer::Pass | Answer::Continue | Answer::Cancel => break,
                Answer::Auto => {
                    self.note(|| format!("(P{} lets the bot play the rest of this phase)", p));
                    ai::main_phase(self, p, post);
                    break;
                }
                Answer::Concede => {
                    self.concede(p);
                    break;
                }
                Answer::Pick(v) => {
                    if let Some(&(_, act)) = v.first().and_then(|&i| acts.get(i)) {
                        self.human_do(p, act, false);
                    }
                }
                Answer::AutoTap(i) => {
                    if let Some(&(_, act)) = acts.get(i) {
                        self.human_do(p, act, true);
                    }
                }
                a => {
                    self.side_answer(p, &a);
                }
            }
        }
        self.ext.reserve.set(0);
        self.ext.follow.set(Mana::default());
        self.empty_pool();
    }

    /// An instant-speed window for the human (`what`: "end of P2's turn"). `bot` is what the
    /// AI would do in this window, for "let the bot decide".
    pub(crate) fn human_window(&mut self, p: usize, what: &str, bot: impl FnOnce(&mut Game<'a>)) {
        self.set_phase(what);
        let mut bot = Some(bot);
        for _ in 0..200 {
            if !self.players[p].alive || self.alive_count() <= 1 {
                break;
            }
            let acts = self.human_actions(p, false);
            let pool_empty = self.pool_get().iter().all(|&n| n == 0);
            if acts.is_empty() && pool_empty && !self.link().opts.stop_every_window {
                break;
            }
            let a = self.ask(
                Ask::Act {
                    phase: what.into(),
                    actions: acts.iter().map(|a| a.0.clone()).collect(),
                    window: true,
                    suggested: None,
                },
                true,
            );
            match a {
                Answer::Pass | Answer::Continue | Answer::Cancel => break,
                Answer::Auto => {
                    if let Some(f) = bot.take() {
                        f(self);
                    }
                    break;
                }
                Answer::Concede => {
                    self.concede(p);
                    break;
                }
                Answer::Pick(v) => {
                    if let Some(&(_, act)) = v.first().and_then(|&i| acts.get(i)) {
                        self.human_do(p, act, false);
                    }
                }
                Answer::AutoTap(i) => {
                    if let Some(&(_, act)) = acts.get(i) {
                        self.human_do(p, act, true);
                    }
                }
                a => {
                    self.side_answer(p, &a);
                }
            }
        }
        self.empty_pool();
    }

    fn concede(&mut self, p: usize) {
        self.note(|| format!("P{} concedes", p));
        self.players[p].alive = false;
        self.players[p].life = 0;
    }

    // ------------------------------------------------------------ combat

    pub(crate) fn human_attacks(&mut self, a: usize) -> Vec<(u32, Def)> {
        self.set_phase("Declare attackers");
        let cands: Vec<(u32, String)> = (0..self.perms.len())
            .filter(|&i| {
                self.perms[i].ctrl as usize == a
                    && self.perms[i].is_creature()
                    && self.can_attack(i)
            })
            .map(|i| (self.perms[i].id, self.perm_desc(&self.perms[i])))
            .collect();
        if cands.is_empty() {
            return Vec::new();
        }
        let mut defs: Vec<Def> = Vec::new();
        let mut labels = Vec::new();
        for o in self.opponents(a) {
            defs.push(Def::Player(o));
            labels.push(format!(
                "P{} {} ({} life)",
                o,
                self.commander_label(o),
                self.players[o].life
            ));
            for x in self
                .perms
                .iter()
                .filter(|x| x.ctrl as usize == o && x.is(T_PLANESWALKER))
            {
                defs.push(Def::Walker(x.id));
                labels.push(format!("planeswalker {}", self.perm_desc(x)));
            }
        }
        let ai_pick = ai::declare_attacks(self, a);
        let suggested: Vec<(u32, usize)> = ai_pick
            .iter()
            .filter_map(|(id, d)| defs.iter().position(|x| x == d).map(|k| (*id, k)))
            .collect();
        loop {
            match self.ask(
                Ask::Attack {
                    attackers: cands.clone(),
                    defenders: labels.clone(),
                    suggested: suggested.clone(),
                },
                true,
            ) {
                Answer::Attacks(v) => {
                    return v
                        .into_iter()
                        .filter(|(id, _)| cands.iter().any(|c| c.0 == *id))
                        .filter_map(|(id, k)| defs.get(k).map(|&d| (id, d)))
                        .collect();
                }
                Answer::Auto => return ai_pick,
                Answer::Pass | Answer::Cancel | Answer::Continue => return Vec::new(),
                x => {
                    if !self.side_answer(a, &x) {
                        return Vec::new();
                    }
                }
            }
        }
    }

    fn commander_label(&self, p: usize) -> String {
        let pl = &self.players[p];
        pl.cards
            .iter()
            .enumerate()
            .find(|(k, _)| {
                pl.command.iter().any(|&(c, _)| c as usize == *k)
                    || self.perms.iter().any(|x| {
                        x.commander > 0
                            && x.owner as usize == p
                            && x.card == Some((p as u8, *k as u16))
                    })
            })
            .map(|(_, c)| c.name.clone())
            .unwrap_or_default()
    }

    pub(crate) fn human_blocks(&mut self, d: usize, attacks: &[Attack]) -> Vec<(u32, u32)> {
        self.set_phase("Declare blockers");
        let atts: Vec<(u32, String)> = attacks
            .iter()
            .filter_map(|at| {
                self.idx(at.attacker)
                    .map(|i| (at.attacker, self.perm_desc(&self.perms[i])))
            })
            .collect();
        let blockers: Vec<(u32, String, Vec<u32>)> = (0..self.perms.len())
            .filter(|&b| self.perms[b].ctrl as usize == d && self.perms[b].is_creature())
            .filter_map(|b| {
                let can: Vec<u32> = attacks
                    .iter()
                    .filter(|at| {
                        self.idx(at.attacker)
                            .is_some_and(|ai_| self.can_block(b, ai_))
                    })
                    .map(|at| at.attacker)
                    .collect();
                (!can.is_empty()).then(|| (self.perms[b].id, self.perm_desc(&self.perms[b]), can))
            })
            .collect();
        if blockers.is_empty() || atts.is_empty() {
            return Vec::new();
        }
        let ai_pick = ai::declare_blocks(self, d, attacks);
        loop {
            match self.ask(
                Ask::Block {
                    attackers: atts.clone(),
                    blockers: blockers.clone(),
                    suggested: ai_pick.clone(),
                },
                true,
            ) {
                Answer::Blocks(v) => return v,
                Answer::Auto => return ai_pick,
                Answer::Pass | Answer::Cancel | Answer::Continue => return Vec::new(),
                x => {
                    if !self.side_answer(d, &x) {
                        return Vec::new();
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------ choices while resolving

    fn source_label(&self, ctx: &Ctx) -> String {
        ctx.card
            .map(|c| c.name.clone())
            .or_else(|| ctx.src.map(|id| self.id_desc(id)))
            .unwrap_or_else(|| "An ability".into())
    }

    /// Targets among `cands` (permanent indices): the AI's pick, or the human's.
    pub(crate) fn h_targets(
        &self,
        ctx: &Ctx,
        cands: &[usize],
        n: usize,
        harmful: bool,
    ) -> Vec<usize> {
        let ai_pick = if harmful {
            ai::choose_victims(self, ctx.ctrl, cands, n)
        } else {
            ai::choose_friends(self, ctx.ctrl, cands, n)
        };
        if !self.is_human(ctx.ctrl) || cands.is_empty() {
            return ai_pick;
        }
        let up_to = ctx.text.contains("up to") || ctx.text.contains("any number of");
        let max = n.min(cands.len());
        let min = if up_to { 0 } else { max };
        let options: Vec<String> = cands
            .iter()
            .map(|&i| self.perm_desc(&self.perms[i]))
            .collect();
        let suggested: Vec<usize> = ai_pick
            .iter()
            .filter_map(|i| cands.iter().position(|c| c == i))
            .collect();
        match self.ask(
            Ask::Pick {
                prompt: format!(
                    "{}: \"{}\" - choose {}{} target(s)",
                    self.source_label(ctx),
                    short(ctx.text),
                    if up_to { "up to " } else { "" },
                    max
                ),
                options,
                min,
                max,
                suggested,
            },
            false,
        ) {
            Answer::Pick(v) => {
                let mut v: Vec<usize> = v.into_iter().filter(|&k| k < cands.len()).collect();
                v.dedup();
                v.truncate(max);
                if v.len() < min {
                    return ai_pick;
                }
                v.into_iter().map(|k| cands[k]).collect()
            }
            _ => ai_pick,
        }
    }

    /// Where damage goes ("deals 3 damage to any target").
    pub(crate) fn h_damage_target(&self, ctx: &Ctx, to: &Target, n: i32) -> ai::Hit {
        let ai_pick = ai::damage_target(self, ctx, to, n);
        if !self.is_human(ctx.ctrl) {
            return ai_pick;
        }
        let mut hits: Vec<ai::Hit> = Vec::new();
        let mut options = Vec::new();
        if to.kind == Kind::AnyTarget || to.kind == Kind::Player {
            for o in self.opponents(ctx.ctrl) {
                hits.push(ai::Hit::Player(o));
                options.push(format!("P{} ({} life)", o, self.players[o].life));
            }
        }
        let t1 = Target { count: 1, ..*to };
        for i in self.candidates(ctx, &t1, true) {
            hits.push(ai::Hit::Perm(self.perms[i].id));
            options.push(self.perm_desc(&self.perms[i]));
        }
        if hits.is_empty() {
            return ai_pick;
        }
        let suggested: Vec<usize> = hits
            .iter()
            .position(|h| *h == ai_pick)
            .into_iter()
            .collect();
        match self.ask(
            Ask::Pick {
                prompt: format!(
                    "{}: {} damage - choose the target",
                    self.source_label(ctx),
                    n
                ),
                options,
                min: 1,
                max: 1,
                suggested,
            },
            false,
        ) {
            Answer::Pick(v) => v
                .first()
                .and_then(|&k| hits.get(k).copied())
                .unwrap_or(ai_pick),
            _ => ai_pick,
        }
    }

    /// "Target opponent" / "choose an opponent".
    pub(crate) fn h_choose_opponent(&self, me: usize, n: i32) -> Option<usize> {
        let ai_pick = ai::choose_opponent(self, me, n);
        if !self.is_human(me) {
            return ai_pick;
        }
        let opps = self.opponents(me);
        if opps.len() <= 1 {
            return ai_pick;
        }
        let options = opps
            .iter()
            .map(|&o| {
                format!(
                    "P{} {} ({} life)",
                    o,
                    self.commander_label(o),
                    self.players[o].life
                )
            })
            .collect();
        let suggested = ai_pick
            .and_then(|o| opps.iter().position(|&x| x == o))
            .into_iter()
            .collect();
        match self.ask(
            Ask::Pick {
                prompt: "Choose an opponent".into(),
                options,
                min: 1,
                max: 1,
                suggested,
            },
            false,
        ) {
            Answer::Pick(v) => v.first().and_then(|&k| opps.get(k).copied()).or(ai_pick),
            _ => ai_pick,
        }
    }

    /// Thanos: odd (true) or even.
    pub(crate) fn h_parity(&self, me: usize, src: Option<u32>) -> (bool, f32) {
        let ai_pick = ai::best_parity(self, me, src);
        if !self.is_human(me) {
            return ai_pick;
        }
        let count = |odd: bool| -> String {
            let mut mine = 0;
            let mut theirs = 0;
            for x in self
                .perms
                .iter()
                .filter(|x| x.is_creature() && Some(x.id) != src && !x.has(INDESTRUCTIBLE))
            {
                if (ai::perm_mv(self, x) % 2 == 1) == odd {
                    if x.ctrl as usize == me {
                        mine += 1;
                    } else {
                        theirs += 1;
                    }
                }
            }
            format!("{} of theirs, {} of yours", theirs, mine)
        };
        match self.ask(
            Ask::Pick {
                prompt: "Choose odd or even: destroy each other creature with mana value of that \
                         quality"
                    .into(),
                options: vec![
                    format!("Odd ({})", count(true)),
                    format!("Even ({})", count(false)),
                ],
                min: 1,
                max: 1,
                suggested: vec![if ai_pick.0 { 0 } else { 1 }],
            },
            false,
        ) {
            Answer::Pick(v) if v.first() == Some(&0) => (true, ai_pick.1),
            Answer::Pick(v) if v.first() == Some(&1) => (false, ai_pick.1),
            _ => ai_pick,
        }
    }

    /// "You may ...".
    pub(crate) fn h_may(&self, ctx: &Ctx, ab: &SimAbility) -> bool {
        let ai_pick = ai::wants_optional(self, ctx, ab);
        if !self.is_human(ctx.ctrl) {
            return ai_pick;
        }
        match self.ask(
            Ask::YesNo {
                prompt: format!(
                    "{}: \"{}\" - do it?",
                    self.source_label(ctx),
                    short(&ab.text)
                ),
                suggested: ai_pick,
            },
            false,
        ) {
            Answer::Yes(b) => b,
            _ => ai_pick,
        }
    }

    /// "You may pay ...".
    pub(crate) fn h_pay_ok(&self, ctx: &Ctx, ab: &SimAbility, pc: &PayCost) -> bool {
        let ai_pick = ai::wants_to_pay(self, ctx, ab, pc);
        if !self.is_human(ctx.ctrl) {
            return ai_pick;
        }
        match self.ask(
            Ask::YesNo {
                prompt: format!(
                    "{}: \"{}\" - pay the cost?",
                    self.source_label(ctx),
                    short(&ab.text)
                ),
                suggested: ai_pick,
            },
            false,
        ) {
            Answer::Yes(b) => b,
            _ => ai_pick,
        }
    }

    /// Modes of a modal ability.
    pub(crate) fn h_modes(&self, ctx: &Ctx, ab: &SimAbility, used: u32) -> Vec<usize> {
        let ai_pick = ai::choose_modes(self, ctx, ab, used);
        if !self.is_human(ctx.ctrl) || ab.mode_texts.is_empty() {
            return ai_pick;
        }
        let t = ab.text.to_lowercase();
        let all = ab.mode_texts.len();
        let (min, max) = if t.contains("one or more") || t.contains("any number") {
            (1, all)
        } else if t.contains("one or both") {
            (1, 2.min(all))
        } else if t.contains("up to two") {
            (0, 2.min(all))
        } else if t.contains("up to three") {
            (0, 3.min(all))
        } else if t.contains("choose two") {
            (2.min(all), 2.min(all))
        } else if t.contains("choose three") {
            (3.min(all), 3.min(all))
        } else {
            (1, 1)
        }
        .clone();
        // Bound by what the AI's reading allows too (teamwork/both modes, "hasn't been chosen").
        let max = max.max(ai_pick.len()).min(all);
        let options: Vec<String> = ab
            .mode_texts
            .iter()
            .enumerate()
            .map(|(m, s)| {
                if used & (1 << m) != 0 {
                    format!("{} (already chosen)", s)
                } else {
                    s.clone()
                }
            })
            .collect();
        match self.ask(
            Ask::Pick {
                prompt: format!("{}: choose the mode(s)", self.source_label(ctx)),
                options,
                min: min.min(max),
                max,
                suggested: ai_pick.clone(),
            },
            false,
        ) {
            Answer::Pick(v) => {
                let v: Vec<usize> = v
                    .into_iter()
                    .filter(|&m| m < all && used & (1 << m) == 0)
                    .collect();
                if v.is_empty() && min > 0 { ai_pick } else { v }
            }
            _ => ai_pick,
        }
    }

    /// A card to discard (hand slot).
    pub(crate) fn h_discard(&self, p: usize) -> Option<usize> {
        let ai_pick = ai::discard_choice(self, p);
        if !self.is_human(p) || self.players[p].hand.is_empty() {
            return ai_pick;
        }
        let pl = &self.players[p];
        let options = pl
            .hand
            .iter()
            .map(|&c| pl.cards[c as usize].name.clone())
            .collect();
        match self.ask(
            Ask::Pick {
                prompt: "Discard a card".into(),
                options,
                min: 1,
                max: 1,
                suggested: ai_pick.into_iter().collect(),
            },
            false,
        ) {
            Answer::Pick(v) => v
                .first()
                .copied()
                .filter(|&h| h < pl.hand.len())
                .or(ai_pick),
            _ => ai_pick,
        }
    }

    /// A permanent player `o` sacrifices (it must meet `r`).
    pub(crate) fn h_sacrifice(&self, o: usize, types: u16, r: &Restr) -> Option<usize> {
        let ai_pick = ai::sacrifice_choice_r(self, o, types, r);
        if !self.is_human(o) {
            return ai_pick;
        }
        let d = self.dyn_values(r, None, o, None);
        let mut cands: Vec<usize> = (0..self.perms.len())
            .filter(|&i| {
                self.perms[i].ctrl as usize == o
                    && (self.perms[i].types & types != 0
                        || (types & T_CREATURE != 0 && self.perms[i].is_creature()))
                    && self.perm_ok(r, &self.perms[i], &d)
            })
            .collect();
        self.keep_greatest(r, &mut cands);
        self.pick_perm(&cands, "Sacrifice a permanent", ai_pick, false)
    }

    /// What to sacrifice for a cost paid as an ability resolves (None: don't).
    pub(crate) fn h_fodder(&self, p: usize, types: u16, src_id: u32) -> Option<usize> {
        let ai_pick = ai::fodder(self, p, types, src_id);
        if !self.is_human(p) {
            return ai_pick;
        }
        let cands: Vec<usize> = (0..self.perms.len())
            .filter(|&i| {
                let x = &self.perms[i];
                x.ctrl as usize == p
                    && x.id != src_id
                    && (x.types & types != 0 || (types & T_CREATURE != 0 && x.is_creature()))
            })
            .collect();
        self.pick_perm(
            &cands,
            "Sacrifice a permanent to pay the cost (or none)",
            ai_pick,
            true,
        )
    }

    fn pick_perm(
        &self,
        cands: &[usize],
        prompt: &str,
        ai_pick: Option<usize>,
        optional: bool,
    ) -> Option<usize> {
        if cands.is_empty() {
            return ai_pick;
        }
        let options = cands
            .iter()
            .map(|&i| self.perm_desc(&self.perms[i]))
            .collect();
        let suggested = ai_pick
            .and_then(|i| cands.iter().position(|&c| c == i))
            .into_iter()
            .collect();
        match self.ask(
            Ask::Pick {
                prompt: prompt.into(),
                options,
                min: if optional { 0 } else { 1 },
                max: 1,
                suggested,
            },
            false,
        ) {
            Answer::Pick(v) => match v.first() {
                Some(&k) => cands.get(k).copied().or(ai_pick),
                None if optional => None,
                None => ai_pick,
            },
            _ => ai_pick,
        }
    }

    /// "Target opponent sacrifices a creature": which opponent.
    pub(crate) fn h_edict_victim(&self, me: usize, r: &Restr) -> Option<usize> {
        let ai_pick = ai::edict_victim(self, me, r);
        if !self.is_human(me) {
            return ai_pick;
        }
        self.h_choose_opponent(me, 0).or(ai_pick)
    }

    /// A card from `p`'s library (library index), among `cands`.
    fn pick_library(
        &self,
        p: usize,
        cands: Vec<usize>,
        prompt: &str,
        ai_pick: Option<usize>,
    ) -> Option<usize> {
        if cands.is_empty() {
            return ai_pick;
        }
        let pl = &self.players[p];
        let options = cands
            .iter()
            .map(|&k| pl.cards[pl.library[k] as usize].name.clone())
            .collect();
        let suggested = ai_pick
            .and_then(|i| cands.iter().position(|&c| c == i))
            .into_iter()
            .collect();
        match self.ask(
            Ask::Pick {
                prompt: prompt.into(),
                options,
                min: 0,
                max: 1,
                suggested,
            },
            false,
        ) {
            Answer::Pick(v) => v.first().and_then(|&k| cands.get(k).copied()),
            _ => ai_pick,
        }
    }

    /// Searching the library ("search your library for a creature card").
    pub(crate) fn h_tutor(&self, p: usize, text: &str, r: &Restr) -> Option<usize> {
        let ai_pick = ai::tutor_choice(self, p, text, r);
        if !self.is_human(p) {
            return ai_pick;
        }
        let cands = ai::tutor_candidates(self, p, text, r);
        self.pick_library(
            p,
            cands,
            &format!("Search your library: \"{}\"", short(text)),
            ai_pick,
        )
    }

    /// A land to fetch from the library.
    pub(crate) fn h_land_to_fetch(&self, p: usize, basic_only: bool) -> Option<usize> {
        let ai_pick = ai::land_to_fetch(self, p, basic_only);
        if !self.is_human(p) {
            return ai_pick;
        }
        let pl = &self.players[p];
        let cands = (0..pl.library.len())
            .filter(|&k| {
                let c = &pl.cards[pl.library[k] as usize];
                c.is(T_LAND) && (!basic_only || c.is(T_BASIC))
            })
            .collect();
        self.pick_library(
            p,
            cands,
            if basic_only {
                "Search for a basic land"
            } else {
                "Search for a land"
            },
            ai_pick,
        )
    }

    /// A card back from the graveyard to the hand (graveyard slot).
    pub(crate) fn h_regrowth(
        &self,
        me: usize,
        creature: bool,
        r: &Restr,
        d: &Dyn,
    ) -> Option<usize> {
        let ai_pick = ai::regrowth_choice(self, me, creature, r, d);
        if !self.is_human(me) {
            return ai_pick;
        }
        let pl = &self.players[me];
        let cands: Vec<usize> = (0..pl.graveyard.len())
            .filter(|&k| !creature || pl.cards[pl.graveyard[k] as usize].is(T_CREATURE))
            .filter(|&k| Game::card_ok(r, &pl.cards[pl.graveyard[k] as usize], d))
            .collect();
        if cands.is_empty() {
            return ai_pick;
        }
        let options = cands
            .iter()
            .map(|&k| pl.cards[pl.graveyard[k] as usize].name.clone())
            .collect();
        let suggested = ai_pick
            .and_then(|i| cands.iter().position(|&c| c == i))
            .into_iter()
            .collect();
        match self.ask(
            Ask::Pick {
                prompt: "Return a card from your graveyard to your hand".into(),
                options,
                min: 1,
                max: 1,
                suggested,
            },
            false,
        ) {
            Answer::Pick(v) => v.first().and_then(|&k| cands.get(k).copied()).or(ai_pick),
            _ => ai_pick,
        }
    }

    /// A creature (or permanent) card to put onto the battlefield from a graveyard.
    pub(crate) fn h_reanimate(
        &self,
        me: usize,
        any: bool,
        r: &Restr,
        d: &Dyn,
    ) -> Option<(usize, usize)> {
        let ai_pick = ai::reanimate_choice(self, me, any, r, d);
        if !self.is_human(me) {
            return ai_pick;
        }
        let creature = r.any_types == 0 && r.not_types & T_LAND == 0;
        let mut cands: Vec<(usize, usize)> = Vec::new();
        for o in 0..self.players.len() {
            if !any && o != me {
                continue;
            }
            let pl = &self.players[o];
            for (k, &c) in pl.graveyard.iter().enumerate() {
                let card = &pl.cards[c as usize];
                let fits = if creature {
                    card.is(T_CREATURE)
                } else {
                    card.is_permanent()
                };
                if fits && Game::card_ok(r, card, d) {
                    cands.push((o, k));
                }
            }
        }
        if cands.is_empty() {
            return ai_pick;
        }
        let options = cands
            .iter()
            .map(|&(o, k)| {
                format!(
                    "{} (P{}'s graveyard)",
                    self.players[o].cards[self.players[o].graveyard[k] as usize].name, o
                )
            })
            .collect();
        let suggested = ai_pick
            .and_then(|x| cands.iter().position(|&c| c == x))
            .into_iter()
            .collect();
        match self.ask(
            Ask::Pick {
                prompt: "Choose a card to put onto the battlefield".into(),
                options,
                min: 1,
                max: 1,
                suggested,
            },
            false,
        ) {
            Answer::Pick(v) => v.first().and_then(|&k| cands.get(k).copied()).or(ai_pick),
            _ => ai_pick,
        }
    }

    /// Scry / surveil N: which cards stay on top (the rest go to the bottom / graveyard).
    pub(crate) fn h_scry(&mut self, p: usize, k: usize, surveil: bool) {
        if !self.is_human(p) {
            ai::scry(self, p, k, surveil);
            return;
        }
        let n = self.players[p].library.len();
        let k = k.min(n);
        if k == 0 {
            return;
        }
        // Top first.
        let top: Vec<u16> = self.players[p].library[n - k..]
            .iter()
            .rev()
            .copied()
            .collect();
        let options = top
            .iter()
            .map(|&c| self.players[p].cards[c as usize].name.clone())
            .collect();
        let keep = match self.ask(
            Ask::Pick {
                prompt: format!(
                    "{} {}: choose the cards to keep on top (the rest go {})",
                    if surveil { "Surveil" } else { "Scry" },
                    k,
                    if surveil {
                        "to your graveyard"
                    } else {
                        "to the bottom"
                    }
                ),
                options,
                min: 0,
                max: k,
                suggested: (0..k).collect(),
            },
            false,
        ) {
            Answer::Pick(v) => v,
            _ => {
                ai::scry(self, p, k, surveil);
                return;
            }
        };
        self.players[p].library.truncate(n - k);
        let mut away = Vec::new();
        let mut kept = Vec::new();
        for (j, &c) in top.iter().enumerate() {
            if keep.contains(&j) {
                kept.push(c);
            } else {
                away.push(c);
            }
        }
        for &c in kept.iter().rev() {
            self.players[p].library.push(c);
        }
        for c in away {
            if surveil {
                self.players[p].graveyard.push(c);
            } else {
                self.players[p].library.insert(0, c);
            }
        }
        self.players[p].scried_turn = true;
        self.emit(Ev::Scry, p, None);
    }

    /// Counter a spell with this counterspell? (The AI's verdict suggested.)
    pub(crate) fn h_counter(
        &self,
        o: usize,
        caster: usize,
        spell: &str,
        counter: &str,
        ai: bool,
    ) -> bool {
        match self.ask(
            Ask::YesNo {
                prompt: format!("P{} casts {}. Counter it with {}?", caster, spell, counter),
                suggested: ai,
            },
            false,
        ) {
            Answer::Yes(b) => b,
            _ => {
                let _ = o;
                ai
            }
        }
    }

    // ------------------------------------------------------------ sandbox edits

    pub fn apply_edit(&mut self, e: &Edit) {
        let what = format!("{:?}", e);
        match e {
            Edit::Tap { id } => {
                if let Some(i) = self.idx(*id) {
                    self.perms[i].tapped = !self.perms[i].tapped;
                }
            }
            Edit::SetLife { player, life } => {
                if let Some(pl) = self.players.get_mut(*player) {
                    pl.life = *life;
                }
            }
            Edit::Counters { id, plus, other } => {
                if let Some(i) = self.idx(*id) {
                    self.perms[i].counters += plus;
                    if self.perms[i].is(T_PLANESWALKER) {
                        self.perms[i].loyalty += other;
                    } else {
                        self.perms[i].other_counters += other;
                    }
                    self.recalc();
                }
            }
            Edit::Move { from, to } => self.edit_move(*from, *to),
        }
        self.note(|| format!("[manual edit] {}", what));
        self.run_pending();
        self.sba();
    }

    fn edit_move(&mut self, from: Loc, to: Dest) {
        // Take the card out of where it is: (owner, card).
        let card: Option<(usize, u16)> = match from {
            Loc::Perm(id) => {
                let Some(i) = self.idx(id) else { return };
                let x = &self.perms[i];
                let zone = match to {
                    Dest::Battlefield => return,
                    Dest::Hand => Zone::Hand,
                    Dest::Graveyard => Zone::Graveyard,
                    Dest::Exile => Zone::Exile,
                    Dest::LibraryTop => Zone::Library(true),
                    Dest::LibraryBottom => Zone::Library(false),
                };
                let _ = x;
                self.leave(i, zone, true);
                return;
            }
            Loc::Hand(p, k) => take(&mut self.players[p].hand, k).map(|c| (p, c)),
            Loc::Graveyard(p, k) => take(&mut self.players[p].graveyard, k).map(|c| (p, c)),
            Loc::Exile(p, k) => take(&mut self.players[p].exile, k).map(|c| (p, c)),
            Loc::LibraryTop(p) => self.players[p].library.pop().map(|c| (p, c)),
            Loc::Command(p, k) => {
                if k < self.players[p].command.len() {
                    Some((p, self.players[p].command.remove(k).0))
                } else {
                    None
                }
            }
        };
        let Some((p, c)) = card else { return };
        match to {
            Dest::Battlefield => {
                if let Some(i) = self.enter_card(p, c, p, false) {
                    self.perms[i].sick = false;
                }
            }
            Dest::Hand => self.players[p].hand.push(c),
            Dest::Graveyard => self.players[p].graveyard.push(c),
            Dest::Exile => self.players[p].exile.push(c),
            Dest::LibraryTop => self.players[p].library.push(c),
            Dest::LibraryBottom => self.players[p].library.insert(0, c),
        }
    }
}

fn take(v: &mut Vec<u16>, k: usize) -> Option<u16> {
    (k < v.len()).then(|| v.remove(k))
}

/// An ability's text, shortened for a button.
fn short(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() > 110 {
        format!("{}...", s.chars().take(107).collect::<String>())
    } else {
        s.to_string()
    }
}

// ------------------------------------------------------------ games and reports

/// Everything needed to play a game again: the seed, the options, every deck, and the
/// human's answers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Setup {
    pub seed: u64,
    pub seat: usize,
    pub opts: PlayOptions,
    pub rules: Rules,
    /// Deck lists (deck file text), seat by seat.
    pub decks: Vec<(String, String)>,
}

/// A bug report: the setup, the answers up to the bug, and what the player saw.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub note: String,
    pub setup: Setup,
    pub answers: Vec<Answer>,
}

/// Plays a game with the human seat answered by `io`: returns the winner and the answers
/// used. Unwinds with `Quit` if the player leaves.
pub fn play_with(
    setup: &Setup,
    db: &CardDb,
    io: Box<dyn HumanIo>,
) -> (usize, Vec<Answer>, Option<GameView>) {
    let decks: Vec<Deck> = setup
        .decks
        .iter()
        .map(|(name, text)| Deck::from_text(name, text, db))
        .collect();
    let prepared: Vec<Prepared> = decks.iter().map(|d| Prepared::new(d, db)).collect();
    let refs: Vec<&Prepared> = prepared.iter().collect();
    let link = HumanLink::new(setup.seat, setup.opts.clone(), io);
    let mut g = Game::new_human(&refs, &setup.rules, Rng::new(setup.seed), link);
    let w = super::play(&mut g);
    let view = g.human.as_ref().map(|_| g.view());
    let answers = g
        .human
        .as_ref()
        .map(|h| h.transcript.borrow().clone())
        .unwrap_or_default();
    if let Some(h) = &g.human {
        h.io.send(FromEngine::Over {
            winner: w,
            view: Box::new(view.clone().unwrap_or_default()),
            transcript: answers.clone(),
        });
    }
    (w, answers, view)
}

/// A game on its own thread, for the GUI.
pub struct PlayHandle {
    pub tx: Sender<ToEngine>,
    pub rx: Receiver<FromEngine>,
}

/// Starts a game: the engine thread owns the decks and the game.
pub fn start(setup: Setup, db: CardDb) -> PlayHandle {
    let (to_engine, from_gui) = std::sync::mpsc::channel::<ToEngine>();
    let (to_gui, from_engine) = std::sync::mpsc::channel::<FromEngine>();
    std::thread::spawn(move || {
        let fail = to_gui.clone();
        let io = ChannelIo {
            tx: to_gui,
            rx: from_gui,
        };
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            play_with(&setup, &db, Box::new(io));
        }));
        if let Err(e) = r {
            if e.downcast_ref::<Quit>().is_none() {
                let msg = e
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "the engine stopped".into());
                let _ = fail.send(FromEngine::Failed(msg));
            }
        }
    });
    PlayHandle {
        tx: to_engine,
        rx: from_engine,
    }
}

/// Replays a report's answers and stops where they end: the log and the game as it was at
/// the last question.
pub fn replay(report: &Report, db: &CardDb) -> (Vec<String>, Option<GameView>, Vec<Ask>) {
    let io = std::rc::Rc::new(ScriptIo {
        answers: RefCell::new(report.answers.clone().into()),
        stop_when_done: true,
        ..Default::default()
    });
    struct Shared(std::rc::Rc<ScriptIo>);
    impl HumanIo for Shared {
        fn send(&self, msg: FromEngine) {
            self.0.send(msg)
        }
        fn recv(&self) -> ToEngine {
            self.0.recv()
        }
    }
    let setup = report.setup.clone();
    let shared = Shared(io.clone());
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        play_with(&setup, db, Box::new(shared));
    }));
    let log = io.log.borrow().clone();
    let view = io.last_view.borrow().clone();
    let asks = io.asks.borrow().clone();
    (log, view, asks)
}

/// The text of a bug report file: the note, the state dump, the log, and (last, as JSON)
/// the setup and answers that replay it (`examples/replay_bug.rs`).
pub fn report_text(report: &Report, pending: Option<&Ask>, dump: &str, log: &[String]) -> String {
    let mut s = String::new();
    s += "MSH Deckbuilder - Play tab bug report\n\n== Note\n";
    s += &report.note;
    s += "\n\n== Pending question\n";
    s += &format!("{:?}\n", pending);
    s += &format!(
        "\n== Setup\nseed {}, human seat P{}, options {:?}, {} answers recorded\n",
        report.setup.seed,
        report.setup.seat,
        report.setup.opts,
        report.answers.len()
    );
    for (p, (name, text)) in report.setup.decks.iter().enumerate() {
        s += &format!("\n-- P{} deck: {}\n{}\n", p, name, text.trim());
    }
    s += "\n== State\n";
    s += dump;
    s += "\n== Log\n";
    for l in log {
        s += l;
        s += "\n";
    }
    s += "\n== Replay data (JSON)\n";
    s += &serde_json::to_string(report).unwrap_or_default();
    s += "\n";
    s
}

/// Reads the replay data back from a report file's text.
pub fn parse_report(text: &str) -> Option<Report> {
    let at = text.rfind("== Replay data (JSON)\n")?;
    let json = text[at..].lines().nth(1)?;
    serde_json::from_str(json).ok()
}
