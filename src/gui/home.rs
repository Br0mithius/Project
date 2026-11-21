//! Home: the deck, the next session, the real record and what needs doing, on one screen,
//! with buttons to where each thing is done.

use super::{Action, GuiApp, Tab};
use crate::analysis;
use eframe::egui::{self, Color32, CornerRadius, RichText, Ui};

const GOOD: Color32 = Color32::from_rgb(130, 220, 140);
const WARN: Color32 = Color32::from_rgb(255, 190, 110);
const GOLD: Color32 = Color32::from_rgb(255, 205, 80);

/// The game log, re-read every couple of seconds (not every frame).
#[derive(Default)]
pub struct HomeState {
    log: Option<(std::time::Instant, crate::games::GameLog)>,
    /// The simulator's forecast for the current deck against the next table: the deck
    /// revision it was made for, and (win rate, rounds) once the background run is back.
    forecast: Option<(u64, Option<(f32, f32)>)>,
    forecast_rx: Option<std::sync::mpsc::Receiver<(u64, Option<(f32, f32)>)>>,
}

/// A titled panel.
pub(super) fn card(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    // The frame's margins and border come off the width, so the panel stays in its column.
    let inner = (ui.available_width() - 32.0).max(120.0);
    egui::Frame::new()
        .fill(Color32::from_rgb(34, 35, 42))
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(52, 54, 64)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(14.0)
        .show(ui, |ui| {
            ui.set_width(inner);
            ui.label(RichText::new(title).size(16.0).strong().color(GOLD));
            ui.add_space(4.0);
            body(ui);
        });
}

impl GuiApp {
    fn game_log(&mut self) -> crate::games::GameLog {
        let fresh = self
            .home
            .log
            .as_ref()
            .is_some_and(|(t, _)| t.elapsed().as_secs_f32() < 2.0);
        if !fresh {
            self.home.log = Some((
                std::time::Instant::now(),
                crate::games::GameLog::load(&self.data_dir),
            ));
        }
        self.home
            .log
            .as_ref()
            .map(|l| l.1.clone())
            .unwrap_or_default()
    }

    /// Starts (once per deck change) and collects the win-rate forecast.
    fn forecast(&mut self, ctx: &egui::Context) -> Option<Option<(f32, f32)>> {
        if let Some(rx) = &self.home.forecast_rx {
            match rx.try_recv() {
                Ok(r) => {
                    self.home.forecast = Some(r);
                    self.home.forecast_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.home.forecast_rx = None,
                Err(_) => ctx.request_repaint_after(std::time::Duration::from_millis(400)),
            }
        }
        let stale = self
            .home
            .forecast
            .as_ref()
            .is_none_or(|(rev, _)| *rev != self.rev);
        if stale && self.home.forecast_rx.is_none() && self.deck.commander.is_some() {
            let (deck, db, rules, table, rev) = (
                self.deck.clone(),
                self.db.clone(),
                self.rules.clone(),
                self.table.clone(),
                self.rev,
            );
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let g = crate::table::gauntlet(&table, &db);
                let _ = tx.send((rev, crate::games::predict(&deck, &g, &db, &rules, 600)));
            });
            self.home.forecast_rx = Some(rx);
        }
        self.home
            .forecast
            .as_ref()
            .filter(|(rev, _)| *rev == self.rev)
            .map(|(_, f)| *f)
    }

    pub(super) fn home_tab(&mut self, ui: &mut Ui, _acts: &mut Vec<Action>) {
        let log = self.game_log();
        let forecast = self.forecast(ui.ctx());
        let lands = crate::mana::land_equivalents(&self.deck, &self.db);
        let cap = analysis::land_cap(&self.rules);
        let total = self.deck.total_count(&self.rules);
        let size = self.rules.deck_size;
        let filler: Vec<String> = analysis::resolved(&self.deck, &self.db)
            .iter()
            .filter(|(c, _)| crate::cohesion::is_filler(c))
            .map(|(c, _)| c.name.clone())
            .collect();
        let cast = crate::mana::castability(&self.deck, &self.db);
        let at_table = self.table.next_count(&self.db);
        // What needs doing, most pressing first: (text, where it's done).
        let mut todo: Vec<(String, Tab)> = Vec::new();
        if self.deck.commander.is_none() {
            todo.push(("Pick a commander to start a deck.".into(), Tab::Commanders));
        } else {
            if total != size {
                todo.push((
                    format!("The deck has {} of {} cards.", total, size),
                    Tab::Build,
                ));
            }
            if lands > cap {
                todo.push((
                    format!("{} land-equivalents: over the cap of {}.", lands, cap),
                    Tab::Stats,
                ));
            }
            if !filler.is_empty()
                && analysis::deck_plan(&self.deck, &self.db, &self.rules)
                    .archetype
                    .label()
                    .to_lowercase()
                    .contains("control")
            {
                todo.push((
                    format!(
                        "{} card(s) with no job: {}.",
                        filler.len(),
                        filler.join(", ")
                    ),
                    Tab::Stats,
                ));
            }
            if cast.mean < 0.65 {
                todo.push((
                    format!(
                        "Spells are castable on curve only {:.0}% of the time: re-pick the basics.",
                        cast.mean * 100.0
                    ),
                    Tab::Stats,
                ));
            }
        }
        if self.coll.total() == 0 {
            todo.push((
                "Your pool is empty: import it from Mana Mixer.".into(),
                Tab::Collection,
            ));
        }
        if at_table == 0 {
            todo.push((
                "Add the people at your next table, so tests play against their decks.".into(),
                Tab::Season,
            ));
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_max_width(1100.0);
            ui.add_space(6.0);
            ui.columns(2, |cols| {
                // ---------------------------------------------------- the deck
                card(&mut cols[0], "Your deck", |ui| match self.deck.commander.clone() {
                    None => {
                        ui.label("No deck yet.");
                        if ui.button("Choose a commander").clicked() {
                            self.tab = Tab::Commanders;
                        }
                    }
                    Some(cmd) => {
                        ui.label(RichText::new(&cmd).size(18.0).strong());
                        let plan = analysis::deck_plan(&self.deck, &self.db, &self.rules);
                        ui.label(format!("{} · {} of {} cards · {} lands", plan.archetype.label(), total, size, analysis::land_count(&self.deck, &self.db)));
                        ui.label(
                            RichText::new(format!(
                                "Castable on curve {:.0}% · {}",
                                cast.mean * 100.0,
                                if filler.is_empty() { "every spell has a job".to_string() } else { format!("{} filler", filler.len()) }
                            ))
                            .color(if filler.is_empty() && lands <= cap { GOOD } else { WARN }),
                        );
                        match forecast {
                            Some(Some((win, rounds))) => {
                                ui.label(format!(
                                    "Simulator forecast against your next table: wins {:.0}% (an even game is 25%), games of {:.0} rounds",
                                    win * 100.0,
                                    rounds
                                ))
                                .on_hover_text("600 simulated games against the decks ticked at your next table, with their trade picks in. Log real games to check it.");
                            }
                            Some(None) => {
                                ui.label(RichText::new("No forecast: tick the people at your next table first.").weak());
                            }
                            None => {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label(RichText::new("Forecasting against your next table...").weak());
                                });
                            }
                        }
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("Edit deck").clicked() {
                                self.tab = Tab::Build;
                            }
                            if ui.button("Stats and mana").clicked() {
                                self.tab = Tab::Stats;
                            }
                            if ui.button("Optimise").on_hover_text("Search for a stronger version by playtests").clicked() {
                                self.tab = Tab::Lab;
                                self.lab_view = super::lab_extra::LabView::Optimise;
                            }
                        });
                    }
                });
                cols[0].add_space(10.0);
                card(&mut cols[0], "To do", |ui| {
                    if todo.is_empty() {
                        ui.label(RichText::new("Nothing needs fixing.").color(GOOD));
                    }
                    for (text, tab) in &todo {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new("•").color(WARN));
                            if ui.link(text.as_str()).clicked() {
                                self.tab = *tab;
                            }
                        });
                    }
                });
                // ---------------------------------------------------- the league
                card(&mut cols[1], "Next session", |ui| {
                    let s = crate::league::session(self.rules.session)
                        .unwrap_or_else(crate::league::upcoming_session);
                    let (cards, single) = s.deck_rules();
                    ui.label(RichText::new(format!("Session {} · {}", s.number, s.date_label())).size(18.0).strong());
                    ui.label(format!("{} · {} cards{}", s.label, cards, if single { ", singleton" } else { "" }));
                    let days = crate::league::days_until(s);
                    ui.label(
                        RichText::new(match days {
                            0 => "Today".to_string(),
                            1 => "Tomorrow".to_string(),
                            d if d > 1 => format!("In {} days", d),
                            d => format!("{} days ago - pick the next session at the top", -d),
                        })
                        .color(if (0..=2).contains(&days) { WARN } else { GOOD }),
                    );
                    ui.label(RichText::new("Trades for the round lock before the session: check Mana Mixer for the exact time.").small().weak());
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Plan trades").clicked() {
                            self.tab = Tab::Trade;
                        }
                        if ui.button("Upgrades").clicked() {
                            self.tab = Tab::Upgrades;
                        }
                        if ui.button("Draft helper").clicked() {
                            self.tab = Tab::Draft;
                        }
                        if ui.button("Schedule").clicked() {
                            self.tab = Tab::Season;
                        }
                    });
                });
                cols[1].add_space(10.0);
                card(&mut cols[1], "Real games", |ui| {
                    let r = log.record(None);
                    if r.played == 0 {
                        ui.label("No games logged yet. Logging results shows whether the simulator can be trusted.");
                    } else {
                        ui.label(RichText::new(format!("{} - {}", r.won, r.played - r.won)).size(18.0).strong());
                        ui.label(format!("{:.0}% of {} games won", r.rate() * 100.0, r.played));
                        for line in log.calibration(None) {
                            ui.label(RichText::new(line).small());
                        }
                    }
                    ui.label(
                        RichText::new(if at_table == 0 {
                            "No next table set.".to_string()
                        } else {
                            format!("Next table: {}", self.table.next_names(&self.db).join(", "))
                        })
                        .small(),
                    );
                    ui.add_space(4.0);
                    if ui.button("Log a game / my table").clicked() {
                        self.tab = Tab::Season;
                    }
                });
            });
        });
    }
}

/// A one-line summary with the full explanation on hover (a "(?)" marks it).
pub(super) fn help(ui: &mut Ui, short: &str, long: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(short).weak());
        ui.label(RichText::new("(?)").color(GOLD))
            .on_hover_text(long);
    });
}
