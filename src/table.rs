//! "My table": the user's real opponents - player name, commander and decklist - kept in
//! `data/table.json` (personal data: it lives only in the user's data folder). The ones
//! ticked as at the next table form a fixed gauntlet for playtests (`gauntlet`), so the
//! lab can test a deck against the decks it will actually meet.

use crate::db::{CardDb, Collection, Rules};
use crate::deck::Deck;
use crate::playtest::Prepared;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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
/// A card another player got in the trade market: a trade-pool pick, a shop or Skill Tree
/// purchase, or an auction win. Assumed to go in their deck when it fits their colours.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TradePick {
    pub card: String,
    /// Market round it was taken in.
    pub round: u32,
    /// "pick", "shop", "skill tree" or "auction".
    pub how: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Table {
    pub opponents: Vec<RealOpponent>,
    /// Every player's trade-market cards, by player name (as the league shows it).
    pub picks: BTreeMap<String, Vec<TradePick>>,
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

    /// The next table's playable decks (with their trade picks in), with the player names.
    pub fn next_decks(&self, db: &CardDb) -> Vec<(String, Deck)> {
        self.next_table()
            .filter_map(|o| Some((o.player.clone(), self.deck_of(o, db).ok()?)))
            .collect()
    }

    /// How many of the next table's decks are playable (cheap: no trade picks merged).
    pub fn next_count(&self, db: &CardDb) -> usize {
        self.next_table().filter(|o| o.playable(db).is_ok()).count()
    }

    /// The next table's players and commanders (cheap, for labels).
    pub fn next_names(&self, db: &CardDb) -> Vec<String> {
        self.next_table()
            .filter_map(|o| {
                let d = o.playable(db).ok()?;
                Some(format!(
                    "{} ({})",
                    o.player,
                    d.commander.unwrap_or_default()
                ))
            })
            .collect()
    }

    /// Replaces the trade picks of every player named in `text`: a Mana Mixer league
    /// overview (JSON), or a "Player:" line followed by one card per line. Returns how many
    /// players and cards were read.
    pub fn import_picks(&mut self, text: &str) -> Result<(usize, usize), String> {
        let got = if text.trim_start().starts_with('{') {
            picks_from_league(text)?
        } else {
            // "Player:" on a line of its own starts that player's list; every other line is
            // one card (names can hold commas and colons: "Aurelia, the Warleader",
            // "Avengers: Under Siege"). "Player: A; B; C" on one line also works.
            let mut got: BTreeMap<String, Vec<TradePick>> = BTreeMap::new();
            let mut current: Option<String> = None;
            let pick = |c: &str| {
                // "1 Card Name" lines from a decklist: drop the count.
                let c = c.trim();
                let c = match c.split_once(' ') {
                    Some((n, rest)) if n.trim_end_matches('x').parse::<u32>().is_ok() => {
                        rest.trim()
                    }
                    _ => c,
                };
                TradePick {
                    card: c.to_string(),
                    round: 0,
                    how: "pick".into(),
                }
            };
            for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
                if let Some(p) = line.strip_suffix(':') {
                    current = Some(p.trim().to_string());
                    got.entry(p.trim().to_string()).or_default();
                } else if let (Some((p, rest)), true) = (line.split_once(':'), line.contains(';')) {
                    current = Some(p.trim().to_string());
                    let e = got.entry(p.trim().to_string()).or_default();
                    e.extend(
                        rest.split(';')
                            .map(str::trim)
                            .filter(|c| !c.is_empty())
                            .map(pick),
                    );
                } else if let Some(p) = &current {
                    got.entry(p.clone()).or_default().push(pick(line));
                }
            }
            got
        };
        if got.is_empty() {
            return Err("No players' picks found in that text.".into());
        }
        let cards = got.values().map(|v| v.len()).sum();
        let players = got.len();
        for (p, v) in got {
            let key = |s: &str| s.trim().to_lowercase();
            self.picks.retain(|k, _| key(k) != key(&p));
            self.picks.insert(p, v);
        }
        Ok((players, cards))
    }

    /// A player's trade picks (name matched ignoring case and spaces).
    pub fn picks_of(&self, player: &str) -> &[TradePick] {
        let key = |s: &str| s.trim().to_lowercase();
        self.picks
            .iter()
            .find(|(k, _)| key(k) == key(player))
            .map(|(_, v)| v.as_slice())
            .unwrap_or(&[])
    }

    /// `o`'s playable deck with their trade picks in: each pick inside the commander's
    /// colours that isn't already there goes in, in place of the deck's weakest spell (a
    /// land pick replaces a basic land), so the deck stays the same size. Returns the deck.
    pub fn deck_of(&self, o: &RealOpponent, db: &CardDb) -> Result<Deck, String> {
        Ok(self.with_picks(o, o.playable(db)?, db).0)
    }

    /// `deck` with `o`'s fitting trade picks swapped in, and the names that went in.
    pub fn with_picks(&self, o: &RealOpponent, mut deck: Deck, db: &CardDb) -> (Deck, Vec<String>) {
        let Some(id) = deck.identity(db) else {
            return (deck, Vec::new());
        };
        let rules = Rules::default();
        let mut added = Vec::new();
        for p in self.picks_of(&o.player) {
            let Some(card) = db.get(&p.card) else {
                continue;
            };
            if deck.count_of(&card.name) > 0
                || deck.commander.as_deref() == Some(card.name.as_str())
                || !card.identity().is_subset_of(id)
            {
                continue;
            }
            let out = if card.is_land() {
                deck.cards
                    .keys()
                    .find(|n| db.get(n).is_some_and(|c| c.is_basic()))
                    .cloned()
            } else {
                // Never swap out a picked card (one just added, or one they had already).
                let picked = |n: &str| self.picks_of(&o.player).iter().any(|p| p.card == n);
                crate::analysis::weakest(&deck, db, &rules)
                    .iter()
                    .find(|w| !picked(&w.card.name))
                    .map(|w| w.card.name.clone())
            };
            let Some(out) = out else { continue };
            deck.remove(&out, 1);
            deck.add(&card.name, 1);
            added.push(card.name.clone());
        }
        (deck, added)
    }
}

/// Every player's trade-market cards from a Mana Mixer league overview (its "results":
/// trade-pool picks - auction wins included - and shop / Skill Tree purchases). A pick the
/// player traded back in a later round is dropped. Pack rerolls aren't cards.
pub fn picks_from_league(json: &str) -> Result<BTreeMap<String, Vec<TradePick>>, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let results = v.get("results").unwrap_or(&v);
    let picks = results
        .get("picks")
        .and_then(|p| p.as_object())
        .ok_or("no \"results.picks\" in the league data")?;
    let round =
        |x: &serde_json::Value| x.get("roundNumber").and_then(|r| r.as_u64()).unwrap_or(0) as u32;
    let mut out: BTreeMap<String, Vec<TradePick>> = BTreeMap::new();
    for (player, list) in picks {
        let traded_in: Vec<(String, u32)> = results
            .get("tradeIns")
            .and_then(|t| t.get(player))
            .and_then(|t| t.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| Some((x.get("cardName")?.as_str()?.to_string(), round(x))))
                    .collect()
            })
            .unwrap_or_default();
        let mut mine: Vec<TradePick> = Vec::new();
        for x in list.as_array().into_iter().flatten() {
            let Some(name) = x.get("cardName").and_then(|n| n.as_str()) else {
                continue;
            };
            let r = round(x);
            if traded_in.iter().any(|(n, tr)| n == name && *tr > r) {
                continue;
            }
            mine.push(TradePick {
                card: name.to_string(),
                round: r,
                how: "pick".into(),
            });
        }
        for x in results
            .get("extras")
            .and_then(|e| e.get(player))
            .and_then(|e| e.as_array())
            .into_iter()
            .flatten()
        {
            let how = match x.get("kind").and_then(|k| k.as_str()) {
                Some("SHOP") => "shop",
                Some("SKILL_TREE") => "skill tree",
                _ => continue,
            };
            let Some(name) = x.get("name").and_then(|n| n.as_str()) else {
                continue;
            };
            mine.push(TradePick {
                card: name.to_string(),
                round: round(x),
                how: how.into(),
            });
        }
        out.insert(player.clone(), mine);
    }
    Ok(out)
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
    fn league_picks_import_and_merge() {
        let json = r#"{"results":{"picks":{"Drake":[
            {"cardName":"Llanowar Elves","roundNumber":1},
            {"cardName":"Pelakka Wurm","roundNumber":1},
            {"cardName":"Hero's Downfall","roundNumber":2}]},
          "tradeIns":{"Drake":[{"cardName":"Pelakka Wurm","roundNumber":2}]},
          "extras":{"Drake":[{"kind":"SKILL_TREE","name":"Sol Ring","roundNumber":1},
            {"kind":"REROLL","name":"Pack Reroll","roundNumber":1}]}}}"#;
        let picks = picks_from_league(json).unwrap();
        let names: Vec<&str> = picks["Drake"].iter().map(|p| p.card.as_str()).collect();
        // Pelakka Wurm was traded back in a later round; rerolls aren't cards.
        assert_eq!(names, ["Llanowar Elves", "Hero's Downfall", "Sol Ring"]);
        let mut t = Table::default();
        let (players, cards) = t
            .import_picks("Thomas:\nAurelia, the Warleader\nAvengers: Under Siege\n\nJD:\n1 Opt\n")
            .unwrap();
        assert_eq!((players, cards), (2, 3));
        assert_eq!(t.picks_of("thomas")[1].card, "Avengers: Under Siege");
        assert_eq!(t.picks_of("JD")[0].card, "Opt");
        let db = db();
        let table = Table {
            opponents: vec![],
            picks,
        };
        let o = RealOpponent {
            player: "drake".into(),
            commander: "Tatyova, Benthic Druid".into(),
            ..Default::default()
        };
        let base = strong_build("Tatyova, Benthic Druid", &db, &Rules::default()).unwrap();
        let mut base = base;
        base.remove("Llanowar Elves", 9);
        base.remove("Sol Ring", 9);
        if base.main_count() < 59 {
            base.add("Forest", 59 - base.main_count());
        }
        let size = base.main_count();
        let (deck, added) = table.with_picks(&o, base, &db);
        // In colours: Elves and Sol Ring go in; Hero's Downfall (black) doesn't.
        assert!(added.contains(&"Llanowar Elves".to_string()), "{:?}", added);
        assert!(added.contains(&"Sol Ring".to_string()), "{:?}", added);
        assert!(!added.contains(&"Hero's Downfall".to_string()));
        assert_eq!(deck.main_count(), size);
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
            ..Default::default()
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
            ..Default::default()
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
