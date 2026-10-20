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

use crate::analysis::Rng;
use crate::card::{Card, Role, strip_reminder};
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
    /// Destroy or exile any creature.
    Any,
    /// Damage to one creature (kills toughness up to N).
    Damage(i32),
    /// Return a creature to its owner's hand.
    Bounce,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Wipe {
    All,
    Damage(i32),
}

/// How often an effect happens: once (cast / enters), or every turn while it's in play.
#[derive(Debug, Clone, Copy, Default)]
struct Effect {
    now: f32,
    per_turn: f32,
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
    draw: Effect,
    tokens: Effect,
    token_size: (i32, i32),
    drain: Effect,
    gain: Effect,
    anthem: i32,
    equip: (i32, i32),
}

fn number_word(w: &str) -> Option<i32> {
    Some(match w.trim_matches(|c: char| !c.is_alphanumeric()) {
        "a" | "an" | "one" => 1,
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "five" => 5,
        "six" => 6,
        "x" => 3,
        n => n.parse().ok()?,
    })
}

/// "+2/+1" or "3/3" at the start of `s`.
fn pt_at(s: &str) -> Option<(i32, i32)> {
    let word = s.split_whitespace().next()?;
    let (p, t) = word.trim_end_matches([',', '.']).split_once('/')?;
    Some((
        p.trim_start_matches('+').parse().ok()?,
        t.trim_start_matches('+').parse().ok()?,
    ))
}

/// The number right before `marker` in `line` ("deals 3 damage" -> 3).
fn number_before(line: &str, marker: &str) -> Option<i32> {
    let i = line.find(marker)?;
    number_word(line[..i].split_whitespace().last()?)
}

/// The number right after `marker` in `line` ("loses 2 life" with marker "loses " -> 2).
fn number_after(line: &str, marker: &str) -> Option<i32> {
    let i = line.find(marker)?;
    number_word(line[i + marker.len()..].split_whitespace().next()?)
}

impl Profile {
    pub fn from_card(card: &Card) -> Profile {
        // Front face only for double-faced cards.
        let text = strip_reminder(&card.oracle_text).to_lowercase();
        let text = text.split("// back face").next().unwrap_or("").to_string();
        let roles = card.roles();
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
            kw: 0,
            kill: None,
            wipe: None,
            draw: Effect::default(),
            tokens: Effect::default(),
            token_size: (1, 1),
            drain: Effect::default(),
            gain: Effect::default(),
            anthem: 0,
            equip: (0, 0),
        };
        if p.land {
            if p.produces == 0 {
                p.produces = 0b11111 & card.identity().0;
            }
            return p;
        }
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            // Keyword lines: "Flying, vigilance", "Ward {2}".
            let words: Vec<&str> = line.trim_end_matches('.').split(", ").collect();
            let is_kw = |w: &str| KEYWORDS.iter().any(|(k, _)| *k == w) || w.starts_with("ward");
            if words.iter().all(|w| is_kw(w)) {
                for w in words {
                    if w.starts_with("ward") {
                        p.kw |= HEXPROOF;
                    }
                    for (k, bit) in KEYWORDS {
                        if w == k {
                            p.kw |= bit;
                        }
                    }
                }
                continue;
            }
            // When does this line happen? Once, or every turn?
            let repeat = if line.starts_with("at the beginning of") {
                1.0
            } else if line.starts_with("whenever") {
                0.5
            } else if line.contains("{t}") && line.contains(':') && p.permanent {
                if line.split(':').next().unwrap_or("").contains("{t}")
                    && !line.contains("sacrifice")
                {
                    0.5
                } else {
                    0.0
                }
            } else {
                0.0
            };
            let add = |e: &mut Effect, n: f32| {
                if repeat > 0.0 {
                    e.per_turn += n * repeat;
                } else {
                    e.now += n;
                }
            };
            if line.contains("can't be blocked") && !line.contains("target") {
                p.kw |= UNBLOCKABLE;
            }
            if p.permanent && line.contains("{t}: add") {
                // Mana rocks and mana creatures alike add one mana a turn.
                p.rock = true;
            }
            if line.contains("search your library for a basic land")
                || (line.contains("land card") && line.contains("onto the battlefield"))
            {
                p.fetch = true;
            }
            if line.contains("draw ") && !line.contains("then discard") {
                let n = if line.contains("draw two") {
                    2.0
                } else if line.contains("draw three") {
                    3.0
                } else {
                    1.0
                };
                add(&mut p.draw, n);
            }
            if line.contains("create") && line.contains("token") && line.contains("creature") {
                let n = number_after(line, "create ").unwrap_or(1) as f32;
                if let Some(i) = line.find("create ") {
                    if let Some(sz) = line[i..].split_whitespace().find_map(pt_at) {
                        p.token_size = sz;
                    }
                }
                add(&mut p.tokens, n);
            }
            if line.contains("each opponent loses") {
                add(
                    &mut p.drain,
                    number_after(line, "loses ").unwrap_or(1) as f32,
                );
            } else if line.contains("damage to each opponent") {
                add(
                    &mut p.drain,
                    number_before(line, " damage").unwrap_or(1) as f32,
                );
            }
            if line.contains("you gain") {
                add(
                    &mut p.gain,
                    number_after(line, "you gain ").unwrap_or(2) as f32,
                );
            }
            if line.contains("creatures you control get +") {
                if let Some(i) = line.find("get +") {
                    p.anthem += pt_at(&line[i + 4..]).map(|x| x.0).unwrap_or(1);
                }
            }
            for marker in ["equipped creature gets ", "enchanted creature gets "] {
                if let Some(i) = line.find(marker) {
                    if let Some(b) = pt_at(&line[i + marker.len()..]) {
                        if b.0 > 0 || b.1 > 0 {
                            p.equip = (p.equip.0 + b.0, p.equip.1 + b.1);
                        }
                    }
                }
            }
        }
        if roles.contains(&Role::Sweeper) {
            p.wipe = Some(match number_before(&text, " damage to each") {
                Some(n) => Wipe::Damage(n),
                None => Wipe::All,
            });
        } else if roles.contains(&Role::Removal) {
            p.kill = Some(
                if text.contains("destroy target") || text.contains("exile target") {
                    Kill::Any
                } else if let Some(n) = number_before(&text, " damage to") {
                    Kill::Damage(n)
                } else if text.contains("fight") || text.contains("damage equal to") {
                    Kill::Damage(3)
                } else if text.contains("gets -") {
                    Kill::Damage(number_after(&text, "gets -").unwrap_or(2))
                } else {
                    Kill::Any
                },
            );
        } else if roles.contains(&Role::Bounce) && text.contains("target") {
            p.kill = Some(Kill::Bounce);
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
        let mut cards = Vec::new();
        let mut library = Vec::new();
        for (name, n) in &deck.cards {
            if let Some(c) = db.get(name) {
                cards.push(Profile::from_card(c));
                for _ in 0..*n {
                    library.push(cards.len() - 1);
                }
            }
        }
        let mut extra = |name: &Option<String>| {
            name.as_ref().and_then(|n| db.get(n)).map(|c| {
                cards.push(Profile::from_card(c));
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
            let ok = match kill {
                Kill::Any => c.kw & INDESTRUCTIBLE == 0,
                Kill::Damage(n) => c.toughness - c.damage <= n && c.kw & INDESTRUCTIBLE == 0,
                Kill::Bounce => true,
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

fn enter(players: &mut [Player<'_>], me: usize, c: usize, commander: bool) {
    let cards = players[me].cards;
    let prof = &cards[c];
    // One-shot effects.
    if let Some(kill) = prof.kill {
        if let Some((pi, ci)) = best_target(players, me, kill) {
            remove(players, pi, ci, kill == Kill::Bounce);
        }
    }
    if let Some(wipe) = prof.wipe {
        for p in players.iter_mut() {
            let mut i = 0;
            while i < p.board.len() {
                let c = &p.board[i];
                let dies = c.kw & INDESTRUCTIBLE == 0
                    && match wipe {
                        Wipe::All => true,
                        Wipe::Damage(n) => c.toughness - c.damage <= n,
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
        && (prof.draw.per_turn > 0.0 || prof.tokens.per_turn > 0.0 || prof.drain.per_turn > 0.0)
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
        let mine = p.board_power();
        // A damage wipe that can't kill much isn't worth holding for.
        let floor = if matches!(wipe, Wipe::Damage(n) if n <= 2) {
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
        enter(players, me, c, commander);
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

fn combat(players: &mut [Player<'_>], me: usize) {
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
                combat(&mut players, me);
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
        let mut table: Vec<&Prepared> = (1..seats)
            .map(|_| &gauntlet[(rng.next() % gauntlet.len() as u64) as usize])
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

/// Opponent decks for playtesting: decks auto-built from simulated league pools of the
/// same size as yours at this point of the season (`count` of them).
pub fn gauntlet(
    db: &CardDb,
    rules: &Rules,
    starter: &str,
    count: usize,
    seed: u64,
) -> Vec<Prepared> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::new();
    let mut tries = 0;
    while out.len() < count && tries < count * 4 {
        tries += 1;
        let pool = crate::packs::simulated_pool(db, starter, rules.session.max(1), &mut rng);
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
        assert_eq!(wipe.wipe, Some(Wipe::All));
        let maker = Profile::from_card(&card(
            "Maker",
            "{3}{W}",
            "Sorcery",
            "Create two 2/2 white Soldier creature tokens.",
            None,
        ));
        assert_eq!((maker.tokens.now, maker.token_size), (2.0, (2, 2)));
    }
}
