//! The Play tab: you play the Deck Builder's deck by hand against three simulated league
//! opponents, with the simulator's own rules engine (`playtest::human`). Mana is tapped by
//! hand; "Report bug" saves everything needed to replay the game to that moment.

use super::GuiApp;
use crate::deck::Deck;
use crate::playtest::human::*;
use eframe::egui;
use egui::{Color32, Panel, RichText, ScrollArea, Ui};
use std::sync::mpsc::Receiver;

const GOOD: Color32 = Color32::from_rgb(110, 220, 120);
const WARN: Color32 = Color32::from_rgb(255, 190, 110);
const BAD: Color32 = Color32::from_rgb(255, 120, 100);
const MANA: [Color32; 6] = [
    Color32::from_rgb(240, 230, 190),
    Color32::from_rgb(120, 170, 240),
    Color32::from_rgb(170, 150, 170),
    Color32::from_rgb(240, 120, 100),
    Color32::from_rgb(110, 200, 120),
    Color32::from_rgb(190, 190, 190),
];

#[derive(Default)]
pub(super) struct PlayState {
    handle: Option<PlayHandle>,
    setup: Option<Setup>,
    /// The pending question, and whether manual tapping and edits are allowed now.
    ask: Option<Ask>,
    edits_ok: bool,
    view: Option<GameView>,
    log: Vec<String>,
    /// Answers sent (a fallback for reports if the engine stopped).
    sent: Vec<Answer>,
    over: Option<usize>,
    transcript: Vec<Answer>,
    failed: Option<String>,
    // Setting up a game.
    seed: u64,
    opts: PlayOptions,
    opps: Option<Vec<Deck>>,
    opps_job: Option<Receiver<Vec<Deck>>>,
    pick: [usize; 3],
    // The current question's controls.
    picks: Vec<bool>,
    number: i32,
    attacks: Vec<Option<usize>>,
    blocks: Vec<Option<u32>>,
    // Bug reports.
    report_open: bool,
    report_note: String,
    report_wait: bool,
    report_status: String,
    log_filter: String,
}

impl PlayState {
    fn running(&self) -> bool {
        self.handle.is_some() && self.over.is_none() && self.failed.is_none()
    }

    /// Sets the question's controls to the bot's suggestion.
    fn reset_controls(&mut self) {
        match &self.ask {
            Some(Ask::Pick {
                options, suggested, ..
            }) => {
                self.picks = (0..options.len()).map(|k| suggested.contains(&k)).collect();
            }
            Some(Ask::Number { suggested, .. }) => self.number = *suggested,
            Some(Ask::Attack {
                attackers,
                suggested,
                ..
            }) => {
                self.attacks = attackers
                    .iter()
                    .map(|(id, _)| suggested.iter().find(|s| s.0 == *id).map(|s| s.1))
                    .collect();
            }
            Some(Ask::Block {
                blockers,
                suggested,
                ..
            }) => {
                self.blocks = blockers
                    .iter()
                    .map(|(id, _, _)| suggested.iter().find(|s| s.0 == *id).map(|s| s.1))
                    .collect();
            }
            _ => {}
        }
    }
}

/// "{G}" etc. as a coloured label.
fn pool_label(ui: &mut Ui, pool: &[u8; 6]) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Mana pool:");
        if pool.iter().all(|&n| n == 0) {
            ui.label(RichText::new("empty").weak());
        }
        for (c, &n) in pool.iter().enumerate() {
            if n > 0 {
                ui.label(
                    RichText::new(format!("{}x{{{}}}", n, POOL_NAMES[c]))
                        .strong()
                        .color(MANA[c]),
                );
            }
        }
    });
}

/// A log line showing a bot's hidden cards or the AI's reasoning (shown only with "show the
/// bots' hands"; bug reports keep every line).
fn private_to_a_bot(line: &str, seat: usize, players: usize) -> bool {
    (0..players).filter(|&o| o != seat).any(|o| {
        line.contains(&format!("AI P{} ", o))
            || line.contains(&format!("P{} opening hand", o))
            || line.contains(&format!("P{} draws ", o))
            || line.contains(&format!("P{} puts ", o)) && line.contains("on the bottom")
            || line.contains(&format!("P{} life", o)) && line.contains("hand before the draw")
            || line.contains(&format!("P{} holds ", o))
    })
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The game as text (for a report when the engine can no longer dump it).
fn view_text(v: &GameView) -> String {
    let mut s = format!("Round {}, active P{}, {}\n", v.round, v.active, v.phase);
    for (p, pl) in v.players.iter().enumerate() {
        s += &format!(
            "\nP{} {} - life {}, poison {}, alive {}, hand {} ({}), library {}\n  graveyard: {}\n  exile: {}\n",
            p,
            pl.name,
            pl.life,
            pl.poison,
            pl.alive,
            pl.hand_count,
            pl.hand.join("; "),
            pl.library,
            pl.graveyard.join("; "),
            pl.exile.join("; ")
        );
        for x in v.perms.iter().filter(|x| x.ctrl == p) {
            s += &format!("  {:?}\n", x);
        }
    }
    s
}

impl GuiApp {
    /// Reads what the engine sent.
    fn play_poll(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.play.opps_job {
            if let Ok(d) = rx.try_recv() {
                let n = d.len().max(1);
                self.play.pick = [0 % n, 1 % n, 2 % n];
                self.play.opps = Some(d);
                self.play.opps_job = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
        }
        let mut msgs = Vec::new();
        if let Some(h) = &self.play.handle {
            while let Ok(m) = h.rx.try_recv() {
                msgs.push(m);
            }
        }
        for m in msgs {
            match m {
                FromEngine::Ask {
                    ask,
                    view,
                    edits_ok,
                } => {
                    self.play.log.extend(view.new_log.iter().cloned());
                    self.play.view = Some(*view);
                    self.play.ask = Some(ask);
                    self.play.edits_ok = edits_ok;
                    self.play.reset_controls();
                }
                FromEngine::Dump { text, answers } => {
                    if self.play.report_wait {
                        self.play.report_wait = false;
                        self.save_report(&text, answers);
                    }
                }
                FromEngine::Over {
                    winner,
                    view,
                    transcript,
                } => {
                    self.play.log.extend(view.new_log.iter().cloned());
                    self.play.view = Some(*view);
                    self.play.over = Some(winner);
                    self.play.transcript = transcript;
                    self.play.ask = None;
                }
                FromEngine::Failed(msg) => {
                    self.play.failed = Some(msg);
                    self.play.ask = None;
                }
            }
        }
        if self.play.running() && self.play.ask.is_none() {
            ctx.request_repaint_after(std::time::Duration::from_millis(40));
        }
    }

    fn play_send(&mut self, a: Answer) {
        if let Some(h) = &self.play.handle {
            let _ = h.tx.send(ToEngine::Answer(a.clone()));
            self.play.sent.push(a);
            self.play.ask = None;
        }
    }

    fn start_game(&mut self) {
        let Some(opps) = &self.play.opps else { return };
        if opps.is_empty() {
            return;
        }
        let mut decks = vec![(self.deck.name.clone(), self.deck.to_text())];
        for &k in &self.play.pick {
            let d = &opps[k % opps.len()];
            decks.push((d.name.clone(), d.to_text()));
        }
        let setup = Setup {
            seed: self.play.seed,
            seat: 0,
            opts: self.play.opts.clone(),
            rules: self.rules.clone(),
            decks,
        };
        let handle = start(setup.clone(), self.db.clone());
        let opps_keep = self.play.opps.take();
        let job = self.play.opps_job.take();
        let (seed, opts, pick) = (self.play.seed, self.play.opts.clone(), self.play.pick);
        self.play = PlayState {
            handle: Some(handle),
            setup: Some(setup),
            opps: opps_keep,
            opps_job: job,
            seed,
            opts,
            pick,
            ..Default::default()
        };
        self.status = "Game started: you are P0 (bottom). Tap lands for mana, then cast.".into();
    }

    /// Writes the bug report file (`answers`: the engine's record, which replays the game).
    fn save_report(&mut self, dump: &str, answers: Vec<Answer>) {
        let Some(setup) = self.play.setup.clone() else {
            return;
        };
        let report = Report {
            note: self.play.report_note.clone(),
            setup,
            answers,
        };
        let text = report_text(&report, self.play.ask.as_ref(), dump, &self.play.log);
        let dir = self.data_dir.join("bug_reports");
        let path = dir.join(format!(
            "report-{}-{}.txt",
            crate::ratings::today(),
            now_secs() % 100_000
        ));
        self.play.report_status =
            match std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, text)) {
                Ok(()) => {
                    self.play.report_open = false;
                    self.play.report_note.clear();
                    format!("Saved {} - send this file to Claude.", path.display())
                }
                Err(e) => format!("Couldn't save the report: {}", e),
            };
        self.status = self.play.report_status.clone();
    }

    fn request_report(&mut self) {
        if self.play.running() && self.play.ask.is_some() {
            // The engine answers with a full dump and its record of the answers.
            if let Some(h) = &self.play.handle {
                let _ = h.tx.send(ToEngine::Dump);
                self.play.report_wait = true;
                self.play.report_status = "Collecting the game state...".into();
            }
        } else {
            let dump = self.play.view.as_ref().map(view_text).unwrap_or_default();
            let answers = if self.play.over.is_some() {
                self.play.transcript.clone()
            } else {
                self.play.sent.clone()
            };
            let dump = match &self.play.failed {
                Some(f) => format!("ENGINE CRASH: {}\n\n{}", f, dump),
                None => dump,
            };
            self.save_report(&dump, answers);
        }
    }

    /// The Play tab.
    pub(super) fn play_tab(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.play_poll(&ctx);
        if self.play.handle.is_none() {
            egui::CentralPanel::default().show(ui, |ui| self.play_setup(ui));
            return;
        }
        Panel::right("play_side")
            .resizable(true)
            .default_size(440.0)
            .show(ui, |ui| self.play_side(ui));
        egui::CentralPanel::default().show(ui, |ui| {
            ScrollArea::vertical()
                .id_salt("play_board")
                .auto_shrink([false, false])
                .show(ui, |ui| self.play_board(ui));
        });
        if self.play.report_open {
            let mut open = true;
            egui::Window::new("Report a bug")
                .open(&mut open)
                .default_width(460.0)
                .show(&ctx, |ui| {
                    ui.label(
                        "Describe what went wrong (what you expected, what happened). The report \
                         also saves the full log, every zone and permanent, the pending question, \
                         the decks, the seed and all your answers, so the game can be replayed to \
                         this exact moment.",
                    );
                    ui.add(
                        egui::TextEdit::multiline(&mut self.play.report_note)
                            .desired_rows(5)
                            .desired_width(f32::INFINITY),
                    );
                    if self.play.report_wait {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Collecting the game state...");
                        });
                    } else if ui.button(RichText::new("Save report").strong()).clicked() {
                        self.request_report();
                    }
                    if !self.play.report_status.is_empty() {
                        ui.label(RichText::new(&self.play.report_status).small());
                    }
                });
            if !open {
                self.play.report_open = false;
            }
        }
    }

    fn play_setup(&mut self, ui: &mut Ui) {
        ui.heading("Play a game by hand");
        ui.label(
            "You play the deck from the Deck Builder (seat P0) against three simulated league \
             opponents, with the same rules engine as the Playtest Lab. You tap your own mana; \
             every other decision of yours is asked in the side panel, and the bot's suggestion \
             is always shown. If something looks wrong, press \"Report bug\".",
        );
        ui.add_space(8.0);
        ui.label(format!(
            "Your deck: {} ({} cards{})",
            self.deck.name,
            self.deck.total_count(&self.rules),
            self.deck
                .commander
                .as_ref()
                .map(|c| format!(", commander {}", c))
                .unwrap_or_else(|| ", no commander".into())
        ));
        if self.play.seed == 0 {
            self.play.seed = now_secs() % 1_000_000;
        }
        if self.play.opps.is_none() && self.play.opps_job.is_none() {
            let (db, rules) = (self.db.clone(), self.rules.clone());
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let d = crate::opponents::decks(&db, &rules, crate::ranking::GAUNTLET_SEEDS[0]);
                let _ = tx.send(d);
            });
            self.play.opps_job = Some(rx);
        }
        let ctx = ui.ctx().clone();
        self.play_poll(&ctx);
        match &self.play.opps {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Simulating the league's opponent decks (cached afterwards)...");
                });
            }
            Some(opps) if opps.is_empty() => {
                ui.label(RichText::new("No opponent decks for these league rules.").color(BAD));
            }
            Some(opps) => {
                let names: Vec<String> = opps
                    .iter()
                    .map(|d| {
                        format!(
                            "{} ({})",
                            d.commander.clone().unwrap_or_else(|| d.name.clone()),
                            d.name
                        )
                    })
                    .collect();
                for k in 0..3 {
                    ui.horizontal(|ui| {
                        ui.label(format!("Opponent P{}:", k + 1));
                        let cur = self.play.pick[k] % names.len();
                        egui::ComboBox::from_id_salt(("play_opp", k))
                            .selected_text(&names[cur])
                            .width(380.0)
                            .show_ui(ui, |ui| {
                                for (i, n) in names.iter().enumerate() {
                                    ui.selectable_value(&mut self.play.pick[k], i, n);
                                }
                            });
                    });
                }
            }
        }
        ui.horizontal(|ui| {
            ui.label("Seed:");
            ui.add(egui::DragValue::new(&mut self.play.seed));
            if ui.button("New seed").clicked() {
                self.play.seed = now_secs() % 1_000_000;
            }
        });
        ui.checkbox(
            &mut self.play.opts.auto_mana,
            "Pay costs automatically (no manual tapping)",
        );
        ui.checkbox(
            &mut self.play.opts.stop_on_bot_spells,
            "Stop when a bot casts a spell",
        );
        ui.checkbox(
            &mut self.play.opts.step_bot_turns,
            "Stop when a bot attacks and at the end of each bot turn",
        );
        ui.checkbox(
            &mut self.play.opts.stop_every_window,
            "Ask at every instant-speed window (even with nothing to cast)",
        );
        ui.checkbox(
            &mut self.play.opts.show_bot_hands,
            "Show the bots' hands (for checking the AI)",
        );
        ui.add_space(8.0);
        let ready =
            self.play.opps.as_ref().is_some_and(|o| !o.is_empty()) && self.deck.commander.is_some();
        if ui
            .add_enabled(
                ready,
                egui::Button::new(RichText::new("Start game").strong()),
            )
            .clicked()
        {
            self.start_game();
        }
        if self.deck.commander.is_none() {
            ui.label(RichText::new("Pick a commander in the Deck Builder first.").color(WARN));
        }
    }

    // ------------------------------------------------------------ the side panel

    fn play_side(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if ui
                .button(RichText::new("Report bug").strong().color(BAD))
                .on_hover_text("Save the log, the full game state and your answers to a file")
                .clicked()
            {
                self.play.report_open = true;
                self.play.report_status.clear();
            }
            if ui.button("New game").clicked() {
                // Dropping the channels ends the engine thread.
                let (opps, seed, opts, pick) = (
                    self.play.opps.take(),
                    now_secs() % 1_000_000,
                    self.play.opts.clone(),
                    self.play.pick,
                );
                self.play = PlayState {
                    opps,
                    seed,
                    opts,
                    pick,
                    ..Default::default()
                };
                return;
            }
            if let Some(s) = &self.play.setup {
                ui.label(RichText::new(format!("seed {}", s.seed)).small().weak());
            }
        });
        ui.separator();
        if let Some(w) = self.play.over {
            let text = if w == 0 {
                "You won!".to_string()
            } else if w >= 8 {
                "The game is a draw.".to_string()
            } else {
                format!("P{} won.", w)
            };
            ui.heading(RichText::new(text).color(if w == 0 { GOOD } else { WARN }));
        }
        if let Some(f) = &self.play.failed {
            ui.label(
                RichText::new(format!(
                    "The engine stopped with an error: {}. Please press Report bug.",
                    f
                ))
                .color(BAD),
            );
        }
        self.question(ui);
        ui.separator();
        self.mana_panel(ui);
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Log");
            ui.add(
                egui::TextEdit::singleline(&mut self.play.log_filter)
                    .hint_text("filter")
                    .desired_width(140.0),
            );
        });
        let filter = self.play.log_filter.to_lowercase();
        let (seat, players) = self
            .play
            .view
            .as_ref()
            .map_or((0, 4), |v| (v.seat, v.players.len()));
        let open = self.play.opts.show_bot_hands;
        ScrollArea::vertical()
            .id_salt("play_log")
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for l in self
                    .play
                    .log
                    .iter()
                    .filter(|l| open || !private_to_a_bot(l, seat, players))
                    .filter(|l| filter.is_empty() || l.to_lowercase().contains(&filter))
                {
                    let c = if l.contains("[manual edit]") {
                        WARN
                    } else if l.starts_with("P0") || l.contains(" P0 ") {
                        Color32::from_rgb(200, 220, 255)
                    } else {
                        Color32::GRAY
                    };
                    ui.label(RichText::new(l).small().color(c));
                }
            });
    }

    fn question(&mut self, ui: &mut Ui) {
        let Some(ask) = self.play.ask.clone() else {
            if self.play.running() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("The bots are playing...");
                });
            }
            return;
        };
        let phase = self
            .play
            .view
            .as_ref()
            .map(|v| format!("Round {} - P{}'s turn - {}", v.round, v.active, v.phase));
        if let Some(p) = phase {
            ui.label(RichText::new(p).small().weak());
        }
        match ask {
            Ask::Mulligan {
                hand,
                mulligans,
                suggest_keep,
            } => {
                ui.label(
                    RichText::new(format!("Opening hand (mulligans so far: {})", mulligans))
                        .strong(),
                );
                for c in &hand {
                    ui.label(format!("  {}", c));
                }
                ui.label(
                    RichText::new(format!(
                        "Bot would {}",
                        if suggest_keep { "keep" } else { "mulligan" }
                    ))
                    .small()
                    .weak(),
                );
                ui.horizontal(|ui| {
                    if ui.button("Keep").clicked() {
                        self.play_send(Answer::Yes(true));
                    }
                    if ui.button("Mulligan").clicked() {
                        self.play_send(Answer::Yes(false));
                    }
                });
            }
            Ask::Act {
                phase,
                actions,
                window,
                ..
            } => {
                ui.label(RichText::new(&phase).strong());
                if actions.is_empty() {
                    ui.label(RichText::new("Nothing you can do right now.").weak());
                }
                let mut chosen = None;
                ScrollArea::vertical()
                    .id_salt("play_actions")
                    .max_height(300.0)
                    .show(ui, |ui| {
                        for (k, a) in actions.iter().enumerate() {
                            ui.horizontal(|ui| {
                                let label = if a.cost.is_empty() {
                                    a.label.clone()
                                } else {
                                    format!("{}  {}", a.label, a.cost)
                                };
                                let b = ui.add_enabled(
                                    a.payable,
                                    egui::Button::new(RichText::new(label).color(if a.payable {
                                        GOOD
                                    } else {
                                        Color32::GRAY
                                    })),
                                );
                                let b = if a.payable {
                                    b
                                } else {
                                    b.on_disabled_hover_text(format!(
                                        "Tap {} more first (or use Auto-tap)",
                                        a.missing
                                    ))
                                };
                                if b.clicked() {
                                    chosen = Some(Answer::Pick(vec![k]));
                                }
                                if !a.payable {
                                    ui.label(
                                        RichText::new(format!("needs {}", a.missing))
                                            .small()
                                            .color(WARN),
                                    );
                                }
                                if !a.payable && a.auto_ok && ui.small_button("Auto-tap").clicked()
                                {
                                    chosen = Some(Answer::AutoTap(k));
                                }
                            });
                        }
                    });
                ui.horizontal(|ui| {
                    let pass = if window { "Pass" } else { "End phase" };
                    if ui.button(RichText::new(pass).strong()).clicked() {
                        chosen = Some(Answer::Pass);
                    }
                    let bot = if window {
                        "Let the bot decide"
                    } else {
                        "Let the bot play this phase"
                    };
                    if ui.button(bot).clicked() {
                        chosen = Some(Answer::Auto);
                    }
                    if ui.small_button("Concede").clicked() {
                        chosen = Some(Answer::Concede);
                    }
                });
                if let Some(a) = chosen {
                    self.play_send(a);
                }
            }
            Ask::Pick {
                prompt,
                options,
                min,
                max,
                suggested,
            } => {
                ui.label(RichText::new(&prompt).strong());
                ui.label(
                    RichText::new(if min == max {
                        format!("Choose {}", min)
                    } else {
                        format!("Choose {} to {}", min, max)
                    })
                    .small(),
                );
                if self.play.picks.len() != options.len() {
                    self.play.picks = vec![false; options.len()];
                }
                ScrollArea::vertical()
                    .id_salt("play_pick")
                    .max_height(300.0)
                    .show(ui, |ui| {
                        for (k, o) in options.iter().enumerate() {
                            let mark = if suggested.contains(&k) {
                                "  (bot)"
                            } else {
                                ""
                            };
                            if max == 1 {
                                if ui
                                    .selectable_label(self.play.picks[k], format!("{}{}", o, mark))
                                    .clicked()
                                {
                                    let was = self.play.picks[k];
                                    self.play.picks.iter_mut().for_each(|p| *p = false);
                                    self.play.picks[k] = !was;
                                }
                            } else {
                                ui.checkbox(&mut self.play.picks[k], format!("{}{}", o, mark));
                            }
                        }
                    });
                let n = self.play.picks.iter().filter(|&&p| p).count();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(n >= min && n <= max, egui::Button::new("OK"))
                        .clicked()
                    {
                        let v = (0..options.len()).filter(|&k| self.play.picks[k]).collect();
                        self.play_send(Answer::Pick(v));
                    }
                    if ui.button("Bot's choice").clicked() {
                        self.play_send(Answer::Auto);
                    }
                });
            }
            Ask::YesNo { prompt, suggested } => {
                ui.label(RichText::new(&prompt).strong());
                ui.label(
                    RichText::new(format!(
                        "Bot would say {}",
                        if suggested { "yes" } else { "no" }
                    ))
                    .small()
                    .weak(),
                );
                ui.horizontal(|ui| {
                    if ui.button("Yes").clicked() {
                        self.play_send(Answer::Yes(true));
                    }
                    if ui.button("No").clicked() {
                        self.play_send(Answer::Yes(false));
                    }
                });
            }
            Ask::Number {
                prompt,
                min,
                max,
                suggested,
            } => {
                ui.label(RichText::new(&prompt).strong());
                ui.add(egui::Slider::new(&mut self.play.number, min..=max));
                ui.label(
                    RichText::new(format!("Bot would choose {}", suggested))
                        .small()
                        .weak(),
                );
                if ui.button("OK").clicked() {
                    let n = self.play.number;
                    self.play_send(Answer::Num(n));
                }
            }
            Ask::Attack {
                attackers,
                defenders,
                ..
            } => {
                ui.label(RichText::new("Declare attackers").strong());
                if self.play.attacks.len() != attackers.len() {
                    self.play.attacks = vec![None; attackers.len()];
                }
                for (k, (_, desc)) in attackers.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(desc);
                        let cur = match self.play.attacks[k] {
                            Some(d) => defenders.get(d).cloned().unwrap_or_default(),
                            None => "doesn't attack".into(),
                        };
                        egui::ComboBox::from_id_salt(("play_att", k))
                            .selected_text(cur)
                            .width(200.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.play.attacks[k],
                                    None,
                                    "doesn't attack",
                                );
                                for (d, name) in defenders.iter().enumerate() {
                                    ui.selectable_value(&mut self.play.attacks[k], Some(d), name);
                                }
                            });
                    });
                }
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Attack").strong()).clicked() {
                        let v = attackers
                            .iter()
                            .zip(&self.play.attacks)
                            .filter_map(|((id, _), d)| d.map(|d| (*id, d)))
                            .collect();
                        self.play_send(Answer::Attacks(v));
                    }
                    if ui.button("No attack").clicked() {
                        self.play_send(Answer::Attacks(Vec::new()));
                    }
                    if ui.button("Bot's attack").clicked() {
                        self.play_send(Answer::Auto);
                    }
                });
                ui.label(
                    RichText::new("Pre-set to what the bot would do.")
                        .small()
                        .weak(),
                );
            }
            Ask::Block {
                attackers,
                blockers,
                ..
            } => {
                ui.label(RichText::new("Declare blockers").strong());
                for (_, a) in &attackers {
                    ui.label(RichText::new(format!("Attacking: {}", a)).color(BAD));
                }
                if self.play.blocks.len() != blockers.len() {
                    self.play.blocks = vec![None; blockers.len()];
                }
                for (k, (_, desc, can)) in blockers.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(desc);
                        let name = |id: u32| {
                            attackers
                                .iter()
                                .find(|a| a.0 == id)
                                .map(|a| a.1.clone())
                                .unwrap_or_default()
                        };
                        let cur = match self.play.blocks[k] {
                            Some(id) => name(id),
                            None => "doesn't block".into(),
                        };
                        egui::ComboBox::from_id_salt(("play_blk", k))
                            .selected_text(cur)
                            .width(220.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.play.blocks[k],
                                    None,
                                    "doesn't block",
                                );
                                for &a in can {
                                    ui.selectable_value(&mut self.play.blocks[k], Some(a), name(a));
                                }
                            });
                    });
                }
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Block").strong()).clicked() {
                        let v = blockers
                            .iter()
                            .zip(&self.play.blocks)
                            .filter_map(|((id, _, _), a)| a.map(|a| (*id, a)))
                            .collect();
                        self.play_send(Answer::Blocks(v));
                    }
                    if ui.button("No blocks").clicked() {
                        self.play_send(Answer::Blocks(Vec::new()));
                    }
                    if ui.button("Bot's blocks").clicked() {
                        self.play_send(Answer::Auto);
                    }
                });
                ui.label(
                    RichText::new("Pre-set to what the bot would do.")
                        .small()
                        .weak(),
                );
            }
            Ask::Pay {
                prompt,
                cost,
                missing,
                auto_ok,
            } => {
                ui.label(RichText::new(&prompt).strong());
                ui.label(format!("Cost {} - still needed: {}", cost, missing));
                ui.label(
                    RichText::new("Tap mana below (or click your lands on the board).")
                        .small()
                        .weak(),
                );
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(auto_ok, egui::Button::new("Auto-tap the rest"))
                        .clicked()
                    {
                        self.play_send(Answer::Auto);
                    }
                    if ui.button("Cancel").clicked() {
                        self.play_send(Answer::Cancel);
                    }
                });
            }
            Ask::Continue { prompt } => {
                ui.label(RichText::new(&prompt).strong());
                if ui.button("Continue").clicked() {
                    self.play_send(Answer::Continue);
                }
            }
        }
    }

    fn mana_panel(&mut self, ui: &mut Ui) {
        let Some(v) = self.play.view.clone() else {
            return;
        };
        pool_label(ui, &v.pool);
        let can_tap = self.play.edits_ok && self.play.ask.is_some();
        if v.sources.is_empty() {
            ui.label(RichText::new("No untapped mana sources.").small().weak());
            return;
        }
        let mut send = None;
        egui::CollapsingHeader::new("Tap mana")
            .default_open(true)
            .show(ui, |ui| {
                for s in &v.sources {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(&s.name).small());
                        for c in 0..6u8 {
                            if s.colors & (1 << c) == 0 {
                                continue;
                            }
                            let txt = if s.amount > 1 {
                                format!("{}x{{{}}}", s.amount, POOL_NAMES[c as usize])
                            } else {
                                format!("{{{}}}", POOL_NAMES[c as usize])
                            };
                            if ui
                                .add_enabled(
                                    can_tap,
                                    egui::Button::new(RichText::new(txt).color(MANA[c as usize]))
                                        .small(),
                                )
                                .clicked()
                            {
                                send = Some(Answer::Tap { id: s.id, color: c });
                            }
                        }
                        if let Some((fee, made)) = s.filter {
                            ui.label(
                                RichText::new(format!("filter: pay {{{}}}, make {} of", fee, made))
                                    .small(),
                            );
                            for c in 0..5u8 {
                                if ui
                                    .add_enabled(
                                        can_tap,
                                        egui::Button::new(
                                            RichText::new(format!(
                                                "{{{}}}",
                                                POOL_NAMES[c as usize]
                                            ))
                                            .color(MANA[c as usize]),
                                        )
                                        .small(),
                                    )
                                    .clicked()
                                {
                                    send = Some(Answer::Filter {
                                        id: s.id,
                                        colors: vec![c; made as usize],
                                    });
                                }
                            }
                        }
                    });
                }
            });
        if let Some(a) = send {
            self.play_send(a);
        }
    }

    // ------------------------------------------------------------ the board

    fn play_board(&mut self, ui: &mut Ui) {
        let Some(v) = self.play.view.clone() else {
            ui.label("Starting...");
            return;
        };
        let n = v.players.len();
        // Opponents across the top, you at the bottom.
        ui.columns(n.saturating_sub(1).max(1), |cols| {
            for (k, p) in (0..n).filter(|&p| p != v.seat).enumerate() {
                if let Some(ui) = cols.get_mut(k) {
                    self.player_area(ui, &v, p);
                }
            }
        });
        ui.separator();
        self.player_area(ui, &v, v.seat);
    }

    fn player_area(&mut self, ui: &mut Ui, v: &GameView, p: usize) {
        let pl = &v.players[p];
        let me = p == v.seat;
        let head = format!(
            "P{} {}{} - life {}{}",
            p,
            pl.name,
            if v.active == p { " (active)" } else { "" },
            pl.life,
            if pl.poison > 0 {
                format!(", poison {}", pl.poison)
            } else {
                String::new()
            }
        );
        ui.label(RichText::new(head).strong().color(if !pl.alive {
            Color32::DARK_GRAY
        } else if me {
            Color32::from_rgb(200, 220, 255)
        } else {
            Color32::WHITE
        }));
        let mut life = pl.life;
        if me || self.play.edits_ok {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "hand {} · library {} · graveyard {} · exile {}",
                        pl.hand_count,
                        pl.library,
                        pl.graveyard.len(),
                        pl.exile.len()
                    ))
                    .small(),
                );
                if self.play.edits_ok {
                    ui.label(RichText::new("life:").small());
                    if ui.add(egui::DragValue::new(&mut life)).changed() {
                        self.play_send(Answer::Edit(Edit::SetLife { player: p, life }));
                    }
                }
            });
        } else {
            ui.label(
                RichText::new(format!(
                    "hand {} · library {} · graveyard {} · exile {}",
                    pl.hand_count,
                    pl.library,
                    pl.graveyard.len(),
                    pl.exile.len()
                ))
                .small(),
            );
        }
        if !pl.commander_damage.is_empty() {
            ui.label(
                RichText::new(format!(
                    "commander damage: {}",
                    pl.commander_damage.join(", ")
                ))
                .small()
                .color(WARN),
            );
        }
        if !pl.command.is_empty() {
            ui.label(
                RichText::new(format!(
                    "command zone: {}",
                    pl.command
                        .iter()
                        .map(|(n, tax)| if *tax > 0 {
                            format!("{} (tax {})", n, tax)
                        } else {
                            n.clone()
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
                .small(),
            );
        }
        let perms: Vec<PermView> = v.perms.iter().filter(|x| x.ctrl == p).cloned().collect();
        for (title, sel) in [("Creatures", 0), ("Other permanents", 1), ("Lands", 2)] {
            let row: Vec<&PermView> = perms
                .iter()
                .filter(|x| match sel {
                    0 => x.creature,
                    1 => !x.creature && !x.land,
                    _ => x.land && !x.creature,
                })
                .collect();
            if row.is_empty() {
                continue;
            }
            ui.label(RichText::new(title).small().weak());
            ui.horizontal_wrapped(|ui| {
                for x in row {
                    self.perm_chip(ui, v, x);
                }
            });
        }
        if !pl.hand.is_empty() {
            ui.label(
                RichText::new(if me { "Your hand" } else { "Hand" })
                    .small()
                    .weak(),
            );
            ui.horizontal_wrapped(|ui| {
                for (k, name) in pl.hand.iter().enumerate() {
                    self.hand_chip(ui, p, k, name, me);
                }
            });
        }
        ui.collapsing(
            format!(
                "Graveyard ({}) / exile ({})",
                pl.graveyard.len(),
                pl.exile.len()
            ),
            |ui| {
                for (k, name) in pl.graveyard.iter().enumerate() {
                    let r = ui.label(RichText::new(format!("gy: {}", name)).small());
                    let r = self.zoom_by_name(r, name);
                    self.zone_menu(r, Loc::Graveyard(p, k), name);
                }
                for (k, name) in pl.exile.iter().enumerate() {
                    let r = ui.label(RichText::new(format!("exile: {}", name)).small());
                    let r = self.zoom_by_name(r, name);
                    self.zone_menu(r, Loc::Exile(p, k), name);
                }
            },
        );
        ui.add_space(6.0);
    }

    fn zoom_by_name(&mut self, resp: egui::Response, name: &str) -> egui::Response {
        let base = name.split(" (").next().unwrap_or(name).trim();
        match self.db.get(base).cloned() {
            Some(card) => self.with_zoom(resp, &card, None),
            None => resp,
        }
    }

    fn perm_chip(&mut self, ui: &mut Ui, v: &GameView, x: &PermView) {
        let mut text = x.name.clone();
        if x.creature {
            text += &format!(" {}/{}", x.p, x.t);
        }
        if x.planeswalker {
            text += &format!(" [{}]", x.loyalty);
        }
        if x.counters != 0 {
            text += &format!(" ({:+} counters)", x.counters);
        }
        if x.other_counters != 0 && !x.planeswalker {
            text += &format!(" ({} other)", x.other_counters);
        }
        if x.damage > 0 {
            text += &format!(" dmg {}", x.damage);
        }
        if x.attacking {
            text += " ⚔";
        }
        if x.blocking {
            text += " 🛡";
        }
        let mut rt = RichText::new(text).small();
        if x.tapped {
            rt = rt.italics().color(Color32::GRAY);
        } else if x.sick {
            rt = rt.color(Color32::from_rgb(170, 170, 200));
        }
        if x.commander {
            rt = rt.strong();
        }
        let resp = ui.add(egui::Button::new(rt).selected(x.attacking || x.blocking));
        let mut hover = format!(
            "{}{}{}",
            if x.tapped { "tapped" } else { "untapped" },
            if x.sick { ", summoning sick" } else { "" },
            if x.keywords.is_empty() {
                String::new()
            } else {
                format!(", {}", x.keywords)
            }
        );
        if x.token {
            hover += ", token";
        }
        let resp = resp.on_hover_text(hover);
        let resp = self.zoom_by_name(resp, &x.name);
        // Clicking your own mana source taps it (one colour: right away).
        let me = x.ctrl == v.seat;
        let can = self.play.edits_ok && self.play.ask.is_some();
        if resp.clicked() && me && can && !x.tapped {
            if let Some(s) = v.sources.iter().find(|s| s.id == x.id) {
                let colors: Vec<u8> = (0..6).filter(|&c| s.colors & (1 << c) != 0).collect();
                if colors.len() == 1 {
                    self.play_send(Answer::Tap {
                        id: x.id,
                        color: colors[0],
                    });
                } else {
                    self.status =
                        "This source makes several colours: choose one under \"Tap mana\".".into();
                }
            }
        }
        if can {
            let id = x.id;
            let name = x.name.clone();
            let srcs: Vec<SourceView> = if me {
                v.sources.iter().filter(|s| s.id == id).cloned().collect()
            } else {
                Vec::new()
            };
            let mut send = None;
            resp.context_menu(|ui| {
                ui.label(RichText::new(&name).strong());
                for s in &srcs {
                    for c in 0..6u8 {
                        if s.colors & (1 << c) != 0
                            && ui
                                .button(format!("Tap for {{{}}}", POOL_NAMES[c as usize]))
                                .clicked()
                        {
                            send = Some(Answer::Tap { id, color: c });
                            ui.close();
                        }
                    }
                }
                ui.separator();
                ui.label(RichText::new("Fix the game state").small().weak());
                let mut edit = |e: Edit, label: &str, ui: &mut Ui| {
                    if ui.button(label).clicked() {
                        send = Some(Answer::Edit(e));
                        ui.close();
                    }
                };
                edit(Edit::Tap { id }, "Tap / untap", ui);
                edit(
                    Edit::Counters {
                        id,
                        plus: 1,
                        other: 0,
                    },
                    "+1/+1 counter",
                    ui,
                );
                edit(
                    Edit::Counters {
                        id,
                        plus: -1,
                        other: 0,
                    },
                    "Remove a +1/+1 counter",
                    ui,
                );
                edit(
                    Edit::Counters {
                        id,
                        plus: 0,
                        other: 1,
                    },
                    "Other counter (loyalty, lore...) +1",
                    ui,
                );
                edit(
                    Edit::Counters {
                        id,
                        plus: 0,
                        other: -1,
                    },
                    "Other counter -1",
                    ui,
                );
                for (to, label) in [
                    (Dest::Hand, "To owner's hand"),
                    (Dest::Graveyard, "To graveyard"),
                    (Dest::Exile, "To exile"),
                    (Dest::LibraryTop, "To top of library"),
                    (Dest::LibraryBottom, "To bottom of library"),
                ] {
                    edit(
                        Edit::Move {
                            from: Loc::Perm(id),
                            to,
                        },
                        label,
                        ui,
                    );
                }
            });
            if let Some(a) = send {
                self.play_send(a);
            }
        }
    }

    fn hand_chip(&mut self, ui: &mut Ui, p: usize, k: usize, name: &str, me: bool) {
        let resp = ui.add(egui::Button::new(RichText::new(name).small()));
        let resp = self.zoom_by_name(resp, name);
        if me && resp.clicked() {
            // Do this card's first action (the engine asks for mana if the pool is short).
            if let Some(Ask::Act { actions, .. }) = &self.play.ask {
                if let Some(k) = actions
                    .iter()
                    .position(|a| a.card.as_deref() == Some(name) && (a.payable || !a.auto_ok))
                    .or_else(|| actions.iter().position(|a| a.card.as_deref() == Some(name)))
                {
                    self.play_send(Answer::Pick(vec![k]));
                }
            }
        }
        self.zone_menu(resp, Loc::Hand(p, k), name);
    }

    /// Right-click menu to move a card between zones (sandbox).
    fn zone_menu(&mut self, resp: egui::Response, from: Loc, name: &str) {
        if !(self.play.edits_ok && self.play.ask.is_some()) {
            return;
        }
        let mut send = None;
        let auto = match (&self.play.ask, from) {
            (Some(Ask::Act { actions, .. }), Loc::Hand(..)) => actions
                .iter()
                .position(|a| a.card.as_deref() == Some(name) && a.auto_ok),
            _ => None,
        };
        resp.context_menu(|ui| {
            ui.label(RichText::new(name).strong());
            if let Some(k) = auto {
                if ui.button("Auto-tap and play it").clicked() {
                    send = Some(Answer::AutoTap(k));
                    ui.close();
                }
            }
            ui.separator();
            ui.label(RichText::new("Fix the game state").small().weak());
            for (to, label) in [
                (Dest::Battlefield, "Put onto the battlefield"),
                (Dest::Hand, "To hand"),
                (Dest::Graveyard, "To graveyard"),
                (Dest::Exile, "To exile"),
                (Dest::LibraryTop, "To top of library"),
                (Dest::LibraryBottom, "To bottom of library"),
            ] {
                if ui.button(label).clicked() {
                    send = Some(Answer::Edit(Edit::Move { from, to }));
                    ui.close();
                }
            }
        });
        if let Some(a) = send {
            self.play_send(a);
        }
    }
}
