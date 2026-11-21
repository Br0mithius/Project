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
    /// The "not claimable" name box.
    pub unavailable_text: String,
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
        super::home::help(
            ui,
            "Finds the strongest version of your deck by playtests, and tests what you could claim.",
            "Optimise: whole-deck builds are raced against yours, then single swaps are searched round after round, and the result is checked against your deck on fresh games, so a lucky first run can't fool it. Claims: every Public Pool card you could claim is played in your deck and ranked by its gain.",
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
        ui.horizontal_wrapped(|ui| {
            ui.label("Not claimable (ghost listings):").on_hover_text(
                "Cards Mana Mixer still lists in the trade pool after someone took them (e.g. as a Shop purchase). Claim tests and upgrades skip them.",
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.opt.unavailable_text)
                    .hint_text("card name")
                    .desired_width(200.0),
            );
            if ui.button("Mark").clicked() {
                let q = self.opt.unavailable_text.trim().to_string();
                match self.db.get(&q).map(|c| c.name.clone()) {
                    Some(name) => match crate::upgrades::mark_unavailable(&self.data_dir, &name) {
                        Ok(()) => {
                            self.public_pool = crate::upgrades::load_public_pool(&self.data_dir);
                            self.status = format!("{} marked not claimable.", name);
                            self.opt.unavailable_text.clear();
                        }
                        Err(e) => self.status = format!("Couldn't save: {}", e),
                    },
                    None => self.status = format!("Not a card: {}", q),
                }
            }
            let gone = crate::upgrades::unavailable(&self.data_dir);
            if !gone.is_empty() {
                ui.label(RichText::new(gone.join(", ")).small().weak());
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
                    show_opt_report(ui, r);
                    ui.separator();
                }
                if let Some(c) = &self.opt.claims {
                    show_claims(ui, c);
                }
                if self.opt.report.is_none() && self.opt.claims.is_none() {
                    ui.label(RichText::new("No results yet.").weak());
                }
            });
    }
}

const GOOD: egui::Color32 = egui::Color32::from_rgb(130, 220, 140);
const BAD: egui::Color32 = egui::Color32::from_rgb(255, 130, 110);

/// "+2.2 ± 0.4", green when clearly better (2+ standard errors), red when clearly worse.
fn gain_text(m: &crate::playtest::lab::Measure) -> RichText {
    let t = RichText::new(format!("{:+.1} ± {:.1}", m.gain * 100.0, m.stderr * 100.0)).monospace();
    if m.z() >= 2.0 {
        t.color(GOOD).strong()
    } else if m.z() <= -2.0 {
        t.color(BAD)
    } else {
        t
    }
}

fn show_opt_report(ui: &mut Ui, r: &OptReport) {
    super::home::card(ui, "Optimiser result", |ui| {
        ui.label(
            RichText::new(if r.table_decks > 0 {
                format!(
                    "Played against your next table ({} decks, {:.0}% of the score) and the simulated league. Numbers are win-rate points; green = clearly better.",
                    r.table_decks,
                    r.table_weight * 100.0
                )
            } else {
                "Played against the simulated league. Numbers are win-rate points; green = clearly better.".to_string()
            })
            .small()
            .weak(),
        );
        match &r.final_gain {
            None => {
                ui.label(RichText::new("No change held up on fresh games: your deck is already the strongest found.").color(GOOD));
            }
            Some(g) => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Optimised deck vs yours, on fresh games:").strong());
                    ui.label(gain_text(&g.score).size(16.0));
                    if let Some(t) = &g.table {
                        ui.label(RichText::new("your table").weak());
                        ui.label(gain_text(t));
                    }
                    if let Some(l) = &g.league {
                        ui.label(RichText::new("league").weak());
                        ui.label(gain_text(l));
                    }
                });
                ui.label(if g.score.z() >= 2.0 {
                    RichText::new("Clearly better: worth playing.").color(GOOD)
                } else if g.score.gain > 0.0 {
                    RichText::new(
                        "Better on these games but within noise: the gain may be small or none.",
                    )
                } else {
                    RichText::new("Not better on fresh games: keep your deck.").color(BAD)
                });
                ui.add_space(6.0);
                let (mut adds, mut cuts) = (Vec::new(), Vec::new());
                let names: std::collections::BTreeSet<&String> =
                    r.start.cards.keys().chain(r.best.cards.keys()).collect();
                for n in names {
                    let (a, b) = (r.start.count_of(n), r.best.count_of(n));
                    if b > a {
                        adds.push(format!("{} {}", b - a, n));
                    } else if a > b {
                        cuts.push(format!("{} {}", a - b, n));
                    }
                }
                ui.columns(2, |cols| {
                    cols[0].label(RichText::new("In").strong().color(GOOD));
                    for a in &adds {
                        cols[0].label(format!("+ {}", a));
                    }
                    cols[1].label(RichText::new("Out").strong().color(BAD));
                    for c in &cuts {
                        cols[1].label(format!("- {}", c));
                    }
                });
            }
        }
        if !r.steps.is_empty() {
            ui.add_space(6.0);
            ui.label(
                RichText::new("How it got there (each step measured on games that didn't pick it)")
                    .strong(),
            );
            egui::Grid::new("opt_steps").striped(true).show(ui, |ui| {
                for st in &r.steps {
                    ui.label(gain_text(&st.gain));
                    ui.add(egui::Label::new(st.what.as_str()).wrap());
                    ui.end_row();
                }
            });
        }
        for n in &r.notes {
            ui.label(RichText::new(n).small().weak());
        }
    });
}

fn show_claims(ui: &mut Ui, c: &ClaimReport) {
    super::home::card(ui, "Claims tested in your deck", |ui| {
        ui.label(
            RichText::new(format!(
                "{} cards played in your deck in place of the card shown ({} skipped: off-colour, unclaimable or already in). Gain = win-rate points over your deck; the top rows were measured again on fresh games.",
                c.tested, c.skipped
            ))
            .small()
            .weak(),
        );
        egui::Grid::new("claims_grid")
            .striped(true)
            .min_col_width(40.0)
            .show(ui, |ui| {
                for h in ["Gain", "Card", "Cost", "Replaces", ""] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for cl in c.claims.iter().take(30) {
                    let m = cl.confirmed.unwrap_or(cl.screened);
                    ui.label(gain_text(&m));
                    ui.label(cl.card.as_str());
                    ui.label(if cl.vp == 0 {
                        "shop/tree".to_string()
                    } else {
                        format!("{} VP", cl.vp)
                    });
                    ui.label(RichText::new(cl.cut.as_str()).weak());
                    ui.label(
                        RichText::new(if cl.confirmed.is_some() {
                            "confirmed"
                        } else {
                            "screened only"
                        })
                        .small()
                        .weak(),
                    );
                    ui.end_row();
                }
            });
    });
}
