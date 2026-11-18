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

impl GameLog {
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
        let _ = std::fs::remove_dir_all(&dir);
    }
}
