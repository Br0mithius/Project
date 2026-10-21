//! Paired playtests: measure what a change to a deck is worth by playing the deck and the
//! changed deck through exactly the same games.
//!
//! Every game has its own seed (from the run's seed and the game's number), which fixes
//! the opponents, our seat and the whole game - shuffles included. A variant keeps the
//! library's layout and only changes which card one library entry is, so until the
//! swapped card is drawn (or searched for, or milled) the variant's game is the base
//! game, move for move. The difference "variant won - base won" is then zero in most
//! games, and its average measures the swap with far less noise than two independent
//! win rates would (common random numbers).

use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// A changed deck: each (out, in) pair repoints one library entry that is card `out`
/// (an index into `Prepared::cards`) to the card `in`. Several pairs make a combination;
/// a basic land out and a spell in (or the reverse) changes the land count.
#[derive(Clone)]
pub struct Variant {
    pub swaps: Vec<(u16, SimCard)>,
}

/// A paired measurement against the base deck: the average of (variant won - base won)
/// over the games, as a fraction (0.02 = two win-rate points), and its standard error.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Measure {
    pub gain: f32,
    pub stderr: f32,
    pub games: u32,
}

impl Measure {
    /// How many standard errors the gain is from zero.
    pub fn z(&self) -> f32 {
        if self.stderr > 0.0 {
            self.gain / self.stderr
        } else if self.gain > 0.0 {
            f32::INFINITY
        } else if self.gain < 0.0 {
            f32::NEG_INFINITY
        } else {
            0.0
        }
    }

    /// Two independent measurements of the same change (different seeds) pooled.
    pub fn pool(&self, other: &Measure) -> Measure {
        let (a, b) = (self.games as f32, other.games as f32);
        if a + b == 0.0 {
            return Measure::default();
        }
        Measure {
            gain: (self.gain * a + other.gain * b) / (a + b),
            stderr: ((self.stderr * a).powi(2) + (other.stderr * b).powi(2)).sqrt() / (a + b),
            games: self.games + other.games,
        }
    }
}

/// Who won each game, for decks that all played the same numbered games.
#[derive(Debug, PartialEq)]
pub struct Outcomes {
    pub games: u32,
    /// `wins[deck][game]`.
    wins: Vec<Vec<bool>>,
}

impl Outcomes {
    pub fn decks(&self) -> usize {
        self.wins.len()
    }

    pub fn win_rate(&self, deck: usize) -> f32 {
        let w = self.wins[deck].iter().filter(|&&x| x).count();
        w as f32 / self.games.max(1) as f32
    }

    /// Paired difference: deck `a`'s wins minus deck `b`'s, game by game.
    pub fn diff(&self, a: usize, b: usize) -> Measure {
        let n = self.games as usize;
        if n == 0 {
            return Measure::default();
        }
        let (mut sum, mut sq) = (0i64, 0i64);
        for g in 0..n {
            let d = self.wins[a][g] as i64 - self.wins[b][g] as i64;
            sum += d;
            sq += d * d;
        }
        let mean = sum as f64 / n as f64;
        let var = if n > 1 {
            ((sq as f64 - n as f64 * mean * mean) / (n as f64 - 1.0)).max(0.0)
        } else {
            0.0
        };
        Measure {
            gain: mean as f32,
            stderr: (var / n as f64).sqrt() as f32,
            games: n as u32,
        }
    }
}

impl Prepared {
    /// The index of a main-deck card by name (not the commanders).
    pub fn card_index(&self, name: &str) -> Option<u16> {
        self.library
            .iter()
            .copied()
            .find(|&i| self.cards[i as usize].name == name)
    }

    /// Cards in the library (one per copy).
    pub fn library_len(&self) -> usize {
        self.library.len()
    }

    /// Copies of card `idx` in the library.
    pub fn copies(&self, idx: u16) -> usize {
        self.library.iter().filter(|&&i| i == idx).count()
    }

    /// This deck with one library entry repointed per swap (see `Variant`). A card coming
    /// in that the deck already has shares that card's index, so the legend rule and
    /// "another copy" checks see them as the same card.
    pub fn with_swaps(&self, swaps: &[(u16, SimCard)]) -> Prepared {
        let mut out = Prepared {
            cards: self.cards.clone(),
            library: self.library.clone(),
            commanders: self.commanders.clone(),
        };
        let cmd: Vec<u16> = self.commanders.iter().map(|c| c.0).collect();
        let mut moved = vec![false; out.library.len()];
        for (from, card) in swaps {
            let Some(slot) = (0..out.library.len()).find(|&k| !moved[k] && out.library[k] == *from)
            else {
                debug_assert!(false, "swap out of a card the deck doesn't have");
                continue;
            };
            let existing = (0..out.cards.len())
                .find(|&i| !cmd.contains(&(i as u16)) && out.cards[i].name == card.name);
            let to = match existing {
                Some(i) => i as u16,
                None => {
                    out.cards.push(card.clone());
                    (out.cards.len() - 1) as u16
                }
            };
            out.library[slot] = to;
            moved[slot] = true;
        }
        out
    }
}

/// A well-mixed 64-bit value from `x` (splitmix64), so neighbouring game numbers get
/// unrelated seeds.
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E3779B97F4A7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D049BB133111EB);
    x ^ (x >> 31)
}

/// Plays game number `g` once with each of `decks` in our seat: the same opponents, seat
/// and game seed for all of them. Returns whether each deck won.
fn one_game(
    g: u32,
    decks: &[&Prepared],
    gauntlet: &[Prepared],
    rules: &Rules,
    seed: u64,
) -> Vec<bool> {
    let seats = rules.players.clamp(2, 4) as usize;
    let mut rng = Rng::new(mix(seed ^ mix(g as u64)));
    let mut picks: Vec<usize> = (0..gauntlet.len()).collect();
    rng.shuffle(&mut picks);
    let opponents: Vec<&Prepared> = (1..seats)
        .map(|i| &gauntlet[picks[(i - 1) % picks.len()]])
        .collect();
    let seat = g as usize % seats;
    let game_seed = rng.next();
    decks
        .iter()
        .map(|&d| {
            let mut table = opponents.clone();
            table.insert(seat, d);
            let mut game = Game::new(&table, rules, Rng::new(game_seed));
            play(&mut game) == seat
        })
        .collect()
}

fn threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
}

/// Plays `games` numbered games with every deck in `decks` (each deck in our seat, the
/// rest of the table from `gauntlet`). `done` counts deck-games played; setting `cancel`
/// stops early (the outcomes are then incomplete - check `cancel` before using them).
pub fn play_all(
    decks: &[&Prepared],
    gauntlet: &[Prepared],
    rules: &Rules,
    games: u32,
    seed: u64,
    done: &AtomicUsize,
    cancel: &AtomicBool,
) -> Outcomes {
    play_all_threads(decks, gauntlet, rules, games, seed, done, cancel, threads())
}

/// `play_all` on a given number of threads (the outcomes don't depend on it).
#[allow(clippy::too_many_arguments)]
pub fn play_all_threads(
    decks: &[&Prepared],
    gauntlet: &[Prepared],
    rules: &Rules,
    games: u32,
    seed: u64,
    done: &AtomicUsize,
    cancel: &AtomicBool,
    threads: usize,
) -> Outcomes {
    let mut wins = vec![vec![false; games as usize]; decks.len()];
    if gauntlet.is_empty() || decks.is_empty() || games == 0 {
        return Outcomes {
            games: if gauntlet.is_empty() { 0 } else { games },
            wins,
        };
    }
    let next = AtomicUsize::new(0);
    let results = std::sync::Mutex::new(Vec::with_capacity(games as usize));
    std::thread::scope(|scope| {
        for _ in 0..threads.clamp(1, games as usize) {
            scope.spawn(|| {
                let mut mine = Vec::new();
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let g = next.fetch_add(1, Ordering::Relaxed);
                    if g >= games as usize {
                        break;
                    }
                    mine.push((g, one_game(g as u32, decks, gauntlet, rules, seed)));
                    done.fetch_add(decks.len(), Ordering::Relaxed);
                }
                results.lock().unwrap().extend(mine);
            });
        }
    });
    for (g, w) in results.into_inner().unwrap() {
        for (d, won) in w.into_iter().enumerate() {
            wins[d][g] = won;
        }
    }
    Outcomes { games, wins }
}

/// Measures each variant of `base` against it, every one on the same `games` games
/// (`seed` fixes them). `progress` counts deck-games played.
pub fn paired(
    base: &Prepared,
    variants: &[Variant],
    gauntlet: &[Prepared],
    rules: &Rules,
    games: u32,
    seed: u64,
    progress: &AtomicUsize,
) -> Vec<Measure> {
    let built: Vec<Prepared> = variants.iter().map(|v| base.with_swaps(&v.swaps)).collect();
    let mut decks: Vec<&Prepared> = vec![base];
    decks.extend(built.iter());
    let out = play_all(
        &decks,
        gauntlet,
        rules,
        games,
        seed,
        progress,
        &AtomicBool::new(false),
    );
    (1..decks.len()).map(|v| out.diff(v, 0)).collect()
}
