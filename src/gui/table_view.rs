//! Season & Rules tab, "My table": the user's real opponents (data/table.json) - add, edit,
//! remove, paste a decklist or auto-build a strong version of their commander, and tick
//! who is at the next table (the Playtest Lab can play against them).

use super::GuiApp;
use crate::table::RealOpponent;
use eframe::egui::{self, Color32, RichText, Ui};

const GOOD: Color32 = Color32::from_rgb(110, 220, 120);
const NOTE: Color32 = Color32::from_rgb(255, 190, 110);

#[derive(Default)]
pub struct TableUi {
    /// The opponent being edited.
    pub editing: Option<usize>,
}

impl GuiApp {
    fn save_table(&mut self) {
        if let Err(e) = self.table.save(&self.data_dir) {
            self.status = format!("Couldn't save {}: {}", crate::table::FILE, e);
        }
    }

    pub(super) fn table_section(&mut self, ui: &mut Ui) {
        egui::CollapsingHeader::new(RichText::new("My table (real opponents)").strong())
            .default_open(true)
            .show(ui, |ui| self.table_body(ui));
    }

    fn table_body(&mut self, ui: &mut Ui) {
        ui.label(
            RichText::new(format!(
                "The people you actually play against: paste each one's decklist, or pick their commander and auto-build \
                 a strong version (every league card available). Tick who sits at your next table - the Playtest Lab and \
                 the card synergy test can then play against exactly those decks (\"Test against: my next table\"). \
                 Saved on this PC only, in {}.",
                crate::table::FILE
            ))
            .small(),
        );
        let mut changed = false;
        let mut remove = None;
        let mut edit_toggle = None;
        let infos: Vec<Result<(String, u32), String>> = self
            .table
            .opponents
            .iter()
            .map(|o| {
                o.playable(&self.db)
                    .map(|d| (d.commander.clone().unwrap_or_default(), d.main_count()))
            })
            .collect();
        if self.table.opponents.is_empty() {
            ui.label(RichText::new("No real opponents yet.").weak());
        } else {
            egui::Grid::new("my_table").striped(true).show(ui, |ui| {
                ui.label(RichText::new("Next table").strong());
                ui.label(RichText::new("Player").strong());
                ui.label(RichText::new("Commander").strong());
                ui.label(RichText::new("Deck").strong());
                ui.label("");
                ui.end_row();
                for (i, o) in self.table.opponents.iter_mut().enumerate() {
                    changed |= ui
                        .checkbox(&mut o.at_next_table, "")
                        .on_hover_text("Plays at your next table")
                        .changed();
                    ui.label(if o.player.trim().is_empty() {
                        RichText::new("(no name)").weak()
                    } else {
                        RichText::new(&o.player).strong()
                    });
                    match &infos[i] {
                        Ok((cmd, n)) => {
                            ui.label(cmd.as_str());
                            ui.label(RichText::new(format!("{} cards", n)).color(GOOD));
                        }
                        Err(e) => {
                            ui.label(o.commander.as_str());
                            ui.label(RichText::new(format!("not playable: {}", e)).color(NOTE));
                        }
                    }
                    ui.horizontal(|ui| {
                        let editing = self.table_ui.editing == Some(i);
                        if ui
                            .small_button(if editing { "Done" } else { "Edit" })
                            .clicked()
                        {
                            edit_toggle = Some(i);
                        }
                        if ui.small_button("Remove").clicked() {
                            remove = Some(i);
                        }
                        if !o.decklist.trim().is_empty() && ui.small_button("Copy deck").clicked() {
                            ui.ctx().copy_text(o.decklist.clone());
                        }
                    });
                    ui.end_row();
                }
            });
        }
        if let Some(i) = edit_toggle {
            self.table_ui.editing = if self.table_ui.editing == Some(i) {
                None
            } else {
                Some(i)
            };
        }
        if let Some(i) = remove {
            let name = self.table.opponents.remove(i).player;
            self.table_ui.editing = None;
            self.status = format!("Removed {} from your table.", name);
            changed = true;
        }
        ui.horizontal(|ui| {
            if ui.button("+ Add opponent").clicked() {
                self.table.opponents.push(RealOpponent {
                    at_next_table: true,
                    ..Default::default()
                });
                self.table_ui.editing = Some(self.table.opponents.len() - 1);
                changed = true;
            }
            let n = self.table.next_decks(&self.db).len();
            ui.label(
                RichText::new(format!("{} playable decks ticked for the next table", n))
                    .small()
                    .weak(),
            );
        });
        if let Some(i) = self
            .table_ui
            .editing
            .filter(|&i| i < self.table.opponents.len())
        {
            changed |= self.table_editor(ui, i);
        }
        if changed {
            self.save_table();
        }
    }

    /// The editor for opponent `i`; true when something changed.
    fn table_editor(&mut self, ui: &mut Ui, i: usize) -> bool {
        let mut changed = false;
        let mut build = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width().min(760.0));
            let o = &mut self.table.opponents[i];
            ui.horizontal(|ui| {
                ui.label("Player:");
                changed |= ui
                    .add(egui::TextEdit::singleline(&mut o.player).desired_width(180.0))
                    .changed();
                ui.label("Commander:");
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut o.commander)
                            .hint_text("Type a commander's name")
                            .desired_width(240.0),
                    )
                    .changed();
            });
            // Commander suggestions while the name isn't a known commander.
            let q = crate::db::normalize(&o.commander);
            let known = self
                .db
                .get(o.commander.trim())
                .is_some_and(|c| c.can_be_commander() && c.name == o.commander.trim());
            if q.len() >= 2 && !known {
                ui.horizontal_wrapped(|ui| {
                    for c in self
                        .db
                        .cards
                        .iter()
                        .filter(|c| c.can_be_commander())
                        .filter(|c| crate::db::normalize(&c.name).contains(&q))
                        .take(8)
                    {
                        if ui.small_button(&c.name).clicked() {
                            o.commander = c.name.clone();
                            changed = true;
                        }
                    }
                });
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(known, egui::Button::new("Auto-build strong version"))
                    .on_hover_text("Replaces the decklist with an auto-build of this commander from every league card - a strong guess when you don't know their list.")
                    .clicked()
                {
                    build = true;
                }
                if ui.button("Clear decklist").clicked() {
                    o.decklist.clear();
                    changed = true;
                }
            });
            ui.label(
                RichText::new("Decklist (paste from Moxfield / Archidekt / Mana Mixer: \"1 Card Name\" lines, a Commander section optional):")
                    .small(),
            );
            egui::ScrollArea::vertical()
                .id_salt(("table_list", i))
                .max_height(260.0)
                .min_scrolled_height(200.0)
                .show(ui, |ui| {
                    changed |= ui
                        .add(
                            egui::TextEdit::multiline(&mut o.decklist)
                                .desired_rows(8)
                                .desired_width(f32::INFINITY)
                                .font(egui::TextStyle::Monospace),
                        )
                        .changed();
                });
        });
        if build {
            let o = &self.table.opponents[i];
            match crate::table::strong_build(&o.commander, &self.db, &self.rules) {
                Ok(d) => {
                    self.status = format!(
                        "Built a strong {} deck for {} ({} cards).",
                        o.commander,
                        if o.player.is_empty() {
                            "this opponent"
                        } else {
                            &o.player
                        },
                        d.main_count()
                    );
                    self.table.opponents[i].decklist = d.to_text();
                    changed = true;
                }
                Err(e) => self.status = e,
            }
        }
        // A pasted list that names its commander fills the commander field.
        let o = &mut self.table.opponents[i];
        if changed && o.commander.trim().is_empty() {
            if let Some(c) = crate::deck::Deck::from_text("", &o.decklist, &self.db).commander {
                o.commander = c;
            }
        }
        changed
    }
}
