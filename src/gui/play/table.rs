//! The Play tab's table view: the game drawn as a card table. Opponents across the top,
//! your battlefield, mat and hand along the bottom, a turn bar between them, and a card
//! preview and the log on the right. Every click maps onto an answer the engine already
//! takes (`playtest::human::Answer`), so the rules engine is the same one the text view
//! and the simulator use.
//!
//! - A card you can play now glows (green: the mana is in your pool; amber: it can be paid
//!   by tapping your lands for you). Click it to play it; several options open a menu.
//! - Click a land or mana rock to tap it for mana (a menu when it makes several colours).
//! - Attacking: click an opponent's mat to aim, then click your creatures.
//! - Blocking: click an attacker, then the creature that blocks it.
//! - Everything else (targets, yes/no, numbers, mulligans, paying) opens a panel over the
//!   table.

use super::*;
use crate::card::Card;
use crate::gui::cards::{CARD_RATIO, CardLook, card_widget, paint_card};
use egui::{Align2, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};

/// Something clicked on the table this frame (handled once drawing is done).
enum Click {
    /// A card in hand: (slot, name).
    Hand(usize, String),
    /// Your commander in the command zone.
    Command(String),
    Perm(u32),
    Mat(usize),
    Send(Answer),
}

/// A small menu opened by a click: labelled answers.
#[derive(Clone)]
pub(super) struct Menu {
    title: String,
    items: Vec<(String, Answer)>,
    pos: Pos2,
}

const GLOW_PAY: Color32 = Color32::from_rgb(90, 220, 120);
const GLOW_AUTO: Color32 = Color32::from_rgb(240, 190, 80);
const ATTACK: Color32 = Color32::from_rgb(240, 90, 80);
const BLOCK: Color32 = Color32::from_rgb(90, 160, 250);
const SELECT: Color32 = Color32::from_rgb(250, 230, 90);
const FELT: Color32 = Color32::from_rgb(22, 30, 34);
const MAT: Color32 = Color32::from_rgb(30, 38, 44);

/// The steps of a turn, as the turn bar shows them, and how to spot each in the engine's
/// phase text.
const STEPS: [(&str, &[&str]); 8] = [
    ("Upkeep", &["upkeep", "start of game"]),
    ("Draw", &["draw"]),
    ("Main 1", &["main phase 1"]),
    ("Combat", &["beginning of combat", "combat"]),
    ("Attackers", &["attack"]),
    ("Blockers", &["block"]),
    ("Main 2", &["main phase 2", "after combat"]),
    ("End", &["end"]),
];

fn step_index(phase: &str) -> Option<usize> {
    let p = phase.to_lowercase();
    // Most specific first: "after blockers" is the blockers step, not "combat".
    let order: [(&str, usize); 10] = [
        ("main phase 2", 6),
        ("main phase 1", 2),
        ("block", 5),
        ("attack", 4),
        ("damage", 5),
        ("upkeep", 0),
        ("start of game", 0),
        ("draw", 1),
        ("combat", 3),
        ("end", 7),
    ];
    order.iter().find(|(k, _)| p.contains(k)).map(|(_, i)| *i)
}

/// The card to draw for a permanent: the database card, or a plain stand-in for tokens.
fn perm_card(db: &crate::db::CardDb, x: &PermView) -> Card {
    if let Some(c) = db.get(&x.name) {
        return c.clone();
    }
    Card {
        name: x.name.clone(),
        type_line: if x.creature {
            "Token Creature".into()
        } else if x.land {
            "Land".into()
        } else {
            "Token".into()
        },
        power: x.creature.then(|| x.p.to_string()),
        toughness: x.creature.then(|| x.t.to_string()),
        oracle_text: x.keywords.clone(),
        ..Default::default()
    }
}

/// The simulator's own bookkeeping lines (board summaries, AI reasoning): shown in the text
/// view, not on the table.
fn is_debug(line: &str) -> bool {
    line.contains(" threat ")
        || line.contains("board P")
        || line.contains("| board:")
        || line.contains("========")
        || line.contains("hand before the draw")
        || line.contains("AI P")
}

/// The card name in a choice: "Gollum the Abandoned 2/2 (P3)" or "Chandra [4] (P2)" ->
/// "Gollum the Abandoned" / "Chandra".
fn option_card_name(o: &str) -> &str {
    let mut s = o.split(" (P").next().unwrap_or(o).trim();
    if let Some((head, last)) = s.rsplit_once(' ') {
        let stat = last.split('/').count() == 2
            && last.split('/').all(|x| {
                !x.is_empty()
                    && x.chars()
                        .all(|c| c.is_ascii_digit() || c == '-' || c == '*')
            });
        let loyalty = last.starts_with('[') && last.ends_with(']');
        if stat || loyalty {
            s = head.trim();
        }
    }
    s.split(" (").next().unwrap_or(s).trim()
}

fn hand_card(db: &crate::db::CardDb, name: &str) -> Card {
    db.get(name).cloned().unwrap_or_else(|| Card {
        name: name.to_string(),
        ..Default::default()
    })
}

/// The defender index of player `p` in an attack question ("P2 ..." labels).
fn defender_of(defenders: &[String], p: usize) -> Option<usize> {
    let tag = format!("P{}", p);
    defenders
        .iter()
        .position(|d| d.split_whitespace().next() == Some(tag.as_str()))
}

impl GuiApp {
    /// The table view of a running (or finished) game.
    pub(super) fn play_table(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let Some(v) = self.play.view.clone() else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Shuffling up...");
            });
            return;
        };
        let ask = self.play.ask.clone();
        let mut clicks: Vec<Click> = Vec::new();
        // The preview keeps the last card pointed at.
        if let Some(h) = self.play.hover.take() {
            self.play.hover_shown = Some(h);
        }

        Panel::right("table_side")
            .resizable(true)
            .default_size(330.0)
            .show(ui, |ui| self.table_side(ui, &v));
        Panel::bottom("table_hand")
            .resizable(false)
            .exact_size(225.0)
            .show(ui, |ui| {
                egui::Frame::new().fill(FELT).show(ui, |ui| {
                    self.prompt_bar(ui, &v, ask.as_ref(), &mut clicks);
                    ui.add_space(4.0);
                    self.hand_strip(ui, &v, ask.as_ref(), &mut clicks);
                });
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(FELT).inner_margin(6.0))
            .show(ui, |ui| {
                let full = ui.available_rect_before_wrap();
                self.play.perm_rects.clear();
                self.play.mat_rects = vec![None; v.players.len()];
                let opps: Vec<usize> = (0..v.players.len()).filter(|&p| p != v.seat).collect();
                if let Some(f) = self.play.focus_opp.filter(|f| opps.contains(f)) {
                    // One opponent's board fills the table.
                    ui.horizontal(|ui| {
                        if ui.button("< Back to the table (Esc)").clicked() {
                            self.play.focus_opp = None;
                        }
                        for &p in &opps {
                            if p != f
                                && ui.button(format!("P{} {}", p, v.players[p].name)).clicked()
                            {
                                self.play.focus_opp = Some(p);
                            }
                        }
                    });
                    ui.add_space(4.0);
                    let r = ui.available_rect_before_wrap();
                    self.player_mat(ui, &v, ask.as_ref(), f, r.size(), false, &mut clicks);
                    self.combat_arrows(ui, &v, ask.as_ref());
                    return;
                }
                // Top half: the opponents in equal boxes. Bottom half: the turn bar and you.
                let gap = 6.0;
                let half = (full.height() - gap) / 2.0;
                let n = opps.len().max(1) as f32;
                let bw = (full.width() - gap * (n - 1.0)) / n;
                for (k, &p) in opps.iter().enumerate() {
                    let rect = Rect::from_min_size(
                        full.min + vec2(k as f32 * (bw + gap), 0.0),
                        vec2(bw, half),
                    );
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(rect)
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                    );
                    child.set_clip_rect(rect.intersect(ui.clip_rect()));
                    self.player_mat(
                        &mut child,
                        &v,
                        ask.as_ref(),
                        p,
                        rect.size(),
                        true,
                        &mut clicks,
                    );
                }
                let bottom =
                    Rect::from_min_max(egui::pos2(full.min.x, full.min.y + half + gap), full.max);
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(bottom)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                self.turn_bar(&mut child, &v);
                child.add_space(4.0);
                let r = child.available_rect_before_wrap();
                self.player_mat(
                    &mut child,
                    &v,
                    ask.as_ref(),
                    v.seat,
                    r.size(),
                    false,
                    &mut clicks,
                );
                self.combat_arrows(ui, &v, ask.as_ref());
                ui.allocate_rect(full, Sense::hover());
            });
        // Keyboard: Space does the main thing, Esc closes a menu.
        let typing = ctx.memory(|m| m.focused().is_some());
        if !typing && self.play.menu.is_none() {
            if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
                let main = match ask.as_ref() {
                    Some(Ask::Act { .. }) => Some(Answer::Pass),
                    Some(Ask::Continue { .. }) => Some(Answer::Continue),
                    Some(Ask::Attack { attackers, .. }) => Some(Answer::Attacks(
                        attackers
                            .iter()
                            .zip(&self.play.attacks)
                            .filter_map(|((id, _), d)| d.map(|d| (*id, d)))
                            .collect(),
                    )),
                    Some(Ask::Block { blockers, .. }) => Some(Answer::Blocks(
                        blockers
                            .iter()
                            .zip(&self.play.blocks)
                            .filter_map(|((id, _, _), a)| a.map(|a| (*id, a)))
                            .collect(),
                    )),
                    // Choices: Space accepts the bot's suggestion; mulligans keep.
                    Some(Ask::Pick { .. }) | Some(Ask::YesNo { .. }) | Some(Ask::Number { .. }) => {
                        Some(Answer::Auto)
                    }
                    Some(Ask::Mulligan { .. }) => Some(Answer::Yes(true)),
                    _ => None,
                };
                if let Some(a) = main {
                    clicks.push(Click::Send(a));
                }
            }
            // B: let the bot decide this one (plays your phase, attacks, blocks, picks).
            if ctx.input(|i| i.key_pressed(egui::Key::B)) && ask.is_some() {
                clicks.push(Click::Send(Answer::Auto));
            }
            // 1-9: the nth card in your hand, as if clicked.
            const NUMS: [egui::Key; 9] = [
                egui::Key::Num1,
                egui::Key::Num2,
                egui::Key::Num3,
                egui::Key::Num4,
                egui::Key::Num5,
                egui::Key::Num6,
                egui::Key::Num7,
                egui::Key::Num8,
                egui::Key::Num9,
            ];
            if let Some(Ask::Act { .. }) = ask.as_ref() {
                let hand = v
                    .players
                    .get(v.seat)
                    .map(|p| p.hand.clone())
                    .unwrap_or_default();
                for (k, key) in NUMS.iter().enumerate() {
                    if ctx.input(|i| i.key_pressed(*key)) {
                        if let Some(name) = hand.get(k) {
                            clicks.push(Click::Hand(k, name.clone()));
                        }
                    }
                }
            }
            // S: pass everything until your turn.
            if ctx.input(|i| i.key_pressed(egui::Key::S))
                && v.active != v.seat
                && matches!(
                    ask.as_ref(),
                    Some(Ask::Act { window: true, .. }) | Some(Ask::Continue { .. })
                )
            {
                self.play.skip_to_mine = true;
                clicks.push(Click::Send(match ask.as_ref() {
                    Some(Ask::Continue { .. }) => Answer::Continue,
                    _ => Answer::Pass,
                }));
            }
            if ctx.input(|i| i.key_pressed(egui::Key::F1) || i.key_pressed(egui::Key::Questionmark))
            {
                self.play.show_keys = !self.play.show_keys;
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            // Esc closes a menu first, then leaves an expanded board.
            if self.play.menu.take().is_none() {
                self.play.focus_opp = None;
            }
        }
        self.table_banner(&ctx);
        self.zone_window(&ctx, &v);
        self.keys_window(&ctx);
        if self.play.over.is_some() && !self.play.over_closed {
            self.game_over_window(&ctx, &v);
        }
        self.table_overlay(&ctx, &v, ask.as_ref());
        self.table_menu(&ctx);
        for c in clicks {
            self.table_click(c, &v, ask.as_ref(), &ctx);
        }
    }

    /// Combat as arrows: each attacker to the player it attacks, each blocker to the attacker
    /// it blocks (including the attacks and blocks you are choosing now).
    fn combat_arrows(&self, ui: &Ui, v: &GameView, ask: Option<&Ask>) {
        let mut attacks: Vec<(u32, usize)> = v
            .perms
            .iter()
            .filter_map(|x| x.attack_target.map(|d| (x.id, d)))
            .collect();
        let mut blocks: Vec<(u32, u32)> = v
            .perms
            .iter()
            .filter_map(|x| x.blocks.map(|a| (x.id, a)))
            .collect();
        match ask {
            Some(Ask::Attack {
                attackers,
                defenders,
                ..
            }) => {
                for ((id, _), d) in attackers.iter().zip(&self.play.attacks) {
                    let player = d.and_then(|d| {
                        (0..v.players.len()).find(|&p| defender_of(defenders, p) == Some(d))
                    });
                    if let Some(p) = player {
                        attacks.push((*id, p));
                    }
                }
            }
            Some(Ask::Block { blockers, .. }) => {
                for ((id, _, _), a) in blockers.iter().zip(&self.play.blocks) {
                    if let Some(a) = a {
                        blocks.push((*id, *a));
                    }
                }
            }
            _ => {}
        }
        let painter = ui.painter();
        let arrow = |from: Pos2, to: Pos2, col: Color32| {
            let d = to - from;
            if d.length() < 12.0 {
                return;
            }
            let dir = d.normalized();
            let end = to - dir * 4.0;
            let shadow = Stroke::new(6.0, Color32::from_black_alpha(120));
            painter.line_segment([from, end], shadow);
            painter.line_segment([from, end], Stroke::new(3.0, col));
            let side = vec2(-dir.y, dir.x) * 7.0;
            let back = end - dir * 14.0;
            painter.add(egui::Shape::convex_polygon(
                vec![end, back + side, back - side],
                col,
                Stroke::NONE,
            ));
        };
        for (id, p) in attacks {
            let (Some(r), Some(Some(m))) =
                (self.play.perm_rects.get(&id), self.play.mat_rects.get(p))
            else {
                continue;
            };
            let from = r.center();
            let to = m.shrink(10.0).clamp(from);
            arrow(from, to, ATTACK.gamma_multiply(0.85));
        }
        for (b, a) in blocks {
            let (Some(rb), Some(ra)) = (self.play.perm_rects.get(&b), self.play.perm_rects.get(&a))
            else {
                continue;
            };
            arrow(
                rb.center(),
                ra.clamp(rb.center()),
                BLOCK.gamma_multiply(0.9),
            );
        }
    }

    /// "Your turn" across the table for a moment when your turn starts.
    fn table_banner(&mut self, ctx: &egui::Context) {
        let Some(text) = self.play.banner.clone() else {
            return;
        };
        let mine = text.starts_with("Your turn");
        if !mine || self.play.over.is_some() {
            self.play.banner = None;
            return;
        }
        let now = ctx.input(|i| i.time);
        let at = *self.play.banner_at.get_or_insert(now);
        let age = (now - at) as f32;
        const SHOW: f32 = 1.4;
        if age > SHOW {
            self.play.banner = None;
            return;
        }
        let alpha = if age < SHOW - 0.4 {
            1.0
        } else {
            (SHOW - age) / 0.4
        };
        let screen = ctx.content_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique("turn_banner"),
        ));
        let r = Rect::from_center_size(
            pos2(screen.center().x - 160.0, screen.center().y - 80.0),
            vec2(420.0, 70.0),
        );
        painter.rect_filled(
            r,
            CornerRadius::from(12.0),
            Color32::from_black_alpha((200.0 * alpha) as u8),
        );
        painter.rect_stroke(
            r,
            CornerRadius::from(12.0),
            Stroke::new(2.0, GOOD.gamma_multiply(alpha)),
            StrokeKind::Inside,
        );
        painter.text(
            r.center(),
            Align2::CENTER_CENTER,
            &text,
            FontId::proportional(30.0),
            Color32::WHITE.gamma_multiply(alpha),
        );
        ctx.request_repaint();
    }

    /// A player's graveyard and exile, as cards.
    fn zone_window(&mut self, ctx: &egui::Context, v: &GameView) {
        let Some(p) = self.play.zone_view else {
            return;
        };
        let Some(pl) = v.players.get(p).cloned() else {
            self.play.zone_view = None;
            return;
        };
        let mut open = true;
        let title = if p == v.seat {
            "Your graveyard and exile".to_string()
        } else {
            format!("P{} {} - graveyard and exile", p, pl.name)
        };
        egui::Window::new(title)
            .id(egui::Id::unique("zone_view"))
            .open(&mut open)
            .collapsible(false)
            .default_size(vec2(640.0, 420.0))
            .default_pos(ctx.content_rect().center() - vec2(420.0, 260.0))
            .show(ctx, |ui| {
                ScrollArea::vertical().show(ui, |ui| {
                    for (label, cards) in [("Graveyard", &pl.graveyard), ("Exile", &pl.exile)] {
                        ui.label(RichText::new(format!("{} ({})", label, cards.len())).strong());
                        if cards.is_empty() {
                            ui.label(RichText::new("empty").weak());
                        }
                        ui.horizontal_wrapped(|ui| {
                            // Newest first.
                            for name in cards.iter().rev() {
                                let card = hand_card(&self.db, name);
                                let r = self.table_card(ui, &card, 92.0, None, false);
                                self.note_hover(&r, name);
                            }
                        });
                        ui.add_space(6.0);
                    }
                    ui.label(
                        RichText::new(format!("Library {} · hand {}", pl.library, pl.hand_count))
                            .weak(),
                    );
                });
            });
        if !open {
            self.play.zone_view = None;
        }
    }

    /// The keyboard shortcuts (F1 or ?).
    fn keys_window(&mut self, ctx: &egui::Context) {
        if !self.play.show_keys {
            return;
        }
        let mut open = true;
        egui::Window::new("Keyboard shortcuts")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                egui::Grid::new("keys").striped(true).show(ui, |ui| {
                    for (k, what) in [
                        ("Space", "Pass / end the phase, confirm attacks and blocks, keep a hand, accept the suggested choice"),
                        ("B", "Let the bot decide this one"),
                        ("1 - 9", "Play the nth card in your hand"),
                        ("S", "Skip to your turn (pass every chance to respond)"),
                        ("Esc", "Close a menu, then leave an expanded board"),
                        ("F1 or ?", "Show or hide this list"),
                    ] {
                        ui.label(RichText::new(k).strong().monospace());
                        ui.label(what);
                        ui.end_row();
                    }
                });
            });
        if !open {
            self.play.show_keys = false;
        }
    }

    /// The result, how each player finished, and what next.
    fn game_over_window(&mut self, ctx: &egui::Context, v: &GameView) {
        let Some(w) = self.play.over else { return };
        let (text, col) = if w == v.seat {
            ("You win!".to_string(), GOOD)
        } else if w == crate::playtest::NO_WINNER {
            ("Draw".to_string(), WARN)
        } else {
            let who = v.players.get(w).map_or("", |p| p.name.as_str());
            (format!("P{} wins - {}", w, who), WARN)
        };
        let mut action = None;
        egui::Window::new("Game over")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(-160.0, -60.0))
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(text).size(30.0).strong().color(col));
                    ui.label(RichText::new(format!("after {} rounds", v.round)).weak());
                });
                ui.add_space(6.0);
                egui::Grid::new("standings").striped(true).show(ui, |ui| {
                    ui.label(RichText::new("Player").strong());
                    ui.label(RichText::new("Life").strong());
                    ui.label(RichText::new("Finish").strong());
                    ui.end_row();
                    // Winner first, then the latest to go out.
                    let mut order: Vec<usize> = (0..v.players.len()).collect();
                    order.sort_by_key(|&k| {
                        (
                            k != w,
                            std::cmp::Reverse(
                                self.play
                                    .out_round
                                    .get(k)
                                    .copied()
                                    .flatten()
                                    .unwrap_or(u32::MAX),
                            ),
                        )
                    });
                    for k in order {
                        let pl = &v.players[k];
                        let name = if k == v.seat {
                            format!("You - {}", pl.name)
                        } else {
                            format!("P{} {}", k, pl.name)
                        };
                        ui.label(RichText::new(name).color(if k == w {
                            col
                        } else {
                            Color32::LIGHT_GRAY
                        }));
                        ui.label(format!("{}", pl.life));
                        ui.label(if k == w {
                            "winner".to_string()
                        } else {
                            match self.play.out_round.get(k).copied().flatten() {
                                Some(r) => format!("out in round {}", r),
                                None => "still in".to_string(),
                            }
                        });
                        ui.end_row();
                    }
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(RichText::new("Rematch").strong())
                        .on_hover_text("Same decks, a new shuffle")
                        .clicked()
                    {
                        action = Some(0);
                    }
                    if ui
                        .button("Replay this game")
                        .on_hover_text("Same decks and the same shuffle (seed)")
                        .clicked()
                    {
                        action = Some(1);
                    }
                    if ui.button("Change decks").clicked() {
                        action = Some(2);
                    }
                    if ui.button("Look at the board").clicked() {
                        action = Some(3);
                    }
                });
            });
        match action {
            Some(0) => {
                self.play.seed = now_secs() % 1_000_000;
                self.start_game();
            }
            Some(1) => {
                if let Some(s) = &self.play.setup {
                    self.play.seed = s.seed;
                }
                self.start_game();
            }
            Some(2) => {
                self.play.handle = None;
                self.play.view = None;
                self.play.ask = None;
            }
            Some(3) => self.play.over_closed = true,
            _ => {}
        }
    }

    /// Right column: game buttons, the hovered card, mana pool and the log.
    fn table_side(&mut self, ui: &mut Ui, v: &GameView) {
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(RichText::new("Report bug").color(BAD))
                .on_hover_text("Save everything needed to replay this game to this moment")
                .clicked()
            {
                self.play.report_open = true;
            }
            if ui.button("New game").clicked() {
                self.play.handle = None;
                self.play.view = None;
                self.play.ask = None;
            }
            if ui
                .button("Text view")
                .on_hover_text("The plain list view of the same game")
                .clicked()
            {
                self.play.classic = true;
            }
        });
        if let Some(s) = &self.play.setup {
            ui.label(RichText::new(format!("seed {}", s.seed)).small().weak());
        }
        ui.separator();
        // The card under the pointer (from the previous frame), large.
        let name = self.play.hover_shown.clone();
        if let Some(card) = name.as_deref().and_then(|n| self.db.get(n)).cloned() {
            let w = (ui.available_width() - 8.0).min(300.0);
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
                ui.label(RichText::new(card.front_text()).small());
            }
            if let Some(back) = card.back_face() {
                ui.label(
                    RichText::new(format!("Other side: {} - {}", back.name, back.oracle_text))
                        .small()
                        .weak(),
                );
            }
        } else {
            ui.label(RichText::new("Point at a card to see it here.").weak());
        }
        ui.separator();
        pool_label(ui, &v.pool);
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Log");
            ui.add(
                egui::TextEdit::singleline(&mut self.play.log_filter)
                    .hint_text("filter")
                    .desired_width(140.0),
            );
        });
        let f = self.play.log_filter.to_lowercase();
        let players = v.players.len();
        ScrollArea::vertical()
            .id_salt("table_log")
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for line in self.play.log.iter().filter(|l| {
                    (f.is_empty() || l.to_lowercase().contains(&f))
                        && !private_to_a_bot(l, v.seat, players)
                        && !is_debug(l)
                }) {
                    ui.label(RichText::new(line).small());
                }
            });
    }

    /// The turn: round, whose turn, and the current step lit up.
    fn turn_bar(&mut self, ui: &mut Ui, v: &GameView) {
        egui::Frame::new()
            .fill(MAT)
            .corner_radius(6.0)
            .inner_margin(6.0)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let who = v
                        .players
                        .get(v.active)
                        .map(|p| p.name.clone())
                        .unwrap_or_default();
                    let mine = v.active == v.seat;
                    ui.label(
                        RichText::new(format!(
                            "Round {} · {}",
                            v.round,
                            if mine {
                                "your turn".to_string()
                            } else {
                                format!("P{}'s turn ({})", v.active, who)
                            }
                        ))
                        .strong()
                        .color(if mine {
                            GOOD
                        } else {
                            Color32::LIGHT_GRAY
                        }),
                    );
                    ui.separator();
                    let cur = step_index(&v.phase);
                    for (k, (name, _)) in STEPS.iter().enumerate() {
                        let on = cur == Some(k);
                        let txt = RichText::new(*name).small();
                        ui.label(if on {
                            txt.strong().color(Color32::BLACK).background_color(SELECT)
                        } else {
                            txt.weak()
                        });
                    }
                    ui.separator();
                    ui.label(RichText::new(&v.phase).small().italics());
                    // What the others cast since your last decision, as cards; otherwise
                    // the latest event (the log on the right has the rest).
                    let casts = self.play.fresh_casts.clone();
                    if !casts.is_empty() {
                        ui.separator();
                        ui.label(RichText::new("Cast since your last move:").small().weak());
                        let shown = casts.len().saturating_sub(8);
                        if shown > 0 {
                            ui.label(RichText::new(format!("+{} more", shown)).small().weak());
                        }
                        for (p, name) in casts.iter().skip(shown) {
                            ui.label(RichText::new(format!("P{}", p)).small().strong());
                            let card = hand_card(&self.db, name);
                            let r = self.table_card(ui, &card, 24.0, None, false);
                            self.note_hover(&r, name);
                            r.on_hover_text(format!("P{} cast {}", p, name));
                        }
                    } else {
                        let players = v.players.len();
                        if let Some(l) = self
                            .play
                            .log
                            .iter()
                            .rev()
                            .find(|l| !private_to_a_bot(l, v.seat, players) && !is_debug(l))
                        {
                            ui.separator();
                            ui.add(egui::Label::new(RichText::new(l).small().weak()).truncate());
                        }
                    }
                });
            });
    }

    /// One player's area: the mat (commander, life, zones) and their battlefield.
    fn player_mat(
        &mut self,
        ui: &mut Ui,
        v: &GameView,
        ask: Option<&Ask>,
        p: usize,
        size: Vec2,
        expandable: bool,
        clicks: &mut Vec<Click>,
    ) {
        let Some(pl) = v.players.get(p).cloned() else {
            return;
        };
        let me = p == v.seat;
        let aimed = match ask {
            Some(Ask::Attack { defenders, .. }) => {
                !me && defender_of(defenders, p) == Some(self.play.attack_target)
            }
            _ => false,
        };
        let stroke = if aimed {
            Stroke::new(2.5, ATTACK)
        } else if v.active == p {
            Stroke::new(1.5, Color32::from_rgb(120, 140, 150))
        } else {
            Stroke::new(1.0, Color32::from_rgb(50, 60, 66))
        };
        let frame = egui::Frame::new()
            .fill(MAT)
            .stroke(stroke)
            .corner_radius(8.0)
            .inner_margin(6.0);
        let inner = size - vec2(12.0, 12.0);
        let resp = frame
            .show(ui, |ui| {
                ui.set_min_size(inner);
                ui.set_max_size(inner);
                let top = ui.cursor().min.y;
                // Header: name, life, zones.
                ui.horizontal_wrapped(|ui| {
                    let life_col = if !pl.alive {
                        Color32::DARK_GRAY
                    } else if pl.life <= 10 {
                        BAD
                    } else {
                        Color32::WHITE
                    };
                    ui.label(
                        RichText::new(format!("{}", pl.life))
                            .size(if me { 26.0 } else { 22.0 })
                            .strong()
                            .color(life_col),
                    );
                    // How much it moved since your last decision.
                    let d = self.play.life_delta.get(p).copied().unwrap_or(0);
                    if d != 0 {
                        ui.label(RichText::new(format!("{:+}", d)).strong().color(if d < 0 {
                            BAD
                        } else {
                            GOOD
                        }));
                    }
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(if me {
                                format!("You - {}", pl.name)
                            } else {
                                format!("P{} {}", p, pl.name)
                            })
                            .strong(),
                        );
                        let mut bits = vec![
                            format!("hand {}", pl.hand_count),
                            format!("library {}", pl.library),
                            format!("graveyard {}", pl.graveyard.len()),
                        ];
                        if !pl.exile.is_empty() {
                            bits.push(format!("exile {}", pl.exile.len()));
                        }
                        if pl.poison > 0 {
                            bits.push(format!("poison {}", pl.poison));
                        }
                        if pl.floating > 0 {
                            bits.push(format!("{} floating", pl.floating));
                        }
                        ui.label(RichText::new(bits.join(" · ")).small().weak());
                        // Opponents' hands as card backs.
                        if !me && pl.hand_count > 0 {
                            let n = pl.hand_count.min(12);
                            let (r, _) = ui.allocate_exact_size(
                                vec2(8.0 * n as f32 + 10.0, 14.0),
                                Sense::hover(),
                            );
                            for k in 0..n {
                                let c = Rect::from_min_size(
                                    r.min + vec2(8.0 * k as f32, 0.0),
                                    vec2(10.0, 14.0),
                                );
                                ui.painter().rect_filled(
                                    c,
                                    CornerRadius::from(2.0),
                                    Color32::from_rgb(70, 50, 40),
                                );
                                ui.painter().rect_stroke(
                                    c,
                                    CornerRadius::from(2.0),
                                    Stroke::new(1.0, Color32::from_rgb(150, 120, 80)),
                                    StrokeKind::Inside,
                                );
                            }
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
                        if !pl.alive {
                            ui.label(RichText::new("out of the game").color(BAD));
                        }
                    });
                    // Command zone and graveyard as small cards.
                    for (name, tax) in &pl.command {
                        let card = hand_card(&self.db, name);
                        let r =
                            self.table_card(ui, &card, if me { 46.0 } else { 34.0 }, None, false);
                        self.note_hover(&r, name);
                        // Castable from here: glow like a playable hand card.
                        if me {
                            if let Some(Ask::Act { actions, .. }) = ask {
                                if let Some(a) = actions.iter().find(|a| {
                                    a.card.as_deref() == Some(name.as_str())
                                        && a.label.contains("command zone")
                                }) {
                                    let c = if a.payable {
                                        GLOW_PAY
                                    } else if a.auto_ok {
                                        GLOW_AUTO
                                    } else {
                                        Color32::GRAY
                                    };
                                    ui.painter().rect_stroke(
                                        r.rect,
                                        CornerRadius::from(3.0),
                                        Stroke::new(2.5, c),
                                        StrokeKind::Outside,
                                    );
                                }
                            }
                        }
                        let r = r.on_hover_text(format!("Command zone (commander tax {})", tax));
                        if me && r.clicked() {
                            clicks.push(Click::Command(name.clone()));
                        }
                    }
                    if !pl.graveyard.is_empty() || !pl.exile.is_empty() {
                        if ui
                            .small_button("zones")
                            .on_hover_text("Graveyard and exile, as cards")
                            .clicked()
                        {
                            self.play.zone_view = Some(p);
                        }
                    }
                    // Aiming attacks: a button on the mat (the whole frame being clickable
                    // took clicks meant for the cards on it).
                    if let Some(Ask::Attack { .. }) = ask {
                        if !me && pl.alive {
                            let b = egui::Button::new(
                                RichText::new(if aimed { "Target" } else { "Aim here" })
                                    .strong()
                                    .color(if aimed { Color32::WHITE } else { ATTACK }),
                            )
                            .fill(if aimed {
                                ATTACK
                            } else {
                                Color32::TRANSPARENT
                            });
                            if ui.add(b).clicked() {
                                clicks.push(Click::Mat(p));
                            }
                        }
                    }
                });
                // Battlefield: creatures and planeswalkers, other permanents, lands.
                let mine: Vec<PermView> = v.perms.iter().filter(|x| x.ctrl == p).cloned().collect();
                let front: Vec<&PermView> = mine
                    .iter()
                    .filter(|x| x.creature || x.planeswalker)
                    .collect();
                let back: Vec<&PermView> = mine
                    .iter()
                    .filter(|x| !x.creature && !x.planeswalker && !x.land)
                    .collect();
                let lands: Vec<&PermView> = mine.iter().filter(|x| x.land && !x.creature).collect();
                let mut land_groups: Vec<(&str, bool)> = Vec::new();
                for x in &lands {
                    if !land_groups.contains(&(x.name.as_str(), x.tapped)) {
                        land_groups.push((x.name.as_str(), x.tapped));
                    }
                }
                let room = vec2(inner.x, (inner.y - (ui.cursor().min.y - top)).max(40.0));
                let sp = ui.spacing().item_spacing;
                let (card_w, one_row) = fit_card_w(
                    room - vec2(14.0, 0.0),
                    sp,
                    front.len(),
                    back.len(),
                    land_groups.len(),
                );
                ScrollArea::vertical()
                    .id_salt(("mat", p))
                    .auto_shrink([false, false])
                    .max_height(room.y)
                    .show(ui, |ui| {
                        // Creatures and planeswalkers first, then other permanents and land
                        // stacks: on one shared row when that gives bigger cards.
                        let mut groups: Vec<(String, bool, Vec<&PermView>)> = Vec::new();
                        for x in lands {
                            match groups.iter_mut().find(|g| g.0 == x.name && g.1 == x.tapped) {
                                Some(g) => g.2.push(x),
                                None => groups.push((x.name.clone(), x.tapped, vec![x])),
                            }
                        }
                        groups.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
                        let mut rest: Vec<(&PermView, f32, usize)> =
                            back.iter().map(|x| (*x, 0.8, 1)).collect();
                        rest.extend(groups.iter().map(|g| (g.2[0], 0.7, g.2.len())));
                        let lead: Vec<(&PermView, f32, usize)> =
                            front.iter().map(|x| (*x, 1.0, 1)).collect();
                        let rows: Vec<Vec<(&PermView, f32, usize)>> = if one_row {
                            vec![lead.into_iter().chain(rest).collect()]
                        } else {
                            vec![lead, rest]
                        };
                        for row in rows.into_iter().filter(|r| !r.is_empty()) {
                            ui.horizontal_wrapped(|ui| {
                                for (x, k, n) in row {
                                    self.perm_card_ui(ui, v, ask, x, card_w * k, n, clicks);
                                }
                            });
                        }
                    });
            })
            .response;
        if let Some(m) = self.play.mat_rects.get_mut(p) {
            *m = Some(resp.rect);
        }
        // Opponents' boards open full size from a button in the corner.
        if expandable {
            let r = Rect::from_min_size(resp.rect.right_top() + vec2(-66.0, 6.0), vec2(60.0, 20.0));
            if ui
                .put(r, egui::Button::new(RichText::new("Expand").small()))
                .on_hover_text("Fill the table with this board")
                .clicked()
            {
                self.play.focus_opp = Some(p);
            }
        }
    }

    /// A card on the table (with the real image when downloaded), recording the hover.
    fn table_card(
        &mut self,
        ui: &mut Ui,
        card: &Card,
        w: f32,
        badge: Option<String>,
        dimmed: bool,
    ) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(vec2(w, w * CARD_RATIO), Sense::click());
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
                    card,
                    width: w,
                    image: tex.as_ref(),
                    badge,
                    dimmed,
                    selected: false,
                },
                resp.hovered(),
            );
        }
        resp
    }

    fn note_hover(&mut self, r: &egui::Response, name: &str) {
        if r.hovered() {
            self.play.hover = Some(name.split(" (").next().unwrap_or(name).to_string());
        }
    }

    /// A permanent: its card with what matters drawn over it (tapped, counters, damage,
    /// attacking or blocking, playable). `count` > 1: a stack of identical lands.
    #[allow(clippy::too_many_arguments)]
    fn perm_card_ui(
        &mut self,
        ui: &mut Ui,
        v: &GameView,
        ask: Option<&Ask>,
        x: &PermView,
        w: f32,
        count: usize,
        clicks: &mut Vec<Click>,
    ) {
        let card = perm_card(&self.db, x);
        let badge = (count > 1).then(|| format!("x{}", count));
        let resp = self.table_card(ui, &card, w, badge, false);
        let rect = resp.rect;
        self.play.perm_rects.insert(x.id, rect);
        self.note_hover(&resp, &x.name);
        let painter = ui.painter_at(rect.expand(3.0));
        let small = FontId::proportional((w * 0.17).clamp(9.0, 13.0));
        if x.tapped {
            painter.rect_filled(
                rect,
                CornerRadius::from(w * 0.05),
                Color32::from_black_alpha(130),
            );
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "TAPPED",
                small.clone(),
                Color32::from_gray(210),
            );
        }
        // Outline: what this card is doing, or can do, right now.
        let mut outline: Option<Color32> = None;
        match ask {
            Some(Ask::Act { actions, .. }) => {
                if let Some(a) = actions.iter().find(|a| a.card.as_deref() == Some(&x.name)) {
                    outline = Some(if a.payable {
                        GLOW_PAY
                    } else if a.auto_ok {
                        GLOW_AUTO
                    } else {
                        Color32::GRAY
                    });
                }
            }
            Some(Ask::Attack { attackers, .. }) => {
                if let Some(k) = attackers.iter().position(|a| a.0 == x.id) {
                    outline = Some(match self.play.attacks.get(k).copied().flatten() {
                        Some(_) => ATTACK,
                        None => Color32::from_gray(200),
                    });
                }
            }
            Some(Ask::Block {
                attackers,
                blockers,
                ..
            }) => {
                if attackers.iter().any(|a| a.0 == x.id) {
                    outline = Some(if self.play.block_sel == Some(x.id) {
                        SELECT
                    } else {
                        ATTACK
                    });
                } else if let Some(k) = blockers.iter().position(|b| b.0 == x.id) {
                    outline = Some(match self.play.blocks.get(k).copied().flatten() {
                        Some(_) => BLOCK,
                        None => Color32::from_gray(200),
                    });
                }
            }
            Some(Ask::Pick { options, .. }) => {
                let tag = format!("(P{})", x.ctrl);
                let k = options
                    .iter()
                    .position(|o| o.starts_with(&x.name) && o.contains(&tag))
                    .or_else(|| options.iter().position(|o| o.starts_with(&x.name)));
                if let Some(k) = k {
                    outline = Some(if self.play.picks.get(k).copied().unwrap_or(false) {
                        SELECT
                    } else {
                        Color32::from_rgb(235, 215, 130)
                    });
                }
            }
            _ => {}
        }
        if outline.is_none() {
            if x.attacking {
                outline = Some(ATTACK);
            } else if x.blocking {
                outline = Some(BLOCK);
            }
        }
        // A mana source you can tap now.
        let tappable =
            self.play.edits_ok && ask.is_some() && v.sources.iter().any(|s| s.id == x.id);
        if outline.is_none() && tappable && x.ctrl == v.seat {
            outline = Some(Color32::from_rgb(150, 170, 120));
        }
        // Which attacker each blocker is set to block.
        if let Some(Ask::Block {
            attackers,
            blockers,
            ..
        }) = ask
        {
            if let Some(k) = blockers.iter().position(|b| b.0 == x.id) {
                if let Some(a) = self.play.blocks.get(k).copied().flatten() {
                    let who = attackers
                        .iter()
                        .find(|t| t.0 == a)
                        .map(|t| {
                            option_card_name(&t.1)
                                .split(',')
                                .next()
                                .unwrap_or("")
                                .to_string()
                        })
                        .unwrap_or_default();
                    // As much of the name as fits the card.
                    let room = (((w * 0.96) / (small.size * 0.55)) as usize)
                        .saturating_sub(4)
                        .max(4);
                    let who = if who.chars().count() > room {
                        format!("{}.", who.chars().take(room - 1).collect::<String>())
                    } else {
                        who
                    };
                    let r = Rect::from_min_size(
                        rect.left_top() + vec2(2.0, 2.0),
                        vec2(w * 0.96, w * 0.22),
                    );
                    painter.rect_filled(r, CornerRadius::from(3.0), BLOCK);
                    painter.text(
                        r.center(),
                        Align2::CENTER_CENTER,
                        format!("x {}", who),
                        small.clone(),
                        Color32::WHITE,
                    );
                }
            }
        }
        // Whom each attacker is set to attack.
        if let Some(Ask::Attack {
            attackers,
            defenders,
            ..
        }) = ask
        {
            if let Some(k) = attackers.iter().position(|a| a.0 == x.id) {
                if let Some(d) = self.play.attacks.get(k).copied().flatten() {
                    let who = defenders
                        .get(d)
                        .and_then(|s| s.split_whitespace().next())
                        .unwrap_or("?");
                    let r = Rect::from_min_size(
                        rect.left_top() + vec2(2.0, 2.0),
                        vec2(w * 0.5, w * 0.22),
                    );
                    painter.rect_filled(r, CornerRadius::from(3.0), ATTACK);
                    painter.text(
                        r.center(),
                        Align2::CENTER_CENTER,
                        format!("> {}", who),
                        small.clone(),
                        Color32::WHITE,
                    );
                }
            }
        }
        // Arrived since your last decision (what the bots just played).
        if outline.is_none() && self.play.new_ids.contains(&x.id) {
            outline = Some(Color32::from_rgb(80, 210, 230));
        }
        if let Some(c) = outline {
            painter.rect_stroke(
                rect,
                CornerRadius::from(w * 0.05),
                Stroke::new(2.5, c),
                StrokeKind::Outside,
            );
        }
        // Counters, damage, loyalty, power/toughness.
        let mut top = Vec::new();
        if x.counters != 0 {
            top.push(format!("{:+}/{:+}", x.counters, x.counters));
        }
        if x.other_counters > 0 {
            top.push(format!("{} ctr", x.other_counters));
        }
        if x.commander {
            top.push("CMDR".into());
        }
        if !top.is_empty() {
            let t = top.join(" ");
            let r = Rect::from_min_size(
                rect.right_top() + vec2(-w * 0.62, 2.0),
                vec2(w * 0.6, w * 0.2),
            );
            painter.rect_filled(r, CornerRadius::from(3.0), Color32::from_rgb(30, 90, 50));
            painter.text(
                r.center(),
                Align2::CENTER_CENTER,
                t,
                small.clone(),
                Color32::WHITE,
            );
        }
        if x.creature {
            let dmg = x.damage > 0;
            let t = format!("{}/{}", x.p, x.t - x.damage.max(0));
            let r = Rect::from_min_size(
                rect.right_bottom() - vec2(w * 0.5, w * 0.24),
                vec2(w * 0.48, w * 0.22),
            );
            painter.rect_filled(
                r,
                CornerRadius::from(3.0),
                if dmg {
                    Color32::from_rgb(130, 30, 30)
                } else {
                    Color32::from_rgb(20, 20, 20)
                },
            );
            painter.text(
                r.center(),
                Align2::CENTER_CENTER,
                t,
                small.clone(),
                Color32::WHITE,
            );
            if x.sick && x.ctrl == v.seat && !x.tapped {
                painter.text(
                    rect.left_bottom() + vec2(3.0, -3.0),
                    Align2::LEFT_BOTTOM,
                    "sick",
                    small.clone(),
                    Color32::from_gray(180),
                );
            }
        }
        if x.planeswalker {
            let r = Rect::from_min_size(
                rect.left_bottom() - vec2(-2.0, w * 0.26),
                vec2(w * 0.32, w * 0.24),
            );
            painter.rect_filled(r, CornerRadius::from(4.0), Color32::from_rgb(40, 40, 60));
            painter.text(
                r.center(),
                Align2::CENTER_CENTER,
                format!("{}", x.loyalty),
                small.clone(),
                Color32::WHITE,
            );
        }
        if let Some(host) = x.attached {
            if let Some(h) = v.perms.iter().find(|y| y.id == host) {
                resp.clone()
                    .on_hover_text(format!("attached to {}", h.name));
            }
        }
        if resp.clicked() {
            // A stack of lands: the first untapped one.
            clicks.push(Click::Perm(x.id));
        }
    }

    /// The prompt bar above your hand: what the game is waiting for, and the main buttons.
    fn prompt_bar(
        &mut self,
        ui: &mut Ui,
        v: &GameView,
        ask: Option<&Ask>,
        clicks: &mut Vec<Click>,
    ) {
        ui.horizontal_wrapped(|ui| {
            if let Some(w) = self.play.over {
                let text = if w == v.seat {
                    "You win!".to_string()
                } else if w == crate::playtest::NO_WINNER {
                    "Draw".to_string()
                } else {
                    format!("P{} wins", w)
                };
                ui.label(
                    RichText::new(text)
                        .size(20.0)
                        .strong()
                        .color(if w == v.seat { GOOD } else { WARN }),
                );
                return;
            }
            if let Some(f) = &self.play.failed {
                ui.label(RichText::new(format!("The engine stopped: {}", f)).color(BAD));
                return;
            }
            let Some(ask) = ask else {
                ui.spinner();
                ui.label("The opponents are playing...");
                return;
            };
            match ask {
                Ask::Act {
                    phase,
                    actions,
                    window,
                    ..
                } => {
                    ui.label(RichText::new(phase).strong());
                    let playable = actions.iter().filter(|a| a.payable || a.auto_ok).count();
                    ui.label(
                        RichText::new(if playable == 0 {
                            "Nothing to play right now.".to_string()
                        } else {
                            format!(
                                "{} play{} available - glowing cards",
                                playable,
                                if playable == 1 { "" } else { "s" }
                            )
                        })
                        .small()
                        .weak(),
                    );
                    let pass = if *window {
                        "Pass (Space)"
                    } else {
                        "End phase (Space)"
                    };
                    if ui.button(RichText::new(pass).size(16.0).strong()).clicked() {
                        clicks.push(Click::Send(Answer::Pass));
                    }
                    if ui
                        .button(if *window {
                            "Bot decides"
                        } else {
                            "Bot plays this phase"
                        })
                        .clicked()
                    {
                        clicks.push(Click::Send(Answer::Auto));
                    }
                    if v.active != v.seat {
                        if ui
                            .button("Skip to my turn")
                            .on_hover_text("Pass every chance to respond until your next turn")
                            .clicked()
                        {
                            self.play.skip_to_mine = true;
                            clicks.push(Click::Send(Answer::Pass));
                        }
                    }
                    // Everything, as a list (for actions that aren't on a card).
                    ui.menu_button("All actions", |ui| {
                        for (k, a) in actions.iter().enumerate() {
                            let label = if a.cost.is_empty() {
                                a.label.clone()
                            } else {
                                format!("{}  {}", a.label, a.cost)
                            };
                            if ui
                                .add_enabled(a.payable || a.auto_ok, egui::Button::new(label))
                                .clicked()
                            {
                                clicks.push(Click::Send(if a.payable {
                                    Answer::Pick(vec![k])
                                } else {
                                    Answer::AutoTap(k)
                                }));
                                ui.close();
                            }
                        }
                        ui.separator();
                        if ui.button(RichText::new("Concede").color(BAD)).clicked() {
                            clicks.push(Click::Send(Answer::Concede));
                            ui.close();
                        }
                    });
                }
                Ask::Attack {
                    attackers,
                    defenders,
                    ..
                } => {
                    if self.play.attacks.len() != attackers.len() {
                        self.play.attacks = vec![None; attackers.len()];
                    }
                    let n = self.play.attacks.iter().filter(|a| a.is_some()).count();
                    let target = defenders
                        .get(self.play.attack_target)
                        .cloned()
                        .unwrap_or_default();
                    ui.label(RichText::new("Declare attackers").strong());
                    ui.label(
                        RichText::new(format!(
                            "Aiming at {} - click an opponent to change, then click your creatures",
                            target
                        ))
                        .small()
                        .weak(),
                    );
                    if ui
                        .button(
                            RichText::new(format!("Attack with {}", n))
                                .size(16.0)
                                .strong()
                                .color(ATTACK),
                        )
                        .clicked()
                    {
                        let v: Vec<(u32, usize)> = attackers
                            .iter()
                            .zip(&self.play.attacks)
                            .filter_map(|((id, _), d)| d.map(|d| (*id, d)))
                            .collect();
                        clicks.push(Click::Send(Answer::Attacks(v)));
                    }
                    if ui.button("All at target").clicked() {
                        let t = self.play.attack_target;
                        self.play.attacks.iter_mut().for_each(|a| *a = Some(t));
                    }
                    if ui.button("No attack").clicked() {
                        clicks.push(Click::Send(Answer::Attacks(Vec::new())));
                    }
                    if ui.button("Bot's attack").clicked() {
                        clicks.push(Click::Send(Answer::Auto));
                    }
                }
                Ask::Block { blockers, .. } => {
                    if self.play.blocks.len() != blockers.len() {
                        self.play.blocks = vec![None; blockers.len()];
                    }
                    let n = self.play.blocks.iter().filter(|b| b.is_some()).count();
                    ui.label(RichText::new("Declare blockers").strong());
                    ui.label(
                        RichText::new("Click an attacker (red), then the creature that blocks it")
                            .small()
                            .weak(),
                    );
                    if ui
                        .button(
                            RichText::new(format!("Block with {}", n))
                                .size(16.0)
                                .strong()
                                .color(BLOCK),
                        )
                        .clicked()
                    {
                        let v: Vec<(u32, u32)> = blockers
                            .iter()
                            .zip(&self.play.blocks)
                            .filter_map(|((id, _, _), a)| a.map(|a| (*id, a)))
                            .collect();
                        clicks.push(Click::Send(Answer::Blocks(v)));
                    }
                    if ui.button("No blocks").clicked() {
                        clicks.push(Click::Send(Answer::Blocks(Vec::new())));
                    }
                    if ui.button("Bot's blocks").clicked() {
                        clicks.push(Click::Send(Answer::Auto));
                    }
                }
                Ask::Pay { cost, missing, .. } => {
                    ui.label(
                        RichText::new(format!("Pay {} - still needed {}", cost, missing)).strong(),
                    );
                    ui.label(
                        RichText::new("Click your lands and rocks to tap them")
                            .small()
                            .weak(),
                    );
                }
                Ask::Continue { prompt } => {
                    ui.label(RichText::new(prompt).strong());
                    if ui
                        .button(RichText::new("Continue").size(16.0).strong())
                        .clicked()
                    {
                        clicks.push(Click::Send(Answer::Continue));
                    }
                }
                _ => {
                    ui.label(RichText::new("Make your choice in the panel").strong());
                }
            }
        });
    }

    /// Your hand, fanned along the bottom: the card under the pointer lifts and comes to
    /// the front.
    fn hand_strip(
        &mut self,
        ui: &mut Ui,
        v: &GameView,
        ask: Option<&Ask>,
        clicks: &mut Vec<Click>,
    ) {
        let hand: Vec<String> = v
            .players
            .get(v.seat)
            .map(|p| p.hand.clone())
            .unwrap_or_default();
        let avail = ui.available_size();
        let card_w = ((avail.y - 22.0) / CARD_RATIO).clamp(60.0, 140.0);
        let card_h = card_w * CARD_RATIO;
        let (strip, _) = ui.allocate_exact_size(vec2(avail.x, card_h + 18.0), Sense::hover());
        if hand.is_empty() {
            ui.painter().text(
                strip.center(),
                Align2::CENTER_CENTER,
                "No cards in hand",
                FontId::proportional(14.0),
                Color32::GRAY,
            );
            return;
        }
        let n = hand.len() as f32;
        let step = ((strip.width() - card_w) / (n - 1.0).max(1.0)).min(card_w + 8.0);
        let total = step * (n - 1.0) + card_w;
        let x0 = strip.center().x - total / 2.0;
        let rects: Vec<Rect> = (0..hand.len())
            .map(|k| {
                Rect::from_min_size(
                    pos2(x0 + step * k as f32, strip.top() + 16.0),
                    vec2(card_w, card_h),
                )
            })
            .collect();
        let pointer = ui.ctx().pointer_hover_pos();
        let hovered = pointer.and_then(|p| (0..rects.len()).rev().find(|&k| rects[k].contains(p)));
        let glow = |name: &str| -> Option<Color32> {
            match ask {
                Some(Ask::Act { actions, .. }) => actions
                    .iter()
                    .filter(|a| a.card.as_deref() == Some(name))
                    .map(|a| {
                        if a.payable {
                            GLOW_PAY
                        } else if a.auto_ok {
                            GLOW_AUTO
                        } else {
                            Color32::TRANSPARENT
                        }
                    })
                    .find(|c| *c != Color32::TRANSPARENT),
                Some(Ask::Pick { options, .. }) => options
                    .iter()
                    .any(|o| o.starts_with(name))
                    .then_some(SELECT),
                _ => None,
            }
        };
        let order: Vec<usize> = (0..hand.len())
            .filter(|&k| Some(k) != hovered)
            .chain(hovered)
            .collect();
        for k in order {
            let name = &hand[k];
            let card = hand_card(&self.db, name);
            let mut r = rects[k];
            if Some(k) == hovered {
                r = r.translate(vec2(0.0, -14.0));
            }
            let tex = if self.settings.real_images {
                self.images.thumb(&card)
            } else {
                None
            };
            paint_card(
                ui,
                r,
                &CardLook {
                    card: &card,
                    width: card_w,
                    image: tex.as_ref(),
                    badge: None,
                    dimmed: false,
                    selected: false,
                },
                Some(k) == hovered,
            );
            if let Some(c) = glow(name) {
                ui.painter().rect_stroke(
                    r,
                    CornerRadius::from(card_w * 0.05),
                    Stroke::new(3.0, c),
                    StrokeKind::Outside,
                );
            }
        }
        if let Some(k) = hovered {
            self.play.hover = Some(hand[k].clone());
        }
        // A click area for every card, every frame (the slice of it that shows, the whole of
        // the last one), so a click lands even when the pointer only just arrived.
        for k in 0..hand.len() {
            let r = rects[k];
            // The lifted card under the pointer: all of it.
            let hit = if Some(k) == hovered {
                Rect::from_min_max(r.min - vec2(0.0, 14.0), r.max)
            } else if k + 1 < hand.len() {
                Rect::from_min_max(r.min - vec2(0.0, 14.0), pos2(r.min.x + step, r.max.y))
            } else {
                Rect::from_min_max(r.min - vec2(0.0, 14.0), r.max)
            };
            if Some(k) == hovered {
                continue;
            }
            let resp = ui.interact(hit, ui.scope_id().with(("hand", k)), Sense::click());
            if resp.clicked() {
                clicks.push(Click::Hand(k, hand[k].clone()));
            }
        }
        if let Some(k) = hovered {
            let r = rects[k];
            let hit = Rect::from_min_max(r.min - vec2(0.0, 14.0), r.max);
            let resp = ui.interact(hit, ui.scope_id().with(("hand", k)), Sense::click());
            if resp.clicked() {
                clicks.push(Click::Hand(k, hand[k].clone()));
            }
        }
    }

    /// Questions that don't fit the table (choices, yes/no, numbers, mulligans, paying):
    /// a panel over the middle of the table.
    fn table_overlay(&mut self, ctx: &egui::Context, v: &GameView, ask: Option<&Ask>) {
        let Some(ask) = ask.cloned() else { return };
        let title = match &ask {
            Ask::Mulligan { .. } => "Opening hand",
            Ask::Pick { .. } => "Choose",
            Ask::YesNo { .. } => "Decide",
            Ask::Number { .. } => "Choose a number",
            Ask::Pay { .. } => "Pay the cost",
            _ => return,
        };
        egui::Window::new(title)
            // Its own id for the opening hand: a window keeps where it was last shown.
            .id(egui::Id::unique(if matches!(ask, Ask::Mulligan { .. }) {
                "table_overlay_hand"
            } else {
                "table_overlay"
            }))
            .default_pos({
                // Over your hand (not needed while choosing), so the boards with the
                // targets stay in view; the opening hand sits in the middle.
                let r = ctx.content_rect();
                match &ask {
                    Ask::Mulligan { .. } => pos2(r.center().x - 330.0, 150.0),
                    _ => pos2(r.center().x - 360.0, r.bottom() - 330.0),
                }
            })
            .movable(true)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                if let Ask::Mulligan { hand, mulligans, suggest_keep } = &ask {
                    ui.label(format!("Mulligans so far: {}", mulligans));
                    ui.horizontal(|ui| {
                        for name in hand {
                            let card = hand_card(&self.db, name);
                            let r = self.table_card(ui, &card, 92.0, None, false);
                            self.note_hover(&r, name);
                        }
                    });
                    ui.label(RichText::new(format!("The bot would {}", if *suggest_keep { "keep" } else { "mulligan" })).small().weak());
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("Keep").size(16.0).strong()).clicked() {
                            self.play_send(Answer::Yes(true));
                        }
                        if ui.button(RichText::new("Mulligan").size(16.0)).clicked() {
                            self.play_send(Answer::Yes(false));
                        }
                    });
                    return;
                }
                // A choice among cards (surveil, discard, tutor, a graveyard target): the
                // cards themselves, click to choose.
                if let Ask::Pick { prompt, options, min, max, suggested } = &ask {
                    let names: Vec<Option<String>> = options
                        .iter()
                        .map(|o| {
                            let base = option_card_name(o);
                            self.db.get(base).map(|c| c.name.clone())
                        })
                        .collect();
                    if !options.is_empty() && names.iter().all(|n| n.is_some()) && options.len() <= 24 {
                        if self.play.picks.len() != options.len() {
                            self.play.picks = (0..options.len()).map(|k| suggested.contains(&k)).collect();
                        }
                        ui.label(RichText::new(prompt).strong());
                        ui.label(
                            RichText::new(if min == max {
                                format!("Choose {} - click cards; yellow = chosen, \"bot\" = what the bot would pick", min)
                            } else {
                                format!("Choose {} to {} - click cards", min, max)
                            })
                            .small()
                            .weak(),
                        );
                        let w = if options.len() > 8 { 70.0 } else { 92.0 };
                        let mut toggled = None;
                        ui.horizontal_wrapped(|ui| {
                            ui.set_max_width(9.0 * (w + 8.0));
                            for (k, name) in names.iter().enumerate() {
                                let card = hand_card(&self.db, name.as_deref().unwrap_or(""));
                                // Whose it is (a permanent on the table) and the bot's pick.
                                let owner = options[k]
                                    .rsplit_once("(P")
                                    .and_then(|(_, r)| r.split(')').next())
                                    .map(|p| format!("P{}", p));
                                let badge = match (owner, suggested.contains(&k)) {
                                    (Some(o), true) => Some(format!("{} bot", o)),
                                    (Some(o), false) => Some(o),
                                    (None, true) => Some("bot".to_string()),
                                    (None, false) => None,
                                };
                                let r = self.table_card(ui, &card, w, badge, false);
                                self.note_hover(&r, &card.name);
                                if self.play.picks.get(k).copied().unwrap_or(false) {
                                    ui.painter().rect_stroke(
                                        r.rect,
                                        CornerRadius::from(w * 0.05),
                                        Stroke::new(3.0, SELECT),
                                        StrokeKind::Outside,
                                    );
                                }
                                if r.clicked() {
                                    toggled = Some(k);
                                }
                            }
                        });
                        if let Some(k) = toggled {
                            self.toggle_pick(k, *max);
                        }
                        let n = self.play.picks.iter().filter(|&&p| p).count();
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(n >= *min && n <= *max, egui::Button::new(RichText::new("OK").size(16.0).strong()))
                                .clicked()
                            {
                                let v = (0..options.len()).filter(|&k| self.play.picks[k]).collect();
                                self.play_send(Answer::Pick(v));
                            }
                            if ui.button("Bot's choice").clicked() {
                                self.play_send(Answer::Auto);
                            }
                        });
                        return;
                    }
                }
                // The rest: the same controls as the text view.
                if let Ask::Pick { options, .. } = &ask {
                    if options.iter().any(|o| o.contains("(P")) {
                        ui.label(
                            RichText::new("Or click the glowing cards on the table.")
                                .small()
                                .color(Color32::from_rgb(235, 215, 130)),
                        );
                    }
                }
                self.question(ui);
                if matches!(ask, Ask::Pay { .. }) {
                    ui.separator();
                    self.mana_panel(ui);
                }
                let _ = v;
            });
    }

    /// The click menu, at the pointer.
    fn table_menu(&mut self, ctx: &egui::Context) {
        let Some(menu) = self.play.menu.clone() else {
            return;
        };
        let mut close = false;
        let mut send = None;
        let area = egui::Area::new(egui::Id::unique("table_click_menu"))
            .fixed_pos(menu.pos)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(RichText::new(&menu.title).strong());
                    for (label, a) in &menu.items {
                        if ui.button(label).clicked() {
                            send = Some(a.clone());
                        }
                    }
                    if ui.small_button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
        if ctx.input(|i| i.pointer.any_click())
            && !area.response.hovered()
            && send.is_none()
            && !close
        {
            // A click elsewhere closes it (not on the frame it opened).
            if self.play.menu_age > 0 {
                close = true;
            }
        }
        self.play.menu_age += 1;
        if let Some(a) = send {
            self.play.menu = None;
            self.play_send(a);
        } else if close {
            self.play.menu = None;
        }
    }

    fn open_menu(&mut self, ctx: &egui::Context, title: String, items: Vec<(String, Answer)>) {
        let pos = ctx.pointer_interact_pos().unwrap_or(pos2(400.0, 300.0)) + vec2(8.0, 8.0);
        self.play.menu = Some(Menu { title, items, pos });
        self.play.menu_age = 0;
    }

    /// What a click on the table does, for the question being asked.
    fn table_click(&mut self, c: Click, v: &GameView, ask: Option<&Ask>, ctx: &egui::Context) {
        if let Click::Send(a) = c {
            self.play_send(a);
            return;
        }
        let Some(ask) = ask else { return };
        // Actions on a card, by name.
        // Actions on a card, by name and by where it is: a hand card's are casts and land
        // plays, the command zone's the commander cast, a permanent's its abilities.
        #[derive(PartialEq)]
        enum From {
            Hand,
            Command,
            Field,
        }
        let card_actions = |name: &str, from: From| -> Vec<(String, Answer)> {
            let cast = |l: &str| l.starts_with("Cast ") || l.starts_with("Play land");
            match ask {
                Ask::Act { actions, .. } => actions
                    .iter()
                    .enumerate()
                    .filter(|(_, a)| a.card.as_deref() == Some(name))
                    .filter(|(_, a)| match from {
                        From::Hand => cast(&a.label) && !a.label.contains("command zone"),
                        From::Command => a.label.contains("command zone"),
                        From::Field => !cast(&a.label),
                    })
                    .filter_map(|(k, a)| {
                        let label = if a.cost.is_empty() {
                            a.label.clone()
                        } else {
                            format!("{}  {}", a.label, a.cost)
                        };
                        if a.payable {
                            Some((label, Answer::Pick(vec![k])))
                        } else if a.auto_ok {
                            Some((format!("{}  (tap for me)", label), Answer::AutoTap(k)))
                        } else {
                            None
                        }
                    })
                    // Copies of a card offer the same action once each: one is enough.
                    .fold(Vec::new(), |mut acc: Vec<(String, Answer)>, (l, a)| {
                        if !acc.iter().any(|(m, _)| *m == l) {
                            acc.push((l, a));
                        }
                        acc
                    }),
                _ => Vec::new(),
            }
        };
        match c {
            Click::Hand(slot, name) => {
                if let Ask::Pick { options, max, .. } = ask {
                    // The k-th copy in hand is the k-th matching option.
                    let hand: Vec<String> = v
                        .players
                        .get(v.seat)
                        .map(|p| p.hand.clone())
                        .unwrap_or_default();
                    let nth = hand[..slot.min(hand.len())]
                        .iter()
                        .filter(|h| **h == name)
                        .count();
                    let matching: Vec<usize> = options
                        .iter()
                        .enumerate()
                        .filter(|(_, o)| o.split(" (").next().unwrap_or(o).trim() == name)
                        .map(|(k, _)| k)
                        .collect();
                    if let Some(&k) = matching.get(nth).or(matching.first()) {
                        self.toggle_pick(k, *max);
                    }
                    return;
                }
                if !matches!(ask, Ask::Act { .. }) {
                    return;
                }
                let items = card_actions(&name, From::Hand);
                match items.len() {
                    0 => self.status = format!("{} can't be played right now.", name),
                    1 => self.play_send(items[0].1.clone()),
                    _ => self.open_menu(ctx, name, items),
                }
            }
            Click::Command(name) => {
                let items = card_actions(&name, From::Command);
                match items.len() {
                    0 => self.status = format!("{} can't be cast right now.", name),
                    1 => self.play_send(items[0].1.clone()),
                    _ => self.open_menu(ctx, name, items),
                }
            }
            Click::Perm(id) => {
                let Some(x) = v.perms.iter().find(|x| x.id == id).cloned() else {
                    return;
                };
                match ask {
                    Ask::Attack { attackers, .. } => {
                        if let Some(k) = attackers.iter().position(|a| a.0 == id) {
                            if self.play.attacks.len() != attackers.len() {
                                self.play.attacks = vec![None; attackers.len()];
                            }
                            self.play.attacks[k] = match self.play.attacks[k] {
                                Some(_) => None,
                                None => Some(self.play.attack_target),
                            };
                        }
                    }
                    Ask::Block {
                        attackers,
                        blockers,
                        ..
                    } => {
                        if attackers.iter().any(|a| a.0 == id) {
                            self.play.block_sel = Some(id);
                        } else if let Some(k) = blockers.iter().position(|b| b.0 == id) {
                            if self.play.blocks.len() != blockers.len() {
                                self.play.blocks = vec![None; blockers.len()];
                            }
                            self.play.blocks[k] = match (self.play.blocks[k], self.play.block_sel) {
                                (Some(_), _) => None,
                                (None, Some(a)) if blockers[k].2.contains(&a) => Some(a),
                                (None, Some(_)) => {
                                    self.status = format!("{} can't block that attacker.", x.name);
                                    None
                                }
                                (None, None) => {
                                    self.status = "Click an attacker first.".into();
                                    None
                                }
                            };
                        }
                    }
                    Ask::Pick { options, max, .. } => {
                        let tag = format!("(P{})", x.ctrl);
                        let k = options
                            .iter()
                            .position(|o| o.starts_with(&x.name) && o.contains(&tag))
                            .or_else(|| options.iter().position(|o| o.starts_with(&x.name)));
                        if let Some(k) = k {
                            self.toggle_pick(k, *max);
                        }
                    }
                    _ => {
                        let mut items = card_actions(&x.name, From::Field);
                        // Tapping for mana.
                        if self.play.edits_ok {
                            if let Some(s) = v.sources.iter().find(|s| s.id == id) {
                                for c in 0..6u8 {
                                    if s.colors & (1 << c) != 0 {
                                        items.push((
                                            format!("Tap for {{{}}}", POOL_NAMES[c as usize]),
                                            Answer::Tap { id, color: c },
                                        ));
                                    }
                                }
                            } else if x.land && x.ctrl == v.seat && x.tapped {
                                self.status = format!("{} is already tapped.", x.name);
                            }
                        }
                        match items.len() {
                            0 => {}
                            1 => self.play_send(items[0].1.clone()),
                            _ => self.open_menu(ctx, x.name.clone(), items),
                        }
                    }
                }
            }
            Click::Mat(p) => {
                if let Ask::Attack { defenders, .. } = ask {
                    if let Some(d) = defender_of(defenders, p) {
                        self.play.attack_target = d;
                    }
                }
            }
            Click::Send(_) => {}
        }
    }

    fn toggle_pick(&mut self, k: usize, max: usize) {
        if k >= self.play.picks.len() {
            return;
        }
        let was = self.play.picks[k];
        if max == 1 {
            self.play.picks.iter_mut().for_each(|p| *p = false);
        }
        self.play.picks[k] = !was;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choice_options_name_their_cards() {
        assert_eq!(
            option_card_name("Gollum the Abandoned 2/2 (P3)"),
            "Gollum the Abandoned"
        );
        assert_eq!(
            option_card_name("Chandra, Torch of Defiance [4] (P2)"),
            "Chandra, Torch of Defiance"
        );
        assert_eq!(option_card_name("Stasis Snare"), "Stasis Snare");
        assert_eq!(option_card_name("Stasis Snare (bot)"), "Stasis Snare");
        assert_eq!(step_index("After blockers (combat tricks)"), Some(5));
        assert_eq!(step_index("Main phase 2"), Some(6));
        assert_eq!(step_index("Declare attackers"), Some(4));
    }
}

/// The biggest card width at which a board fits its box, and whether to lay it out as one
/// flow (small boards) rather than creatures above the rest. Creatures and planeswalkers
/// are full size, other permanents 0.8 and land stacks 0.7.
fn fit_card_w(room: Vec2, sp: Vec2, front: usize, back: usize, lands: usize) -> (f32, bool) {
    // Rows needed for `units` cards of width `w` (a mixed row measured in land widths).
    let rows = |units: f32, w: f32| -> f32 {
        if units <= 0.0 {
            0.0
        } else {
            let per = ((room.x + sp.x) / (w + sp.x)).max(1.0);
            (units / per).ceil()
        }
    };
    let rest = back as f32 * 0.8 / 0.7 + lands as f32;
    let fit = |one: bool| -> f32 {
        let mut w: f32 = 130.0;
        while w > 30.0 {
            let h = if one {
                let r = rows(front as f32 / 0.7 + rest, w * 0.7);
                r * (w * CARD_RATIO + sp.y)
            } else {
                rows(front as f32, w) * (w * CARD_RATIO + sp.y)
                    + rows(rest, w * 0.7) * (w * 0.8 * CARD_RATIO + sp.y)
            };
            if h + 4.0 <= room.y {
                break;
            }
            w -= 2.0;
        }
        w
    };
    let (two, one) = (fit(false), fit(true));
    if one > two { (one, true) } else { (two, false) }
}
