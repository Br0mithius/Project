//! "My table": the user's real opponents - player name, commander and decklist - kept in
//! `data/table.json` (personal data: it lives only in the user's data folder). The ones
//! ticked as at the next table form a fixed gauntlet for playtests (`gauntlet`), so the
//! lab can test a deck against the decks it will actually meet.

use crate::db::{CardDb, Collection, Rules};
use crate::deck::Deck;
use crate::playtest::Prepared;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const FILE: &str = "table.json";

/// One real opponent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RealOpponent {
    pub player: String,
    /// Their commander (also read from the decklist's Commander section when it has one).
    pub commander: String,
    /// Their decklist ("count name" lines, a Commander section optional).
    pub decklist: String,
    /// Sitting at the next table: played against when testing "my next table".
    pub at_next_table: bool,
}

impl RealOpponent {
    /// Their deck: the decklist, with `commander` as the commander when the list names none.
    pub fn deck(&self, db: &CardDb) -> Deck {
        let name = if self.player.trim().is_empty() {
            "opponent"
        } else {
            self.player.trim()
        };
        let mut d = Deck::from_text(name, &self.decklist, db);
        if d.commander.is_none() && !self.commander.trim().is_empty() {
            let c = self.commander.trim();
            d.commander = Some(db.get(c).map(|c| c.name.clone()).unwrap_or(c.to_string()));
            // A list that has the commander among its cards: it's in the command zone.
            if let Some(cmd) = d.commander.clone() {
                d.remove(&cmd, 1);
            }
        }
        d
    }

    /// Can it be played: a known commander and some cards.
    pub fn playable(&self, db: &CardDb) -> Result<Deck, String> {
        let d = self.deck(db);
        match d.commander.as_deref() {
            None => Err("no commander".into()),
            Some(c) if db.get(c).is_none() => Err(format!("unknown commander {}", c)),
            Some(_) if d.main_count() < 20 => Err(format!("only {} cards", d.main_count())),
            Some(_) => Ok(d),
        }
    }
}

/// The user's real opponents.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Table {
    pub opponents: Vec<RealOpponent>,
}

impl Table {
    /// `dir/table.json`, or an empty table.
    pub fn load(dir: &Path) -> Table {
        std::fs::read_to_string(dir.join(FILE))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(dir.join(FILE), json + "\n").map_err(|e| e.to_string())
    }

    /// The opponents ticked as at the next table.
    pub fn next_table(&self) -> impl Iterator<Item = &RealOpponent> {
        self.opponents.iter().filter(|o| o.at_next_table)
    }

    /// The next table's playable decks, with the player names.
    pub fn next_decks(&self, db: &CardDb) -> Vec<(String, Deck)> {
        self.next_table()
            .filter_map(|o| Some((o.player.clone(), o.playable(db).ok()?)))
            .collect()
    }
}

/// The fixed gauntlet the next table's decks make (every game seats them all when the pod
/// has room, in shuffled seats - `playtest::lab::play_all`).
pub fn gauntlet(table: &Table, db: &CardDb) -> Vec<Prepared> {
    table
        .next_decks(db)
        .iter()
        .map(|(_, d)| Prepared::new(d, db))
        .collect()
}

/// A strong version of `commander`'s deck: auto-build from every league card (one copy
/// each), the way an opponent with a deep pool would build it.
pub fn strong_build(commander: &str, db: &CardDb, rules: &Rules) -> Result<Deck, String> {
    let cmd = db
        .get(commander.trim())
        .ok_or_else(|| format!("{} isn't in the card database.", commander.trim()))?;
    if !cmd.can_be_commander() {
        return Err(format!("{} can't be a commander.", cmd.name));
    }
    let all = Collection::new(
        std::path::PathBuf::new(),
        db.cards.iter().map(|c| (c.name.clone(), 1)).collect(),
    );
    let (mut d, _, _) = crate::archetypes::autobuild(cmd, db, &all, rules, None, None);
    d.name = format!("strong {}", cmd.name);
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn db() -> CardDb {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
        CardDb::load(&dir.join("cards.json")).unwrap()
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("msh_table_{}_{}", tag, std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn table_json_round_trip() {
        let dir = scratch("rt");
        assert_eq!(Table::load(&dir), Table::default());
        let t = Table {
            opponents: vec![
                RealOpponent {
                    player: "Alice".into(),
                    commander: "Gandalf, Wandering Wizard".into(),
                    decklist: "Commander\n1 Gandalf, Wandering Wizard\n\nDeck\n1 Sol Ring\n".into(),
                    at_next_table: true,
                },
                RealOpponent {
                    player: "Bob".into(),
                    ..Default::default()
                },
            ],
        };
        t.save(&dir).unwrap();
        assert_eq!(Table::load(&dir), t);
        // Missing fields read as defaults.
        std::fs::write(dir.join(FILE), r#"{"opponents":[{"player":"C"}]}"#).unwrap();
        let t2 = Table::load(&dir);
        assert_eq!(t2.opponents[0].player, "C");
        assert!(!t2.opponents[0].at_next_table);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn next_table_gauntlet_uses_only_ticked_playable_decks() {
        let db = db();
        let rules = Rules::default();
        let cmd = db
            .cards
            .iter()
            .filter(|c| c.can_be_commander())
            .map(|c| c.name.clone())
            .next()
            .unwrap();
        let strong = strong_build(&cmd, &db, &rules).unwrap();
        assert_eq!(strong.commander.as_deref(), Some(cmd.as_str()));
        assert!(strong.main_count() >= 50);
        // A list without a Commander section, with the commander given apart.
        let no_section: String = strong
            .cards
            .iter()
            .map(|(n, k)| format!("{} {}\n", k, n))
            .collect();
        let t = Table {
            opponents: vec![
                RealOpponent {
                    player: "A".into(),
                    commander: String::new(),
                    decklist: strong.to_text(),
                    at_next_table: true,
                },
                RealOpponent {
                    player: "B".into(),
                    commander: cmd.clone(),
                    decklist: no_section,
                    at_next_table: true,
                },
                RealOpponent {
                    player: "not at the table".into(),
                    commander: cmd.clone(),
                    decklist: strong.to_text(),
                    at_next_table: false,
                },
                RealOpponent {
                    player: "no list".into(),
                    commander: cmd.clone(),
                    decklist: String::new(),
                    at_next_table: true,
                },
            ],
        };
        let decks = t.next_decks(&db);
        assert_eq!(
            decks.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(),
            vec!["A", "B"]
        );
        assert_eq!(decks[0].1.cards, decks[1].1.cards);
        assert_eq!(decks[1].1.commander.as_deref(), Some(cmd.as_str()));
        let g = gauntlet(&t, &db);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].commanders_names(), vec![cmd.clone()]);
        assert!(t.opponents[3].playable(&db).is_err());
    }
}
