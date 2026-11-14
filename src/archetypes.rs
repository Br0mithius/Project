//! Deck archetypes the builder can recognise and build: the broad Commander archetypes (go-wide
//! aggro, voltron, midrange value, stax-lite and reactive control, ramp, tokens, counters,
//! aristocrats, spellslinger, artifacts, lifegain, landfall, card draw) and the league sets'
//! key traits as themes (Villains, Heroes, Adventure, power-up, teamwork, Plans, Infinity
//! Stones, sneak, storied, Treasure, and the main tribes: Dwarves, Elves, Goblins, Wolves,
//! Birds/fliers, Dinosaurs).
//!
//! Each archetype names the themes it combines; a card fits when it pays one off or feeds it,
//! read with the same rules-text reading the builder scores commanders with
//! (`analysis::MECHANICS`, creature types through `analysis::cares_about_type`, card roles),
//! plus a few keyword/phrase and custom readings. `data/cards.json` is the source of truth for
//! what exists: some set traits have very few cards (one Infinity Stone, one sneak card), so
//! they are catalogued for detection but marked `sparse` (not buildable decks).
//!
//! `build` makes a deck for an archetype following its `Targets` (package of fitting cards,
//! minimum counts of removal/draw/ramp/creatures/counterspells, land count, top-end cap);
//! the targets in `TARGETS` / `DEFAULT_TARGETS` are the ones paired playtests supported.

use crate::analysis;
use crate::card::{Card, ColorSet, Role};
use crate::db::{CardDb, Collection};
use crate::deck::Deck;

/// One thing an archetype is built on.
#[derive(Debug, Clone, Copy)]
pub enum Theme {
    /// A mechanic the builder reads, by its `need` in `analysis::MECHANICS` (e.g.
    /// "Equipment"): its payoff and enabler readings.
    Mechanic(&'static str),
    /// A creature type: creatures of the type (and cards that make them) enable it, cards
    /// that care about it ("other Elves you control") pay it off.
    Tribe(&'static str),
    /// A keyword or phrase: cards with one of `enablers` (a keyword, or a whole word in the
    /// rules text) enable it, cards whose text has one of `payoffs` pay it off.
    Words {
        label: &'static str,
        enablers: &'static [&'static str],
        payoffs: &'static [&'static str],
    },
    /// Cards with a role (`card::Role`) enable it; with `core`, they're the archetype's
    /// point and count as payoffs too (sweepers in control).
    Role {
        label: &'static str,
        role: Role,
        core: bool,
    },
    /// A custom reading: (payoff, enabler).
    Custom {
        label: &'static str,
        read: fn(&Card) -> (f32, f32),
    },
}

impl PartialEq for Theme {
    fn eq(&self, other: &Theme) -> bool {
        self.label() == other.label()
    }
}

impl Theme {
    pub fn label(&self) -> String {
        match self {
            Theme::Mechanic(need) => need.to_string(),
            Theme::Tribe(t) => format!("{} (creature type)", plural(t)),
            Theme::Words { label, .. }
            | Theme::Role { label, .. }
            | Theme::Custom { label, .. } => label.to_string(),
        }
    }

    /// (payoff, enabler), each 0..1.
    pub fn read(&self, card: &Card) -> (f32, f32) {
        let t = card.text_lower();
        let yes = |b: bool| if b { 1.0 } else { 0.0 };
        match self {
            Theme::Mechanic(need) => (
                analysis::mechanic_deck_payoff(need, card).unwrap_or(0.0),
                analysis::mechanic_feed(need, card).unwrap_or(0.0),
            ),
            Theme::Tribe(tribe) => {
                let low = tribe.to_lowercase();
                let member = card.is_creature()
                    && (card.parsed().has_keyword("changeling")
                        || card
                            .subtypes()
                            .iter()
                            .any(|s| s.eq_ignore_ascii_case(tribe)));
                // "Create a 1/1 green Elf creature token", "amass Goblins 2".
                let maker = t.contains(&format!("{} creature token", low))
                    || t.contains(&format!("amass {}", plural(&low)));
                let payoff = analysis::cares_about_type(t, &low);
                (
                    yes(payoff),
                    if member {
                        1.0
                    } else if maker {
                        0.8
                    } else {
                        0.0
                    },
                )
            }
            Theme::Words {
                enablers, payoffs, ..
            } => {
                let p = card.parsed();
                let enabler = enablers.iter().any(|w| p.has_keyword(w) || has_word(t, w));
                (yes(payoffs.iter().any(|w| t.contains(w))), yes(enabler))
            }
            Theme::Role { role, core, .. } => {
                let has = card.has_role(*role);
                (yes(has && *core), yes(has))
            }
            Theme::Custom { read, .. } => read(card),
        }
    }
}

/// What kind of archetype it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// A broad Commander deck plan, any colours.
    Commander,
    /// A league set's key trait (tribe or mechanic).
    SetTrait,
}

/// A deck archetype.
#[derive(Debug)]
pub struct SetArchetype {
    pub name: &'static str,
    pub kind: Kind,
    /// Set code the trait comes from ("MSH", "HOB", "FRA", "FDN"), "" for Commander ones.
    pub set: &'static str,
    /// Colours it needs, WUBRG letters ("" = any: the commander's).
    pub colors: &'static str,
    /// The plan in a sentence.
    pub plan: &'static str,
    pub themes: &'static [Theme],
    /// League legendary creatures that lead it (the first is the reference build).
    pub commanders: &'static [&'static str],
    /// Signpost and payoff cards (all in `data/cards.json`).
    pub key_cards: &'static [&'static str],
    /// Too few cards in the league sets to build a deck on (detection only).
    pub sparse: bool,
}

impl SetArchetype {
    pub fn color_set(&self) -> ColorSet {
        ColorSet::parse(self.colors).unwrap_or_default()
    }

    /// How well `card` fits (lands never do).
    pub fn fit(&self, card: &Card) -> Fit {
        let mut fit = Fit {
            key: self.key_cards.contains(&card.name.as_str()),
            ..Default::default()
        };
        if card.is_land() {
            return fit;
        }
        for th in self.themes {
            let (p, e) = th.read(card);
            if p > 0.0 || e > 0.0 {
                fit.themes.push(th.label());
            }
            fit.payoff = fit.payoff.max(p);
            fit.enabler = fit.enabler.max(e);
        }
        fit
    }

    /// Is `card` playable in this archetype's colours (any card when it has none)?
    pub fn in_colors(&self, card: &Card) -> bool {
        self.colors.is_empty() || card.identity().is_subset_of(self.color_set())
    }
}

/// How a card fits an archetype.
#[derive(Debug, Clone, Default)]
pub struct Fit {
    /// Rewards one of the archetype's themes (0..1).
    pub payoff: f32,
    /// Feeds one of them (0..1).
    pub enabler: f32,
    /// One of the archetype's named key cards.
    pub key: bool,
    /// The themes it touches.
    pub themes: Vec<String>,
}

impl Fit {
    /// 0..~1.25: the better of payoff and enabler, a bit more for both, at least 1 for a key
    /// card.
    pub fn score(&self) -> f32 {
        let s = self.payoff.max(self.enabler) + 0.25 * self.payoff.min(self.enabler);
        if self.key { s.max(1.0) } else { s }
    }

    pub fn fits(&self) -> bool {
        self.score() >= 0.5
    }

    pub fn is_payoff(&self) -> bool {
        self.payoff >= 0.75
    }
}

/// Enablers and payoffs a 60-card deck (about 34 spells) wants for an archetype to work.
pub const ENABLERS_WANTED: f32 = 12.0;
pub const PAYOFFS_WANTED: f32 = 5.0;

/// What a pool or deck has for one archetype.
#[derive(Debug)]
pub struct Report<'a> {
    pub arch: &'static SetArchetype,
    /// Fitting cards, best first.
    pub cards: Vec<(&'a Card, Fit)>,
    /// Enablers (each counted once, weighted by how well it feeds).
    pub enablers: f32,
    /// Clear payoffs.
    pub payoffs: f32,
    /// Commanders available (owned, or leading the deck).
    pub commanders: Vec<&'static str>,
    /// 0..1: how close to a working deck (half enablers, 30% payoffs, 20% a commander).
    pub closeness: f32,
}

fn report<'a>(
    arch: &'static SetArchetype,
    cards: impl Iterator<Item = &'a Card>,
    identity: Option<ColorSet>,
    commanders: Vec<&'static str>,
) -> Report<'a> {
    let mut fitting: Vec<(&Card, Fit)> = cards
        .filter(|c| !c.is_land() && arch.in_colors(c))
        .filter(|c| identity.is_none_or(|id| c.identity().is_subset_of(id)))
        .map(|c| (c, arch.fit(c)))
        .filter(|(_, f)| f.fits())
        .collect();
    fitting.sort_by(|a, b| {
        b.1.score()
            .partial_cmp(&a.1.score())
            .unwrap()
            .then_with(|| a.0.name.cmp(&b.0.name))
    });
    let enablers: f32 = fitting.iter().map(|(_, f)| f.enabler.min(1.0)).sum::<f32>() + 0.0;
    let payoffs = fitting.iter().filter(|(_, f)| f.is_payoff()).count() as f32;
    let closeness = 0.5 * (enablers / ENABLERS_WANTED).min(1.0)
        + 0.3 * (payoffs / PAYOFFS_WANTED).min(1.0)
        + if commanders.is_empty() { 0.0 } else { 0.2 };
    Report {
        arch,
        cards: fitting,
        enablers,
        payoffs,
        commanders,
        closeness,
    }
}

/// What the pool `coll` has for `arch` (each owned card counted once: singleton decks),
/// within the colours of its best owned commander (any colours when none is owned).
pub fn pool_report<'a>(
    arch: &'static SetArchetype,
    db: &'a CardDb,
    coll: &Collection,
) -> Report<'a> {
    let owned = || db.cards.iter().filter(|c| coll.owned(&c.name) > 0);
    let commanders: Vec<&'static str> = arch
        .commanders
        .iter()
        .copied()
        .filter(|n| coll.owned(n) > 0)
        .collect();
    let best = commanders
        .iter()
        .filter_map(|n| db.get(n))
        .map(|c| {
            report(
                arch,
                owned().filter(|o| o.name != c.name),
                Some(c.identity()),
                vec![
                    arch.commanders
                        .iter()
                        .find(|n| **n == c.name)
                        .copied()
                        .unwrap(),
                ],
            )
        })
        .max_by(|a, b| a.closeness.partial_cmp(&b.closeness).unwrap());
    match best {
        Some(mut r) => {
            r.commanders = commanders;
            r
        }
        None => report(arch, owned(), None, Vec::new()),
    }
}

/// How `deck` plays `arch`, or None when the archetype's colours aren't all in the deck.
pub fn deck_report<'a>(
    arch: &'static SetArchetype,
    deck: &Deck,
    db: &'a CardDb,
) -> Option<Report<'a>> {
    let identity = deck.identity(db)?;
    if !arch.color_set().is_subset_of(identity) {
        return None;
    }
    let cmds: Vec<&Card> = deck.commanders().iter().filter_map(|n| db.get(n)).collect();
    // The commander leads it when it's one of the archetype's, or rewards one of its themes.
    let leads = cmds.iter().any(|c| {
        arch.commanders.contains(&c.name.as_str())
            || arch.themes.iter().any(|t| t.read(c).0 >= 0.75)
    });
    let commanders = if leads {
        vec!["(the deck's commander)"]
    } else {
        Vec::new()
    };
    let cards = deck
        .cards
        .keys()
        .filter_map(|n| db.get(n))
        .filter(|c| !cmds.iter().any(|m| m.name == c.name));
    Some(report(arch, cards, None, commanders))
}

/// The archetype `deck` plays best (highest closeness, then most fitting cards), if any.
pub fn best_for_deck<'a>(deck: &Deck, db: &'a CardDb) -> Option<Report<'a>> {
    ARCHETYPES
        .iter()
        .filter_map(|a| deck_report(a, deck, db))
        .max_by(|a, b| {
            a.closeness
                .partial_cmp(&b.closeness)
                .unwrap()
                .then_with(|| a.cards.len().cmp(&b.cards.len()))
        })
}

/// The archetype a commander leads: the first one listing it.
pub fn for_commander(name: &str) -> Option<&'static SetArchetype> {
    ARCHETYPES
        .iter()
        .find(|a| !a.sparse && a.commanders.first() == Some(&name))
        .or_else(|| {
            ARCHETYPES
                .iter()
                .find(|a| !a.sparse && a.commanders.contains(&name))
        })
}

/// Archetypes whose name or set code matches `query` (case-insensitive substring), or
/// "commander" / "traits" for one kind.
pub fn find(query: &str) -> Vec<&'static SetArchetype> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return ARCHETYPES.iter().collect();
    }
    ARCHETYPES
        .iter()
        .filter(|a| {
            a.name.to_lowercase().contains(&q)
                || a.set.to_lowercase() == q
                || (q == "commander" && a.kind == Kind::Commander)
                || (q == "traits" && a.kind == Kind::SetTrait)
        })
        .collect()
}

/// English plural of a creature type ("Elf" -> "Elves", "Wolf" -> "Wolves", "Hero" ->
/// "Heroes").
pub fn plural(word: &str) -> String {
    let lower = word.to_lowercase();
    let keep_case = |s: &str| -> String {
        if word.chars().next().is_some_and(|c| c.is_uppercase()) {
            let mut c = s.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        } else {
            s.to_string()
        }
    };
    let p = if lower == "dwarf" || lower == "elf" || lower == "wolf" {
        format!("{}ves", &lower[..lower.len() - 1])
    } else if lower.ends_with('o') || lower.ends_with('s') || lower.ends_with('x') {
        format!("{}es", lower)
    } else if lower.ends_with('y') && !lower.ends_with("ey") {
        format!("{}ies", &lower[..lower.len() - 1])
    } else {
        format!("{}s", lower)
    };
    keep_case(&p)
}

/// `w` as a whole word in `t` ("flash" isn't in "flashback").
fn has_word(t: &str, w: &str) -> bool {
    t.match_indices(w).any(|(i, _)| {
        let before = t[..i].chars().next_back();
        let after = t[i + w.len()..].chars().next();
        before.is_none_or(|c| !c.is_alphanumeric()) && after.is_none_or(|c| !c.is_alphanumeric())
    })
}

// ---------- custom readings ----------

fn yes(b: bool) -> f32 {
    if b { 1.0 } else { 0.0 }
}

/// Cheap attackers for aggro: creatures of mana value 3 or less with 2+ power, evasion or
/// haste.
fn cheap_attacker(c: &Card) -> (f32, f32) {
    let power: i32 = c.power.as_deref().and_then(|v| v.parse().ok()).unwrap_or(0);
    let p = c.parsed();
    let ok = c.is_creature()
        && c.mana_value() <= 3
        && (power >= 2 || p.evasive() || p.has_keyword("haste"));
    (0.0, yes(ok))
}

/// Voltron: Auras that pump the enchanted creature enable, and cards that reward an
/// equipped/enchanted creature pay off.
fn voltron(c: &Card) -> (f32, f32) {
    let t = c.text_lower();
    let aura = c.type_line.contains("Aura") && t.contains("enchanted creature gets +");
    let payoff = t.contains("for each equipment attached")
        || t.contains("for each aura attached")
        || t.contains("equipped or enchanted")
        || t.contains("whenever an equipment you control becomes attached");
    (yes(payoff), yes(aura))
}

/// Stax-lite: taxes and lock pieces (Propaganda, Monologue Tax, "can't cast spells during
/// your turn"); they are the deck's point, so they count as payoffs too.
fn tax(c: &Card) -> (f32, f32) {
    let t = c.text_lower();
    let lock = t.contains("can't attack you")
        || t.contains("costs {1} more")
        || t.contains("cost {1} more")
        || t.contains("more to cast")
        || t.contains("can't cast spells during your turn")
        || (t.contains("whenever an opponent casts their second spell"));
    (yes(lock), yes(lock))
}

/// Reactive control: answers at instant speed (instants or flash) - removal, counterspells,
/// bounce.
fn instant_answer(c: &Card) -> (f32, f32) {
    let fast = c.type_line.contains("Instant") || c.parsed().has_keyword("flash");
    let answer = c.has_role(Role::Removal) || c.has_role(Role::Counter) || c.has_role(Role::Bounce);
    (0.0, yes(fast && answer))
}

/// Ramp into big threats: big creatures (6+ mana, or power 6+) are the payoff.
fn big_threat(c: &Card) -> (f32, f32) {
    let power: i32 = c.power.as_deref().and_then(|v| v.parse().ok()).unwrap_or(0);
    (
        yes(c.is_creature() && (c.mana_value() >= 6 || power >= 6)),
        0.0,
    )
}

/// Adventure cards (HOB) and their successor, prepared creatures (FRA): two spells in one
/// card. Casting from exile / "other than your hand" (Bilbo, Thief in the Night) and copying
/// prepared spells (Codie) pay off.
fn adventure(c: &Card) -> (f32, f32) {
    let t = c.text_lower();
    let is = c.oracle_text.contains("— Adventure")
        || t.contains("enters prepared")
        || t.contains("becomes prepared");
    let payoff = t.contains("other than your hand")
        || t.contains("adventure spell")
        || t.contains("cast a prepared spell");
    (yes(payoff), yes(is))
}

/// Infinity Stones (MSH): artifacts with a harness / ∞ ability.
fn infinity(c: &Card) -> (f32, f32) {
    let t = c.text_lower();
    let stone = t.contains("harness") || c.oracle_text.contains('∞');
    (yes(t.contains("harnessed") && !stone), yes(stone))
}

/// Deck-building targets for an archetype deck (60 cards; scaled for 75). `None`/0 leaves
/// a part to the builder's own plan (`analysis::deck_plan`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Targets {
    /// Lands (None: `analysis::recommended_lands`).
    pub lands: Option<u32>,
    /// The archetype's best fitting cards put in first (by fit and builder score).
    pub package: u32,
    /// Minimum counts, filled with the best-scoring cards of that kind before the rest.
    pub removal: u32,
    pub draw: u32,
    pub ramp: u32,
    pub creatures: u32,
    pub counters: u32,
    /// Most cards of mana value 6+ (None: no cap).
    pub topend: Option<u32>,
}

impl Targets {
    /// The builder as it is: no package, no quotas.
    pub const PLAIN: Targets = Targets {
        lands: None,
        package: 0,
        removal: 0,
        draw: 0,
        ramp: 0,
        creatures: 0,
        counters: 0,
        topend: None,
    };
}

/// The general rules every archetype deck starts from. Paired sweeps (25 archetypes, one
/// commander each, built from every league card, 4000 games per variant against the
/// simulated league) measured, against a 16-card archetype package on the builder's own
/// plan (26 lands, no minimums):
/// - lands: 24 -2.1 points, 25 -1.0, 27 +0.7, 28 +1.4 (better in 16 archetypes, worse in
///   3): these decks average 4.2-5.3 mana value, so 27 is the safe general value;
/// - at least 24 creatures: +3.2 (better in 11, never worse): creature-light builds lose;
/// - at least 11 card draw: +0.9 (better in 11, worse in 1); 8 draw: no change;
/// - the archetype package: none -3.2 (worse in 13, better in 6), 24 cards +1.1 (mixed);
/// - minimum removal (6/9: -0.6/-0.5), counterspells (4: -1.7) and a cap on 6+ drops (2:
///   -3.6, worse in 16) hurt: the builder's own removal pick and the big threats are better.
/// - minimum ramp (7/10: +0.3/+0.2) is mixed: per archetype only.
pub const DEFAULT_TARGETS: Targets = rules(27, 16, 24, 11, 0, 0, 0);

const fn rules(
    lands: u32,
    package: u32,
    creatures: u32,
    draw: u32,
    ramp: u32,
    removal: u32,
    counters: u32,
) -> Targets {
    Targets {
        lands: Some(lands),
        package,
        removal,
        draw,
        ramp,
        creatures,
        counters,
        topend: None,
    }
}

/// Per-archetype targets: the general rules with the sweep's significant per-archetype
/// differences (more than 2 standard errors and 1 point better than the general value).
/// `autobuild` follows only archetypes listed here. Each was retested against the old
/// auto-build (3 seeds x 12000 games against the simulated league, the same with focus 2.5,
/// and against a fixed real pod), and kept only when it helped in the league and with focus
/// and didn't lose significantly against the pod. Dropped, so they keep the plain
/// auto-build: Reactive control, Aggro / go-wide, Ramp into big threats (no gain), Tokens,
/// +1/+1 counters, Elves (worse), Treasure, Landfall, Teamwork, Dwarves (better in the
/// league, worse against the pod).
pub const TARGETS: &[(&str, Targets)] = &[
    ("Aristocrats", rules(27, 24, 24, 11, 0, 0, 0)),
    ("Artifacts", rules(27, 0, 24, 11, 0, 0, 0)),
    ("Card draw / second card", rules(27, 24, 24, 11, 10, 0, 4)),
    ("Birds and fliers", rules(26, 24, 24, 11, 0, 0, 0)),
    ("Adventure", rules(26, 16, 24, 11, 0, 0, 0)),
    ("Goblins", rules(26, 16, 24, 11, 0, 0, 0)),
    ("Storied", rules(27, 24, 24, 11, 10, 0, 0)),
    ("Wolves", rules(27, 24, 24, 11, 10, 0, 0)),
    ("Lifegain", rules(27, 16, 24, 11, 10, 0, 0)),
    ("Midrange value", rules(27, 16, 24, 11, 10, 0, 0)),
    ("Heroes", rules(27, 24, 24, 11, 10, 0, 0)),
    ("Power-up", rules(28, 16, 24, 11, 0, 0, 0)),
    ("Spellslinger", rules(27, 16, 24, 11, 0, 0, 0)),
    // Package 16, not 24: a 24-card package filled the deck with on-theme but weak cards
    // (Rapid Rescue, HULK SMASH!) and left out Captain Marvel; the sweep that chose 24
    // compared against an illegal 121-card build.
    ("Stax-lite control", rules(27, 16, 18, 11, 10, 0, 0)),
    ("Voltron", rules(27, 16, 24, 11, 0, 0, 0)),
    ("Dinosaurs", DEFAULT_TARGETS),
    ("Plans", DEFAULT_TARGETS),
    ("Villains", DEFAULT_TARGETS),
];

/// The targets to build `arch` with.
pub fn targets(arch: &SetArchetype) -> Targets {
    TARGETS
        .iter()
        .find(|(n, _)| *n == arch.name)
        .map(|(_, t)| *t)
        .unwrap_or(DEFAULT_TARGETS)
}

/// Builds a singleton/normal deck for `cmd` from `coll` the way `analysis::autobuild` does,
/// following `t`: the archetype package first, then the minimum counts, then the best
/// remaining cards (no more 6+ drops than `t.topend`), on `t.lands` lands.
/// The builder score a package card needs (about a fair creature of its cost).
pub const PACKAGE_FLOOR: f32 = 6.0;

pub fn build(
    arch: &SetArchetype,
    cmd: &Card,
    db: &CardDb,
    coll: &Collection,
    rules: &crate::db::Rules,
    t: &Targets,
) -> (Deck, u32) {
    let scale = rules.deck_size as f32 / 60.0;
    let n = |x: u32| (x as f32 * scale).round() as u32;
    let mut start = Deck::new(&format!("{} ({})", arch.name, cmd.name));
    start.commander = Some(cmd.name.clone());
    let identity = cmd.identity();
    let plays = ColorSet(
        analysis::best_colors(cmd, db, coll, rules)
            .union(arch.color_set())
            .0
            & identity.0,
    );
    // Nonbasic lands that make our colours, as auto-build does.
    let (ability_colors, needs_colorless) = cmd.ability_colors();
    let wanted = plays.union(ColorSet(ability_colors.0 & identity.0));
    for land in db.cards.iter().filter(|c| c.is_land() && !c.is_basic()) {
        let ok = !rules.enforce_color_identity || land.identity().is_subset_of(identity);
        let useful = land.produces().0 & wanted.0 != 0 || needs_colorless && land.makes_colorless();
        if ok && useful {
            let k = analysis::available(land, &start, coll, rules);
            if k > 0 && k < 99 {
                start.add(&land.name, k);
            }
        }
    }
    analysis::trim_lands(
        &mut start,
        db,
        wanted,
        t.lands.map(n).unwrap_or(analysis::land_range(rules).0),
    );
    let main = start.main_target(rules);
    let topend = |deck: &Deck| -> u32 {
        analysis::resolved(deck, db)
            .iter()
            .filter(|(c, _)| !c.is_land() && c.mana_value() >= 6)
            .map(|(_, k)| *k)
            .sum()
    };
    let fill = |lands: u32| -> (Deck, u32) {
        let mut deck = start.clone();
        let slots = main.saturating_sub(lands);
        let spells = |d: &Deck| d.main_count() - analysis::land_count(d, db);
        // The package.
        if t.package > 0 {
            let mut cands: Vec<(&Card, f32)> = db
                .cards
                .iter()
                .filter(|c| !c.is_land() && c.name != cmd.name && arch.in_colors(c))
                .filter(|c| c.identity().is_subset_of(identity) && c.playable_with(plays))
                .filter(|c| analysis::available(c, &deck, coll, rules) > 0)
                .filter_map(|c| {
                    let f = arch.fit(c);
                    if !f.fits() {
                        return None;
                    }
                    // On theme isn't enough: only cards about as good as a fair creature of
                    // their cost go in first (weak fits wait for the general fill).
                    let s = analysis::score_card(c, &deck, db, rules).0;
                    (s >= PACKAGE_FLOOR).then(|| (c, s + 1.5 * f.score()))
                })
                .collect();
            cands.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
            for (c, _) in cands.iter().take(n(t.package) as usize) {
                if spells(&deck) < slots {
                    deck.add(&c.name, 1);
                }
            }
        }
        let cap = t.topend.map(n);
        let pick = |deck: &Deck, want: &dyn Fn(&Card) -> bool| -> Option<String> {
            let top = topend(deck);
            analysis::suggestions_within(deck, db, coll, rules, Some(plays))
                .into_iter()
                .find(|s| {
                    want(s.card) && !(cap.is_some_and(|m| top >= m) && s.card.mana_value() >= 6)
                })
                .map(|s| s.card.name.clone())
        };
        // The minimum counts.
        let count = |deck: &Deck, f: &dyn Fn(&Card) -> bool| -> u32 {
            analysis::resolved(deck, db)
                .iter()
                .filter(|(c, _)| !c.is_land() && f(c))
                .map(|(_, k)| *k)
                .sum()
        };
        let quotas: [(u32, &dyn Fn(&Card) -> bool); 5] = [
            (t.removal, &|c: &Card| c.has_role(Role::Removal)),
            (t.draw, &|c: &Card| c.has_role(Role::Draw)),
            (t.ramp, &|c: &Card| c.has_role(Role::Ramp)),
            (t.counters, &|c: &Card| c.has_role(Role::Counter)),
            (t.creatures, &|c: &Card| c.is_creature()),
        ];
        for (min, f) in quotas {
            while count(&deck, f) < n(min) && spells(&deck) < slots {
                match pick(&deck, f) {
                    Some(name) => deck.add(&name, 1),
                    None => break,
                }
            }
        }
        while spells(&deck) < slots {
            match pick(&deck, &|_| true) {
                Some(name) => deck.add(&name, 1),
                None => break,
            }
        }
        let short = slots.saturating_sub(spells(&deck));
        (deck, short)
    };
    let deck = match t.lands {
        Some(l) => fill(n(l)),
        None => {
            // As auto-build: re-check the land count against the curve.
            let mut lands = analysis::recommended_lands(3.0, rules);
            let mut deck = fill(lands);
            for _ in 0..2 {
                let new = analysis::recommended_lands(analysis::stats(&deck.0, db).avg_mv, rules);
                if new == lands {
                    break;
                }
                lands = new;
                deck = fill(lands);
            }
            deck
        }
    };
    let (mut deck, short) = deck;
    analysis::fill_basics(&mut deck, db, rules);
    (deck, short)
}

/// Auto-build that follows the commander's archetype: when `cmd` leads an archetype with
/// playtest-backed targets (`TARGETS`) and no colours or Prismatic Piper are forced, the
/// deck is built with `build` and those targets; otherwise it's `analysis::autobuild`.
/// Returns the deck, the spell slots the pool couldn't fill, and the archetype followed.
pub fn autobuild(
    cmd: &Card,
    db: &CardDb,
    coll: &Collection,
    rules: &crate::db::Rules,
    colors: Option<ColorSet>,
    piper: Option<char>,
) -> (Deck, u32, Option<&'static SetArchetype>) {
    if colors.is_none() && piper.is_none() {
        if let Some(arch) = for_commander(&cmd.name) {
            if let Some((_, t)) = TARGETS.iter().find(|(n, _)| *n == arch.name) {
                let (mut deck, short) = build(arch, cmd, db, coll, rules, t);
                deck.name = format!("auto-{}", cmd.name);
                return (deck, short, Some(arch));
            }
        }
    }
    let (deck, short) = analysis::autobuild(cmd, db, coll, rules, colors, piper);
    (deck, short, None)
}

use Theme::{Mechanic as M, Tribe as T};

const fn commander(
    name: &'static str,
    plan: &'static str,
    themes: &'static [Theme],
    commanders: &'static [&'static str],
    key_cards: &'static [&'static str],
) -> SetArchetype {
    SetArchetype {
        name,
        kind: Kind::Commander,
        set: "",
        colors: "",
        plan,
        themes,
        commanders,
        key_cards,
        sparse: false,
    }
}

const fn tr(
    name: &'static str,
    set: &'static str,
    plan: &'static str,
    themes: &'static [Theme],
    commanders: &'static [&'static str],
    key_cards: &'static [&'static str],
) -> SetArchetype {
    SetArchetype {
        name,
        kind: Kind::SetTrait,
        set,
        colors: "",
        plan,
        themes,
        commanders,
        key_cards,
        sparse: false,
    }
}

pub const ARCHETYPES: &[SetArchetype] = &[
    // ---------------- Commander archetypes ----------------
    commander(
        "Aggro / go-wide",
        "Cheap attackers and creature tokens, anthems and haste: win before the table stabilises.",
        &[
            Theme::Custom {
                label: "cheap attackers",
                read: cheap_attacker,
            },
            M("creature tokens for anthems"),
            M("evasive/haste creatures"),
        ],
        &[
            "Aurelia, the Warleader",
            "Black Panther, Vanguard",
            "Madame Hydra",
        ],
        &["Heroic Reinforcements", "Avengers Assemble!"],
    ),
    commander(
        "Voltron",
        "Suit up one creature (often the commander) with Equipment and Auras and protect it.",
        &[
            M("Equipment"),
            Theme::Custom {
                label: "Auras and voltron payoffs",
                read: voltron,
            },
            Theme::Role {
                label: "protection",
                role: Role::Protection,
                core: false,
            },
        ],
        &[
            "Winter Soldier, Icy Assassin",
            "The Mighty Thor, Jane Foster",
            "Dwalin, Weaponmaster",
        ],
        &[
            "Captain America's Shield",
            "Iron Man Armor",
            "Super-Soldier Serum",
        ],
    ),
    commander(
        "Midrange value",
        "Creatures with enters-the-battlefield value, removal and card draw: trade up over a long game.",
        &[
            M("enters-effect creatures"),
            Theme::Role {
                label: "removal",
                role: Role::Removal,
                core: false,
            },
            Theme::Role {
                label: "card draw",
                role: Role::Draw,
                core: false,
            },
        ],
        &[
            "Killmonger, Scourge of Wakanda",
            "Black Widow, Double Agent",
        ],
        &[],
    ),
    commander(
        "Stax-lite control",
        "Taxes (Propaganda, Monologue Tax), board wipes and removal keep the table down while a few threats win.",
        &[
            Theme::Custom {
                label: "taxes and locks",
                read: tax,
            },
            Theme::Role {
                label: "board wipes",
                role: Role::Sweeper,
                core: true,
            },
            Theme::Role {
                label: "removal",
                role: Role::Removal,
                core: false,
            },
        ],
        &[
            "Thanos, the Mad Titan",
            "Jennifer Walters",
            "Storm, Windrider",
        ],
        &["Propaganda", "Monologue Tax", "Fumigate", "Austere Command"],
    ),
    commander(
        "Reactive control",
        "Counterspells, instant-speed answers and card draw; win late with card advantage.",
        &[
            Theme::Role {
                label: "counterspells",
                role: Role::Counter,
                core: true,
            },
            Theme::Custom {
                label: "instant-speed answers",
                read: instant_answer,
            },
            Theme::Role {
                label: "card draw",
                role: Role::Draw,
                core: false,
            },
        ],
        &["King T'Challa", "Leader, Super-Genius"],
        &["Counterspell"],
    ),
    commander(
        "Ramp into big threats",
        "Mana rocks, land ramp and dorks into 6+ mana threats.",
        &[
            Theme::Role {
                label: "ramp",
                role: Role::Ramp,
                core: false,
            },
            Theme::Custom {
                label: "big threats",
                read: big_threat,
            },
            M("big creatures"),
        ],
        &["Ruby, Daring Tracker", "Koma, World-Eater"],
        &["Sol Ring", "Cultivate", "Arcane Signet"],
    ),
    commander(
        "Tokens",
        "Make many tokens and reward them (doublers, anthems, token-count payoffs).",
        &[M("token makers"), M("creature tokens for anthems")],
        &["Bard, King of Dale", "Krenko, Mob Boss"],
        &[],
    ),
    commander(
        "+1/+1 counters",
        "Put +1/+1 counters everywhere; payoffs trigger on each counter.",
        &[M("+1/+1 counter cards")],
        &[
            "Ant-Man, Colony Commander",
            "The Astonishing Ant-Man",
            "Mabel, Valley Hero",
        ],
        &["Training Regimen", "Good-Fortune Unicorn"],
    ),
    commander(
        "Aristocrats",
        "Sacrifice outlets, fodder and death triggers drain the table.",
        &[M("sacrifice outlets / fodder")],
        &[
            "Garna, Bloodfist of Keld",
            "Edgar, Ancient Bloodlord",
            "Tom, Bert, and William",
        ],
        &[],
    ),
    commander(
        "Spellslinger",
        "Instants, sorceries and other noncreature spells trigger prowess and cast payoffs.",
        &[M("instants and sorceries"), M("other noncreature spells")],
        &[
            "Balmor, Battlemage Captain",
            "Vraska, Soul of Stone",
            "Niv-Mizzet, Visionary",
            "Thor Odinson",
        ],
        &["Teach by Example"],
    ),
    commander(
        "Artifacts",
        "Artifacts matter: payoffs grow and draw with each artifact.",
        &[M("artifacts")],
        &["Iron Man, Master of Machines", "Tony Stark"],
        &["Armor Wars"],
    ),
    commander(
        "Lifegain",
        "Gain life every turn; payoffs draw, grow and drain.",
        &[M("life gain cards")],
        &[
            "Kwia Vigorbloom",
            "Ayli, Eternal Pilgrim",
            "Elenda, Saint of Dusk",
        ],
        &["Bloombrute", "Fiendish Panda"],
    ),
    commander(
        "Landfall",
        "Extra land drops and fetching trigger landfall: cards, life and tokens.",
        &[M("land fetchers")],
        &["Tatyova, Benthic Druid", "Thranduil, Sindarin Liege"],
        &["Cultivate"],
    ),
    commander(
        "Card draw / second card",
        "Draw extra cards each turn; payoffs trigger on your second card.",
        &[
            M("card draw"),
            Theme::Words {
                label: "connive / second card",
                enablers: &["connive", "connives"],
                payoffs: &["second card"],
            },
        ],
        &[
            "Kang, Temporal Tyrant",
            "Bard the Bowman",
            "Moon Girl and Devil Dinosaur",
        ],
        &["King T'Challa", "Kid Loki"],
    ),
    // ---------------- League set traits ----------------
    tr(
        "Villains",
        "MSH",
        "Villain tribal: Villain-spell triggers, Villain anthems and sacrifice.",
        &[T("Villain")],
        &[
            "Madame Hydra",
            "Bullseye, Death Dealer",
            "The Masters of Evil",
        ],
        &[
            "Madame Hydra",
            "Avengers: Under Siege",
            "Baron Strucker, HYDRA Overlord",
        ],
    ),
    tr(
        "Heroes",
        "MSH",
        "Hero tribal: every Hero entering makes a token or pumps the team.",
        &[T("Hero"), M("creature tokens for anthems")],
        &["Black Panther, Vanguard", "Captain America, Super-Soldier"],
        &["Avengers Assemble!", "Avengers Tower", "Hero in Training"],
    ),
    tr(
        "Adventure",
        "HOB",
        "Adventure cards and FRA's prepared creatures (two spells in one card); Codie copies prepared spells, Bilbo makes spells cast from exile cheaper.",
        &[Theme::Custom {
            label: "Adventure / prepared cards",
            read: adventure,
        }],
        &[
            "Codie, Ravenous Codex",
            "Bilbo, Thief in the Night",
            "Thranduil, Sindarin Liege",
        ],
        &[
            "Bilbo, Thief in the Night",
            "An Unexpected Party",
            "The Arkenstone",
        ],
    ),
    tr(
        "Power-up",
        "MSH",
        "Power-up creatures grow late; Hulk makes power-up abilities cheaper.",
        &[M("power-up creatures"), M("big creatures")],
        &["Hulk, Gamma Goliath", "Abomination, Terrifying Titan"],
        &["Hulk, Gamma Goliath", "She-Hulk, Jade Defender"],
    ),
    tr(
        "Teamwork",
        "MSH",
        "Tap creatures for teamwork costs; Captain America untaps them.",
        &[M("Teamwork spells"), T("Hero")],
        &[
            "Captain America, Living Legend",
            "Spider-Woman, Secret Agent",
        ],
        &[
            "Captain America, Living Legend",
            "Murdock's Crusade",
            "Agent Maria Hill",
        ],
    ),
    tr(
        "Plans",
        "MSH",
        "Plans: enchantments that pay off over turns; Doctor Doom fetches them.",
        &[M("Plans"), T("Villain")],
        &["Doctor Doom", "The Masters of Evil"],
        &[
            "Doctor Doom",
            "Doom Reigns Supreme",
            "Construct a Cosmic Cube",
        ],
    ),
    SetArchetype {
        name: "Infinity Stones",
        kind: Kind::SetTrait,
        set: "MSH",
        colors: "",
        plan: "Harness Infinity Stones for their ∞ abilities (only The Mind Stone is in the league card pool).",
        themes: &[Theme::Custom {
            label: "Infinity Stones",
            read: infinity,
        }],
        commanders: &[],
        key_cards: &["The Mind Stone"],
        sparse: true,
    },
    SetArchetype {
        name: "Sneak",
        kind: Kind::SetTrait,
        set: "MSH",
        colors: "",
        plan: "Sneak: return an unblocked attacker to cast a card tapped and attacking (only Elektra has it).",
        themes: &[Theme::Words {
            label: "sneak",
            enablers: &["sneak"],
            payoffs: &[],
        }],
        commanders: &["Elektra, Daughter of the Hand"],
        key_cards: &["Elektra, Daughter of the Hand"],
        sparse: true,
    },
    tr(
        "Storied",
        "HOB",
        "Three artifacts, legendaries or Sagas give an enduring story that switches on storied cards.",
        &[M("legendaries/artifacts/Sagas"), M("Equipment")],
        &[
            "Bifur, Melodic Rider",
            "Thorin Oakenshield",
            "Balin, Loremaster",
        ],
        &[
            "Bifur, Melodic Rider",
            "Thorin Oakenshield",
            "Balin, Loremaster",
        ],
    ),
    tr(
        "Treasure",
        "HOB",
        "Make Treasure and spend it; Smaug draws when Treasure mana casts a spell.",
        &[M("Treasure makers"), M("artifacts")],
        &["Smaug, Wicked Worm", "Smaug the Magnificent"],
        &["Smaug, Wicked Worm", "The Misty Mountains Cold"],
    ),
    tr(
        "Dwarves",
        "HOB",
        "Dwarf tribal with Equipment and storied.",
        &[T("Dwarf"), M("Equipment")],
        &[
            "Thorin Oakenshield",
            "Dáin Ironfoot",
            "Dwalin, Weaponmaster",
        ],
        &["Dáin's Company", "Iron Hills Blacksmith"],
    ),
    tr(
        "Elves",
        "HOB",
        "Elf tribal: lords, Elf tokens and landfall.",
        &[T("Elf"), M("land fetchers")],
        &["Thranduil, Sindarin Liege", "Lathril, Blade of the Elves"],
        &[
            "Thranduil's Company",
            "Elvish Archdruid",
            "Galion, Elvenking's Butler",
        ],
    ),
    tr(
        "Goblins",
        "HOB",
        "Goblin tribal and amass Armies; sacrifice Goblins for value.",
        &[T("Goblin"), M("amass cards")],
        &["The Great Goblin", "Krenko, Mob Boss", "Bolg of the North"],
        &["Bolg's Company", "Fearsome Goblin Pair"],
    ),
    tr(
        "Wolves",
        "HOB",
        "Wolves and ferocious: a 4-power creature turns the pack on.",
        &[T("Wolf"), M("big creatures")],
        &["The Chief Warg", "Tom, Bert, and William"],
        &["Chief Warg's Company", "Wargling", "Ravening Warg"],
    ),
    tr(
        "Birds and fliers",
        "FDN",
        "Birds and fliers with lords (Empyrean Eagle).",
        &[T("Bird"), M("creatures with flying")],
        &["Kykar, Zephyr Awakener", "The Lord of the Eagles"],
        &["Empyrean Eagle"],
    ),
    tr(
        "Dinosaurs",
        "FDN",
        "Dinosaurs: big bodies cheapened by Ghalta.",
        &[T("Dinosaur"), M("big creatures")],
        &["Ghalta, Primal Hunger", "Moon Girl and Devil Dinosaur"],
        &["Gigantosaurus"],
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn db() -> CardDb {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
        CardDb::load(&dir.join("cards.json")).unwrap()
    }

    fn all(db: &CardDb) -> Collection {
        Collection::new(
            PathBuf::new(),
            db.cards.iter().map(|c| (c.name.clone(), 1)).collect(),
        )
    }

    fn arch(name: &str) -> &'static SetArchetype {
        ARCHETYPES.iter().find(|a| a.name == name).unwrap()
    }

    /// Every named card exists and every commander can lead; every mechanic theme is one
    /// the builder reads; buildable archetypes have enough cards in the database.
    #[test]
    fn catalogue_matches_the_card_database() {
        let db = db();
        let all = all(&db);
        let needs: Vec<&str> = analysis::mechanic_needs().collect();
        assert!(ARCHETYPES.len() >= 28);
        for (n, t) in TARGETS {
            let a = ARCHETYPES.iter().find(|a| a.name == *n).expect(n);
            assert!(!a.sparse, "{}", n);
            assert!(t.lands.is_some_and(|l| (24..=30).contains(&l)), "{}", n);
            assert!(t.package + t.creatures <= 60, "{}", n);
        }
        for a in ARCHETYPES {
            for th in a.themes {
                if let Theme::Mechanic(n) = th {
                    assert!(needs.contains(n), "{}: unknown mechanic {}", a.name, n);
                }
            }
            for n in a.commanders {
                let c = db
                    .get(n)
                    .unwrap_or_else(|| panic!("{}: no card {}", a.name, n));
                assert_eq!(c.name, *n, "spelling");
                assert!(c.can_be_commander(), "{} can't lead", n);
            }
            for n in a.key_cards {
                let c = db
                    .get(n)
                    .unwrap_or_else(|| panic!("{}: no card {}", a.name, n));
                assert_eq!(c.name, *n, "spelling");
            }
            let r = pool_report(a, &db, &all);
            if a.sparse {
                assert!(r.cards.len() < 12, "{} isn't sparse", a.name);
            } else {
                assert!(!a.commanders.is_empty(), "{}", a.name);
                assert!(
                    r.cards.len() >= 15,
                    "{}: only {} fitting cards",
                    a.name,
                    r.cards.len()
                );
            }
        }
    }

    /// Hand-checked readings: payoffs and enablers of the sets' signature traits and the
    /// Commander archetypes.
    #[test]
    fn archetype_detection_reads_the_signposts() {
        let db = db();
        let fit = |a: &str, card: &str| {
            let mut c = db.get(card).unwrap().clone();
            // Judge the reading, not the key-card list.
            c.name = format!("{} (test)", c.name);
            arch(a).fit(&c)
        };
        // Elves/Dwarves/Wolves: irregular plurals ("other Elves you control").
        assert!(fit("Elves", "Thranduil, Sindarin Liege").is_payoff());
        assert!(fit("Wolves", "Chief Warg's Company").is_payoff());
        assert!(fit("Wolves", "Wargling").enabler >= 1.0);
        assert!(fit("Storied", "Thorin Oakenshield").is_payoff());
        // Captain America untaps creatures that become tapped: a teamwork payoff.
        assert!(fit("Teamwork", "Captain America, Living Legend").is_payoff());
        assert!(fit("Teamwork", "Murdock's Crusade").enabler >= 1.0);
        // Power-up: Hulk discounts them, power-up creatures feed him.
        assert!(fit("Power-up", "Hulk, Gamma Goliath").is_payoff());
        assert!(fit("Power-up", "She-Hulk, Jade Defender").enabler >= 1.0);
        // Prowess is a noncreature-spell payoff.
        assert!(fit("Spellslinger", "Thor Odinson").is_payoff());
        // Fliers lord, Treasure payoff, second-card payoffs.
        assert!(fit("Birds and fliers", "Empyrean Eagle").is_payoff());
        assert!(fit("Treasure", "Smaug, Wicked Worm").is_payoff());
        assert!(fit("Card draw / second card", "Kang, Temporal Tyrant").is_payoff());
        assert!(fit("Card draw / second card", "King T'Challa").is_payoff());
        // Villain tribe: members enable, "Villain spell" pays off.
        assert!(fit("Villains", "Madame Hydra").is_payoff());
        assert!(fit("Villains", "Red Hulk").enabler >= 1.0);
        // Goblin Army makers count for the Goblin tribe.
        assert!(fit("Goblins", "Fearsome Goblin Pair").fits());
        // Commander archetypes.
        assert!(fit("Stax-lite control", "Propaganda").is_payoff());
        assert!(fit("Stax-lite control", "Monologue Tax").is_payoff());
        assert!(fit("Reactive control", "Counterspell").is_payoff());
        assert!(fit("Voltron", "Captain America's Shield").enabler >= 1.0);
        assert!(fit("Ramp into big threats", "Sol Ring").enabler >= 1.0);
        assert!(fit("Adventure", "Bilbo, Thief in the Night").is_payoff());
        assert!(fit("Adventure", "Bilbo Baggins, Burglar").enabler >= 1.0);
        assert!(fit("Infinity Stones", "The Mind Stone").enabler >= 1.0);
        // Not everything fits.
        assert!(!fit("Artifacts", "Counterspell").fits());
        assert!(!fit("Elves", "Red Hulk").fits());
    }

    #[test]
    fn plurals_and_lookup() {
        assert_eq!(plural("Elf"), "Elves");
        assert_eq!(plural("dwarf"), "dwarves");
        assert_eq!(plural("Hero"), "Heroes");
        assert_eq!(plural("Army"), "Armies");
        assert_eq!(plural("Goblin"), "Goblins");
        assert!(find("hob").len() >= 6);
        assert!(find("commander").len() >= 14);
        assert_eq!(find("dwarves")[0].name, "Dwarves");
        assert_eq!(
            for_commander("Thanos, the Mad Titan").unwrap().name,
            "Stax-lite control"
        );
        assert_eq!(for_commander("Thorin Oakenshield").unwrap().name, "Dwarves");
        assert!(for_commander("Elektra, Daughter of the Hand").is_none());
        assert_eq!(targets(arch("Elves")), DEFAULT_TARGETS);
        assert_eq!(targets(arch("Artifacts")).package, 0);
        assert_eq!(targets(arch("Sneak")), DEFAULT_TARGETS);
    }

    /// A deck built around a commander reads as that commander's archetype.
    #[test]
    fn deck_best_fit() {
        let db = db();
        let all = all(&db);
        let mut deck = Deck::new("t");
        deck.commander = Some("Thranduil, Sindarin Liege".into());
        let elves = arch("Elves");
        let id = db.get("Thranduil, Sindarin Liege").unwrap().identity();
        for (c, _) in pool_report(elves, &db, &all)
            .cards
            .iter()
            .filter(|(c, _)| c.identity().is_subset_of(id) && c.is_creature())
            .take(25)
        {
            deck.add(&c.name, 1);
        }
        let best = best_for_deck(&deck, &db).unwrap();
        assert_eq!(best.arch.name, "Elves");
        assert!(best.closeness > 0.8, "{}", best.closeness);
    }

    /// The archetype build follows its targets: legal size, the land count, the creature
    /// minimum, and the commander's archetype is detected by `autobuild`.
    #[test]
    fn build_follows_targets() {
        let db = db();
        let all = all(&db);
        let mut rules = crate::db::Rules::default();
        let _ = crate::league::apply_session(&mut rules, 6);
        let cmd = db.get("Bifur, Melodic Rider").unwrap();
        let (deck, short, a) = autobuild(cmd, &db, &all, &rules, None, None);
        assert_eq!(a.unwrap().name, "Storied");
        assert_eq!(short, 0);
        assert_eq!(deck.total_count(&rules), 60);
        let st = analysis::stats(&deck, &db);
        let t = targets(arch("Storied"));
        assert_eq!(st.lands, t.lands.unwrap());
        assert!(st.creatures >= t.creatures, "{} creatures", st.creatures);
        let r = deck_report(arch("Storied"), &deck, &db).unwrap();
        assert!(r.cards.len() >= t.package as usize);
        // Forced colours fall back to the plain auto-build.
        let (_, _, none) = autobuild(cmd, &db, &all, &rules, Some(cmd.identity()), None);
        assert!(none.is_none());
        // Archetypes whose targets didn't hold up keep the plain auto-build.
        let elf = db.get("Thranduil, Sindarin Liege").unwrap();
        assert!(autobuild(elf, &db, &all, &rules, None, None).2.is_none());
    }
}
