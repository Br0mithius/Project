//! The windowed app: a deck builder with rendered cards, plus league tools
//! (commanders, draft helper, trades & VP, stats, season rules, collection).

use eframe::egui;
mod cards;
mod images;
mod opponents;

use crate::analysis::{self, SimResult, curve_label};
use crate::card::{BASIC_FOR_COLOR, COLORS, Card, ColorSet, Role};
use crate::db::{CardDb, Collection, Lookup, Rules};
use crate::deck::{Deck, PIPER, Severity};
use crate::league;
use crate::paths;
use cards::{CardLook, card_size, card_widget, cost_widget, identity_dots, paint_card};
use egui::{
    Align, Color32, CornerRadius, FontId, Layout, Panel, RichText, ScrollArea, Sense, Stroke,
    TextureHandle, Ui, Vec2, pos2, vec2,
};
use images::ImageCache;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub fn run(data_dir: PathBuf) -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("MSH Deckbuilder")
            .with_inner_size([1500.0, 920.0])
            .with_min_inner_size([1000.0, 640.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "MSH Deckbuilder",
        options,
        Box::new(move |cc| Ok(Box::new(GuiApp::new(cc, data_dir)))),
    )
    .map_err(|e| e.to_string())
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Build,
    Lab,
    Commanders,
    Draft,
    Trade,
    Upgrades,
    Stats,
    Season,
    Collection,
}

const TABS: [(Tab, &str); 9] = [
    (Tab::Build, "Deck Builder"),
    (Tab::Lab, "Playtest Lab"),
    (Tab::Commanders, "Commanders"),
    (Tab::Draft, "Draft Helper"),
    (Tab::Trade, "Trades & VP"),
    (Tab::Upgrades, "Upgrades"),
    (Tab::Stats, "Stats & Playtest"),
    (Tab::Season, "Season & Rules"),
    (Tab::Collection, "My Pool"),
];

#[derive(Clone, Copy, PartialEq)]
enum SortBy {
    Type,
    ManaValue,
    Name,
    Score,
}

const TYPE_FILTERS: [&str; 8] = [
    "All types",
    "Creature",
    "Instant",
    "Sorcery",
    "Artifact",
    "Enchantment",
    "Land",
    "Legendary",
];

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    real_images: bool,
    card_width: f32,
    visual_deck: bool,
    /// Show a big, readable copy of a card while the mouse rests on it.
    hover_zoom: bool,
    zoom_width: f32,
    /// How much each part of the commander ranking counts.
    weights: crate::ranking::Weights,
    /// Playtest lab run started when a commander is chosen or a deck auto-built.
    lab_auto: LabAuto,
}

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
enum LabAuto {
    Off,
    Quick,
    Thorough,
}

impl LabAuto {
    fn effort(self) -> Option<crate::lab::Effort> {
        match self {
            LabAuto::Off => None,
            LabAuto::Quick => Some(crate::lab::Effort::QUICK),
            LabAuto::Thorough => Some(crate::lab::Effort::THOROUGH),
        }
    }

    fn label(self) -> &'static str {
        match self {
            LabAuto::Off => "nothing (run it by hand)",
            LabAuto::Quick => "a quick lab (about 1 minute)",
            LabAuto::Thorough => "a thorough lab (about 5 minutes)",
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            real_images: true,
            card_width: 140.0,
            visual_deck: true,
            hover_zoom: true,
            zoom_width: 400.0,
            weights: crate::ranking::Weights::default(),
            lab_auto: LabAuto::Thorough,
        }
    }
}

/// Things the UI asks for while drawing; applied after the frame's widgets are laid out.
enum Action {
    Select(String),
    Add(String, u32),
    Remove(String, u32),
    SetCommander(String),
    SetPartner(Option<String>),
    PiperColor(char),
    AutoBuild(String, Option<char>),
    Fill,
    ClearDeck,
    Undo,
    NewDeck,
    Save,
    Load(String),
    Session(u32),
    Own(String, i64),
    DraftAdd(String),
    DraftRemove(usize),
    DraftTook(String),
    Copy(String, &'static str),
    Simulate,
    NewHand,
    Draw,
    RulesChanged,
    /// Mana Mixer import: (list text, replace the whole pool?).
    Import(String, bool),
    /// Replace the deck (a playtest lab suggestion), with a status message.
    ApplyDeck(Deck, String),
    RunLab(crate::lab::Effort),
    StopLab,
}

/// A playtest lab running on a background thread.
struct LabJob {
    progress: std::sync::Arc<crate::lab::Progress>,
    rx: std::sync::mpsc::Receiver<Option<crate::lab::LabReport>>,
    commander: String,
    started: std::time::Instant,
}

/// A commander ranking being built on a background thread.
struct RankJob {
    done: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    total: usize,
    rx: std::sync::mpsc::Receiver<crate::ranking::Rankings>,
    key: String,
}

/// Results of the slower calculations, kept until the deck, pool or rules change
/// (`GuiApp::rev`), so redrawing the window stays cheap.
#[derive(Default)]
struct Memo {
    rev: u64,
    errors: Option<Vec<String>>,
    scores: Option<std::collections::HashMap<String, f32>>,
    suggestions: Option<Vec<(Card, f32, Vec<String>)>>,
    draft: Option<(Vec<String>, Vec<(Card, f32, f32, u32, Vec<String>)>)>,
    trade: Option<((u32, usize), Vec<(Card, u32, u32, String)>)>,
    weakest: Option<Vec<(String, f32)>>,
    upgrades: Option<Vec<crate::upgrades::Upgrade>>,
}

pub struct GuiApp {
    data_dir: PathBuf,
    db: CardDb,
    coll: Collection,
    rules: Rules,
    deck: Deck,
    history: Vec<Deck>,
    settings: Settings,
    images: ImageCache,
    tab: Tab,
    selected: Option<String>,
    status: String,
    // pool filters
    search: String,
    colors: [bool; 6],
    type_filter: usize,
    role_filter: Option<Role>,
    mv_max: u32,
    fits_only: bool,
    avail_only: bool,
    whole_set: bool,
    sort: SortBy,
    // other tabs
    deck_name: String,
    saved_decks: Vec<String>,
    draft_query: String,
    draft_pack: Vec<String>,
    coll_query: String,
    /// Pool browser set filter ("" = all sets).
    set_filter: String,
    import_text: String,
    /// The league's Public Pool (pasted from Mana Mixer), for the upgrade planner.
    public_pool: Collection,
    public_text: String,
    trade_target: u32,
    trade_need: usize,
    sim: Option<SimResult>,
    hand: Vec<String>,
    library: Vec<String>,
    ranking: Option<crate::ranking::Rankings>,
    ranking_key: String,
    rank_job: Option<RankJob>,
    lab_job: Option<LabJob>,
    /// The last playtest lab report.
    lab: Option<crate::lab::LabReport>,
    data_job: Option<std::sync::mpsc::Receiver<String>>,
    /// Next background update also re-checks Scryfall for new league cards.
    force_card_update: bool,
    error: Option<String>,
    /// Bumped whenever the deck, pool or rules change; invalidates `memo`.
    rev: u64,
    memo: Memo,
}

/// "45 s", "3 min".
fn fmt_secs(secs: f32) -> String {
    if secs < 90.0 {
        format!("{:.0} s", secs.max(1.0))
    } else {
        format!("{:.0} min", secs / 60.0)
    }
}

fn settings_path(dir: &std::path::Path) -> PathBuf {
    dir.join("settings.json")
}

/// Earlier versions saved images as data/images/msh-149.jpg; move them to the new
/// images folder under readable names so nothing is downloaded twice.
fn migrate_old_images(data_dir: &std::path::Path, images_dir: &std::path::Path, db: &CardDb) {
    let old = data_dir.join("images");
    if !old.is_dir() || old == images_dir {
        return;
    }
    let _ = std::fs::create_dir_all(images_dir);
    for c in &db.cards {
        let set = if c.set.is_empty() {
            "msh".to_string()
        } else {
            c.set.to_lowercase()
        };
        if let Some(n) = &c.collector_number {
            let from = old.join(format!("{}-{}.jpg", set, n));
            let to = images_dir.join(images::file_name(c));
            if from.exists() && !to.exists() {
                let _ = std::fs::rename(&from, &to);
            }
        }
    }
    let _ = std::fs::remove_dir(&old); // only succeeds if now empty
}

impl GuiApp {
    fn new(cc: &eframe::CreationContext<'_>, data_dir: PathBuf) -> GuiApp {
        let ctx = &cc.egui_ctx;
        let public_pool = crate::upgrades::load_public_pool(&data_dir);
        // Always dark: the card colours and text are designed for it, whatever the OS theme.
        ctx.set_theme(egui::ThemePreference::Dark);
        ctx.global_style_mut(|s| {
            s.spacing.item_spacing = vec2(8.0, 6.0);
            s.spacing.button_padding = vec2(8.0, 4.0);
            // Hover zoom: appear quickly once the mouse rests, and allow a big card.
            s.interaction.tooltip_delay = 0.25;
            s.interaction.show_tooltips_only_when_still = true;
            s.spacing.tooltip_width = 720.0;
            for (style, id) in s.text_styles.iter_mut() {
                id.size = match style {
                    egui::TextStyle::Heading => 20.0,
                    egui::TextStyle::Small => 11.0,
                    _ => 14.0,
                };
            }
        });
        let mut settings: Settings = std::fs::read_to_string(settings_path(&data_dir))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        settings.weights.replace_old_defaults();
        let mut error = None;
        let mut db = CardDb::load(&data_dir.join("cards.json")).unwrap_or_else(|e| {
            error = Some(e);
            CardDb::new(Vec::new())
        });
        db.attach_ratings(&data_dir);
        let coll = Collection::load(&data_dir.join("collection.txt")).unwrap_or_else(|e| {
            error = Some(e);
            Collection::new(data_dir.join("collection.txt"), Default::default())
        });
        let rules_path = data_dir.join("rules.json");
        let mut rules = if rules_path.exists() {
            Rules::load(&rules_path)
        } else {
            let mut r = Rules::default();
            let _ = league::apply_session(&mut r, league::upcoming_session().number);
            r
        };
        rules.data_dir = Some(data_dir.clone());
        let current = paths::decks_dir(&data_dir).join("_current.txt");
        let deck = Deck::load(&current, &db).unwrap_or_else(|_| Deck::new("my deck"));
        let deck_name = if deck.name == "_current" {
            "my deck".into()
        } else {
            deck.name.clone()
        };
        let images_dir = paths::images_dir(&data_dir);
        migrate_old_images(&data_dir, &images_dir, &db);
        let images = ImageCache::new(&images_dir, ctx, settings.real_images);
        let mut app = GuiApp {
            data_dir,
            db,
            coll,
            rules,
            deck,
            history: Vec::new(),
            settings,
            images,
            tab: Tab::Build,
            selected: None,
            status:
                "Welcome! Pick a commander on the Commanders tab, or click any card to inspect it."
                    .into(),
            search: String::new(),
            colors: [false; 6],
            type_filter: 0,
            role_filter: None,
            mv_max: 7,
            fits_only: true,
            avail_only: false,
            whole_set: false,
            sort: SortBy::Type,
            deck_name,
            saved_decks: Vec::new(),
            draft_query: String::new(),
            draft_pack: Vec::new(),
            coll_query: String::new(),
            set_filter: String::new(),
            import_text: String::new(),
            public_pool,
            public_text: String::new(),
            trade_target: 10,
            trade_need: 0,
            sim: None,
            hand: Vec::new(),
            library: Vec::new(),
            ranking: None,
            ranking_key: String::new(),
            rank_job: None,
            lab_job: None,
            lab: None,
            data_job: None,
            force_card_update: false,
            error,
            rev: 1,
            memo: Memo::default(),
        };
        app.deck.name = app.deck_name.clone();
        app.refresh_saved_decks();
        if app.coll.total() == 0 {
            app.tab = Tab::Collection;
            app.status = "Your pool is empty - drag your Mana Mixer file onto this window or paste it into the import box (Ctrl+V), or add cards one by one.".into();
        }
        // Ratings are downloaded once, on first launch; after that only when you ask
        // (Settings > Update ratings), so the sites aren't hit repeatedly.
        let stamp = app.data_dir.join("ratings_downloaded.txt");
        if !stamp.exists() {
            let _ = std::fs::write(&stamp, crate::ratings::today());
            app.start_data_download();
        }
        if app.settings.real_images {
            // Fetch the images for everything you own up front (cached ones load from disk).
            let owned: Vec<Card> = app
                .coll
                .counts
                .keys()
                .filter_map(|n| app.db.get(n).cloned())
                .collect();
            app.images.prefetch(owned.iter());
        }
        app
    }

    fn save_settings(&self) {
        if let Ok(json) = serde_json::to_string_pretty(&self.settings) {
            let _ = std::fs::write(settings_path(&self.data_dir), json);
        }
    }

    fn refresh_saved_decks(&mut self) {
        let dir = paths::decks_dir(&self.data_dir);
        self.saved_decks = std::fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .filter(|n| n.ends_with(".txt") && !n.starts_with('_'))
                    .map(|n| n.trim_end_matches(".txt").to_string())
                    .collect()
            })
            .unwrap_or_default();
        self.saved_decks.sort();
    }

    fn autosave(&self) {
        let _ = self
            .deck
            .save(&paths::decks_dir(&self.data_dir).join("_current.txt"));
    }

    fn card(&self, name: &str) -> Option<&Card> {
        self.db.get(name)
    }

    fn available(&self, card: &Card) -> u32 {
        analysis::available(card, &self.deck, &self.coll, &self.rules)
    }

    fn fits(&self, card: &Card) -> bool {
        match self.deck.identity(&self.db) {
            Some(id) if self.rules.enforce_color_identity => card.identity().is_subset_of(id),
            _ => true,
        }
    }

    /// Full-resolution image (or the thumbnail while it loads) for big views.
    fn full_texture(&mut self, card: &Card) -> Option<TextureHandle> {
        if self.settings.real_images {
            self.images.full(card)
        } else {
            None
        }
    }

    /// Draws a card: the official image when available (loaded only once it's on
    /// screen), otherwise a drawn card face; plus the hover zoom.
    fn show_card(&mut self, ui: &mut Ui, look: CardLook) -> egui::Response {
        self.show_card_noted(ui, look, None)
    }

    fn show_card_noted(
        &mut self,
        ui: &mut Ui,
        look: CardLook,
        note: Option<String>,
    ) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(card_size(look.width), Sense::click());
        let card = look.card;
        if ui.is_rect_visible(rect) {
            let tex = if self.settings.real_images {
                self.images.thumb(card)
            } else {
                None
            };
            paint_card(
                ui,
                rect,
                &CardLook {
                    image: tex.as_ref(),
                    ..look
                },
                resp.hovered(),
            );
        }
        self.with_zoom(resp, card, note)
    }

    /// While the mouse rests on a card, shows a large readable copy next to the pointer.
    fn with_zoom(
        &mut self,
        resp: egui::Response,
        card: &Card,
        note: Option<String>,
    ) -> egui::Response {
        if !self.settings.hover_zoom {
            return match note {
                Some(n) => resp.on_hover_text(n),
                None => resp,
            };
        }
        if !resp.hovered() {
            return resp;
        }
        let tex = self.full_texture(card);
        let w = self.settings.zoom_width;
        resp.on_hover_ui_at_pointer(|ui| {
            card_widget(
                ui,
                CardLook {
                    card,
                    width: w,
                    image: tex.as_ref(),
                    badge: None,
                    dimmed: false,
                    selected: false,
                },
            );
            if let Some(n) = &note {
                ui.label(RichText::new(n).small());
            }
        })
    }

    // ------------------------------------------------------------------ actions

    fn apply(&mut self, actions: Vec<Action>, ctx: &egui::Context) {
        let changes_data = actions.iter().any(|a| {
            !matches!(
                a,
                Action::Select(_)
                    | Action::Copy(..)
                    | Action::Simulate
                    | Action::NewHand
                    | Action::Draw
                    | Action::DraftAdd(_)
                    | Action::DraftRemove(_)
            )
        });
        if changes_data {
            self.rev += 1;
        }
        let commander_before = self.deck.commander.clone();
        let mut chose_commander = false;
        for a in actions {
            let before = self.deck.clone();
            match a {
                Action::Select(n) => self.selected = Some(n),
                Action::Add(n, k) => self.add(&n, k),
                Action::Remove(n, k) => {
                    let took = self.deck.remove(&n, k);
                    if took > 0 {
                        self.status = format!("Removed {} x {}.", took, n);
                    }
                }
                Action::SetCommander(n) => {
                    self.set_commander(&n);
                    chose_commander |= self.deck.commander != commander_before;
                }
                Action::SetPartner(p) => {
                    match &p {
                        Some(name) if name == PIPER => {
                            let mono = self
                                .deck
                                .commander
                                .as_ref()
                                .and_then(|c| self.db.get(c))
                                .map(|c| c.identity().count() == 1)
                                .unwrap_or(false);
                            if !mono {
                                self.status =
                                    "The Prismatic Piper can only join a mono-colour commander."
                                        .into();
                                continue;
                            }
                        }
                        Some(name) => {
                            let ok = self
                                .db
                                .get(name)
                                .map(|c| c.oracle_text.contains("Partner"))
                                .unwrap_or(false);
                            if !ok {
                                self.status = format!("{} doesn't have Partner.", name);
                                continue;
                            }
                        }
                        None => self.deck.piper_color = None,
                    }
                    if let Some(name) = &p {
                        self.deck.remove(name, 1);
                    }
                    self.deck.partner = p;
                }
                Action::PiperColor(c) => {
                    self.deck.partner = Some(PIPER.into());
                    self.deck.piper_color = Some(c);
                    self.status = format!(
                        "The Prismatic Piper adds {}. Deck colours: {}",
                        analysis::color_name(ColorSet::from_char(c).indices().next().unwrap_or(0)),
                        self.deck
                            .identity(&self.db)
                            .map(|i| i.to_string())
                            .unwrap_or_default()
                    );
                }
                Action::AutoBuild(n, piper) => {
                    if let Some(c) = self.db.get(&n).cloned() {
                        let (mut d, short) =
                            analysis::autobuild(&c, &self.db, &self.coll, &self.rules, None, piper);
                        d.name = self.deck.name.clone();
                        self.deck = d;
                        self.tab = Tab::Build;
                        chose_commander = true;
                        self.status = if short > 0 {
                            format!(
                                "Built a deck around {} - your pool ran {} cards short, so extra basics fill the gap.",
                                n, short
                            )
                        } else {
                            format!(
                                "Built a full deck around {}. Tweak it, or press Undo to go back.",
                                n
                            )
                        };
                    }
                }
                Action::Fill => {
                    if self.deck.commander.is_none() {
                        self.status =
                            "Pick a commander first so I know which basic lands to add.".into();
                    } else {
                        let added = analysis::fill_basics(&mut self.deck, &self.db, &self.rules);
                        let parts: Vec<String> = (0..5)
                            .filter(|&i| added[i] > 0)
                            .map(|i| format!("{} {}", added[i], BASIC_FOR_COLOR[i]))
                            .collect();
                        self.status = if parts.is_empty() {
                            "The deck is already full.".into()
                        } else {
                            format!("Added {}.", parts.join(", "))
                        };
                    }
                }
                Action::ClearDeck => {
                    self.deck.cards.clear();
                    self.status = "Main deck emptied (commander kept). Undo brings it back.".into();
                }
                Action::Undo => match self.history.pop() {
                    Some(d) => {
                        self.deck = d;
                        self.deck_name = self.deck.name.clone();
                        self.status = "Undone.".into();
                        self.autosave();
                        continue;
                    }
                    None => self.status = "Nothing to undo.".into(),
                },
                Action::NewDeck => {
                    self.deck = Deck::new("new deck");
                    self.deck_name = "new deck".into();
                    self.status = "Started a new deck. Undo brings the old one back.".into();
                }
                Action::Save => {
                    let name: String = self
                        .deck_name
                        .trim()
                        .chars()
                        .map(|c| {
                            if c.is_alphanumeric() || c == ' ' || c == '-' {
                                c
                            } else {
                                '_'
                            }
                        })
                        .collect();
                    let name = if name.is_empty() {
                        "my deck".to_string()
                    } else {
                        name
                    };
                    let path = paths::decks_dir(&self.data_dir).join(format!("{}.txt", name));
                    self.deck.name = name.clone();
                    self.status = match self.deck.save(&path) {
                        Ok(()) => format!("Saved to {}", path.display()),
                        Err(e) => e,
                    };
                    self.refresh_saved_decks();
                }
                Action::Load(n) => {
                    let path = paths::decks_dir(&self.data_dir).join(format!("{}.txt", n));
                    match Deck::load(&path, &self.db) {
                        Ok(d) => {
                            self.deck = d;
                            self.deck_name = n.clone();
                            self.status = format!("Opened {}.", n);
                        }
                        Err(e) => self.status = e,
                    }
                }
                Action::Session(n) => {
                    if let Ok(s) = league::apply_session(&mut self.rules, n) {
                        let _ = self.rules.save(&self.data_dir.join("rules.json"));
                        let (size, single) = s.deck_rules();
                        self.status = format!(
                            "Building for Session {} ({}): {} cards including commander, {}.",
                            n,
                            s.date_label(),
                            size,
                            if single { "singleton" } else { "no singleton" }
                        );
                        self.sim = None;
                    }
                }
                Action::RulesChanged => {
                    self.rules.session = 0;
                    let _ = self.rules.save(&self.data_dir.join("rules.json"));
                }
                Action::Own(n, delta) => match self.coll.adjust(&n, delta) {
                    Ok(now) => {
                        self.status = format!("Your pool now has {} x {}.", now, n);
                    }
                    Err(e) => self.status = e,
                },
                Action::Import(text, replace) => {
                    match league::import_pool(&text, replace, &self.db, &mut self.coll) {
                        Ok(r) => {
                            self.status = format!(
                                "{} {} cards ({} different).{}",
                                if replace { "Your pool is now" } else { "Added" },
                                r.cards,
                                r.distinct,
                                if r.unknown.is_empty() {
                                    String::new()
                                } else {
                                    format!(
                                        " Not in the card database (kept; try Settings > Update card database): {}",
                                        r.unknown.join(", ")
                                    )
                                }
                            );
                            self.import_text.clear();
                        }
                        Err(e) => self.status = e,
                    }
                }
                Action::DraftAdd(n) => {
                    self.draft_pack.push(n);
                    self.draft_query.clear();
                }
                Action::DraftRemove(i) => {
                    if i < self.draft_pack.len() {
                        self.draft_pack.remove(i);
                    }
                }
                Action::DraftTook(n) => {
                    let _ = self.coll.adjust(&n, 1);
                    if let Some(i) = self.draft_pack.iter().position(|x| *x == n) {
                        self.draft_pack.remove(i);
                    }
                    self.status = format!(
                        "Added {} to your pool. Clear the pack when it passes on.",
                        n
                    );
                }
                Action::Copy(text, what) => {
                    ctx.copy_text(text);
                    self.status = format!(
                        "Copied {} to the clipboard - paste it where you need it.",
                        what
                    );
                }
                Action::Simulate => {
                    let r = analysis::simulate(&self.deck, &self.db, 20000);
                    self.status = format!(
                        "Simulated 20,000 opening hands: {:.0}% are keepable.",
                        r.keepable * 100.0
                    );
                    self.sim = Some(r);
                }
                Action::NewHand => {
                    let mut lib: Vec<String> = analysis::library(&self.deck, &self.db)
                        .iter()
                        .map(|c| c.name.clone())
                        .collect();
                    analysis::Rng::seeded().shuffle(&mut lib);
                    self.hand = lib.drain(..7.min(lib.len())).collect();
                    self.library = lib;
                }
                Action::Draw => {
                    if !self.library.is_empty() {
                        let c = self.library.remove(0);
                        self.hand.push(c);
                    }
                }
                Action::ApplyDeck(mut d, msg) => {
                    d.name = self.deck.name.clone();
                    self.deck = d;
                    self.status = msg;
                }
                Action::RunLab(effort) => self.start_lab(effort),
                Action::StopLab => {
                    self.stop_lab();
                    self.status = "Playtest lab stopped.".into();
                }
            }
            if self.deck != before {
                self.history.push(before);
                if self.history.len() > 200 {
                    self.history.remove(0);
                }
                self.autosave();
            }
        }
        // Choosing a commander (or auto-building) starts the playtest lab on that deck.
        if chose_commander {
            match self.settings.lab_auto.effort() {
                Some(e) => self.start_lab(e),
                None => self.stop_lab(),
            }
        }
    }

    fn add(&mut self, name: &str, n: u32) {
        let Some(card) = self.db.get(name).cloned() else {
            return;
        };
        if !self.fits(&card) {
            self.status = format!(
                "{} is outside your commander's colours ({}).",
                name,
                self.deck
                    .identity(&self.db)
                    .map(|i| i.to_string())
                    .unwrap_or_default()
            );
            return;
        }
        let take = n.min(self.available(&card));
        if take == 0 {
            self.status = if self.rules.singleton && self.deck.used(name) > 0 {
                format!("Singleton is on: {} is already in the deck.", name)
            } else {
                format!("No copies of {} left in your pool.", name)
            };
            return;
        }
        self.deck.add(name, take);
        self.status = format!(
            "Added {} x {}. Deck: {}/{}",
            take,
            name,
            self.deck.total_count(&self.rules),
            self.rules.deck_size
        );
    }

    fn set_commander(&mut self, name: &str) {
        let Some(card) = self.db.get(name).cloned() else {
            return;
        };
        if !card.can_be_commander() {
            self.status = format!(
                "{} isn't a legendary creature, so it can't be a commander.",
                name
            );
            return;
        }
        if self.rules.enforce_collection && self.coll.owned(name) == 0 && name != PIPER {
            self.status = format!("You don't have {} in your pool.", name);
            return;
        }
        if self.deck.count_of(name) > 0 && self.coll.owned(name) <= self.deck.count_of(name) {
            self.deck.remove(name, 1);
        }
        self.deck.commander = Some(name.to_string());
        if self.deck.partner.as_deref() == Some(PIPER) && card.identity().count() != 1 {
            self.deck.partner = None;
            self.deck.piper_color = None;
        }
        self.status = format!("{} is your commander (colours {}).", name, card.identity());
    }

    // ------------------------------------------------------------------ chrome

    fn top_bar(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("MSH Deckbuilder").heading().strong().color(Color32::from_rgb(255, 205, 80)));
            ui.separator();
            // Session picker.
            let current = league::session(self.rules.session);
            let label = match current {
                Some(s) => {
                    let (size, single) = s.deck_rules();
                    format!(
                        "Session {} · {} · {} cards{}",
                        s.number,
                        s.date_label(),
                        size,
                        if single { " · singleton" } else { "" }
                    )
                }
                None => format!("Custom rules · {} cards", self.rules.deck_size),
            };
            egui::ComboBox::from_id_salt("session")
                .selected_text(label)
                .width(300.0)
                .show_ui(ui, |ui| {
                    let next = league::upcoming_session().number;
                    for s in &league::SESSIONS {
                        let (size, single) = s.deck_rules();
                        let text = format!(
                            "{:>2}. {} - {} cards{}{}",
                            s.number,
                            s.date_label(),
                            size,
                            if single { ", singleton" } else { "" },
                            if s.number == next { "  (next)" } else { "" }
                        );
                        if ui.selectable_label(self.rules.session == s.number, text).clicked() {
                            acts.push(Action::Session(s.number));
                        }
                    }
                });
            ui.separator();
            ui.label("Deck:");
            ui.add(egui::TextEdit::singleline(&mut self.deck_name).desired_width(140.0));
            if ui.button("Save").on_hover_text("Save to the decks folder").clicked() {
                acts.push(Action::Save);
            }
            ui.menu_button("Open", |ui| {
                if self.saved_decks.is_empty() {
                    ui.label("No saved decks yet.");
                }
                for n in self.saved_decks.clone() {
                    if ui.button(&n).clicked() {
                        acts.push(Action::Load(n));
                        ui.close();
                    }
                }
            });
            if ui.button("New").clicked() {
                acts.push(Action::NewDeck);
            }
            if ui.add_enabled(!self.history.is_empty(), egui::Button::new("Undo")).clicked() {
                acts.push(Action::Undo);
            }
            ui.menu_button("Export", |ui| {
                if ui.button("Copy deck list (Moxfield / Archidekt)").clicked() {
                    acts.push(Action::Copy(self.deck.to_text(), "the deck list"));
                    ui.close();
                }
                if ui.button("Copy deck for Mana Mixer").clicked() {
                    acts.push(Action::Copy(league::export_deck(&self.deck, &self.db), "the Mana Mixer deck list"));
                    ui.close();
                }
                if ui.button("Copy my whole pool for Mana Mixer").clicked() {
                    acts.push(Action::Copy(league::export_pool(&self.db, &self.coll), "your pool list"));
                    ui.close();
                }
            });
            ui.menu_button("Settings", |ui| {
                if ui.button("Import my pool from Mana Mixer...").clicked() {
                    self.tab = Tab::Collection;
                    ui.close();
                }
                ui.separator();
                if ui
                    .checkbox(&mut self.settings.real_images, "Show official card images (full art, from Scryfall)")
                    .changed()
                {
                    self.images.enabled = self.settings.real_images;
                    self.save_settings();
                }
                ui.label(
                    RichText::new(format!(
                        "{} images saved on this PC, {} downloading, {} unavailable. Each image is downloaded once and then always read from the images folder.",
                        self.images.loaded(),
                        self.images.pending(),
                        self.images.failures()
                    ))
                    .small(),
                );
                ui.horizontal(|ui| {
                    ui.label(RichText::new(self.images.folder().display().to_string()).small().weak());
                    if ui.small_button("Open folder").clicked() {
                        let dir = self.images.folder().to_path_buf();
                        let _ = std::fs::create_dir_all(&dir);
                        let opener = if cfg!(windows) {
                            "explorer"
                        } else if cfg!(target_os = "macos") {
                            "open"
                        } else {
                            "xdg-open"
                        };
                        let _ = std::process::Command::new(opener).arg(&dir).spawn();
                    }
                });
                if let Some(e) = &self.images.last_error {
                    ui.label(RichText::new(format!("Last download problem: {}", e)).small().color(Color32::from_rgb(255, 170, 120)));
                }
                if ui.button("Download images for my whole pool").clicked() {
                    let names: Vec<String> = self.coll.counts.keys().cloned().collect();
                    let cards: Vec<Card> = names.iter().filter_map(|n| self.db.get(n).cloned()).collect();
                    let n = self.images.prefetch(cards.iter());
                    self.status = format!("Downloading {} card images in the background.", n);
                    ui.close();
                }
                if ui.button("Download images for the whole set (for drafting)").clicked() {
                    let cards: Vec<Card> = self.db.cards.iter().filter(|c| !c.is_basic()).cloned().collect();
                    let n = self.images.prefetch(cards.iter());
                    self.status = format!("Downloading {} card images in the background (about a minute).", n);
                    ui.close();
                }
                if self.images.failures() > 0 && ui.button("Retry images that failed").clicked() {
                    self.images.retry_failed();
                }
                ui.separator();
                ui.label(RichText::new("Rankings data").strong());
                ui.label(
                    RichText::new(if self.db.edhrec.is_empty() {
                        "No EDHREC data yet.".to_string()
                    } else {
                        format!(
                            "EDHREC from {} ({} cards ranked, {} commander pages).",
                            self.db.edhrec.fetched,
                            self.db.edhrec.ranked_cards(),
                            self.db.edhrec.commander_count()
                        )
                    })
                    .small(),
                );
                if !self.db.tags_date.is_empty() {
                    ui.label(RichText::new(format!("Scryfall Tagger tags from {}.", self.db.tags_date)).small());
                }
                ui.label(
                    RichText::new("Downloaded once and saved; re-downloaded only when you press a button below.")
                        .small()
                        .weak(),
                );
                if ui.add_enabled(self.data_job.is_none(), egui::Button::new("Update ratings now")).clicked() {
                    self.start_data_download();
                    ui.close();
                }
                if ui
                    .add_enabled(self.data_job.is_none(), egui::Button::new("Update card database (all league sets)"))
                    .on_hover_text("Adds new cards from Marvel Super Heroes, The Hobbit, Reality Fracture, Nauctis and Foundations via Scryfall")
                    .clicked()
                {
                    self.force_card_update = true;
                    self.start_data_download();
                    ui.close();
                }
                ui.separator();
                if ui
                    .checkbox(&mut self.settings.hover_zoom, "Zoom in on a card when the mouse rests on it")
                    .changed()
                {
                    self.save_settings();
                }
                ui.add_enabled_ui(self.settings.hover_zoom, |ui| {
                    ui.label("Zoomed card size");
                    if ui.add(egui::Slider::new(&mut self.settings.zoom_width, 260.0..=620.0)).changed() {
                        self.save_settings();
                    }
                });
                ui.separator();
                ui.label("Card size in lists");
                if ui.add(egui::Slider::new(&mut self.settings.card_width, 100.0..=240.0)).changed() {
                    self.save_settings();
                }
            });
        });
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            for (t, label) in TABS {
                let sel = self.tab == t;
                let text = RichText::new(label).size(15.0);
                let text = if sel {
                    text.strong().color(Color32::from_rgb(255, 205, 80))
                } else {
                    text
                };
                if ui.selectable_label(sel, text).clicked() {
                    self.tab = t;
                }
            }
        });
    }

    fn deck_summary(&mut self, ui: &mut Ui) {
        let total = self.deck.total_count(&self.rules);
        let size = self.rules.deck_size;
        let errors: Vec<String> = self
            .memo
            .errors
            .get_or_insert_with(|| {
                self.deck
                    .validate(&self.db, &self.coll, &self.rules)
                    .into_iter()
                    .filter(|p| p.severity == Severity::Error)
                    .map(|p| p.message)
                    .collect()
            })
            .clone();
        ui.horizontal(|ui| {
            let frac = (total as f32 / size.max(1) as f32).min(1.0);
            ui.add(
                egui::ProgressBar::new(frac)
                    .desired_width(150.0)
                    .text(format!("{}/{} cards", total, size)),
            );
            if errors.is_empty() {
                ui.label(
                    RichText::new("Legal")
                        .color(Color32::from_rgb(110, 220, 120))
                        .strong(),
                );
            } else {
                ui.label(
                    RichText::new(format!("{} to fix (hover)", errors.len()))
                        .color(Color32::from_rgb(255, 120, 100))
                        .strong(),
                )
                .on_hover_ui(|ui| {
                    for e in &errors {
                        ui.label(e.as_str());
                    }
                });
            }
            // The playtest lab, running or done.
            let lab = if let Some(job) = &self.lab_job {
                Some(format!(
                    "Playtest lab {:.0}%...",
                    job.progress.fraction() * 100.0
                ))
            } else {
                self.lab
                    .as_ref()
                    .filter(|r| self.deck.commander.as_deref() == Some(r.commander.as_str()))
                    .map(|r| match &r.best {
                        Some(b) if b.deck.cards == self.deck.cards => {
                            "Playtest lab: best combination applied".to_string()
                        }
                        Some(b) => format!(
                            "Playtest lab: {} swaps measured {} pts",
                            b.swaps.len(),
                            crate::lab::pts_se(&b.gain)
                        ),
                        None => "Playtest lab: no change beats this build".to_string(),
                    })
            };
            if let Some(text) = lab {
                ui.separator();
                if ui
                    .link(text)
                    .on_hover_text("Open the Playtest Lab tab")
                    .clicked()
                {
                    self.tab = Tab::Lab;
                }
            }
        });
    }

    // ------------------------------------------------------------------ build tab

    fn pool_cards(&self) -> Vec<&Card> {
        let identity = self.deck.identity(&self.db);
        let q = crate::db::normalize(&self.search);
        let any_color = self.colors.iter().any(|b| *b);
        let mut cards: Vec<&Card> = self
            .db
            .cards
            .iter()
            .filter(|c| {
                self.whole_set
                    || self.coll.owned(&c.name) > 0
                    || (c.is_basic() && self.rules.unlimited_basics)
            })
            .filter(|c| !c.is_basic() || self.type_filter == 6)
            .filter(|c| {
                q.is_empty()
                    || crate::db::normalize(&c.name).contains(&q)
                    || c.type_line.to_lowercase().contains(&q)
                    || c.oracle_text.to_lowercase().contains(&q)
            })
            .filter(|c| {
                if !any_color {
                    return true;
                }
                let id = c.identity();
                (self.colors[5] && id.is_empty())
                    || (0..5).any(|i| self.colors[i] && id.contains(i))
            })
            .filter(|c| match self.type_filter {
                0 => true,
                7 => c.is_legendary(),
                i => c.type_line.contains(TYPE_FILTERS[i]),
            })
            .filter(|c| self.role_filter.is_none_or(|r| c.has_role(r)))
            .filter(|c| self.mv_max >= 7 || c.mana_value() <= self.mv_max)
            .filter(|c| !self.fits_only || identity.is_none_or(|id| c.identity().is_subset_of(id)))
            .filter(|c| !self.avail_only || self.available(c) > 0)
            .filter(|c| self.set_filter.is_empty() || c.set == self.set_filter)
            .collect();
        match self.sort {
            SortBy::Name => cards.sort_by(|a, b| a.name.cmp(&b.name)),
            SortBy::ManaValue => {
                cards.sort_by(|a, b| a.cmc.partial_cmp(&b.cmc).unwrap().then(a.name.cmp(&b.name)))
            }
            SortBy::Score => {
                let mut scored: Vec<(f32, &Card)> = cards
                    .into_iter()
                    .map(|c| {
                        let s = self
                            .memo
                            .scores
                            .as_ref()
                            .and_then(|m| m.get(&c.name).copied());
                        (
                            s.unwrap_or_else(|| {
                                analysis::score_card(c, &self.deck, &self.db, &self.rules).0
                            }),
                            c,
                        )
                    })
                    .collect();
                scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
                cards = scored.into_iter().map(|(_, c)| c).collect();
            }
            SortBy::Type => cards.sort_by(|a, b| {
                a.main_type()
                    .cmp(b.main_type())
                    .then(a.cmc.partial_cmp(&b.cmc).unwrap())
                    .then(a.name.cmp(&b.name))
            }),
        }
        cards
    }

    fn pool_panel(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            ui.heading(if self.whole_set {
                "All league cards"
            } else {
                "Your pool"
            });
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Search name, type or text…")
                    .desired_width(ui.available_width() - 10.0),
            );
        });
        ui.horizontal_wrapped(|ui| {
            for (i, sym) in ["W", "U", "B", "R", "G", "C"].iter().enumerate() {
                let (fill, text) = cards::symbol_colors(sym);
                let on = self.colors[i];
                let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                ui.painter().circle_filled(
                    rect.center(),
                    11.0,
                    if on { fill } else { fill.gamma_multiply(0.35) },
                );
                ui.painter().circle_stroke(
                    rect.center(),
                    11.0,
                    Stroke::new(
                        if on { 2.5 } else { 1.0 },
                        if on {
                            Color32::from_rgb(255, 205, 80)
                        } else {
                            Color32::GRAY
                        },
                    ),
                );
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    *sym,
                    FontId::proportional(12.0),
                    text,
                );
                if resp.on_hover_text("Filter by colour").clicked() {
                    self.colors[i] = !self.colors[i];
                }
            }
            egui::ComboBox::from_id_salt("type")
                .selected_text(TYPE_FILTERS[self.type_filter])
                .width(110.0)
                .show_ui(ui, |ui| {
                    for (i, t) in TYPE_FILTERS.iter().enumerate() {
                        ui.selectable_value(&mut self.type_filter, i, *t);
                    }
                });
            egui::ComboBox::from_id_salt("role")
                .selected_text(self.role_filter.map(|r| r.label()).unwrap_or("Any role"))
                .width(100.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.role_filter, None, "Any role");
                    for r in Role::ALL {
                        ui.selectable_value(&mut self.role_filter, Some(r), r.label());
                    }
                });
            ui.label("Max MV");
            ui.add(
                egui::Slider::new(&mut self.mv_max, 0..=7).custom_formatter(|v, _| {
                    if v >= 7.0 {
                        "any".into()
                    } else {
                        format!("{}", v)
                    }
                }),
            );
        });
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.fits_only, "Fits commander");
            ui.checkbox(&mut self.avail_only, "Not used up");
            ui.checkbox(&mut self.whole_set, "Show all league cards");
            let label = |code: &str| match code {
                "" => "All sets".to_string(),
                "MSH" => "Marvel Super Heroes".to_string(),
                "HOB" => "The Hobbit".to_string(),
                "FRA" => "Reality Fracture".to_string(),
                "FDN" => "Foundations".to_string(),
                "LEAGUE" => "Skill Tree & Shop".to_string(),
                other => other.to_string(),
            };
            let mut sets = vec![String::new()];
            sets.extend(self.db.league_sets());
            sets.push("LEAGUE".into());
            egui::ComboBox::from_id_salt("setfilter")
                .selected_text(label(&self.set_filter))
                .width(130.0)
                .show_ui(ui, |ui| {
                    for s in sets {
                        let text = label(&s);
                        ui.selectable_value(&mut self.set_filter, s, text);
                    }
                });
            egui::ComboBox::from_id_salt("sort")
                .selected_text(match self.sort {
                    SortBy::Type => "Sort: type",
                    SortBy::ManaValue => "Sort: mana value",
                    SortBy::Name => "Sort: name",
                    SortBy::Score => "Sort: best for deck",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.sort, SortBy::Type, "Type");
                    ui.selectable_value(&mut self.sort, SortBy::ManaValue, "Mana value");
                    ui.selectable_value(&mut self.sort, SortBy::Name, "Name");
                    ui.selectable_value(&mut self.sort, SortBy::Score, "Best for deck");
                });
        });
        let cards: Vec<Card> = self.pool_cards().into_iter().cloned().collect();
        ui.label(
            RichText::new(format!(
                "{} cards · click to inspect · double-click to add · right-click for more",
                cards.len()
            ))
            .small()
            .weak(),
        );
        let w = self.settings.card_width;
        ScrollArea::vertical()
            .id_salt("pool_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    for card in &cards {
                        let owned = self.coll.owned(&card.name);
                        let avail = self.available(card);
                        let fits = self.fits(card);
                        let badge = if card.is_basic() {
                            Some(format!("{} in deck", self.deck.count_of(&card.name)))
                        } else if owned == 0 {
                            Some("not owned".into())
                        } else {
                            Some(format!("{}/{}", avail.min(owned), owned))
                        };
                        let resp = self.show_card(
                            ui,
                            CardLook {
                                card,
                                width: w,
                                image: None,
                                badge,
                                dimmed: avail == 0 || !fits,
                                selected: self.selected.as_deref() == Some(card.name.as_str()),
                            },
                        );
                        self.card_interactions(&resp, card, acts);
                    }
                });
            });
    }

    /// Click / double-click / right-click behaviour shared by every card thumbnail.
    fn card_interactions(&self, resp: &egui::Response, card: &Card, acts: &mut Vec<Action>) {
        if resp.clicked() {
            acts.push(Action::Select(card.name.clone()));
        }
        if resp.double_clicked() {
            acts.push(Action::Add(card.name.clone(), 1));
        }
        let name = card.name.clone();
        let can_command = card.can_be_commander();
        let in_deck = self.deck.count_of(&card.name) > 0;
        resp.context_menu(|ui| {
            ui.label(RichText::new(&name).strong());
            if ui.button("Add to deck").clicked() {
                acts.push(Action::Add(name.clone(), 1));
                ui.close();
            }
            if in_deck && ui.button("Remove one from deck").clicked() {
                acts.push(Action::Remove(name.clone(), 1));
                ui.close();
            }
            if can_command && ui.button("Make commander").clicked() {
                acts.push(Action::SetCommander(name.clone()));
                ui.close();
            }
            if can_command && ui.button("Auto-build around it").clicked() {
                acts.push(Action::AutoBuild(name.clone(), None));
                ui.close();
            }
            if ui.button("Inspect").clicked() {
                acts.push(Action::Select(name.clone()));
                ui.close();
            }
        });
    }

    fn inspector(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let Some(name) = self.selected.clone() else {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Click a card to see it here.").weak());
            });
            return;
        };
        let Some(card) = self.card(&name).cloned() else {
            return;
        };
        ScrollArea::vertical().id_salt("inspector").show(ui, |ui| {
            let w = (ui.available_width() - 8.0).min(420.0);
            let tex = self.full_texture(&card);
            ui.vertical_centered(|ui| {
                card_widget(
                    ui,
                    CardLook {
                        card: &card,
                        width: w,
                        image: tex.as_ref(),
                        badge: None,
                        dimmed: false,
                        selected: false,
                    },
                );
            });
            if tex.is_some() {
                // The picture shows the printed text; repeat it below for easy reading.
                ui.label(RichText::new(&card.oracle_text).size(13.0));
            }
            ui.add_space(4.0);
            let owned = self.coll.owned(&card.name);
            let used = self.deck.used(&card.name);
            let avail = self.available(&card);
            ui.label(format!(
                "In your pool: {} · in deck: {} · can add: {}",
                owned,
                used,
                if avail >= 99 {
                    "any".to_string()
                } else {
                    avail.to_string()
                }
            ));
            ui.label(format!(
                "{} · {} {} · trade value {} VP (claim cost {} VP)",
                card.rarity,
                if card.set.is_empty() {
                    "MSH"
                } else {
                    &card.set
                },
                card.collector_number.clone().unwrap_or_default(),
                league::vp_value(&card),
                league::claim_cost(&card)
            ));
            if !card.tags.is_empty() {
                ui.label(RichText::new(format!("Scryfall tags: {}", card.tags.join(", "))).small())
                    .on_hover_text("Community Tagger tags from Scryfall - used to confirm roles and synergy");
            }
            for line in crate::edhrec::card_summary(&self.db.edhrec, &card.name, &self.deck.commanders()) {
                ui.label(RichText::new(line).small());
            }
            if card.can_be_commander() {
                if let Some(s) = self.db.commander_stat(&card.name) {
                    ui.label(
                        RichText::new(format!(
                            "As a commander: {:.0}% win rate in {} logged games ({}; 25% is average in 4-player pods)",
                            s.win_rate * 100.0,
                            s.games,
                            s.source
                        ))
                        .small(),
                    );
                }
            }
            ui.horizontal_wrapped(|ui| {
                let fits = self.fits(&card);
                if ui
                    .add_enabled(avail > 0 && fits, egui::Button::new("+ Add to deck"))
                    .clicked()
                {
                    acts.push(Action::Add(card.name.clone(), 1));
                }
                if ui
                    .add_enabled(
                        self.deck.count_of(&card.name) > 0,
                        egui::Button::new("- Remove"),
                    )
                    .clicked()
                {
                    acts.push(Action::Remove(card.name.clone(), 1));
                }
                if card.can_be_commander() {
                    if ui.button("Make commander").clicked() {
                        acts.push(Action::SetCommander(card.name.clone()));
                    }
                    if ui.button("Auto-build").clicked() {
                        acts.push(Action::AutoBuild(card.name.clone(), None));
                    }
                }
                if card.name == PIPER || card.oracle_text.contains("Partner") {
                    if ui.button("Add as 2nd commander").clicked() {
                        acts.push(Action::SetPartner(Some(card.name.clone())));
                    }
                }
            });
            if !self.fits(&card) {
                ui.colored_label(
                    Color32::from_rgb(255, 150, 110),
                    format!(
                        "Outside your commander's colours ({}).",
                        self.deck
                            .identity(&self.db)
                            .map(|i| i.to_string())
                            .unwrap_or_default()
                    ),
                );
            } else if !card.is_land() && self.deck.commander.is_some() {
                let (score, why) = analysis::score_card(&card, &self.deck, &self.db, &self.rules);
                ui.label(RichText::new(format!("Fit for this deck: {:.1}", score)).strong());
                for w in why {
                    ui.label(RichText::new(format!("  • {}", w)).small());
                }
            }
            let roles: Vec<&str> = card.roles().iter().map(|r| r.label()).collect();
            if !roles.is_empty() {
                ui.label(RichText::new(format!("Roles: {}", roles.join(", "))).small());
            }
            let parsed = card.parsed();
            egui::CollapsingHeader::new(RichText::new("How the app reads this card").small())
                .id_salt(("reads", &card.name))
                .show(ui, |ui| {
                    for line in parsed.describe() {
                        ui.label(RichText::new(format!("• {}", line)).small());
                    }
                    if let Some(r) = parsed.removal() {
                        ui.label(
                            RichText::new(format!(
                                "Removal: {} ({:.1} points{}{})",
                                r.label,
                                r.points,
                                if r.mass { ", board wipe" } else { "" },
                                if r.repeatable { ", repeatable" } else { "" }
                            ))
                            .small()
                            .color(Color32::from_rgb(150, 220, 150)),
                        );
                    }
                    for u in &parsed.unparsed {
                        ui.label(
                            RichText::new(format!("? not understood: {}", u))
                                .small()
                                .color(Color32::from_rgb(255, 190, 110)),
                        );
                    }
                });
            if !parsed.unparsed.is_empty() {
                ui.label(
                    RichText::new("? Some of this card's text isn't modelled - its scores may miss part of what it does.")
                        .small()
                        .color(Color32::from_rgb(255, 190, 110)),
                );
            }
            if !card.is_verified() {
                ui.colored_label(
                    Color32::YELLOW,
                    "Text not fully verified - check the physical card.",
                );
            }
            ui.label(RichText::new(&card.note).small().weak());
        });
    }

    fn deck_panel(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        // Commander row.
        ui.horizontal(|ui| {
            let cmd_w = 118.0;
            let commanders: Vec<String> = self.deck.commanders().into_iter().cloned().collect();
            if commanders.is_empty() {
                let (rect, resp) = ui.allocate_exact_size(card_size(cmd_w), Sense::click());
                ui.painter().rect_stroke(
                    rect,
                    CornerRadius::from(6.0),
                    Stroke::new(2.0, Color32::from_gray(90)),
                    egui::StrokeKind::Inside,
                );
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "No commander\n(Commanders tab)",
                    FontId::proportional(12.0),
                    Color32::GRAY,
                );
                if resp.clicked() {
                    self.tab = Tab::Commanders;
                }
            }
            for name in &commanders {
                if let Some(card) = self.card(name).cloned() {
                    let badge = if *name == PIPER {
                        self.deck.piper_color.map(|c| format!("adds {}", c))
                    } else {
                        Some("Commander".into())
                    };
                    let resp = self.show_card(
                        ui,
                        CardLook {
                            card: &card,
                            width: cmd_w,
                            image: None,
                            badge,
                            dimmed: false,
                            selected: false,
                        },
                    );
                    if resp.clicked() {
                        acts.push(Action::Select(card.name.clone()));
                    }
                    if *name == PIPER {
                        resp.context_menu(|ui| {
                            if ui.button("Remove The Prismatic Piper").clicked() {
                                acts.push(Action::SetPartner(None));
                                ui.close();
                            }
                        });
                    }
                }
            }
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.heading(
                        self.deck
                            .commander
                            .clone()
                            .unwrap_or_else(|| "New deck".into()),
                    );
                    if let Some(id) = self.deck.identity(&self.db) {
                        identity_dots(ui, id, 16.0);
                    }
                });
                self.deck_summary(ui);
                ui.horizontal_wrapped(|ui| {
                    let cmd = self.deck.commander.clone();
                    if ui
                        .add_enabled(cmd.is_some(), egui::Button::new("Auto-build"))
                        .on_hover_text(
                            "Fill the deck with the best cards from your pool for this commander",
                        )
                        .clicked()
                    {
                        let piper = if self.deck.partner.as_deref() == Some(PIPER) {
                            self.deck.piper_color
                        } else {
                            None
                        };
                        acts.push(Action::AutoBuild(cmd.unwrap(), piper));
                    }
                    if ui.button("Fill with basic lands").clicked() {
                        acts.push(Action::Fill);
                    }
                    if ui.button("Clear").clicked() {
                        acts.push(Action::ClearDeck);
                    }
                    ui.separator();
                    ui.selectable_value(&mut self.settings.visual_deck, true, "Cards");
                    ui.selectable_value(&mut self.settings.visual_deck, false, "List");
                });
                // The Prismatic Piper offer for mono-colour commanders.
                if let Some(cmd) = self.deck.commander.as_ref().and_then(|c| self.db.get(c)) {
                    if cmd.identity().count() == 1 {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Add a 2nd colour with The Prismatic Piper:").small(),
                            );
                            for (i, c) in COLORS.iter().enumerate() {
                                if cmd.identity().contains(i) {
                                    continue;
                                }
                                let (fill, text) = cards::symbol_colors(&c.to_string());
                                let chosen = self.deck.piper_color == Some(*c);
                                if ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new(c.to_string()).color(text).strong(),
                                        )
                                        .fill(fill)
                                        .selected(chosen),
                                    )
                                    .clicked()
                                {
                                    acts.push(Action::PiperColor(*c));
                                }
                            }
                            if self.deck.partner.is_some() && ui.small_button("none").clicked() {
                                acts.push(Action::SetPartner(None));
                            }
                        });
                    }
                }
            });
        });
        ui.separator();
        // Basic land controls.
        if let Some(id) = self.deck.identity(&self.db) {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Basic lands:").strong());
                for i in id.indices() {
                    let name = BASIC_FOR_COLOR[i];
                    let n = self.deck.count_of(name);
                    let (fill, text) = cards::symbol_colors(&COLORS[i].to_string());
                    ui.label(
                        RichText::new(format!(" {} ", COLORS[i]))
                            .background_color(fill)
                            .color(text)
                            .strong(),
                    );
                    ui.label(format!("{} {}", n, name));
                    if ui.small_button("-").clicked() {
                        acts.push(Action::Remove(name.into(), 1));
                    }
                    if ui.small_button("+").clicked() {
                        acts.push(Action::Add(name.into(), 1));
                    }
                    ui.add_space(8.0);
                }
                let st = analysis::stats(&self.deck, &self.db);
                let (lo, hi) = analysis::land_range(&self.rules);
                ui.label(
                    RichText::new(format!("Lands: {} (league advice {}-{})", st.lands, lo, hi))
                        .color(if st.lands >= lo.saturating_sub(1) && st.lands <= hi + 1 {
                            Color32::from_rgb(140, 220, 140)
                        } else {
                            Color32::from_rgb(255, 190, 110)
                        }),
                );
            });
            ui.separator();
        }
        let avail_h = ui.available_height();
        ScrollArea::both().id_salt("deck_scroll").max_height(avail_h - 190.0).auto_shrink([false, false]).show(ui, |ui| {
            if self.deck.cards.is_empty() {
                ui.label(RichText::new("The deck is empty. Double-click cards in your pool to add them, or press Auto-build.").weak());
            } else if self.settings.visual_deck {
                self.visual_deck(ui, acts);
            } else {
                self.list_deck(ui, acts);
            }
        });
        ui.separator();
        self.suggestion_strip(ui, acts);
    }

    /// Cards in columns by mana value, stacked like a real table spread.
    fn visual_deck(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let mut cols: Vec<Vec<(Card, u32)>> = vec![Vec::new(); 7];
        for (name, n) in self.deck.cards.clone() {
            if let Some(c) = self.card(&name).cloned() {
                let col = if c.is_land() {
                    6
                } else {
                    analysis::curve_bucket(c.mana_value())
                };
                cols[col].push((c, n));
            }
        }
        for col in cols.iter_mut() {
            col.sort_by(|a, b| {
                a.0.cmc
                    .partial_cmp(&b.0.cmc)
                    .unwrap()
                    .then(a.0.name.cmp(&b.0.name))
            });
        }
        let used = cols.iter().filter(|c| !c.is_empty()).count().max(1) as f32;
        let w = ((ui.available_width() - used * 10.0) / used).clamp(96.0, 150.0);
        let step = w * 0.25;
        ui.horizontal_top(|ui| {
            for (i, col) in cols.iter().enumerate() {
                if col.is_empty() {
                    continue;
                }
                ui.vertical(|ui| {
                    let count: u32 = col.iter().map(|(_, n)| n).sum();
                    let title = if i == 6 {
                        "Lands".to_string()
                    } else {
                        format!("MV {}", curve_label(i))
                    };
                    ui.label(RichText::new(format!("{} ({})", title, count)).strong());
                    if col.is_empty() {
                        ui.allocate_space(vec2(w, 10.0));
                        return;
                    }
                    let full = card_size(w);
                    let height = full.y + step * (col.len() as f32 - 1.0);
                    let (area, _) = ui.allocate_exact_size(vec2(w, height), Sense::hover());
                    for (k, (card, n)) in col.iter().enumerate() {
                        let top = area.top() + step * k as f32;
                        let rect = egui::Rect::from_min_size(pos2(area.left(), top), full);
                        let hit = if k + 1 == col.len() {
                            rect
                        } else {
                            egui::Rect::from_min_size(rect.min, vec2(w, step))
                        };
                        let resp = ui.interact(
                            hit,
                            ui.scope_id().with(("deckcard", &card.name)),
                            Sense::click(),
                        );
                        let tex = if self.settings.real_images && ui.is_rect_visible(rect) {
                            self.images.thumb(card)
                        } else {
                            None
                        };
                        paint_card(
                            ui,
                            rect,
                            &CardLook {
                                card,
                                width: w,
                                image: tex.as_ref(),
                                badge: if *n > 1 {
                                    Some(format!("x{}", n))
                                } else {
                                    None
                                },
                                dimmed: false,
                                selected: self.selected.as_deref() == Some(card.name.as_str()),
                            },
                            resp.hovered(),
                        );
                        if resp.clicked() {
                            acts.push(Action::Select(card.name.clone()));
                        }
                        if resp.double_clicked() {
                            acts.push(Action::Remove(card.name.clone(), 1));
                        }
                        let name = card.name.clone();
                        let resp = self.with_zoom(
                            resp,
                            card,
                            Some("Click to inspect · double-click to remove one".into()),
                        );
                        resp.context_menu(|ui| {
                            if ui.button("Remove one").clicked() {
                                acts.push(Action::Remove(name.clone(), 1));
                                ui.close();
                            }
                            if ui.button("Add another").clicked() {
                                acts.push(Action::Add(name.clone(), 1));
                                ui.close();
                            }
                        });
                    }
                });
            }
        });
    }

    fn list_deck(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let cards: Vec<(Card, u32)> = analysis::resolved(&self.deck, &self.db)
            .into_iter()
            .map(|(c, n)| (c.clone(), n))
            .collect();
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
            let mut group: Vec<&(Card, u32)> =
                cards.iter().filter(|(c, _)| c.main_type() == t).collect();
            if group.is_empty() {
                continue;
            }
            group.sort_by(|a, b| {
                a.0.cmc
                    .partial_cmp(&b.0.cmc)
                    .unwrap()
                    .then(a.0.name.cmp(&b.0.name))
            });
            let total: u32 = group.iter().map(|(_, n)| n).sum();
            ui.label(
                RichText::new(format!("{} ({})", t, total))
                    .strong()
                    .size(15.0),
            );
            for (card, n) in group {
                ui.horizontal(|ui| {
                    if ui.small_button("-").clicked() {
                        acts.push(Action::Remove(card.name.clone(), 1));
                    }
                    ui.label(RichText::new(format!("{}", n)).monospace());
                    if ui.small_button("+").clicked() {
                        acts.push(Action::Add(card.name.clone(), 1));
                    }
                    let color = cards::frame_color(card);
                    let resp = ui.add(
                        egui::Label::new(
                            RichText::new(&card.name).color(cards::lighten_for_text(color)),
                        )
                        .sense(Sense::click()),
                    );
                    let resp = self.with_zoom(resp, card, None);
                    if resp.clicked() {
                        acts.push(Action::Select(card.name.clone()));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        cost_widget(ui, &card.mana_cost, 16.0);
                    });
                });
            }
            ui.add_space(6.0);
        }
    }

    fn suggestion_strip(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let space = self.rules.deck_size as i64 - self.deck.total_count(&self.rules) as i64;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Suggested additions").strong());
            ui.label(
                RichText::new(format!(
                    "({} slots open · double-click to add)",
                    space.max(0)
                ))
                .small()
                .weak(),
            );
        });
        if self.deck.commander.is_none() {
            ui.label(
                RichText::new("Choose a commander to get suggestions that fit its colours.").weak(),
            );
            return;
        }
        let picks: Vec<(Card, f32, Vec<String>)> = self
            .memo
            .suggestions
            .get_or_insert_with(|| {
                analysis::suggestions(&self.deck, &self.db, &self.coll, &self.rules)
                    .into_iter()
                    .take(12)
                    .map(|s| (s.card.clone(), s.score, s.reasons))
                    .collect()
            })
            .clone();
        ScrollArea::horizontal().id_salt("suggest").show(ui, |ui| {
            ui.horizontal(|ui| {
                for (card, score, why) in &picks {
                    ui.vertical(|ui| {
                        let resp = self.show_card(
                            ui,
                            CardLook {
                                card,
                                width: 100.0,
                                image: None,
                                badge: Some(format!("{:.1}", score)),
                                dimmed: false,
                                selected: self.selected.as_deref() == Some(card.name.as_str()),
                            },
                        );
                        let resp = resp.on_hover_text(why.join("\n"));
                        self.card_interactions(&resp, card, acts);
                    });
                }
            });
        });
    }

    fn build_tab(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        Panel::right("inspector")
            .resizable(true)
            .default_size(330.0)
            .size_range(260.0..=480.0)
            .show(ui, |ui| self.inspector(ui, acts));
        Panel::left("pool")
            .resizable(true)
            .default_size(470.0)
            .size_range(300.0..=900.0)
            .show(ui, |ui| self.pool_panel(ui, acts));
        egui::CentralPanel::default().show(ui, |ui| self.deck_panel(ui, acts));
    }

    // ------------------------------------------------------------------ commanders

    /// What the commander ranking depends on; when it changes, the ranking is rebuilt.
    fn ranking_key(&self) -> String {
        crate::ranking::ranking_key(&self.db, &self.coll, &self.rules)
    }

    /// Starts rebuilding the ranking on a background thread (a deck build and playtest per
    /// option, twice while the singleton preview applies).
    fn start_ranking(&mut self) {
        let key = self.ranking_key();
        if self.rank_job.as_ref().is_some_and(|j| j.key == key) {
            return;
        }
        let (db, coll, rules, weights) = (
            self.db.clone(),
            self.coll.clone(),
            self.rules.clone(),
            self.settings.weights,
        );
        let total = crate::ranking::build_count(&db, &coll, &rules).max(1);
        let done = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (tx, rx) = std::sync::mpsc::channel();
        let progress = done.clone();
        std::thread::spawn(move || {
            let ranks = crate::ranking::rank_with_preview(&db, &coll, &rules, &weights, &progress);
            let _ = tx.send(ranks);
        });
        self.rank_job = Some(RankJob {
            done,
            total,
            rx,
            key,
        });
    }

    /// Starts the playtest lab on the current deck (or, if it isn't complete, on the deck
    /// auto-build makes for the commander), replacing any lab already running.
    fn start_lab(&mut self, effort: crate::lab::Effort) {
        self.stop_lab();
        let Some(cmd) = self
            .deck
            .commander
            .as_ref()
            .and_then(|c| self.db.get(c))
            .cloned()
        else {
            self.status = "Choose a commander first.".into();
            return;
        };
        if self.lab.as_ref().is_some_and(|r| r.commander != cmd.name) {
            self.lab = None;
        }
        let (db, coll, rules, deck) = (
            self.db.clone(),
            self.coll.clone(),
            self.rules.clone(),
            self.deck.clone(),
        );
        let progress = std::sync::Arc::new(crate::lab::Progress::new());
        let (tx, rx) = std::sync::mpsc::channel();
        let p = progress.clone();
        let name = cmd.name.clone();
        std::thread::spawn(move || {
            let report = crate::lab::explore(&cmd, &deck, &db, &coll, &rules, effort, &p);
            let _ = tx.send(report);
        });
        self.lab_job = Some(LabJob {
            progress,
            rx,
            commander: name,
            started: std::time::Instant::now(),
        });
    }

    fn stop_lab(&mut self) {
        if let Some(job) = self.lab_job.take() {
            job.progress.cancel();
        }
    }

    fn poll_lab(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.lab_job else { return };
        // A lab for a commander the deck no longer has is of no use: stop it.
        if self.deck.commander.as_deref() != Some(job.commander.as_str()) {
            self.stop_lab();
            return;
        }
        match job.rx.try_recv() {
            Ok(Some(report)) => {
                self.status = match &report.best {
                    Some(b) => format!(
                        "Playtest lab finished: a combination of {} swaps measured {} win-rate points - see the Playtest Lab tab.",
                        b.swaps.len(),
                        crate::lab::pts_se(&b.gain)
                    ),
                    None => "Playtest lab finished: no change beat the current build (details on the Playtest Lab tab).".into(),
                };
                self.lab = Some(report);
                self.lab_job = None;
            }
            Ok(None) | Err(std::sync::mpsc::TryRecvError::Disconnected) => self.lab_job = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(300))
            }
        }
    }

    fn poll_jobs(&mut self, ctx: &egui::Context) {
        self.poll_lab(ctx);
        if let Some(job) = &self.rank_job {
            match job.rx.try_recv() {
                Ok(ranks) => {
                    self.ranking = Some(ranks);
                    self.ranking_key = job.key.clone();
                    self.rank_job = None;
                    // The draft helper judges picks against the top commanders' decks.
                    self.memo.draft = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(150))
                }
                Err(_) => self.rank_job = None,
            }
        }
        if let Some(rx) = &self.data_job {
            match rx.try_recv() {
                Ok(msg) => {
                    // The card list may have grown; reload it with the new ratings.
                    if let Ok(mut db) = CardDb::load(&self.data_dir.join("cards.json")) {
                        db.attach_ratings(&self.data_dir);
                        self.db = db;
                    } else {
                        self.db.attach_ratings(&self.data_dir);
                    }
                    self.rev += 1;
                    self.status = msg;
                    self.data_job = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(300))
                }
                Err(_) => self.data_job = None,
            }
        }
    }

    /// Downloads Playgroup.gg commander stats, EDHREC data and Scryfall tags in the background.
    fn start_data_download(&mut self) {
        if self.data_job.is_some() {
            return;
        }
        let dir = self.data_dir.clone();
        let names: Vec<String> = self
            .db
            .cards
            .iter()
            .filter(|c| c.can_be_commander() && self.coll.owned(&c.name) > 0)
            .map(|c| c.name.clone())
            .collect();
        let force_cards = self.force_card_update;
        self.force_card_update = false;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // New league cards from Scryfall (e.g. Nauctis once released) - only on request.
            let mut parts = Vec::new();
            if force_cards {
                match crate::setdata::update_cards(&dir) {
                    Ok((added, _, missing)) => {
                        parts.push(format!("{} new cards", added));
                        if !missing.is_empty() {
                            parts.push(format!("not released yet: {}", missing.join(", ")));
                        }
                    }
                    Err(_) => parts.push(
                        "Scryfall unavailable (offline?) - card database not updated".to_string(),
                    ),
                }
            }
            let b = match crate::ratings::fetch_playgroup(&dir, &names) {
                Ok(n) => format!("Playgroup.gg: {} commanders", n),
                Err(_) => "Playgroup.gg unavailable".to_string(),
            };
            parts.push(b);
            // EDHREC: card ranks, and a fresh page for each commander you own.
            let c = match CardDb::load(&dir.join("cards.json"))
                .map_err(|e| e.to_string())
                .and_then(|db| crate::edhrec::fetch(&dir, &db, &names, true))
            {
                Ok(f) => format!(
                    "EDHREC: {} cards ranked, {} commander pages{}{}",
                    f.ranks,
                    f.pages,
                    if f.no_page > 0 {
                        format!(" ({} commanders have no page yet)", f.no_page)
                    } else {
                        String::new()
                    },
                    match &f.first_error {
                        Some(e) => format!(
                            " - some pages failed ({}; details in data/edhrec_log.txt)",
                            e
                        ),
                        None => String::new(),
                    }
                ),
                Err(_) => "EDHREC unavailable (offline?)".to_string(),
            };
            parts.push(c);
            let d = match CardDb::load(&dir.join("cards.json"))
                .map_err(|e| e.to_string())
                .and_then(|db| crate::tags::fetch(&dir, &db))
            {
                Ok((cards, tags)) => format!("Scryfall tags: {} cards, {} tags", cards, tags),
                Err(_) => "Scryfall tags unavailable (offline?)".to_string(),
            };
            parts.push(d);
            let _ = tx.send(format!("Updated - {}.", parts.join("; ")));
        });
        self.data_job = Some(rx);
        self.status = "Downloading card ratings in the background...".into();
    }

    // ------------------------------------------------------------------ playtest lab

    fn lab_tab(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        const GOOD: Color32 = Color32::from_rgb(110, 220, 120);
        const BAD: Color32 = Color32::from_rgb(255, 120, 100);
        const NOTE: Color32 = Color32::from_rgb(255, 190, 110);
        // Coloured when clearly (2+ standard errors) better or worse.
        let gain = |m: &crate::lab::Measure| {
            let t = RichText::new(crate::lab::pts_se(m)).monospace();
            if m.z() > 2.0 {
                t.color(GOOD)
            } else if m.z() < -2.0 {
                t.color(BAD)
            } else {
                t
            }
        };
        ui.heading("Playtest lab");
        ui.label(
            "The deck builder picks cards by score, and scores can be wrong. The lab plays your deck and changed versions \
             of it through the same simulated 4-player games, measures what each card and each possible addition is worth, \
             and confirms the best combination on fresh games. Your deck only changes if you apply a suggestion.",
        );
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            let has_cmd = self.deck.commander.is_some();
            if let Some(job) = &self.lab_job {
                let f = job.progress.fraction();
                let secs = job.started.elapsed().as_secs_f32();
                let left = if f > 0.03 {
                    format!(" · about {} left", fmt_secs(secs * (1.0 - f) / f))
                } else {
                    String::new()
                };
                ui.add(egui::ProgressBar::new(f).desired_width(360.0).text(format!(
                    "{} {:.0}%{}",
                    job.progress.stage(),
                    f * 100.0,
                    left
                )));
                if ui.button("Stop").clicked() {
                    acts.push(Action::StopLab);
                }
            } else {
                if ui
                    .add_enabled(has_cmd, egui::Button::new("Run quick lab (about 1 min)"))
                    .clicked()
                {
                    acts.push(Action::RunLab(crate::lab::Effort::QUICK));
                }
                if ui
                    .add_enabled(has_cmd, egui::Button::new("Run thorough lab (about 5 min)"))
                    .clicked()
                {
                    acts.push(Action::RunLab(crate::lab::Effort::THOROUGH));
                }
            }
            ui.separator();
            ui.label("When I choose a commander or auto-build, run");
            let before = self.settings.lab_auto;
            egui::ComboBox::from_id_salt("lab_auto")
                .selected_text(self.settings.lab_auto.label())
                .show_ui(ui, |ui| {
                    for a in [LabAuto::Thorough, LabAuto::Quick, LabAuto::Off] {
                        ui.selectable_value(&mut self.settings.lab_auto, a, a.label());
                    }
                });
            if self.settings.lab_auto != before {
                self.save_settings();
            }
        });
        ui.separator();
        let Some(r) = &self.lab else {
            ui.label(if self.lab_job.is_some() {
                "Playtesting in the background - you can keep building meanwhile."
            } else if self.deck.commander.is_none() {
                "Choose a commander (Commanders tab) and the lab starts by itself."
            } else {
                "No report yet: run the lab above."
            });
            return;
        };
        let current = r.fits(&self.deck);
        let applied = r
            .best
            .as_ref()
            .is_some_and(|b| b.deck.cards == self.deck.cards);
        let mins = (r.seconds / 60.0).floor();
        ui.label(
            RichText::new(format!(
                "{} - the tested deck won {:.1}% of {} games (25% = even) · {} lab, {} games in {}m {:02}s",
                r.commander,
                r.base_win * 100.0,
                r.base_games,
                r.effort.name,
                r.games,
                mins,
                (r.seconds - mins * 60.0).round() as u32
            ))
            .strong(),
        );
        for n in &r.notes {
            ui.label(RichText::new(n.as_str()).color(NOTE));
        }
        if !current && !applied {
            ui.label(
                RichText::new("Your deck has changed since this report - run the lab again to test the new list.")
                    .color(NOTE),
            );
        }
        ScrollArea::vertical()
            .id_salt("lab_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // Best combination.
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    match &r.best {
                        Some(b) => {
                            ui.label(
                                RichText::new(format!(
                                    "Best combination the playtests support: {} win-rate points",
                                    crate::lab::pts_se(&b.gain)
                                ))
                                .heading()
                                .color(GOOD),
                            );
                            ui.label(format!(
                                "Confirmed on {} fresh games (not the ones that found it). Each swap's number is the combination with it minus without it.",
                                b.gain.games
                            ));
                            egui::Grid::new("lab_best").striped(true).show(ui, |ui| {
                                for (s, m) in &b.swaps {
                                    ui.label("cut");
                                    if ui.link(s.cut.as_str()).clicked() {
                                        acts.push(Action::Select(s.cut.clone()));
                                    }
                                    ui.label("add");
                                    if ui.link(s.add.as_str()).clicked() {
                                        acts.push(Action::Select(s.add.clone()));
                                    }
                                    ui.label(gain(m));
                                    ui.end_row();
                                }
                            });
                            ui.horizontal(|ui| {
                                if applied {
                                    ui.label(RichText::new("Applied to your deck (Undo takes it back).").color(GOOD));
                                } else {
                                    let ok = current;
                                    let resp = ui.add_enabled(ok, egui::Button::new("Apply best combination"));
                                    if resp.clicked() {
                                        acts.push(Action::ApplyDeck(
                                            b.deck.clone(),
                                            format!(
                                                "Applied the lab's best combination ({} win-rate points). Undo takes it back.",
                                                crate::lab::pts_se(&b.gain)
                                            ),
                                        ));
                                    }
                                    if !ok {
                                        let _ = resp.on_disabled_hover_text("Your deck changed since the lab ran.");
                                    }
                                }
                            });
                        }
                        None => {
                            ui.label(RichText::new("No change beats the current build.").heading());
                            if let Some(c) = &r.rejected {
                                let list: Vec<String> = c
                                    .swaps
                                    .iter()
                                    .map(|(s, _)| format!("{} -> {}", s.cut, s.add))
                                    .collect();
                                ui.label(format!(
                                    "The best try ({}) measured {} points on {} fresh games - not clearly better (needs 2 standard errors).",
                                    list.join(", "),
                                    crate::lab::pts_se(&c.gain),
                                    c.gain.games
                                ));
                            }
                        }
                    }
                });
                ui.add_space(6.0);
                if !r.colors.is_empty() {
                    ui.label(RichText::new("Colour sets").strong());
                    ui.label(
                        RichText::new(format!(
                            "The deck rebuilt by auto-build in each colour set, {} games each against the same opponents and seats.",
                            r.colors[0].games
                        ))
                        .small()
                        .weak(),
                    );
                    egui::Grid::new("lab_colors").striped(true).show(ui, |ui| {
                        for c in &r.colors {
                            ui.label(RichText::new(c.colors.to_string()).monospace().strong());
                            ui.label(format!("won {:.1}%", c.win * 100.0));
                            if c.current {
                                ui.label("the tested deck");
                                ui.label("");
                            } else {
                                ui.label(gain(&c.vs_current));
                                if ui
                                    .small_button("Use this build")
                                    .on_hover_text("Replace the deck with this colour set's auto-build (Undo takes it back)")
                                    .clicked()
                                {
                                    acts.push(Action::ApplyDeck(
                                        c.deck.clone(),
                                        format!("Rebuilt in {}. Undo takes it back.", c.colors),
                                    ));
                                }
                            }
                            ui.end_row();
                        }
                    });
                    ui.add_space(6.0);
                }
                if !r.lands.is_empty() {
                    ui.label(RichText::new("Land count").strong());
                    egui::Grid::new("lab_lands").striped(true).show(ui, |ui| {
                        for l in &r.lands {
                            ui.label(format!("{} lands", l.lands));
                            ui.label(gain(&l.gain));
                            ui.label(
                                RichText::new(format!("cut {}, add {}", l.swap.cut, l.swap.add))
                                    .small()
                                    .weak(),
                            );
                            ui.end_row();
                        }
                    });
                    ui.add_space(6.0);
                }
                let repl = r.worth.first().map(|w| w.replacement.clone()).unwrap_or_default();
                egui::CollapsingHeader::new(RichText::new("What each card is worth").strong())
                    .default_open(true)
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(format!(
                                "Win-rate points lost when one copy is replaced by the best card left on your bench ({}), {} games each. \
                                 Dead weight: replacing it helped, clearly more than replacing a typical card of the deck. \
                                 Score: the deck builder's estimate.",
                                repl,
                                r.worth.first().map_or(0, |w| w.worth.games)
                            ))
                            .small()
                            .weak(),
                        );
                        egui::Grid::new("lab_worth").striped(true).show(ui, |ui| {
                            for w in &r.worth {
                                ui.label(gain(&w.worth));
                                let name = if w.copies > 1 {
                                    format!("{} (1 of {})", w.name, w.copies)
                                } else {
                                    w.name.clone()
                                };
                                if ui.link(name).clicked() {
                                    acts.push(Action::Select(w.name.clone()));
                                }
                                ui.label(RichText::new(format!("score {:.1}", w.heuristic)).weak());
                                if w.dead_weight {
                                    ui.label(RichText::new("dead weight").color(BAD));
                                } else {
                                    ui.label("");
                                }
                                ui.end_row();
                            }
                        });
                    });
                egui::CollapsingHeader::new(RichText::new("Additions from your pool").strong())
                    .default_open(true)
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(format!(
                                "The best-scoring cards you could add, each tried in place of the least valuable cards; the best place is shown. {} games each.",
                                r.additions.first().map_or(0, |a| a.gain.games)
                            ))
                            .small()
                            .weak(),
                        );
                        egui::Grid::new("lab_adds").striped(true).show(ui, |ui| {
                            for a in &r.additions {
                                ui.label(gain(&a.gain));
                                if ui.link(a.add.as_str()).clicked() {
                                    acts.push(Action::Select(a.add.clone()));
                                }
                                ui.label(RichText::new(format!("for {}", a.cut)).weak());
                                ui.label(RichText::new(format!("score {:.1}", a.heuristic)).weak());
                                ui.end_row();
                            }
                        });
                    });
                egui::CollapsingHeader::new("How to read this").show(ui, |ui| {
                    ui.label(
                        "Numbers are win-rate points (+2.0 = the deck wins 2% more of its games) ± one standard error. \
                         Each change is paired: the deck and the changed deck play exactly the same games (same opponents, \
                         seats and shuffles), so the games only differ once the changed card shows up - much less noise than \
                         comparing two separate win rates. Anything within about ±2 standard errors could be luck. \
                         Card worths and additions are screening numbers; the best combination is re-measured on fresh games, \
                         because the best of many noisy screens tends to be the luckiest one.",
                    );
                });
            });
    }

    fn commanders_tab(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        if self.ranking.is_none() || self.ranking_key != self.ranking_key() {
            self.start_ranking();
        }
        ui.horizontal(|ui| {
            ui.heading("Your possible commanders");
            if let Some(job) = &self.rank_job {
                let done = job.done.load(std::sync::atomic::Ordering::Relaxed);
                ui.add(
                    egui::ProgressBar::new(done as f32 / job.total as f32)
                        .desired_width(220.0)
                        .text(format!("building test decks {}/{}", done, job.total)),
                );
            }
        });
        ui.label(
            "Each commander is ranked by the full deck auto-build makes from your pool, graded on eight parts below and playtested \
             in simulated 4-player games. Mono-colour commanders are also tried with The Prismatic Piper adding their two most \
             promising colours. Scores run 0-100: 50 is your middle option.",
        );
        ui.horizontal_wrapped(|ui| {
            let data = if self.db.edhrec.is_empty() {
                RichText::new("No EDHREC data yet - press Update ratings.")
                    .color(Color32::from_rgb(255, 190, 110))
            } else {
                RichText::new(format!(
                    "EDHREC data from {} ({} cards ranked, {} commander pages)",
                    self.db.edhrec.fetched,
                    self.db.edhrec.ranked_cards(),
                    self.db.edhrec.commander_count()
                ))
                .color(Color32::from_rgb(150, 220, 150))
            };
            ui.label(data);
            ui.label(
                RichText::new(format!(
                    "· Playgroup.gg records for {} commanders",
                    self.db.commander_stats.len()
                ))
                .weak(),
            );
            if ui
                .add_enabled(self.data_job.is_none(), egui::Button::new("Update ratings"))
                .clicked()
            {
                self.start_data_download();
            }
        });
        let mut weights_changed = false;
        ui.collapsing("How much each part matters (drag to change the ranking)", |ui| {
            let w = &mut self.settings.weights;
            egui::Grid::new("weights").show(ui, |ui| {
                for (label, value, help) in [
                    ("Cards", &mut w.cards, "average strength of the deck's spells (roles, synergy, EDHREC rank, 4-player value)"),
                    ("Curve", &mut w.curve, "mana curve vs. the commander's plan (aggro decks want it lower, big-mana decks higher)"),
                    ("Creatures", &mut w.creatures, "creature count vs. the plan (e.g. 18-24 aggro, 15-20 midrange, 9-14 spells)"),
                    ("Interaction", &mut w.interaction, "removal (board wipes count extra) and card draw vs. the plan"),
                    ("Mana", &mut w.mana, "simulated hands: keepable, commander cast on time"),
                    ("Commander", &mut w.commander, "the commander card itself: Playgroup.gg win rate, abilities, cost"),
                    ("Playtest", &mut w.playtest, "win rate in simulated 4-player games vs. decks from simulated league pools"),
                    ("EDHREC", &mut w.edhrec, "how much of the deck is what EDHREC players run with this commander, and its popularity there"),
                ] {
                    ui.label(label);
                    weights_changed |= ui.add(egui::Slider::new(value, 0.0..=3.0).step_by(0.1)).changed();
                    ui.label(RichText::new(help).small().weak());
                    ui.end_row();
                }
            });
            if ui.button("Reset to defaults").clicked() {
                *w = crate::ranking::Weights::default();
                weights_changed = true;
            }
        });
        if weights_changed {
            let w = self.settings.weights;
            if let Some(r) = &mut self.ranking {
                r.reweigh(&w);
            }
            self.save_settings();
        }
        let Some(ranks) = self.ranking.clone() else {
            ui.label("Working out the rankings...");
            return;
        };
        if ranks.singleton.is_some() {
            ui.label(
                RichText::new(
                    "\"Singleton\" shows where each commander lands once singleton starts (Session 6), using today's pool.",
                )
                .small()
                .weak(),
            );
        }
        let groups: Vec<Vec<crate::ranking::Evaluation>> =
            crate::ranking::best_per_commander(&ranks.now)
                .into_iter()
                .map(|g| g.into_iter().cloned().collect())
                .collect();
        ScrollArea::vertical()
            .id_salt("cmd_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.with_layout(Layout::left_to_right(Align::Min).with_main_wrap(true), |ui| {
                    ui.spacing_mut().item_spacing = vec2(14.0, 14.0);
                    for (rank, group) in groups.iter().enumerate() {
                        let e = &group[0];
                        let Some(card) = self.card(&e.commander).cloned() else {
                            continue;
                        };
                        ui.allocate_ui(vec2(230.0, 640.0), |ui| {
                            ui.vertical(|ui| {
                                ui.set_width(220.0);
                                let current =
                                    self.deck.commander.as_deref() == Some(e.commander.as_str());
                                let resp = self.show_card(
                                    ui,
                                    CardLook {
                                        card: &card,
                                        width: 200.0,
                                        image: None,
                                        badge: Some(format!("#{}", rank + 1)),
                                        dimmed: false,
                                        selected: current,
                                    },
                                );
                                if resp.clicked() {
                                    acts.push(Action::Select(e.commander.clone()));
                                }
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(format!("{:.0}", e.total))
                                            .size(22.0)
                                            .strong()
                                            .color(Color32::from_rgb(255, 205, 80)),
                                    )
                                    .on_hover_text("Out of 100; 50 = your middle option");
                                    ui.vertical(|ui| {
                                        if let Some(c) = e.piper {
                                            ui.label(
                                                RichText::new(format!(
                                                    "+ The Prismatic Piper ({})",
                                                    c
                                                ))
                                                .small()
                                                .color(Color32::from_rgb(200, 170, 255)),
                                            );
                                        }
                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new("plays").small());
                                            identity_dots(ui, e.plays, 12.0);
                                            ui.label(RichText::new(e.archetype.label()).small().weak());
                                        });
                                        if let Some((place, score)) = ranks.singleton_place(&e.commander) {
                                            let moved = place as i64 - (rank as i64 + 1);
                                            let color = if moved > 0 {
                                                Color32::from_rgb(230, 140, 110)
                                            } else if moved < 0 {
                                                Color32::from_rgb(140, 210, 140)
                                            } else {
                                                Color32::GRAY
                                            };
                                            ui.label(
                                                RichText::new(format!("Singleton: #{} ({:.0})", place, score))
                                                    .small()
                                                    .color(color),
                                            )
                                            .on_hover_text("Where this commander ranks once singleton starts (Session 6), with your current pool");
                                        }
                                    });
                                });
                                // Part bars on the common scale: green above your average option, red below.
                                for (label, v) in
                                    crate::ranking::Parts::LABELS.iter().zip(e.scaled)
                                {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(format!("{:<11}", label))
                                                .monospace()
                                                .size(11.0),
                                        );
                                        let (rect, _) = ui
                                            .allocate_exact_size(vec2(110.0, 10.0), Sense::hover());
                                        let mid = rect.left() + 30.0;
                                        ui.painter().rect_filled(
                                            rect,
                                            CornerRadius::from(2.0),
                                            Color32::from_gray(45),
                                        );
                                        let len = (v.abs() * 22.0).min(if v >= 0.0 {
                                            80.0
                                        } else {
                                            30.0
                                        });
                                        let bar = if v >= 0.0 {
                                            egui::Rect::from_min_max(
                                                pos2(mid, rect.top()),
                                                pos2(mid + len, rect.bottom()),
                                            )
                                        } else {
                                            egui::Rect::from_min_max(
                                                pos2(mid - len, rect.top()),
                                                pos2(mid, rect.bottom()),
                                            )
                                        };
                                        ui.painter().rect_filled(
                                            bar,
                                            CornerRadius::from(2.0),
                                            if v >= 0.0 {
                                                Color32::from_rgb(90, 190, 110)
                                            } else {
                                                Color32::from_rgb(220, 90, 80)
                                            },
                                        );
                                        ui.label(RichText::new(format!("{:+.1}", v)).small());
                                    });
                                }
                                ui.label(
                                    RichText::new(format!(
                                        "{} creatures · {} removal · {} draw · {:.0}% keepable",
                                        e.creatures,
                                        e.removal,
                                        e.draw,
                                        e.keepable * 100.0
                                    ))
                                    .small(),
                                );
                                if group.len() > 1 {
                                    let others: Vec<String> = group[1..]
                                        .iter()
                                        .map(|o| match o.piper {
                                            Some(c) => format!("+Piper({}) {:.0}", c, o.total),
                                            None => format!("alone {:.0}", o.total),
                                        })
                                        .collect();
                                    ui.label(
                                        RichText::new(format!("Also: {}", others.join(" · ")))
                                            .small()
                                            .weak(),
                                    );
                                }
                                egui::CollapsingHeader::new(RichText::new("Why?").small())
                                    .id_salt(("why", &e.commander))
                                    .show(ui, |ui| {
                                        for n in &e.notes {
                                            ui.label(RichText::new(format!("• {}", n)).small());
                                        }
                                    });
                                ui.horizontal(|ui| {
                                    if ui
                                        .button("Build")
                                        .on_hover_text("Build this exact deck")
                                        .clicked()
                                    {
                                        acts.push(Action::AutoBuild(e.commander.clone(), e.piper));
                                    }
                                    if ui
                                        .button("Set")
                                        .on_hover_text("Make this your commander and build by hand")
                                        .clicked()
                                    {
                                        acts.push(Action::SetCommander(e.commander.clone()));
                                        if let Some(c) = e.piper {
                                            acts.push(Action::PiperColor(c));
                                        }
                                        self.tab = Tab::Build;
                                    }
                                });
                            });
                        });
                    }
                });
            });
    }

    // ------------------------------------------------------------------ draft

    fn name_search(
        &self,
        ui: &mut Ui,
        query_id: &str,
        query: &mut String,
        whole_set: bool,
    ) -> Option<String> {
        let mut chosen = None;
        let resp = ui.add(
            egui::TextEdit::singleline(query)
                .id_salt(query_id)
                .hint_text("Type a card name…")
                .desired_width(320.0),
        );
        let q = crate::db::normalize(query);
        if q.len() >= 2 {
            let hits: Vec<&Card> = self
                .db
                .cards
                .iter()
                .filter(|c| !c.is_basic() && (whole_set || self.coll.owned(&c.name) > 0))
                .filter(|c| crate::db::normalize(&c.name).contains(&q))
                .take(10)
                .collect();
            for c in hits {
                ui.horizontal(|ui| {
                    if ui.button("Add").clicked() {
                        chosen = Some(c.name.clone());
                    }
                    ui.label(
                        RichText::new(&c.name)
                            .color(cards::lighten_for_text(cards::frame_color(c))),
                    );
                    cost_widget(ui, &c.mana_cost, 14.0);
                    ui.label(
                        RichText::new(format!("{} · {}", c.set, c.rarity))
                            .small()
                            .weak(),
                    );
                });
            }
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                if let Lookup::Found(c) = self.db.lookup(query) {
                    chosen = Some(c.name.clone());
                }
            }
        }
        chosen
    }

    fn draft_tab(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        ui.heading("Draft helper");
        ui.label(
            "At a draft, add the cards you see in the pack. They're ranked for your deck: fit with your commander first, \
             then trade value (VP) and raw strength - worth taking so a table-mate doesn't get it.",
        );
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(380.0);
                ui.label(RichText::new("Add a card from the pack").strong());
                let mut q = std::mem::take(&mut self.draft_query);
                if let Some(n) = self.name_search(ui, "draft_q", &mut q, true) {
                    acts.push(Action::DraftAdd(n));
                    q.clear();
                }
                self.draft_query = q;
                ui.add_space(8.0);
                if ui.add_enabled(!self.draft_pack.is_empty(), egui::Button::new("Clear pack")).clicked() {
                    self.draft_pack.clear();
                }
                ui.label(RichText::new(format!("{} cards in this pack", self.draft_pack.len())).small());
                if self.deck.commander.is_none() {
                    let text = if self.rank_job.is_some() {
                        "No commander chosen: working out your best commanders (see the Commanders tab), then picks are judged against their decks."
                    } else {
                        "No commander chosen, so cards are judged against the decks of your three best-ranked commanders."
                    };
                    ui.label(RichText::new(text).small().weak());
                }
            });
            ui.separator();
            if self.deck.commander.is_none() && (self.ranking.is_none() || self.ranking_key != self.ranking_key()) {
                self.start_ranking();
            }
            let stale = self.memo.draft.as_ref().is_none_or(|(pack, _)| *pack != self.draft_pack);
            if stale {
                let pack: Vec<&Card> = self.draft_pack.iter().filter_map(|n| self.db.get(n)).collect();
                let options: Vec<Deck> = match (&self.deck.commander, &self.ranking) {
                    (None, Some(r)) => crate::ranking::best_per_commander(&r.now)
                        .iter()
                        .take(3)
                        .map(|g| g[0].deck.clone())
                        .collect(),
                    _ => Vec::new(),
                };
                let ranked = league::rank_pack(&pack, &self.deck, &self.db, &self.coll, &self.rules, &options)
                    .into_iter()
                    .map(|p| (p.card.clone(), p.total, p.fit, p.vp, p.reasons))
                    .collect();
                self.memo.draft = Some((self.draft_pack.clone(), ranked));
            }
            let picks: Vec<(Card, f32, f32, u32, Vec<String>)> =
                self.memo.draft.as_ref().map(|(_, r)| r.clone()).unwrap_or_default();
            ScrollArea::vertical().id_salt("draft_scroll").auto_shrink([false, false]).show(ui, |ui| {
                ui.with_layout(Layout::left_to_right(Align::Min).with_main_wrap(true), |ui| {
                    ui.spacing_mut().item_spacing = vec2(12.0, 12.0);
                    for (rank, (card, total, fit, vp, why)) in picks.iter().enumerate() {
                        ui.allocate_ui(vec2(190.0, 420.0), |ui| {
                            ui.vertical(|ui| {
                                                                let resp = self.show_card(
                                    ui,
                                    CardLook {
                                        card,
                                        width: 180.0,
                                        image: None,
                                        badge: Some(if rank == 0 { "Pick #1".into() } else { format!("#{}", rank + 1) },
                                ),
                                        dimmed: false,
                                        selected: rank == 0,
                                    },
                                );
                                if resp.clicked() {
                                    acts.push(Action::Select(card.name.clone()));
                                }
                                ui.label(RichText::new(format!("Score {:.1} · fit {:.1} · {} VP", total, fit, vp)).strong().small());
                                for w in why.iter().take(4) {
                                    ui.label(RichText::new(format!("• {}", w)).small());
                                }
                                ui.horizontal(|ui| {
                                    if ui.button("I took it").on_hover_text("Adds it to your pool").clicked() {
                                        acts.push(Action::DraftTook(card.name.clone()));
                                    }
                                    if ui.button("x").on_hover_text("Remove from the pack").clicked() {
                                        if let Some(i) = self.draft_pack.iter().position(|n| *n == card.name) {
                                            acts.push(Action::DraftRemove(i));
                                        }
                                    }
                                });
                            });
                        });
                    }
                });
            });
        });
    }

    // ------------------------------------------------------------------ trades & VP

    fn trade_tab(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        ui.heading("Trades & Value Points");
        ui.columns(2, |cols| {
            let ui = &mut cols[0];
            ui.label(RichText::new("Trade-in plan for this round").strong().size(16.0));
            ui.label("Each round you must trade in at least 10 VP; any shortfall becomes debt. Only spare copies are suggested - cards in your deck are kept.");
            ui.horizontal(|ui| {
                ui.label("Target VP");
                ui.add(egui::DragValue::new(&mut self.trade_target).range(1..=60));
                ui.label("Planning to claim:");
                ui.selectable_value(&mut self.trade_need, 0, "nothing rare");
                ui.selectable_value(&mut self.trade_need, 1, "a rare");
                ui.selectable_value(&mut self.trade_need, 2, "a mythic");
            });
            let need = match self.trade_need {
                1 => Some("rare"),
                2 => Some("mythic"),
                _ => None,
            };
            let key = (self.trade_target, self.trade_need);
            if self.memo.trade.as_ref().is_none_or(|(k, _)| *k != key) {
                let plan = league::plan_trade_in(&self.deck, &self.db, &self.coll, &self.rules, self.trade_target, need)
                    .into_iter()
                    .map(|t| (t.card.clone(), t.copies, t.vp, t.why))
                    .collect();
                self.memo.trade = Some((key, plan));
            }
            let plan: Vec<(Card, u32, u32, String)> =
                self.memo.trade.as_ref().map(|(_, p)| p.clone()).unwrap_or_default();
            let total: u32 = plan.iter().map(|p| p.2).sum();
            ui.label(
                RichText::new(format!("Plan: {} VP", total))
                    .strong()
                    .color(if total >= self.trade_target { Color32::from_rgb(130, 220, 130) } else { Color32::from_rgb(255, 150, 110) }),
            );
            if need.is_some() {
                ui.label(RichText::new("Rarity floor: claiming a rare needs a rare traded in; a mythic needs a mythic or two rares.").small());
            }
            ScrollArea::vertical().id_salt("trade_plan").max_height(ui.available_height() - 10.0).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (card, copies, vp, why) in &plan {
                        ui.vertical(|ui| {
                                                        let resp = self.show_card(
                                ui,
                                CardLook { card, width: 130.0, image: None, badge: Some(format!("{} VP", vp)), dimmed: false, selected: false },
                            );
                            if resp.clicked() {
                                acts.push(Action::Select(card.name.clone()));
                            }
                            ui.label(RichText::new(format!("{}x · {}", copies, why)).small());
                        });
                    }
                });
            });

            let ui = &mut cols[1];
            let total_vp: u32 = self
                .coll
                .counts
                .iter()
                .filter_map(|(n, c)| self.db.get(n).map(|x| league::vp_value(x) * c))
                .sum();
            ui.label(RichText::new("Values").strong().size(16.0));
            ui.label(format!("Your whole pool is worth {} VP if traded in.", total_vp));
            egui::Grid::new("vp_grid").striped(true).show(ui, |ui| {
                ui.label("Mythic");
                ui.label("8 VP in · 16 VP to claim");
                ui.end_row();
                ui.label("Rare");
                ui.label("4 VP in · 8 VP to claim");
                ui.end_row();
                ui.label("Uncommon");
                ui.label("2 VP in · 4 VP to claim");
                ui.end_row();
                ui.label("Common");
                ui.label("1 VP in · 2 VP to claim");
                ui.end_row();
                ui.label("Each game you win");
                ui.label("+5 VP");
                ui.end_row();
            });
            ui.add_space(8.0);
            ui.label(RichText::new("Skill Tree staples (claim in any order)").strong());
            ui.label(RichText::new("Each one costs 5 VP more than the last: 5, 10, 15 … 45 (225 for all nine). Suggested order for value:").small());
            egui::Grid::new("staples").striped(true).show(ui, |ui| {
                for (k, s) in league::STAPLES.iter().enumerate() {
                    ui.label(format!("{}.", k + 1));
                    ui.label(*s);
                    ui.label(format!("{} VP", league::staple_cost(k as u32 + 1)));
                    ui.end_row();
                }
            });
            ui.add_space(8.0);
            ui.label(RichText::new("Shop (30 VP each, one purchase per session)").strong());
            let id = self.deck.identity(&self.db).unwrap_or(ColorSet(0b11111));
            for (colour, items) in league::SHOP {
                let relevant = match colour {
                    "Land" | "Colorless" => true,
                    c => ColorSet::parse(c).map(|s| s.is_subset_of(id)).unwrap_or(false),
                };
                ui.label(
                    RichText::new(format!("{}: {}", colour, items.join(", ")))
                        .small()
                        .color(if relevant { Color32::from_rgb(200, 230, 200) } else { Color32::from_gray(120) }),
                );
            }
        });
    }

    // ------------------------------------------------------------------ upgrades

    fn upgrade_row(&mut self, ui: &mut Ui, u: &crate::upgrades::Upgrade, acts: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{:+.1}", u.gain))
                    .strong()
                    .color(Color32::from_rgb(140, 210, 140)),
            )
            .on_hover_text(format!(
                "Scores {:.1} better than {}, your weakest card",
                u.gain, u.replaces
            ));
            let resp = ui.add(
                egui::Label::new(
                    RichText::new(&u.card.name)
                        .color(cards::lighten_for_text(cards::frame_color(&u.card))),
                )
                .sense(Sense::click()),
            );
            let resp = self.with_zoom(resp, &u.card, None);
            if resp.clicked() {
                acts.push(Action::Select(u.card.name.clone()));
            }
            cost_widget(ui, &u.card.mana_cost, 13.0);
            ui.label(RichText::new(&u.card.rarity).small().weak());
        });
        let detail = match &u.source {
            crate::upgrades::Source::Pool { cost, floor } => format!(
                "Claim for {} VP{} · replaces {}",
                cost,
                floor
                    .map(|f| format!(" (trade in {} that round)", f))
                    .unwrap_or_default(),
                u.replaces
            ),
            crate::upgrades::Source::Packs {
                set_name,
                packs,
                usable_from,
                you,
                table,
                league,
            } => format!(
                "{} · {}, usable from Session {}\nYou open it {:.0}% · your table {:.0}% · anyone in the league {:.0}%",
                set_name,
                if *packs == 0 {
                    "not in your packs - draft it from a table-mate".to_string()
                } else {
                    format!("{} of your packs", packs)
                },
                usable_from,
                you * 100.0,
                table * 100.0,
                league * 100.0
            ),
        };
        ui.label(RichText::new(detail).small());
        if !u.reasons.is_empty() {
            ui.label(
                RichText::new(
                    u.reasons
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" · "),
                )
                .small()
                .weak(),
            );
        }
        ui.add_space(6.0);
    }

    fn upgrades_tab(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        ui.heading("Upgrades for this deck");
        if self.deck.commander.is_none() {
            ui.label("Choose a commander first (Commanders tab), then this shows the cards that would improve its deck and how to get them.");
            return;
        }
        ui.label(
            "Cards you don't have yet that score better than your deck's weakest card: what's in the Public Pool to claim now, \
             and what could come in the league packs still to be handed out, with the odds of opening it.",
        );
        let next = league::upcoming_session().number;
        if self.memo.upgrades.is_none() {
            self.memo.upgrades = Some(crate::upgrades::upgrades(
                &self.deck,
                &self.db,
                &self.coll,
                &self.rules,
                &self.public_pool,
                next,
            ));
        }
        let ups = self.memo.upgrades.clone().unwrap_or_default();
        let (pool, packs): (Vec<_>, Vec<_>) = ups
            .into_iter()
            .partition(|u| matches!(u.source, crate::upgrades::Source::Pool { .. }));
        ui.columns(2, |cols| {
            let ui = &mut cols[0];
            ui.label(RichText::new("Claim from the Public Pool now").strong().size(16.0));
            ui.label(
                RichText::new(format!(
                    "Public Pool list: {} cards ({} different). Claims cost 2x the card's VP; rares and mythics need a rare/mythic traded in the same round.",
                    self.public_pool.total(),
                    self.public_pool.counts.len()
                ))
                .small(),
            );
            egui::CollapsingHeader::new("Update the Public Pool list")
                .default_open(self.public_pool.counts.is_empty())
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("Copy the Public Pool from Mana Mixer's trade page and paste it here (Ctrl+V), or drop the file on the box.")
                            .small(),
                    );
                    ScrollArea::vertical().id_salt("public_scroll").max_height(140.0).show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.public_text)
                                .hint_text("1 Arbiter of Woe\n1 Incinerating Blast\n...")
                                .desired_rows(5)
                                .desired_width(f32::INFINITY)
                                .font(egui::TextStyle::Monospace),
                        );
                    });
                    let n = league::parse_import(&self.public_text).len();
                    if ui.add_enabled(n > 0, egui::Button::new(format!("Save as the Public Pool ({} cards)", n))).clicked() {
                        match crate::upgrades::save_public_pool(&self.data_dir, &self.public_text) {
                            Ok(n) => {
                                self.public_pool = crate::upgrades::load_public_pool(&self.data_dir);
                                self.public_text.clear();
                                self.memo.upgrades = None;
                                self.status = format!("Saved {} different cards as the Public Pool.", n);
                            }
                            Err(e) => self.status = e,
                        }
                    }
                });
            ui.add_space(6.0);
            ScrollArea::vertical().id_salt("pool_ups").auto_shrink([false, false]).show(ui, |ui| {
                if pool.is_empty() {
                    ui.label(RichText::new(if self.public_pool.counts.is_empty() {
                        "No Public Pool list yet."
                    } else {
                        "Nothing in the pool beats your weakest card."
                    }).weak());
                }
                if let Some(top) = pool.first() {
                    let plan = crate::upgrades::trade_ins_for(top, &self.deck, &self.db, &self.coll, &self.rules);
                    let vp: u32 = plan.iter().map(|t| t.vp).sum();
                    ui.label(
                        RichText::new(format!(
                            "To claim {} this round, trade in: {} ({} VP)",
                            top.card.name,
                            plan.iter().map(|t| format!("{}x {}{}", t.copies, t.card.name, crate::league::deck_note(t))).collect::<Vec<_>>().join(", "),
                            vp
                        ))
                        .small()
                        .color(Color32::from_rgb(255, 205, 80)),
                    );
                    ui.add_space(6.0);
                }
                for u in pool.iter().take(25) {
                    self.upgrade_row(ui, u, acts);
                }
            });
            let ui = &mut cols[1];
            ui.label(RichText::new(format!("Could come in packs (Session {} on)", next)).strong().size(16.0));
            ui.label(
                RichText::new(
                    "\"Your table\" = opened at your 4-player draft table, so it's in your face-up draft. \"League\" = opened by any of the 8 \
                     players, who might trade it into the pool. Who opens Marvel or Hobbit packs comes from Season & Rules > League starter \
                     packs; your table is you plus 3 of the other 7 (any seating equally likely). Odds use typical Play Booster rarity rates. \
                     Nauctis cards appear once it's released and added (Settings > Update card database).",
                )
                .small(),
            );
            ui.add_space(6.0);
            ScrollArea::vertical().id_salt("pack_ups").auto_shrink([false, false]).show(ui, |ui| {
                if packs.is_empty() {
                    ui.label(RichText::new("Nothing found.").weak());
                }
                for u in packs.iter().take(40) {
                    self.upgrade_row(ui, u, acts);
                }
            });
        });
    }

    // ------------------------------------------------------------------ stats

    fn stats_tab(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let st = analysis::stats(&self.deck, &self.db);
        let plan = analysis::deck_plan(&self.deck, &self.db, &self.rules);
        let targets = plan.curve;
        ui.heading("Stats & playtest");
        ScrollArea::vertical()
            .id_salt("stats_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.columns(3, |cols| {
                    // Mana curve chart.
                    let ui = &mut cols[0];
                    ui.label(RichText::new("Mana curve (non-land)").strong());
                    let (rect, _) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 180.0), Sense::hover());
                    let max = st
                        .curve
                        .iter()
                        .chain(targets.iter())
                        .copied()
                        .max()
                        .unwrap_or(1)
                        .max(1) as f32;
                    let bw = rect.width() / 6.0;
                    for b in 0..6 {
                        let x = rect.left() + b as f32 * bw;
                        let h = (rect.height() - 34.0) * st.curve[b] as f32 / max;
                        let bar = egui::Rect::from_min_max(
                            pos2(x + 8.0, rect.bottom() - 18.0 - h),
                            pos2(x + bw - 8.0, rect.bottom() - 18.0),
                        );
                        ui.painter().rect_filled(
                            bar,
                            CornerRadius::from(3.0),
                            Color32::from_rgb(90, 150, 230),
                        );
                        let ty =
                            rect.bottom() - 18.0 - (rect.height() - 34.0) * targets[b] as f32 / max;
                        ui.painter().line_segment(
                            [pos2(x + 4.0, ty), pos2(x + bw - 4.0, ty)],
                            Stroke::new(2.0, Color32::from_rgb(255, 205, 80)),
                        );
                        ui.painter().text(
                            pos2(x + bw / 2.0, bar.top() - 2.0),
                            egui::Align2::CENTER_BOTTOM,
                            st.curve[b].to_string(),
                            FontId::proportional(13.0),
                            Color32::WHITE,
                        );
                        ui.painter().text(
                            pos2(x + bw / 2.0, rect.bottom()),
                            egui::Align2::CENTER_BOTTOM,
                            curve_label(b),
                            FontId::proportional(12.0),
                            Color32::GRAY,
                        );
                    }
                    ui.label(
                        RichText::new(format!(
                            "Average mana value {:.2} · yellow lines = target for a {} deck",
                            st.avg_mv,
                            plan.archetype.label()
                        ))
                        .small(),
                    )
                    .on_hover_text(format!(
                        "Read from your commander: {}. Aim for {}-{} creatures, about {:.0} removal and {:.0} card draw.",
                        plan.signals.join(", "),
                        plan.creatures.0,
                        plan.creatures.1,
                        plan.removal,
                        plan.draw
                    ));
                    ui.add_space(6.0);
                    ui.label(RichText::new("Card types").strong());
                    for (t, n) in &st.types {
                        ui.label(format!("{}: {}", t, n));
                    }

                    // Colours.
                    let ui = &mut cols[1];
                    ui.label(RichText::new("Colour needs vs. mana sources").strong());
                    let id = self.deck.identity(&self.db).unwrap_or_default();
                    for i in 0..5 {
                        if st.pips[i] == 0 && !id.contains(i) {
                            continue;
                        }
                        ui.horizontal(|ui| {
                            let (fill, _) = cards::symbol_colors(&COLORS[i].to_string());
                            let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                            ui.painter().circle_filled(r.center(), 7.0, fill);
                            ui.painter().circle_stroke(
                                r.center(),
                                7.0,
                                Stroke::new(1.0, Color32::from_gray(150)),
                            );
                            ui.label(format!(
                                "{}: {} pips · {} sources",
                                analysis::color_name(i),
                                st.pips[i],
                                st.sources[i]
                            ));
                        });
                    }
                    let rec = analysis::recommended_lands(st.avg_mv, &self.rules);
                    let (lo, hi) = analysis::land_range(&self.rules);
                    ui.label(format!(
                        "Lands: {} · recommended {} (league range {}-{})",
                        st.lands, rec, lo, hi
                    ));
                    ui.add_space(6.0);
                    ui.label(RichText::new("What the deck does").strong());
                    for r in Role::ALL {
                        if let Some(n) = st.roles.get(&r) {
                            ui.label(format!("{}: {}", r.label(), n));
                        }
                    }
                    let removal = *st.roles.get(&Role::Removal).unwrap_or(&0);
                    if removal < 5 {
                        ui.colored_label(
                            Color32::from_rgb(255, 190, 110),
                            format!("Only {} removal - aim for 5+.", removal),
                        );
                    }
                    if st.creatures < 14 && st.nonlands > 20 {
                        ui.colored_label(
                            Color32::from_rgb(255, 190, 110),
                            format!("{} creatures is low - aim for 15+.", st.creatures),
                        );
                    }

                    // Weakest cards.
                    let ui = &mut cols[2];
                    ui.label(RichText::new("Weakest cards (cut candidates)").strong());
                    let weakest = self
                        .memo
                        .weakest
                        .get_or_insert_with(|| {
                            analysis::weakest(&self.deck, &self.db, &self.rules)
                                .iter()
                                .take(10)
                                .map(|s| (s.card.name.clone(), s.score))
                                .collect()
                        })
                        .clone();
                    for (name, score) in weakest {
                        ui.horizontal(|ui| {
                            ui.label(format!("{:.1}", score));
                            if ui.link(&name).clicked() {
                                acts.push(Action::Select(name.clone()));
                                self.tab = Tab::Build;
                            }
                        });
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Simulate 20,000 opening hands").clicked() {
                        acts.push(Action::Simulate);
                    }
                    if ui.button("Draw a sample hand").clicked() {
                        acts.push(Action::NewHand);
                    }
                    if !self.hand.is_empty() && ui.button("Draw a card").clicked() {
                        acts.push(Action::Draw);
                    }
                });
                if let Some(r) = &self.sim {
                    ui.columns(2, |cols| {
                        let ui = &mut cols[0];
                        ui.label(RichText::new("Lands in opening hand").strong());
                        for (k, n) in r.land_hist.iter().enumerate() {
                            let p = *n as f32 / r.trials as f32;
                            if *n > 0 {
                                ui.horizontal(|ui| {
                                    ui.label(format!("{} lands", k));
                                    ui.add(
                                        egui::ProgressBar::new(p)
                                            .desired_width(220.0)
                                            .text(format!("{:.0}%", p * 100.0)),
                                    );
                                });
                            }
                        }
                        ui.label(
                            RichText::new(format!(
                                "Keepable (2-5 lands): {:.0}%",
                                r.keepable * 100.0
                            ))
                            .strong(),
                        );
                        let ui = &mut cols[1];
                        ui.label(RichText::new("Hitting land drops (on the play)").strong());
                        for k in 0..6 {
                            ui.label(format!(
                                "{} lands by turn {}: {:.0}%",
                                k + 1,
                                k + 1,
                                r.land_drops[k] * 100.0
                            ));
                        }
                        if let Some((mv, p)) = r.commander_on_curve {
                            ui.label(
                                RichText::new(format!(
                                    "Commander castable on turn {}: {:.0}%",
                                    mv,
                                    p * 100.0
                                ))
                                .strong(),
                            );
                        }
                    });
                }
                if !self.hand.is_empty() {
                    let lands = self
                        .hand
                        .iter()
                        .filter(|n| self.db.get(n).is_some_and(|c| c.is_land()))
                        .count();
                    ui.label(
                        RichText::new(format!(
                            "Sample hand: {} cards, {} lands",
                            self.hand.len(),
                            lands
                        ))
                        .strong(),
                    );
                    ui.horizontal_wrapped(|ui| {
                        for n in self.hand.clone() {
                            if let Some(c) = self.db.get(&n).cloned() {
                                let resp = self.show_card(
                                    ui,
                                    CardLook {
                                        card: &c,
                                        width: 140.0,
                                        image: None,
                                        badge: None,
                                        dimmed: false,
                                        selected: false,
                                    },
                                );
                                if resp.clicked() {
                                    acts.push(Action::Select(c.name.clone()));
                                }
                            }
                        }
                    });
                }
            });
    }

    // ------------------------------------------------------------------ season

    fn season_tab(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        ScrollArea::vertical()
            .id_salt("season_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| self.season_body(ui, acts));
    }

    fn season_body(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        ui.heading("Season 2 schedule & rules");
        let next = league::upcoming_session().number;
        egui::Grid::new("schedule")
            .striped(true)
            .spacing(vec2(16.0, 6.0))
            .show(ui, |ui| {
                for h in ["#", "Date", "Deck", "What happens", ""] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for s in &league::SESSIONS {
                    let (size, single) = s.deck_rules();
                    let using = s.number == self.rules.session;
                    let mark = |t: String| {
                        if using {
                            RichText::new(t)
                                .strong()
                                .color(Color32::from_rgb(255, 205, 80))
                        } else {
                            RichText::new(t)
                        }
                    };
                    ui.label(mark(s.number.to_string()));
                    ui.label(mark(s.date_label()));
                    ui.label(mark(format!(
                        "{} cards{}",
                        size,
                        if single { ", singleton" } else { "" }
                    )));
                    ui.label(mark(format!(
                        "{}{}",
                        s.label,
                        if s.number == next { "  (next)" } else { "" }
                    )));
                    if using {
                        ui.label(RichText::new("rules in use").small());
                    } else if ui.small_button("Build for this").clicked() {
                        acts.push(Action::Session(s.number));
                    }
                    ui.end_row();
                }
            });
        ui.add_space(10.0);
        ui.label(RichText::new("Rules (all season)").strong().size(16.0));
        ui.label("• Commander(s) count toward the deck size. • Cards only from your league pool. • Basic lands are free. • Colour identity applies.");
        ui.label("• The Prismatic Piper may partner with a mono-colour commander to add one colour (never a 3rd).");
        ui.label(format!(
            "• Land advice: {}-{} lands at this deck size.",
            analysis::land_range(&self.rules).0,
            analysis::land_range(&self.rules).1
        ));
        ui.add_space(6.0);
        egui::CollapsingHeader::new(RichText::new("League starter packs (who opens Marvel / Hobbit)").strong())
            .default_open(false)
            .show(ui, |ui| {
                ui.label(
                    RichText::new(
                        "Used for pack odds on the Upgrades tab (Hobbit cards opened at your table can be drafted) \
                         and for the simulated opponents in commander playtests.",
                    )
                    .small(),
                );
                let mut changed = false;
                let roster = &mut self.rules.roster;
                egui::Grid::new("roster").striped(true).spacing(vec2(12.0, 4.0)).show(ui, |ui| {
                    for h in ["Player", "Kickoff Marvel", "Kickoff Hobbit", "Drafted packs (Sessions 2-4)"] {
                        ui.label(RichText::new(h).small().strong());
                    }
                    ui.end_row();
                    for (i, p) in std::iter::once(&mut roster.me).chain(roster.others.iter_mut()).enumerate() {
                        changed |= ui
                            .add_sized(
                                [140.0, 20.0],
                                egui::TextEdit::singleline(&mut p.name).interactive(i > 0),
                            )
                            .changed();
                        if ui.add(egui::DragValue::new(&mut p.kickoff_marvel).range(0..=9)).changed() {
                            p.kickoff_hobbit = 9 - p.kickoff_marvel;
                            changed = true;
                        }
                        ui.label(p.kickoff_hobbit.to_string());
                        ui.horizontal(|ui| {
                            changed |= ui.selectable_value(&mut p.weekly, "MSH".to_string(), "Marvel").changed();
                            changed |= ui.selectable_value(&mut p.weekly, "HOB".to_string(), "Hobbit").changed();
                        });
                        ui.end_row();
                    }
                });
                if ui.small_button("Reset to the Season 2 starting mix").clicked() {
                    *roster = crate::packs::Roster::default();
                    changed = true;
                }
                if changed {
                    let _ = self.rules.save(&self.data_dir.join("rules.json"));
                    self.rev += 1;
                }
            });
        self.opponents_section(ui);
        ui.collapsing("Advanced: change rules by hand", |ui| {
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label("Deck size");
                changed |= ui
                    .add(egui::DragValue::new(&mut self.rules.deck_size).range(40..=100))
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Players per game");
                changed |= ui
                    .add(egui::DragValue::new(&mut self.rules.players).range(2..=6))
                    .changed();
                ui.label(
                    RichText::new(
                        "(3+ values board wipes, card draw and \"each opponent\" effects more)",
                    )
                    .small()
                    .weak(),
                );
            });
            changed |= ui
                .checkbox(
                    &mut self.rules.singleton,
                    "Singleton (one of each card except basics)",
                )
                .changed();
            changed |= ui
                .checkbox(
                    &mut self.rules.commander_counts_toward_size,
                    "Commander counts toward deck size",
                )
                .changed();
            changed |= ui
                .checkbox(
                    &mut self.rules.enforce_color_identity,
                    "Enforce colour identity",
                )
                .changed();
            changed |= ui
                .checkbox(
                    &mut self.rules.enforce_collection,
                    "Only cards from my pool",
                )
                .changed();
            changed |= ui
                .checkbox(
                    &mut self.rules.piper_free,
                    "The Prismatic Piper doesn't need to be in my pool",
                )
                .changed();
            if changed {
                acts.push(Action::RulesChanged);
            }
        });
        let dupes = league::singleton_conflicts(&self.deck, &self.db);
        if !dupes.is_empty() {
            ui.add_space(8.0);
            ui.label(
                RichText::new("Before singleton (Session 6), cut these extra copies:").strong(),
            );
            for (n, c) in dupes {
                ui.label(format!("{} x {}", c, n));
            }
        }
    }

    // ------------------------------------------------------------------ collection

    fn collection_tab(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        ui.heading("My league pool");
        ui.horizontal(|ui| {
            ui.label(format!(
                "{} cards ({} different).",
                self.coll.total(),
                self.coll.counts.len()
            ));
            if ui.button("Copy pool for Mana Mixer").clicked() {
                acts.push(Action::Copy(
                    league::export_pool(&self.db, &self.coll),
                    "your pool list",
                ));
            }
        });
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(380.0);
                ui.label(RichText::new("Add cards you opened or traded for").strong());
                let mut q = std::mem::take(&mut self.coll_query);
                if let Some(n) = self.name_search(ui, "coll_q", &mut q, true) {
                    acts.push(Action::Own(n, 1));
                }
                self.coll_query = q;
                ui.add_space(14.0);
                ui.separator();
                ui.label(RichText::new("Import from Mana Mixer").strong());
                ui.label(
                    RichText::new(
                        "Drag your Mana Mixer (or ManaBox) export file onto this window, or paste it below with Ctrl+V - lines like \"1 Red Hulk (MSH) 149\" or a CSV with Name and Quantity columns.",
                    )
                    .small(),
                );
                // Buttons above the box, so a long list can't push them off the screen.
                let parsed = league::parse_import(&self.import_text);
                let total: u32 = parsed.iter().map(|l| l.count).sum();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!parsed.is_empty(), egui::Button::new("Add to my pool"))
                        .on_hover_text("Adds these cards to what you already have")
                        .clicked()
                    {
                        acts.push(Action::Import(self.import_text.clone(), false));
                    }
                    if ui
                        .add_enabled(!parsed.is_empty(), egui::Button::new("Replace my pool"))
                        .on_hover_text("Your pool becomes exactly this list (the old one is saved as data/collection.backup.txt)")
                        .clicked()
                    {
                        acts.push(Action::Import(self.import_text.clone(), true));
                    }
                    if ui
                        .add_enabled(!self.import_text.is_empty(), egui::Button::new("Clear"))
                        .clicked()
                    {
                        self.import_text.clear();
                    }
                });
                if !parsed.is_empty() {
                    ui.label(
                        RichText::new(format!("{} cards ({} different) ready to import.", total, parsed.len()))
                            .small()
                            .color(Color32::from_rgb(150, 220, 150)),
                    );
                }
                ScrollArea::vertical()
                    .id_salt("import_scroll")
                    .max_height(320.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.import_text)
                                .hint_text("1 Red Hulk (MSH) 149\n2 Web Up (MSH) 41\n...")
                                .desired_rows(8)
                                .desired_width(370.0)
                                .font(egui::TextStyle::Monospace),
                        );
                    });
            });
            ui.separator();
            let mut rows: Vec<(String, u32)> = self
                .coll
                .counts
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect();
            rows.sort_by(|a, b| a.0.cmp(&b.0));
            ScrollArea::vertical()
                .id_salt("coll_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Grid::new("coll").striped(true).show(ui, |ui| {
                        for (name, n) in rows {
                            let card = self.db.get(&name).cloned();
                            if ui.small_button("-").clicked() {
                                acts.push(Action::Own(name.clone(), -1));
                            }
                            ui.label(RichText::new(n.to_string()).monospace());
                            if ui.small_button("+").clicked() {
                                acts.push(Action::Own(name.clone(), 1));
                            }
                            match &card {
                                Some(c) => {
                                    let resp =
                                        ui.add(
                                            egui::Label::new(RichText::new(&name).color(
                                                cards::lighten_for_text(cards::frame_color(c)),
                                            ))
                                            .sense(Sense::click()),
                                        );
                                    let resp = self.with_zoom(resp, c, None);
                                    if resp.clicked() {
                                        acts.push(Action::Select(name.clone()));
                                    }
                                    cost_widget(ui, &c.mana_cost, 14.0);
                                    ui.label(RichText::new(&c.rarity).small());
                                    ui.label(
                                        RichText::new(format!("{} VP", league::vp_value(c)))
                                            .small(),
                                    );
                                }
                                None => {
                                    ui.colored_label(
                                        Color32::YELLOW,
                                        format!("{} (not in card database)", name),
                                    );
                                    ui.label("");
                                    ui.label("");
                                    ui.label("");
                                }
                            }
                            ui.end_row();
                        }
                    });
                });
        });
    }
}

impl GuiApp {
    /// A card list dragged onto the window goes into the My Pool import box, ready to check
    /// and import; while a file hovers over the window, a hint says where it will go.
    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let (hovering, dropped) =
            ctx.input(|i| (!i.raw.hovered_files.is_empty(), i.raw.dropped_files.clone()));
        if hovering {
            let screen = ctx.content_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::unique("drop_hint"),
            ));
            painter.rect_filled(screen, CornerRadius::ZERO, Color32::from_black_alpha(190));
            painter.text(
                screen.center(),
                egui::Align2::CENTER_CENTER,
                "Drop your Mana Mixer file to import your pool",
                FontId::proportional(28.0),
                Color32::WHITE,
            );
        }
        if dropped.is_empty() {
            return;
        }
        let mut text = String::new();
        let mut names = Vec::new();
        for f in &dropped {
            let label = f
                .path()
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| f.path().display().to_string());
            match f.bytes() {
                Ok(b) => {
                    text += &league::decode_text(&b);
                    text.push('\n');
                    names.push(label);
                }
                Err(e) => self.status = format!("Couldn't read {}: {}", label, e),
            }
        }
        let parsed = league::parse_import(&text);
        if parsed.is_empty() {
            if !names.is_empty() {
                self.status = format!("No card list found in {}.", names.join(", "));
            }
            return;
        }
        let total: u32 = parsed.iter().map(|l| l.count).sum();
        self.import_text = text;
        self.tab = Tab::Collection;
        self.status = format!(
            "Loaded {} cards from {} - press \"Add to my pool\" or \"Replace my pool\" to import them.",
            total,
            names.join(", ")
        );
    }
}

impl eframe::App for GuiApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let frame_start = std::time::Instant::now();
        self.images.poll(&ctx);
        self.poll_jobs(&ctx);
        self.handle_dropped_files(&ctx);
        if self.memo.rev != self.rev {
            self.memo = Memo {
                rev: self.rev,
                ..Default::default()
            };
        }
        if self.sort == SortBy::Score && self.memo.scores.is_none() {
            let scores = self
                .db
                .cards
                .iter()
                .map(|c| {
                    (
                        c.name.clone(),
                        analysis::score_card(c, &self.deck, &self.db, &self.rules).0,
                    )
                })
                .collect();
            self.memo.scores = Some(scores);
        }
        let mut acts: Vec<Action> = Vec::new();
        Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            self.top_bar(ui, &mut acts);
            ui.add_space(2.0);
        });
        Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&self.status).color(Color32::from_rgb(200, 220, 255)));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("Files: {}", self.data_dir.display()))
                            .small()
                            .weak(),
                    );
                    if self.settings.real_images {
                        let pending = self.images.pending();
                        let text = if pending > 0 {
                            format!(
                                "Card images: {} saved, downloading {} more · ",
                                self.images.loaded(),
                                pending
                            )
                        } else if self.images.failures() > 0 && self.images.loaded() == 0 {
                            "Card images unavailable (offline?) - showing drawn cards · "
                                .to_string()
                        } else {
                            format!("Card images: {} saved · ", self.images.loaded())
                        };
                        ui.label(RichText::new(text).small().weak());
                    }
                });
            });
        });
        if let Some(e) = &self.error {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.heading("Something went wrong loading your files");
                ui.label(e.as_str());
            });
            return;
        }
        match self.tab {
            Tab::Build => self.build_tab(ui, &mut acts),
            Tab::Lab => {
                Panel::right("inspector_l")
                    .resizable(true)
                    .default_size(330.0)
                    .show(ui, |ui| self.inspector(ui, &mut acts));
                egui::CentralPanel::default().show(ui, |ui| self.lab_tab(ui, &mut acts));
            }
            Tab::Commanders => {
                Panel::right("inspector_c")
                    .resizable(true)
                    .default_size(330.0)
                    .show(ui, |ui| self.inspector(ui, &mut acts));
                egui::CentralPanel::default().show(ui, |ui| self.commanders_tab(ui, &mut acts));
            }
            Tab::Draft => {
                Panel::right("inspector_d")
                    .resizable(true)
                    .default_size(330.0)
                    .show(ui, |ui| self.inspector(ui, &mut acts));
                egui::CentralPanel::default().show(ui, |ui| self.draft_tab(ui, &mut acts));
            }
            Tab::Trade => {
                Panel::right("inspector_t")
                    .resizable(true)
                    .default_size(330.0)
                    .show(ui, |ui| self.inspector(ui, &mut acts));
                egui::CentralPanel::default().show(ui, |ui| self.trade_tab(ui, &mut acts));
            }
            Tab::Upgrades => {
                Panel::right("inspector_u")
                    .resizable(true)
                    .default_size(330.0)
                    .show(ui, |ui| self.inspector(ui, &mut acts));
                egui::CentralPanel::default().show(ui, |ui| self.upgrades_tab(ui, &mut acts));
            }
            Tab::Stats => {
                egui::CentralPanel::default().show(ui, |ui| self.stats_tab(ui, &mut acts));
            }
            Tab::Season => {
                egui::CentralPanel::default().show(ui, |ui| self.season_tab(ui, &mut acts));
            }
            Tab::Collection => {
                Panel::right("inspector_p")
                    .resizable(true)
                    .default_size(330.0)
                    .show(ui, |ui| self.inspector(ui, &mut acts));
                egui::CentralPanel::default().show(ui, |ui| self.collection_tab(ui, &mut acts));
            }
        }
        if !acts.is_empty() {
            self.apply(acts, &ctx);
        }
        if std::env::var_os("MSH_PERF").is_some() {
            eprintln!(
                "frame {:.2} ms",
                frame_start.elapsed().as_secs_f64() * 1000.0
            );
        }
        let _ = Vec2::ZERO;
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop_lab();
        self.autosave();
        self.save_settings();
    }
}
