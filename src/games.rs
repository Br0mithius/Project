//! Real games: what happened at the table, kept in `data/games.json` (personal data, only in
//! the user's data folder). Two uses: cards an opponent was seen playing are folded into
//! their deck for playtests (like trade picks), and the real record is compared with what
//! the simulator predicts - the program is only as strong as its simulator, and this is
//! how to tell when it's wrong.

use serde::{Deserialize, Serialize};
use std::path::Path;

pub const FILE: &str = "games.json";

/// One opponent in a logged game.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Seat {
    pub player: String,
    pub commander: String,
    /// Cards they were seen playing.
    pub seen: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameRecord {
    pub date: String,
    pub session: u32,
    pub our_commander: String,
    pub opponents: Vec<Seat>,
    pub won: bool,
    /// Who won (a player's name, or "me").
    pub winner: String,
    pub notes: String,
    /// Turns the game lasted (0 = not noted).
    pub turns: u32,
    /// How the mana went: "flooded", "fine" or "short" (empty = not noted).
    pub mana: String,
    /// The simulator's win rate for the deck against that table when the game was logged.
    pub predicted: Option<f32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameLog {
    pub games: Vec<GameRecord>,
}

/// The real record: games, wins, win rate and its standard error.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Record {
    pub played: usize,
    pub won: usize,
}

impl Record {
    pub fn rate(&self) -> f32 {
        if self.played == 0 {
            0.0
        } else {
            self.won as f32 / self.played as f32
        }
    }

    /// Standard error of the win rate (binomial; 0 with no games).
    pub fn se(&self) -> f32 {
        if self.played == 0 {
            return 0.0;
        }
        let p = self.rate().clamp(0.05, 0.95);
        (p * (1.0 - p) / self.played as f32).sqrt()
    }

    /// How the simulator's predicted win rate `predicted` sits against the real record:
    /// a short verdict once there are enough games to say anything.
    pub fn verdict(&self, predicted: f32) -> String {
        if self.played < 5 {
            return format!(
                "{} game{} logged: too few to judge the simulator yet (it predicts {:.0}%).",
                self.played,
                if self.played == 1 { "" } else { "s" },
                predicted * 100.0
            );
        }
        let z = (self.rate() - predicted) / self.se().max(0.01);
        let what = if z > 2.0 {
            "you win clearly more than the simulator predicts: it may undervalue your deck (or overvalue the opponents')"
        } else if z < -2.0 {
            "you win clearly less than the simulator predicts: it may overvalue your deck or misplay the opponents - worth a look"
        } else {
            "in line with the simulator, within the noise of so few games"
        };
        format!(
            "Real: {:.0}% ± {:.0} over {} games; simulator: {:.0}% - {}.",
            self.rate() * 100.0,
            self.se() * 100.0,
            self.played,
            predicted * 100.0,
            what
        )
    }
}

/// What the simulator expects for `deck` against the `table` decks: (win rate, average
/// game length in rounds), over `games` games with our seat rotating.
pub fn predict(
    deck: &crate::deck::Deck,
    table: &[crate::playtest::Prepared],
    db: &crate::db::CardDb,
    rules: &crate::db::Rules,
    games: u32,
) -> Option<(f32, f32)> {
    if table.is_empty() || deck.commander.is_none() {
        return None;
    }
    let me = crate::playtest::Prepared::new(deck, db);
    // The win rate from the same paired-playtest machinery every other number uses.
    let o = crate::playtest::lab::play_all(
        &[&me],
        table,
        rules,
        games,
        0x9A3E,
        &std::sync::atomic::AtomicUsize::new(0),
        &std::sync::atomic::AtomicBool::new(false),
    );
    // Game length from a smaller sample (well-mixed seeds: neighbouring seeds correlate).
    let n = games.min(200);
    let mut rounds = 0u64;
    for k in 0..n {
        let seat = (k % 4) as usize;
        let mut seats: Vec<&crate::playtest::Prepared> = (0..3)
            .map(|j| &table[(k as usize + j) % table.len()])
            .collect();
        seats.insert(seat, &me);
        let mut g = crate::playtest::state::Game::new(
            &seats,
            rules,
            crate::analysis::Rng::new(crate::playtest::lab::mix(0x9A3E_0000 + k as u64)),
        );
        crate::playtest::play(&mut g);
        rounds += g.round as u64;
    }
    Some((o.win_rate(0), rounds as f32 / n as f32))
}

impl GameLog {
    /// Real games against what the simulator said, in plain lines: win rate against the
    /// predictions stored with each game, game length, and how the mana went.
    pub fn calibration(&self, sim_rounds: Option<f32>) -> Vec<String> {
        let mut out = Vec::new();
        let with_pred: Vec<&GameRecord> = self
            .games
            .iter()
            .filter(|g| g.predicted.is_some())
            .collect();
        if !with_pred.is_empty() {
            let n = with_pred.len() as f32;
            let real = with_pred.iter().filter(|g| g.won).count() as f32 / n;
            let pred = with_pred.iter().filter_map(|g| g.predicted).sum::<f32>() / n;
            let se = (pred.clamp(0.05, 0.95) * (1.0 - pred.clamp(0.05, 0.95)) / n).sqrt();
            let z = (real - pred) / se.max(0.01);
            out.push(format!(
                "Win rate: real {:.0}% over {} games, simulator predicted {:.0}% - {}.",
                real * 100.0,
                with_pred.len(),
                pred * 100.0,
                if with_pred.len() < 5 {
                    "too few games to judge yet"
                } else if z > 2.0 {
                    "you win clearly more than predicted: the simulator underrates your deck or overrates theirs"
                } else if z < -2.0 {
                    "you win clearly less than predicted: the simulator overrates your deck - worth a look"
                } else {
                    "in line, within the noise of this few games"
                }
            ));
        }
        let timed: Vec<u32> = self
            .games
            .iter()
            .map(|g| g.turns)
            .filter(|t| *t > 0)
            .collect();
        if !timed.is_empty() {
            let avg = timed.iter().sum::<u32>() as f32 / timed.len() as f32;
            let mut line = format!(
                "Game length: real {:.1} turns over {} games",
                avg,
                timed.len()
            );
            if let Some(s) = sim_rounds {
                line += &format!(", simulated {:.1}", s);
                if timed.len() >= 4 && (avg - s).abs() >= 2.0 {
                    line += if avg < s {
                        " - real games end sooner: the simulator overvalues lands and late-game cards"
                    } else {
                        " - real games run longer: the simulator undervalues lands and late-game cards"
                    };
                }
            }
            out.push(line + ".");
        }
        let count = |m: &str| self.games.iter().filter(|g| g.mana == m).count();
        let (fl, fi, sh) = (count("flooded"), count("fine"), count("short"));
        if fl + fi + sh > 0 {
            let mut line = format!("Mana: flooded {} · fine {} · short {}", fl, fi, sh);
            if fl + fi + sh >= 4 {
                if fl > sh + 1 {
                    line += " - flooding more than running short: one land fewer is worth testing";
                } else if sh > fl + 1 {
                    line += " - running short more than flooding: check the colours (`mana`) before adding lands";
                }
            }
            out.push(line + ".");
        }
        out
    }

    pub fn load(dir: &Path) -> GameLog {
        std::fs::read_to_string(dir.join(FILE))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(dir.join(FILE), json + "\n").map_err(|e| e.to_string())
    }

    /// The record, overall or with one commander.
    pub fn record(&self, commander: Option<&str>) -> Record {
        let gs: Vec<&GameRecord> = self
            .games
            .iter()
            .filter(|g| commander.is_none_or(|c| g.our_commander == c))
            .collect();
        Record {
            played: gs.len(),
            won: gs.iter().filter(|g| g.won).count(),
        }
    }

    /// Wins and games against a player.
    pub fn against(&self, player: &str) -> Record {
        let key = player.trim().to_lowercase();
        let gs: Vec<&GameRecord> = self
            .games
            .iter()
            .filter(|g| {
                g.opponents
                    .iter()
                    .any(|s| s.player.trim().to_lowercase() == key)
            })
            .collect();
        Record {
            played: gs.len(),
            won: gs.iter().filter(|g| g.won).count(),
        }
    }

    /// Every card a player was seen playing, once each, oldest first.
    pub fn seen_by(&self, player: &str) -> Vec<String> {
        let key = player.trim().to_lowercase();
        let mut out: Vec<String> = Vec::new();
        for g in &self.games {
            for s in g
                .opponents
                .iter()
                .filter(|s| s.player.trim().to_lowercase() == key)
            {
                for c in &s.seen {
                    if !out.contains(c) {
                        out.push(c.clone());
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_seen_cards_and_round_trip() {
        let dir = std::env::temp_dir().join(format!("msh_games_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut log = GameLog::default();
        for (won, seen) in [(true, vec!["Opt"]), (false, vec!["Opt", "Refute"])] {
            log.games.push(GameRecord {
                our_commander: "Thanos, the Mad Titan".into(),
                opponents: vec![Seat {
                    player: "JD".into(),
                    commander: "The Mighty Thor, Jane Foster".into(),
                    seen: seen.into_iter().map(String::from).collect(),
                }],
                won,
                ..Default::default()
            });
        }
        log.save(&dir).unwrap();
        let back = GameLog::load(&dir);
        assert_eq!(back, log);
        assert_eq!(back.record(None), Record { played: 2, won: 1 });
        assert_eq!(back.against("jd").played, 2);
        assert_eq!(back.seen_by("JD"), ["Opt", "Refute"]);
        assert!(back.record(None).verdict(0.5).contains("too few"));
        // Calibration: six short, flooded losses against a 70% prediction read as a warning.
        let mut cal = GameLog::default();
        for _ in 0..6 {
            cal.games.push(GameRecord {
                won: false,
                turns: 7,
                mana: "flooded".into(),
                predicted: Some(0.7),
                ..Default::default()
            });
        }
        let lines = cal.calibration(Some(12.0)).join(" ");
        assert!(lines.contains("clearly less than predicted"), "{}", lines);
        assert!(lines.contains("real games end sooner"), "{}", lines);
        assert!(lines.contains("one land fewer"), "{}", lines);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
