//! Playtest Lab > Optimise: the deck optimiser and the claim tester (see `crate::optimize`),
//! run on a background thread with progress and Stop.

use super::{Action, GuiApp};
use crate::lab::{Effort, Progress};
use crate::optimize::{ClaimReport, Opponents, OptReport};
use eframe::egui::{self, RichText, ScrollArea, Ui};
use std::sync::Arc;

pub enum OptResult {
    Opt(Option<OptReport>),
    Claims(Option<ClaimReport>),
}

pub struct OptJob {
    progress: Arc<Progress>,
    rx: std::sync::mpsc::Receiver<OptResult>,
    started: std::time::Instant,
    what: &'static str,
}

#[derive(Default)]
pub struct OptState {
    pub job: Option<OptJob>,
    pub report: Option<OptReport>,
    pub claims: Option<ClaimReport>,
    /// Score weight of the next table (0..1).
    pub table_weight: Option<f32>,
}

impl GuiApp {
    fn opponents_for_opt(&self, weight: f32) -> (Vec<crate::playtest::Prepared>, f32) {
        (crate::table::gauntlet(&self.table, &self.db), weight)
    }

    fn start_opt(&mut self, claims: bool, effort: Effort) {
        if self.opt.job.is_some() {
            return;
        }
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
        let weight = self.opt.table_weight.unwrap_or(0.5);
        let (table, weight) = self.opponents_for_opt(weight);
        let names: Vec<String> = self.public_pool.counts.keys().cloned().collect();
        if claims && names.is_empty() {
            self.status = "No Public Pool list yet: update it on the Upgrades tab first.".into();
            return;
        }
        let (db, coll, rules, deck) = (
            self.db.clone(),
            self.coll.clone(),
            self.rules.clone(),
            self.deck.clone(),
        );
        let progress = Arc::new(Progress::new());
        let p = progress.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let opp = Opponents {
                table,
                league: crate::ranking::TestBed::new(&db, &rules).gauntlet,
                table_weight: weight,
            };
            let r = if claims {
                OptResult::Claims(crate::optimize::test_claims(
                    &deck, &names, &db, &rules, &opp, 800, 8, &p,
                ))
            } else {
                let rounds = if effort == Effort::THOROUGH { 4 } else { 2 };
                OptResult::Opt(crate::optimize::optimize(
                    &cmd, &deck, &db, &coll, &rules, &opp, effort, rounds, &p,
                ))
            };
            let _ = tx.send(r);
        });
        self.opt.job = Some(OptJob {
            progress,
            rx,
            started: std::time::Instant::now(),
            what: if claims {
                "Testing claims"
            } else {
                "Optimising"
            },
        });
    }

    pub(super) fn poll_opt(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.opt.job else {
            return;
        };
        match job.rx.try_recv() {
            Ok(r) => {
                self.opt.job = None;
                match r {
                    OptResult::Opt(Some(rep)) => {
                        self.status = "Deck optimiser finished.".into();
                        self.opt.report = Some(rep);
                    }
                    OptResult::Claims(Some(rep)) => {
                        self.status = "Claim test finished.".into();
                        self.opt.claims = Some(rep);
                    }
                    _ => self.status = "Stopped.".into(),
                }
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.opt.job = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(300))
            }
        }
    }

    pub(super) fn optimize_view(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        ui.label(
            "Searches for the strongest deck by playtests: whole-deck builds raced against each other, then the lab's \
             swap search round after round, and a final check against your deck on fresh games (a lucky first run \
             can't fool it). Claims: every Public Pool card you could claim, played in your deck and ranked by gain.",
        );
        let at_table = self.table.next_count(&self.db);
        ui.horizontal_wrapped(|ui| {
            ui.label("Weight of your next table:");
            let mut w = self.opt.table_weight.unwrap_or(0.5) * 100.0;
            if ui
                .add_enabled(
                    at_table > 0,
                    egui::Slider::new(&mut w, 0.0..=100.0).step_by(10.0).suffix("%"),
                )
                .on_hover_text("How much of the score comes from your ticked next table (the rest: the simulated league)")
                .changed()
            {
                self.opt.table_weight = Some(w / 100.0);
            }
            if at_table == 0 {
                ui.label(RichText::new("no next table ticked: league only").weak());
            }
        });
        let has_cmd = self.deck.commander.is_some();
        ui.horizontal_wrapped(|ui| {
            if let Some(job) = &self.opt.job {
                let f = job.progress.fraction();
                ui.add(egui::ProgressBar::new(f).desired_width(320.0).text(format!(
                    "{}: {} ({:.0} s)",
                    job.what,
                    job.progress.stage(),
                    job.started.elapsed().as_secs_f32()
                )));
                if ui.button("Stop").clicked() {
                    job.progress.cancel();
                }
            } else {
                if ui
                    .add_enabled(has_cmd, egui::Button::new("Optimise deck (a few minutes)"))
                    .clicked()
                {
                    self.start_opt(false, Effort::QUICK);
                }
                if ui
                    .add_enabled(has_cmd, egui::Button::new("Optimise thoroughly (longer)"))
                    .clicked()
                {
                    self.start_opt(false, Effort::THOROUGH);
                }
                if ui
                    .add_enabled(has_cmd, egui::Button::new("Test Public Pool claims"))
                    .on_hover_text("Plays every Public Pool card in your colours in your deck (update the list on the Upgrades tab)")
                    .clicked()
                {
                    self.start_opt(true, Effort::QUICK);
                }
            }
        });
        ui.separator();
        ScrollArea::vertical()
            .id_salt("opt_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if let Some(r) = &self.opt.report {
                    if r.final_gain.is_some() && r.start == self.deck {
                        let clear = r.final_gain.is_some_and(|g| g.score.z() >= 2.0);
                        let label = if clear {
                            "Use the optimised deck"
                        } else {
                            "Use the optimised deck anyway (not clearly better)"
                        };
                        if ui.button(label).clicked() {
                            let mut d = r.best.clone();
                            d.name = self.deck.name.clone();
                            acts.push(Action::ApplyDeck(
                                d,
                                "Switched to the optimised deck. Undo goes back.".into(),
                            ));
                        }
                    }
                    ui.label(RichText::new(r.text()).monospace());
                    ui.separator();
                }
                if let Some(c) = &self.opt.claims {
                    ui.label(RichText::new(c.text()).monospace());
                }
                if self.opt.report.is_none() && self.opt.claims.is_none() {
                    ui.label(RichText::new("No results yet.").weak());
                }
            });
    }
}
