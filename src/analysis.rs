//! Deck statistics, card scoring, suggestions, auto-building and opening-hand simulation.

use crate::card::{BASIC_FOR_COLOR, COLORS, Card, ColorSet, Role};
use crate::db::{CardDb, Collection, Rules};
use crate::deck::Deck;
use std::collections::BTreeMap;

/// Ideal number of nonland cards at each mana value (index 0 = MV 0-1, 5 = MV 6+)
/// for a 60-card deck with ~26 lands (33 spells). Scaled for other sizes.
const CURVE_TARGET_60: [f32; 6] = [3.0, 7.0, 8.0, 7.0, 5.0, 3.0];

pub fn curve_bucket(mv: u32) -> usize {
    (mv.max(1) as usize - 1).min(5)
}

pub fn curve_label(bucket: usize) -> &'static str {
    ["0-1", "2", "3", "4", "5", "6+"][bucket]
}

pub fn curve_targets(rules: &Rules) -> [u32; 6] {
    let scale = rules.deck_size as f32 / 60.0;
    CURVE_TARGET_60.map(|t| (t * scale).round() as u32)
}

/// Every card in the main deck with its count, resolved against the database.
pub fn resolved<'a>(deck: &Deck, db: &'a CardDb) -> Vec<(&'a Card, u32)> {
    deck.cards
        .iter()
        .filter_map(|(name, n)| db.get(name).map(|c| (c, *n)))
        .collect()
}

pub struct Stats {
    pub lands: u32,
    pub nonlands: u32,
    pub creatures: u32,
    pub curve: [u32; 6],
    pub avg_mv: f32,
    pub pips: [u32; 5],
    pub types: BTreeMap<&'static str, u32>,
    pub roles: BTreeMap<Role, u32>,
    pub sources: [u32; 5],
    pub subtypes: BTreeMap<String, u32>,
}

pub fn stats(deck: &Deck, db: &CardDb) -> Stats {
    let mut s = Stats {
        lands: 0,
        nonlands: 0,
        creatures: 0,
        curve: [0; 6],
        avg_mv: 0.0,
        pips: [0; 5],
        types: BTreeMap::new(),
        roles: BTreeMap::new(),
        sources: [0; 5],
        subtypes: BTreeMap::new(),
    };
    let mut mv_total = 0u32;
    for (card, n) in resolved(deck, db) {
        *s.types.entry(card.main_type()).or_insert(0) += n;
        if card.is_land() {
            s.lands += n;
            let prod = card.produces();
            for i in prod.indices() {
                s.sources[i] += n;
            }
            continue;
        }
        if card.oracle_text.to_lowercase().contains("{t}: add") {
            for i in card.produces().indices() {
                s.sources[i] += n;
            }
        }
        s.nonlands += n;
        if card.is_creature() {
            s.creatures += n;
        }
        s.curve[curve_bucket(card.mana_value())] += n;
        mv_total += card.mana_value() * n;
        let p = card.pips();
        for i in 0..5 {
            s.pips[i] += p[i] * n;
        }
        for r in card.roles() {
            *s.roles.entry(r).or_insert(0) += n;
        }
        for st in card.subtypes() {
            if st == "Hero" || st == "Villain" || card.is_creature() {
                *s.subtypes.entry(st).or_insert(0) += n;
            }
        }
    }
    if s.nonlands > 0 {
        s.avg_mv = mv_total as f32 / s.nonlands as f32;
    }
    s
}

/// League guidance: 25-27 lands in a 60-card deck, scaled for other sizes (31-34 at 75).
/// Low curves get the bottom of the range, high curves the top.
pub fn land_range(rules: &Rules) -> (u32, u32) {
    let scale = rules.deck_size as f32 / 60.0;
    ((25.0 * scale).round() as u32, (27.0 * scale).round() as u32)
}

pub fn recommended_lands(avg_mv: f32, rules: &Rules) -> u32 {
    let (lo, hi) = land_range(rules);
    let mid = (lo + hi) / 2;
    if avg_mv == 0.0 {
        mid
    } else if avg_mv < 2.7 {
        lo
    } else if avg_mv > 3.4 {
        hi
    } else {
        mid
    }
}

/// Splits `count` basic lands between the commander's colours in proportion to the
/// coloured pips in the deck (commander pips weighted double), minus existing sources.
pub fn basic_split(deck: &Deck, db: &CardDb, count: u32) -> [u32; 5] {
    let mut out = [0u32; 5];
    let Some(identity) = deck.identity(db) else {
        return out;
    };
    if identity.is_empty() || count == 0 {
        return out;
    }
    let st = stats(deck, db);
    let mut weight = [0f32; 5];
    let cmd_pips = deck
        .commander
        .as_ref()
        .and_then(|c| db.get(c))
        .map(|c| c.pips())
        .unwrap_or([0; 5]);
    // Only colours the deck actually casts get basics (a 5-colour commander playing
    // three colours shouldn't get Islands); fall back to the whole identity.
    let used = ColorSet(
        identity
            .indices()
            .filter(|&i| st.pips[i] > 0 || cmd_pips[i] > 0)
            .fold(0, |acc, i| acc | (1 << i)),
    );
    let identity = if used.is_empty() { identity } else { used };
    for i in identity.indices() {
        // Every used colour gets a small floor so splashes still get a land.
        let nonbasic_sources = st.sources[i].saturating_sub(deck.count_of(BASIC_FOR_COLOR[i]));
        weight[i] =
            st.pips[i] as f32 + 2.0 * cmd_pips[i] as f32 + 1.0 - 0.5 * nonbasic_sources as f32;
        weight[i] = weight[i].max(0.5);
    }
    let total: f32 = weight.iter().sum();
    let mut rema: Vec<(usize, f32)> = Vec::new();
    let mut given = 0;
    for i in identity.indices() {
        let exact = count as f32 * weight[i] / total;
        out[i] = exact.floor() as u32;
        given += out[i];
        rema.push((i, exact - exact.floor()));
    }
    rema.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    for (i, _) in rema.iter().cycle().take((count - given) as usize) {
        out[*i] += 1;
    }
    out
}

pub struct Scored<'a> {
    pub card: &'a Card,
    pub score: f32,
    pub reasons: Vec<String>,
}

fn numeric(s: &Option<String>) -> Option<f32> {
    s.as_ref().and_then(|v| v.parse::<f32>().ok())
}

/// How good `card` would be as the next addition to `deck`. Higher is better.
/// This is a heuristic to rank options, not gospel - always read the cards.
pub fn score_card(card: &Card, deck: &Deck, db: &CardDb, rules: &Rules) -> (f32, Vec<String>) {
    let mut score = card.rarity_rank() as f32 * 0.6;
    let mut why = Vec::new();
    if card.is_land() {
        return (0.0, why);
    }
    let roles = card.roles();
    for r in &roles {
        let (pts, label) = match r {
            Role::Removal => (3.0, "removal"),
            Role::Counter => (1.5, "counterspell"),
            Role::Bounce => (1.0, "bounce"),
            Role::Draw => (1.5, "card advantage"),
            Role::Ramp => (1.0, "mana"),
            Role::Tokens => (1.0, "makes tokens"),
            Role::Protection => (0.5, "protection"),
            Role::Evasion if card.is_creature() => (0.75, "evasive"),
            Role::Trick => (0.5, "combat trick"),
            _ => (0.0, ""),
        };
        if pts > 0.0 {
            score += pts;
            why.push(label.to_string());
        }
    }
    let st = stats(deck, db);
    if card.is_creature() {
        if let (Some(p), Some(t)) = (numeric(&card.power), numeric(&card.toughness)) {
            let eff = (p + t) / (card.mana_value().max(1) as f32);
            let bonus = (eff - 1.6).clamp(-1.0, 1.5);
            score += bonus;
            if bonus >= 0.5 {
                why.push(format!("efficient body {}/{}", p, t));
            }
        }
        let creature_target = (rules.deck_size as f32 * 0.27) as u32;
        if st.creatures < creature_target {
            score += 1.0;
            why.push(format!(
                "deck needs creatures ({}/{})",
                st.creatures, creature_target
            ));
        }
    }
    let bucket = curve_bucket(card.mana_value());
    let target = curve_targets(rules)[bucket];
    if st.curve[bucket] < target {
        score += 0.75;
        why.push(format!("fills the {}-drop slot", curve_label(bucket)));
    } else if st.curve[bucket] >= target + 2 {
        score -= 1.0;
    }
    if card.mana_value() >= 7 {
        score -= 1.5;
    }
    if let Some(cmd) = deck.commander.as_ref().and_then(|c| db.get(c)) {
        let (syn, mut reasons) = synergy(cmd, card);
        score += syn;
        why.append(&mut reasons);
    }
    // With a 3+ colour commander, adding a colour the deck doesn't cast yet hurts consistency.
    if let Some(cmd) = deck.commander.as_ref().and_then(|c| db.get(c)) {
        if cmd.identity().count() > 2 && st.nonlands >= 8 {
            let cast = ColorSet(
                (0..5)
                    .filter(|&i| st.pips[i] > 0)
                    .fold(0, |acc, i| acc | (1 << i)),
            )
            .union(cmd.cost_colors());
            if !card.cost_colors().is_subset_of(cast) {
                score -= 1.0;
                why.push("adds a new colour".into());
            }
        }
    }
    // Tribal pull: cards that care about Heroes/Villains get better with more of them.
    let text = card.oracle_text.to_lowercase();
    for tribe in ["Hero", "Villain", "Robot"] {
        if text.contains(&tribe.to_lowercase()) {
            let n = *st.subtypes.get(tribe).unwrap_or(&0);
            let bonus = (n as f32 / 5.0).min(1.5);
            if bonus > 0.0 {
                score += bonus;
                why.push(format!("{} {}s in deck", n, tribe));
            }
        }
    }
    if roles.contains(&Role::Teamwork) && st.creatures >= 12 {
        score += 0.5;
        why.push("enough creatures for teamwork".into());
    }
    let copies = deck.count_of(&card.name);
    if copies > 0 && !roles.contains(&Role::Removal) {
        score -= 0.4 * copies as f32;
    }
    (score, why)
}

/// Bonus for cards that play into what the commander's text asks for.
pub fn synergy(cmd: &Card, card: &Card) -> (f32, Vec<String>) {
    let ct = cmd.oracle_text.to_lowercase();
    let ty = card.type_line.to_lowercase();
    let text = card.oracle_text.to_lowercase();
    let mut score = 0.0;
    let mut why = Vec::new();
    let mut add = |pts: f32, msg: String| {
        score += pts;
        why.push(msg);
    };
    for sub in card.subtypes() {
        let s = sub.to_lowercase();
        if s.len() > 2 && ct.contains(&s) {
            add(2.0, format!("commander cares about {}", sub));
        }
    }
    let checks: [(&str, bool, &str); 8] = [
        (
            "artifact",
            ty.contains("artifact"),
            "artifact for commander",
        ),
        (
            "equipment",
            ty.contains("equipment"),
            "Equipment for commander",
        ),
        ("vehicle", ty.contains("vehicle"), "Vehicle for commander"),
        (
            "noncreature spell",
            !card.is_creature(),
            "noncreature spell triggers commander",
        ),
        (
            "instant or sorcery",
            ty.contains("instant") || ty.contains("sorcery"),
            "spell triggers commander",
        ),
        (
            "+1/+1 counter",
            text.contains("counter"),
            "counters synergy",
        ),
        (
            "enchantment",
            ty.contains("enchantment"),
            "enchantment for commander",
        ),
        ("token", text.contains("token"), "token synergy"),
    ];
    for (needle, hit, msg) in checks {
        if ct.contains(needle) && hit {
            add(1.5, msg.to_string());
        }
    }
    if (ct.contains("dealt damage") || ct.contains("enrage")) && text.contains("damage") {
        add(0.75, "damage synergy with Enrage".into());
    }
    if ct.contains("noncombat damage") && text.contains("damage") && !card.is_creature() {
        add(1.5, "burn boosted by commander".into());
    }
    let cmd_tribes: Vec<String> = cmd.subtypes();
    for tribe in ["Hero", "Villain"] {
        if cmd_tribes.iter().any(|t| t == tribe) && card.subtypes().iter().any(|t| t == tribe) {
            add(0.5, format!("fellow {}", tribe));
        }
    }
    (score, why)
}

/// Owned copies not already used by `deck`.
pub fn available(card: &Card, deck: &Deck, coll: &Collection, rules: &Rules) -> u32 {
    let used = deck.used(&card.name);
    if card.is_basic() && rules.unlimited_basics {
        return 99;
    }
    let cap = if rules.singleton {
        1u32.saturating_sub(used)
    } else {
        99
    };
    if !rules.enforce_collection || (card.name == crate::deck::PIPER && rules.piper_free) {
        return cap;
    }
    coll.owned(&card.name).saturating_sub(used).min(cap)
}

/// Nonland cards you could add right now, best first.
pub fn suggestions<'a>(
    deck: &Deck,
    db: &'a CardDb,
    coll: &Collection,
    rules: &Rules,
) -> Vec<Scored<'a>> {
    suggestions_within(deck, db, coll, rules, None)
}

/// Like `suggestions`, optionally limited to cards inside `colors`.
pub fn suggestions_within<'a>(
    deck: &Deck,
    db: &'a CardDb,
    coll: &Collection,
    rules: &Rules,
    colors: Option<ColorSet>,
) -> Vec<Scored<'a>> {
    let identity = deck.identity(db);
    let mut out: Vec<Scored> = db
        .cards
        .iter()
        .filter(|c| !c.is_land())
        .filter(|c| available(c, deck, coll, rules) > 0)
        .filter(|c| match identity {
            Some(id) if rules.enforce_color_identity => c.identity().is_subset_of(id),
            _ => true,
        })
        .filter(|c| colors.is_none_or(|lim| c.identity().is_subset_of(lim)))
        .map(|c| {
            let (score, reasons) = score_card(c, deck, db, rules);
            Scored {
                card: c,
                score,
                reasons,
            }
        })
        .collect();
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    out
}

/// Weakest cards currently in the deck, worst first.
pub fn weakest<'a>(deck: &Deck, db: &'a CardDb, rules: &Rules) -> Vec<Scored<'a>> {
    let mut out = Vec::new();
    for (card, _) in resolved(deck, db) {
        if card.is_land() {
            continue;
        }
        let mut without = deck.clone();
        without.remove(&card.name, 1);
        let (score, reasons) = score_card(card, &without, db, rules);
        out.push(Scored {
            card,
            score,
            reasons,
        });
    }
    out.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap());
    out
}

/// Builds a full legal deck around `commander` from what you own. Also returns how many
/// spell slots couldn't be filled because the pool ran out (those become extra basics).
/// `colors` limits which of the commander's colours to play; `None` picks the best ones.
/// `piper` pairs a mono-colour commander with The Prismatic Piper in that colour.
pub fn autobuild(
    commander: &Card,
    db: &CardDb,
    coll: &Collection,
    rules: &Rules,
    colors: Option<ColorSet>,
    piper: Option<char>,
) -> (Deck, u32) {
    let mut deck = Deck::new(&format!("auto-{}", commander.name));
    deck.commander = Some(commander.name.clone());
    if piper.is_some() {
        deck.partner = Some(crate::deck::PIPER.to_string());
        deck.piper_color = piper;
    }
    let identity = deck.identity(db).unwrap_or_else(|| commander.identity());
    let main = deck.main_target(rules);
    let plays = colors.unwrap_or_else(|| {
        if piper.is_some() {
            identity
        } else {
            best_colors(commander, db, coll, rules)
        }
    });

    // Nonbasic lands that make our colours come first; they count toward the land total.
    for land in db.cards.iter().filter(|c| c.is_land() && !c.is_basic()) {
        let ok_identity = !rules.enforce_color_identity || land.identity().is_subset_of(identity);
        let useful = land.produces().0 & plays.0 != 0;
        if ok_identity && useful {
            let n = available(land, &deck, coll, rules);
            if n > 0 && n < 99 {
                deck.add(&land.name, n);
            }
        }
    }

    // Pick spells, then re-check the land count against the resulting curve.
    let base = deck.clone();
    let mut lands = recommended_lands(3.0, rules);
    let mut shortfall = 0;
    for attempt in 0..3 {
        deck = base.clone();
        let spell_slots = main.saturating_sub(lands);
        while deck.main_count() - land_count(&deck, db) < spell_slots {
            let best = suggestions_within(&deck, db, coll, rules, Some(plays));
            let Some(pick) = best.first() else { break };
            let name = pick.card.name.clone();
            deck.add(&name, 1);
        }
        shortfall = spell_slots.saturating_sub(deck.main_count() - land_count(&deck, db));
        let new_lands = recommended_lands(stats(&deck, db).avg_mv, rules);
        if new_lands == lands || attempt == 2 || shortfall > 0 {
            break;
        }
        lands = new_lands;
    }
    fill_basics(&mut deck, db, rules);
    (deck, shortfall)
}

pub fn land_count(deck: &Deck, db: &CardDb) -> u32 {
    resolved(deck, db)
        .iter()
        .filter(|(c, _)| c.is_land())
        .map(|(_, n)| n)
        .sum()
}

/// Tops the deck up to the required size with basics split by colour need.
/// Returns how many of each basic were added.
pub fn fill_basics(deck: &mut Deck, db: &CardDb, rules: &Rules) -> [u32; 5] {
    let missing = deck.main_target(rules).saturating_sub(deck.main_count());
    let split = basic_split(deck, db, missing);
    for i in 0..5 {
        if split[i] > 0 {
            deck.add(BASIC_FOR_COLOR[i], split[i]);
        }
    }
    split
}

/// Average score of the best spells you own inside `colors` - how strong a deck those colours make.
fn pool_quality(
    cmd: &Card,
    colors: ColorSet,
    db: &CardDb,
    coll: &Collection,
    rules: &Rules,
) -> f32 {
    let mut deck = Deck::new("probe");
    deck.commander = Some(cmd.name.clone());
    let mut scores = Vec::new();
    for c in db
        .cards
        .iter()
        .filter(|c| !c.is_land() && c.identity().is_subset_of(colors))
    {
        let n = available(c, &deck, coll, rules);
        if n == 0 || n >= 99 {
            continue;
        }
        let (s, _) = score_card(c, &deck, db, rules);
        for _ in 0..n {
            scores.push(s);
        }
    }
    scores.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let slots = rules
        .main_deck_size()
        .saturating_sub(recommended_lands(3.0, rules)) as usize;
    // Empty slots count as zero, so thin colour choices are penalised.
    scores.iter().take(slots).sum::<f32>() / slots.max(1) as f32
}

/// For commanders with 3+ colours in their identity, picks the subset of colours to actually
/// play: always the colours in the commander's own cost, plus the best others, at most
/// max(2, cost colours) in total. Two-colour-or-less commanders just use their identity.
pub fn best_colors(cmd: &Card, db: &CardDb, coll: &Collection, rules: &Rules) -> ColorSet {
    let id = cmd.identity();
    if id.count() <= 2 {
        return id;
    }
    let base = cmd.cost_colors();
    let max = base.count().max(2);
    let mut best = (f32::MIN, id);
    for mask in 1u8..32 {
        let s = ColorSet(mask);
        if !s.is_subset_of(id) || !base.is_subset_of(s) || s.count() > max {
            continue;
        }
        let q = pool_quality(cmd, s, db, coll, rules);
        if q > best.0 {
            best = (q, s);
        }
    }
    best.1
}

pub struct CommanderOption<'a> {
    pub card: &'a Card,
    pub identity: ColorSet,
    /// The colours a deck for this commander would actually play.
    pub plays: ColorSet,
    pub playables: u32,
    pub removal: u32,
    pub creatures: u32,
    pub quality: f32,
}

/// Ranks every legendary creature you own by how deep a pool it would let you build.
pub fn rank_commanders<'a>(
    db: &'a CardDb,
    coll: &Collection,
    rules: &Rules,
) -> Vec<CommanderOption<'a>> {
    let mut out = Vec::new();
    for cmd in db
        .cards
        .iter()
        .filter(|c| c.can_be_commander() && coll.owned(&c.name) > 0)
    {
        let id = cmd.identity();
        let plays = best_colors(cmd, db, coll, rules);
        let deck = Deck::new("probe");
        let mut playables = 0;
        let mut removal = 0;
        let mut creatures = 0;
        for c in db
            .cards
            .iter()
            .filter(|c| !c.is_land() && c.identity().is_subset_of(plays))
        {
            let n = available(c, &deck, coll, rules);
            if n == 0 || n >= 99 || c.name == cmd.name {
                continue;
            }
            playables += n;
            if c.has_role(Role::Removal) {
                removal += n;
            }
            if c.is_creature() {
                creatures += n;
            }
        }
        let quality = pool_quality(cmd, plays, db, coll, rules);
        out.push(CommanderOption {
            card: cmd,
            identity: id,
            plays,
            playables,
            removal,
            creatures,
            quality,
        });
    }
    out.sort_by(|a, b| {
        let key = |o: &CommanderOption| o.quality + (o.playables.min(40) as f32) * 0.05;
        key(b).partial_cmp(&key(a)).unwrap()
    });
    out
}

/// Small deterministic PRNG so simulations don't need extra crates.
pub struct Rng(u64);

impl Rng {
    pub fn seeded() -> Rng {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E3779B97F4A7C15);
        Rng(t | 1)
    }

    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = (self.next() % (i as u64 + 1)) as usize;
            v.swap(i, j);
        }
    }
}

pub fn library<'a>(deck: &Deck, db: &'a CardDb) -> Vec<&'a Card> {
    let mut lib = Vec::new();
    for (c, n) in resolved(deck, db) {
        for _ in 0..n {
            lib.push(c);
        }
    }
    lib
}

pub struct SimResult {
    pub trials: u32,
    pub land_hist: [u32; 8],
    /// P(at least k lands by turn k), k = 1..=6, on the play.
    pub land_drops: [f32; 6],
    pub commander_on_curve: Option<(u32, f32)>,
    pub keepable: f32,
}

pub fn simulate(deck: &Deck, db: &CardDb, trials: u32) -> SimResult {
    let mut lib = library(deck, db);
    let mut rng = Rng::seeded();
    let mut hist = [0u32; 8];
    let mut drops = [0u32; 6];
    let mut keep = 0;
    let cmd = deck.commander.as_ref().and_then(|c| db.get(c));
    let mut cmd_hits = 0;
    for _ in 0..trials {
        rng.shuffle(&mut lib);
        let hand_lands = lib.iter().take(7).filter(|c| c.is_land()).count();
        hist[hand_lands.min(7)] += 1;
        if (2..=5).contains(&hand_lands) {
            keep += 1;
        }
        for k in 1..=6usize {
            let seen = (7 + k - 1).min(lib.len());
            if lib[..seen].iter().filter(|c| c.is_land()).count() >= k {
                drops[k - 1] += 1;
            }
        }
        if let Some(cmd) = cmd {
            let mv = cmd.mana_value().max(1) as usize;
            let seen = (7 + mv - 1).min(lib.len());
            let lands: Vec<&&Card> = lib[..seen].iter().filter(|c| c.is_land()).collect();
            let pips = cmd.pips();
            let colors_ok = (0..5).all(|i| {
                pips[i] == 0
                    || lands.iter().filter(|l| l.produces().contains(i)).count() as u32 >= pips[i]
            });
            if lands.len() >= mv && colors_ok {
                cmd_hits += 1;
            }
        }
    }
    let t = trials.max(1) as f32;
    SimResult {
        trials,
        land_hist: hist,
        land_drops: drops.map(|d| d as f32 / t),
        commander_on_curve: cmd.map(|c| (c.mana_value(), cmd_hits as f32 / t)),
        keepable: keep as f32 / t,
    }
}

pub fn color_name(i: usize) -> &'static str {
    ["White", "Blue", "Black", "Red", "Green"][i]
}

pub fn color_char(i: usize) -> char {
    COLORS[i]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn mk(name: &str, cost: &str, ty: &str, text: &str, id: &str) -> Card {
        Card {
            name: name.into(),
            mana_cost: cost.into(),
            cmc: crate::card::symbols_in(cost)
                .iter()
                .map(|s| s.parse::<f32>().unwrap_or(1.0))
                .sum(),
            type_line: ty.into(),
            oracle_text: text.into(),
            power: Some("2".into()),
            toughness: Some("2".into()),
            rarity: "common".into(),
            color_identity: id.chars().map(|c| c.to_string()).collect(),
            confidence: "verified".into(),
            ..Default::default()
        }
    }

    fn setup() -> (CardDb, Collection) {
        let mut cards = vec![mk(
            "Boss",
            "{2}{B}{R}",
            "Legendary Creature — Human Villain",
            "Whenever you cast a Villain spell, create a token.",
            "BR",
        )];
        let mut counts = BTreeMap::new();
        counts.insert("Boss".to_string(), 1);
        for i in 0..50 {
            let name = format!("Goon {}", i);
            let cost = ["{R}", "{1}{B}", "{2}{R}", "{3}{B}", "{4}{R}"][i % 5];
            cards.push(mk(
                &name,
                cost,
                "Creature — Human Villain",
                "",
                if i % 2 == 0 { "R" } else { "B" },
            ));
            counts.insert(name, 1);
        }
        cards.push(mk("Off Color", "{G}", "Creature — Elf", "", "G"));
        counts.insert("Off Color".into(), 4);
        for (i, b) in BASIC_FOR_COLOR.iter().enumerate() {
            cards.push(Card::basic_land(b, COLORS[i]));
        }
        cards.sort_by(|a, b| a.name.cmp(&b.name));
        (
            CardDb { cards },
            Collection {
                path: PathBuf::new(),
                counts,
            },
        )
    }

    #[test]
    fn autobuild_is_legal() {
        let (db, coll) = setup();
        let rules = Rules::default();
        let boss = db.get("Boss").unwrap().clone();
        let (deck, short) = autobuild(&boss, &db, &coll, &rules, None, None);
        assert_eq!(short, 0);
        assert!(land_count(&deck, &db) < 30, "{}", deck.to_text());
        let problems: Vec<String> = deck
            .validate(&db, &coll, &rules)
            .into_iter()
            .map(|p| p.message)
            .collect();
        assert!(problems.is_empty(), "{:?}", problems);
        assert_eq!(deck.total_count(&rules), 60);
        assert!(deck.count_of("Off Color") == 0);
        assert!(deck.count_of("Mountain") > 0 && deck.count_of("Swamp") > 0);
        assert_eq!(deck.count_of("Forest"), 0);
    }

    #[test]
    fn villain_synergy_counts() {
        let (db, _) = setup();
        let boss = db.get("Boss").unwrap();
        let goon = db.get("Goon 1").unwrap();
        let (s, why) = synergy(boss, goon);
        assert!(s >= 2.0, "{} {:?}", s, why);
    }

    #[test]
    fn simulation_runs() {
        let (db, coll) = setup();
        let rules = Rules::default();
        let (deck, _) = autobuild(db.get("Boss").unwrap(), &db, &coll, &rules, None, None);
        let r = simulate(&deck, &db, 2000);
        assert_eq!(r.land_hist.iter().sum::<u32>(), 2000);
        assert!(r.keepable > 0.5);
    }
}
