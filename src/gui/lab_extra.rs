//! Playtest Lab extras: the measured card power list, the card synergy (package) test,
//! and the measured-power / simulator-coverage lines of the card details.

use super::{Action, GuiApp, fmt_secs};
use crate::card::Card;
use crate::db::CardDb;
use crate::playtest::coverage::Handling;
use crate::playtest::lab::Measure;
use crate::playtest::synergy::PackageReport;
use eframe::egui::{self, Color32, RichText, ScrollArea, Ui};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const GOOD: Color32 = Color32::from_rgb(110, 220, 120);
const BAD: Color32 = Color32::from_rgb(255, 120, 100);
const NOTE: Color32 = Color32::from_rgb(255, 190, 110);

/// The Playtest Lab's sub-views.
#[derive(Clone, Copy, PartialEq, Default)]
pub enum LabView {
    #[default]
    Deck,
    Power,
    Synergy,
}

#[derive(Clone, Copy, PartialEq, Default)]
pub enum PowerScope {
    #[default]
    All,
    Pool,
    Deck,
}

#[derive(Clone, Copy, PartialEq, Default)]
pub enum PowerSort {
    #[default]
    Value,
    Name,
    Se,
    Decks,
    Spread,
}

/// A card synergy test running on a background thread.
pub struct SynJob {
    done: Arc<AtomicUsize>,
    total: usize,
    cancel: Arc<AtomicBool>,
    rx: std::sync::mpsc::Receiver<Result<Option<PackageReport>, String>>,
    started: std::time::Instant,
    against_table: bool,
}

pub struct ExtraState {
    pub scope: PowerScope,
    pub sort: PowerSort,
    pub descending: bool,
    pub search: String,
    pub consistent_only: bool,
    /// The package to test.
    pub syn_cards: Vec<String>,
    pub syn_query: String,
    pub syn_games: u32,
    pub syn_table: bool,
    pub syn_job: Option<SynJob>,
    /// The last result, and whether it was against my table.
    pub syn_result: Option<(Result<PackageReport, String>, bool)>,
}

impl Default for ExtraState {
    fn default() -> Self {
        ExtraState {
            scope: PowerScope::All,
            sort: PowerSort::Value,
            descending: true,
            search: String::new(),
            consistent_only: false,
            syn_cards: Vec::new(),
            syn_query: String::new(),
            syn_games: 2000,
            syn_table: false,
            syn_job: None,
            syn_result: None,
        }
    }
}

/// "Measured power +2.1 ± 0.6 (4 decks, consistent)", if the card was measured.
pub fn power_line(db: &CardDb, name: &str) -> Option<String> {
    let p = db.sim_values.power(name)?;
    Some(format!(
        "Measured power {:+.1} ± {:.1} win-rate points over a vanilla 3-drop ({} deck{}{})",
        p.value,
        p.se,
        p.decks,
        if p.decks == 1 { "" } else { "s" },
        if p.consistent() { ", consistent" } else { "" }
    ))
}

fn handling_color(h: Handling) -> Color32 {
    match h {
        Handling::Exact => GOOD,
        Handling::Approx => NOTE,
        Handling::None => BAD,
    }
}

/// "+1.23 ± 0.45" in win-rate points, coloured when clearly (2+ standard errors) positive
/// or negative.
fn pts(m: &Measure) -> RichText {
    let t = RichText::new(crate::lab::pts_se(m)).monospace();
    if m.z() > 2.0 {
        t.color(GOOD)
    } else if m.z() < -2.0 {
        t.color(BAD)
    } else {
        t
    }
}

impl GuiApp {
    /// Measured power and how the simulator handles the card, for the card details.
    pub(super) fn card_facts(&mut self, ui: &mut Ui, card: &Card) {
        match self.db.sim_values.power(&card.name) {
            Some(p) => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!("Measured power {:+.1} ± {:.1}", p.value, p.se))
                            .strong(),
                    )
                    .on_hover_text(
                        "Win-rate points over a vanilla 3-drop, from playtests (Playtest Lab > Measured card power).",
                    );
                    ui.label(
                        RichText::new(format!(
                            "{} deck{}",
                            p.decks,
                            if p.decks == 1 { "" } else { "s" }
                        ))
                        .small(),
                    );
                    if p.consistent() {
                        ui.label(RichText::new("consistent").small().color(GOOD));
                    }
                });
            }
            None if !card.is_land() => {
                ui.label(
                    RichText::new("Not measured by playtests yet.")
                        .small()
                        .weak(),
                );
            }
            None => {}
        }
        let cov = self
            .coverage
            .entry(card.name.clone())
            .or_insert_with(|| crate::playtest::coverage::card_coverage(card));
        let notes = cov.notes();
        let status = cov.status;
        let parts: Vec<(Handling, String, String)> = cov
            .parts
            .iter()
            .map(|p| (p.handling, p.text.clone(), p.notes.clone()))
            .collect();
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Simulator:").small());
            let resp = ui.label(
                RichText::new(status.label())
                    .small()
                    .strong()
                    .color(handling_color(status)),
            );
            if !notes.is_empty() {
                let first: String = notes[0].chars().take(90).collect();
                ui.label(RichText::new(first).small().weak());
            }
            resp.on_hover_ui(|ui| {
                ui.label(RichText::new("How the game simulator plays each part:").strong());
                for (h, text, note) in &parts {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(h.label()).small().color(handling_color(*h)));
                        ui.label(RichText::new(text).small());
                        if !note.is_empty() {
                            ui.label(RichText::new(format!("({})", note)).small().weak());
                        }
                    });
                }
                if parts.is_empty() {
                    ui.label(RichText::new("Nothing to play beyond its cost and body.").small());
                }
            });
        });
    }

    // ------------------------------------------------------------------ measured power

    pub(super) fn power_view(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        ui.label(
            "Card power measured by playtests: each card against a vanilla creature of its cost in the same games, \
             on one scale (win-rate points over a vanilla 3-drop). Measured in more decks = more trustworthy; \
             consistent = three or more decks with similar results. The deck builder weighs these most.",
        );
        self.measure_controls(ui, acts);
        ui.separator();
        let all = self.db.sim_values.all();
        if all.is_empty() {
            ui.label(
                "Nothing measured yet. Choose a commander and press \"Measure card power\" (or \"... and in every league deck\" for a card's power across many decks).",
            );
            return;
        }
        let st = &mut self.extra;
        ui.horizontal_wrapped(|ui| {
            ui.label("Show:");
            ui.selectable_value(&mut st.scope, PowerScope::All, "all measured");
            ui.selectable_value(&mut st.scope, PowerScope::Pool, "my pool");
            ui.selectable_value(&mut st.scope, PowerScope::Deck, "my deck");
            ui.separator();
            ui.checkbox(&mut st.consistent_only, "consistent only");
            ui.separator();
            ui.add(
                egui::TextEdit::singleline(&mut st.search)
                    .hint_text("Filter by name")
                    .desired_width(180.0),
            );
        });
        let q = crate::db::normalize(&self.extra.search);
        let mut rows: Vec<(String, crate::playtest::simvalues::Power)> = all
            .into_iter()
            .filter(|(n, p)| {
                let owned = self.coll.owned(n) > 0;
                let in_deck = self.deck.used(n) > 0;
                (match self.extra.scope {
                    PowerScope::All => true,
                    PowerScope::Pool => owned,
                    PowerScope::Deck => in_deck,
                }) && (!self.extra.consistent_only || p.consistent())
                    && (q.is_empty() || crate::db::normalize(n).contains(&q))
            })
            .collect();
        let sort = self.extra.sort;
        rows.sort_by(|a, b| {
            let o = match sort {
                PowerSort::Value => a.1.value.partial_cmp(&b.1.value).unwrap(),
                PowerSort::Name => b.0.cmp(&a.0),
                PowerSort::Se => b.1.se.partial_cmp(&a.1.se).unwrap(),
                PowerSort::Decks => a.1.decks.cmp(&b.1.decks),
                PowerSort::Spread => b.1.spread.partial_cmp(&a.1.spread).unwrap(),
            };
            o.then_with(|| b.0.cmp(&a.0))
        });
        if self.extra.descending {
            rows.reverse();
        }
        ui.label(
            RichText::new(format!(
                "{} cards · {} decks measured · click a column to sort",
                rows.len(),
                self.db.sim_values.decks().len()
            ))
            .small()
            .weak(),
        );
        ScrollArea::vertical()
            .id_salt("power_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("power_grid")
                    .striped(true)
                    .min_col_width(60.0)
                    .show(ui, |ui| {
                        for (s, label) in [
                            (PowerSort::Name, "Card"),
                            (PowerSort::Value, "Power"),
                            (PowerSort::Se, "± SE"),
                            (PowerSort::Decks, "Decks"),
                            (PowerSort::Spread, "Spread"),
                        ] {
                            let arrow = if self.extra.sort == s {
                                if self.extra.descending {
                                    " ⏷"
                                } else {
                                    " ⏶"
                                }
                            } else {
                                ""
                            };
                            if ui
                                .button(RichText::new(format!("{}{}", label, arrow)).strong())
                                .clicked()
                            {
                                if self.extra.sort == s {
                                    self.extra.descending = !self.extra.descending;
                                } else {
                                    self.extra.sort = s;
                                    self.extra.descending = s != PowerSort::Name;
                                }
                            }
                        }
                        ui.label("");
                        ui.label("");
                        ui.end_row();
                        for (name, p) in &rows {
                            if ui.link(name.as_str()).clicked() {
                                acts.push(Action::Select(name.clone()));
                            }
                            let z = if p.se > 0.0 { p.value / p.se } else { 0.0 };
                            let v = RichText::new(format!("{:+.1}", p.value)).monospace();
                            ui.label(if z > 2.0 {
                                v.color(GOOD)
                            } else if z < -2.0 {
                                v.color(BAD)
                            } else {
                                v
                            });
                            ui.label(RichText::new(format!("{:.1}", p.se)).monospace().weak());
                            ui.label(p.decks.to_string());
                            ui.label(
                                RichText::new(if p.decks > 1 {
                                    format!("{:.1}", p.spread)
                                } else {
                                    "-".into()
                                })
                                .monospace()
                                .weak(),
                            );
                            if p.consistent() {
                                ui.label(RichText::new("consistent").small().color(GOOD));
                            } else {
                                ui.label("");
                            }
                            let tag = if self.deck.used(name) > 0 {
                                "in deck"
                            } else if self.coll.owned(name) > 0 {
                                "owned"
                            } else {
                                ""
                            };
                            ui.label(RichText::new(tag).small().weak());
                            ui.end_row();
                        }
                    });
            });
    }

    /// The "Measure card power" buttons, or the running measurement's progress and Stop.
    pub(super) fn measure_controls(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let has_cmd = self.deck.commander.is_some();
        ui.horizontal_wrapped(|ui| {
            if let Some(job) = &self.measure_job {
                let f = (job.done.load(Ordering::Relaxed) as f32 / job.total.max(1) as f32).min(1.0);
                let secs = job.started.elapsed().as_secs_f32();
                let left = if f > 0.03 {
                    format!(" · about {} left", fmt_secs(secs * (1.0 - f) / f))
                } else {
                    String::new()
                };
                ui.add(egui::ProgressBar::new(f).desired_width(300.0).text(format!(
                    "Measuring card power {:.0}%{}",
                    f * 100.0,
                    left
                )));
                if ui.button("Stop measuring").clicked() {
                    acts.push(Action::StopMeasure);
                }
            } else {
                if ui
                    .add_enabled(has_cmd, egui::Button::new("Measure card power"))
                    .on_hover_text("Playtests every card in your deck and every other spell you own, each against a vanilla creature of its cost. The builder then weighs these results most.")
                    .clicked()
                {
                    acts.push(Action::Measure(false));
                }
                if ui
                    .add_enabled(has_cmd, egui::Button::new("... and in every league deck"))
                    .on_hover_text("Also measures the cards of every simulated league opponent's deck: a card's power across many decks and commanders, and how consistent it is.")
                    .clicked()
                {
                    acts.push(Action::Measure(true));
                }
            }
        });
    }

    // ------------------------------------------------------------------ synergy test

    pub(super) fn start_synergy(&mut self) {
        if self.extra.syn_job.is_some() {
            return;
        }
        let n = self.extra.syn_cards.len();
        if !(2..=8).contains(&n) {
            self.status = "Pick 2 to 8 cards for the synergy test.".into();
            return;
        }
        if self.deck.commander.is_none() {
            self.status = "Choose a commander first: the package is tested in your deck.".into();
            return;
        }
        let against_table = self.extra.syn_table;
        let table = if against_table {
            let g = crate::table::gauntlet(&self.table, &self.db);
            if g.is_empty() {
                self.status =
                    "No one is ticked as at your next table (Season & Rules > My table).".into();
                return;
            }
            Some(g)
        } else {
            None
        };
        let (db, rules, deck, cards) = (
            self.db.clone(),
            self.rules.clone(),
            self.deck.clone(),
            self.extra.syn_cards.clone(),
        );
        let games = self.extra.syn_games.clamp(100, 50_000);
        let total = crate::playtest::synergy::package_size(n, games);
        let done = Arc::new(AtomicUsize::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let (d, c) = (done.clone(), cancel.clone());
        std::thread::spawn(move || {
            let gauntlet = match table {
                Some(g) => g,
                None => crate::ranking::TestBed::new(&db, &rules).gauntlet,
            };
            let r = crate::playtest::synergy::package_synergy(
                &deck, &cards, &db, &rules, &gauntlet, games, 20261029, &d, &c,
            );
            let _ = tx.send(r);
        });
        self.extra.syn_job = Some(SynJob {
            done,
            total,
            cancel,
            rx,
            started: std::time::Instant::now(),
            against_table,
        });
    }

    pub(super) fn stop_synergy(&mut self) {
        if let Some(j) = self.extra.syn_job.take() {
            j.cancel.store(true, Ordering::Relaxed);
            self.status = "Card synergy test stopped.".into();
        }
    }

    pub(super) fn poll_synergy(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.extra.syn_job else {
            return;
        };
        match job.rx.try_recv() {
            Ok(r) => {
                let t = job.against_table;
                self.extra.syn_job = None;
                match r {
                    Ok(Some(rep)) => {
                        self.status = format!(
                            "Card synergy test finished: interaction {} win-rate points.",
                            crate::lab::pts_se(&rep.interaction)
                        );
                        self.extra.syn_result = Some((Ok(rep), t));
                    }
                    Ok(None) => {}
                    Err(e) => {
                        self.status = e.clone();
                        self.extra.syn_result = Some((Err(e), t));
                    }
                }
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.extra.syn_job = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(300))
            }
        }
    }

    pub(super) fn synergy_view(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        ui.label(
            "Are some cards worth more together than apart? Pick 2-8 cards: the test plays your deck with parts of the \
             package swapped for neutral stand-ins (a vanilla creature of the same cost), all on the same games. \
             Each card alone (the rest out), each card in the package (left out alone), the whole package, and the \
             interaction: the package minus the sum of the cards alone (positive = synergy). Cards from your pool \
             that aren't in the deck go in place of its weakest spells.",
        );
        ui.add_space(4.0);
        let running = self.extra.syn_job.is_some();
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!("Package ({}/8):", self.extra.syn_cards.len())).strong(),
            );
            if self.extra.syn_cards.is_empty() {
                ui.label(RichText::new("no cards yet - add them below").weak());
            }
            let mut remove = None;
            for (i, c) in self.extra.syn_cards.iter().enumerate() {
                let tag = if self.deck.used(c) > 0 { "" } else { " (pool)" };
                if ui
                    .add_enabled(!running, egui::Button::new(format!("{}{}  x", c, tag)))
                    .on_hover_text("Remove from the package")
                    .clicked()
                {
                    remove = Some(i);
                }
            }
            if let Some(i) = remove {
                self.extra.syn_cards.remove(i);
            }
            if !self.extra.syn_cards.is_empty()
                && ui
                    .add_enabled(!running, egui::Button::new("Clear"))
                    .clicked()
            {
                self.extra.syn_cards.clear();
            }
        });
        let full = self.extra.syn_cards.len() >= 8;
        ui.add_enabled_ui(!running && !full, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("From the deck:");
                let mut spells: Vec<String> = self
                    .deck
                    .cards
                    .keys()
                    .filter(|n| self.db.get(n).is_some_and(|c| !c.is_land()))
                    .filter(|n| !self.extra.syn_cards.contains(n))
                    .cloned()
                    .collect();
                spells.sort();
                let mut pick = None;
                egui::ComboBox::from_id_salt("syn_deck_pick")
                    .selected_text(if spells.is_empty() {
                        "(no spells in the deck)"
                    } else {
                        "choose a card"
                    })
                    .width(280.0)
                    .height(400.0)
                    .show_ui(ui, |ui| {
                        for n in &spells {
                            if ui.selectable_label(false, n.as_str()).clicked() {
                                pick = Some(n.clone());
                            }
                        }
                    });
                if let Some(n) = pick {
                    self.extra.syn_cards.push(n);
                }
                ui.separator();
                ui.label("From your pool:");
                let mut q = std::mem::take(&mut self.extra.syn_query);
                let chosen = ui
                    .vertical(|ui| self.name_search(ui, "syn_pool", &mut q, false))
                    .inner;
                self.extra.syn_query = q;
                if let Some(n) = chosen {
                    let land = self.db.get(&n).is_some_and(|c| c.is_land());
                    if land {
                        self.status = format!("{} is a land: the synergy test takes spells.", n);
                    } else if !self.extra.syn_cards.contains(&n) {
                        self.extra.syn_cards.push(n);
                        self.extra.syn_query.clear();
                    }
                }
            });
        });
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if let Some(job) = &self.extra.syn_job {
                let f =
                    (job.done.load(Ordering::Relaxed) as f32 / job.total.max(1) as f32).min(1.0);
                let secs = job.started.elapsed().as_secs_f32();
                let left = if f > 0.03 {
                    format!(" · about {} left", fmt_secs(secs * (1.0 - f) / f))
                } else {
                    String::new()
                };
                ui.add(egui::ProgressBar::new(f).desired_width(360.0).text(format!(
                    "Testing the package {:.0}%{}",
                    f * 100.0,
                    left
                )));
                if ui.button("Stop").clicked() {
                    acts.push(Action::StopSynergy);
                }
            } else {
                let ok =
                    (2..=8).contains(&self.extra.syn_cards.len()) && self.deck.commander.is_some();
                if ui
                    .add_enabled(ok, egui::Button::new("Run synergy test"))
                    .on_disabled_hover_text("Choose a commander and pick 2-8 cards.")
                    .clicked()
                {
                    acts.push(Action::RunSynergy);
                }
                ui.label("games:");
                ui.add(
                    egui::DragValue::new(&mut self.extra.syn_games)
                        .range(200..=20000)
                        .speed(100),
                );
                ui.label("against:");
                ui.selectable_value(&mut self.extra.syn_table, false, "simulated league");
                ui.selectable_value(&mut self.extra.syn_table, true, "my next table");
                let n = self.extra.syn_cards.len().max(2);
                ui.label(
                    RichText::new(format!(
                        "{} deck-games",
                        crate::playtest::synergy::package_size(n, self.extra.syn_games)
                    ))
                    .small()
                    .weak(),
                );
            }
        });
        ui.separator();
        let Some((r, table)) = &self.extra.syn_result else {
            ui.label(RichText::new("No result yet.").weak());
            return;
        };
        let r = match r {
            Ok(r) => r,
            Err(e) => {
                ui.label(RichText::new(e).color(BAD));
                return;
            }
        };
        ui.label(
            RichText::new(format!(
                "{} games against {} · full deck won {:.1}%, with the package out {:.1}%",
                r.games,
                if *table {
                    "your next table"
                } else {
                    "the simulated league"
                },
                r.full_win * 100.0,
                r.out_win * 100.0
            ))
            .strong(),
        );
        for (add, cut) in &r.swapped_in {
            ui.label(
                RichText::new(format!(
                    "{} played in place of {} (not in the deck).",
                    add, cut
                ))
                .small()
                .color(NOTE),
            );
        }
        ScrollArea::vertical()
            .id_salt("syn_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("syn_grid").striped(true).show(ui, |ui| {
                    ui.label(RichText::new("Card").strong());
                    ui.label(RichText::new("Alone").strong())
                        .on_hover_text("The card with the rest of the package out, against the deck with all of it out.");
                    ui.label(RichText::new("In the package").strong())
                        .on_hover_text("The full deck against the deck with only this card out.");
                    ui.end_row();
                    for (i, c) in r.cards.iter().enumerate() {
                        if ui.link(c.as_str()).clicked() {
                            acts.push(Action::Select(c.clone()));
                        }
                        ui.label(pts(&r.alone[i]));
                        ui.label(pts(&r.in_package[i]));
                        ui.end_row();
                    }
                    ui.label(RichText::new("Sum").weak());
                    ui.label(pts(&r.sum_alone));
                    ui.label(pts(&r.sum_in_package));
                    ui.end_row();
                    ui.label(RichText::new("Whole package").strong());
                    ui.label(pts(&r.package));
                    ui.label("");
                    ui.end_row();
                });
                ui.add_space(6.0);
                let i = &r.interaction;
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Interaction").size(18.0).strong());
                    ui.label(pts(i).size(18.0).strong());
                });
                ui.label(if i.z() > 2.0 {
                    RichText::new("Interaction: these cards are clearly worth more together than apart (synergy).").color(GOOD)
                } else if i.z() < -2.0 {
                    RichText::new("Interaction: these cards are worth less together than apart (they compete for the same job).").color(BAD)
                } else {
                    RichText::new("Interaction: no clear synergy - within about 2 standard errors of zero (more games narrow it).")
                });
                ui.label(
                    RichText::new(
                        "Win-rate points (+2.0 = the deck wins 2% more of its games) ± one standard error, from paired games.",
                    )
                    .small()
                    .weak(),
                );
            });
    }
}
