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
    /// The trade-picks paste box.
    pub picks_text: String,
    /// The game log's "seen cards" box.
    pub seen_text: String,
    /// The last real-vs-simulated verdict.
    pub verdict: Option<String>,
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
                 the card synergy test can then play against exactly those decks (\"Play against: my next table\"). \
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
                o.playable(&self.db).map(|d| {
                    (
                        d.commander.clone().unwrap_or_default(),
                        d.total_count(&self.rules),
                    )
                })
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
                ui.label(RichText::new("Trade picks").strong()).on_hover_text(
                    "Cards they got in the trade market. The ones in their colours are assumed to be \
                     in their deck (in place of its weakest cards) when you play against them.",
                );
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
                    let picks: Vec<String> = self
                        .table
                        .picks
                        .iter()
                        .find(|(k, _)| k.trim().eq_ignore_ascii_case(o.player.trim()))
                        .map(|(_, v)| v.iter().map(|p| p.card.clone()).collect())
                        .unwrap_or_default();
                    if picks.is_empty() {
                        ui.label(RichText::new("none tracked").weak());
                    } else {
                        ui.label(format!("{} cards", picks.len()))
                            .on_hover_text(picks.join("\n"));
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
            let n = self.table.next_count(&self.db);
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
        ui.add_space(6.0);
        egui::CollapsingHeader::new("Everyone's trade picks")
            .id_salt("trade_picks")
            .show(ui, |ui| {
                ui.label(
                    RichText::new(
                        "Paste each player's trade-market cards: the player's name and a colon on one line, then one \
                         card per line (a Mana Mixer league export works too). A player's list replaces what was \
                         tracked for them. Cards in an opponent's colours are assumed to be in their deck for playtests.",
                    )
                    .small(),
                );
                ui.add(
                    egui::TextEdit::multiline(&mut self.table_ui.picks_text)
                        .hint_text("Drake:\nLlanowar Elves\nPelakka Wurm\n\nJD:\nUltron, Artificial Malevolence")
                        .desired_rows(4)
                        .desired_width(f32::INFINITY),
                );
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!self.table_ui.picks_text.trim().is_empty(), egui::Button::new("Import picks"))
                        .clicked()
                    {
                        match self.table.import_picks(&self.table_ui.picks_text) {
                            Ok((p, c)) => {
                                self.status = format!("Tracked {} trade picks for {} players.", c, p);
                                self.table_ui.picks_text.clear();
                                changed = true;
                            }
                            Err(e) => self.status = e,
                        }
                    }
                    if !self.table.picks.is_empty() && ui.button("Forget all picks").clicked() {
                        self.table.picks.clear();
                        changed = true;
                    }
                });
                for (player, list) in &self.table.picks {
                    ui.label(
                        RichText::new(format!(
                            "{}: {}",
                            player,
                            list.iter().map(|p| p.card.as_str()).collect::<Vec<_>>().join(" · ")
                        ))
                        .small(),
                    );
                }
            });
        ui.add_space(6.0);
        egui::CollapsingHeader::new("Game log (real games)")
            .id_salt("game_log")
            .show(ui, |ui| self.game_log_ui(ui));
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

impl GuiApp {
    /// Real games: log results and cards seen, and compare the record with the simulator.
    fn game_log_ui(&mut self, ui: &mut Ui) {
        let mut log = crate::games::GameLog::load(&self.data_dir);
        ui.label(
            RichText::new(
                "After each real game, log the result. Cards an opponent was seen playing go into their deck for \
                 playtests, and your real record is compared with what the simulator predicts - the program is only \
                 as good as its simulator. Saved on this PC only, in games.json.",
            )
            .small(),
        );
        let mut changed = false;
        ui.horizontal(|ui| {
            let names: Vec<String> = self.table.next_table().map(|o| o.player.clone()).collect();
            for won in [true, false] {
                if ui
                    .button(if won { "I won" } else { "I lost" })
                    .on_hover_text(format!(
                        "Logs a game against your ticked next table: {}",
                        if names.is_empty() {
                            "nobody ticked".to_string()
                        } else {
                            names.join(", ")
                        }
                    ))
                    .clicked()
                {
                    log.games.push(crate::games::GameRecord {
                        date: crate::ratings::today(),
                        session: self.rules.session,
                        our_commander: self.deck.commander.clone().unwrap_or_default(),
                        opponents: self
                            .table
                            .next_table()
                            .map(|o| crate::games::Seat {
                                player: o.player.clone(),
                                commander: o.commander.clone(),
                                seen: Vec::new(),
                            })
                            .collect(),
                        won,
                        winner: if won { "me".into() } else { String::new() },
                        notes: String::new(),
                    });
                    changed = true;
                }
            }
            if !log.games.is_empty() && ui.button("Undo last game").clicked() {
                log.games.pop();
                changed = true;
            }
        });
        ui.label(
            RichText::new(
                "Cards seen in the last game (one line per player: \"JD: Opt; Refute\"):",
            )
            .small(),
        );
        ui.add(
            egui::TextEdit::multiline(&mut self.table_ui.seen_text)
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        );
        if ui
            .add_enabled(
                !self.table_ui.seen_text.trim().is_empty() && !log.games.is_empty(),
                egui::Button::new("Add seen cards to the last game"),
            )
            .clicked()
        {
            let mut notes = Vec::new();
            let text = self.table_ui.seen_text.clone();
            if let Some(g) = log.games.last_mut() {
                for line in text.lines() {
                    let Some((player, cards)) = line.split_once(':') else {
                        continue;
                    };
                    let key = player.trim().to_lowercase();
                    let Some(seat) = g
                        .opponents
                        .iter_mut()
                        .find(|s| s.player.to_lowercase() == key)
                    else {
                        notes.push(format!("{} wasn't in the last game", player.trim()));
                        continue;
                    };
                    for c in cards.split(';').map(str::trim).filter(|c| !c.is_empty()) {
                        match self.db.get(c) {
                            Some(card) if !seat.seen.contains(&card.name) => {
                                seat.seen.push(card.name.clone())
                            }
                            Some(_) => {}
                            None => notes.push(format!("not a card: {}", c)),
                        }
                    }
                }
            }
            self.status = if notes.is_empty() {
                "Seen cards added: their decks include them for playtests.".into()
            } else {
                notes.join("; ")
            };
            self.table_ui.seen_text.clear();
            changed = true;
        }
        if changed {
            if let Err(e) = log.save(&self.data_dir) {
                self.status = format!("Couldn't save games.json: {}", e);
            }
            self.table.load_seen(&self.data_dir);
            self.table_ui.verdict = None;
        }
        let r = log.record(None);
        if r.played > 0 {
            ui.label(
                RichText::new(format!(
                    "Record: {}-{} ({:.0}%)",
                    r.won,
                    r.played - r.won,
                    r.rate() * 100.0
                ))
                .strong(),
            );
            let mut players: Vec<String> = Vec::new();
            for g in &log.games {
                for s in &g.opponents {
                    if !players.iter().any(|p| p.eq_ignore_ascii_case(&s.player)) {
                        players.push(s.player.clone());
                    }
                }
            }
            for p in players {
                let a = log.against(&p);
                let seen = log.seen_by(&p);
                ui.label(
                    RichText::new(format!(
                        "vs {}: {}-{}{}",
                        p,
                        a.won,
                        a.played - a.won,
                        if seen.is_empty() {
                            String::new()
                        } else {
                            format!(" · seen: {}", seen.join(", "))
                        }
                    ))
                    .small(),
                );
            }
            if ui
                .add_enabled(self.deck.commander.is_some(), egui::Button::new("Compare with the simulator"))
                .on_hover_text("Plays your current deck against your next table (2000 games, a few seconds) and compares with your real record")
                .clicked()
            {
                let g = crate::table::gauntlet(&self.table, &self.db);
                self.table_ui.verdict = Some(if g.is_empty() {
                    "Tick your next table first.".to_string()
                } else {
                    let p = crate::playtest::Prepared::new(&self.deck, &self.db);
                    let o = crate::playtest::lab::play_all(
                        &[&p],
                        &g,
                        &self.rules,
                        2000,
                        0x6A3E,
                        &std::sync::atomic::AtomicUsize::new(0),
                        &std::sync::atomic::AtomicBool::new(false),
                    );
                    log.record(self.deck.commander.as_deref()).verdict(o.win_rate(0))
                });
            }
            if let Some(v) = &self.table_ui.verdict {
                ui.label(v.as_str());
            }
        } else {
            ui.label(RichText::new("No games logged yet.").weak());
        }
    }
}
