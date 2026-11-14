//! How the game simulator handles a card: per ability (and keyword, and text the parser
//! skips) whether it is played exactly, approximated, or not at all, with a note - and the
//! card's overall status (its worst part). Used by `examples/sim_coverage.rs` and the
//! app's card details.

use super::compile::{self, Cond, Ev, Trig};
use super::{KEYWORDS_PLAYED, effect_handling};
use crate::card::Card;

/// How a part of a card is played, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Handling {
    Exact,
    Approx,
    None,
}

impl Handling {
    /// "exact", "approx", "none" (the sim_coverage TSV's words).
    pub fn code(self) -> &'static str {
        match self {
            Handling::Exact => "exact",
            Handling::Approx => "approx",
            Handling::None => "none",
        }
    }

    /// For people: "exact", "approximated", "not simulated".
    pub fn label(self) -> &'static str {
        match self {
            Handling::Exact => "exact",
            Handling::Approx => "approximated",
            Handling::None => "not simulated",
        }
    }

    fn from_effect(h: &str) -> Handling {
        match h {
            "exact" => Handling::Exact,
            "approximated" => Handling::Approx,
            _ => Handling::None,
        }
    }
}

/// One part of a card and how it is played.
#[derive(Debug, Clone)]
pub struct Part {
    /// "1", "2"... for abilities, "kw" for a keyword, "?" for skipped text.
    pub id: String,
    /// The trigger ("static", "enters"...), "keyword" or "unparsed".
    pub trigger: String,
    pub text: String,
    /// What the parser read it as.
    pub parsed_as: String,
    pub handling: Handling,
    pub notes: String,
}

/// How the simulator handles a card.
#[derive(Debug, Clone)]
pub struct CardCoverage {
    /// The worst handling of any part (exact for a card with nothing to play).
    pub status: Handling,
    pub parts: Vec<Part>,
}

impl CardCoverage {
    /// The notes of the parts that aren't exact, for a one-line summary.
    pub fn notes(&self) -> Vec<String> {
        self.parts
            .iter()
            .filter(|p| p.handling != Handling::Exact)
            .map(|p| {
                if p.notes.is_empty() {
                    format!("{}: {}", p.trigger, p.text)
                } else {
                    p.notes.clone()
                }
            })
            .collect()
    }
}

/// A trigger's name and how it is handled ("exact", "no effect").
pub fn trig_name(t: &Trig) -> (String, &'static str) {
    match t {
        Trig::Spell => ("spell resolves".into(), "exact"),
        Trig::Enters => ("enters".into(), "exact"),
        Trig::Dies => ("dies".into(), "exact"),
        Trig::Attacks => ("attacks".into(), "exact"),
        Trig::CombatDamage => ("deals combat damage to a player".into(), "exact"),
        Trig::Phase {
            phase,
            any_player,
            opponents_only,
        } => (
            format!(
                "beginning of {:?}{}",
                phase,
                if *opponents_only {
                    " (each opponent's)"
                } else if *any_player {
                    " (each player's)"
                } else {
                    ""
                }
            ),
            "exact",
        ),
        Trig::Event(e, _) => (
            format!("event: {:?}", e),
            if *e == Ev::Other {
                "no effect"
            } else {
                "exact"
            },
        ),
        Trig::Activated { .. } => ("activated ability".into(), "exact"),
        Trig::PowerUp(_) => ("power-up".into(), "exact"),
        Trig::Loyalty(_) => ("loyalty ability".into(), "exact"),
        Trig::Chapter(_) => ("Saga chapter".into(), "exact"),
        Trig::Reflexive => ("reflexive (\"when you do\")".into(), "exact"),
        Trig::Static => ("static".into(), "exact"),
    }
}

/// A condition's name and how it is handled.
pub fn cond_name(c: &Cond) -> (String, &'static str) {
    let name = format!("{:?}", c);
    let name = name.split('(').next().unwrap_or(&name).to_string();
    let how = if matches!(c, Cond::Chance(_)) {
        "approximated"
    } else {
        "exact"
    };
    (name, how)
}

/// Is keyword `k` (as the parser reads it) one the simulator plays?
pub fn keyword_base(k: &str) -> Option<&'static str> {
    KEYWORDS_PLAYED
        .iter()
        .find(|w| {
            k == **w
                || k.starts_with(&format!("{} ", w))
                || k.starts_with(&format!("{}—", w))
                || k.starts_with(&format!("{} {{", w))
        })
        .copied()
}

/// Text the simulator reads straight off an ability (`text.contains("...")` in its sources):
/// a static the parser leaves without effects can still be played this way.
pub fn sim_literals() -> &'static [String] {
    static LITS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    LITS.get_or_init(|| {
        let srcs = [
            include_str!("combat.rs"),
            include_str!("state.rs"),
            include_str!("mod.rs"),
            include_str!("ai.rs"),
            include_str!("resolve.rs"),
            include_str!("compile.rs"),
            include_str!("targets.rs"),
            include_str!("../restrict.rs"),
        ];
        let mut out = Vec::new();
        for s in srcs {
            for pat in [
                "contains(\"",
                "starts_with(\"",
                "split(\"",
                "find(\"",
                "strip_prefix(\"",
            ] {
                for part in s.split(pat).skip(1) {
                    if let Some(lit) = part.split('"').next() {
                        // Short words ("flying", "draw") would match anything.
                        if lit.len() >= 12 && !lit.contains('\\') {
                            out.push(lit.to_string());
                        }
                    }
                }
            }
        }
        out.sort();
        out.dedup();
        out
    })
}

/// How the simulator plays `card`, part by part.
pub fn card_coverage(card: &Card) -> CardCoverage {
    let lits = sim_literals();
    let sim = compile::compile(card);
    let parsed = card.parsed();
    let mut parts: Vec<Part> = Vec::new();
    let mut special = Vec::new();
    if sim.rules != 0 {
        special.push(format!("rules bits {:#x}", sim.rules));
    }
    if sim.attack_tax.is_some() {
        special.push("attack tax".into());
    }
    if sim.opp_tax.is_some() {
        special.push("opponent spell tax".into());
    }
    if sim.cost_reduction.is_some() {
        special.push("cost reduction".into());
    }
    if sim.attached_sets.is_some() {
        special.push("attached sets".into());
    }
    if sim.hexproof_from != 0 {
        special.push("hexproof from".into());
    }
    let back = sim.back.iter().flat_map(|b| b.abilities.iter());
    for (k, a) in sim.abilities.iter().chain(back).enumerate() {
        let (trig, _) = trig_name(&a.trig);
        let mut how = Handling::Exact;
        let mut notes: Vec<String> = Vec::new();
        if let Trig::Event(Ev::Other, _) = a.trig {
            how = Handling::None;
            notes.push("trigger event never fires".into());
        }
        if matches!(a.cond, Cond::Chance(_)) {
            how = how.max(Handling::Approx);
            notes.push("condition decided by chance".into());
        }
        for e in &a.effects {
            let (h, note) = effect_handling(e);
            let h = Handling::from_effect(h);
            how = how.max(h);
            if h != Handling::Exact {
                let d = format!("{:?}", e);
                let d = match e {
                    crate::abilities::Effect::Rule(l) => format!("Rule \"{}\"", l),
                    _ => d.split([' ', '(', '{']).next().unwrap_or(&d).to_string(),
                };
                notes.push(format!("{}: {}", d, note));
            }
        }
        let compiled = (a.text.contains("enters tapped")
            && (sim.enters_tapped || sim.enters_tapped_unless.is_some()))
            || (a.text.contains("can't block") && sim.kw & compile::CANT_BLOCK != 0)
            || (a.text.contains("can't be blocked") && sim.kw & compile::UNBLOCKABLE != 0)
            // (A spell face's additional cost: compiled into its cast cost.)
            || a.text.starts_with("as an additional cost to cast ~");
        if a.effects.is_empty() && compiled {
            notes.push("compiled into the card's static fields".into());
        } else if a.effects.is_empty() && a.text.starts_with("as ~ enters, choose") {
            how = Handling::Approx;
            notes.push("the choice is made where it's used (best type / colour)".into());
        } else if a.effects.is_empty() && matches!(a.trig, Trig::Static) {
            let hit: Vec<&String> = lits
                .iter()
                .filter(|l| a.text.contains(l.as_str()))
                .collect();
            if let Some(l) = hit.first() {
                how = Handling::Approx;
                notes.push(format!(
                    "no parsed effect; simulator reads the text (\"{}\")",
                    l
                ));
            } else if !special.is_empty() {
                how = Handling::Approx;
                notes.push(format!(
                    "no parsed effect; compiled into {}",
                    special.join(", ")
                ));
            } else {
                how = Handling::None;
                notes.push("no parsed effect and nothing in the simulator reads it".into());
            }
        } else if a.effects.is_empty() {
            how = Handling::None;
            notes.push("no parsed effect".into());
        }
        let parsed_as = a
            .effects
            .iter()
            .map(|e| e.describe())
            .collect::<Vec<_>>()
            .join("; ");
        parts.push(Part {
            id: format!("{}", k + 1),
            trigger: trig,
            text: a.text.clone(),
            parsed_as,
            handling: how,
            notes: notes.join(" | "),
        });
    }
    for kw in &parsed.keywords {
        let played = keyword_base(kw).is_some() || kw.contains("cycling");
        let how = if !played {
            Handling::None
        } else if kw.starts_with("sneak") {
            Handling::Approx
        } else {
            Handling::Exact
        };
        parts.push(Part {
            id: "kw".into(),
            trigger: "keyword".into(),
            text: kw.clone(),
            parsed_as: kw.clone(),
            handling: how,
            notes: match how {
                Handling::None => format!("keyword {} isn't played", kw),
                Handling::Approx => format!("keyword {} is simplified", kw),
                Handling::Exact => String::new(),
            },
        });
    }
    for u in &parsed.unparsed {
        parts.push(Part {
            id: "?".into(),
            trigger: "unparsed".into(),
            text: u.clone(),
            parsed_as: "(not understood)".into(),
            handling: Handling::None,
            notes: format!("the parser skips \"{}\"", u),
        });
    }
    let status = parts
        .iter()
        .map(|p| p.handling)
        .max()
        .unwrap_or(Handling::Exact);
    CardCoverage { status, parts }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::CardDb;

    #[test]
    fn coverage_classifies_cards() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
        let db = CardDb::load(&dir.join("cards.json")).unwrap();
        let vanilla = super::super::simvalues::vanilla(3);
        assert_eq!(card_coverage(&vanilla).status, Handling::Exact);
        let all: Vec<CardCoverage> = db.cards.iter().map(card_coverage).collect();
        assert!(all.iter().any(|c| c.status == Handling::Exact));
        // The status is the worst part, and every non-exact card says why.
        for c in &all {
            let worst = c.parts.iter().map(|p| p.handling).max();
            assert_eq!(c.status, worst.unwrap_or(Handling::Exact));
            if c.status != Handling::Exact {
                assert!(!c.notes().is_empty());
            }
        }
        assert!(!sim_literals().is_empty());
    }
}
