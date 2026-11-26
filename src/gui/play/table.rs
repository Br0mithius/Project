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
use egui::{Align2, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

/// Something clicked on the table this frame (handled once drawing is done).
enum Click {
    Hand(String),
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
            .exact_size(250.0)
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
                ScrollArea::vertical()
                    .id_salt("table_board")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // Cards shrink as boards fill up, so both fit above your hand.
                        let busiest = (0..v.players.len())
                            .filter(|&p| p != v.seat)
                            .map(|p| v.perms.iter().filter(|x| x.ctrl == p).count())
                            .max()
                            .unwrap_or(0);
                        let mine = v.perms.iter().filter(|x| x.ctrl == v.seat).count();
                        let opp_w = match busiest {
                            0..=6 => 58.0,
                            7..=11 => 50.0,
                            _ => 42.0,
                        };
                        let my_w = match mine {
                            0..=8 => 80.0,
                            9..=14 => 68.0,
                            _ => 58.0,
                        };
                        let opps: Vec<usize> =
                            (0..v.players.len()).filter(|&p| p != v.seat).collect();
                        let n = opps.len().max(1) as f32;
                        let w = (ui.available_width() - 8.0 * (n - 1.0)) / n;
                        ui.horizontal_top(|ui| {
                            for &p in &opps {
                                ui.allocate_ui_with_layout(
                                    vec2(w, 10.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        ui.set_width(w);
                                        self.player_mat(ui, &v, ask.as_ref(), p, opp_w, &mut clicks);
                                    },
                                );
                            }
                        });
                        ui.add_space(6.0);
                        self.turn_bar(ui, &v);
                        ui.add_space(6.0);
                        self.player_mat(ui, &v, ask.as_ref(), v.seat, my_w, &mut clicks);
                    });
            });
        self.table_overlay(&ctx, &v, ask.as_ref());
        self.table_menu(&ctx);
        for c in clicks {
            self.table_click(c, &v, ask.as_ref(), &ctx);
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
                });
                // The latest events, so the bots' plays are easy to follow.
                let players = v.players.len();
                let recent: Vec<&String> = self
                    .play
                    .log
                    .iter()
                    .rev()
                    .filter(|l| !private_to_a_bot(l, v.seat, players) && !is_debug(l))
                    .take(2)
                    .collect();
                for l in recent.into_iter().rev() {
                    ui.label(RichText::new(l).small().weak());
                }
            });
    }

    /// One player's area: the mat (commander, life, zones) and their battlefield.
    fn player_mat(
        &mut self,
        ui: &mut Ui,
        v: &GameView,
        ask: Option<&Ask>,
        p: usize,
        card_w: f32,
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
        let resp = frame
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
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
                        let r = self.table_card(ui, &card, 34.0, None, false);
                        self.note_hover(&r, name);
                        r.on_hover_text(format!("Command zone (tax {})", tax));
                    }
                    if !pl.graveyard.is_empty() || !pl.exile.is_empty() {
                        ui.menu_button(RichText::new("zones").small(), |ui| {
                            ui.label(RichText::new("Graveyard").strong());
                            for c in pl.graveyard.iter().rev() {
                                let r = ui.label(RichText::new(c).small());
                                self.note_hover(&r, c);
                            }
                            if !pl.exile.is_empty() {
                                ui.label(RichText::new("Exile").strong());
                                for c in &pl.exile {
                                    let r = ui.label(RichText::new(c).small());
                                    self.note_hover(&r, c);
                                }
                            }
                        });
                    }
                    if aimed {
                        ui.label(RichText::new("target").color(ATTACK).strong());
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
                if !front.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        for x in front {
                            self.perm_card_ui(ui, v, ask, x, card_w, 1, clicks);
                        }
                    });
                }
                ui.horizontal_wrapped(|ui| {
                    for x in back {
                        self.perm_card_ui(ui, v, ask, x, card_w * 0.8, 1, clicks);
                    }
                    // Lands grouped by name and tapped state.
                    let mut groups: Vec<(String, bool, Vec<&PermView>)> = Vec::new();
                    for x in lands {
                        match groups.iter_mut().find(|g| g.0 == x.name && g.1 == x.tapped) {
                            Some(g) => g.2.push(x),
                            None => groups.push((x.name.clone(), x.tapped, vec![x])),
                        }
                    }
                    groups.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
                    for (_, _, g) in groups {
                        self.perm_card_ui(ui, v, ask, g[0], card_w * 0.7, g.len(), clicks);
                    }
                });
            })
            .response;
        if !me && resp.interact(Sense::click()).clicked() {
            clicks.push(Click::Mat(p));
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
                    "new",
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
                    let pass = if *window { "Pass" } else { "End phase" };
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
                clicks.push(Click::Hand(hand[k].clone()));
            }
        }
        if let Some(k) = hovered {
            let r = rects[k];
            let hit = Rect::from_min_max(r.min - vec2(0.0, 14.0), r.max);
            let resp = ui.interact(hit, ui.scope_id().with(("hand", k)), Sense::click());
            if resp.clicked() {
                clicks.push(Click::Hand(hand[k].clone()));
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
            .id(egui::Id::unique("table_overlay"))
            .anchor(Align2::CENTER_CENTER, [0.0, -60.0])
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
                            let base = o.split(" (").next().unwrap_or(o).trim();
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
                                let badge = suggested.contains(&k).then(|| "bot".to_string());
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
        let card_actions = |name: &str| -> Vec<(String, Answer)> {
            match ask {
                Ask::Act { actions, .. } => actions
                    .iter()
                    .enumerate()
                    .filter(|(_, a)| a.card.as_deref() == Some(name))
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
                    .collect(),
                _ => Vec::new(),
            }
        };
        match c {
            Click::Hand(name) => {
                if let Ask::Pick { options, max, .. } = ask {
                    if let Some(k) = options.iter().position(|o| o.starts_with(&name)) {
                        self.toggle_pick(k, *max);
                    }
                    return;
                }
                let items = card_actions(&name);
                match items.len() {
                    0 => self.status = format!("{} can't be played right now.", name),
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
                        if let Some(k) = options.iter().position(|o| o.starts_with(&x.name)) {
                            self.toggle_pick(k, *max);
                        }
                    }
                    _ => {
                        let mut items = card_actions(&x.name);
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
