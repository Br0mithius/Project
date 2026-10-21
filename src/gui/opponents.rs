//! Season & Rules tab: the simulated opponents the playtests use - each one's deck and
//! season log - with pinning of a real commander and a "simulate again" button.

use super::GuiApp;
use crate::season::Season;
use eframe::egui::{self, RichText, Ui};
use std::sync::{Arc, Mutex};

/// A season being simulated (or done) on a background thread, and what it was asked for.
#[derive(Clone)]
struct Job {
    key: String,
    result: Arc<Mutex<Option<Arc<Season>>>>,
}

impl GuiApp {
    pub(super) fn opponents_section(&mut self, ui: &mut Ui) {
        egui::CollapsingHeader::new(RichText::new("Simulated opponents (playtest decks)").strong())
            .default_open(false)
            .show(ui, |ui| self.opponents_body(ui));
    }

    fn opponents_body(&mut self, ui: &mut Ui) {
        ui.label(
            RichText::new(
                "The other players' seasons are simulated up to the session you're building for: their packs, \
                 Sessions 2-4 table drafts, a rebuild each session (cutting weaker cards, singleton from Session 6) \
                 and a trading round each session (10 VP minimum, claims from a shared Public Pool at 2x VP with the \
                 rarity floor, Skill Tree staples). Your imported Public Pool is used for this session's claims. \
                 Pin an opponent's real commander and the simulation builds around it.",
            )
            .small(),
        );
        let id = egui::Id::unique("opponents_job");
        let key = format!(
            "{}|{}|{}|{}|{:?}|{}",
            self.rules.session,
            self.rules.deck_size,
            self.rules.singleton,
            self.rules.players,
            self.rules.roster,
            crate::opponents::inputs_stamp(&self.rules)
        );
        let job: Option<Job> = ui.data(|d| d.get_temp(id));
        let job = match job {
            Some(j) if j.key == key => j,
            _ => {
                let j = Job {
                    key,
                    result: Arc::new(Mutex::new(None)),
                };
                let (db, rules, out) = (self.db.clone(), self.rules.clone(), j.result.clone());
                std::thread::spawn(move || {
                    let s =
                        crate::opponents::season(&db, &rules, crate::ranking::GAUNTLET_SEEDS[0]);
                    *out.lock().unwrap() = Some(s);
                });
                ui.data_mut(|d| d.insert_temp(id, j.clone()));
                j
            }
        };
        let season = job.result.lock().unwrap().clone();
        let Some(season) = season else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Simulating the season (cached afterwards)...");
            });
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(200));
            return;
        };
        ui.label(
            RichText::new(format!(
                "After Session {}'s trading round{}.",
                season.session,
                if season.real_pool {
                    ", claiming from your imported Public Pool"
                } else {
                    ""
                }
            ))
            .small()
            .weak(),
        );
        for o in &season.opponents {
            let pinned = match o.pinned.as_str() {
                "commander" => "  (commander pinned)",
                "decklist" => "  (real decklist)",
                _ => "",
            };
            let title = format!(
                "{} - {}{}",
                o.name,
                o.commander.as_deref().unwrap_or("no commander"),
                pinned
            );
            egui::CollapsingHeader::new(title)
                .id_salt(("opponent", &o.name))
                .show(ui, |ui| {
                    ui.label(format!(
                        "{} VP banked, {} Skill Tree staples, {} cards in their pool.",
                        o.vp,
                        o.staples,
                        o.pool.values().sum::<u32>()
                    ));
                    let edit = egui::Id::unique(("pin_text", &o.name));
                    let mut text: String = ui.data(|d| d.get_temp(edit)).unwrap_or_default();
                    ui.horizontal(|ui| {
                        ui.label("Real commander:");
                        ui.add(egui::TextEdit::singleline(&mut text).desired_width(220.0));
                        if ui.button("Pin").clicked() {
                            let what = format!("commander {}", text.trim());
                            self.status = match crate::opponents::pin(
                                &self.db,
                                &self.rules,
                                &o.name,
                                &what,
                            ) {
                                Ok(m) | Err(m) => m,
                            };
                        }
                        if !o.pinned.is_empty() && ui.button("Clear pinned info").clicked() {
                            self.status = match crate::opponents::pin(
                                &self.db,
                                &self.rules,
                                &o.name,
                                "clear",
                            ) {
                                Ok(m) | Err(m) => m,
                            };
                        }
                    });
                    ui.data_mut(|d| d.insert_temp(edit, text));
                    if ui.small_button("Copy their deck").clicked() {
                        if let Some(d) = o.deck() {
                            ui.ctx().copy_text(d.to_text());
                        }
                    }
                    for l in &o.log {
                        for line in l.lines() {
                            ui.label(RichText::new(line).small().monospace());
                        }
                    }
                });
        }
        if ui
            .button("Simulate again")
            .on_hover_text("Forget the cached opponents (data/gauntlet_cache.json) and simulate the season again.")
            .clicked()
        {
            if let Some(dir) = self.rules.data_dir.clone() {
                crate::opponents::refresh(&dir);
            }
            ui.data_mut(|d| d.remove::<Job>(id));
        }
    }
}
