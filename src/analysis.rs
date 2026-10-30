//! Deck statistics, card scoring, suggestions, auto-building and opening-hand simulation.

use crate::abilities::{Effect as AbEffect, Event, When};
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

/// The kind of deck a commander wants to be in, read from its cost, body and rules text.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Archetype {
    /// Cheap attackers, combat triggers: low curve, lots of creatures.
    Aggro,
    Midrange,
    /// Expensive or big-mana payoffs: higher curve, ramp, fewer small creatures.
    Big,
    /// Instant/sorcery or noncreature-spell payoffs: fewer creatures, more interaction.
    Spells,
}

impl Archetype {
    pub fn label(self) -> &'static str {
        match self {
            Archetype::Aggro => "aggro",
            Archetype::Midrange => "midrange",
            Archetype::Big => "big mana",
            Archetype::Spells => "spells",
        }
    }
}

/// Deck-building targets for one commander: mana curve (nonland cards per mana value),
/// creature range, and how much removal and card draw to aim for.
#[derive(Debug, Clone)]
pub struct Plan {
    pub archetype: Archetype,
    pub curve: [u32; 6],
    pub creatures: (u32, u32),
    pub removal: f32,
    pub draw: f32,
    /// What in the commander's card pointed to this plan.
    pub signals: Vec<String>,
}

/// Reads a commander's card for what kind of deck it wants.
pub fn archetype(cmd: &Card) -> (Archetype, Vec<String>) {
    let t = cmd.text_lower();
    let mv = cmd.mana_value();
    let mut aggro = 0.0;
    let mut big = 0.0;
    let mut spells = 0.0;
    let mut why = Vec::new();
    let hit = |pts: &mut f32, add: f32, cond: bool, label: &str, why: &mut Vec<String>| {
        if cond {
            *pts += add;
            why.push(label.to_string());
        }
    };
    hit(&mut aggro, 1.0, mv <= 3, "cheap commander", &mut why);
    hit(
        &mut aggro,
        1.5,
        t.contains(" attacks") || t.contains("attacking creature"),
        "rewards attacking",
        &mut why,
    );
    hit(
        &mut aggro,
        1.5,
        t.contains("combat damage to a player"),
        "combat-damage trigger",
        &mut why,
    );
    hit(&mut aggro, 0.5, t.contains("haste"), "haste", &mut why);
    hit(
        &mut aggro,
        1.0,
        t.contains("double strike"),
        "double strike",
        &mut why,
    );
    hit(
        &mut aggro,
        1.0,
        t.contains("creatures you control get +"),
        "anthem for your team",
        &mut why,
    );
    hit(
        &mut aggro,
        0.5,
        mv <= 4 && t.contains("create") && t.contains("token"),
        "makes tokens",
        &mut why,
    );
    hit(&mut big, 2.0, mv >= 6, "expensive commander", &mut why);
    hit(&mut big, 0.5, mv == 5, "five mana", &mut why);
    hit(
        &mut big,
        1.5,
        t.contains("mana value 5 or greater")
            || t.contains("mana value 4 or greater")
            || t.contains("power 4 or greater")
            || t.contains("greatest power"),
        "rewards big spells",
        &mut why,
    );
    hit(
        &mut big,
        1.0,
        t.contains("landfall") || t.contains("lands you control"),
        "cares about lands",
        &mut why,
    );
    hit(
        &mut big,
        0.5,
        cmd.mana_cost.contains("{X}"),
        "X cost",
        &mut why,
    );
    hit(
        &mut spells,
        2.5,
        t.contains("instant or sorcery")
            || t.contains("instants and sorceries")
            || t.contains("instant and sorcery")
            || t.contains("noncreature spell"),
        "instant/sorcery payoff",
        &mut why,
    );
    hit(
        &mut spells,
        1.0,
        t.contains("whenever you draw"),
        "card-draw payoff",
        &mut why,
    );
    let best = [
        (spells, Archetype::Spells),
        (big, Archetype::Big),
        (aggro, Archetype::Aggro),
    ]
    .into_iter()
    .fold((0.0, Archetype::Midrange), |acc, x| {
        if x.0 > acc.0 { x } else { acc }
    });
    if best.0 < 1.5 {
        return (Archetype::Midrange, vec!["no strong lean".into()]);
    }
    (best.1, why)
}

/// Targets for a deck led by `cmd` (midrange targets without a commander).
pub fn plan_for(cmd: Option<&Card>, rules: &Rules) -> Plan {
    let (arch, signals) = cmd
        .map(archetype)
        .unwrap_or((Archetype::Midrange, Vec::new()));
    // Per 60 cards with ~26 lands (33 nonland cards each).
    let (curve, creatures, removal, draw): ([f32; 6], (f32, f32), f32, f32) = match arch {
        Archetype::Aggro => ([5.0, 9.0, 8.0, 6.0, 3.0, 2.0], (18.0, 24.0), 5.0, 3.0),
        Archetype::Midrange => (CURVE_TARGET_60, (15.0, 20.0), 7.0, 5.0),
        Archetype::Big => ([3.0, 6.0, 7.0, 7.0, 6.0, 4.0], (13.0, 18.0), 6.0, 5.0),
        Archetype::Spells => ([4.0, 8.0, 8.0, 6.0, 4.0, 3.0], (9.0, 14.0), 8.0, 7.0),
    };
    let scale = rules.deck_size as f32 / 60.0;
    Plan {
        archetype: arch,
        curve: curve.map(|t| (t * scale).round() as u32),
        creatures: (
            (creatures.0 * scale).round() as u32,
            (creatures.1 * scale).round() as u32,
        ),
        removal: removal * scale,
        draw: draw * scale,
        signals,
    }
}

/// The plan for a deck's commander.
pub fn deck_plan(deck: &Deck, db: &CardDb, rules: &Rules) -> Plan {
    plan_for(deck.commander.as_ref().and_then(|c| db.get(c)), rules)
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

/// The league's mana base goal: the middle of its land range (26 in a 60-card deck). A deck
/// that has reached it isn't advised to play more lands (the playtest lab then suggests
/// better fixing rather than another basic).
pub fn land_goal(rules: &Rules) -> u32 {
    if let Some(n) = rules.land_goal_override {
        return n;
    }
    let (lo, hi) = land_range(rules);
    (lo + hi) / 2
}

/// Lands for a deck with this average mana value: the bottom of the league's range for a
/// cheap curve, otherwise its goal (`land_goal`, 26 of 60). Not more for a high curve: the
/// playtest lab measured a 27th land at no gain for Thanos decks averaging 3.5+ (and the
/// league's goal is 26).
pub fn recommended_lands(avg_mv: f32, rules: &Rules) -> u32 {
    let (lo, _) = land_range(rules);
    if avg_mv > 0.0 && avg_mv < 2.7 {
        lo
    } else {
        land_goal(rules)
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

/// Base strength of a card on its own, before roles and synergy: an average card (1.5),
/// nudged by rarity - rares and mythics are only a little stronger than commons on average,
/// so each rarity step is worth 0.25.
pub fn card_power(card: &Card) -> f32 {
    match card.rarity_rank() {
        0 => 1.5, // special printings (league staples)
        r => 1.0 + 0.25 * r as f32,
    }
}

/// Adjustments for 3+ player games: effects that hit every opponent, board wipes, card
/// flow and ramp gain value; small aggressive creatures and 1-for-1 trades lose some.
fn multiplayer_adjust(
    card: &Card,
    roles: &[Role],
    rules: &Rules,
    each_opponent: f32,
) -> (f32, Vec<String>) {
    let mut score = 0.0;
    let mut why = Vec::new();
    if rules.players < 3 {
        return (score, why);
    }
    let opponents = rules.players.saturating_sub(1) as f32;
    if roles.contains(&Role::EachOpponent) && each_opponent > 0.0 {
        score += 0.5 * (opponents - 1.0) * each_opponent;
        why.push(format!("hits all {} opponents", opponents as u32));
    }
    if roles.contains(&Role::Sweeper) {
        let one_sided = card.parsed().removal().is_some_and(|r| r.one_sided);
        score += if one_sided { 2.0 } else { 1.5 };
        why.push(if one_sided {
            "one-sided board wipe (great with 4 players)".into()
        } else {
            "board wipe (great with 4 players)".into()
        });
    } else if roles.contains(&Role::Removal) {
        score -= 0.4; // a 1-for-1 only answers one of three opponents
    }
    if roles.contains(&Role::Draw) {
        score += 0.6;
        why.push("card flow matters in long multiplayer games".into());
    }
    if roles.contains(&Role::Ramp) {
        score += 0.4;
    }
    if roles.contains(&Role::Lifegain) {
        score += 0.2;
    }
    if card.text_lower().contains("goad") {
        score += 0.8;
        why.push("goad (politics)".into());
    }
    if card.is_creature() {
        let t = numeric(&card.toughness).unwrap_or(0.0);
        if t >= 4.0 {
            score += 0.25; // deters attacks from three directions
        }
    }
    (score, why)
}

/// How good `card` would be as the next addition to `deck`. Higher is better.
/// This is a heuristic to rank options, not gospel - always read the cards.
pub fn score_card(card: &Card, deck: &Deck, db: &CardDb, rules: &Rules) -> (f32, Vec<String>) {
    let mut why = Vec::new();
    if card.is_land() {
        return (0.0, why);
    }
    let mut score = card_power(card);
    // How much Commander players play it overall, compared with its set (EDHREC).
    if let Some((pts, note)) = crate::edhrec::rank_points(&db.edhrec, &card.name) {
        score += pts;
        why.push(note);
    }
    let roles = card.roles();
    let parsed = card.parsed();
    // Removal is graded by what it can actually answer (see `abilities::Parsed::removal`).
    let removal = parsed.removal();
    let creature = card.is_creature();
    for r in &roles {
        // A creature's abilities are already counted in its value for its cost below.
        if creature
            && matches!(
                r,
                Role::Removal
                    | Role::Counter
                    | Role::Bounce
                    | Role::Draw
                    | Role::Ramp
                    | Role::Tokens
                    | Role::Protection
                    | Role::Evasion
                    | Role::Trick
            )
        {
            continue;
        }
        let (pts, label) = match r {
            Role::Removal => match removal {
                Some(rm) => (rm.points, rm.label),
                None => (2.0, "removal"),
            },
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
    // What the deck lets the card's abilities do (Equipment, creature types).
    let gate = ability_gate(deck, db, rules);
    // "Each opponent loses X life" on a creature counts as far as its condition can be met
    // (Whiplash's drain needs Equipment).
    let each_opponent = if creature {
        parsed
            .abilities
            .iter()
            .filter(|a| a.text.contains("each opponent"))
            .map(|a| gate.of(a))
            .fold(0.0, f32::max)
    } else {
        1.0
    };
    let (mp, mut mp_why) = multiplayer_adjust(card, &roles, rules, each_opponent);
    score += mp;
    why.append(&mut mp_why);
    // A spell that makes creature tokens puts bodies on the board, which the playtests
    // value like a creature's (Borough Backup's two 3/2s measure like a big creature): half
    // what the same bodies score on a creature card (`value::creature_points`), on top of
    // its "makes tokens" role.
    if !creature {
        let bodies: f32 = parsed
            .abilities
            .iter()
            .filter(|a| matches!(a.when.first(), Some(When::Spell | When::Enters)))
            .flat_map(|a| a.effects.iter())
            .map(|e| match e {
                AbEffect::Tokens {
                    amount,
                    power,
                    toughness,
                    ..
                } => amount.value() * (*power as f32 + 0.7 * *toughness as f32).max(0.0),
                _ => 0.0,
            })
            .sum();
        if bodies > 0.0 {
            score += 0.15 * crate::value::BODY_WEIGHT * bodies;
            why.push(format!("creature tokens ({:.0} body points)", bodies));
        }
    }
    // Permanents that keep producing value (each-turn triggers, repeatable abilities) are
    // worth more in long 4-player games.
    if !creature && !matches!(card.main_type(), "Instant" | "Sorcery") {
        let engine = parsed.repeatable_value().min(2.0);
        if engine >= 0.5 {
            score += 0.5 * engine;
            why.push("repeatable value".into());
        }
    }
    // "Choose one that hasn't been chosen" ending in "you lose the game" (Demonic Pact): the
    // last mode comes round a few turns later unless the card leaves first.
    if parsed.all_abilities().any(|a| {
        a.modal
            && a.text.contains("hasn't been chosen")
            && a.effects
                .iter()
                .any(|e| matches!(e, AbEffect::Rule(r) if r.contains("lose the game")))
    }) {
        score -= 12.0;
        why.push("its last mode makes you lose the game".into());
    }
    let st = stats(deck, db);
    let plan = deck_plan(deck, db, rules);
    if card.is_creature() {
        // Body, keywords and abilities against what a fair creature of its cost gives,
        // each ability counted as far as the deck can use it.
        let v = crate::value::creature_value_in(card, &|a| gate.of(a));
        score += crate::value::creature_points(&v);
        let best: Vec<String> = v
            .parts
            .iter()
            .filter(|(_, x)| *x > 0.0)
            .take(3)
            .map(|(l, _)| l.clone())
            .collect();
        why.push(format!(
            "{:.0}% of fair value for {} mana ({})",
            v.efficiency * 100.0,
            card.mana_value(),
            best.join(", ")
        ));
        let creature_target = plan.creatures.0;
        if st.creatures < creature_target {
            score += 1.0;
            why.push(format!(
                "deck needs creatures ({}/{})",
                st.creatures, creature_target
            ));
        }
    }
    // The deck plan's removal and card draw targets, like its creature count.
    let have_removal = st.roles.get(&Role::Removal).copied().unwrap_or(0) as f32;
    if roles.contains(&Role::Removal) && have_removal < plan.removal {
        score += 1.0;
        why.push(format!(
            "deck needs removal ({:.0}/{:.0})",
            have_removal, plan.removal
        ));
    }
    let have_draw = st.roles.get(&Role::Draw).copied().unwrap_or(0) as f32;
    if roles.contains(&Role::Draw) && have_draw < plan.draw {
        score += 0.75;
        why.push(format!(
            "deck needs card draw ({:.0}/{:.0})",
            have_draw, plan.draw
        ));
    }
    let bucket = curve_bucket(card.mana_value());
    let target = plan.curve[bucket];
    if st.curve[bucket] < target {
        score += 0.75;
        why.push(format!("fills the {}-drop slot", curve_label(bucket)));
    } else if st.curve[bucket] >= target + 2 {
        score -= 1.0;
    }
    if card.mana_value() >= 7 {
        score -= 1.5;
    }
    // Payoff cards need their enablers too: a "whenever a token enters" creature is weak in
    // a deck with no token makers (cards that are enablers themselves are exempt).
    let (pen, mut pen_why) = payoff_penalty(card, deck, db, rules);
    score -= pen;
    why.append(&mut pen_why);
    // EDHREC players' picks for this commander (or its partner): what real decks run with it.
    if let Some((pts, note)) = crate::edhrec::pick_bonus(&db.edhrec, &deck.commanders(), &card.name)
    {
        score += pts;
        why.push(note);
    }
    let cmd = deck.commander.as_ref().and_then(|c| db.get(c));
    if let Some(cmd) = cmd {
        let (syn, mut reasons) = synergy(cmd, card);
        score += syn;
        why.append(&mut reasons);
        // ...and with the rest of the deck being built.
        let (ds, mut ds_why) = deck_synergy(card, deck, db);
        score += ds;
        why.append(&mut ds_why);
        // With a 3+ colour commander, adding a colour the deck doesn't cast yet hurts consistency.
        if cmd.identity().count() > 2 && st.nonlands >= 8 {
            let cast = ColorSet(
                (0..5)
                    .filter(|&i| st.pips[i] > 0)
                    .fold(0, |acc, i| acc | (1 << i)),
            )
            .union(cmd.cost_colors());
            if !card.castable_with(cast) {
                score -= 1.0;
                why.push("adds a new colour".into());
            }
        }
    }
    // Tribal pull: cards that care about Heroes/Villains get better with more of them - not
    // cards that merely make a Villain token ("costs {1} less if you control a Villain"
    // counts).
    let text = card.text_lower();
    for tribe in ["Hero", "Villain", "Robot"] {
        let tl = tribe.to_lowercase();
        if cares_about(text, &tl) || text.contains(&format!("control a {}", tl)) {
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
    if let Some((floor, label)) = removal_floor(card, deck, db, rules) {
        if score < floor {
            why.push(format!(
                "{}: rated at least like a solid {}-mana creature ({:.1})",
                label,
                card.mana_value(),
                floor
            ));
            score = floor;
        }
    }
    (score, why)
}

/// How much of a creature's ability the deck can use (0..1), for abilities that need other
/// cards: Equipment ("if he's equipped", "sacrifice an Equipment attached to ~", mana only
/// for Equipment spells) and creature types ("other Villains you control get +2/+1",
/// "whenever another nontoken Hero you control enters").
struct AbilityGate {
    /// Equipment in the deck against the ~6 per 60 cards an Equipment payoff wants.
    equipment: f32,
    /// Creature type (lower case) -> cards of that type against the ~12 per 60 a
    /// type-matters ability wants.
    tribes: Vec<(String, f32)>,
}

impl AbilityGate {
    fn of(&self, a: &crate::abilities::Ability) -> f32 {
        let t = &a.text;
        let mut g: f32 = 1.0;
        if t.contains("equipped")
            || t.contains("sacrifice an equipment")
            || t.contains("equipment spells")
            || t.contains("equipment attached")
        {
            g = g.min(self.equipment);
        }
        for (tribe, ratio) in &self.tribes {
            // A whole word ("elf" isn't the end of "itself, ").
            let word = t.match_indices(tribe.as_str()).any(|(i, _)| {
                !t[..i]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphabetic())
            });
            if word && cares_about(t, tribe) {
                g = g.min(*ratio);
            }
        }
        g
    }
}

fn ability_gate(deck: &Deck, db: &CardDb, rules: &Rules) -> AbilityGate {
    let scale = rules.deck_size as f32 / 60.0;
    let cards = resolved(deck, db);
    let equipment: f32 = cards
        .iter()
        .filter(|(c, _)| c.type_line.contains("Equipment"))
        .map(|(_, n)| *n as f32)
        .sum();
    let mut counts: BTreeMap<String, f32> = BTreeMap::new();
    for (c, n) in &cards {
        if c.is_land() {
            continue;
        }
        for st in c.subtypes() {
            if st == "Hero" || st == "Villain" || c.is_creature() {
                *counts.entry(st.to_lowercase()).or_insert(0.0) += *n as f32;
            }
        }
    }
    let mut tribes: Vec<String> = [
        "hero", "villain", "robot", "human", "soldier", "dragon", "elf", "goblin", "zombie",
        "wizard", "spider", "angel", "cat", "dwarf", "orc",
    ]
    .iter()
    .map(|t| t.to_string())
    .chain(counts.keys().filter(|k| k.len() >= 3).cloned())
    .collect();
    tribes.sort();
    tribes.dedup();
    AbilityGate {
        equipment: (equipment / (6.0 * scale)).min(1.0),
        tribes: tribes
            .into_iter()
            .map(|t| {
                let n = counts.get(&t).copied().unwrap_or(0.0);
                (t, (n / (12.0 * scale)).min(1.0))
            })
            .collect(),
    }
}

thread_local! {
    /// (deck key, the deck's creatures as (mana value, score)) for `removal_floor`.
    static CREATURE_SCORES: std::cell::RefCell<Option<(u64, Vec<(u32, f32)>)>> =
        const { std::cell::RefCell::new(None) };
}

/// What a solid playable creature costing about `mv` scores in `deck`: the median score of
/// the deck's own creatures within one mana of it (all its creatures when fewer than three
/// are that close), or with fewer than three creatures in all, a vanilla body worth about
/// 130% of fair value (`value::fair_value`) for `mv`.
fn solid_creature_score(card: &Card, mv: u32, deck: &Deck, db: &CardDb, rules: &Rules) -> f32 {
    let key = deck_key(deck);
    let cached = CREATURE_SCORES.with(|c| {
        c.borrow()
            .as_ref()
            .filter(|(k, _)| *k == key)
            .map(|(_, v)| v.clone())
    });
    let scores = cached.unwrap_or_else(|| {
        let v: Vec<(u32, f32)> = resolved(deck, db)
            .into_iter()
            .filter(|(c, _)| c.is_creature() && !c.is_land())
            .map(|(c, _)| {
                let mut without = deck.clone();
                without.remove(&c.name, 1);
                (c.mana_value(), score_card(c, &without, db, rules).0)
            })
            .collect();
        CREATURE_SCORES.with(|c| *c.borrow_mut() = Some((key, v.clone())));
        v
    });
    let median = |mut v: Vec<f32>| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[v.len() / 2]
    };
    let near: Vec<f32> = scores
        .iter()
        .filter(|(m, _)| m.abs_diff(mv) <= 1)
        .map(|x| x.1)
        .collect();
    if near.len() >= 3 {
        return median(near);
    }
    if scores.len() >= 3 {
        return median(scores.iter().map(|x| x.1).collect());
    }
    let target = crate::value::fair_value(mv) * 1.3;
    let p = (target / 1.7).round().max(1.0);
    let t = ((target - p) / 0.7).round().max(1.0);
    let solid = Card {
        name: format!("(a solid {}-mana creature)", mv),
        mana_cost: card.mana_cost.clone(),
        cmc: mv as f32,
        type_line: "Creature — Human".into(),
        power: Some(format!("{}", p)),
        toughness: Some(format!("{}", t)),
        rarity: card.rarity.clone(),
        color_identity: card.color_identity.clone(),
        confidence: "verified".into(),
        ..Default::default()
    };
    score_card(&solid, deck, db, rules).0
}

/// A score floor for efficient removal, so it never ranks like filler: what a solid
/// playable creature of about the same cost scores in this deck (`solid_creature_score`),
/// half a point more at instant speed. Removal that answers most creatures at a fair price
/// qualifies - destroy or exile any creature, steal one, 4+ damage or -4/-4 - costing 5 or
/// less; conditional removal (a toughness or mana value limit, 2-3 damage) gets a floor 1.5
/// lower. None for anything else (bounce, taps, X spells, board wipes, and creatures, whose
/// removal is valued with their bodies). Removal costing less than 4 is floored at least like
/// a 4-mana creature: the playtests measure 2- and 3-mana removal (Hero's Downfall, Murdock's
/// Crusade) as worth at least what 4-mana removal is (Hour of Defeat), so a cheaper answer
/// must never rank below a dearer one.
fn removal_floor(
    card: &Card,
    deck: &Deck,
    db: &CardDb,
    rules: &Rules,
) -> Option<(f32, &'static str)> {
    if card.is_creature() || card.is_land() || card.mana_cost.contains("{X}") {
        return None;
    }
    let rm = card.parsed().removal()?;
    let mv = card.mana_value();
    if rm.mass || mv > 5 {
        return None;
    }
    let broad = matches!(
        rm.label,
        "unconditional destroy"
            | "unconditional exile"
            | "exile until it leaves"
            | "steals a creature"
    ) || matches!(rm.label, "damage-based removal" | "-X/-X removal")
        && rm.points >= 2.6;
    let conditional = !broad
        && matches!(
            rm.label,
            "conditional removal" | "damage-based removal" | "-X/-X removal"
        )
        && rm.points >= 1.8;
    if !broad && !conditional {
        return None;
    }
    let base = solid_creature_score(card, mv, deck, db, rules)
        .max(solid_creature_score(card, mv.max(4), deck, db, rules));
    let instant = card.type_line.contains("Instant") || card.text_lower().contains("flash");
    Some(if broad {
        (base + if instant { 0.5 } else { 0.0 }, "efficient removal")
    } else {
        (base - 1.5, "conditional removal")
    })
}

/// Is `word` mentioned in `text` as something the commander *cares about* (e.g. "Villain
/// spell", "other Heroes", "Equipment you control") rather than just named in passing?
fn cares_about(text: &str, word: &str) -> bool {
    let w = word.to_lowercase();
    let plural = format!("{}s", w);
    [
        format!("{} spell", w),
        format!("{} card", w),
        format!("{} you control", w),
        format!("{} you control", plural),
        format!("other {}", w),
        format!("nontoken {}", w),
        format!("{} creatures you control", w),
        format!("each {}", w),
        format!("another {}", w),
        format!("{} enters", w),
        format!("{}, ", w), // lists like "a Hero, Equipment, or Vehicle card"
        format!("or {} card", w),
    ]
    .iter()
    .any(|p| text.contains(p.as_str()))
}

/// A mechanic a commander can reward (the payoff) and the cards that feed it (enablers).
/// Both sides are read from the parsed abilities (see `abilities`), so "whenever a creature
/// you control dies" pairs with sacrifice outlets and token makers, "landfall" with land
/// fetching, "whenever you draw your second card" with card draw and looting, and so on.
struct Mechanic {
    label: &'static str,
    /// What the commander needs, in plain words ("Equipment", "token makers").
    need: &'static str,
    /// Enablers a 60-card deck wants for the payoff to work reliably.
    wanted: f32,
    /// The commander's ability does little without them (an "if equipped" drain, a
    /// "whenever a token enters" trigger). False = just better with them (attack or
    /// landfall triggers still happen without help).
    gates: bool,
    points: f32,
    /// How strongly the commander rewards it (0 = not at all, 1 = a trigger built on it).
    payoff: fn(&Card) -> f32,
    /// How well a card feeds it (0..1).
    enabler: fn(&Card) -> f32,
}

/// How well a card feeds a mechanic (0..1): the rules-text reading, or a Scryfall Tagger tag
/// saying it does (a tag alone counts a bit less than a clear reading).
fn feed(m: &Mechanic, card: &Card) -> f32 {
    let read = (m.enabler)(card).min(1.0);
    if read < 0.7 && crate::tags::feeds(&card.tags, m.need) {
        0.7
    } else {
        read
    }
}

/// The commander's triggered-ability events.
fn events(cmd: &Card) -> Vec<crate::abilities::Event> {
    use crate::abilities::When;
    cmd.parsed()
        .abilities
        .iter()
        .flat_map(|a| a.when.iter())
        .filter_map(|w| match w {
            When::Event(e) => Some(e.clone()),
            _ => None,
        })
        .collect()
}

fn has_event(cmd: &Card, f: fn(&crate::abilities::Event) -> bool) -> bool {
    events(cmd).iter().any(f)
}

/// 1.0 for a matching trigger, 0.6 for a static mention in the commander's text.
fn payoff_by(cmd: &Card, f: fn(&crate::abilities::Event) -> bool, words: &[&str]) -> f32 {
    if has_event(cmd, f) {
        1.0
    } else if words.iter().any(|w| cmd.text_lower().contains(w)) {
        0.6
    } else {
        0.0
    }
}

fn yes(b: bool) -> f32 {
    if b { 1.0 } else { 0.0 }
}

const MECHANICS: &[Mechanic] = &[
    Mechanic {
        label: "token maker for a token commander",
        need: "token makers",
        wanted: 8.0,
        gates: true,
        points: 1.25,
        payoff: |c| {
            payoff_by(
                c,
                |e| matches!(e, Event::Tokens),
                &["tokens you control", "token you control", "for each token"],
            )
        },
        enabler: |c| yes(c.parsed().makes_tokens()),
    },
    Mechanic {
        label: "+1/+1 counters for a counters commander",
        need: "+1/+1 counter cards",
        wanted: 8.0,
        gates: true,
        points: 1.25,
        payoff: |c| {
            payoff_by(
                c,
                |e| matches!(e, Event::Counters),
                &[
                    "+1/+1 counter on it",
                    "with +1/+1 counters",
                    "counters on each",
                    "plus one",
                    "+1/+1 counters you",
                ],
            )
        },
        enabler: |c| {
            yes(c.parsed().effects().any(|(_, e)| {
                matches!(
                    e,
                    AbEffect::PlusCounters { .. } | AbEffect::Proliferate | AbEffect::Amass(_)
                )
            }))
        },
    },
    Mechanic {
        label: "sacrifice fodder/outlet for a death commander",
        need: "sacrifice outlets / fodder",
        wanted: 6.0,
        gates: false,
        points: 1.25,
        payoff: |c| {
            payoff_by(
                c,
                |e| matches!(e, Event::Death | Event::Sacrifice),
                &["morbid", "creature died", "dies this turn"],
            )
        },
        enabler: |c| {
            let p = c.parsed();
            let outlet = p.abilities.iter().any(|a| {
                a.when
                    .iter()
                    .any(|w| matches!(w, When::Activated(cost) if cost.sacrifice_other))
            });
            let dies = p.abilities.iter().any(|a| a.is(&When::Dies));
            (yes(outlet) + 0.7 * yes(p.makes_creature_tokens()) + 0.6 * yes(dies)).min(1.0)
        },
    },
    Mechanic {
        label: "fills the graveyard",
        need: "graveyard fillers",
        wanted: 6.0,
        gates: true,
        points: 1.0,
        payoff: |c| {
            let t = c.text_lower();
            yes(t.contains("threshold")
                || t.contains("cards in your graveyard")
                || t.contains("from your graveyard"))
        },
        enabler: |c| {
            yes(c.parsed().effects().any(|(_, e)| {
                matches!(
                    e,
                    AbEffect::Mill { .. } | AbEffect::Loot | AbEffect::Scry(_)
                )
            }))
        },
    },
    Mechanic {
        label: "instant/sorcery for a spells commander",
        need: "instants and sorceries",
        wanted: 12.0,
        gates: true,
        points: 1.5,
        payoff: |c| {
            payoff_by(
                c,
                |e| matches!(e, Event::Cast(w) if w.contains("instant") || w.contains("sorcery") || w.contains("noncreature")),
                &["instant or sorcery", "noncreature spell"],
            )
        },
        enabler: |c| yes(matches!(c.main_type(), "Instant" | "Sorcery")),
    },
    Mechanic {
        label: "noncreature spell for the commander",
        need: "other noncreature spells",
        wanted: 6.0,
        gates: false,
        points: 0.6,
        payoff: |c| {
            yes(has_event(
                c,
                |e| matches!(e, Event::Cast(w) if w.contains("noncreature")),
            ))
        },
        enabler: |c| {
            yes(!c.is_creature() && !c.is_land() && !matches!(c.main_type(), "Instant" | "Sorcery"))
        },
    },
    Mechanic {
        label: "artifact for an artifact commander",
        need: "artifacts",
        wanted: 10.0,
        gates: true,
        points: 1.25,
        payoff: |c| {
            payoff_by(
                c,
                |e| matches!(e, Event::Cast(w) | Event::OtherEnters(w) if w.contains("artifact")),
                &[
                    "artifact you control",
                    "artifacts you control",
                    "improvise",
                    "affinity for artifacts",
                    "sacrifice an artifact",
                ],
            )
        },
        enabler: |c| {
            (yes(c.type_line.contains("Artifact"))
                + 0.6
                    * yes(c.parsed().effects().any(|(a, e)| {
                        matches!(e, AbEffect::ItemTokens { .. })
                            && !a.text.contains(". they create ")
                    })))
            .min(1.0)
        },
    },
    Mechanic {
        label: "Equipment for an equipped commander",
        need: "Equipment",
        wanted: 6.0,
        gates: true,
        points: 2.0,
        payoff: |c| {
            let t = c.text_lower();
            yes(t.contains("equipped") || t.contains("equipment"))
        },
        enabler: |c| {
            yes(c.type_line.contains("Equipment"))
                + 0.3
                    * yes(c
                        .parsed()
                        .effects()
                        .any(|(_, e)| matches!(e, AbEffect::Attach)))
        },
    },
    Mechanic {
        label: "Vehicle for a crew commander",
        need: "Vehicles",
        wanted: 4.0,
        gates: true,
        points: 1.5,
        payoff: |c| {
            let t = c.text_lower();
            yes(t.contains("vehicle") || t.contains("crew"))
        },
        enabler: |c| yes(c.type_line.contains("Vehicle")),
    },
    Mechanic {
        label: "enchantment for an enchantment commander",
        need: "enchantments",
        wanted: 8.0,
        gates: true,
        points: 1.0,
        payoff: |c| {
            payoff_by(
                c,
                |e| matches!(e, Event::Cast(w) | Event::OtherEnters(w) if w.contains("enchantment")),
                &[
                    "enchantments you control",
                    "enchantment you control",
                    "non-aura enchantment",
                ],
            )
        },
        enabler: |c| yes(c.type_line.contains("Enchantment")),
    },
    Mechanic {
        label: "Plan for a Plan commander",
        need: "Plans",
        wanted: 3.0,
        gates: true,
        points: 1.5,
        payoff: |c| yes(c.text_lower().contains("plan")),
        enabler: |c| yes(c.type_line.contains("Plan") && !c.type_line.contains("Planeswalker")),
    },
    Mechanic {
        label: "extra lands for landfall",
        need: "land fetchers",
        wanted: 4.0,
        gates: false,
        points: 1.0,
        payoff: |c| {
            payoff_by(
                c,
                |e| matches!(e, Event::Landfall),
                &["landfall", "lands you control"],
            )
        },
        enabler: |c| {
            yes(c
                .parsed()
                .effects()
                .any(|(_, e)| matches!(e, AbEffect::FetchLand { .. }))
                || c.text_lower().contains("additional land"))
        },
    },
    Mechanic {
        label: "life gain for a life-gain commander",
        need: "life gain cards",
        wanted: 8.0,
        gates: true,
        points: 1.0,
        payoff: |c| {
            payoff_by(
                c,
                |e| matches!(e, Event::GainLife),
                &["gain life", "gained life"],
            )
        },
        enabler: |c| yes(c.parsed().gains_life()),
    },
    Mechanic {
        label: "extra card draw for a draw commander",
        need: "card draw",
        wanted: 8.0,
        gates: true,
        points: 1.5,
        payoff: |c| {
            payoff_by(
                c,
                |e| matches!(e, Event::Draw),
                &["draw your second card", "second card each turn"],
            )
        },
        enabler: |c| {
            let p = c.parsed();
            yes(p.draws()
                || p.effects()
                    .any(|(_, e)| matches!(e, AbEffect::Dig | AbEffect::Loot)))
        },
    },
    Mechanic {
        label: "discard outlet for a discard commander",
        need: "discard outlets",
        wanted: 5.0,
        gates: true,
        points: 1.25,
        payoff: |c| {
            payoff_by(
                c,
                |e| matches!(e, Event::Discard),
                &["whenever you discard", "discard a nonland card"],
            )
        },
        enabler: |c| {
            let p = c.parsed();
            yes(p.effects().any(|(_, e)| matches!(e, AbEffect::Loot))
                || p.abilities.iter().any(|a| {
                    a.when
                        .iter()
                        .any(|w| matches!(w, When::Activated(cost) if cost.discard))
                }))
        },
    },
    Mechanic {
        label: "helps attacks for an attack commander",
        need: "evasive/haste creatures",
        wanted: 6.0,
        gates: false,
        points: 0.8,
        payoff: |c| {
            let p = c.parsed();
            if p.abilities
                .iter()
                .any(|a| a.is(&When::Attacks) || a.is(&When::CombatDamage))
                || has_event(c, |e| matches!(e, Event::OthersAttack))
            {
                1.0
            } else {
                0.0
            }
        },
        enabler: |c| {
            let p = c.parsed();
            let evasive_creature = c.is_creature() && p.evasive();
            let haste = c.is_creature() && p.has_keyword("haste");
            let team = p.effects().any(|(_, e)| {
                matches!(e, AbEffect::Pump { to, .. } | AbEffect::Grant { to, .. } if to.mass && to.whose == crate::abilities::Whose::Yours)
                    || matches!(e, AbEffect::ExtraCombat)
            });
            (0.7 * yes(evasive_creature) + 0.5 * yes(haste) + yes(team)).min(1.0)
        },
    },
    Mechanic {
        label: "enters-the-battlefield value for an ETB commander",
        need: "enters-effect creatures",
        wanted: 8.0,
        gates: false,
        points: 0.8,
        payoff: |c| {
            yes(has_event(
                c,
                |e| matches!(e, Event::OtherEnters(w) if w.contains("creature") || w.contains("another") || w.contains("nontoken")),
            ))
        },
        enabler: |c| {
            let p = c.parsed();
            yes(c.is_creature()
                && p.abilities
                    .iter()
                    .any(|a| a.is(&When::Enters) && !a.effects.is_empty()))
            .max(yes(p.effects().any(|(_, e)| matches!(e, AbEffect::Blink))))
        },
    },
    Mechanic {
        label: "legendary/artifact for a Storied commander",
        need: "legendaries/artifacts/Sagas",
        wanted: 8.0,
        gates: true,
        points: 1.0,
        payoff: |c| {
            let p = c.parsed();
            yes(p.has_keyword("storied")
                || c.text_lower().contains("enduring story")
                || c.text_lower().contains("legendary spell"))
        },
        enabler: |c| {
            yes(c.is_legendary()
                || c.type_line.contains("Artifact")
                || c.type_line.contains("Saga"))
        },
    },
    Mechanic {
        label: "empowers Jace for a planeswalker commander",
        need: "Jace/planeswalker cards",
        wanted: 5.0,
        gates: true,
        points: 1.25,
        payoff: |c| {
            let t = c.text_lower();
            // Cards that reward planeswalkers - not removal that can hit one ("destroy target
            // creature or planeswalker").
            yes(t.contains("planeswalker you control")
                || t.contains("planeswalkers you control")
                || t.contains("loyalty")
                || t.contains("empower"))
        },
        enabler: |c| {
            yes(c.type_line.contains("Planeswalker")
                || c.parsed()
                    .effects()
                    .any(|(_, e)| matches!(e, AbEffect::Empower(_))))
        },
    },
    Mechanic {
        label: "teamwork spell (taps your creatures)",
        need: "Teamwork spells",
        wanted: 4.0,
        gates: false,
        points: 0.6,
        payoff: |c| {
            // Commanders with big bodies or that make creatures pay teamwork costs easily.
            let power: i32 = c.power.as_deref().and_then(|v| v.parse().ok()).unwrap_or(0);
            (yes(power >= 4)
                + yes(c.parsed().makes_creature_tokens())
                + yes(c.text_lower().contains("teamwork")))
            .min(1.0)
        },
        enabler: |c| yes(c.parsed().has_keyword("teamwork")),
    },
    Mechanic {
        label: "damage effect boosted by the commander",
        need: "damage spells",
        wanted: 8.0,
        gates: true,
        points: 1.5,
        payoff: |c| {
            let t = c.text_lower();
            yes(t.contains("noncombat damage")
                || c.parsed()
                    .effects()
                    .any(|(_, e)| matches!(e, AbEffect::Doubler(k) if k == "damage")))
        },
        enabler: |c| {
            yes(!c.is_creature()
                && c.parsed()
                    .effects()
                    .any(|(_, e)| matches!(e, AbEffect::Damage { .. } | AbEffect::Drain { .. })))
        },
    },
    Mechanic {
        label: "big creature for a big-creature commander",
        need: "big creatures",
        wanted: 8.0,
        gates: true,
        points: 1.0,
        payoff: |c| {
            let t = c.text_lower();
            yes(t.contains("power 4 or greater")
                || t.contains("greatest power")
                || t.contains("mana value 5 or greater")
                || t.contains("mana value 4 or greater"))
        },
        enabler: |c| {
            let power: i32 = c.power.as_deref().and_then(|v| v.parse().ok()).unwrap_or(0);
            yes(c.is_creature() && (power >= 4 || c.mana_value() >= 5))
        },
    },
    Mechanic {
        label: "mana for the commander's expensive abilities",
        need: "mana sources",
        wanted: 6.0,
        gates: false,
        points: 0.6,
        payoff: |c| {
            let p = c.parsed();
            yes(p.abilities.iter().any(|a| {
                a.when.iter().any(
                    |w| matches!(w, When::PowerUp(cost) | When::Activated(cost) if cost.mana >= 4),
                )
            }) || c.mana_cost.contains("{X}"))
        },
        enabler: |c| yes(c.parsed().ramps()),
    },
    Mechanic {
        label: "damages your creatures for an Enrage commander",
        need: "ways to damage your creatures",
        wanted: 4.0,
        // Combat damage triggers Enrage too, so it isn't dead without help.
        gates: false,
        points: 1.25,
        payoff: |c| yes(has_event(c, |e| matches!(e, Event::Damaged))),
        enabler: |c| {
            yes(c.parsed().effects().any(|(_, e)| {
                matches!(e, AbEffect::Fight(_)) || matches!(e, AbEffect::Damage { to, .. } if to.mass && to.whose != crate::abilities::Whose::Theirs)
            }))
        },
    },
    Mechanic {
        label: "grows your Army",
        need: "amass cards",
        wanted: 5.0,
        gates: true,
        points: 1.0,
        payoff: |c| {
            let t = c.text_lower();
            yes(t.contains("army") || t.contains("amass"))
        },
        enabler: |c| {
            yes(c
                .parsed()
                .effects()
                .any(|(_, e)| matches!(e, AbEffect::Amass(_))))
        },
    },
];

/// Bonus for cards that play into what the commander rewards: creature types it cares
/// about (Villain spells, other Heroes...) and the mechanics in `MECHANICS`.
pub fn synergy(cmd: &Card, card: &Card) -> (f32, Vec<String>) {
    let ct = cmd.text_lower();
    let mut score: f32 = 0.0;
    let mut why = Vec::new();
    for sub in card.subtypes() {
        let s = sub.to_lowercase();
        if s.len() < 3 || !ct.contains(&s) {
            continue;
        }
        if cares_about(ct, &s) {
            score += 2.0;
            why.push(format!("commander cares about {}", sub));
        } else {
            score += 0.4;
            why.push(format!("{} (mentioned by commander)", sub));
        }
    }
    if card.name != cmd.name {
        for m in MECHANICS {
            let pay = (m.payoff)(cmd);
            if pay <= 0.0 {
                continue;
            }
            let feed = feed(m, card);
            if feed > 0.0 {
                score += m.points * pay * feed;
                why.push(m.label.to_string());
            }
        }
    }
    // Stop one card from looking amazing just because it ticks many boxes.
    (score.min(4.0), why)
}

/// How well a deck supplies what its commander's abilities depend on.
#[derive(Debug, Clone)]
pub struct Support {
    pub need: String,
    /// Payoff strength on the commander (1 = a trigger built on it).
    pub payoff: f32,
    /// Enablers in the deck (cards weighted by how well they feed it, times copies).
    pub have: f32,
    pub wanted: f32,
    /// Without enablers the ability does little.
    pub gates: bool,
}

impl Support {
    /// 0..1: how much of what's wanted the deck has.
    pub fn ratio(&self) -> f32 {
        (self.have / self.wanted.max(1.0)).min(1.0)
    }
}

/// What `cmd`'s abilities depend on, and how much of it `deck` (or a pool) has: the
/// mechanics in `MECHANICS`, plus creature types the commander cares about (a "Villain
/// spell" commander needs Villains).
pub fn commander_support(cmd: &Card, deck: &Deck, db: &CardDb, rules: &Rules) -> Vec<Support> {
    let scale = rules.deck_size as f32 / 60.0;
    let cards: Vec<(&Card, u32)> = resolved(deck, db)
        .into_iter()
        .filter(|(c, _)| c.name != cmd.name && !c.is_basic())
        .collect();
    let mut out = Vec::new();
    for m in MECHANICS {
        let payoff = (m.payoff)(cmd);
        if payoff <= 0.0 {
            continue;
        }
        let have: f32 = cards.iter().map(|(c, n)| feed(m, c) * *n as f32).sum();
        out.push(Support {
            need: m.need.to_string(),
            payoff,
            have,
            wanted: m.wanted * scale,
            gates: m.gates,
        });
    }
    let ct = cmd.text_lower();
    for sub in [
        "villain", "hero", "dwarf", "elf", "goblin", "orc", "human", "robot", "cat", "dragon",
        "angel", "zombie", "wizard", "soldier", "spider",
    ] {
        if cares_about(ct, sub) {
            let have: f32 = cards
                .iter()
                .filter(|(c, _)| c.subtypes().iter().any(|t| t.eq_ignore_ascii_case(sub)))
                .map(|(_, n)| *n as f32)
                .sum();
            let mut name = sub.to_string();
            name[..1].make_ascii_uppercase();
            out.push(Support {
                need: format!("{}s", name),
                payoff: 1.0,
                have,
                wanted: 12.0 * scale,
                gates: true,
            });
        }
    }
    out
}

/// Penalty for a card whose own triggered ability depends on enablers the deck lacks.
fn payoff_penalty(card: &Card, deck: &Deck, db: &CardDb, rules: &Rules) -> (f32, Vec<String>) {
    let mut pen = 0.0;
    let mut why = Vec::new();
    // "Costs {2} less if you control a Vehicle" works without one - it's a discount, not a payoff.
    let t = card.text_lower();
    if t.contains("less to cast") || t.contains("costs {") {
        return (pen, why);
    }
    let scale = rules.deck_size as f32 / 60.0;
    for m in MECHANICS.iter().filter(|m| m.gates) {
        if (m.payoff)(card) < 1.0 || feed(m, card) > 0.0 {
            continue;
        }
        let have: f32 = resolved(deck, db)
            .iter()
            .filter(|(c, _)| c.name != card.name)
            .map(|(c, n)| feed(m, c) * *n as f32)
            .sum();
        let ratio = (have / (m.wanted * scale).max(1.0)).min(1.0);
        if ratio < 1.0 {
            pen += 1.2 * (1.0 - ratio);
            why.push(format!(
                "needs {}: {:.0} in deck (wants ~{:.0})",
                m.need,
                have,
                m.wanted * scale
            ));
        }
    }
    (pen, why)
}

/// What the rest of a deck does, for judging how a card fits into it: per mechanic, how
/// strongly the deck's cards reward it (payoffs) and feed it (enablers), plus which creature
/// types its cards care about. Built once per deck version and reused while auto-build
/// scores every candidate against the same deck.
struct DeckProfile {
    key: u64,
    /// Per `MECHANICS` entry: (payoff strength x copies, enabler strength x copies).
    mech: Vec<(f32, f32)>,
    /// Per card name: its own (payoff, feed) per mechanic, so a card isn't its own partner.
    own: std::collections::HashMap<String, Vec<(f32, f32)>>,
    /// Creature type -> deck cards that care about it (filled on demand).
    tribes: std::collections::HashMap<String, u32>,
    texts: Vec<(String, String, u32)>,
}

fn deck_key(deck: &Deck) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    deck.commander.hash(&mut h);
    deck.partner.hash(&mut h);
    deck.cards.hash(&mut h);
    h.finish()
}

thread_local! {
    static PROFILE: std::cell::RefCell<Option<DeckProfile>> = const { std::cell::RefCell::new(None) };
}

fn with_profile<R>(deck: &Deck, db: &CardDb, f: impl FnOnce(&mut DeckProfile) -> R) -> R {
    let key = deck_key(deck);
    PROFILE.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.as_ref().is_none_or(|p| p.key != key) {
            let cmds: Vec<&String> = deck.commanders();
            let cards: Vec<(&Card, u32)> = resolved(deck, db)
                .into_iter()
                .filter(|(c, _)| !c.is_land() && !cmds.contains(&&c.name))
                .collect();
            let mut mech = vec![(0.0, 0.0); MECHANICS.len()];
            let mut own = std::collections::HashMap::new();
            for (c, n) in &cards {
                let mine: Vec<(f32, f32)> = MECHANICS
                    .iter()
                    .map(|m| (deck_payoff(m, c), feed(m, c)))
                    .collect();
                for (k, (pay, fd)) in mine.iter().enumerate() {
                    mech[k].0 += pay * *n as f32;
                    mech[k].1 += fd * *n as f32;
                }
                own.insert(c.name.clone(), mine);
            }
            *slot = Some(DeckProfile {
                key,
                mech,
                own,
                tribes: std::collections::HashMap::new(),
                texts: cards
                    .iter()
                    .map(|(c, n)| (c.name.clone(), c.text_lower().to_string(), *n))
                    .collect(),
            });
        }
        f(slot.as_mut().unwrap())
    })
}

/// A card's payoff for a mechanic, for deck synergy: only clear payoffs (the looser
/// commander heuristics - "big creatures pay teamwork costs" - don't count for every card).
fn deck_payoff(m: &Mechanic, c: &Card) -> f32 {
    if m.need == "Teamwork spells" {
        // Cards that reward teamwork, not the teamwork spells themselves.
        return yes(c.text_lower().contains("teamwork") && !c.parsed().has_keyword("teamwork"));
    }
    let p = (m.payoff)(c);
    if p >= 0.75 { p } else { 0.0 }
}

/// How well a card fits the deck being built (beyond its commander): it feeds payoffs
/// already in the deck, its own payoff is fed by the deck's enablers, or it's a creature
/// type other cards in the deck care about. About 0 .. +2.
pub fn deck_synergy(card: &Card, deck: &Deck, db: &CardDb) -> (f32, Vec<String>) {
    if card.is_land() {
        return (0.0, Vec::new());
    }
    with_profile(deck, db, |prof| {
        let mut score = 0.0;
        let mut why = Vec::new();
        let copies = deck.count_of(&card.name) as f32;
        let own = prof.own.get(&card.name).cloned();
        let mut mech_total = 0.0;
        for (k, m) in MECHANICS.iter().enumerate() {
            let (mut payoffs, mut enablers) = prof.mech[k];
            if let Some(o) = &own {
                payoffs -= o[k].0 * copies;
                enablers -= o[k].1 * copies;
            }
            let fd = feed(m, card);
            let pay = deck_payoff(m, card);
            let mut pts = 0.0;
            if fd > 0.0 && payoffs >= 0.75 {
                pts += fd * 0.2 * payoffs.min(4.0);
                why.push(format!(
                    "feeds {:.0} {} payoff(s) in the deck",
                    payoffs, m.need
                ));
            }
            if pay > 0.0 && enablers >= 1.0 {
                pts += pay * 0.15 * enablers.min(6.0);
                why.push(format!("{:.0} {} in the deck feed it", enablers, m.need));
            }
            mech_total += pts.min(1.0);
        }
        score += mech_total.min(1.5);
        // Creature types the deck's other cards care about ("other Villains you control").
        if card.is_creature() {
            let changeling = card.parsed().has_keyword("changeling");
            let subs: Vec<String> = if changeling {
                vec!["hero".into(), "villain".into()]
            } else {
                card.subtypes().iter().map(|s| s.to_lowercase()).collect()
            };
            for sub in subs.iter().filter(|s| s.len() >= 3) {
                let n = match prof.tribes.get(sub) {
                    Some(n) => *n,
                    None => {
                        let n: u32 = prof
                            .texts
                            .iter()
                            .filter(|(name, text, _)| name != &card.name && cares_about(text, sub))
                            .map(|(_, _, n)| *n)
                            .sum();
                        prof.tribes.insert(sub.clone(), n);
                        n
                    }
                };
                if n > 0 {
                    score += 0.3 * n.min(4) as f32;
                    why.push(format!("{} card(s) in the deck care about {}s", n, sub));
                }
            }
        }
        (score.min(2.0), why)
    })
}

/// How much of a commander's ability value its deck can actually use (0.25..1).
/// Only abilities that need enablers count; a commander with none is fully supported.
pub fn support_factor(support: &[Support]) -> f32 {
    let gated: Vec<&Support> = support.iter().filter(|s| s.gates).collect();
    if gated.is_empty() {
        return 1.0;
    }
    let weight: f32 = gated.iter().map(|s| s.payoff).sum();
    let met: f32 = gated.iter().map(|s| s.payoff * s.ratio()).sum::<f32>() / weight;
    0.25 + 0.75 * met
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
    if !rules.enforce_collection {
        return cap;
    }
    // The Prismatic Piper is free only as a partner; copies in the main deck must be owned.
    let counted = if card.name == crate::deck::PIPER && rules.piper_free {
        deck.count_of(&card.name)
    } else {
        used
    };
    coll.owned(&card.name).saturating_sub(counted).min(cap)
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
        .filter(|c| colors.is_none_or(|lim| c.playable_with(lim)))
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
    // "Our colours" include what the commander's abilities cost (Thanos's power-up needs
    // {C}, blue and green on top of his red-white-black casting cost).
    let (ability_colors, needs_colorless) = commander.ability_colors();
    let wanted = plays.union(ColorSet(ability_colors.0 & identity.0));
    for land in db.cards.iter().filter(|c| c.is_land() && !c.is_basic()) {
        let ok_identity = !rules.enforce_color_identity || land.identity().is_subset_of(identity);
        let useful = land.produces().0 & wanted.0 != 0 || needs_colorless && land.makes_colorless();
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
pub fn pool_quality_for(
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
        .filter(|c| !c.is_land() && c.playable_with(colors))
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

/// What one more colour than the commander's cost colours costs a deck (in average card
/// score): fewer sources for each colour, more awkward hands. The playtest lab measured
/// Thanos (cost WBR) rebuilt in WUBRG and WBRG about 2-3 points ahead of WBR (WUBR a little
/// behind); his pool picks WUBRG with this, and the 2-3 point gain doesn't need more.
const EXTRA_COLOR: f32 = 0.1;

/// For commanders with 3+ colours in their identity, picks the subset of colours to actually
/// play: always the colours in the commander's own cost, plus the best others, at most
/// max(2, cost colours) in total - or the whole identity when the commander's own abilities
/// cost more colours than it does (Thanos's power-up wants all five: the deck runs lands for
/// them anyway, so their spells come almost free). Each colour past the cost colours has to
/// raise the pool's average card score by `EXTRA_COLOR` to be picked. Two-colour-or-less
/// commanders just use their identity.
pub fn best_colors(cmd: &Card, db: &CardDb, coll: &Collection, rules: &Rules) -> ColorSet {
    let id = cmd.identity();
    if id.count() <= 2 {
        return id;
    }
    let base = cmd.cost_colors();
    let wants = ColorSet(cmd.ability_colors().0.0 & id.0);
    let max = if wants.is_subset_of(base) {
        base.count().max(2)
    } else {
        id.count()
    };
    let mut best = (f32::MIN, id);
    for mask in 1u8..32 {
        let s = ColorSet(mask);
        if !s.is_subset_of(id) || !base.is_subset_of(s) || s.count() > max {
            continue;
        }
        let extra = s.count().saturating_sub(base.count().max(2)) as f32;
        let q = pool_quality_for(cmd, s, db, coll, rules) - EXTRA_COLOR * extra;
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
            .filter(|c| !c.is_land() && c.playable_with(plays))
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
        let quality = pool_quality_for(cmd, plays, db, coll, rules);
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
    /// A repeatable sequence: the same seed always gives the same shuffles.
    pub fn new(seed: u64) -> Rng {
        let mut r = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1);
        r.next();
        r
    }

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
    // A fixed seed: every deck is dealt the same shuffles, so results don't change from
    // run to run and decks are compared on equal luck.
    let mut rng = Rng::new(0x5EED_CAFE);
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
        (CardDb::new(cards), Collection::new(PathBuf::new(), counts))
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
    fn efficient_removal_scores_like_a_solid_creature() {
        let (mut db, coll) = setup();
        let rules = Rules::default();
        let (deck, _) = autobuild(db.get("Boss").unwrap(), &db, &coll, &rules, None, None);
        let mut cards = db.cards.clone();
        cards.push(mk(
            "Murder",
            "{1}{B}{B}",
            "Instant",
            "Destroy target creature.",
            "B",
        ));
        cards.push(mk(
            "Small Kill",
            "{1}{B}",
            "Sorcery",
            "Destroy target creature with mana value 2 or less.",
            "B",
        ));
        cards.push(mk(
            "Slow Murder",
            "{3}{B}",
            "Instant",
            "Destroy target creature.",
            "B",
        ));
        cards.push(mk(
            "Unsummon",
            "{U}",
            "Instant",
            "Return target creature to its owner's hand.",
            "U",
        ));
        db = CardDb::new(cards);
        let score = |n: &str| score_card(db.get(n).unwrap(), &deck, &db, &rules);
        // The deck's creatures within a mana of `mv`, as the builder rates them: the median.
        let solid_at = |mv: u32| {
            let mut near: Vec<f32> = resolved(&deck, &db)
                .into_iter()
                .filter(|(c, _)| c.is_creature() && c.mana_value().abs_diff(mv) <= 1)
                .map(|(c, _)| {
                    let mut without = deck.clone();
                    without.remove(&c.name, 1);
                    score_card(c, &without, &db, &rules).0
                })
                .collect();
            near.sort_by(|a, b| a.partial_cmp(b).unwrap());
            near[near.len() / 2]
        };
        let solid = solid_at(3).max(solid_at(4));
        let (murder, why) = score("Murder");
        assert!(
            murder >= solid + 0.5 - 1e-3,
            "instant unconditional removal at least a solid 4-drop and a half ({} vs {}): {:?}",
            murder,
            solid,
            why
        );
        // The same answer for more mana never ranks above the cheap one.
        let (slow, _) = score("Slow Murder");
        assert!(slow <= murder + 1e-3, "{} vs {}", slow, murder);
        let (small, _) = score("Small Kill");
        assert!(
            small < murder,
            "conditional removal stays lower: {} vs {}",
            small,
            murder
        );
        // Small Kill costs two: its floor is a solid 2- or 4-drop's, less 1.5.
        assert!(small >= solid_at(2).max(solid_at(4)) - 1.5 - 1e-3);
        let (bounce, why) = score("Unsummon");
        assert!(
            !why.iter().any(|w| w.contains("rated at least")),
            "no floor for bounce: {} {:?}",
            bounce,
            why
        );
    }

    /// `setup`'s pool plus `extra` cards, and Boss's auto-built deck from the original pool.
    fn with_cards(extra: Vec<Card>) -> (CardDb, Deck) {
        let (db, coll) = setup();
        let rules = Rules::default();
        let (deck, _) = autobuild(db.get("Boss").unwrap(), &db, &coll, &rules, None, None);
        let mut cards = db.cards.clone();
        cards.extend(extra);
        cards.sort_by(|a, b| a.name.cmp(&b.name));
        (CardDb::new(cards), deck)
    }

    #[test]
    fn equipment_payoffs_need_equipment() {
        let mut whip = mk(
            "Whip",
            "{B}",
            "Legendary Creature — Human Villain",
            "Whenever Whip attacks, if he's equipped, each opponent loses 3 life and you gain 3 life.",
            "B",
        );
        whip.rarity = "rare".into();
        let mut plain = whip.clone();
        plain.name = "Plain Whip".into();
        plain.oracle_text =
            "Whenever Plain Whip attacks, each opponent loses 3 life and you gain 3 life.".into();
        let sword = mk(
            "Sword",
            "{1}",
            "Artifact — Equipment",
            "Equipped creature gets +2/+0. Equip {2}",
            "",
        );
        let (db, deck) = with_cards(vec![whip, plain, sword]);
        let rules = Rules::default();
        let score = |n: &str, d: &Deck| score_card(db.get(n).unwrap(), d, &db, &rules).0;
        // No Equipment: the drain is worth nothing, and doesn't hit every opponent either.
        let (s, why) = score_card(db.get("Whip").unwrap(), &deck, &db, &rules);
        assert!(score("Plain Whip", &deck) > s + 2.0, "{} {:?}", s, why);
        assert!(!why.iter().any(|w| w.contains("hits all")), "{:?}", why);
        // Six Equipment: full value.
        let mut geared = deck.clone();
        geared.add("Sword", 6);
        assert!((score("Plain Whip", &geared) - score("Whip", &geared)).abs() < 1.0);
    }

    #[test]
    fn tribal_lords_and_pull_follow_the_deck() {
        let lord = mk(
            "Lord",
            "{3}{B}",
            "Creature — Human Villain",
            "Other Villains you control get +2/+1.",
            "B",
        );
        let maker = mk(
            "Maker",
            "{1}{B}",
            "Creature — Human Soldier",
            "When this creature dies, create a 2/1 black Villain creature token with menace.",
            "B",
        );
        let (db, villains) = with_cards(vec![lord, maker]);
        let rules = Rules::default();
        // The same deck with its Villains swapped for Heroes of the same costs.
        let mut cards = db.cards.clone();
        for c in cards.iter_mut() {
            if c.name.starts_with("Goon") {
                c.type_line = "Creature — Human Hero".into();
            }
        }
        let heroes_db = CardDb::new(cards);
        let lord_with = score_card(db.get("Lord").unwrap(), &villains, &db, &rules).0;
        let (lord_without, why) = score_card(
            heroes_db.get("Lord").unwrap(),
            &villains,
            &heroes_db,
            &rules,
        );
        assert!(
            lord_with > lord_without + 2.0,
            "a Villain lord needs Villains: {} vs {} {:?}",
            lord_with,
            lord_without,
            why
        );
        // Making a Villain token isn't caring about Villains.
        let (_, why) = score_card(db.get("Maker").unwrap(), &villains, &db, &rules);
        assert!(
            !why.iter().any(|w| w.contains("Villains in deck")),
            "{:?}",
            why
        );
        let (_, why) = score_card(db.get("Lord").unwrap(), &villains, &db, &rules);
        assert!(
            why.iter().any(|w| w.contains("Villains in deck")),
            "{:?}",
            why
        );
    }

    #[test]
    fn token_spells_count_their_bodies() {
        let army = mk(
            "Army",
            "{3}{R}",
            "Sorcery",
            "Create two 3/2 red Soldier creature tokens with vigilance.",
            "R",
        );
        let flare = mk(
            "Flare",
            "{3}{R}",
            "Sorcery",
            "Create a 1/1 red Soldier creature token.",
            "R",
        );
        let (db, deck) = with_cards(vec![army, flare]);
        let rules = Rules::default();
        let (army, why) = score_card(db.get("Army").unwrap(), &deck, &db, &rules);
        let flare = score_card(db.get("Flare").unwrap(), &deck, &db, &rules).0;
        assert!(army > flare + 2.0, "{} vs {} {:?}", army, flare, why);
        assert!(
            why.iter().any(|w| w.contains("creature tokens")),
            "{:?}",
            why
        );
    }

    #[test]
    fn colours_the_commanders_abilities_want_are_played() {
        let (db, coll) = setup();
        let rules = Rules::default();
        let boss = db.get("Boss").unwrap();
        // A three-colour identity without colourful abilities: at most the cost colours.
        let mut tri = boss.clone();
        tri.color_identity = ["B", "R", "G"].iter().map(|c| c.to_string()).collect();
        assert_eq!(best_colors(&tri, &db, &coll, &rules), boss.cost_colors());
        // A black-red commander in a five-colour identity whose ability costs all five
        // colours (like Thanos's power-up) may play every colour its pool supports.
        let mut titan = boss.clone();
        titan.name = "Titan Boss".into();
        titan.color_identity = ["W", "U", "B", "R", "G"]
            .iter()
            .map(|c| c.to_string())
            .collect();
        titan.oracle_text = "{W}{U}{B}{R}{G}: Draw a card.".into();
        let mut counts = coll.counts.clone();
        counts.insert("Titan Boss".into(), 1);
        let mut cards = db.cards.clone();
        cards.push(titan);
        // Strong green cards: worth a fourth colour.
        for i in 0..12 {
            let name = format!("Beast {}", i);
            let mut c = mk(&name, "{2}{G}", "Creature — Beast", "", "G");
            c.power = Some("5".into());
            c.toughness = Some("5".into());
            c.rarity = "mythic".into();
            cards.push(c);
            counts.insert(name, 1);
        }
        cards.sort_by(|a, b| a.name.cmp(&b.name));
        let db = CardDb::new(cards);
        let coll = Collection::new(PathBuf::new(), counts);
        let titan = db.get("Titan Boss").unwrap();
        let picked = best_colors(titan, &db, &coll, &rules);
        assert!(picked.count() >= 3 && picked.contains(4), "{}", picked);
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

#[cfg(test)]
mod power_tests {
    use super::*;

    #[test]
    fn rarity_nudges_card_power() {
        let card = |rarity: &str| Card {
            name: "Test".into(),
            rarity: rarity.into(),
            ..Default::default()
        };
        assert!((card_power(&card("common")) - 1.25).abs() < 1e-4);
        assert!((card_power(&card("mythic")) - 2.0).abs() < 1e-4);
        assert!((card_power(&card("special")) - 1.5).abs() < 1e-4);
    }

    #[test]
    fn land_recommendation_stops_at_the_goal() {
        let rules = Rules::default();
        assert_eq!(land_goal(&rules), 26);
        assert_eq!(recommended_lands(2.4, &rules), 25);
        assert_eq!(recommended_lands(3.0, &rules), 26);
        assert_eq!(recommended_lands(4.2, &rules), 26);
    }
}
