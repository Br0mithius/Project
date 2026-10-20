//! A quick, simplified 4-player game simulator, so decks can be compared by playing them
//! rather than only by scoring their cards.
//!
//! Each card is reduced to what the simulator understands: lands and mana rocks, creature
//! bodies and combat keywords, removal (destroy/exile, damage, bounce, board wipes), card
//! draw, tokens, "each opponent loses life" drains, life gain, anthems and Equipment/Aura
//! bonuses, and whether those happen once or every turn. Counterspells, combat tricks and
//! most unusual abilities are ignored, so a card the simulator can't read plays as a body
//! (creatures) or does nothing (other spells). It's a rough second opinion, not a rules
//! engine.

use crate::abilities;
use crate::analysis::Rng;
use crate::card::Card;
use crate::db::{CardDb, Rules};
use crate::deck::Deck;

const FLYING: u16 = 1;
const REACH: u16 = 1 << 1;
const TRAMPLE: u16 = 1 << 2;
const MENACE: u16 = 1 << 3;
const DEATHTOUCH: u16 = 1 << 4;
const LIFELINK: u16 = 1 << 5;
const HASTE: u16 = 1 << 6;
const FIRST_STRIKE: u16 = 1 << 7;
const VIGILANCE: u16 = 1 << 8;
const INDESTRUCTIBLE: u16 = 1 << 9;
const HEXPROOF: u16 = 1 << 10;
const DOUBLE_STRIKE: u16 = 1 << 11;
const DEFENDER: u16 = 1 << 12;
const UNBLOCKABLE: u16 = 1 << 13;

const KEYWORDS: [(&str, u16); 14] = [
    ("flying", FLYING),
    ("reach", REACH),
    ("trample", TRAMPLE),
    ("menace", MENACE),
    ("deathtouch", DEATHTOUCH),
    ("lifelink", LIFELINK),
    ("haste", HASTE),
    ("first strike", FIRST_STRIKE),
    ("vigilance", VIGILANCE),
    ("indestructible", INDESTRUCTIBLE),
    ("hexproof", HEXPROOF),
    ("shroud", HEXPROOF),
    ("double strike", DOUBLE_STRIKE),
    ("defender", DEFENDER),
];

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kill {
    /// Destroy (indestructible creatures survive).
    Destroy,
    /// Exile, -X/-X to death, lock down or steal: indestructible doesn't help (CR 702.12b).
    Exile,
    /// Damage to one creature (kills toughness up to N).
    Damage(i32),
    /// Return a creature to its owner's hand.
    Bounce,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Wipe {
    /// None = destroy/exile everything; Some(n) = damage or -N/-N to each creature.
    damage: Option<i32>,
    /// Only opponents' creatures.
    one_sided: bool,
    /// Exile (or -X/-X): indestructible doesn't save them.
    exile: bool,
}

/// How often an effect happens: once (cast / enters), or every turn while it's in play.
#[derive(Debug, Clone, Copy, Default)]
struct Effect {
    now: f32,
    per_turn: f32,
    /// Each time the creature attacks.
    per_attack: f32,
}

impl Effect {
    fn add(&mut self, when: Timing, n: f32) {
        match when {
            Timing::Now(w) => self.now += n * w,
            Timing::PerTurn(r) => self.per_turn += n * r,
            Timing::PerAttack(r) => self.per_attack += n * r,
            Timing::Static | Timing::Never => {}
        }
    }
}

/// When a parsed ability does its thing, for the simulator (weights already applied).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Timing {
    Now(f32),
    PerTurn(f32),
    PerAttack(f32),
    Static,
    Never,
}

/// MSH Power-up (CR 702.193): an activated ability usable once, its cost reduced by the
/// permanent's mana cost on the turn it entered.
#[derive(Debug, Clone, Default)]
struct PowerUp {
    cost: u32,
    grow: i32,
    kill: Option<Kill>,
    wipe: Option<Wipe>,
    draw: f32,
    tokens: f32,
    kw: u16,
}

/// A card as the simulator sees it.
#[derive(Debug, Clone)]
pub struct Profile {
    mv: u32,
    pips: [u32; 5],
    land: bool,
    produces: u8,
    rock: bool,
    fetch: bool,
    creature: bool,
    permanent: bool,
    power: i32,
    toughness: i32,
    kw: u16,
    kill: Option<Kill>,
    wipe: Option<Wipe>,
    /// Removal it keeps doing (activated or triggered), per turn.
    kill_rate: f32,
    kill_repeat: Option<Kill>,
    /// Each opponent sacrifices a creature.
    edict: Effect,
    draw: Effect,
    tokens: Effect,
    token_size: (i32, i32),
    drain: Effect,
    gain: Effect,
    /// +1/+1 counters on itself.
    grow: Effect,
    anthem: i32,
    equip: (i32, i32),
    power_up: Option<PowerUp>,
}

fn keyword_bits(words: &[String]) -> u16 {
    let mut bits = 0;
    for w in words {
        if w.starts_with("ward") || w.starts_with("hexproof") {
            bits |= HEXPROOF;
        }
        if w == "unblockable" {
            bits |= UNBLOCKABLE;
        }
        for (k, bit) in KEYWORDS {
            if w == k {
                bits |= bit;
            }
        }
    }
    bits
}

/// The simulator's version of a removal effect, if it hits opponents' creatures.
fn kill_of(e: &abilities::Effect) -> Option<Result<Kill, Wipe>> {
    use abilities::Effect as E;
    let mass = |t: &abilities::Target, damage: Option<i32>, exile: bool| {
        Err(Wipe {
            damage,
            one_sided: t.whose == abilities::Whose::Theirs,
            exile,
        })
    };
    Some(match e {
        E::Destroy(t) if t.hits_their_creatures() => {
            if t.mass {
                mass(t, None, false)
            } else {
                Ok(Kill::Destroy)
            }
        }
        E::Exile { target: t, .. } if t.hits_their_creatures() => {
            if t.mass {
                mass(t, None, true)
            } else {
                Ok(Kill::Exile)
            }
        }
        E::Damage { amount, to } if to.hits_their_creatures() => {
            if to.mass {
                mass(to, Some(amount.n), false)
            } else {
                Ok(Kill::Damage(amount.n))
            }
        }
        E::Shrink { amount, to, .. } if to.hits_their_creatures() => {
            if to.mass {
                mass(to, Some(amount.n), true)
            } else {
                Ok(Kill::Damage(amount.n))
            }
        }
        E::Fight(t) if t.hits_their_creatures() => Ok(Kill::Damage(3)),
        E::Bounce(t) if t.hits_their_creatures() => {
            if t.mass {
                mass(t, None, true)
            } else {
                Ok(Kill::Bounce)
            }
        }
        E::Lock {
            target,
            lasting: true,
        } if target.hits_their_creatures() => Ok(Kill::Exile),
        E::GainControl(t) if t.hits_their_creatures() => Ok(Kill::Exile),
        _ => return None,
    })
}

fn kill_value(k: Kill) -> i32 {
    match k {
        Kill::Exile => 10,
        Kill::Destroy => 9,
        Kill::Damage(n) => n.min(8),
        Kill::Bounce => 1,
    }
}

impl Profile {
    pub fn from_card(card: &Card) -> Profile {
        Profile::in_deck(card, 6)
    }

    /// Reads a card (via the ability parser) for a deck; `equipment` = how many Equipment
    /// the deck has (for "if it's equipped" conditions).
    pub fn in_deck(card: &Card, equipment: u32) -> Profile {
        use abilities::{Effect as E, When};
        let parsed = card.parsed();
        let mut p = Profile {
            mv: card.mana_value(),
            pips: card.pips(),
            land: card.is_land(),
            produces: card.produces().0,
            rock: false,
            fetch: false,
            creature: card.is_creature(),
            permanent: !matches!(card.main_type(), "Instant" | "Sorcery"),
            power: card
                .power
                .as_deref()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            toughness: card
                .toughness
                .as_deref()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            kw: keyword_bits(&parsed.keywords),
            kill: None,
            wipe: None,
            kill_rate: 0.0,
            kill_repeat: None,
            edict: Effect::default(),
            draw: Effect::default(),
            tokens: Effect::default(),
            token_size: (1, 1),
            drain: Effect::default(),
            gain: Effect::default(),
            grow: Effect::default(),
            anthem: 0,
            equip: (0, 0),
            power_up: None,
        };
        if p.land {
            if p.produces == 0 {
                p.produces = 0b11111 & card.identity().0;
            }
            return p;
        }
        let loyalty_count = parsed
            .abilities
            .iter()
            .filter(|a| a.when.iter().any(|w| matches!(w, When::Loyalty(_))))
            .count()
            .max(1) as f32;
        // The front face, plus an Adventure / prepare spell on the back (cast once, extra value).
        let mut abilities: Vec<(&abilities::Ability, f32)> =
            parsed.abilities.iter().map(|a| (a, 1.0)).collect();
        if let Some(back) = &parsed.back {
            let castable = back
                .abilities
                .iter()
                .all(|a| a.when.iter().all(|w| *w == When::Spell));
            if castable && !back.abilities.is_empty() {
                abilities.extend(back.abilities.iter().map(|a| (a, 0.8)));
            }
        }
        for (a, base) in abilities {
            // Conditions: "if it's equipped" holds about as often as the deck can find and
            // attach Equipment (~6 makes it near-reliable); others about half the time.
            let cond = if !a.conditional {
                1.0
            } else if a.text.contains("equipped") {
                (equipment as f32 / 6.0).min(0.9)
            } else {
                0.5
            };
            let w = base * cond;
            let timing = match a.when.first() {
                _ if w == 0.0 => Timing::Never,
                Some(When::Spell | When::Enters | When::Chapter(1)) => Timing::Now(w),
                Some(When::Chapter(_)) => Timing::Now(0.8 * w),
                Some(When::Reflexive) => Timing::Now(0.5 * w),
                Some(When::Dies) => Timing::Now(0.4 * w),
                Some(When::EachTurn) => Timing::PerTurn(w),
                Some(When::Event(_)) => Timing::PerTurn(a.rate() * w),
                Some(When::Attacks) => Timing::PerAttack(w),
                Some(When::CombatDamage) => Timing::PerAttack(0.6 * w),
                Some(When::Activated(c)) => {
                    if a.rate() > 0.0 {
                        // Mana-costed abilities compete with casting spells for mana.
                        let share = if c.mana > 0 { 0.5 } else { 1.0 };
                        Timing::PerTurn(a.rate() * share * w)
                    } else if c.sacrifice {
                        Timing::Now(0.4 * w)
                    } else {
                        Timing::Never
                    }
                }
                Some(When::Loyalty(n)) => {
                    if *n >= 0 {
                        Timing::PerTurn(w / loyalty_count)
                    } else {
                        Timing::Now(0.5 * w)
                    }
                }
                Some(When::PowerUp(_)) => Timing::Never, // handled below
                // An Aura's "enchanted creature can't attack or block" works from the
                // moment it's cast - it's removal.
                Some(When::Static)
                    if card.type_line.contains("Aura")
                        && a.text.starts_with("enchanted creature")
                        && a.effects.iter().any(|e| kill_of(e).is_some()) =>
                {
                    Timing::Now(w)
                }
                Some(When::Static) | None => Timing::Static,
            };
            // Mana abilities: {T}: Add... on a permanent is a mana rock (or mana creature).
            if matches!(a.when.first(), Some(When::Activated(c)) if c.tap && c.mana == 0 && !c.sacrifice)
                && a.effects.iter().any(|e| matches!(e, E::AddMana(_)))
            {
                p.rock = true;
                continue;
            }
            if let Some(When::PowerUp(c)) = a.when.first() {
                let mut pu = PowerUp {
                    cost: c.mana,
                    ..Default::default()
                };
                for e in &a.effects {
                    match e {
                        E::PlusCounters { amount, to } if to.kind == abilities::Kind::It => {
                            pu.grow += amount.n
                        }
                        E::Draw { amount, you: true } => pu.draw += amount.value(),
                        E::Tokens {
                            amount,
                            power,
                            toughness,
                            ..
                        } => {
                            pu.tokens += amount.value();
                            p.token_size = (*power, *toughness);
                        }
                        E::Grant { keywords, to, .. } if to.kind == abilities::Kind::It => {
                            pu.kw |= keyword_bits(keywords)
                        }
                        _ => match kill_of(e) {
                            Some(Ok(k)) => pu.kill = Some(k),
                            Some(Err(wipe)) => pu.wipe = Some(wipe),
                            None => {}
                        },
                    }
                }
                p.power_up = Some(pu);
                continue;
            }
            for e in &a.effects {
                match e {
                    E::Draw { amount, you: true } => p.draw.add(timing, amount.value()),
                    E::Dig => p.draw.add(timing, 0.7),
                    E::Empower(n) => p.draw.add(timing, n.value() / 3.0),
                    E::ItemTokens { kind, .. } if kind == "clue" => p.draw.add(timing, 0.5),
                    E::Tokens {
                        amount,
                        power,
                        toughness,
                        ..
                    } => {
                        p.tokens.add(timing, amount.value());
                        p.token_size = (*power, *toughness);
                    }
                    E::Amass(n) => {
                        p.tokens.add(timing, 1.0);
                        p.token_size = (n.n.max(1), n.n.max(1));
                    }
                    E::PlusCounters { amount, to } if to.kind == abilities::Kind::It => {
                        match timing {
                            Timing::Now(w) if p.creature => {
                                p.power += (amount.n as f32 * w).round() as i32;
                                p.toughness += (amount.n as f32 * w).round() as i32;
                            }
                            _ => p.grow.add(timing, amount.value()),
                        }
                    }
                    E::Drain { amount, each } => p
                        .drain
                        .add(timing, amount.value() * if *each { 1.0 } else { 0.4 }),
                    E::GainLife(n) => p.gain.add(timing, n.value()),
                    E::Edict { .. } => p.edict.add(timing, 1.0),
                    E::FetchLand { battlefield: true } if matches!(timing, Timing::Now(_)) => {
                        p.fetch = true
                    }
                    E::Pump {
                        power,
                        toughness,
                        to,
                        until_eot: false,
                    } if timing == Timing::Static => {
                        if to.mass && to.whose == abilities::Whose::Yours {
                            p.anthem += power;
                        } else if a.text.starts_with("equipped") || a.text.starts_with("enchanted")
                        {
                            p.equip = (p.equip.0 + power, p.equip.1 + toughness);
                        }
                    }
                    E::Grant {
                        keywords,
                        to,
                        until_eot: false,
                    } if timing == Timing::Static && to.kind == abilities::Kind::It => {
                        p.kw |= keyword_bits(keywords);
                    }
                    _ => match (kill_of(e), timing) {
                        (Some(Ok(k)), Timing::Now(_)) => {
                            if p.kill.is_none_or(|old| kill_value(k) > kill_value(old)) {
                                p.kill = Some(k);
                            }
                        }
                        (Some(Err(wipe)), Timing::Now(_)) => p.wipe = Some(wipe),
                        (Some(Ok(k)), Timing::PerTurn(r) | Timing::PerAttack(r)) => {
                            p.kill_rate += r;
                            p.kill_repeat = Some(k);
                        }
                        _ => {}
                    },
                }
            }
        }
        p
    }
}

#[derive(Debug, Clone)]
struct Perm {
    card: usize,
    power: i32,
    toughness: i32,
    kw: u16,
    sick: bool,
    tapped: bool,
    token: bool,
    commander: bool,
    damage: i32,
    /// Its power-up has been used.
    powered: bool,
    /// Round it entered (power-up is cheaper that round).
    born: u32,
}

/// `Perm::card` for creature tokens (they have no card behind them).
const TOKEN: usize = usize::MAX;

/// A deck read once into simulator profiles, ready to play many games.
pub struct Prepared {
    cards: Vec<Profile>,
    library: Vec<usize>,
    commander: Option<usize>,
    /// The Prismatic Piper (a second creature in the command zone; starts in hand).
    partner: Option<usize>,
}

impl Prepared {
    pub fn new(deck: &Deck, db: &CardDb) -> Prepared {
        let equipment: u32 = deck
            .cards
            .iter()
            .filter(|(n, _)| db.get(n).is_some_and(|c| c.type_line.contains("Equipment")))
            .map(|(_, k)| *k)
            .sum();
        let mut cards = Vec::new();
        let mut library = Vec::new();
        for (name, n) in &deck.cards {
            if let Some(c) = db.get(name) {
                cards.push(Profile::in_deck(c, equipment));
                for _ in 0..*n {
                    library.push(cards.len() - 1);
                }
            }
        }
        let mut extra = |name: &Option<String>| {
            name.as_ref().and_then(|n| db.get(n)).map(|c| {
                cards.push(Profile::in_deck(c, equipment));
                cards.len() - 1
            })
        };
        let commander = extra(&deck.commander);
        let partner = extra(&deck.partner);
        Prepared {
            cards,
            library,
            commander,
            partner,
        }
    }
}

struct Player<'a> {
    cards: &'a [Profile],
    /// +N/+N from anthems in play.
    anthem: i32,
    library: Vec<usize>,
    hand: Vec<usize>,
    commander: Option<usize>,
    commander_home: bool,
    commander_casts: u32,
    lands: u32,
    rocks: u32,
    sources: [u32; 5],
    board: Vec<Perm>,
    life: i32,
    alive: bool,
    commander_damage: [i32; 4],
}

impl<'a> Player<'a> {
    fn new(prep: &'a Prepared, life: i32, rng: &mut Rng) -> Player<'a> {
        let mut library = prep.library.clone();
        rng.shuffle(&mut library);
        let mut p = Player {
            cards: &prep.cards,
            anthem: 0,
            library,
            hand: prep.partner.into_iter().collect(),
            commander: prep.commander,
            commander_home: prep.commander.is_some(),
            commander_casts: 0,
            lands: 0,
            rocks: 0,
            sources: [0; 5],
            board: Vec::new(),
            life,
            alive: true,
            commander_damage: [0; 4],
        };
        // One mulligan for hands with fewer than 2 or more than 5 lands.
        let start = p.hand.len();
        p.draw(7);
        let lands = p.hand[start..].iter().filter(|&&c| p.cards[c].land).count();
        if !(2..=5).contains(&lands) {
            let back: Vec<usize> = p.hand.drain(start..).collect();
            p.library.extend(back);
            rng.shuffle(&mut p.library);
            p.draw(7);
        }
        p
    }

    fn draw(&mut self, n: u32) {
        for _ in 0..n {
            match self.library.pop() {
                Some(c) => self.hand.push(c),
                None => self.alive = false,
            }
        }
    }

    fn mana(&self) -> u32 {
        self.lands + self.rocks
    }

    fn can_cast(&self, c: usize, mana: u32, extra: u32) -> bool {
        let p = &self.cards[c];
        p.mv + extra <= mana && (0..5).all(|i| p.pips[i] == 0 || self.sources[i] >= p.pips[i])
    }

    fn board_power(&self) -> i32 {
        self.board.iter().map(|x| x.power.max(0)).sum()
    }

    fn add_sources(&mut self, produces: u8) {
        for i in 0..5 {
            if produces & (1 << i) != 0 {
                self.sources[i] += 1;
            }
        }
    }
}

/// The opponent creature most worth removing that `kill` can deal with.
fn best_target(players: &[Player<'_>], me: usize, kill: Kill) -> Option<(usize, usize)> {
    let mut best: Option<(i32, usize, usize)> = None;
    for (pi, p) in players.iter().enumerate() {
        if pi == me || !p.alive {
            continue;
        }
        for (ci, c) in p.board.iter().enumerate() {
            if c.kw & HEXPROOF != 0 {
                continue;
            }
            if c.toughness == 0 && c.power == 0 {
                continue; // engine placeholders, not creatures
            }
            let ok = match kill {
                Kill::Destroy => c.kw & INDESTRUCTIBLE == 0,
                Kill::Exile | Kill::Bounce => true,
                Kill::Damage(n) => c.toughness - c.damage <= n && c.kw & INDESTRUCTIBLE == 0,
            };
            if !ok {
                continue;
            }
            let evasive = c.kw & (FLYING | UNBLOCKABLE | MENACE | TRAMPLE) != 0;
            let threat = c.power + if c.commander { 3 } else { 0 } + if evasive { 1 } else { 0 }
                - if c.token { 1 } else { 0 };
            if threat >= 2 && best.is_none_or(|b| threat > b.0) {
                best = Some((threat, pi, ci));
            }
        }
    }
    best.map(|(_, p, c)| (p, c))
}

/// Takes a creature off the battlefield: commanders go home, tokens vanish, bounced cards
/// return to hand, others are gone.
fn remove(players: &mut [Player<'_>], pi: usize, ci: usize, to_hand: bool) {
    let perm = players[pi].board.remove(ci);
    if perm.commander {
        players[pi].commander_home = true;
    } else if to_hand && !perm.token {
        players[pi].hand.push(perm.card);
    }
}

fn kill_one(players: &mut [Player<'_>], me: usize, kill: Kill) {
    if let Some((pi, ci)) = best_target(players, me, kill) {
        remove(players, pi, ci, kill == Kill::Bounce);
    }
}

fn wipe_board(players: &mut [Player<'_>], me: usize, wipe: Wipe) {
    for (pi, p) in players.iter_mut().enumerate() {
        if wipe.one_sided && pi == me {
            continue;
        }
        let mut i = 0;
        while i < p.board.len() {
            let c = &p.board[i];
            let placeholder = c.toughness == 0 && c.power == 0;
            let survives_destroy = c.kw & INDESTRUCTIBLE != 0 && !wipe.exile;
            let dies = !placeholder
                && !survives_destroy
                && match wipe.damage {
                    None => true,
                    Some(n) => c.toughness - c.damage <= n,
                };
            if dies {
                let perm = p.board.remove(i);
                if perm.commander {
                    p.commander_home = true;
                }
            } else {
                i += 1;
            }
        }
    }
}

/// Each opponent sacrifices their weakest creature (edicts ignore hexproof, CR 701.21a).
fn edict(players: &mut [Player<'_>], me: usize) {
    for pi in 0..players.len() {
        if pi == me || !players[pi].alive {
            continue;
        }
        let weakest = players[pi]
            .board
            .iter()
            .enumerate()
            .filter(|(_, c)| !(c.toughness == 0 && c.power == 0))
            .min_by_key(|(_, c)| c.power + c.toughness)
            .map(|(i, _)| i);
        if let Some(i) = weakest {
            remove(players, pi, i, false);
        }
    }
}

fn enter(players: &mut [Player<'_>], me: usize, c: usize, commander: bool, turn: u32) {
    let cards = players[me].cards;
    let prof = &cards[c];
    // One-shot effects.
    if let Some(kill) = prof.kill {
        kill_one(players, me, kill);
    }
    if let Some(wipe) = prof.wipe {
        wipe_board(players, me, wipe);
    }
    if prof.edict.now >= 0.5 {
        edict(players, me);
    }
    apply_effects(
        players,
        me,
        prof.draw.now,
        prof.tokens.now,
        prof.token_size,
        prof.drain.now,
        prof.gain.now,
    );
    if prof.creature {
        let anthem = players[me].anthem;
        players[me].board.push(Perm {
            card: c,
            power: prof.power + anthem,
            toughness: prof.toughness + anthem,
            kw: prof.kw,
            sick: prof.kw & HASTE == 0,
            tapped: false,
            token: false,
            commander,
            damage: 0,
            powered: false,
            born: turn,
        });
    } else if prof.permanent && (prof.anthem != 0 || prof.equip != (0, 0)) {
        let p = &mut players[me];
        if prof.anthem != 0 {
            p.anthem += prof.anthem;
            for x in p.board.iter_mut().filter(|x| x.card != c) {
                x.power += prof.anthem;
                x.toughness += prof.anthem;
            }
        }
        if prof.equip != (0, 0) {
            if let Some(best) = p.board.iter_mut().max_by_key(|x| x.power) {
                best.power += prof.equip.0;
                best.toughness += prof.equip.1;
            }
        }
    }
    if prof.permanent
        && !prof.creature
        && (prof.draw.per_turn > 0.0
            || prof.tokens.per_turn > 0.0
            || prof.drain.per_turn > 0.0
            || prof.kill_rate > 0.0
            || prof.edict.per_turn > 0.0)
    {
        // Non-creature engines: remember them as zero-power "permanents".
        players[me].board.push(Perm {
            card: c,
            power: 0,
            toughness: 0,
            kw: INDESTRUCTIBLE | HEXPROOF | DEFENDER,
            sick: false,
            tapped: false,
            token: true,
            commander: false,
            damage: 0,
            powered: false,
            born: 0,
        });
    }
}

fn chance(rng: &mut Rng, rate: f32) -> u32 {
    let whole = rate.floor() as u32;
    let frac = rate - rate.floor();
    whole + u32::from((rng.next() % 1000) as f32 / 1000.0 < frac)
}

fn apply_effects(
    players: &mut [Player<'_>],
    me: usize,
    draw: f32,
    tokens: f32,
    token_size: (i32, i32),
    drain: f32,
    gain: f32,
) {
    players[me].draw(draw.round() as u32);
    for _ in 0..tokens.round() as u32 {
        let anthem = players[me].anthem;
        players[me].board.push(Perm {
            card: TOKEN,
            power: token_size.0 + anthem,
            toughness: token_size.1 + anthem,
            kw: 0,
            sick: true,
            tapped: false,
            token: true,
            commander: false,
            damage: 0,
            powered: false,
            born: 0,
        });
    }
    let drain = drain.round() as i32;
    if drain > 0 {
        for (i, p) in players.iter_mut().enumerate() {
            if i != me && p.alive {
                p.life -= drain;
            }
        }
    }
    players[me].life += gain.round() as i32;
}

/// Upkeep: every-turn effects of the permanents in play.
fn upkeep(players: &mut [Player<'_>], me: usize, rng: &mut Rng) {
    let cards = players[me].cards;
    let engines: Vec<usize> = players[me]
        .board
        .iter()
        .map(|x| x.card)
        .filter(|&c| c != TOKEN)
        .collect();
    for c in engines {
        let prof = &cards[c];
        if let Some(k) = prof.kill_repeat {
            if chance(rng, prof.kill_rate.min(1.0)) > 0 {
                kill_one(players, me, k);
            }
        }
        if chance(rng, prof.edict.per_turn.min(1.0)) > 0 {
            edict(players, me);
        }
        let grow = chance(rng, prof.grow.per_turn) as i32;
        if grow > 0 {
            if let Some(x) = players[me].board.iter_mut().find(|x| x.card == c) {
                x.power += grow;
                x.toughness += grow;
            }
        }
        if prof.draw.per_turn + prof.tokens.per_turn + prof.drain.per_turn + prof.gain.per_turn
            == 0.0
        {
            continue;
        }
        let draw = chance(rng, prof.draw.per_turn) as f32;
        let tokens = chance(rng, prof.tokens.per_turn) as f32;
        let drain = chance(rng, prof.drain.per_turn) as f32;
        let gain = chance(rng, prof.gain.per_turn) as f32;
        apply_effects(players, me, draw, tokens, prof.token_size, drain, gain);
    }
}

/// How much the active player wants to cast a card now (higher first; None = hold it).
fn priority(players: &[Player<'_>], me: usize, c: usize, turn: u32) -> Option<f32> {
    let p = &players[me];
    let prof = &p.cards[c];
    if let Some(kill) = prof.kill {
        let target = best_target(players, me, kill).is_some();
        if !target && !prof.creature {
            return None;
        }
        if target {
            return Some(10.0 + prof.mv as f32);
        }
    }
    if let Some(wipe) = prof.wipe {
        let theirs: i32 = players
            .iter()
            .enumerate()
            .filter(|(i, q)| *i != me && q.alive)
            .map(|(_, q)| q.board_power())
            .sum::<i32>()
            / 2;
        let mine = if wipe.one_sided { 0 } else { p.board_power() };
        // A damage wipe that can't kill much isn't worth holding for.
        let floor = if wipe.one_sided {
            2
        } else if matches!(wipe.damage, Some(n) if n <= 2) {
            3
        } else {
            5
        };
        return (theirs - mine >= floor).then_some(12.0);
    }
    if prof.rock || prof.fetch {
        return Some(if turn <= 5 { 9.0 } else { 2.0 });
    }
    let mut v = prof.mv as f32;
    if prof.creature {
        v += 1.0;
    }
    if prof.draw.now > 0.0 || prof.draw.per_turn > 0.0 {
        v += 0.5;
    }
    Some(v)
}

fn main_phase(players: &mut [Player<'_>], me: usize, turn: u32) {
    let mut mana = players[me].mana();
    loop {
        if !players[me].alive {
            return;
        }
        let tax = 2 * players[me].commander_casts;
        let mut best: Option<(f32, Option<usize>, usize)> = None; // (priority, hand index, card)
        if players[me].commander_home {
            if let Some(cmd) = players[me].commander {
                if players[me].can_cast(cmd, mana, tax) {
                    best = Some((players[me].cards[cmd].mv as f32 + 3.0, None, cmd));
                }
            }
        }
        for (hi, &c) in players[me].hand.iter().enumerate() {
            if players[me].cards[c].land || !players[me].can_cast(c, mana, 0) {
                continue;
            }
            if let Some(pr) = priority(players, me, c, turn) {
                if best.is_none_or(|b| pr > b.0) {
                    best = Some((pr, Some(hi), c));
                }
            }
        }
        let Some((_, hand_index, c)) = best else {
            power_ups(players, me, mana, turn);
            return;
        };
        let cost = players[me].cards[c].mv;
        let commander = hand_index.is_none();
        match hand_index {
            Some(hi) => {
                players[me].hand.remove(hi);
                mana -= cost;
            }
            None => {
                players[me].commander_home = false;
                mana -= cost + tax;
                players[me].commander_casts += 1;
            }
        }
        let cards = players[me].cards;
        let prof = &cards[c];
        if prof.rock {
            players[me].rocks += 1;
            players[me].add_sources(prof.produces);
        }
        if prof.fetch {
            players[me].lands += 1;
            players[me].sources = players[me].sources.map(|s| s + 1);
        }
        enter(players, me, c, commander, turn);
    }
}

/// Spends leftover mana on power-up abilities (CR 702.193: once each, cheaper by the
/// permanent's mana cost on the turn it entered).
fn power_ups(players: &mut [Player<'_>], me: usize, mut mana: u32, turn: u32) {
    let cards = players[me].cards;
    loop {
        let pick = players[me]
            .board
            .iter()
            .enumerate()
            .filter(|(_, x)| x.card != TOKEN && !x.powered)
            .filter_map(|(i, x)| {
                let prof = &cards[x.card];
                let pu = prof.power_up.as_ref()?;
                let cost = if x.born == turn {
                    pu.cost.saturating_sub(prof.mv)
                } else {
                    pu.cost
                };
                (cost <= mana).then_some((i, cost))
            })
            .next();
        let Some((i, cost)) = pick else { return };
        mana -= cost;
        players[me].board[i].powered = true;
        let pu = cards[players[me].board[i].card]
            .power_up
            .clone()
            .unwrap_or_default();
        {
            let x = &mut players[me].board[i];
            x.power += pu.grow;
            x.toughness += pu.grow;
            x.kw |= pu.kw;
        }
        if let Some(k) = pu.kill {
            kill_one(players, me, k);
        }
        if let Some(w) = pu.wipe {
            wipe_board(players, me, w);
        }
        let size = cards[players[me]
            .board
            .get(i)
            .map(|x| x.card)
            .unwrap_or(0)
            .min(cards.len() - 1)]
        .token_size;
        apply_effects(players, me, pu.draw, pu.tokens, size, 0.0, 0.0);
    }
}

fn play_land(p: &mut Player<'_>) {
    // The land that adds the colour we have fewest sources of.
    let mut best: Option<(i32, usize)> = None;
    for (hi, &c) in p.hand.iter().enumerate() {
        let prof = &p.cards[c];
        if !prof.land {
            continue;
        }
        let need: i32 = (0..5)
            .filter(|&i| prof.produces & (1 << i) != 0)
            .map(|i| 10 - p.sources[i] as i32)
            .max()
            .unwrap_or(0);
        if best.is_none_or(|b| need > b.0) {
            best = Some((need, hi));
        }
    }
    if let Some((_, hi)) = best {
        let c = p.hand.remove(hi);
        p.lands += 1;
        let produces = p.cards[c].produces;
        p.add_sources(produces);
    }
}

fn can_block(blocker: &Perm, attacker: &Perm) -> bool {
    if blocker.tapped || blocker.power == 0 && blocker.toughness == 0 {
        return false;
    }
    if attacker.kw & UNBLOCKABLE != 0 {
        return false;
    }
    if attacker.kw & FLYING != 0 && blocker.kw & (FLYING | REACH) == 0 {
        return false;
    }
    true
}

fn kills(source: &Perm, target: &Perm) -> bool {
    target.kw & INDESTRUCTIBLE == 0
        && source.power > 0
        && (source.kw & DEATHTOUCH != 0 || source.power >= target.toughness - target.damage)
}

fn combat(players: &mut [Player<'_>], me: usize, rng: &mut Rng) {
    // Attack the opponent that's easiest to hurt: low life, few untapped blockers.
    let target = players
        .iter()
        .enumerate()
        .filter(|(i, p)| *i != me && p.alive)
        .min_by_key(|(_, p)| {
            p.life
                + p.board
                    .iter()
                    .filter(|b| !b.tapped)
                    .map(|b| b.toughness.min(6))
                    .sum::<i32>()
        })
        .map(|(i, _)| i);
    let Some(t) = target else { return };
    let my_power = players[me].board_power();
    let their_power = players[t].board_power();
    let alpha = my_power >= players[t].life || my_power >= 2 * their_power + 6;
    let mut attackers: Vec<usize> = Vec::new();
    for (i, a) in players[me].board.iter().enumerate() {
        if a.sick || a.tapped || a.power <= 0 || a.kw & DEFENDER != 0 {
            continue;
        }
        let blockers: Vec<&Perm> = players[t]
            .board
            .iter()
            .filter(|b| can_block(b, a))
            .collect();
        let menace_safe = a.kw & MENACE != 0 && blockers.len() < 2;
        let safe = blockers.iter().all(|b| !kills(b, a)) || menace_safe;
        if alpha || safe || blockers.is_empty() {
            attackers.push(i);
        }
    }
    if attackers.is_empty() {
        return;
    }
    // Biggest attackers are blocked first.
    attackers.sort_by_key(|&i| -players[me].board[i].power);
    // "Whenever this attacks" triggers.
    let cards = players[me].cards;
    let triggers: Vec<usize> = attackers
        .iter()
        .map(|&i| players[me].board[i].card)
        .filter(|&c| c != TOKEN)
        .filter(|&c| {
            let e = &cards[c];
            e.draw.per_attack
                + e.tokens.per_attack
                + e.drain.per_attack
                + e.gain.per_attack
                + e.grow.per_attack
                > 0.0
        })
        .collect();
    for &i in &attackers {
        let c = players[me].board[i].card;
        if c != TOKEN {
            let g = cards[c].grow.per_attack.round() as i32;
            players[me].board[i].power += g;
            players[me].board[i].toughness += g;
        }
    }
    let incoming: i32 = attackers.iter().map(|&i| players[me].board[i].power).sum();
    let lethal = incoming >= players[t].life;
    let mut used = vec![false; players[t].board.len()];
    let mut dead_attackers = Vec::new();
    let mut dead_blockers = Vec::new();
    let mut damage_to_player = 0;
    let mut commander_damage = 0;
    let mut lifelink = 0;
    for &ai in &attackers {
        let a = players[me].board[ai].clone();
        if a.kw & VIGILANCE == 0 {
            players[me].board[ai].tapped = true;
        }
        let candidates: Vec<usize> = (0..players[t].board.len())
            .filter(|&bi| !used[bi] && can_block(&players[t].board[bi], &a))
            .collect();
        let menace_ok = a.kw & MENACE == 0 || candidates.len() >= 2;
        let pick = if !menace_ok {
            None
        } else {
            let b = |bi: usize| &players[t].board[bi];
            // Kill it and live; else trade evenly or better; else chump if it's lethal.
            candidates
                .iter()
                .copied()
                .filter(|&bi| kills(b(bi), &a) && !kills(&a, b(bi)))
                .min_by_key(|&bi| b(bi).power)
                .or_else(|| {
                    candidates
                        .iter()
                        .copied()
                        .filter(|&bi| {
                            kills(b(bi), &a)
                                && b(bi).power + b(bi).toughness <= a.power + a.toughness + 1
                        })
                        .min_by_key(|&bi| b(bi).power + b(bi).toughness)
                })
                .or_else(|| {
                    candidates
                        .iter()
                        .copied()
                        .filter(|&bi| !kills(&a, b(bi)))
                        .min_by_key(|&bi| b(bi).power)
                })
                .or_else(|| {
                    if lethal {
                        candidates
                            .iter()
                            .copied()
                            .min_by_key(|&bi| b(bi).power + b(bi).toughness)
                    } else {
                        None
                    }
                })
        };
        let strikes = if a.kw & DOUBLE_STRIKE != 0 { 2 } else { 1 };
        match pick {
            None => {
                let dmg = a.power * strikes;
                damage_to_player += dmg;
                if a.commander {
                    commander_damage += dmg;
                }
                if a.kw & LIFELINK != 0 {
                    lifelink += dmg;
                }
            }
            Some(bi) => {
                used[bi] = true;
                let b = players[t].board[bi].clone();
                let a_first = a.kw & (FIRST_STRIKE | DOUBLE_STRIKE) != 0
                    && b.kw & (FIRST_STRIKE | DOUBLE_STRIKE) == 0;
                let b_first = b.kw & (FIRST_STRIKE | DOUBLE_STRIKE) != 0
                    && a.kw & (FIRST_STRIKE | DOUBLE_STRIKE) == 0;
                let a_kills = kills(&a, &b);
                let b_kills = kills(&b, &a);
                let blocker_dies = a_kills && !b_first;
                let attacker_dies = b_kills && !(a_first && a_kills);
                if blocker_dies {
                    dead_blockers.push(bi);
                    if a.kw & TRAMPLE != 0 {
                        damage_to_player += (a.power - (b.toughness - b.damage)).max(0);
                    }
                }
                if attacker_dies {
                    dead_attackers.push(ai);
                }
                if a.kw & LIFELINK != 0 {
                    lifelink += a.power;
                }
            }
        }
    }
    players[t].life -= damage_to_player;
    for c in triggers {
        let e = &cards[c];
        let draw = chance(rng, e.draw.per_attack) as f32;
        let tokens = chance(rng, e.tokens.per_attack) as f32;
        let drain = chance(rng, e.drain.per_attack) as f32;
        let gain = chance(rng, e.gain.per_attack) as f32;
        apply_effects(players, me, draw, tokens, e.token_size, drain, gain);
    }
    players[me].life += lifelink;
    players[t].commander_damage[me] += commander_damage;
    dead_blockers.sort_unstable();
    for bi in dead_blockers.into_iter().rev() {
        remove(players, t, bi, false);
    }
    dead_attackers.sort_unstable();
    for ai in dead_attackers.into_iter().rev() {
        remove(players, me, ai, false);
    }
}

fn check_deaths(players: &mut [Player<'_>]) {
    for p in players.iter_mut() {
        if p.life <= 0 || p.commander_damage.iter().any(|&d| d >= 21) {
            p.alive = false;
        }
        if !p.alive {
            p.board.clear();
        }
    }
}

/// Plays one game; returns the winner's seat (by most life if nobody wins in 25 rounds).
pub fn play_game(decks: &[&Prepared], rules: &Rules, rng: &mut Rng) -> usize {
    let mut players: Vec<Player> = decks
        .iter()
        .map(|d| Player::new(d, rules.starting_life as i32, rng))
        .collect();
    let n = players.len();
    for round in 1..=25u32 {
        for me in 0..n {
            if !players[me].alive {
                continue;
            }
            for x in players[me].board.iter_mut() {
                x.tapped = false;
                x.sick = false;
                x.damage = 0;
            }
            upkeep(&mut players, me, rng);
            if !(round == 1 && me == 0) {
                players[me].draw(1);
            }
            play_land(&mut players[me]);
            main_phase(&mut players, me, round);
            check_deaths(&mut players);
            if players[me].alive {
                combat(&mut players, me, rng);
            }
            check_deaths(&mut players);
            let alive: Vec<usize> = (0..n).filter(|&i| players[i].alive).collect();
            match alive.len() {
                0 => return me,
                1 => return alive[0],
                _ => {}
            }
        }
    }
    (0..n)
        .max_by_key(|&i| {
            if players[i].alive {
                players[i].life
            } else {
                i32::MIN
            }
        })
        .unwrap_or(0)
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

/// Opponent decks for playtesting: one per other player in the league, auto-built from a
/// simulated pool of the packs that player has by this point of the season (their own
/// Marvel / Hobbit mix from `rules.roster`).
pub fn gauntlet(db: &CardDb, rules: &Rules, seed: u64) -> Vec<Prepared> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::new();
    for player in &rules.roster.others {
        // A pool with no legendary creature can't make a deck; reopen (rare).
        for _ in 0..4 {
            let pool = crate::packs::simulated_pool(db, player, rules.session.max(1), &mut rng);
            let best = db
                .cards
                .iter()
                .filter(|c| {
                    c.can_be_commander() && c.name != crate::deck::PIPER && pool.owned(&c.name) > 0
                })
                .map(|c| {
                    let colors = crate::analysis::best_colors(c, db, &pool, rules);
                    (
                        crate::analysis::pool_quality_for(c, colors, db, &pool, rules),
                        c,
                    )
                })
                .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            if let Some((_, cmd)) = best {
                let deck = crate::analysis::autobuild(cmd, db, &pool, rules, None, None).0;
                out.push(Prepared::new(&deck, db));
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(name: &str, cost: &str, ty: &str, text: &str, pt: Option<(&str, &str)>) -> Card {
        Card {
            name: name.into(),
            mana_cost: cost.into(),
            type_line: ty.into(),
            oracle_text: text.into(),
            power: pt.map(|x| x.0.into()),
            toughness: pt.map(|x| x.1.into()),
            ..Default::default()
        }
    }

    #[test]
    fn reads_cards() {
        let p = Profile::from_card(&card(
            "Bird",
            "{1}{U}",
            "Creature — Bird",
            "Flying, vigilance\nWhen this creature enters, draw a card.",
            Some(("2", "1")),
        ));
        assert_eq!(p.kw & (FLYING | VIGILANCE), FLYING | VIGILANCE);
        assert_eq!(p.draw.now, 1.0);
        let bolt = Profile::from_card(&card(
            "Zap",
            "{R}",
            "Instant",
            "Zap deals 3 damage to any target.",
            None,
        ));
        assert_eq!(bolt.kill, Some(Kill::Damage(3)));
        let engine = Profile::from_card(&card(
            "Engine",
            "{2}{B}",
            "Enchantment",
            "At the beginning of your upkeep, each opponent loses 1 life and you gain 1 life.",
            None,
        ));
        assert_eq!((engine.drain.per_turn, engine.gain.per_turn), (1.0, 1.0));
        let wipe = Profile::from_card(&card(
            "Wrath",
            "{2}{W}{W}",
            "Sorcery",
            "Destroy all creatures.",
            None,
        ));
        assert_eq!(
            wipe.wipe,
            Some(Wipe {
                damage: None,
                one_sided: false,
                exile: false
            })
        );
        let maker = Profile::from_card(&card(
            "Maker",
            "{3}{W}",
            "Sorcery",
            "Create two 2/2 white Soldier creature tokens.",
            None,
        ));
        assert_eq!((maker.tokens.now, maker.token_size), (2.0, (2, 2)));
        // A once-only Power-up wipe isn't an "on enter" wipe; an enters trigger and an
        // Aura lock are removal.
        let titan = Profile::from_card(&card(
            "Titan",
            "{2}{B}{R}",
            "Legendary Creature — Titan",
            "Deathtouch\nPower-up — {5}: Destroy each other creature with mana value 3.",
            Some(("4", "4")),
        ));
        assert_eq!((titan.wipe, titan.kill), (None, None));
        let hunter = Profile::from_card(&card(
            "Hunter",
            "{3}{B}",
            "Creature — Assassin",
            "When this creature enters, destroy target creature an opponent controls.",
            Some(("2", "2")),
        ));
        assert_eq!(hunter.kill, Some(Kill::Destroy));
        let lock = Profile::from_card(&card(
            "Lock",
            "{1}{W}",
            "Enchantment — Aura",
            "Enchant creature\nEnchanted creature can't attack or block.",
            None,
        ));
        assert_eq!(lock.kill, Some(Kill::Exile));
        // Attack triggers wait for an attack; mana abilities are used now and then.
        let raider = Profile::from_card(&card(
            "Raider",
            "{2}{B}",
            "Creature — Rogue",
            "Whenever this creature attacks, if it's equipped, each opponent loses 2 life.\n{3}{B}: Draw a card.",
            Some(("2", "2")),
        ));
        assert_eq!(
            (raider.drain.now, raider.drain.per_turn),
            (0.0, 0.0),
            "attack triggers wait for an attack"
        );
        // "If it's equipped" with 6 Equipment in the deck: ~90% of attacks.
        assert!((raider.drain.per_attack - 1.8).abs() < 1e-4);
        // "{3}{B}: Draw a card" is used now and then, not on arrival.
        assert!(raider.draw.now == 0.0 && raider.draw.per_turn > 0.0 && raider.draw.per_turn < 0.3);
        let whip = Profile::in_deck(
            &card(
                "Whip",
                "{2}{B}",
                "Legendary Creature — Villain",
                "Whenever Whip attacks, if he's equipped, each opponent loses X life, where X is the number of Equipment attached to him.",
                Some(("2", "2")),
            ),
            0,
        );
        assert_eq!(whip.drain.per_attack, 0.0);
    }
}
