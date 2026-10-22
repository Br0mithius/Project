//! Mulligans Island Commander League, Season 2: schedule, session rules, Value Points,
//! draft-pick ranking, trade-in planning and Mana Mixer export.

use crate::analysis::{self, available, score_card};
use crate::card::Card;
use crate::db::{CardDb, Collection, Rules};
use crate::deck::Deck;

pub struct Session {
    pub number: u32,
    pub date: (i32, u32, u32),
    pub label: &'static str,
}

pub const SESSIONS: [Session; 13] = [
    Session {
        number: 1,
        date: (2026, 10, 8),
        label: "Kickoff - 9 starter packs, opened & built same day",
    },
    Session {
        number: 2,
        date: (2026, 10, 22),
        label: "Starter set week 2 - 1 pack (drafted)",
    },
    Session {
        number: 3,
        date: (2026, 11, 5),
        label: "Starter set week 3 - 1 pack (drafted)",
    },
    Session {
        number: 4,
        date: (2026, 11, 19),
        label: "Starter set week 4 - 1 pack (drafted)",
    },
    Session {
        number: 5,
        date: (2026, 12, 3),
        label: "Reality Fracture launch - 3 packs given",
    },
    Session {
        number: 6,
        date: (2026, 12, 17),
        label: "Reality Fracture week 2 - singleton starts",
    },
    Session {
        number: 7,
        date: (2027, 1, 14),
        label: "Reality Fracture week 3",
    },
    Session {
        number: 8,
        date: (2027, 1, 28),
        label: "Reality Fracture week 4",
    },
    Session {
        number: 9,
        date: (2027, 2, 11),
        label: "Nauctis launch - 3 packs given",
    },
    Session {
        number: 10,
        date: (2027, 2, 25),
        label: "Nauctis week 2 - decks become 75 cards",
    },
    Session {
        number: 11,
        date: (2027, 3, 11),
        label: "Nauctis week 3",
    },
    Session {
        number: 12,
        date: (2027, 3, 25),
        label: "Nauctis week 4",
    },
    Session {
        number: 13,
        date: (2027, 4, 8),
        label: "Season Finale tournament (75 cards)",
    },
];

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

impl Session {
    pub fn date_label(&self) -> String {
        let (y, m, d) = self.date;
        format!("{} {}, {}", MONTHS[(m - 1) as usize], d, y)
    }

    /// Deck size and singleton for this session (§1 Format Progression).
    pub fn deck_rules(&self) -> (u32, bool) {
        match self.number {
            1..=5 => (60, false),
            6..=9 => (60, true),
            _ => (75, true),
        }
    }
}

/// Days since 1970-01-01 for a calendar date (Howard Hinnant's algorithm).
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn today_days() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64 / 86400)
        .unwrap_or(0)
}

/// The next session on or after today (the one you're building for); the finale after that.
pub fn upcoming_session() -> &'static Session {
    let today = today_days();
    SESSIONS
        .iter()
        .find(|s| days_from_civil(s.date.0, s.date.1, s.date.2) >= today)
        .unwrap_or(&SESSIONS[12])
}

pub fn session(n: u32) -> Option<&'static Session> {
    SESSIONS.iter().find(|s| s.number == n)
}

/// Sets deck size / singleton for a session. Everything else (commander counts toward
/// the 60, unlimited basics, colour identity, pool-only) is the same all season.
pub fn apply_session(rules: &mut Rules, n: u32) -> Result<&'static Session, String> {
    let s = session(n).ok_or_else(|| format!("Sessions are numbered 1-13 (got {}).", n))?;
    let (size, singleton) = s.deck_rules();
    rules.deck_size = size;
    rules.singleton = singleton;
    rules.max_copies = 0;
    rules.commander_counts_toward_size = true;
    rules.unlimited_basics = true;
    rules.enforce_color_identity = true;
    rules.enforce_collection = true;
    rules.session = n;
    Ok(s)
}

/// §4: Mythic 8, Rare 4, Uncommon 2, Common 1.
pub fn vp_value(card: &Card) -> u32 {
    match card.rarity.to_lowercase().as_str() {
        "mythic" => 8,
        "rare" => 4,
        "uncommon" => 2,
        "common" => 1,
        _ => 0,
    }
}

/// Claiming from the Public Pool costs twice the card's VP value.
pub fn claim_cost(card: &Card) -> u32 {
    vp_value(card) * 2
}

/// §5 Skill Tree: the k-th staple claimed (1-based) costs 5k VP.
pub fn staple_cost(k: u32) -> u32 {
    5 * k
}

pub const STAPLES: [&str; 9] = [
    "Sol Ring",
    "Arcane Signet",
    "Command Tower",
    "Thought Vessel",
    "Mind Stone",
    "Commander's Sphere",
    "Fellwar Stone",
    "Exotic Orchard",
    "Path of Ancestry",
];

/// §5 Shop: one copy of each, 30 VP, one purchase per session.
pub const SHOP: [(&str, [&str; 3]); 7] = [
    ("Land", ["Rogue's Passage", "Command Beacon", "Bojuka Bog"]),
    (
        "Colorless",
        ["Swiftfoot Boots", "Chromatic Lantern", "Worn Powerstone"],
    ),
    (
        "White",
        ["Swords to Plowshares", "Austere Command", "Monologue Tax"],
    ),
    ("Blue", ["Counterspell", "Pongify", "Propaganda"]),
    (
        "Black",
        ["Feed the Swarm", "Crux of Fate", "Phyrexian Arena"],
    ),
    ("Red", ["Blasphemous Act", "Chaos Warp", "Vandalblast"]),
    ("Green", ["Lull", "Beast Within", "Cultivate"]),
];

pub struct DraftPick<'a> {
    pub card: &'a Card,
    pub total: f32,
    pub fit: f32,
    pub power: f32,
    pub vp: u32,
    pub reasons: Vec<String>,
}

/// Ranks the cards in a pack for the current deck. Fit to your deck dominates; trade value
/// (VP) and raw card strength (worth taking so a table-mate can't) break ties.
/// Without a commander, `options` are the auto-built decks of your best commander options
/// (from the commander ranking) and each card is judged against the one it fits best.
pub fn rank_pack<'a>(
    pack: &[&'a Card],
    deck: &Deck,
    db: &CardDb,
    coll: &Collection,
    rules: &Rules,
    options: &[Deck],
) -> Vec<DraftPick<'a>> {
    let blank = Deck::new("blank");
    let identity = deck.identity(db);
    let probes: Vec<Deck> = if deck.commander.is_some() {
        vec![deck.clone()]
    } else {
        options.to_vec()
    };
    // The weakest spell in each full deck: a pick that beats it is an upgrade.
    let weakest: Vec<Option<(String, f32)>> = probes
        .iter()
        .map(|p| {
            if p.main_count() + 5 < p.main_target(rules) {
                return None;
            }
            analysis::weakest(p, db, rules)
                .first()
                .map(|s| (s.card.name.clone(), s.score))
        })
        .collect();
    let mut out: Vec<DraftPick> = pack
        .iter()
        .map(|&card| {
            let mut reasons = Vec::new();
            let power = score_card(card, &blank, db, rules).0;
            let mut fit = f32::MIN;
            let mut fit_reasons = Vec::new();
            for probe in &probes {
                let id = probe.identity(db).unwrap_or_default();
                if !card.is_land() && !card.identity().is_subset_of(id) {
                    continue;
                }
                let (s, why) = if card.is_land() {
                    let useful = card.produces().0 & id.0 != 0;
                    (
                        if useful { 2.0 } else { 0.0 },
                        vec!["mana for your colours".to_string()],
                    )
                } else {
                    score_card(card, probe, db, rules)
                };
                if s > fit {
                    fit = s;
                    fit_reasons = why;
                    let pi = probes
                        .iter()
                        .position(|p| std::ptr::eq(p, probe))
                        .unwrap_or(0);
                    if let Some((name, w)) = &weakest[pi] {
                        if s > *w && !card.is_land() && *name != card.name {
                            fit_reasons.insert(0, format!("upgrade over {} ({:.1})", name, w));
                        }
                    }
                    if deck.commander.is_none() {
                        fit_reasons.insert(
                            0,
                            format!("for {}", probe.commander.clone().unwrap_or_default()),
                        );
                    }
                }
            }
            if probes.is_empty() {
                // Empty pool: nothing to fit yet, so judge raw strength.
                fit = power;
                reasons.push("no commander options yet - judged on raw strength".into());
            } else if fit == f32::MIN {
                fit = 0.0;
                reasons.push(match identity {
                    Some(id) => format!("outside your colours ({})", id),
                    None => "outside your best commanders' colours".into(),
                });
            } else {
                reasons.append(&mut fit_reasons);
            }
            let owned = coll.owned(&card.name);
            if rules.singleton && owned > 0 && !card.is_basic() {
                fit *= 0.3;
                reasons.push("you already own one (singleton)".into());
            }
            if card.can_be_commander() {
                reasons.push("possible commander".into());
            }
            let vp = vp_value(card);
            let total = fit + 0.25 * vp as f32 + 0.2 * power;
            DraftPick {
                card,
                total,
                fit,
                power,
                vp,
                reasons,
            }
        })
        .collect();
    out.sort_by(|a, b| b.total.partial_cmp(&a.total).unwrap());
    out
}

pub struct TradeIn<'a> {
    pub card: &'a Card,
    pub copies: u32,
    pub vp: u32,
    /// How much the card could still be worth to you (0 = dead weight).
    pub keep_value: f32,
    pub why: String,
}

/// Picks spare cards (owned copies the deck doesn't use) to trade into the Public Pool,
/// reaching at least `target` VP while giving up as little as possible. `need_rarity`
/// adds the §4 rarity floor for a claim you're planning ("rare" or "mythic").
pub fn plan_trade_in<'a>(
    deck: &Deck,
    db: &'a CardDb,
    coll: &Collection,
    rules: &Rules,
    target: u32,
    need_rarity: Option<&str>,
) -> Vec<TradeIn<'a>> {
    let identity = deck.identity(db);
    let singleton_soon = !rules.singleton;
    let blank = Deck::new("blank");
    let mut spares: Vec<TradeIn> = Vec::new();
    for card in db.cards.iter().filter(|c| !c.is_basic()) {
        let owned = coll.owned(&card.name);
        let used = deck.used(&card.name);
        if owned <= used || vp_value(card) == 0 {
            continue;
        }
        let fits = card.is_land()
            || identity
                .map(|id| card.identity().is_subset_of(id))
                .unwrap_or(true);
        // Off-colour cards still have some value: general strength, and they might lead or
        // join a future deck as the pool grows.
        let general = score_card(card, &blank, db, rules).0.max(0.0);
        let mut keep = if fits && !card.is_land() {
            score_card(card, deck, db, rules).0.max(0.0)
        } else {
            0.5 * general + if card.can_be_commander() { 1.5 } else { 0.0 }
        };
        let why = if !fits {
            "outside your colours".to_string()
        } else if used >= 1 && singleton_soon {
            keep *= 0.3;
            "extra copy - dead once singleton starts (Session 6)".to_string()
        } else if used >= 1 {
            keep *= 0.3;
            "extra copy".to_string()
        } else {
            "not in your deck".to_string()
        };
        for _ in used..owned {
            spares.push(TradeIn {
                card,
                copies: 1,
                vp: vp_value(card),
                keep_value: keep,
                why: why.clone(),
            });
        }
    }
    // Cheapest loss per VP first.
    spares.sort_by(|a, b| {
        (a.keep_value / a.vp as f32)
            .partial_cmp(&(b.keep_value / b.vp as f32))
            .unwrap()
            .then(b.vp.cmp(&a.vp))
    });
    // Rarity floor first, if a rare/mythic claim is planned: from spares when they cover it,
    // otherwise topped up with the deck's least valuable cards of that rarity (said so in `why`).
    let mut picked: Vec<TradeIn> = Vec::new();
    let floor: Vec<(&str, usize)> = match need_rarity {
        // 2 rares also satisfy a mythic claim; whichever costs the deck less.
        Some("mythic") => vec![("mythic", 1), ("rare", 2)],
        Some("rare") => vec![("rare", 1)],
        _ => vec![],
    };
    let in_deck = |rarity: &str| -> Vec<TradeIn<'a>> {
        let commanders = deck.commanders();
        let mut v: Vec<TradeIn> = db
            .cards
            .iter()
            .filter(|c| c.rarity == rarity && !c.is_basic())
            .filter(|c| !commanders.iter().any(|n| **n == c.name))
            .filter(|c| deck.used(&c.name) > 0 && coll.owned(&c.name) > 0)
            .map(|card| TradeIn {
                card,
                copies: 1,
                vp: vp_value(card),
                keep_value: score_card(card, deck, db, rules).0.max(0.0),
                why: "from your deck - the rarity floor needs it".to_string(),
            })
            .collect();
        v.sort_by(|a, b| a.keep_value.partial_cmp(&b.keep_value).unwrap());
        v
    };
    // Each way of meeting the floor: (spare indices, deck cards), cheapest to keep first.
    let options: Vec<(Vec<usize>, Vec<TradeIn>)> = floor
        .iter()
        .filter_map(|&(rarity, n)| {
            let spare: Vec<usize> = spares
                .iter()
                .enumerate()
                .filter(|(_, t)| t.card.rarity == rarity)
                .map(|(i, _)| i)
                .take(n)
                .collect();
            let from_deck: Vec<TradeIn> = in_deck(rarity).into_iter().take(n - spare.len()).collect();
            (spare.len() + from_deck.len() == n).then_some((spare, from_deck))
        })
        .collect();
    let lost = |o: &(Vec<usize>, Vec<TradeIn>)| -> f32 {
        // Spares cost what they'd still be worth; deck cards cost a lot more (the deck gets worse).
        o.0.iter().map(|&i| spares[i].keep_value).sum::<f32>()
            + o.1.iter().map(|t| 10.0 + t.keep_value).sum::<f32>()
    };
    if let Some((spare, from_deck)) = options
        .into_iter()
        .min_by(|a, b| lost(a).partial_cmp(&lost(b)).unwrap())
    {
        for i in spare.into_iter().rev() {
            picked.push(spares.remove(i));
        }
        picked.extend(from_deck);
    }
    let floor_count = picked.len();
    let mut total: u32 = picked.iter().map(|t| t.vp).sum();
    for t in spares {
        if total >= target {
            break;
        }
        total += t.vp;
        picked.push(t);
    }
    // Drop anything not needed to stay at the target, most valuable to keep first.
    let mut order: Vec<usize> = (floor_count..picked.len()).collect();
    order.sort_by(|&a, &b| {
        picked[b]
            .keep_value
            .partial_cmp(&picked[a].keep_value)
            .unwrap()
    });
    let mut drop = Vec::new();
    for i in order {
        if total - picked[i].vp >= target {
            total -= picked[i].vp;
            drop.push(i);
        }
    }
    drop.sort_unstable_by(|a, b| b.cmp(a));
    for i in drop {
        picked.remove(i);
    }
    // Merge copies of the same card.
    let mut merged: Vec<TradeIn> = Vec::new();
    for t in picked {
        match merged.iter_mut().find(|m| m.card.name == t.card.name) {
            Some(m) => {
                m.copies += 1;
                m.vp += t.vp;
            }
            None => merged.push(t),
        }
    }
    merged
}

/// " (from your deck)" for a trade-in the rarity floor had to take from the deck, else "".
pub fn deck_note(t: &TradeIn) -> &'static str {
    if t.why.starts_with("from your deck") {
        " (from your deck)"
    } else {
        ""
    }
}

/// Cards you'd have to cut when singleton starts (extra copies in the deck).
pub fn singleton_conflicts(deck: &Deck, db: &CardDb) -> Vec<(String, u32)> {
    deck.cards
        .iter()
        .filter(|(n, c)| **c > 1 && db.get(n).map(|x| !x.is_basic()).unwrap_or(true))
        .map(|(n, c)| (n.clone(), *c))
        .collect()
}

/// "1 Red Hulk (MSH) 149" lines - the ManaBox-style text list Mana Mixer takes.
/// One line in Mana Mixer's format: "count Name (SET) number", with two-faced cards named
/// "Front // Back" the way Mana Mixer lists them.
pub fn mana_mixer_line(card: Option<&Card>, name: &str, n: u32) -> String {
    match card {
        Some(c) if !c.set.is_empty() && c.set != "LEAGUE" => match &c.collector_number {
            Some(num) => format!("{} {} ({}) {}", n, c.full_name(), c.set, num),
            None => format!("{} {} ({})", n, c.full_name(), c.set),
        },
        Some(c) => format!("{} {}", n, c.full_name()),
        _ => format!("{} {}", n, name),
    }
}

pub fn export_pool(db: &CardDb, coll: &Collection) -> String {
    coll.counts
        .iter()
        .map(|(name, n)| mana_mixer_line(db.get(name), name, *n) + "\n")
        .collect()
}

pub fn export_deck(deck: &Deck, db: &CardDb) -> String {
    let mut out = String::new();
    for c in deck.commanders() {
        out += &(mana_mixer_line(db.get(c), c, 1) + "\n");
    }
    for (name, n) in &deck.cards {
        out += &(mana_mixer_line(db.get(name), name, *n) + "\n");
    }
    out
}

/// How many more of a card you could take into the deck right now.
pub fn can_add(card: &Card, deck: &Deck, coll: &Collection, rules: &Rules) -> u32 {
    available(card, deck, coll, rules)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(
            days_from_civil(2026, 10, 22) - days_from_civil(2026, 10, 8),
            14
        );
        assert_eq!(session(6).unwrap().deck_rules(), (60, true));
        assert_eq!(session(10).unwrap().deck_rules(), (75, true));
        assert_eq!(session(3).unwrap().deck_rules(), (60, false));
    }

    #[test]
    fn session_rules_apply() {
        let mut r = Rules::default();
        apply_session(&mut r, 11).unwrap();
        assert_eq!((r.deck_size, r.singleton, r.session), (75, true, 11));
        assert!(apply_session(&mut r, 14).is_err());
        assert_eq!(staple_cost(9), 45);
        assert_eq!((1..=9).map(staple_cost).sum::<u32>(), 225);
    }

    #[test]
    fn mythic_floor_takes_a_deck_rare_when_spares_are_short() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
        let db = CardDb::load(&dir.join("cards.json")).unwrap();
        let rules = Rules::default();
        let mut deck = Deck::new("t");
        deck.commander = Some("Thanos, the Mad Titan".into());
        for n in ["Construct a Cosmic Cube", "Hawkeye, Master Marksman", "Ronin, Shadow Stalker"] {
            deck.add(n, 1);
        }
        let mut counts: std::collections::BTreeMap<String, u32> = [
            "Thanos, the Mad Titan",
            "Construct a Cosmic Cube",
            "Hawkeye, Master Marksman",
            "Ronin, Shadow Stalker",
            "Nick Fury, Agent of S.H.I.E.L.D.",
            "Kree Commandos",
            "Super Suit",
        ]
        .iter()
        .map(|n| (n.to_string(), 1))
        .collect();
        let coll = Collection::new(dir.join("none.txt"), counts.clone());
        let plan = plan_trade_in(&deck, &db, &coll, &rules, 16, Some("mythic"));
        let rares: Vec<&TradeIn> = plan.iter().filter(|t| t.card.rarity == "rare").collect();
        assert_eq!(rares.len(), 2, "one spare rare + one from the deck");
        assert!(rares.iter().any(|t| t.card.name.starts_with("Nick Fury")));
        assert_eq!(plan.iter().filter(|t| !deck_note(t).is_empty()).count(), 1);
        assert!(!plan.iter().any(|t| t.card.name.starts_with("Thanos")));
        // With two spare rares the deck is left alone.
        counts.insert("Vision Quest".into(), 1);
        let coll = Collection::new(dir.join("none.txt"), counts);
        let plan = plan_trade_in(&deck, &db, &coll, &rules, 16, Some("mythic"));
        assert_eq!(plan.iter().filter(|t| t.card.rarity == "rare").count(), 2);
        assert!(plan.iter().all(|t| deck_note(t).is_empty()));
    }
}

/// One line of an imported card list.
#[derive(Debug, PartialEq)]
pub struct ImportLine {
    pub count: u32,
    pub name: String,
}

/// Turns a dropped or opened file into text: UTF-8 (with or without a byte-order mark) or
/// UTF-16, which Windows Notepad and Excel sometimes save.
pub fn decode_text(bytes: &[u8]) -> String {
    let utf16 = |be: bool| {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| {
                if be {
                    u16::from_be_bytes([c[0], c[1]])
                } else {
                    u16::from_le_bytes([c[0], c[1]])
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xFF, 0xFE, ..] => utf16(false),
        [0xFE, 0xFF, ..] => utf16(true),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// Splits a CSV line on `sep`, honouring quoted fields ("Ka-Zar, of the..." stays one field).
fn csv_fields_by(line: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == sep && !quoted => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter().map(|f| f.trim().to_string()).collect()
}

/// Reads a Mana Mixer / ManaBox export: text lines like "1 Red Hulk (MSH) 149",
/// "2x Web Up", "Web Up", or a CSV with Name and Quantity columns. Repeated cards are summed.
pub fn parse_import(text: &str) -> Vec<ImportLine> {
    let text = text
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut lines = text.lines().filter(|l| !l.trim().is_empty()).peekable();
    let mut out: Vec<ImportLine> = Vec::new();
    let mut push = |count: u32, name: String| {
        // "Front // Back" names (Adventures, double-faced cards): the front face names the card.
        let name = name
            .split(" // ")
            .next()
            .unwrap_or(&name)
            .trim()
            .to_string();
        if name.is_empty() || count == 0 {
            return;
        }
        match out.iter_mut().find(|l| l.name.eq_ignore_ascii_case(&name)) {
            Some(l) => l.count += count,
            None => out.push(ImportLine { count, name }),
        }
    };
    // CSV with a header row?
    if let Some(first) = lines.peek() {
        // Comma, semicolon (European Excel) or tab (copied from a spreadsheet or web table).
        let sep = [',', ';', '\t']
            .into_iter()
            .max_by_key(|c| first.matches(*c).count())
            .unwrap_or(',');
        let header: Vec<String> = csv_fields_by(&first.to_lowercase(), sep)
            .into_iter()
            .map(|h| h.replace(['_', '-'], " "))
            .collect();
        let col = |names: &[&str]| header.iter().position(|h| names.contains(&h.as_str()));
        if let Some(name_col) = col(&["name", "card name", "cardname", "card"]) {
            if first.contains(sep) {
                let qty_col = col(&[
                    "quantity", "qty", "count", "amount", "copies", "owned", "have",
                ]);
                lines.next();
                for l in lines {
                    let f = csv_fields_by(l, sep);
                    let n = qty_col
                        .and_then(|q| f.get(q))
                        .and_then(|q| q.parse().ok())
                        .unwrap_or(1);
                    if let Some(name) = f.get(name_col) {
                        push(n, name.clone());
                    }
                }
                return out;
            }
        }
    }
    for l in lines {
        let l = l.trim();
        if l.starts_with('#') || l.starts_with("//") {
            continue;
        }
        let low = l.to_lowercase();
        if ["deck", "commander", "sideboard", "maybeboard"].contains(&low.trim_end_matches(':')) {
            continue;
        }
        let Some((n, rest)) = crate::db::parse_count_line(l) else {
            continue;
        };
        // Drop " (MSH) 149", foil markers like "*F*", and trailing set codes.
        let mut name = rest.as_str();
        for mark in [" (", " [", "\t"] {
            if let Some(i) = name.find(mark) {
                name = &name[..i];
            }
        }
        let name = name.trim_end_matches("*F*").trim_end_matches("*E*").trim();
        // "Red Hulk x2" puts the count at the end.
        if n == 1 {
            if let Some((front, count)) = name.rsplit_once(' ') {
                if let Ok(c) = count.trim_start_matches(['x', 'X']).parse::<u32>() {
                    if count.starts_with(['x', 'X']) && !front.is_empty() {
                        push(c, front.to_string());
                        continue;
                    }
                }
            }
        }
        push(n, name.to_string());
    }
    out
}

#[cfg(test)]
mod import_tests {
    use super::*;

    #[test]
    fn reads_text_and_csv_exports() {
        let text = "1 Red Hulk (MSH) 149\n2x Web Up\n1 Web Up (MSH) 41 *F*\nKa-Zar of the Savage Land\n\n# comment\n";
        let r = parse_import(text);
        assert_eq!(
            r[0],
            ImportLine {
                count: 1,
                name: "Red Hulk".into()
            }
        );
        assert_eq!(
            r[1],
            ImportLine {
                count: 3,
                name: "Web Up".into()
            }
        );
        assert_eq!(r[2].name, "Ka-Zar of the Savage Land");
        let csv = "Name,Set code,Quantity,Foil\n\"Bullseye, Death Dealer\",MSH,2,normal\nRed Hulk,MSH,1,foil\n";
        let r = parse_import(csv);
        assert_eq!(
            r[0],
            ImportLine {
                count: 2,
                name: "Bullseye, Death Dealer".into()
            }
        );
        assert_eq!(r[1].count, 1);
    }

    #[test]
    fn reads_other_export_shapes() {
        // Byte-order mark, Windows line endings, semicolons.
        let r = parse_import("\u{feff}Card Name;Set;Qty\r\nRed Hulk;MSH;2\r\nWeb Up;MSH;1\r\n");
        assert_eq!(
            (r.len(), r[0].count, r[0].name.as_str()),
            (2, 2, "Red Hulk")
        );
        // Tab-separated, as copied from a spreadsheet.
        let r = parse_import("Quantity\tName\tSet\n3\tWeb Up\tMSH\n");
        assert_eq!((r[0].count, r[0].name.as_str()), (3, "Web Up"));
        // Bracketed set codes and trailing counts.
        let r = parse_import("1x Red Hulk [MSH] 149\nWeb Up x2\n");
        assert_eq!(
            (r[0].name.as_str(), r[1].name.as_str(), r[1].count),
            ("Red Hulk", "Web Up", 2)
        );
        // Two-faced names as Mana Mixer exports them - and they round-trip on export.
        let db = CardDb::load(
            &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data/cards.json"),
        )
        .unwrap();
        let bilbo = db
            .get("Bilbo Baggins, Burglar // Take a Glance")
            .expect("two-faced name resolves");
        assert!(
            mana_mixer_line(Some(bilbo), "", 3)
                .starts_with("3 Bilbo Baggins, Burglar // Take a Glance (HOB) ")
        );
        assert_eq!(
            mana_mixer_line(db.get("Red Hulk"), "", 1),
            "1 Red Hulk (MSH) 149"
        );
        let r = parse_import("3 Bilbo Baggins, Burglar // Take a Glance (HOB) 12\n");
        assert_eq!(
            (r[0].count, r[0].name.as_str()),
            (3, "Bilbo Baggins, Burglar")
        );
        // UTF-16 file from Notepad.
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend("2 Web Up\r\n".encode_utf16().flat_map(|u| u.to_le_bytes()));
        assert_eq!(parse_import(&decode_text(&bytes))[0].count, 2);
    }
}

pub struct ImportResult {
    pub cards: u32,
    pub distinct: usize,
    /// Names the card database doesn't know (kept in the pool so nothing is lost).
    pub unknown: Vec<String>,
}

/// Imports a Mana Mixer / ManaBox list into the pool, either adding to it or replacing it
/// (the old pool is kept as collection.backup.txt).
pub fn import_pool(
    text: &str,
    replace: bool,
    db: &CardDb,
    coll: &mut Collection,
) -> Result<ImportResult, String> {
    let lines = parse_import(text);
    if lines.is_empty() {
        return Err("No cards found in that list.".into());
    }
    let mut unknown = Vec::new();
    let mut resolved: Vec<(u32, String)> = Vec::new();
    for l in &lines {
        let name = match db.lookup(&l.name) {
            crate::db::Lookup::Found(c) => c.name.clone(),
            _ => {
                unknown.push(l.name.clone());
                l.name.clone()
            }
        };
        match resolved.iter_mut().find(|(_, n)| *n == name) {
            Some(r) => r.0 += l.count,
            None => resolved.push((l.count, name)),
        }
    }
    let cards: u32 = resolved.iter().map(|(n, _)| n).sum();
    if replace {
        let counts = resolved
            .iter()
            .map(|(n, name)| (name.clone(), *n))
            .collect();
        let header = format!(
            "# My league pool - one \"count name\" per line.\n# Imported from Mana Mixer on {}.\n\n",
            crate::ratings::today()
        );
        coll.replace_all(counts, &header)?;
    } else {
        for (n, name) in &resolved {
            coll.adjust(name, *n as i64)?;
        }
    }
    Ok(ImportResult {
        cards,
        distinct: resolved.len(),
        unknown,
    })
}
