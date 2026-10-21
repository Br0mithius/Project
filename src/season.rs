//! The other league players' season, simulated together session by session, so playtest
//! opponents keep up with the league: packs on the schedule, table drafts (Sessions 2-4),
//! a rebuild after every session (keeping their commander unless another is clearly
//! better, cutting the weakest cards as better ones arrive, singleton from Session 6), and
//! a trading round each session against a shared Public Pool - at least 10 VP traded in,
//! claims paid at 2x VP with the rarity floor, one copy per card, unspent VP banked, and
//! Skill Tree staples bought when they're worth it.
//!
//! Everything here is pure and deterministic for a seed; `opponents` adds the files
//! (pinned real info, the imported Public Pool, the cache).

use crate::analysis::{self, Rng, score_card};
use crate::card::{Card, ColorSet};
use crate::db::{CardDb, Collection, Rules};
use crate::deck::{Deck, PIPER};
use crate::league::{self, STAPLES, claim_cost, staple_cost, vp_value};
use crate::packs::{self, Starter};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// §4: every player trades in at least this much each session.
pub const MIN_TRADE_VP: u32 = 10;
/// A player only switches commander when another one's pool quality beats the current
/// one's by this fraction (people don't rebuild from scratch for a marginal gain).
pub const SWITCH_MARGIN: f32 = 0.15;
/// Drafts happen within seeded 4-player tables.
pub const POD_SIZE: usize = 4;
/// A claim (or staple) must beat the deck's weakest card by at least this much.
pub const MIN_GAIN: f32 = 0.5;
/// Claims a player makes in one trading round at most (people pick a few targets, not
/// the whole pool).
pub const MAX_CLAIMS: usize = 3;
/// After a claim, this many of the best remaining options are re-ranked for the new deck.
const RESCORE: usize = 24;
/// The Public Pool starts as the Foundations starter kit (one of each card).
pub const POOL_START_SET: &str = "FDN";

/// Real information about an opponent, which overrides the simulation for them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pin {
    /// Their actual commander (they're simulated as owning it from the kickoff).
    pub commander: Option<String>,
    /// Their actual decklist ("count name" lines, a Commander section optional); used as
    /// their deck as-is.
    pub deck: Option<String>,
}

impl Pin {
    pub fn is_empty(&self) -> bool {
        self.commander.is_none() && self.deck.is_none()
    }
}

/// Pinned info by player name (as in the roster).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pins {
    pub players: BTreeMap<String, Pin>,
}

impl Pins {
    pub fn get(&self, player: &str) -> Option<&Pin> {
        self.players
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(player))
            .map(|(_, v)| v)
    }
}

/// What one player did in one session.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionLog {
    pub session: u32,
    /// Packs that became usable this session, e.g. "9 HOB" (drafted cards are in `picks`).
    pub packs: Vec<String>,
    /// Rares and mythics among them.
    pub opened: Vec<String>,
    /// Cards taken in the draft that became usable this session, in pick order.
    pub picks: Vec<String>,
    pub commander: Option<String>,
    /// Set when they switched commander this session.
    pub switched_from: Option<String>,
    /// Deck changes at the rebuild (counts for repeats), and through claims.
    pub added: Vec<String>,
    pub cut: Vec<String>,
    pub traded: Vec<String>,
    pub traded_vp: u32,
    pub claimed: Vec<String>,
    pub staples: Vec<String>,
    /// VP banked after the trading round.
    pub vp: u32,
    pub notes: Vec<String>,
}

impl SessionLog {
    /// A short readable summary, one line per kind of event.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        let list = |v: &[String]| v.join(", ");
        let mut head = format!("Session {}", self.session);
        if let Some(c) = &self.commander {
            head += &format!(": {}", c);
        }
        if let Some(old) = &self.switched_from {
            head += &format!(" (switched from {})", old);
        }
        out.push(head);
        if !self.packs.is_empty() {
            let mut l = format!("  packs: {}", list(&self.packs));
            if !self.opened.is_empty() {
                l += &format!(" - opened {}", list(&self.opened));
            }
            out.push(l);
        }
        if !self.picks.is_empty() {
            out.push(format!("  drafted: {}", list(&self.picks)));
        }
        if !self.added.is_empty() {
            out.push(format!("  in: {}", list(&self.added)));
        }
        if !self.cut.is_empty() {
            out.push(format!("  cut: {}", list(&self.cut)));
        }
        if !self.traded.is_empty() {
            out.push(format!(
                "  traded in {} VP: {}",
                self.traded_vp,
                list(&self.traded)
            ));
        }
        if !self.claimed.is_empty() {
            out.push(format!("  claimed: {}", list(&self.claimed)));
        }
        if !self.staples.is_empty() {
            out.push(format!("  Skill Tree: {}", list(&self.staples)));
        }
        out.push(format!("  VP banked: {}", self.vp));
        for n in &self.notes {
            out.push(format!("  note: {}", n));
        }
        out
    }
}

/// One simulated opponent at the end of the simulated stretch of season.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Opponent {
    pub name: String,
    pub commander: Option<String>,
    /// Main deck (commander not included).
    pub cards: BTreeMap<String, u32>,
    /// Their whole pool (deck included).
    pub pool: BTreeMap<String, u32>,
    pub vp: u32,
    /// Skill Tree staples bought so far.
    pub staples: u32,
    /// "" (simulated), "commander" or "decklist".
    pub pinned: String,
    pub log: Vec<SessionLog>,
}

impl Opponent {
    pub fn deck(&self) -> Option<Deck> {
        let mut d = Deck::new(&self.name);
        d.commander = Some(self.commander.clone()?);
        d.cards = self.cards.clone();
        Some(d)
    }
}

/// The simulated league after `session`'s trading round.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Season {
    pub session: u32,
    pub seed: u64,
    /// The last trading round used the imported Public Pool.
    pub real_pool: bool,
    pub opponents: Vec<Opponent>,
    /// Cards left in the simulated Public Pool.
    pub pool_left: u32,
}

impl Season {
    /// Decks for the opponents that have a commander, in roster order.
    pub fn decks(&self) -> Vec<Deck> {
        self.opponents.iter().filter_map(|o| o.deck()).collect()
    }
}

/// Whether a session's packs are drafted at the table (the schedule says so).
pub fn drafted(session: u32) -> bool {
    league::session(session).is_some_and(|s| s.label.contains("drafted"))
}

/// The rules decks are built under at session `k` on the way to `rules.session`: the
/// league schedule's, except the last session, which uses `rules` as given (so a
/// singleton preview or custom deck size applies to the final decks).
fn rules_at(rules: &Rules, k: u32, last: u32) -> Rules {
    let mut r = rules.clone();
    if k != last {
        let _ = league::apply_session(&mut r, k);
    }
    r
}

struct Player {
    name: String,
    starter: Starter,
    pool: BTreeMap<String, u32>,
    deck: Option<Deck>,
    vp: u32,
    staples: u32,
    pin_cmd: Option<String>,
    log: Vec<SessionLog>,
    memo: Memo,
}

impl Player {
    fn coll(&self) -> Collection {
        Collection::new("simulated-pool.txt".into(), self.pool.clone())
    }

    fn gain(&mut self, name: &str, n: u32) {
        *self.pool.entry(name.to_string()).or_insert(0) += n;
    }

    fn lose(&mut self, name: &str) {
        if let Some(n) = self.pool.get_mut(name) {
            *n -= 1;
            if *n == 0 {
                self.pool.remove(name);
            }
        }
    }

    fn has_commander_option(&self, db: &CardDb) -> bool {
        self.pin_cmd.is_some()
            || self.pool.keys().any(|n| {
                db.get(n)
                    .is_some_and(|c| c.can_be_commander() && c.name != PIPER)
            })
    }

    fn entry(&mut self, k: u32) -> &mut SessionLog {
        if self.log.last().is_none_or(|l| l.session != k) {
            self.log.push(SessionLog {
                session: k,
                ..Default::default()
            });
        }
        self.log.last_mut().unwrap()
    }
}

/// Pack contents per set, worked out once.
struct Packs<'a> {
    db: &'a CardDb,
    sets: HashMap<String, [Vec<&'a Card>; 4]>,
}

impl<'a> Packs<'a> {
    fn open(&mut self, set: &str, rng: &mut Rng) -> Vec<&'a Card> {
        let db = self.db;
        let contents = self
            .sets
            .entry(set.to_string())
            .or_insert_with(|| packs::set_contents(db, set));
        packs::open_pack(contents, rng)
    }
}

/// Simulates the other players' season up to `rules.session` (Session 1 if unset).
/// `real_pool` (the imported Public Pool) replaces the simulated pool for that last
/// session's trading round; `pins` override what's known about a player.
pub fn simulate(
    db: &CardDb,
    rules: &Rules,
    seed: u64,
    real_pool: Option<&Collection>,
    pins: &Pins,
) -> Season {
    let last = rules.session.clamp(1, 13);
    let mut rng = Rng::new(seed);
    let mut packs = Packs {
        db,
        sets: HashMap::new(),
    };
    let mut players: Vec<Player> = rules
        .roster
        .others
        .iter()
        .map(|s| {
            // A pinned commander, or the one heading a pinned decklist.
            let pin_cmd = pins
                .get(&s.name)
                .and_then(|p| {
                    p.commander
                        .clone()
                        .or_else(|| Deck::from_text("pinned", p.deck.as_deref()?, db).commander)
                })
                .and_then(|c| db.get(&c))
                .filter(|c| c.can_be_commander())
                .map(|c| c.name.clone());
            Player {
                name: s.name.clone(),
                starter: s.clone(),
                pool: BTreeMap::new(),
                deck: None,
                vp: 0,
                staples: 0,
                pin_cmd,
                log: Vec::new(),
                memo: Memo::default(),
            }
        })
        .collect();
    let mut public: BTreeMap<String, u32> = db
        .cards
        .iter()
        .filter(|c| c.set == POOL_START_SET && !c.is_basic())
        .map(|c| (c.name.clone(), 1))
        .collect();
    let mut drafted_picks: Vec<Vec<String>> = vec![Vec::new(); players.len()];
    for k in 1..=last {
        let r = rules_at(rules, k, last);
        // Packs that become usable now.
        for (i, p) in players.iter_mut().enumerate() {
            let deliveries: Vec<packs::Delivery> = packs::season_packs(db, &p.starter)
                .into_iter()
                .filter(|d| d.usable_from() == k && !d.set.is_empty() && !drafted(d.session))
                .collect();
            // A kickoff without a single legendary creature can't make a deck: reopen (rare).
            for attempt in 0..4 {
                let mut opened = Vec::new();
                for d in &deliveries {
                    for _ in 0..d.count {
                        opened.extend(packs.open(&d.set, &mut rng));
                    }
                }
                let backup = p.pool.clone();
                for c in &opened {
                    p.gain(&c.name, 1);
                }
                if k > 1 || attempt == 3 || p.has_commander_option(db) {
                    let log = p.entry(k);
                    log.packs = deliveries
                        .iter()
                        .map(|d| format!("{} {}", d.count, d.set))
                        .collect();
                    log.opened = opened
                        .iter()
                        .filter(|c| c.rarity_rank() >= 3)
                        .map(|c| c.name.clone())
                        .collect();
                    break;
                }
                p.pool = backup;
            }
            if k == 1 {
                if let Some(c) = p.pin_cmd.clone() {
                    if p.pool.get(&c).is_none() {
                        p.gain(&c, 1);
                    }
                    p.entry(k).notes.push(format!("commander pinned: {}", c));
                }
            }
            let picks = std::mem::take(&mut drafted_picks[i]);
            for c in &picks {
                p.gain(c, 1);
            }
            let weekly = p.starter.weekly.clone();
            let log = p.entry(k);
            if !picks.is_empty() {
                log.packs.push(format!("1 {} (drafted)", weekly));
                log.picks = picks;
            }
        }
        rebuild_all(&mut players, db, &r, k);
        if k == last {
            if let Some(real) = real_pool.filter(|c| c.total() > 0) {
                public = real
                    .counts
                    .iter()
                    .filter_map(|(n, c)| db.get(n).map(|card| (card.name.clone(), *c)))
                    .collect();
            }
        }
        trading_round(&mut players, &mut public, db, &r, k);
        if drafted(k) && k < last {
            drafted_picks = draft(&players, &rules.roster.me, &mut packs, db, &r, &mut rng);
        }
    }
    let opponents = players
        .into_iter()
        .map(|p| {
            let mut o = Opponent {
                name: p.name.clone(),
                commander: p.deck.as_ref().and_then(|d| d.commander.clone()),
                cards: p.deck.map(|d| d.cards).unwrap_or_default(),
                pool: p.pool,
                vp: p.vp,
                staples: p.staples,
                pinned: if p.pin_cmd.is_some() {
                    "commander".into()
                } else {
                    String::new()
                },
                log: p.log,
            };
            if let Some(text) = pins.get(&p.name).and_then(|x| x.deck.as_deref()) {
                let d = Deck::from_text(&p.name, text, db);
                if d.commander.is_some() || p.pin_cmd.is_some() {
                    o.commander = d.commander.or(p.pin_cmd);
                    o.cards = d.cards;
                    o.pinned = "decklist".into();
                }
            }
            o
        })
        .collect();
    Season {
        session: last,
        seed,
        real_pool: real_pool.is_some_and(|c| c.total() > 0),
        opponents,
        pool_left: public.values().sum(),
    }
}

/// `f` over `items` on every CPU core, results in order.
fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .clamp(1, items.len().max(1));
    let chunk = items.len().div_ceil(threads).max(1);
    let f = &f;
    std::thread::scope(|s| {
        let parts: Vec<_> = items
            .chunks(chunk)
            .map(|part| s.spawn(move || part.iter().map(f).collect::<Vec<R>>()))
            .collect();
        parts.into_iter().flat_map(|h| h.join().unwrap()).collect()
    })
}

/// Every player rebuilds after the session's packs; independent, so on all cores.
fn rebuild_all(players: &mut [Player], db: &CardDb, rules: &Rules, k: u32) {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .clamp(1, players.len().max(1));
    let chunk = players.len().div_ceil(threads).max(1);
    std::thread::scope(|s| {
        for part in players.chunks_mut(chunk) {
            s.spawn(move || {
                for p in part {
                    rebuild(p, db, rules, k);
                }
            });
        }
    });
}

/// Scores of cards for a deck holding only a given commander, which don't change as the
/// pool grows: `pool_quality_for` re-scores them for every commander every session, so
/// remembering them makes the weekly commander check cheap. Keyed by card addresses;
/// cleared when the rules that affect scoring change.
#[derive(Default)]
struct Memo {
    rules: (u32, bool, u32),
    scores: HashMap<(usize, usize), f32>,
}

fn addr(c: &Card) -> usize {
    c as *const Card as usize
}

/// `analysis::pool_quality_for`, with the commander-only scores remembered.
fn pool_quality(
    cmd: &Card,
    colors: ColorSet,
    db: &CardDb,
    coll: &Collection,
    rules: &Rules,
    memo: &mut Memo,
) -> f32 {
    let key = (rules.deck_size, rules.singleton, rules.players);
    if memo.rules != key {
        memo.rules = key;
        memo.scores.clear();
    }
    let mut deck = Deck::new("probe");
    deck.commander = Some(cmd.name.clone());
    let mut scores = Vec::new();
    for c in coll.counts.keys().filter_map(|n| db.get(n)) {
        if c.is_land() || !c.playable_with(colors) {
            continue;
        }
        let n = analysis::available(c, &deck, coll, rules);
        if n == 0 || n >= 99 {
            continue;
        }
        let s = *memo
            .scores
            .entry((addr(cmd), addr(c)))
            .or_insert_with(|| score_card(c, &deck, db, rules).0);
        for _ in 0..n {
            scores.push(s);
        }
    }
    scores.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let slots = rules
        .main_deck_size()
        .saturating_sub(analysis::recommended_lands(3.0, rules)) as usize;
    scores.iter().take(slots).sum::<f32>() / slots.max(1) as f32
}

/// Pool quality of a commander in its best colours (`analysis::best_colors`' choice).
fn quality(c: &Card, db: &CardDb, coll: &Collection, rules: &Rules, memo: &mut Memo) -> f32 {
    let id = c.identity();
    if id.count() <= 2 {
        return pool_quality(c, id, db, coll, rules, memo);
    }
    let base = c.cost_colors();
    let max = base.count().max(2);
    let mut best = f32::MIN;
    for mask in 1u8..32 {
        let s = ColorSet(mask);
        if s.is_subset_of(id) && base.is_subset_of(s) && s.count() <= max {
            best = best.max(pool_quality(c, s, db, coll, rules, memo));
        }
    }
    best
}

/// The commander to play: the current one unless another's pool quality beats it by
/// more than `SWITCH_MARGIN`.
fn choose_commander<'a>(
    db: &'a CardDb,
    coll: &Collection,
    rules: &Rules,
    current: Option<&str>,
    memo: &mut Memo,
) -> Option<&'a Card> {
    let scored: Vec<(f32, &Card)> = coll
        .counts
        .keys()
        .filter_map(|n| db.get(n))
        .filter(|c| c.can_be_commander() && c.name != PIPER)
        .map(|c| (quality(c, db, coll, rules, memo), c))
        .collect();
    let best = scored
        .iter()
        .copied()
        .reduce(|a, b| if b.0 > a.0 { b } else { a })?;
    if let Some(cur) = current.and_then(|n| scored.iter().find(|(_, c)| c.name == n)) {
        if best.0 <= cur.0 + SWITCH_MARGIN * cur.0.abs() {
            return Some(cur.1);
        }
    }
    Some(best.1)
}

/// Names with counts ("2x Web Up") of non-basic cards in `a` but not `b`.
fn diff(a: &BTreeMap<String, u32>, b: &BTreeMap<String, u32>, db: &CardDb) -> Vec<String> {
    a.iter()
        .filter(|(n, _)| db.get(n).is_none_or(|c| !c.is_basic()))
        .filter_map(|(n, &c)| {
            let gone = c.saturating_sub(*b.get(n).unwrap_or(&0));
            match gone {
                0 => None,
                1 => Some(n.clone()),
                g => Some(format!("{}x {}", g, n)),
            }
        })
        .collect()
}

/// Swaps cards that arrived since the last build in for the deck's weakest, one at a time
/// while it's an upgrade - how a player updates a deck they already have. New nonbasic
/// lands for the deck's colours replace basics.
fn improve(deck: &mut Deck, db: &CardDb, coll: &Collection, rules: &Rules) {
    let plays = deck_colors(deck, db);
    for _ in 0..12 {
        let weak = analysis::weakest(deck, db, rules);
        let Some(worst) = weak.first() else { break };
        let mut without = deck.clone();
        without.remove(&worst.card.name, 1);
        let best = analysis::suggestions_within(&without, db, coll, rules, Some(plays));
        let Some(top) = best.first() else { break };
        if top.card.name == worst.card.name || top.score < worst.score + 0.1 {
            break;
        }
        deck.remove(&worst.card.name, 1);
        deck.add(&top.card.name, 1);
    }
    let Some(id) = deck.identity(db) else { return };
    for land in coll.counts.keys().filter_map(|n| db.get(n)) {
        if !land.is_land() || land.is_basic() || !land.identity().is_subset_of(id) {
            continue;
        }
        if land.produces().0 & plays.0 == 0 {
            continue;
        }
        let n = analysis::available(land, deck, coll, rules);
        if n == 0 || n >= 99 {
            continue;
        }
        let basic = deck
            .cards
            .iter()
            .filter(|(b, _)| db.get(b).is_some_and(|c| c.is_basic()))
            .max_by_key(|(_, c)| **c)
            .map(|(b, _)| b.clone());
        let Some(basic) = basic else { break };
        deck.remove(&basic, 1);
        deck.add(&land.name, 1);
    }
}

/// After a session's packs: settle on a commander, then rebuild from scratch (first deck,
/// new commander, new deck size, singleton starting) or swap in the new arrivals.
fn rebuild(p: &mut Player, db: &CardDb, rules: &Rules, k: u32) {
    let coll = p.coll();
    let current = p.deck.as_ref().and_then(|d| d.commander.clone());
    let cmd = match &p.pin_cmd {
        Some(c) => db.get(c),
        None => choose_commander(db, &coll, rules, current.as_deref(), &mut p.memo),
    };
    let Some(cmd) = cmd else {
        p.entry(k)
            .notes
            .push("no legendary creature to lead a deck yet".into());
        return;
    };
    let old = p.deck.take().unwrap_or_default();
    let stale = old.commander.as_deref() != Some(cmd.name.as_str())
        || old.main_count() != old.main_target(rules)
        || (rules.singleton
            && old
                .cards
                .iter()
                .any(|(n, c)| *c > 1 && db.get(n).is_some_and(|x| !x.is_basic())))
        || old.cards.keys().any(|n| {
            db.get(n)
                .is_some_and(|c| !c.is_basic() && coll.owned(n) < old.count_of(n))
        });
    let deck = if stale {
        analysis::autobuild(cmd, db, &coll, rules, None, None).0
    } else {
        let mut d = old.clone();
        improve(&mut d, db, &coll, rules);
        d
    };
    let log = p.entry(k);
    log.commander = Some(cmd.name.clone());
    if current.as_deref().is_some_and(|c| c != cmd.name) {
        log.switched_from = current;
        log.added = vec![format!("new deck around {}", cmd.name)];
    } else if old.commander.is_some() {
        log.added = diff(&deck.cards, &old.cards, db);
        log.cut = diff(&old.cards, &deck.cards, db);
    }
    p.deck = Some(deck);
}

/// Colours the deck's lands make (inside its identity): what its spells can be cast with.
fn deck_colors(deck: &Deck, db: &CardDb) -> ColorSet {
    let id = deck.identity(db).unwrap_or_default();
    let mut made = ColorSet(0);
    for (c, _) in analysis::resolved(deck, db) {
        if c.is_land() {
            made = made.union(c.produces());
        }
    }
    let made = ColorSet(made.0 & id.0);
    if made.is_empty() { id } else { made }
}

/// A player's state during one trading round.
struct Round<'a> {
    /// Remaining spare copies, cheapest loss per VP first.
    spares: Vec<&'a Card>,
    /// Committed trade-ins and whether each already covers a claim's rarity floor.
    traded: Vec<(&'a Card, bool)>,
    spent: u32,
    claimed: Vec<String>,
    /// Ranked claim options (gain, card, card it replaces).
    options: Vec<(f32, &'a Card, String)>,
    /// A claim changed the deck since `options` was ranked.
    stale: bool,
}

impl Round<'_> {
    fn traded_vp(&self) -> u32 {
        self.traded.iter().map(|(c, _)| vp_value(c)).sum()
    }
}

fn rarity(c: &Card) -> String {
    c.rarity.to_lowercase()
}

/// What claiming `card` takes from this round: spare copies to commit (the first `floor`
/// of them covering the rarity floor) and already-traded cards to mark as covering it.
/// None if the player can't meet the floor or the price.
pub(crate) fn plan_claim(
    card: &Card,
    bank: u32,
    traded: &[(&Card, bool)],
    spares: &[&Card],
    spent: u32,
) -> Option<(Vec<usize>, usize, Vec<usize>)> {
    let free = |r: &str| -> Vec<usize> {
        traded
            .iter()
            .enumerate()
            .filter(|(_, (c, used))| !used && rarity(c) == r)
            .map(|(i, _)| i)
            .collect()
    };
    let spare = |r: &str| -> Vec<usize> {
        spares
            .iter()
            .enumerate()
            .filter(|(_, c)| rarity(c) == r)
            .map(|(i, _)| i)
            .collect()
    };
    let mut mark: Vec<usize> = Vec::new();
    let mut commit: Vec<usize> = Vec::new();
    match rarity(card).as_str() {
        "mythic" => {
            if let Some(&i) = free("mythic").first() {
                mark.push(i);
            } else if let Some(&i) = spare("mythic").first() {
                commit.push(i);
            } else {
                // Two rares also cover a mythic.
                let t: Vec<usize> = free("rare").into_iter().take(2).collect();
                let s: Vec<usize> = spare("rare").into_iter().take(2 - t.len()).collect();
                if t.len() + s.len() < 2 {
                    return None;
                }
                mark = t;
                commit = s;
            }
        }
        "rare" => {
            if let Some(&i) = free("rare").first() {
                mark.push(i);
            } else if let Some(&i) = spare("rare").first() {
                commit.push(i);
            } else {
                return None;
            }
        }
        _ => {}
    }
    let floor = commit.len();
    let cost = claim_cost(card);
    let traded_vp: u32 = traded.iter().map(|(c, _)| vp_value(c)).sum();
    let mut vp = (bank + traded_vp).saturating_sub(spent)
        + commit.iter().map(|&i| vp_value(spares[i])).sum::<u32>();
    for (i, c) in spares.iter().enumerate() {
        if vp >= cost {
            break;
        }
        if !commit.contains(&i) {
            commit.push(i);
            vp += vp_value(c);
        }
    }
    (vp >= cost).then_some((commit, floor, mark))
}

/// Claim options for a player's deck from the pool, best first. `among` limits them to
/// a short list (re-ranking the best options after a claim changed the deck).
fn claim_options<'a>(
    p: &Player,
    public: &BTreeMap<String, u32>,
    db: &'a CardDb,
    rules: &Rules,
    among: Option<&[&'a Card]>,
) -> Vec<(f32, &'a Card, String)> {
    let Some(deck) = &p.deck else {
        return Vec::new();
    };
    let Some(id) = deck.identity(db) else {
        return Vec::new();
    };
    let plays = deck_colors(deck, db);
    let weak = analysis::weakest(deck, db, rules);
    let Some(worst) = weak.first() else {
        return Vec::new();
    };
    let mut without = deck.clone();
    without.remove(&worst.card.name, 1);
    let cards: Vec<&Card> = match among {
        Some(list) => list.to_vec(),
        None => public
            .iter()
            .filter(|(_, n)| **n > 0)
            .filter_map(|(name, _)| db.get(name))
            .collect(),
    };
    let mut out: Vec<(f32, &Card, String)> = cards
        .into_iter()
        .filter(|c| {
            !c.is_land()
                && vp_value(c) > 0
                && c.identity().is_subset_of(id)
                && c.playable_with(plays)
                && c.name != worst.card.name
                && !(rules.singleton && p.pool.contains_key(&c.name))
        })
        .map(|c| {
            (
                score_card(c, &without, db, rules).0 - worst.score,
                c,
                worst.card.name.clone(),
            )
        })
        .filter(|(g, _, _)| *g >= MIN_GAIN)
        .collect();
    out.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.name.cmp(&b.1.name)));
    out
}

/// The session's trading round for everyone: claims in turns (rotating who goes first),
/// then the 10 VP minimum, then a Skill Tree staple; trade-ins enter the pool after.
fn trading_round(
    players: &mut [Player],
    public: &mut BTreeMap<String, u32>,
    db: &CardDb,
    rules: &Rules,
    k: u32,
) {
    let n = players.len();
    if n == 0 {
        return;
    }
    let public_now: &BTreeMap<String, u32> = public;
    let mut rounds: Vec<Round> = par_map(players, |p| {
        let deck = p.deck.clone().unwrap_or_else(|| Deck::new("none"));
        // Every spare copy, cheapest loss per VP first (the trade-in planner's order).
        let plan = league::plan_trade_in(&deck, db, &p.coll(), rules, u32::MAX, None);
        let mut spares = Vec::new();
        for t in plan {
            for _ in 0..t.copies {
                spares.push(t.card);
            }
        }
        Round {
            spares,
            traded: Vec::new(),
            spent: 0,
            claimed: Vec::new(),
            options: claim_options(p, public_now, db, rules, None),
            stale: false,
        }
    });
    let start = (k as usize - 1) % n;
    let order: Vec<usize> = (0..n).map(|i| (start + i) % n).collect();
    loop {
        let mut any = false;
        for &i in &order {
            let p = &mut players[i];
            let st = &mut rounds[i];
            if st.claimed.len() >= MAX_CLAIMS {
                continue;
            }
            if st.stale {
                // The deck changed with the last claim: re-rank the best remaining options.
                let short: Vec<&Card> = st
                    .options
                    .iter()
                    .map(|o| o.1)
                    .filter(|c| public.get(&c.name).is_some_and(|n| *n > 0))
                    .take(RESCORE)
                    .collect();
                st.options = claim_options(p, public, db, rules, Some(&short));
                st.stale = false;
            }
            let mut choice = None;
            for (gain, card, replaces) in &st.options {
                if public.get(&card.name).copied().unwrap_or(0) == 0
                    || st.claimed.contains(&card.name)
                {
                    continue;
                }
                if let Some(plan) = plan_claim(card, p.vp, &st.traded, &st.spares, st.spent) {
                    choice = Some((*gain, *card, replaces.clone(), plan));
                    break;
                }
            }
            let Some((gain, card, replaces, (commit, floor, mark))) = choice else {
                continue;
            };
            any = true;
            for &m in &mark {
                st.traded[m].1 = true;
            }
            let mut commit_sorted: Vec<(usize, bool)> = commit
                .iter()
                .enumerate()
                .map(|(j, &s)| (s, j < floor))
                .collect();
            commit_sorted.sort_by(|a, b| b.0.cmp(&a.0));
            for (s, is_floor) in commit_sorted {
                let spare = st.spares.remove(s);
                st.traded.push((spare, is_floor));
            }
            let cost = claim_cost(card);
            st.spent += cost;
            st.claimed.push(card.name.clone());
            st.stale = true;
            if let Some(c) = public.get_mut(&card.name) {
                *c -= 1;
                if *c == 0 {
                    public.remove(&card.name);
                }
            }
            p.gain(&card.name, 1);
            if let Some(d) = &mut p.deck {
                d.add(&card.name, 1);
                d.remove(&replaces, 1);
            }
            let log = p.entry(k);
            log.claimed.push(format!(
                "{} ({}, {} VP, +{:.1} over {})",
                card.name,
                rarity(card),
                cost,
                gain,
                replaces
            ));
            log.added.push(card.name.clone());
            log.cut.push(replaces);
        }
        if !any {
            break;
        }
    }
    let mut incoming: Vec<String> = Vec::new();
    for (p, st) in players.iter_mut().zip(rounds) {
        let mut st = st;
        // The weekly minimum.
        while st.traded_vp() < MIN_TRADE_VP && !st.spares.is_empty() {
            let s = st.spares.remove(0);
            st.traded.push((s, false));
        }
        let traded_vp = st.traded_vp();
        let mut vp = (p.vp + traded_vp).saturating_sub(st.spent);
        let bought = buy_staple(p, db, rules, vp);
        if let Some((name, cost)) = &bought {
            vp -= cost;
            p.staples += 1;
            p.entry(k).staples.push(format!("{} ({} VP)", name, cost));
        }
        p.vp = vp;
        for (c, _) in &st.traded {
            p.lose(&c.name);
            incoming.push(c.name.clone());
        }
        let log = p.entry(k);
        log.traded = st
            .traded
            .iter()
            .map(|(c, _)| format!("{} ({})", c.name, rarity(c)))
            .collect();
        log.traded_vp = traded_vp;
        log.vp = vp;
        if traded_vp < MIN_TRADE_VP {
            log.notes.push(format!(
                "only {} VP of spare cards to trade in (10 VP minimum)",
                traded_vp
            ));
        }
    }
    for name in incoming {
        *public.entry(name).or_insert(0) += 1;
    }
}

/// Buys the next Skill Tree staple when the banked VP covers it and one is worth a slot.
/// Returns its name and cost.
fn buy_staple(p: &mut Player, db: &CardDb, rules: &Rules, vp: u32) -> Option<(String, u32)> {
    let cost = staple_cost(p.staples + 1);
    if vp < cost {
        return None;
    }
    let deck = p.deck.as_ref()?;
    let id = deck.identity(db)?;
    let weak = analysis::weakest(deck, db, rules);
    let worst = weak.first()?;
    let mut without = deck.clone();
    without.remove(&worst.card.name, 1);
    let basic = deck
        .cards
        .iter()
        .filter(|(n, _)| db.get(n).is_some_and(|c| c.is_basic()))
        .max_by_key(|(_, c)| **c)
        .map(|(n, _)| n.clone());
    let mut best: Option<(f32, &Card)> = None;
    for name in STAPLES {
        let Some(c) = db.get(name) else { continue };
        if p.pool.contains_key(&c.name) || !c.identity().is_subset_of(id) {
            continue;
        }
        let gain = if c.is_land() {
            // Rainbow lands: worth a basic's slot in a deck of two or more colours.
            if id.count() >= 2 && basic.is_some() {
                MIN_GAIN
            } else {
                continue;
            }
        } else {
            score_card(c, &without, db, rules).0 - worst.score
        };
        if gain >= MIN_GAIN && best.is_none_or(|(g, _)| gain > g) {
            best = Some((gain, c));
        }
    }
    let (_, c) = best?;
    let replaces = if c.is_land() {
        basic?
    } else {
        worst.card.name.clone()
    };
    let name = c.name.clone();
    p.gain(&name, 1);
    let d = p.deck.as_mut()?;
    d.add(&name, 1);
    d.remove(&replaces, 1);
    Some((name, cost))
}

/// A table draft: everyone (you included, as a seat whose picks aren't tracked) is seated
/// at random in pods of four, opens one pack of their weekly set and picks in turn,
/// passing the packs on. Each simulated player picks the top of the draft helper's ranking
/// for their current deck. Returns each player's picks.
fn draft<'a>(
    players: &[Player],
    me: &Starter,
    packs: &mut Packs<'a>,
    db: &'a CardDb,
    rules: &Rules,
    rng: &mut Rng,
) -> Vec<Vec<String>> {
    let n = players.len();
    let mut seats: Vec<Option<usize>> = (0..n).map(Some).chain([None]).collect();
    rng.shuffle(&mut seats);
    let mut picks = vec![Vec::new(); n];
    let blank = Deck::new("blank");
    let colls: Vec<Collection> = players.iter().map(|p| p.coll()).collect();
    let mut pods: Vec<(Vec<Option<usize>>, Vec<Vec<&Card>>)> = Vec::new();
    for pod in seats.chunks(POD_SIZE) {
        let open: Vec<Vec<&Card>> = pod
            .iter()
            .map(|s| {
                let set = match s {
                    Some(i) => &players[*i].starter.weekly,
                    None => &me.weekly,
                };
                packs.open(set, rng)
            })
            .collect();
        pods.push((pod.to_vec(), open));
    }
    // Pass by pass: every seat picks from a different pack, so the picks of one pass are
    // worked out together on all cores.
    let mut t = 0;
    while pods
        .iter()
        .any(|(_, open)| open.iter().any(|p| !p.is_empty()))
    {
        let jobs: Vec<(usize, usize, Option<usize>)> = pods
            .iter()
            .enumerate()
            .flat_map(|(pi, (pod, open))| {
                let m = pod.len();
                pod.iter()
                    .enumerate()
                    .map(move |(i, seat)| (pi, (i + t) % m, *seat))
                    .filter(|(_, pk, _)| !open[*pk].is_empty())
                    .collect::<Vec<_>>()
            })
            .collect();
        let chosen: Vec<Option<String>> = par_map(&jobs, |&(pi, pk, seat)| {
            let pack = &pods[pi].1[pk];
            match seat {
                Some(who) => {
                    let deck = players[who].deck.as_ref().unwrap_or(&blank);
                    league::rank_pack(pack, deck, db, &colls[who], rules, &[])
                        .first()
                        .map(|d| d.card.name.clone())
                }
                // Your seat: strongest card (your picks are in your real pool).
                None => pack
                    .iter()
                    .max_by(|a, b| {
                        let v = |c: &Card| analysis::card_power(c) + 0.25 * vp_value(c) as f32;
                        v(a).partial_cmp(&v(b)).unwrap()
                    })
                    .map(|c| c.name.clone()),
            }
        });
        for ((pi, pk, seat), name) in jobs.into_iter().zip(chosen) {
            let Some(name) = name else { continue };
            let pack = &mut pods[pi].1[pk];
            if let Some(j) = pack.iter().position(|c| c.name == name) {
                pack.remove(j);
            }
            if let Some(who) = seat {
                picks[who].push(name);
            }
        }
        t += 1;
    }
    picks
}

#[cfg(test)]
mod tests;
