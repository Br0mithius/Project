//! The interactive command set. Each command is a method on `App`.

use crate::analysis::{self, color_char, color_name, curve_label};
use crate::card::{BASIC_FOR_COLOR, Card, ColorSet, Role};
use crate::db::{CardDb, Collection, Lookup, Rules};
use crate::deck::{Deck, Severity, find_in_deck};
use crate::league;
use crate::ui::{Style, bar, compact_cost, pad, pct, wrap};
use std::fs;
use std::path::PathBuf;

pub const COMMANDS: &[(&str, &str, &str)] = &[
    (
        "help",
        "[command]",
        "Show this list, or details for one command",
    ),
    (
        "pool",
        "[filters]",
        "List cards you own (try: pool c:r t:creature mv<=3 role:removal fits avail)",
    ),
    (
        "show",
        "<card>",
        "Full card details, how many you own and how it scores in this deck",
    ),
    (
        "commanders",
        "",
        "Rank every legendary creature you own as a possible commander",
    ),
    (
        "why",
        "<commander>",
        "Explain a commander's ranking (and its best Prismatic Piper colour)",
    ),
    (
        "ratings",
        "",
        "Download EDHREC data, Scryfall tags and Playgroup.gg commander win rates",
    ),
    (
        "import",
        "<file> [replace]",
        "Import a Mana Mixer / ManaBox list into your pool (adds; `replace` swaps the whole pool)",
    ),
    (
        "update-cards",
        "",
        "Add new cards from every league set (incl. Nauctis once released) via Scryfall",
    ),
    ("commander", "<card>", "Set the deck's commander"),
    (
        "add",
        "[n] <card>",
        "Add n copies (default 1) to the main deck",
    ),
    (
        "rm",
        "[n] <card>",
        "Remove n copies (default 1) from the main deck",
    ),
    ("deck", "", "Show the current deck grouped by card type"),
    (
        "stats",
        "",
        "Mana curve, colour pips, card types, roles and mana sources",
    ),
    (
        "check",
        "",
        "Validate the deck against the format rules and your collection",
    ),
    (
        "suggest",
        "[n]",
        "Best cards you could still add (with reasons)",
    ),
    ("cut", "[n]", "Weakest cards currently in the deck"),
    (
        "auto",
        "[commander]",
        "Auto-build a complete legal deck (uses current commander if none given)",
    ),
    ("lands", "", "Recommended land count and basic land split"),
    (
        "fill",
        "",
        "Top the deck up to full size with basics split by colour need",
    ),
    (
        "hand",
        "",
        "Draw a random opening hand plus the next 3 draws",
    ),
    (
        "sim",
        "[trials]",
        "Simulate opening hands: land odds, keepable hands, commander on curve",
    ),
    ("new", "[name]", "Start a new empty deck"),
    ("save", "[name]", "Save the deck to decks/<name>.txt"),
    ("load", "<name>", "Load decks/<name>.txt"),
    ("decks", "", "List saved decks"),
    (
        "export",
        "[deck|pool|mixer]",
        "Deck list for Moxfield/Archidekt; `pool` / `mixer` = Mana Mixer lists",
    ),
    (
        "season",
        "[1-13]",
        "League schedule; `season 6` applies that session's rules (size, singleton)",
    ),
    (
        "partner",
        "<card>|none",
        "Second commander: a Partner card, or The Prismatic Piper",
    ),
    (
        "piper",
        "<colour>",
        "Choose the colour The Prismatic Piper adds (e.g. `piper r`)",
    ),
    (
        "draft",
        "<card>; <card>; ...",
        "Rank a draft pack's cards for your deck (separate names with ;)",
    ),
    (
        "upgrades",
        "",
        "Cards that would improve this deck: Public Pool claims and odds from upcoming packs",
    ),
    (
        "publicpool",
        "<file>",
        "Save the Public Pool list from Mana Mixer (used by `upgrades`)",
    ),
    (
        "roster",
        "[player marvel-kickoff msh|hob] | reset",
        "Who opens Marvel / Hobbit packs (pack odds, playtest opponents), e.g. `roster 2 3 hob`",
    ),
    (
        "trade",
        "[vp] [rare|mythic]",
        "Plan Public Pool trade-ins (default 10 VP minimum)",
    ),
    (
        "vp",
        "",
        "Value Point prices: your pool's trade-in values, Skill Tree costs",
    ),
    (
        "own",
        "[n] <card>",
        "Add cards to your collection (data/collection.txt)",
    ),
    ("unown", "[n] <card>", "Remove cards from your collection"),
    (
        "newcard",
        "",
        "Type in a card that isn't in the database yet",
    ),
    (
        "rules",
        "[key value]",
        "Show or change format rules (deck size, life, copies...)",
    ),
    ("undo", "", "Undo the last deck change"),
    ("clear", "", "Empty the main deck (keeps the commander)"),
    ("quit", "", "Save and exit"),
];

const FILTER_HELP: &str = "Filters (combine freely):
  c:r / c:wu / c:c   card has any of these colours (c = colourless)
  id:rg              card fits inside this colour identity
  fits               card fits the current commander's identity
  t:creature         type line contains text (creature, instant, villain, hero...)
  mv:3  mv<=2  mv>=5 mana value
  o:draw             rules text contains text
  r:uncommon         rarity
  role:removal       removal bounce counter draw ramp tokens trick evasion
                     protection lifegain teamwork enrage
  avail              only cards with copies not already in the deck
  legend             legendary only
  sort:mv|name|score|type
  anything else      matched against the card name";

/// Text the user must answer when a command needs more input (used by `newcard`).
pub type Ask<'a> = &'a mut dyn FnMut(&str) -> Option<String>;

pub struct App {
    pub data_dir: PathBuf,
    pub db: CardDb,
    pub coll: Collection,
    pub rules: Rules,
    pub deck: Deck,
    history: Vec<Deck>,
    pub style: Style,
    /// Set when the card list changes so the shell can refresh tab-completion.
    pub names_changed: bool,
    /// Commander ranking, kept until the pool or rules change (it takes a few seconds).
    ranking: std::cell::RefCell<Option<(String, crate::ranking::Rankings)>>,
}

impl App {
    pub fn new(data_dir: PathBuf, style: Style) -> Result<App, String> {
        let mut db = CardDb::load(&data_dir.join("cards.json"))?;
        db.attach_ratings(&data_dir);
        let coll = Collection::load(&data_dir.join("collection.txt"))?;
        let rules_path = data_dir.join("rules.json");
        let rules = if rules_path.exists() {
            Rules::load(&rules_path)
        } else {
            // First run: use the rules for the next league session.
            let mut r = Rules::default();
            let _ = league::apply_session(&mut r, league::upcoming_session().number);
            r
        };
        let current = crate::paths::decks_dir(&data_dir).join("_current.txt");
        let deck = if current.exists() {
            Deck::load(&current, &db).map(|mut d| {
                d.name = "current".into();
                d
            })?
        } else {
            Deck::new("current")
        };
        Ok(App {
            data_dir,
            db,
            coll,
            rules,
            deck,
            history: Vec::new(),
            style,
            names_changed: false,
            ranking: std::cell::RefCell::new(None),
        })
    }

    fn decks_dir(&self) -> PathBuf {
        crate::paths::decks_dir(&self.data_dir)
    }

    pub fn prompt(&self) -> String {
        let cmd = self
            .deck
            .commander
            .clone()
            .unwrap_or_else(|| "no commander".into());
        format!(
            "[{} | {}/{}] > ",
            cmd,
            self.deck.total_count(&self.rules),
            self.rules.deck_size
        )
    }

    pub fn card_names(&self) -> Vec<String> {
        self.db.cards.iter().map(|c| c.name.clone()).collect()
    }

    fn snapshot(&mut self) {
        self.history.push(self.deck.clone());
        if self.history.len() > 100 {
            self.history.remove(0);
        }
    }

    fn autosave(&self) {
        let _ = self.deck.save(&self.decks_dir().join("_current.txt"));
    }

    /// Runs one command line. Returns false when the user wants to quit.
    pub fn run(&mut self, line: &str, ask: Ask) -> bool {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return true;
        }
        let (cmd, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        let before = self.deck.clone();
        match cmd.to_lowercase().as_str() {
            "help" | "h" | "?" => self.help(rest),
            "pool" | "list" | "ls" => self.pool(rest),
            "show" | "card" | "info" => self.show(rest),
            "commanders" => self.commanders(),
            "why" => self.why(rest),
            "ratings" => self.ratings(),
            "update-cards" => self.update_cards(),
            "import" => self.import(rest),
            "commander" | "cmd" => self.set_commander(rest),
            "add" | "+" => self.add(rest),
            "rm" | "remove" | "-" => self.remove(rest),
            "deck" | "view" => self.view(),
            "stats" => self.stats(),
            "check" | "validate" => self.check(),
            "suggest" => self.suggest(rest),
            "cut" | "weak" => self.cut(rest),
            "auto" | "build" => self.auto(rest),
            "lands" => self.lands(),
            "fill" => self.fill(),
            "hand" => self.hand(),
            "sim" => self.sim(rest),
            "new" => self.new_deck(rest),
            "save" => self.save(rest),
            "load" => self.load(rest),
            "decks" => self.list_decks(),
            "export" => self.export(rest),
            "season" | "session" => self.season(rest),
            "partner" => self.partner(rest),
            "piper" => self.piper(rest),
            "draft" | "pack" => self.draft(rest),
            "trade" => self.trade(rest),
            "upgrades" | "upgrade" => self.upgrades(),
            "publicpool" => self.public_pool(rest),
            "roster" => self.roster(rest),
            "vp" => self.vp(),
            "own" => self.own(rest, 1),
            "unown" => self.own(rest, -1),
            "newcard" => self.new_card(ask),
            "rules" => self.rules_cmd(rest),
            "undo" => self.undo(),
            "clear" => self.clear(),
            "quit" | "exit" | "q" => {
                self.autosave();
                return false;
            }
            _ => println!("Unknown command '{}'. Type `help` for the list.", cmd),
        }
        if self.deck != before && cmd != "undo" {
            self.history.push(before);
            self.autosave();
        }
        true
    }

    // ---------- lookup helpers ----------

    /// Resolves a typed name to a card, printing why if it can't.
    fn resolve(&self, query: &str) -> Option<Card> {
        match self.db.lookup(query) {
            Lookup::Found(c) => Some(c.clone()),
            Lookup::Ambiguous(v) => {
                println!("'{}' matches several cards - be more specific:", query);
                for c in v.iter().take(12) {
                    println!("  {}", self.style.name(c));
                }
                None
            }
            Lookup::NotFound(close) => {
                println!("No card called '{}'.", query);
                if !close.is_empty() {
                    let names: Vec<String> = close.iter().map(|c| c.name.clone()).collect();
                    println!("Did you mean: {}?", names.join(", "));
                } else {
                    println!("If it's a new card, add it with `newcard`.");
                }
                None
            }
        }
    }

    /// Splits "2 Web Up" / "Web Up x2" / "Web Up" into (count, name).
    fn count_and_name(arg: &str) -> (u32, String) {
        let arg = arg.trim();
        if let Some((first, rest)) = arg.split_once(char::is_whitespace) {
            if let Ok(n) = first.trim_end_matches(['x', 'X']).parse::<u32>() {
                return (n, rest.trim().to_string());
            }
        }
        if let Some((rest, last)) = arg.rsplit_once(char::is_whitespace) {
            let l = last.to_lowercase();
            if let Some(n) = l.strip_prefix('x').and_then(|s| s.parse::<u32>().ok()) {
                return (n, rest.trim().to_string());
            }
        }
        (1, arg.to_string())
    }

    fn available(&self, card: &Card) -> u32 {
        analysis::available(card, &self.deck, &self.coll, &self.rules)
    }

    fn roles_str(card: &Card) -> String {
        let r: Vec<&str> = card.roles().iter().map(|r| r.label()).collect();
        r.join(",")
    }

    fn card_row(&self, card: &Card, count_col: &str) -> String {
        let pt = card.power_toughness().unwrap_or_default();
        let flag = if card.is_verified() || card.is_basic() {
            " "
        } else {
            "?"
        };
        format!(
            "{:>5} {}{} {} {} {} {}",
            count_col,
            self.style.name_padded(card, 32),
            flag,
            pad(&compact_cost(&card.mana_cost), 8),
            pad(&card.type_line, 42),
            pad(&pt, 5),
            self.style.dim(&Self::roles_str(card))
        )
    }

    // ---------- commands ----------

    fn help(&self, topic: &str) {
        if !topic.is_empty() {
            match COMMANDS.iter().find(|(c, _, _)| *c == topic) {
                Some((c, args, desc)) => {
                    println!("{} {}\n  {}", self.style.bold(c), args, desc);
                    if *c == "pool" {
                        println!("{}", FILTER_HELP);
                    }
                }
                None => println!("No command '{}'.", topic),
            }
            return;
        }
        println!("{}", self.style.bold("Commands"));
        for (c, args, desc) in COMMANDS {
            println!("  {:<11}{:<14}{}", c, args, desc);
        }
        println!("\n{}", FILTER_HELP);
        println!(
            "\nTips: TAB completes card names. Card names can be partial (`add 2 roxxon`).\n\
             A '?' after a name means its text came from web summaries - double-check the real card."
        );
    }

    fn pool(&self, args: &str) {
        let mut cards: Vec<&Card> = self
            .db
            .cards
            .iter()
            .filter(|c| self.coll.owned(&c.name) > 0)
            .collect();
        let mut sort = "type".to_string();
        let identity = self.deck.identity(&self.db);
        for tok in args.split_whitespace() {
            let t = tok.to_lowercase();
            if let Some(s) = t.strip_prefix("sort:") {
                sort = s.to_string();
                continue;
            }
            cards.retain(|c| self.matches_filter(c, &t, identity));
        }
        match sort.as_str() {
            "name" => cards.sort_by(|a, b| a.name.cmp(&b.name)),
            "mv" | "cmc" => {
                cards.sort_by(|a, b| a.cmc.partial_cmp(&b.cmc).unwrap().then(a.name.cmp(&b.name)))
            }
            "score" => {
                let score = |c: &Card| analysis::score_card(c, &self.deck, &self.db, &self.rules).0;
                cards.sort_by(|a, b| score(b).partial_cmp(&score(a)).unwrap());
            }
            _ => cards.sort_by(|a, b| {
                a.main_type()
                    .cmp(b.main_type())
                    .then(a.cmc.partial_cmp(&b.cmc).unwrap())
                    .then(a.name.cmp(&b.name))
            }),
        }
        let total: u32 = cards.iter().map(|c| self.coll.owned(&c.name)).sum();
        println!(
            "{}",
            self.style.dim(&format!(
                "{:>5} {:<33} {:<8} {:<42} {:<5} roles      (avail/owned)",
                "", "name", "cost", "type", "p/t"
            ))
        );
        let mut last_type = "";
        for c in &cards {
            if sort == "type" && c.main_type() != last_type {
                last_type = c.main_type();
                println!("{}", self.style.bold(&format!("-- {} --", last_type)));
            }
            let col = format!("{}/{}", self.available(c), self.coll.owned(&c.name));
            println!("{}", self.card_row(c, &col));
        }
        println!("{} different cards, {} total.", cards.len(), total);
    }

    fn matches_filter(&self, c: &Card, t: &str, identity: Option<ColorSet>) -> bool {
        let num_cmp = |expr: &str| -> Option<bool> {
            let mv = c.mana_value();
            for (op, f) in [
                ("<=", (|a: u32, b: u32| a <= b) as fn(u32, u32) -> bool),
                (">=", |a, b| a >= b),
                ("<", |a, b| a < b),
                (">", |a, b| a > b),
                (":", |a, b| a == b),
                ("=", |a, b| a == b),
            ] {
                if let Some(v) = expr.strip_prefix(op) {
                    return v.parse::<u32>().ok().map(|n| f(mv, n));
                }
            }
            None
        };
        if let Some(v) = t.strip_prefix("c:").or_else(|| t.strip_prefix("color:")) {
            if v == "c" || v == "colorless" {
                return c.identity().is_empty();
            }
            return ColorSet::parse(v)
                .map(|s| c.identity().0 & s.0 != 0)
                .unwrap_or(false);
        }
        if let Some(v) = t.strip_prefix("id:") {
            return ColorSet::parse(v)
                .map(|s| c.identity().is_subset_of(s))
                .unwrap_or(false);
        }
        if t == "fits" {
            return identity
                .map(|id| c.identity().is_subset_of(id))
                .unwrap_or(true);
        }
        if t == "avail" {
            return self.available(c) > 0;
        }
        if t == "legend" || t == "legendary" {
            return c.is_legendary();
        }
        if let Some(v) = t.strip_prefix("t:").or_else(|| t.strip_prefix("type:")) {
            return c.type_line.to_lowercase().contains(v);
        }
        if let Some(v) = t.strip_prefix("mv").or_else(|| t.strip_prefix("cmc")) {
            return num_cmp(v).unwrap_or(false);
        }
        if let Some(v) = t.strip_prefix("o:").or_else(|| t.strip_prefix("text:")) {
            return c.oracle_text.to_lowercase().contains(v);
        }
        if let Some(v) = t.strip_prefix("r:") {
            return c.rarity.to_lowercase().starts_with(v);
        }
        if let Some(v) = t.strip_prefix("role:") {
            return Role::parse(v).map(|r| c.has_role(r)).unwrap_or(false);
        }
        crate::db::normalize(&c.name).contains(&crate::db::normalize(t))
    }

    fn show(&self, q: &str) {
        let Some(c) = self.resolve(q) else { return };
        let s = &self.style;
        println!("{}  {}", s.bold(&s.name(&c)), compact_cost(&c.mana_cost));
        println!("  {}", c.type_line);
        print!("{}", wrap(&c.oracle_text, 72, "  │ "));
        if let Some(pt) = c.power_toughness() {
            println!("  {}", s.bold(&pt));
        }
        println!(
            "  {} · {} #{} · identity {} · MV {}",
            c.rarity,
            if c.set.is_empty() { "MSH" } else { &c.set },
            c.collector_number.clone().unwrap_or_else(|| "?".into()),
            c.identity(),
            c.mana_value()
        );
        if !c.tags.is_empty() {
            println!("  Scryfall tags: {}", c.tags.join(", "));
        }
        for line in crate::edhrec::card_summary(&self.db.edhrec, &c.name, &self.deck.commanders()) {
            println!("  {}", line);
        }
        let parsed = c.parsed();
        let reading = parsed.describe();
        if !reading.is_empty() || !parsed.unparsed.is_empty() {
            println!("  {}", s.dim("How the app reads it:"));
            for line in reading {
                println!("    {}", s.dim(&line));
            }
            if let Some(r) = parsed.removal() {
                println!(
                    "    {}",
                    s.dim(&format!("Removal: {} ({:.1} points)", r.label, r.points))
                );
            }
            for u in &parsed.unparsed {
                println!("    ? not understood: {}", u);
            }
        }
        if c.can_be_commander() {
            if let Some(st) = self.db.commander_stat(&c.name) {
                println!(
                    "  As a commander: {:.0}% win rate in {} logged games ({})",
                    st.win_rate * 100.0,
                    st.games,
                    st.source
                );
            }
        }
        if !c.is_verified() && !c.is_basic() {
            println!(
                "  {} {}",
                s.warn("text confidence:"),
                if c.confidence.is_empty() {
                    "unknown"
                } else {
                    &c.confidence
                }
            );
        }
        if !c.note.is_empty() {
            print!("{}", s.dim(&wrap(&c.note, 72, "  note: ")));
        }
        println!(
            "  Own {} · in deck {} · available {}",
            self.coll.owned(&c.name),
            self.deck.used(&c.name),
            self.available(&c)
        );
        let roles = Self::roles_str(&c);
        if !roles.is_empty() {
            println!("  Roles: {}", roles);
        }
        if c.can_be_commander() {
            println!("  {}", s.good("Can be your commander."));
        }
        if let Some(id) = self.deck.identity(&self.db) {
            if !c.identity().is_subset_of(id) {
                println!(
                    "  {}",
                    s.bad(&format!("Outside your commander's identity ({}).", id))
                );
            } else if !c.is_land() {
                let (score, why) = analysis::score_card(&c, &self.deck, &self.db, &self.rules);
                println!(
                    "  Fit score for this deck: {:.1}  {}",
                    score,
                    s.dim(&why.join("; "))
                );
            }
        }
    }

    /// The commander ranking for the current pool and rules (built on first use).
    fn rankings(&self) -> crate::ranking::Rankings {
        let key = crate::ranking::ranking_key(&self.db, &self.coll, &self.rules);
        if let Some((k, r)) = self.ranking.borrow().as_ref() {
            if *k == key {
                return r.clone();
            }
        }
        println!(
            "{}",
            self.style.dim(
                "Building and playtesting a deck for every commander option (takes a few seconds)..."
            )
        );
        let done = std::sync::atomic::AtomicUsize::new(0);
        let r = crate::ranking::rank_with_preview(
            &self.db,
            &self.coll,
            &self.rules,
            &crate::ranking::Weights::default(),
            &done,
        );
        *self.ranking.borrow_mut() = Some((key, r.clone()));
        r
    }

    fn commanders(&self) {
        let ranks = self.rankings();
        let evals = &ranks.now;
        if evals.is_empty() {
            println!("You don't own any legendary creatures yet.");
            return;
        }
        let data = if self.db.edhrec.is_empty() {
            "no EDHREC data yet - run `ratings` to download it".to_string()
        } else {
            format!("EDHREC data from {}", self.db.edhrec.fetched)
        };
        println!(
            "Each commander is ranked by the deck auto-build makes for it ({}).",
            data
        );
        println!(
            "{}",
            self.style.dim(&format!(
                "  #  {:<38} {:<6} {:>5} | {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} | {}",
                "commander",
                "plays",
                "score",
                "cards",
                "curve",
                "crea",
                "inter",
                "mana",
                "cmdr",
                "play",
                "edh",
                if ranks.singleton.is_some() {
                    "singleton (Session 6+)"
                } else {
                    ""
                }
            ))
        );
        let groups = crate::ranking::best_per_commander(evals);
        for (i, g) in groups.iter().enumerate() {
            let e = g[0];
            let z = e.scaled;
            let other = g
                .iter()
                .skip(1)
                .map(|o| match o.piper {
                    Some(c) => format!("+Piper({}) {:.0}", c, o.total),
                    None => format!("alone {:.0}", o.total),
                })
                .collect::<Vec<_>>()
                .join(", ");
            let single = ranks
                .singleton_place(&e.commander)
                .map(|(p, t)| format!("#{} ({:.0})", p, t))
                .unwrap_or_default();
            println!(
                "{:>3}  {:<38} {:<6} {:>5.0} | {:>+5.1} {:>+5.1} {:>+5.1} {:>+5.1} {:>+5.1} {:>+5.1} {:>+5.1} {:>+5.1} | {}{}",
                i + 1,
                e.label(),
                e.plays.to_string(),
                e.total,
                z[0],
                z[1],
                z[2],
                z[3],
                z[4],
                z[5],
                z[6],
                z[7],
                single,
                if other.is_empty() {
                    String::new()
                } else {
                    self.style.dim(&format!("   (also: {})", other))
                }
            );
        }
        println!(
            "Score: 0-100, 50 = your middle option. Parts are on one scale: +1 = one typical spread better than your middle option."
        );
        println!(
            "Parts: cards = spell strength; curve and creatures vs. the commander's plan; removal & draw; mana = keepable hands / commander on time; commander's own strength; play = simulated 4-player games; edh = fit with the commander's EDHREC page."
        );
        println!(
            "`why <commander>` explains one; `auto <name> [colours]` builds it; `piper <colour>` adds The Prismatic Piper."
        );
    }

    fn why(&self, q: &str) {
        let Some(c) = self.resolve(q) else { return };
        if !c.can_be_commander() {
            println!("{} isn't a legendary creature.", c.name);
            return;
        }
        if self.coll.owned(&c.name) == 0 {
            println!("{} isn't in your pool, so it isn't ranked.", c.name);
            return;
        }
        let ranks = self.rankings();
        let groups = crate::ranking::best_per_commander(&ranks.now);
        let Some(place) = groups.iter().position(|g| g[0].commander == c.name) else {
            println!("{} isn't in the ranking.", c.name);
            return;
        };
        println!(
            "#{} of {} commanders{}",
            place + 1,
            groups.len(),
            ranks
                .singleton_place(&c.name)
                .map(|(p, t)| format!(" · #{} once singleton starts (score {:.0})", p, t))
                .unwrap_or_default()
        );
        for e in groups[place].iter().take(3) {
            println!(
                "{}  (plays {}) - score {:.0}/100",
                self.style.bold(&e.label()),
                e.plays,
                e.total
            );
            let raw = e.parts.values();
            for (k, label) in crate::ranking::Parts::LABELS.iter().enumerate() {
                println!(
                    "  {:<12} {:>+5.1}   {}",
                    label,
                    e.scaled[k],
                    self.style.dim(&if k == 6 {
                        format!("(won {:.0}%)", raw[k] * 100.0)
                    } else if !raw[k].is_finite() {
                        "(no data - middle)".to_string()
                    } else {
                        format!("(raw {:.2})", raw[k])
                    })
                );
            }
            println!(
                "  {} creatures, {} removal, {} draw · keepable hands {:.0}% · commander on time {:.0}%",
                e.creatures,
                e.removal,
                e.draw,
                e.keepable * 100.0,
                e.on_curve * 100.0
            );
            for n in &e.notes {
                println!("  - {}", n);
            }
        }
    }

    fn upgrades(&self) {
        if self.deck.commander.is_none() {
            println!("Choose a commander first (`commander <name>` or `auto <name>`).");
            return;
        }
        let public = crate::upgrades::load_public_pool(&self.data_dir);
        let next = league::upcoming_session().number;
        let ups =
            crate::upgrades::upgrades(&self.deck, &self.db, &self.coll, &self.rules, &public, next);
        let s = &self.style;
        let (pool, packs): (Vec<_>, Vec<_>) = ups
            .iter()
            .partition(|u| matches!(u.source, crate::upgrades::Source::Pool { .. }));
        println!("{}", s.bold("Claim from the Public Pool now"));
        if public.counts.is_empty() {
            println!(
                "  {}",
                s.dim("No Public Pool list saved - copy it from Mana Mixer and run `publicpool <file>`.")
            );
        } else if pool.is_empty() {
            println!(
                "  {}",
                s.dim("Nothing in the pool beats your weakest card.")
            );
        }
        for u in pool.iter().take(10) {
            println!(
                "  {:+5.1}  {:<34} {}  {}",
                u.gain,
                u.card.name,
                crate::upgrades::describe(u),
                s.dim(&format!("replaces {}", u.replaces))
            );
        }
        if let Some(top) = pool.first() {
            let plan =
                crate::upgrades::trade_ins_for(top, &self.deck, &self.db, &self.coll, &self.rules);
            let vp: u32 = plan.iter().map(|t| t.vp).sum();
            println!(
                "  To claim {} this round, trade in: {} ({} VP)",
                top.card.name,
                plan.iter()
                    .map(|t| format!("{}x {}", t.copies, t.card.name))
                    .collect::<Vec<_>>()
                    .join(", "),
                vp
            );
        }
        println!(
            "{}",
            s.bold(&format!(
                "Could come in the packs still to be handed out (Session {} on)",
                next
            ))
        );
        if packs.is_empty() {
            println!("  {}", s.dim("Nothing found."));
        }
        for u in packs.iter().take(15) {
            println!(
                "  {:+5.1}  {:<34} {}",
                u.gain,
                u.card.name,
                crate::upgrades::describe(u)
            );
        }
        println!(
            "{}",
            s.dim("Gain = how much better than your weakest card. Table = opened at your 4-player draft table (face-up draft, so it's in your draft); league = opened by any of the 8 players (it could be traded into the pool). Pack odds use typical Play Booster rarity rates.")
        );
    }

    fn roster(&mut self, arg: &str) {
        let words: Vec<&str> = arg.split_whitespace().collect();
        match words.as_slice() {
            [] => {}
            ["reset"] => self.rules.roster = crate::packs::Roster::default(),
            [who, marvel, weekly] => {
                let (Ok(i), Ok(m)) = (who.parse::<usize>(), marvel.parse::<u32>()) else {
                    println!(
                        "Usage: roster <player 0-7> <kickoff Marvel packs 0-9> <msh|hob> (player 0 is you)"
                    );
                    return;
                };
                let weekly = weekly.to_uppercase();
                if m > 9
                    || !["MSH", "HOB"].contains(&weekly.as_str())
                    || i > self.rules.roster.others.len()
                {
                    println!(
                        "Usage: roster <player 0-7> <kickoff Marvel packs 0-9> <msh|hob> (player 0 is you)"
                    );
                    return;
                }
                let r = &mut self.rules.roster;
                let p = if i == 0 {
                    &mut r.me
                } else {
                    &mut r.others[i - 1]
                };
                let name = p.name.clone();
                *p = crate::packs::Starter::new(&name, m, &weekly);
            }
            _ => {
                println!(
                    "Usage: roster <player 0-7> <kickoff Marvel packs 0-9> <msh|hob>, or `roster reset`"
                );
                return;
            }
        }
        if !words.is_empty() {
            let _ = self.rules.save(&self.data_dir.join("rules.json"));
        }
        let r = &self.rules.roster;
        println!("League starter packs (used for pack odds and playtest opponents):");
        for (i, p) in std::iter::once(&r.me).chain(&r.others).enumerate() {
            println!("  {}  {}", i, p.describe());
        }
    }

    fn public_pool(&mut self, arg: &str) {
        let path = arg.trim().trim_matches('"');
        if path.is_empty() {
            let public = crate::upgrades::load_public_pool(&self.data_dir);
            println!(
                "Public Pool list: {} cards ({} different). Update it with `publicpool <file>` (a Mana Mixer copy of the pool).",
                public.total(),
                public.counts.len()
            );
            return;
        }
        match std::fs::read(path).map(|b| league::decode_text(&b)) {
            Ok(text) => match crate::upgrades::save_public_pool(&self.data_dir, &text) {
                Ok(n) => println!(
                    "Saved {} different cards as the Public Pool. Run `upgrades` to see what's worth claiming.",
                    n
                ),
                Err(e) => println!("Couldn't save it: {}", e),
            },
            Err(e) => println!("Couldn't read {}: {}", path, e),
        }
    }

    fn import(&mut self, arg: &str) {
        let arg = arg.trim();
        let (path, replace) = match arg.strip_suffix(" replace") {
            Some(p) => (p.trim(), true),
            None => (arg, false),
        };
        if path.is_empty() {
            println!(
                "Usage: import <file> [replace]   (a Mana Mixer / ManaBox text or CSV export)"
            );
            return;
        }
        let text = match fs::read(path.trim_matches('"')).map(|b| league::decode_text(&b)) {
            Ok(t) => t,
            Err(e) => {
                println!("Couldn't read {}: {}", path, e);
                return;
            }
        };
        match league::import_pool(&text, replace, &self.db, &mut self.coll) {
            Ok(r) => {
                println!(
                    "{} {} cards ({} different){}.",
                    if replace {
                        "Replaced your pool with"
                    } else {
                        "Added"
                    },
                    r.cards,
                    r.distinct,
                    if replace {
                        "; the old pool is saved as data/collection.backup.txt"
                    } else {
                        ""
                    }
                );
                if !r.unknown.is_empty() {
                    println!(
                        "{} {}",
                        self.style.warn("Not in the card database (kept, but check spelling or run `update-cards`):"),
                        r.unknown.join(", ")
                    );
                }
            }
            Err(e) => println!("{}", e),
        }
    }

    fn update_cards(&mut self) {
        println!("Checking Scryfall for every league set...");
        match crate::setdata::update_cards(&self.data_dir) {
            Ok((added, checked, missing)) => {
                println!(
                    "  Checked {}; {} new cards added.",
                    checked.join(", "),
                    added
                );
                if !missing.is_empty() {
                    println!("  Not released yet: {}", missing.join(", "));
                }
                if let Ok(mut db) = CardDb::load(&self.data_dir.join("cards.json")) {
                    db.attach_ratings(&self.data_dir);
                    self.db = db;
                    self.names_changed = true;
                }
            }
            Err(e) => println!("  {}", e),
        }
    }

    fn ratings(&mut self) {
        println!("Downloading Playgroup.gg commander win rates...");
        let names: Vec<String> = self
            .db
            .cards
            .iter()
            .filter(|c| c.can_be_commander() && self.coll.owned(&c.name) > 0)
            .map(|c| c.name.clone())
            .collect();
        match crate::ratings::fetch_playgroup(&self.data_dir, &names) {
            Ok(n) => println!("  {} commanders found.", n),
            Err(e) => println!("  {}", e),
        }
        println!(
            "Downloading EDHREC card ranks (via Scryfall) and {} commander pages...",
            names.len()
        );
        match crate::edhrec::fetch(&self.data_dir, &self.db, &names, true) {
            Ok(f) => {
                println!(
                    "  {} cards ranked, {} commander pages, {} commanders without a page.",
                    f.ranks, f.pages, f.no_page
                );
                if let Some(e) = f.first_error {
                    println!(
                        "  Some pages failed ({}); details in data/edhrec_log.txt",
                        e
                    );
                }
            }
            Err(e) => println!("  Couldn't download EDHREC data: {}", e),
        }
        println!("Downloading Scryfall Tagger tags for the league sets...");
        match crate::tags::fetch(&self.data_dir, &self.db) {
            Ok((cards, tags)) => println!("  {} cards tagged across {} tags.", cards, tags),
            Err(e) => println!("  {}", e),
        }
        self.db.attach_ratings(&self.data_dir);
        println!(
            "Using {} EDHREC-ranked cards, {} EDHREC commander pages and {} Playgroup.gg commander records.",
            self.db.edhrec.ranked_cards(),
            self.db.edhrec.commander_count(),
            self.db.commander_stats.len()
        );
    }

    fn set_commander(&mut self, q: &str) {
        if q.is_empty() {
            match &self.deck.commander {
                Some(c) => self.show(&c.clone()),
                None => println!("No commander yet. Try `commanders` to see your options."),
            }
            return;
        }
        let Some(c) = self.resolve(q) else { return };
        if !c.can_be_commander() {
            println!(
                "{} isn't a legendary creature, so it can't be a commander.",
                c.name
            );
            return;
        }
        if self.rules.enforce_collection && self.coll.owned(&c.name) == 0 {
            println!("You don't own {} (use `own {}` if you do).", c.name, c.name);
            return;
        }
        // The commander's copy can't also be in the main deck if you only own one.
        if self.deck.count_of(&c.name) > 0
            && self.coll.owned(&c.name) <= self.deck.count_of(&c.name)
        {
            self.deck.remove(&c.name, 1);
        }
        self.deck.commander = Some(c.name.clone());
        println!(
            "Commander: {} (identity {})",
            self.style.name(&c),
            c.identity()
        );
        let off: Vec<String> = self
            .deck
            .cards
            .keys()
            .filter(|n| {
                self.db
                    .get(n)
                    .map(|x| !x.identity().is_subset_of(c.identity()))
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        if !off.is_empty() {
            println!(
                "{} {}",
                self.style.warn("Now outside the identity:"),
                off.join(", ")
            );
        }
    }

    fn add(&mut self, arg: &str) {
        let (n, q) = Self::count_and_name(arg);
        let Some(c) = self.resolve(&q) else { return };
        if self.rules.enforce_color_identity {
            if let Some(id) = self.deck.identity(&self.db) {
                if !c.identity().is_subset_of(id) {
                    println!(
                        "{} is {} but your commander's identity is {} - not added.",
                        c.name,
                        c.identity(),
                        id
                    );
                    return;
                }
            }
        }
        let avail = self.available(&c);
        let take = n.min(avail);
        if take == 0 {
            println!(
                "No copies of {} left (own {}, already using {}).",
                c.name,
                self.coll.owned(&c.name),
                self.deck.used(&c.name)
            );
            return;
        }
        if self.rules.max_copies > 0
            && !c.is_basic()
            && self.deck.used(&c.name) + take > self.rules.max_copies
        {
            println!("That would break the {}-copy limit.", self.rules.max_copies);
            return;
        }
        self.deck.add(&c.name, take);
        let note = if take < n {
            format!(" (only {} available)", take)
        } else {
            String::new()
        };
        println!(
            "Added {}x {}{}. Deck: {}/{}",
            take,
            self.style.name(&c),
            note,
            self.deck.total_count(&self.rules),
            self.rules.deck_size
        );
    }

    fn remove(&mut self, arg: &str) {
        let (n, q) = Self::count_and_name(arg);
        let hits = find_in_deck(&self.deck, &q);
        let name = match hits.len() {
            0 => {
                println!("'{}' isn't in the deck.", q);
                return;
            }
            1 => hits[0].clone(),
            _ => {
                let names: Vec<&str> = hits.iter().map(|s| s.as_str()).collect();
                println!("Which one? {}", names.join(", "));
                return;
            }
        };
        let took = self.deck.remove(&name, n);
        println!(
            "Removed {}x {}. Deck: {}/{}",
            took,
            name,
            self.deck.total_count(&self.rules),
            self.rules.deck_size
        );
    }

    fn view(&self) {
        let s = &self.style;
        println!("{}", s.bold(&format!("Deck '{}'", self.deck.name)));
        match self.deck.commander.as_ref().and_then(|c| self.db.get(c)) {
            Some(c) => println!(
                "Commander: {}  {}  [{}]",
                s.name(c),
                compact_cost(&c.mana_cost),
                c.identity()
            ),
            None => println!("Commander: {}", s.warn("none")),
        }
        let cards = analysis::resolved(&self.deck, &self.db);
        let mut groups: Vec<(&str, Vec<(&Card, u32)>)> = Vec::new();
        for t in [
            "Creature",
            "Planeswalker",
            "Instant",
            "Sorcery",
            "Artifact",
            "Enchantment",
            "Battle",
            "Other",
            "Land",
        ] {
            let mut g: Vec<(&Card, u32)> = cards
                .iter()
                .filter(|(c, _)| c.main_type() == t)
                .cloned()
                .collect();
            g.sort_by(|a, b| {
                a.0.cmc
                    .partial_cmp(&b.0.cmc)
                    .unwrap()
                    .then(a.0.name.cmp(&b.0.name))
            });
            if !g.is_empty() {
                groups.push((t, g));
            }
        }
        for (t, g) in groups {
            let n: u32 = g.iter().map(|(_, n)| n).sum();
            println!("{}", s.bold(&format!("-- {} ({}) --", t, n)));
            for (c, n) in g {
                println!("{}", self.card_row(c, &format!("{}x", n)));
            }
        }
        let unknown: Vec<&String> = self
            .deck
            .cards
            .keys()
            .filter(|n| self.db.get(n).is_none())
            .collect();
        if !unknown.is_empty() {
            println!("{} {:?}", s.bad("Not in database:"), unknown);
        }
        println!(
            "Total: {}/{} ({} in library{})",
            self.deck.total_count(&self.rules),
            self.rules.deck_size,
            self.deck.main_count(),
            if self.rules.commander_counts_toward_size {
                " + commander"
            } else {
                ""
            }
        );
    }

    fn stats(&self) {
        let s = &self.style;
        let st = analysis::stats(&self.deck, &self.db);
        let plan = analysis::deck_plan(&self.deck, &self.db, &self.rules);
        let targets = plan.curve;
        println!(
            "{}  {}",
            s.bold("Mana curve (non-land)            target"),
            s.dim(&format!(
                "(a {} deck: {}; {}-{} creatures, ~{:.0} removal, ~{:.0} draw)",
                plan.archetype.label(),
                plan.signals.join(", "),
                plan.creatures.0,
                plan.creatures.1,
                plan.removal,
                plan.draw
            ))
        );
        for b in 0..6 {
            println!(
                "  {:>3} | {:<20} {:>2}   {}",
                curve_label(b),
                bar(st.curve[b], 1),
                st.curve[b],
                s.dim(&format!("~{}", targets[b]))
            );
        }
        println!("  Average mana value: {:.2}", st.avg_mv);
        println!("{}", s.bold("Types"));
        for (t, n) in &st.types {
            println!("  {:<12} {}", t, n);
        }
        println!(
            "  Creatures: {}  Non-creature spells: {}  Lands: {}",
            st.creatures,
            st.nonlands - st.creatures,
            st.lands
        );
        println!("{}", s.bold("Colours: pips needed vs. land sources"));
        let identity = self.deck.identity(&self.db).unwrap_or_default();
        for i in 0..5 {
            if st.pips[i] == 0 && !identity.contains(i) {
                continue;
            }
            let basics = self.deck.count_of(BASIC_FOR_COLOR[i]);
            println!(
                "  {:<6} pips {:>3}   sources {:>3} ({} {})",
                color_name(i),
                st.pips[i],
                st.sources[i],
                basics,
                BASIC_FOR_COLOR[i]
            );
        }
        println!("{}", s.bold("Roles"));
        for r in Role::ALL {
            if let Some(n) = st.roles.get(&r) {
                println!("  {:<11} {}", r.label(), n);
            }
        }
        let tribes: Vec<String> = ["Hero", "Villain", "Robot"]
            .iter()
            .filter_map(|t| st.subtypes.get(*t).map(|n| format!("{} {}", n, t)))
            .collect();
        if !tribes.is_empty() {
            println!("  Tribes: {}", tribes.join(", "));
        }
        let rec = analysis::recommended_lands(st.avg_mv, &self.rules);
        let land_note = if st.lands < rec {
            s.warn(&format!("add {}", rec - st.lands))
        } else if st.lands > rec + 1 {
            s.warn(&format!("{} more than suggested", st.lands - rec))
        } else {
            s.good("good")
        };
        println!(
            "Lands: {} (recommended ~{} for this curve) - {}",
            st.lands, rec, land_note
        );
        let removal = *st.roles.get(&Role::Removal).unwrap_or(&0);
        if removal < 5 {
            println!(
                "{}",
                s.warn(&format!(
                    "Only {} removal spells - most limited decks want 5+.",
                    removal
                ))
            );
        }
        if st.creatures < 14 && st.nonlands > 20 {
            println!(
                "{}",
                s.warn(&format!(
                    "{} creatures is low; aim for ~15-18 in 60 cards.",
                    st.creatures
                ))
            );
        }
    }

    fn check(&self) {
        let problems = self.deck.validate(&self.db, &self.coll, &self.rules);
        let errors = problems
            .iter()
            .filter(|p| p.severity == Severity::Error)
            .count();
        for p in &problems {
            match p.severity {
                Severity::Error => println!("  {} {}", self.style.bad("✗"), p.message),
                Severity::Warning => println!("  {} {}", self.style.warn("!"), p.message),
            }
        }
        if errors == 0 {
            println!("{}", self.style.good("✓ Deck is legal for your rules."));
        } else {
            println!(
                "{}",
                self.style.bad(&format!("{} problem(s) to fix.", errors))
            );
        }
    }

    fn suggest(&self, arg: &str) {
        let n = arg.parse::<usize>().unwrap_or(12);
        if self.deck.commander.is_none() {
            println!("{}", self.style.warn("Tip: pick a commander first so suggestions respect colour identity and synergy."));
        }
        let list = analysis::suggestions(&self.deck, &self.db, &self.coll, &self.rules);
        if list.is_empty() {
            println!("Nothing left in your pool that fits.");
            return;
        }
        let space = self.rules.deck_size as i64 - self.deck.total_count(&self.rules) as i64;
        println!("Top picks ({} slots open):", space);
        for s in list.iter().take(n) {
            println!(
                "  {:>5.1}  {} {:<7} {}",
                s.score,
                self.style.name_padded(s.card, 32),
                compact_cost(&s.card.mana_cost),
                self.style.dim(&s.reasons.join("; "))
            );
        }
    }

    fn cut(&self, arg: &str) {
        let n = arg.parse::<usize>().unwrap_or(8);
        let list = analysis::weakest(&self.deck, &self.db, &self.rules);
        if list.is_empty() {
            println!("No non-land cards in the deck yet.");
            return;
        }
        println!("Weakest cards (cut candidates):");
        for s in list.iter().take(n) {
            println!(
                "  {:>5.1}  {} {:<7} {}",
                s.score,
                self.style.name_padded(s.card, 32),
                compact_cost(&s.card.mana_cost),
                self.style.dim(&s.reasons.join("; "))
            );
        }
    }

    fn auto(&mut self, q: &str) {
        // Optional trailing colours: `auto nick fury wu`.
        let (q, colors) = match q.rsplit_once(char::is_whitespace) {
            Some((name, last))
                if !matches!(self.db.lookup(q), Lookup::Found(_))
                    && last.chars().all(|c| "wubrgWUBRG".contains(c)) =>
            {
                (name.trim(), ColorSet::parse(last))
            }
            _ => (q, None),
        };
        let cmd = if q.is_empty() {
            match self.deck.commander.as_ref().and_then(|c| self.db.get(c)) {
                Some(c) => c.clone(),
                None => {
                    println!("Give a commander: `auto <name> [colours]` (see `commanders`).");
                    return;
                }
            }
        } else {
            match self.resolve(q) {
                Some(c) if c.can_be_commander() => c,
                Some(c) => {
                    println!("{} can't be a commander.", c.name);
                    return;
                }
                None => return,
            }
        };
        if let Some(cols) = colors {
            if !cols.is_subset_of(cmd.identity()) {
                println!(
                    "{} is outside {}'s identity ({}).",
                    cols,
                    cmd.name,
                    cmd.identity()
                );
                return;
            }
        }
        let plays = colors
            .unwrap_or_else(|| analysis::best_colors(&cmd, &self.db, &self.coll, &self.rules));
        if plays != cmd.identity() {
            println!(
                "Playing {} out of {}'s {} identity{}.",
                plays,
                cmd.name,
                cmd.identity(),
                if colors.is_none() {
                    " (strongest colours in your pool; pick others with `auto <name> <colours>`)"
                } else {
                    ""
                }
            );
        }
        self.snapshot();
        let (mut deck, shortfall) =
            analysis::autobuild(&cmd, &self.db, &self.coll, &self.rules, Some(plays), None);
        deck.name = self.deck.name.clone();
        self.deck = deck;
        println!(
            "Built a deck around {}. `undo` to go back.\n",
            self.style.name(&cmd)
        );
        self.view();
        println!();
        self.check();
        if shortfall > 0 {
            println!(
                "{}",
                self.style.warn(&format!(
                    "Your pool ran out of {} cards: {} spell slots were filled with extra basic lands.\n\
                     Add more cards (`own`), relax a rule (`rules`), or try a deeper commander (`commanders`).",
                    cmd.identity(),
                    shortfall
                ))
            );
        }
    }

    fn lands(&self) {
        let st = analysis::stats(&self.deck, &self.db);
        let rec = analysis::recommended_lands(st.avg_mv, &self.rules);
        let nonbasic: u32 = analysis::resolved(&self.deck, &self.db)
            .iter()
            .filter(|(c, _)| c.is_land() && !c.is_basic())
            .map(|(_, n)| n)
            .sum();
        println!(
            "Average mana value {:.2} → about {} lands in a {}-card library.",
            st.avg_mv,
            rec,
            self.deck.main_target(&self.rules)
        );
        let basics_needed = rec.saturating_sub(nonbasic);
        let split = analysis::basic_split(&self.deck, &self.db, basics_needed);
        let parts: Vec<String> = (0..5)
            .filter(|&i| split[i] > 0)
            .map(|i| format!("{} {}", split[i], BASIC_FOR_COLOR[i]))
            .collect();
        println!(
            "You have {} nonbasic lands, so run {} basics: {}",
            nonbasic,
            basics_needed,
            parts.join(", ")
        );
        let owned_lands: Vec<&Card> = self
            .db
            .cards
            .iter()
            .filter(|c| c.is_land() && !c.is_basic() && self.coll.owned(&c.name) > 0)
            .collect();
        if !owned_lands.is_empty() {
            println!("Nonbasic lands you own:");
            for c in owned_lands {
                let fits = self
                    .deck
                    .identity(&self.db)
                    .map(|id| c.identity().is_subset_of(id))
                    .unwrap_or(true);
                println!(
                    "  {} makes {} {}",
                    self.style.name_padded(c, 22),
                    c.produces(),
                    if fits {
                        String::new()
                    } else {
                        self.style.warn("(off-identity)")
                    }
                );
            }
        }
    }

    fn fill(&mut self) {
        if self.deck.commander.is_none() {
            println!("Pick a commander first so I know which basics to use.");
            return;
        }
        let added = analysis::fill_basics(&mut self.deck, &self.db, &self.rules);
        let parts: Vec<String> = (0..5)
            .filter(|&i| added[i] > 0)
            .map(|i| format!("{} {}", added[i], BASIC_FOR_COLOR[i]))
            .collect();
        if parts.is_empty() {
            println!("Deck is already full.");
        } else {
            println!("Added {}.", parts.join(", "));
        }
    }

    fn hand(&self) {
        let mut lib = analysis::library(&self.deck, &self.db);
        if lib.len() < 10 {
            println!("Add more cards first.");
            return;
        }
        let mut rng = analysis::Rng::seeded();
        rng.shuffle(&mut lib);
        let lands = lib.iter().take(7).filter(|c| c.is_land()).count();
        println!("Opening hand ({} lands):", lands);
        for c in lib.iter().take(7) {
            println!(
                "  {} {}",
                self.style.name_padded(c, 32),
                compact_cost(&c.mana_cost)
            );
        }
        println!("Next draws:");
        for c in lib.iter().skip(7).take(3) {
            println!(
                "  {} {}",
                self.style.name_padded(c, 32),
                compact_cost(&c.mana_cost)
            );
        }
        let verdict = match lands {
            0 | 1 => self.style.bad("Mulligan - too few lands."),
            6 | 7 => self.style.bad("Mulligan - flood."),
            2 => self.style.warn("Keepable if you have cheap spells."),
            _ => self.style.good("Keep."),
        };
        println!("{}", verdict);
    }

    fn sim(&self, arg: &str) {
        let trials = arg.parse::<u32>().unwrap_or(20000);
        if analysis::library(&self.deck, &self.db).len() < 10 {
            println!("Add more cards first.");
            return;
        }
        let r = analysis::simulate(&self.deck, &self.db, trials);
        println!("{} simulated opening hands:", r.trials);
        for (k, n) in r.land_hist.iter().enumerate() {
            if *n > 0 {
                let p = *n as f32 / r.trials as f32;
                println!(
                    "  {} lands: {} {}",
                    k,
                    pct(p),
                    bar((p * 40.0).round() as u32, 1)
                );
            }
        }
        println!("  Keepable (2-5 lands): {}", pct(r.keepable));
        println!("Hitting land drops on the play:");
        for k in 0..6 {
            println!(
                "  {} lands by turn {}: {}",
                k + 1,
                k + 1,
                pct(r.land_drops[k])
            );
        }
        if let Some((mv, p)) = r.commander_on_curve {
            println!(
                "Commander castable on turn {} (lands + coloured sources): {}",
                mv,
                pct(p)
            );
        }
    }

    fn new_deck(&mut self, name: &str) {
        self.snapshot();
        self.deck = Deck::new(if name.is_empty() { "current" } else { name });
        println!("Started a new empty deck. `undo` brings the old one back.");
    }

    fn save(&mut self, name: &str) {
        let name = if name.is_empty() {
            self.deck.name.clone()
        } else {
            name.to_string()
        };
        let path = self.decks_dir().join(format!("{}.txt", name));
        match self.deck.save(&path) {
            Ok(()) => {
                self.deck.name = name;
                println!("Saved to {}", path.display());
            }
            Err(e) => println!("{}", e),
        }
    }

    fn load(&mut self, name: &str) {
        if name.is_empty() {
            self.list_decks();
            return;
        }
        let path = if name.ends_with(".txt") {
            PathBuf::from(name)
        } else {
            self.decks_dir().join(format!("{}.txt", name))
        };
        match Deck::load(&path, &self.db) {
            Ok(d) => {
                self.snapshot();
                self.deck = d;
                println!(
                    "Loaded {} ({} cards).",
                    path.display(),
                    self.deck.total_count(&self.rules)
                );
            }
            Err(e) => println!("{}", e),
        }
    }

    fn list_decks(&self) {
        let Ok(entries) = fs::read_dir(self.decks_dir()) else {
            println!("No saved decks yet.");
            return;
        };
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".txt") && !n.starts_with('_'))
            .map(|n| n.trim_end_matches(".txt").to_string())
            .collect();
        names.sort();
        if names.is_empty() {
            println!("No saved decks yet (`save <name>`).");
        } else {
            println!("Saved decks: {}", names.join(", "));
        }
    }

    fn own(&mut self, arg: &str, sign: i64) {
        let (n, q) = Self::count_and_name(arg);
        if q.is_empty() {
            println!("Usage: own [n] <card>");
            return;
        }
        let name = match self.db.lookup(&q) {
            Lookup::Found(c) => c.name.clone(),
            _ if sign < 0 => {
                println!("No card called '{}'.", q);
                return;
            }
            _ => {
                println!(
                    "'{}' isn't in the card database. Adding it to your collection anyway -\n\
                     run `newcard` to enter its details so the tool can use it.",
                    q
                );
                q.clone()
            }
        };
        match self.coll.adjust(&name, sign * n as i64) {
            Ok(now) => println!("You now own {}x {}.", now, name),
            Err(e) => println!("{}", e),
        }
    }

    fn new_card(&mut self, ask: Ask) {
        println!("Enter the card's details (blank to skip a field, blank name to cancel).");
        let Some(name) = ask("Name: ").filter(|s| !s.trim().is_empty()) else {
            return;
        };
        let name = name.trim().to_string();
        if self.db.get(&name).is_some() {
            println!("{} is already in the database.", name);
            return;
        }
        let cost = ask("Mana cost (e.g. {2}{R}{R} or 2RR): ").unwrap_or_default();
        let cost = normalize_cost(&cost);
        let type_line =
            ask("Type line (e.g. Legendary Creature — Human Hero): ").unwrap_or_default();
        println!("Rules text - use ' / ' between abilities:");
        let text = ask("Text: ").unwrap_or_default().replace(" / ", "\n");
        let pt = ask("Power/toughness (e.g. 3/2, blank if none): ").unwrap_or_default();
        let rarity = ask("Rarity (common/uncommon/rare/mythic): ").unwrap_or_default();
        let (power, toughness) = match pt.trim().split_once('/') {
            Some((p, t)) => (Some(p.trim().to_string()), Some(t.trim().to_string())),
            None => (None, None),
        };
        let mut card = Card {
            name: name.clone(),
            cmc: crate::card::symbols_in(&cost)
                .iter()
                .map(|s| {
                    if s == "X" {
                        0.0
                    } else {
                        s.parse::<f32>().unwrap_or(1.0)
                    }
                })
                .sum(),
            mana_cost: cost,
            type_line: type_line.trim().to_string(),
            oracle_text: text.trim().to_string(),
            power,
            toughness,
            rarity: rarity.trim().to_lowercase(),
            set: "MSH".into(),
            confidence: "verified".into(),
            note: "Entered by hand".into(),
            ..Default::default()
        };
        card.color_identity = card
            .identity()
            .indices()
            .map(|i| color_char(i).to_string())
            .collect();
        self.db.cards.push(card.clone());
        self.db.reindex();
        match self.db.save(&self.data_dir.join("cards.json")) {
            Ok(()) => println!("Saved {} to data/cards.json.", name),
            Err(e) => println!("{}", e),
        }
        self.names_changed = true;
        if self.coll.owned(&name) == 0 {
            if let Some(n) = ask("How many do you own? [1]: ") {
                let n = n.trim().parse::<i64>().unwrap_or(1);
                let _ = self.coll.adjust(&name, n);
            }
        }
        self.show(&name);
    }

    fn rules_cmd(&mut self, arg: &str) {
        if let Some((k, v)) = arg.split_once(char::is_whitespace) {
            match self.rules.set(k.trim(), v.trim()) {
                Ok(()) => {
                    let _ = self.rules.save(&self.data_dir.join("rules.json"));
                    println!("Updated.");
                }
                Err(e) => {
                    println!("{}", e);
                    return;
                }
            }
        }
        let r = &self.rules;
        println!(
            "{}",
            self.style
                .bold("Format rules (change with `rules <key> <value>`)")
        );
        println!("  deck_size                    {}", r.deck_size);
        println!(
            "  commander_counts_toward_size {}",
            r.commander_counts_toward_size
        );
        println!(
            "  max_copies                   {} {}",
            r.max_copies,
            self.style.dim("(0 = only limited by what you own)")
        );
        println!("  unlimited_basics             {}", r.unlimited_basics);
        println!(
            "  enforce_color_identity       {}",
            r.enforce_color_identity
        );
        println!("  enforce_collection           {}", r.enforce_collection);
        println!("  starting_life                {}", r.starting_life);
        println!("  singleton                    {}", r.singleton);
        println!(
            "  piper_free                   {} {}",
            r.piper_free,
            self.style
                .dim("(The Prismatic Piper needn't be in your pool)")
        );
        match league::session(r.session) {
            Some(s) => println!(
                "{}",
                self.style.dim(&format!(
                    "These are the league rules for Session {} ({}).",
                    s.number,
                    s.date_label()
                ))
            ),
            None => println!(
                "{}",
                self.style
                    .dim("Custom rules. `season <n>` resets to a league session.")
            ),
        }
    }

    fn season(&mut self, arg: &str) {
        let next = league::upcoming_session().number;
        if let Ok(n) = arg.trim().parse::<u32>() {
            match league::apply_session(&mut self.rules, n) {
                Ok(s) => {
                    let _ = self.rules.save(&self.data_dir.join("rules.json"));
                    let (size, single) = s.deck_rules();
                    println!(
                        "Rules set for Session {} ({}): {} cards including commander(s), {}.",
                        s.number,
                        s.date_label(),
                        size,
                        if single { "singleton" } else { "no singleton" }
                    );
                    let dupes = league::singleton_conflicts(&self.deck, &self.db);
                    if single && !dupes.is_empty() {
                        let list: Vec<String> =
                            dupes.iter().map(|(n, c)| format!("{}x {}", c, n)).collect();
                        println!(
                            "{} {}",
                            self.style.warn("Singleton: cut extra copies of"),
                            list.join(", ")
                        );
                    }
                }
                Err(e) => println!("{}", e),
            }
            return;
        }
        println!("{}", self.style.bold("Season 2 schedule"));
        for s in &league::SESSIONS {
            let (size, single) = s.deck_rules();
            let marker = if s.number == self.rules.session {
                self.style.good(" ◀ rules in use")
            } else if s.number == next {
                self.style.warn(" ◀ next")
            } else {
                String::new()
            };
            println!(
                "  {:>2}  {:<13} {:>2} cards  {:<13} {}{}",
                s.number,
                s.date_label(),
                size,
                if single { "singleton" } else { "no singleton" },
                s.label,
                marker
            );
        }
        println!("Use `season <n>` to build for a session.");
    }

    fn partner(&mut self, q: &str) {
        let q = q.trim();
        if q.is_empty() || q.eq_ignore_ascii_case("none") {
            self.deck.partner = None;
            self.deck.piper_color = None;
            println!("Partner removed.");
            return;
        }
        let Some(c) = self.resolve(q) else { return };
        if c.name == crate::deck::PIPER {
            let mono = self
                .deck
                .commander
                .as_ref()
                .and_then(|n| self.db.get(n))
                .map(|m| m.identity().count() == 1);
            if mono == Some(false) {
                println!(
                    "The Prismatic Piper can only join a mono-colour commander (league rule)."
                );
                return;
            }
        } else if !c.oracle_text.contains("Partner") {
            println!(
                "{} doesn't have Partner. Only Partner cards or The Prismatic Piper can be a second commander.",
                c.name
            );
            return;
        }
        self.deck.remove(&c.name, 1);
        self.deck.partner = Some(c.name.clone());
        println!("Second commander: {}", self.style.name(&c));
        if c.name == crate::deck::PIPER && self.deck.piper_color.is_none() {
            println!("Now choose its colour with `piper <colour>` (e.g. `piper g`).");
        }
    }

    fn piper(&mut self, arg: &str) {
        let Some(col) = ColorSet::parse(arg).filter(|c| c.count() == 1) else {
            println!("Give one colour: w, u, b, r or g.");
            return;
        };
        if let Some(id) = self
            .deck
            .commander
            .as_ref()
            .and_then(|n| self.db.get(n))
            .map(|m| m.identity())
        {
            if id.count() != 1 {
                println!(
                    "The Prismatic Piper can only join a mono-colour commander (league rule)."
                );
                return;
            }
            if col.is_subset_of(id) {
                println!("Your commander is already {}; pick a different colour.", id);
                return;
            }
        }
        self.deck.partner = Some(crate::deck::PIPER.to_string());
        self.deck.piper_color = col.to_string().chars().next();
        println!(
            "The Prismatic Piper adds {}. Deck identity: {}",
            color_name(col.indices().next().unwrap()),
            self.deck
                .identity(&self.db)
                .map(|i| i.to_string())
                .unwrap_or_default()
        );
    }

    fn draft(&self, arg: &str) {
        let names: Vec<&str> = arg
            .split([';', '\n'])
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if names.is_empty() {
            println!(
                "List the pack's cards separated by semicolons, e.g. `draft red hulk; web up; truck toss`."
            );
            return;
        }
        let mut pack: Vec<&Card> = Vec::new();
        for n in names {
            match self.db.lookup(n) {
                Lookup::Found(c) => pack.push(c),
                _ => {
                    self.resolve(n);
                }
            }
        }
        let options: Vec<Deck> = if self.deck.commander.is_some() {
            Vec::new()
        } else {
            crate::ranking::best_per_commander(&self.rankings().now)
                .iter()
                .take(3)
                .map(|g| g[0].deck.clone())
                .collect()
        };
        let picks = league::rank_pack(
            &pack,
            &self.deck,
            &self.db,
            &self.coll,
            &self.rules,
            &options,
        );
        println!(
            "{}",
            self.style
                .dim("  pick  score   fit  VP  card                              why")
        );
        for (i, p) in picks.iter().enumerate() {
            println!(
                "  {:>4} {:>6.1} {:>5.1} {:>3}  {} {}",
                i + 1,
                p.total,
                p.fit,
                p.vp,
                self.style.name_padded(p.card, 33),
                self.style.dim(&p.reasons.join("; "))
            );
        }
    }

    fn trade(&self, arg: &str) {
        let mut target = 10;
        let mut need = None;
        for tok in arg.split_whitespace() {
            match tok.to_lowercase().as_str() {
                "rare" => need = Some("rare"),
                "mythic" => need = Some("mythic"),
                t => {
                    if let Ok(n) = t.parse() {
                        target = n
                    }
                }
            }
        }
        let plan =
            league::plan_trade_in(&self.deck, &self.db, &self.coll, &self.rules, target, need);
        let total: u32 = plan.iter().map(|t| t.vp).sum();
        println!(
            "Suggested trade-ins ({} VP; the round minimum is 10):",
            total
        );
        for t in &plan {
            println!(
                "  {}x {} {:>2} VP  {}",
                t.copies,
                self.style.name_padded(t.card, 33),
                t.vp,
                self.style.dim(&format!("{} ({})", t.why, t.card.rarity))
            );
        }
        if total < target {
            println!(
                "{}",
                self.style
                    .warn("Not enough spare cards to reach that - the shortfall becomes debt.")
            );
        }
        println!(
            "{}",
            self.style.dim(
                "Only spare copies are suggested; cards in your current deck are kept. Claims cost 2x VP."
            )
        );
    }

    fn vp(&self) {
        let mut rows: Vec<(&Card, u32)> = self
            .coll
            .counts
            .iter()
            .filter_map(|(n, c)| self.db.get(n).map(|x| (x, *c)))
            .filter(|(c, _)| league::vp_value(c) > 0)
            .collect();
        rows.sort_by(|a, b| {
            league::vp_value(b.0)
                .cmp(&league::vp_value(a.0))
                .then(a.0.name.cmp(&b.0.name))
        });
        let total: u32 = rows.iter().map(|(c, n)| league::vp_value(c) * n).sum();
        println!(
            "Your pool's trade-in value: {} VP (Mythic 8, Rare 4, Uncommon 2, Common 1).",
            total
        );
        for (c, n) in rows.iter().filter(|(c, _)| league::vp_value(c) >= 4) {
            println!(
                "  {}x {} {} VP each",
                n,
                self.style.name_padded(c, 33),
                league::vp_value(c)
            );
        }
        println!(
            "Skill Tree staples cost 5, 10, 15 ... 45 VP in the order you claim them (225 for all 9)."
        );
        println!("Shop items: 30 VP each, one purchase per session. Each win earns 5 VP.");
    }

    fn export(&self, arg: &str) {
        match arg.trim().to_lowercase().as_str() {
            "pool" | "collection" => print!("{}", league::export_pool(&self.db, &self.coll)),
            "mixer" | "manamixer" => print!("{}", league::export_deck(&self.deck, &self.db)),
            _ => print!("{}", self.deck.to_text()),
        }
    }

    fn undo(&mut self) {
        match self.history.pop() {
            Some(d) => {
                self.deck = d;
                self.autosave();
                println!(
                    "Undone. Deck: {}/{}",
                    self.deck.total_count(&self.rules),
                    self.rules.deck_size
                );
            }
            None => println!("Nothing to undo."),
        }
    }

    fn clear(&mut self) {
        self.deck.cards.clear();
        println!("Main deck emptied (commander kept). `undo` to restore.");
    }
}

/// Accepts "{2}{R}{R}", "2RR" or "2 r r" and returns "{2}{R}{R}".
pub fn normalize_cost(input: &str) -> String {
    let s = input.trim();
    if s.contains('{') {
        return s.to_uppercase();
    }
    let mut out = String::new();
    let mut num = String::new();
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            num.push(ch);
            continue;
        }
        if !num.is_empty() {
            out.push_str(&format!("{{{}}}", num));
            num.clear();
        }
        if ch.is_alphabetic() {
            out.push_str(&format!("{{{}}}", ch.to_ascii_uppercase()));
        }
    }
    if !num.is_empty() {
        out.push_str(&format!("{{{}}}", num));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_normalization() {
        assert_eq!(normalize_cost("2RR"), "{2}{R}{R}");
        assert_eq!(normalize_cost("{x}{r}"), "{X}{R}");
        assert_eq!(normalize_cost("10g"), "{10}{G}");
    }

    #[test]
    fn count_and_name_forms() {
        assert_eq!(App::count_and_name("2 Web Up"), (2, "Web Up".into()));
        assert_eq!(App::count_and_name("Web Up x3"), (3, "Web Up".into()));
        assert_eq!(App::count_and_name("Web Up"), (1, "Web Up".into()));
        assert_eq!(App::count_and_name("3x Web Up"), (3, "Web Up".into()));
    }
}
