//! What a card's body and abilities are worth, on one scale: 1 point is about one point of
//! power. A creature's total is compared with what a fair creature of its mana cost gives
//! (`fair_value`), so a cheap creature with useful abilities rates above a big vanilla one.
//! Abilities that matter more in 4-player Commander (card draw, removal, repeating engines,
//! effects that hit every opponent) are worth more than ones that don't.

use crate::abilities::{Ability, Effect, When, Whose};
use crate::card::Card;

/// Ability points are on the card-points scale (a card drawn = 1.3); this puts them on the
/// body scale: a fair 2-mana "1/1, draw a card when it enters" (Elvish Visionary) and a
/// strong 4-mana "2/2, destroy a creature when it enters" (Ravenous Chupacabra) land where
/// players rate them.
pub const ABILITY_SCALE: f32 = 2.0;

/// What one effect is worth on its own (card points; the board decides the rest).
pub fn effect_value(e: &Effect) -> f32 {
    match e {
        Effect::Destroy(t) | Effect::Exile { target: t, .. } if t.whose != Whose::Yours => {
            if t.mass {
                5.0
            } else if t.restricted {
                2.5
            } else {
                3.5
            }
        }
        Effect::Damage { amount, to } if to.whose != Whose::Yours => {
            let n = amount.n.min(6) as f32;
            if to.mass { n * 1.2 } else { 0.5 + n * 0.5 }
        }
        Effect::Fight(_) => 2.5,
        Effect::Shrink { amount, to, .. } if to.whose != Whose::Yours => {
            amount.n.min(5) as f32 * if to.mass { 1.0 } else { 0.6 }
        }
        Effect::Bounce(t) if t.whose != Whose::Yours => 1.5,
        Effect::Lock { lasting, .. } => {
            if *lasting {
                3.0
            } else {
                0.7
            }
        }
        Effect::Edict { .. } => 2.5,
        Effect::CounterSpell => 3.0,
        Effect::Draw { amount, you: true } => 1.3 * amount.value(),
        Effect::Loot => 0.6,
        Effect::Discard { opponents: true } => 1.0,
        Effect::Mill { .. } => 0.3,
        Effect::Scry(n) => 0.25 * n.value(),
        Effect::Tokens {
            amount,
            power,
            toughness,
            keywords,
        } => {
            amount.value()
                * (0.3 + (*power + *toughness).max(0) as f32 * 0.4 + keywords.len() as f32 * 0.2)
        }
        Effect::ItemTokens { amount, kind } => {
            amount.value()
                * match kind.as_str() {
                    "treasure" | "clue" => 1.0,
                    "food" | "map" => 0.6,
                    _ => 0.5,
                }
        }
        Effect::PlusCounters { amount, to } => amount.value() * if to.mass { 2.0 } else { 0.8 },
        Effect::Pump {
            power,
            toughness,
            to,
            until_eot,
        } => {
            let base = (*power + *toughness).max(0) as f32;
            match (to.mass, until_eot) {
                (true, false) => base * 1.5,
                (true, true) => base * 0.8,
                (false, false) => base * 0.6,
                (false, true) => base * 0.3,
            }
        }
        Effect::Grant {
            keywords,
            until_eot,
            to,
        } => {
            keywords.len() as f32
                * match (to.mass, until_eot) {
                    (true, false) => 1.5,
                    (true, true) => 0.6,
                    (false, false) => 0.8,
                    (false, true) => 0.3,
                }
        }
        Effect::GainLife(n) => n.value() * 0.12,
        Effect::Drain { amount, each } => amount.value() * if *each { 1.0 } else { 0.4 },
        Effect::AddMana(n) => n.value() * 0.5,
        Effect::FetchLand { battlefield } => {
            if *battlefield {
                1.8
            } else {
                0.8
            }
        }
        Effect::Reanimate => 4.0,
        Effect::Regrowth => 1.8,
        Effect::Recast => 1.5,
        Effect::Untap(_) => 0.5,
        Effect::GainControl(_) => 4.0,
        Effect::Copy => 0.8,
        Effect::Empower(n) => 0.6 * n.value(),
        Effect::Amass(n) => n.value() * 0.9,
        Effect::Goad => 1.0,
        Effect::Proliferate => 1.0,
        Effect::ExtraCombat => 3.0,
        Effect::Blink => 1.5,
        Effect::Tutor => 2.0,
        Effect::FreeSpell => 2.5,
        Effect::Prepared => 1.0,
        Effect::Attach => 0.8,
        Effect::Doubler(_) => 2.0,
        Effect::Fog => 1.0,
        Effect::Dig => 1.0,
        Effect::OtherCounters(_) => 0.3,
        // Drawbacks: Demonic Pact's last mode, pain lands, Phyrexian Arena's life.
        Effect::Rule(r) if r.contains("lose the game") => -12.0,
        Effect::Rule(r) if r == "damage to you" => -0.3,
        Effect::Rule(r) if r.starts_with("you lose ") => {
            -0.15
                * r.split_whitespace()
                    .nth(2)
                    .and_then(|n| n.parse::<f32>().ok())
                    .unwrap_or(1.0)
        }
        _ => 0.2,
    }
}

/// How many times an ability happens over a typical game, relative to once.
fn times(a: &Ability) -> f32 {
    let w = a.when.first();
    let base = match w {
        Some(When::Spell | When::Enters) => 1.0,
        Some(When::Dies) => 0.6,
        Some(When::Attacks) => 1.5,
        Some(When::CombatDamage) => 1.2,
        Some(When::EachTurn) => 2.2,
        Some(When::Event(_)) => 1.0 + 1.5 * a.rate(),
        // `rate` already falls with the ability's mana cost.
        Some(When::Activated(c)) => {
            if c.sacrifice {
                0.8
            } else {
                1.0 + 1.5 * a.rate()
            }
        }
        // Once only, and its mana has to be found (cheaper the turn it enters).
        Some(When::PowerUp(c)) => (0.8 * (1.0 - 0.05 * c.mana as f32)).max(0.4),
        Some(When::Loyalty(_)) => 1.2,
        Some(When::Chapter(_)) => 1.0,
        Some(When::Reflexive) => 0.5,
        Some(When::Static) | None => 1.5,
    };
    let cond = if !a.conditional {
        1.0
    } else if a.text.contains("teamwork") {
        0.7
    } else {
        0.6
    };
    base * cond
}

/// Points for one ability (body scale).
pub fn ability_value(a: &Ability) -> f32 {
    // Mana abilities ({T}: Add ...): ramp, worth about a mana rock however often it's used.
    if matches!(a.when.first(), Some(When::Activated(c)) if c.tap && !c.sacrifice)
        && !a.effects.is_empty()
        && a.effects.iter().all(|e| matches!(e, Effect::AddMana(_)))
    {
        let n: f32 = a
            .effects
            .iter()
            .map(|e| match e {
                Effect::AddMana(n) => n.value(),
                _ => 0.0,
            })
            .sum();
        return 1.5 + 0.8 * (n - 1.0).max(0.0);
    }
    // Bringing back only itself ("return this card from your graveyard") is resilience,
    // not card advantage.
    let only_itself = a.text.contains("this card from your graveyard")
        || a.text.contains("~ from your graveyard");
    // "Each player draws / creates / gains": opponents get the same, so only a small edge
    // is left (Dragon Mage, Arc of Fortune, Howling Mine-style effects).
    let symmetric = a.text.contains("each player");
    let vals: Vec<f32> = a
        .effects
        .iter()
        .map(|e| match e {
            Effect::Regrowth | Effect::Reanimate | Effect::Recast if only_itself => 0.5,
            Effect::Draw { .. }
            | Effect::Loot
            | Effect::Tokens { .. }
            | Effect::ItemTokens { .. }
            | Effect::GainLife(_)
            | Effect::AddMana(_)
            | Effect::Drain { .. }
            | Effect::Discard { .. }
            | Effect::Edict { .. }
                if symmetric =>
            {
                0.35 * effect_value(e)
            }
            _ => effect_value(e),
        })
        .collect();
    // A modal ability is worth its best mode, but a forced drawback mode still counts.
    let v = if a.modal {
        vals.iter().copied().fold(0.0, f32::max) + vals.iter().filter(|v| **v < -1.0).sum::<f32>()
    } else {
        vals.iter().sum()
    };
    v * times(a) * ABILITY_SCALE
}

/// Keywords' worth on a creature with this power and toughness (body scale).
pub fn keyword_value(kw: &str, p: f32, t: f32) -> f32 {
    let k = kw.split([' ', '—', '{']).next().unwrap_or(kw);
    match k {
        "flying" => 0.5 + 0.4 * p,
        "trample" => 0.15 * p,
        "menace" => 0.3 + 0.25 * p,
        "double" => 0.9 * p,
        "first" => 0.3 + 0.2 * p,
        "deathtouch" => (1.4 - 0.15 * p).max(0.6),
        "lifelink" => 0.3 * p,
        "vigilance" => 0.3 + 0.1 * p,
        "haste" => 0.3 + 0.2 * p,
        "reach" => 0.4,
        "hexproof" => 1.0,
        "shroud" => 0.6,
        "ward" => 0.7,
        "indestructible" => 1.5 + 0.1 * (p + t),
        "protection" => 0.6,
        "flash" => 0.5,
        "prowess" => 0.7,
        "fear" | "intimidate" | "skulk" => 0.3 + 0.3 * p,
        "toxic" => 0.6,
        "defender" => -0.8 * p,
        "flanking" => 0.3,
        _ => 0.0,
    }
}

/// What a fair creature costing `mv` should be worth (body scale): about a vanilla
/// creature one size bigger than its cost, since Commander creatures are expected to do
/// something (a 3-mana 3/3 with no abilities rates a little below fair).
pub fn fair_value(mv: u32) -> f32 {
    if mv == 0 { 1.0 } else { 1.7 * mv as f32 + 1.0 }
}

#[derive(Debug, Clone, Default)]
pub struct CreatureValue {
    pub body: f32,
    pub keywords: f32,
    pub abilities: f32,
    pub total: f32,
    pub fair: f32,
    /// total / fair: 1.0 = fair for its cost.
    pub efficiency: f32,
    /// The biggest contributors, for "why".
    pub parts: Vec<(String, f32)>,
}

/// "*" and "1+*" power/toughness (Tarmogoyf, Lhurgoyf): grows with the game; estimated
/// from the mana cost.
fn stat(v: Option<&str>, mv: u32) -> f32 {
    let v = v.unwrap_or("0");
    let star = (mv as f32 + 1.0).clamp(2.0, 5.0);
    let n = if let Some(base) = v.strip_suffix("+*") {
        base.parse::<f32>().unwrap_or(0.0) + star
    } else if v == "*" {
        star
    } else {
        v.parse::<f32>().unwrap_or(0.0)
    };
    n.max(0.0)
}

/// Rates a creature's body, keywords and abilities against its mana cost.
pub fn creature_value(card: &Card) -> CreatureValue {
    let p = stat(card.power.as_deref(), card.mana_value());
    let t = stat(card.toughness.as_deref(), card.mana_value());
    let parsed = card.parsed();
    let body = p + 0.7 * t;
    let mut parts = vec![(format!("{}/{} body", p, t), body)];
    let mut keywords = 0.0;
    for k in &parsed.keywords {
        let v = keyword_value(k, p, t);
        if v != 0.0 {
            keywords += v;
            parts.push((k.clone(), v));
        }
    }
    let mut abilities = 0.0;
    // The back face (an Adventure or prepared spell, a transformed side) is extra, but it
    // competes with the front for the same card, so it counts for half.
    let back = parsed.back.iter().flat_map(|b| b.abilities.iter());
    let all = parsed
        .abilities
        .iter()
        .map(|a| (a, 1.0))
        .chain(back.map(|a| (a, 0.5)));
    for (a, share) in all {
        let v = ability_value(a) * share;
        if v.abs() >= 0.05 {
            abilities += v;
            let label = a
                .effects
                .iter()
                .map(|e| e.describe())
                .collect::<Vec<_>>()
                .join(", ");
            parts.push((label, v));
        }
    }
    // X costs: count X as 2 when judging the cost.
    let x = card.mana_cost.contains("{X}");
    let mv = card.mana_value() + if x { 2 } else { 0 };
    let total = body + keywords + abilities;
    let fair = fair_value(mv);
    parts.sort_by(|a, b| b.1.abs().partial_cmp(&a.1.abs()).unwrap());
    CreatureValue {
        body,
        keywords,
        abilities,
        total,
        fair,
        efficiency: total / fair,
        parts,
    }
}

/// Score points for a creature's value for its cost (about -2 .. +4): mostly efficiency,
/// plus a little for sheer impact so a fair 6-drop isn't level with a fair 2-drop.
pub fn creature_points(v: &CreatureValue) -> f32 {
    (3.0 * (v.efficiency - 1.0)).clamp(-2.0, 3.0) + 0.08 * v.total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creature(cost: &str, pt: (&str, &str), text: &str) -> Card {
        Card {
            name: "Test".into(),
            mana_cost: cost.into(),
            cmc: crate::card::symbols_in(cost)
                .iter()
                .map(|s| s.parse::<f32>().unwrap_or(1.0))
                .sum(),
            type_line: "Creature — Test".into(),
            oracle_text: text.into(),
            power: Some(pt.0.into()),
            toughness: Some(pt.1.into()),
            ..Default::default()
        }
    }

    #[test]
    fn abilities_beat_vanilla_bodies() {
        let vanilla = creature_value(&creature("{2}{G}", ("3", "3"), ""));
        let flyer = creature_value(&creature("{2}{U}", ("2", "2"), "Flying"));
        let chupa = creature_value(&creature(
            "{2}{B}{B}",
            ("2", "2"),
            "When this creature enters, destroy target creature an opponent controls.",
        ));
        let visionary = creature_value(&creature(
            "{1}{G}",
            ("1", "1"),
            "When this creature enters, draw a card.",
        ));
        assert!(vanilla.efficiency < 1.0, "{:?}", vanilla);
        assert!(chupa.efficiency > 1.2, "{:?}", chupa);
        assert!(visionary.efficiency > 0.85, "{:?}", visionary);
        assert!(creature_points(&chupa) > creature_points(&flyer));
        assert!(creature_points(&flyer) > creature_points(&vanilla) - 0.5);
        let wall = creature_value(&creature("{1}{W}", ("0", "4"), "Defender"));
        assert!(wall.efficiency < 0.8);
    }
}
