//! Archetypes tab: every Commander archetype and set trait with how close your pool and
//! your deck are to it; the selected one's themes, commanders, key cards, build targets
//! and an "Auto-build this archetype" button.

use super::{Action, GuiApp, Tab};
use crate::archetypes::{self as ar, ARCHETYPES, Kind, SetArchetype};
use eframe::egui::{self, Color32, RichText, ScrollArea, Ui, vec2};

const GOOD: Color32 = Color32::from_rgb(110, 220, 120);
const GOLD: Color32 = Color32::from_rgb(255, 205, 80);
const NOTE: Color32 = Color32::from_rgb(255, 190, 110);

/// One archetype's fit for the pool and the deck.
pub struct ArchRow {
    pub idx: usize,
    pub pool: f32,
    pub pool_cards: usize,
    pub owned_cmds: Vec<&'static str>,
    /// None when the deck lacks the archetype's colours (or has no commander).
    pub deck: Option<f32>,
    pub deck_cards: usize,
}

#[derive(Clone, Copy, PartialEq, Default)]
pub enum KindFilter {
    #[default]
    All,
    Commander,
    Traits,
}

#[derive(Default)]
pub struct ArchState {
    pub kind: KindFilter,
    pub selected: Option<usize>,
    /// Sort the list by the deck's fit instead of the pool's.
    pub by_deck: bool,
}

fn index_of(a: &SetArchetype) -> usize {
    ARCHETYPES
        .iter()
        .position(|x| std::ptr::eq(x, a))
        .unwrap_or(0)
}

fn pct_color(f: f32) -> Color32 {
    if f >= 0.75 {
        GOOD
    } else if f >= 0.5 {
        Color32::from_rgb(220, 220, 140)
    } else {
        Color32::from_gray(160)
    }
}

impl GuiApp {
    fn arch_rows(&mut self) -> &Vec<ArchRow> {
        if self.memo.arch_rows.is_none() {
            let rows = ARCHETYPES
                .iter()
                .enumerate()
                .map(|(idx, a)| {
                    let p = ar::pool_report(a, &self.db, &self.coll);
                    let d = ar::deck_report(a, &self.deck, &self.db);
                    ArchRow {
                        idx,
                        pool: p.closeness,
                        pool_cards: p.cards.len(),
                        owned_cmds: p.commanders.clone(),
                        deck: d.as_ref().map(|d| d.closeness),
                        deck_cards: d.map_or(0, |d| d.cards.len()),
                    }
                })
                .collect();
            self.memo.arch_rows = Some(rows);
        }
        self.memo.arch_rows.as_ref().unwrap()
    }

    /// The deck's best-fitting archetype, for the Deck Builder header.
    pub(super) fn best_archetype_label(&mut self, ui: &mut Ui) {
        if self.memo.best_arch.is_none() {
            let best = ar::best_for_deck(&self.deck, &self.db)
                .map(|r| (index_of(r.arch), r.closeness, r.cards.len()));
            self.memo.best_arch = Some(best);
        }
        let Some(Some((i, close, n))) = self.memo.best_arch else {
            return;
        };
        if self.deck.main_count() == 0 {
            return;
        }
        let a = &ARCHETYPES[i];
        ui.separator();
        // Moves to the next line as a whole rather than breaking the name.
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        if ui
            .link(
                RichText::new(format!("Plays like {}", a.name))
                    .strong()
                    .color(GOLD)
                    .size(15.0),
            )
            .on_hover_text(format!(
                "The archetype this deck fits best: {:.0}% of a working {} deck ({} fitting cards). Click to see it on the Archetypes tab.",
                close * 100.0,
                a.name,
                n
            ))
            .clicked()
        {
            self.arch.selected = Some(i);
            self.tab = Tab::Archetypes;
        }
        ui.label(RichText::new(format!("{:.0}%", close * 100.0)).color(pct_color(close)));
        ui.style_mut().wrap_mode = None;
    }

    pub(super) fn archetypes_tab(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        ui.heading("Archetypes");
        ui.label(
            "Deck plans the builder recognises: broad Commander archetypes and the league sets' key traits. \
             Pool fit: how close the cards and commanders you own are to a working deck of that kind (half enablers, \
             30% payoffs, 20% an owned commander). Deck fit: the same for your current deck.",
        );
        ui.horizontal_wrapped(|ui| {
            ui.label("Show:");
            let st = &mut self.arch;
            ui.selectable_value(&mut st.kind, KindFilter::All, "all");
            ui.selectable_value(&mut st.kind, KindFilter::Commander, "Commander archetypes");
            ui.selectable_value(&mut st.kind, KindFilter::Traits, "set traits");
            ui.separator();
            ui.label("Sort by:");
            ui.selectable_value(&mut st.by_deck, false, "pool fit");
            ui.selectable_value(&mut st.by_deck, true, "deck fit");
        });
        ui.separator();
        let (kind, by_deck) = (self.arch.kind, self.arch.by_deck);
        let mut rows: Vec<(usize, f32, usize, Option<f32>, usize, String, bool)> = self
            .arch_rows()
            .iter()
            .filter(|r| match kind {
                KindFilter::All => true,
                KindFilter::Commander => ARCHETYPES[r.idx].kind == Kind::Commander,
                KindFilter::Traits => ARCHETYPES[r.idx].kind == Kind::SetTrait,
            })
            .map(|r| {
                (
                    r.idx,
                    r.pool,
                    r.pool_cards,
                    r.deck,
                    r.deck_cards,
                    r.owned_cmds.join(", "),
                    ARCHETYPES[r.idx].sparse,
                )
            })
            .collect();
        rows.sort_by(|a, b| {
            let (x, y) = if by_deck {
                (a.3.unwrap_or(-1.0), b.3.unwrap_or(-1.0))
            } else {
                (a.1, b.1)
            };
            y.partial_cmp(&x).unwrap()
        });
        if self.arch.selected.is_none() {
            self.arch.selected = rows.first().map(|r| r.0);
        }
        let h = ui.available_height();
        let list_w = (ui.available_width() * 0.46).clamp(320.0, 520.0);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(vec2(list_w, h), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(list_w);
                ScrollArea::vertical()
                    .id_salt("arch_list")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::Grid::new("arch_grid")
                            .striped(true)
                            .num_columns(4)
                            .show(ui, |ui| {
                                ui.label(RichText::new("Archetype").strong());
                                ui.label(RichText::new("Pool").strong());
                                ui.label(RichText::new("Deck").strong());
                                ui.label(RichText::new("Commanders you own").strong());
                                ui.end_row();
                                for (idx, pool, pool_cards, deck, deck_cards, cmds, sparse) in &rows {
                                    let a = &ARCHETYPES[*idx];
                                    let sel = self.arch.selected == Some(*idx);
                                    let name = if a.set.is_empty() {
                                        a.name.to_string()
                                    } else {
                                        format!("{} ({})", a.name, a.set)
                                    };
                                    let text = if *sparse {
                                        RichText::new(name).weak()
                                    } else if sel {
                                        RichText::new(name).strong().color(GOLD)
                                    } else {
                                        RichText::new(name)
                                    };
                                    if ui
                                        .selectable_label(sel, text)
                                        .on_hover_text(a.plan)
                                        .clicked()
                                    {
                                        self.arch.selected = Some(*idx);
                                    }
                                    ui.label(
                                        RichText::new(format!("{:.0}%", pool * 100.0))
                                            .monospace()
                                            .color(pct_color(*pool)),
                                    )
                                    .on_hover_text(format!("{} fitting cards in your pool", pool_cards));
                                    match deck {
                                        Some(d) => {
                                            ui.label(
                                                RichText::new(format!("{:.0}%", d * 100.0))
                                                    .monospace()
                                                    .color(pct_color(*d)),
                                            )
                                            .on_hover_text(format!("{} fitting cards in your deck", deck_cards));
                                        }
                                        None => {
                                            ui.label(RichText::new("-").weak()).on_hover_text(
                                                "Your deck doesn't have this archetype's colours (or no commander yet).",
                                            );
                                        }
                                    }
                                    let c: String = if cmds.is_empty() {
                                        "-".into()
                                    } else if cmds.chars().count() > 34 {
                                        format!("{}...", cmds.chars().take(32).collect::<String>())
                                    } else {
                                        cmds.clone()
                                    };
                                    ui.label(RichText::new(c).small()).on_hover_text(cmds.as_str());
                                    ui.end_row();
                                }
                            });
                    });
            });
            ui.separator();
            let w = ui.available_width();
            ui.allocate_ui_with_layout(vec2(w, h), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(w);
                ScrollArea::vertical()
                    .id_salt("arch_detail")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if let Some(i) = self.arch.selected {
                            self.archetype_detail(ui, i, acts);
                        }
                    });
            });
        });
    }

    fn archetype_detail(&mut self, ui: &mut Ui, i: usize, acts: &mut Vec<Action>) {
        let a: &'static SetArchetype = &ARCHETYPES[i];
        ui.label(
            RichText::new(if a.set.is_empty() {
                a.name.to_string()
            } else {
                format!("{} ({} set trait)", a.name, a.set)
            })
            .heading()
            .color(GOLD),
        );
        ui.label(a.plan);
        if !a.colors.is_empty() {
            ui.label(format!("Colours: {}", a.colors));
        }
        if a.sparse {
            ui.label(
                RichText::new(
                    "Too few cards in the league sets to build a deck on: detection only.",
                )
                .color(NOTE),
            );
        }
        let themes: Vec<String> = a.themes.iter().map(|t| t.label()).collect();
        ui.label(format!("Themes: {}", themes.join(" · ")));
        // Fit.
        let p = ar::pool_report(a, &self.db, &self.coll);
        let d = ar::deck_report(a, &self.deck, &self.db);
        ui.add_space(4.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            // Bar on one line, the counts underneath: side by side, the counts wrapped mid-phrase.
            ui.horizontal(|ui| {
                ui.label(RichText::new("Your pool").strong());
                ui.add(
                    egui::ProgressBar::new(p.closeness)
                        .desired_width(180.0)
                        .text(format!("{:.0}% fit", p.closeness * 100.0)),
                );
            });
            ui.label(
                RichText::new(format!(
                    "{} fitting cards · {:.0} enablers (wants {:.0}) · {:.0} payoffs (wants {:.0})",
                    p.cards.len(),
                    p.enablers,
                    ar::ENABLERS_WANTED,
                    p.payoffs,
                    ar::PAYOFFS_WANTED
                ))
                .small(),
            );
            ui.add_space(4.0);
            ui.vertical(|ui| match &d {
                Some(d) => {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Your deck").strong());
                        ui.add(
                            egui::ProgressBar::new(d.closeness)
                                .desired_width(180.0)
                                .text(format!("{:.0}% fit", d.closeness * 100.0)),
                        );
                    });
                    ui.label(
                        RichText::new(format!(
                            "{} fitting cards · {:.0} enablers · {:.0} payoffs{}",
                            d.cards.len(),
                            d.enablers,
                            d.payoffs,
                            if d.commanders.is_empty() {
                                ""
                            } else {
                                " · the commander leads it"
                            }
                        ))
                        .small(),
                    );
                }
                None => {
                    ui.label(
                        RichText::new(
                            "Your deck: not in your deck's colours (or no commander yet)",
                        )
                        .weak(),
                    );
                }
            });
        });
        // Auto-build.
        ui.add_space(4.0);
        let fits = |n: &str| self.db.get(n).is_some_and(|c| ar::commander_fits(a, c));
        let current = self.deck.commander.clone().filter(|c| fits(c));
        let mut owned: Vec<String> = a
            .commanders
            .iter()
            .filter(|n| self.coll.owned(n) > 0)
            .map(|n| n.to_string())
            .collect();
        for c in self.db.cards.iter() {
            if c.can_be_commander()
                && self.coll.owned(&c.name) > 0
                && !owned.contains(&c.name)
                && ar::commander_fits(a, c)
            {
                owned.push(c.name.clone());
            }
        }
        owned.retain(|n| Some(n) != current.as_ref());
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Auto-build this archetype").strong());
            if a.sparse {
                ui.label(RichText::new("Not buildable: too few cards.").weak());
                return;
            }
            let t = ar::targets(a);
            ui.label(
                RichText::new(format!(
                    "Build targets: {} lands · {} archetype cards first{}{}{}{}{}{}",
                    t.lands.map(|l| l.to_string()).unwrap_or("default".into()),
                    t.package,
                    if t.creatures > 0 { format!(" · creatures {}+", t.creatures) } else { String::new() },
                    if t.draw > 0 { format!(" · draw {}+", t.draw) } else { String::new() },
                    if t.ramp > 0 { format!(" · ramp {}+", t.ramp) } else { String::new() },
                    if t.removal > 0 { format!(" · removal {}+", t.removal) } else { String::new() },
                    if t.counters > 0 { format!(" · counterspells {}+", t.counters) } else { String::new() },
                    t.topend.map(|m| format!(" · at most {} cards of 6+ mana", m)).unwrap_or_default()
                ))
                .small(),
            )
            .on_hover_text("Per-archetype targets are the ones paired playtests supported; others use the general rules.");
            ui.horizontal_wrapped(|ui| {
                match &current {
                    Some(c) => {
                        if ui
                            .button(RichText::new(format!("Auto-build around {} (your commander)", c)).strong())
                            .on_hover_text("Replaces the deck with this archetype's build from your pool (Undo takes it back).")
                            .clicked()
                        {
                            acts.push(Action::AutoBuildArch(i, c.clone()));
                        }
                    }
                    None => {
                        if let Some(c) = &self.deck.commander {
                            ui.label(
                                RichText::new(format!("{} doesn't lead this archetype.", c))
                                    .small()
                                    .weak(),
                            );
                        }
                    }
                }
            });
            if !owned.is_empty() {
                ui.label(RichText::new("Commanders you own that fit:").small());
                ui.horizontal_wrapped(|ui| {
                    for n in owned.iter().take(12) {
                        if ui
                            .button(format!("Auto-build around {}", n))
                            .on_hover_text("Makes it your commander and builds this archetype from your pool (Undo takes it back).")
                            .clicked()
                        {
                            acts.push(Action::AutoBuildArch(i, n.clone()));
                        }
                    }
                });
            } else if current.is_none() {
                ui.label(
                    RichText::new("You own no commander that leads it - see the league commanders below.")
                        .small()
                        .color(NOTE),
                );
            }
        });
        // Commanders and key cards.
        ui.add_space(4.0);
        let owned_mark = |n: &str| self.coll.owned(n) > 0;
        ui.label(RichText::new("League commanders that lead it").strong());
        ui.horizontal_wrapped(|ui| {
            for n in a.commanders {
                let own = owned_mark(n);
                let t = RichText::new(if own {
                    format!("{} (owned)", n)
                } else {
                    n.to_string()
                });
                if ui.link(if own { t.color(GOOD) } else { t }).clicked() {
                    acts.push(Action::Select(n.to_string()));
                }
            }
        });
        if !a.key_cards.is_empty() {
            ui.label(RichText::new("Key cards").strong());
            ui.horizontal_wrapped(|ui| {
                for n in a.key_cards {
                    let own = owned_mark(n);
                    let in_deck = self.deck.used(n) > 0;
                    let label = if in_deck {
                        format!("{} (in deck)", n)
                    } else if own {
                        format!("{} (owned)", n)
                    } else {
                        n.to_string()
                    };
                    let t = RichText::new(label);
                    if ui
                        .link(if own || in_deck {
                            t.color(GOOD)
                        } else {
                            t.weak()
                        })
                        .clicked()
                    {
                        acts.push(Action::Select(n.to_string()));
                    }
                }
            });
        }
        // Fitting cards from the pool.
        ui.add_space(4.0);
        ui.label(RichText::new(format!("Fitting cards you own ({})", p.cards.len())).strong());
        let list: Vec<(String, bool, String, bool)> = p
            .cards
            .iter()
            .take(60)
            .map(|(c, f)| {
                (
                    c.name.clone(),
                    f.is_payoff(),
                    f.themes.join(", "),
                    self.deck.used(&c.name) > 0,
                )
            })
            .collect();
        egui::Grid::new(("arch_cards", i))
            .striped(true)
            .show(ui, |ui| {
                for (name, payoff, themes, in_deck) in &list {
                    if ui.link(name.as_str()).clicked() {
                        acts.push(Action::Select(name.clone()));
                    }
                    ui.label(
                        RichText::new(if *payoff { "payoff" } else { "enabler" })
                            .small()
                            .color(if *payoff {
                                GOLD
                            } else {
                                Color32::from_gray(170)
                            }),
                    );
                    let short: String = if themes.chars().count() > 40 {
                        format!("{}...", themes.chars().take(38).collect::<String>())
                    } else {
                        themes.clone()
                    };
                    ui.label(RichText::new(short).small().weak())
                        .on_hover_text(themes.as_str());
                    ui.label(RichText::new(if *in_deck { "in deck" } else { "" }).small());
                    ui.end_row();
                }
            });
        if p.cards.len() > list.len() {
            ui.label(
                RichText::new(format!("... and {} more", p.cards.len() - list.len()))
                    .small()
                    .weak(),
            );
        }
    }
}
