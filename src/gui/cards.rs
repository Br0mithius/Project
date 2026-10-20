//! Draws Magic cards: a full rendered card face (frame, name bar, mana symbols, art panel,
//! type line, rules text, P/T), or the real card picture when one has been downloaded.

use crate::card::{Card, ColorSet, symbols_in};
use eframe::egui;
use egui::{
    Align2, Color32, CornerRadius, FontId, Rect, Response, Sense, Stroke, StrokeKind,
    TextureHandle, Ui, Vec2, pos2, vec2,
};

/// Standard card proportions (63 x 88 mm).
pub const CARD_RATIO: f32 = 88.0 / 63.0;

pub fn card_size(width: f32) -> Vec2 {
    vec2(width, width * CARD_RATIO)
}

/// Frame colour for a card's colour identity.
pub fn frame_color(card: &Card) -> Color32 {
    let id = if card.is_land() {
        card.produces()
    } else {
        card.identity()
    };
    if card.is_land() && (id.is_empty() || id.count() == 5) {
        return Color32::from_rgb(196, 178, 148);
    }
    match id.count() {
        0 => Color32::from_rgb(178, 188, 196),
        1 => match id.indices().next().unwrap() {
            0 => Color32::from_rgb(244, 238, 212),
            1 => Color32::from_rgb(120, 170, 220),
            2 => Color32::from_rgb(110, 102, 98),
            3 => Color32::from_rgb(222, 112, 84),
            _ => Color32::from_rgb(112, 168, 110),
        },
        _ => Color32::from_rgb(222, 196, 110),
    }
}

/// Darker shade used for the art panel and accents.
fn shade(c: Color32, f: f32) -> Color32 {
    Color32::from_rgb(
        (c.r() as f32 * f) as u8,
        (c.g() as f32 * f) as u8,
        (c.b() as f32 * f) as u8,
    )
}

fn lighten(c: Color32, f: f32) -> Color32 {
    let l = |v: u8| (v as f32 + (255.0 - v as f32) * f) as u8;
    Color32::from_rgb(l(c.r()), l(c.g()), l(c.b()))
}

/// Colour of one mana symbol and the colour of the text on it.
pub fn symbol_colors(sym: &str) -> (Color32, Color32) {
    match sym.split('/').next().unwrap_or("") {
        "W" => (Color32::from_rgb(250, 247, 224), Color32::BLACK),
        "U" => (Color32::from_rgb(40, 120, 200), Color32::WHITE),
        "B" => (Color32::from_rgb(45, 38, 35), Color32::WHITE),
        "R" => (Color32::from_rgb(214, 50, 40), Color32::WHITE),
        "G" => (Color32::from_rgb(20, 130, 70), Color32::WHITE),
        _ => (Color32::from_rgb(204, 198, 192), Color32::BLACK),
    }
}

/// Draws mana symbols right-aligned ending at `right`, vertically centred on `y`.
/// Returns the x where the symbols start.
pub fn draw_cost(painter: &egui::Painter, cost: &str, right: f32, y: f32, d: f32) -> f32 {
    let syms = symbols_in(cost);
    let gap = d * 0.12;
    let mut x = right - syms.len() as f32 * (d + gap);
    let start = x;
    for s in syms {
        let c = pos2(x + d / 2.0, y);
        let (fill, text) = symbol_colors(&s);
        painter.circle_filled(c, d / 2.0, fill);
        if let Some(second) = s.split('/').nth(1) {
            // Hybrid: right half in the second colour.
            let r = d / 2.0;
            let pts: Vec<egui::Pos2> = (0..=16)
                .map(|k| {
                    let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * k as f32 / 16.0;
                    pos2(c.x + r * a.cos(), c.y + r * a.sin())
                })
                .collect();
            painter.add(egui::Shape::convex_polygon(
                pts,
                symbol_colors(second).0,
                Stroke::NONE,
            ));
        }
        painter.circle_stroke(c, d / 2.0, Stroke::new(1.0, Color32::from_gray(150)));
        let label = if s.contains('/') {
            String::new()
        } else {
            s.clone()
        };
        if !label.is_empty() {
            painter.text(
                c,
                Align2::CENTER_CENTER,
                label,
                FontId::proportional(d * 0.72),
                text,
            );
        }
        x += d + gap;
    }
    start
}

/// A row of mana symbols as a widget (used in lists).
pub fn cost_widget(ui: &mut Ui, cost: &str, d: f32) -> Response {
    let n = symbols_in(cost).len() as f32;
    let (rect, resp) = ui.allocate_exact_size(vec2((n * d * 1.12).max(1.0), d), Sense::hover());
    draw_cost(ui.painter(), cost, rect.right(), rect.center().y, d);
    resp
}

pub fn rarity_color(rarity: &str) -> Color32 {
    match rarity {
        "mythic" => Color32::from_rgb(232, 96, 32),
        "rare" => Color32::from_rgb(214, 176, 70),
        "uncommon" => Color32::from_rgb(170, 190, 205),
        "special" => Color32::from_rgb(160, 90, 200),
        _ => Color32::from_rgb(30, 30, 30),
    }
}

pub struct CardLook<'a> {
    pub card: &'a Card,
    pub width: f32,
    /// Real card picture, if downloaded.
    pub image: Option<&'a TextureHandle>,
    /// Small label in the top-left corner, e.g. "2/3" owned or "x2".
    pub badge: Option<String>,
    /// Greyed out (none left to add / not legal here).
    pub dimmed: bool,
    pub selected: bool,
}

/// Draws a card and returns its click response.
pub fn card_widget(ui: &mut Ui, look: CardLook) -> Response {
    let size = card_size(look.width);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        paint_card(ui, rect, &look, resp.hovered());
    }
    resp
}

pub fn paint_card(ui: &Ui, rect: Rect, look: &CardLook, hovered: bool) {
    let painter = ui.painter_at(rect.expand(4.0));
    let w = rect.width();
    let radius = CornerRadius::from(w * 0.045);
    match look.image {
        Some(tex) => {
            painter.image(
                tex.id(),
                rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        None => paint_card_face(&painter, rect, look.card),
    }
    if look.dimmed {
        painter.rect_filled(rect, radius, Color32::from_black_alpha(140));
    }
    if look.selected {
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(3.0, Color32::from_rgb(255, 210, 60)),
            StrokeKind::Outside,
        );
    } else if hovered {
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(2.0, Color32::from_rgb(140, 200, 255)),
            StrokeKind::Outside,
        );
    }
    if let Some(b) = &look.badge {
        let font = FontId::proportional((w * 0.1).clamp(10.0, 18.0));
        let galley = painter.layout_no_wrap(b.clone(), font, Color32::WHITE);
        let pad = vec2(6.0, 2.0);
        let r = Rect::from_min_size(
            rect.min + vec2(w * 0.04, w * 0.16),
            galley.size() + pad * 2.0,
        );
        painter.rect_filled(
            r,
            CornerRadius::from(6.0),
            Color32::from_rgba_unmultiplied(20, 20, 30, 225),
        );
        painter.galley(r.min + pad, galley, Color32::WHITE);
    }
}

/// The rendered card face.
pub fn paint_card_face(painter: &egui::Painter, rect: Rect, card: &Card) {
    let w = rect.width();
    let h = rect.height();
    let u = w / 63.0; // one "millimetre" of card
    let outer = CornerRadius::from(w * 0.045);
    let frame = frame_color(card);
    let ink = Color32::from_rgb(20, 18, 16);

    // Black border and coloured frame.
    painter.rect_filled(rect, outer, Color32::from_rgb(16, 16, 18));
    let inner = rect.shrink(u * 2.6);
    painter.rect_filled(inner, CornerRadius::from(u * 1.5), frame);

    // Name bar.
    let bar_h = u * 6.0;
    let name_bar = Rect::from_min_size(
        inner.min + vec2(u * 0.8, u * 0.8),
        vec2(inner.width() - u * 1.6, bar_h),
    );
    painter.rect_filled(name_bar, CornerRadius::from(u * 2.0), lighten(frame, 0.55));
    painter.rect_stroke(
        name_bar,
        CornerRadius::from(u * 2.0),
        Stroke::new(u * 0.3, shade(frame, 0.55)),
        StrokeKind::Inside,
    );
    let sym_d = bar_h * 0.72;
    let cost_left = draw_cost(
        painter,
        &card.mana_cost,
        name_bar.right() - u * 1.2,
        name_bar.center().y,
        sym_d,
    );
    let name_room = (cost_left - name_bar.left() - u * 2.4).max(u * 10.0);
    let mut name_size = bar_h * 0.62;
    let mut galley =
        painter.layout_no_wrap(card.name.clone(), FontId::proportional(name_size), ink);
    while galley.size().x > name_room && name_size > 6.0 {
        name_size *= 0.92;
        galley = painter.layout_no_wrap(card.name.clone(), FontId::proportional(name_size), ink);
    }
    painter.galley(
        pos2(
            name_bar.left() + u * 1.4,
            name_bar.center().y - galley.size().y / 2.0,
        ),
        galley,
        ink,
    );

    // Art panel: a tinted panel with the card's initial and main type, since there's no art.
    let art = Rect::from_min_size(
        pos2(inner.left() + u * 2.0, name_bar.bottom() + u * 0.8),
        vec2(inner.width() - u * 4.0, h * 0.36),
    );
    let art_col = shade(frame, 0.55);
    painter.rect_filled(art, CornerRadius::ZERO, art_col);
    for i in 0..6 {
        // Soft diagonal stripes for texture.
        let t = i as f32 / 6.0;
        let a = pos2(art.left() + art.width() * t, art.bottom());
        let b = pos2(art.left() + art.width() * (t + 0.35), art.top());
        painter.line_segment([a, b], Stroke::new(u * 3.0, Color32::from_white_alpha(10)));
    }
    let initial: String = card
        .name
        .chars()
        .next()
        .map(|c| c.to_string())
        .unwrap_or_default();
    painter.text(
        art.center(),
        Align2::CENTER_CENTER,
        initial,
        FontId::proportional(art.height() * 0.62),
        Color32::from_white_alpha(46),
    );
    painter.text(
        pos2(art.right() - u * 1.5, art.bottom() - u * 1.2),
        Align2::RIGHT_BOTTOM,
        card.main_type().to_uppercase(),
        FontId::proportional(u * 2.6),
        Color32::from_white_alpha(120),
    );
    painter.rect_stroke(
        art,
        CornerRadius::ZERO,
        Stroke::new(u * 0.4, shade(frame, 0.35)),
        StrokeKind::Inside,
    );

    // Type line.
    let type_bar = Rect::from_min_size(
        pos2(name_bar.left(), art.bottom() + u * 0.6),
        vec2(name_bar.width(), bar_h * 0.9),
    );
    painter.rect_filled(type_bar, CornerRadius::from(u * 2.0), lighten(frame, 0.55));
    painter.rect_stroke(
        type_bar,
        CornerRadius::from(u * 2.0),
        Stroke::new(u * 0.3, shade(frame, 0.55)),
        StrokeKind::Inside,
    );
    let gem_c = pos2(type_bar.right() - u * 3.2, type_bar.center().y);
    painter.circle_filled(gem_c, u * 1.6, rarity_color(&card.rarity));
    painter.circle_stroke(gem_c, u * 1.6, Stroke::new(u * 0.25, Color32::BLACK));
    let mut type_size = bar_h * 0.5;
    let type_room = type_bar.width() - u * 7.0;
    let mut tg =
        painter.layout_no_wrap(card.type_line.clone(), FontId::proportional(type_size), ink);
    while tg.size().x > type_room && type_size > 5.0 {
        type_size *= 0.92;
        tg = painter.layout_no_wrap(card.type_line.clone(), FontId::proportional(type_size), ink);
    }
    painter.galley(
        pos2(
            type_bar.left() + u * 1.4,
            type_bar.center().y - tg.size().y / 2.0,
        ),
        tg,
        ink,
    );

    // Rules text box.
    let text_box = Rect::from_min_max(
        pos2(inner.left() + u * 2.0, type_bar.bottom() + u * 0.6),
        pos2(inner.right() - u * 2.0, inner.bottom() - u * 4.4),
    );
    painter.rect_filled(
        text_box,
        CornerRadius::from(u * 0.6),
        Color32::from_rgb(246, 240, 226),
    );
    let text = rules_text_for_display(card);
    let pad = u * 1.4;
    let wrap = text_box.width() - pad * 2.0;
    let mut size = (u * 2.7).max(5.0);
    let mut g = painter.layout(text.clone(), FontId::proportional(size), ink, wrap);
    while g.size().y > text_box.height() - pad * 2.0 && size > 4.5 {
        size *= 0.92;
        g = painter.layout(text.clone(), FontId::proportional(size), ink, wrap);
    }
    painter
        .with_clip_rect(text_box.shrink(u * 0.5))
        .galley(text_box.min + vec2(pad, pad), g, ink);

    // Footer: set/number, P/T box.
    let foot_y = inner.bottom() - u * 2.2;
    painter.text(
        pos2(inner.left() + u * 2.0, foot_y),
        Align2::LEFT_CENTER,
        format!(
            "{} {} · {}",
            if card.set.is_empty() {
                "MSH"
            } else {
                &card.set
            },
            card.collector_number.clone().unwrap_or_default(),
            card.rarity
                .chars()
                .next()
                .map(|c| c.to_ascii_uppercase())
                .unwrap_or(' ')
        ),
        FontId::proportional(u * 2.2),
        Color32::from_white_alpha(170),
    );
    if let Some(pt) = card.power_toughness() {
        let pt_rect = Rect::from_center_size(
            pos2(inner.right() - u * 7.0, inner.bottom() - u * 3.0),
            vec2(u * 11.0, u * 5.2),
        );
        painter.rect_filled(pt_rect, CornerRadius::from(u * 1.5), lighten(frame, 0.55));
        painter.rect_stroke(
            pt_rect,
            CornerRadius::from(u * 1.5),
            Stroke::new(u * 0.4, shade(frame, 0.45)),
            StrokeKind::Inside,
        );
        painter.text(
            pt_rect.center(),
            Align2::CENTER_CENTER,
            pt,
            FontId::proportional(u * 3.4),
            ink,
        );
    }
}

/// Rules text with mana symbols written plainly ({T} -> "Tap") so it reads well as text.
pub fn rules_text_for_display(card: &Card) -> String {
    card.oracle_text
        .replace("{T}", "Tap")
        .replace("{Q}", "Untap")
        .replace(['{', '}'], "")
}

/// Small coloured dots for a colour identity, e.g. in commander lists.
pub fn identity_dots(ui: &mut Ui, id: ColorSet, d: f32) {
    let n = id.count().max(1) as f32;
    let (rect, _) = ui.allocate_exact_size(vec2(n * (d + 3.0), d), Sense::hover());
    if id.is_empty() {
        ui.painter().circle_filled(
            rect.left_center() + vec2(d / 2.0, 0.0),
            d / 2.0,
            symbol_colors("C").0,
        );
        return;
    }
    for (k, i) in id.indices().enumerate() {
        let sym = crate::card::COLORS[i].to_string();
        let c = rect.left_center() + vec2(k as f32 * (d + 3.0) + d / 2.0, 0.0);
        ui.painter()
            .circle_filled(c, d / 2.0, symbol_colors(&sym).0);
        ui.painter()
            .circle_stroke(c, d / 2.0, Stroke::new(1.0, Color32::from_gray(150)));
    }
}

/// A frame colour made readable as text on the dark background.
pub fn lighten_for_text(c: Color32) -> Color32 {
    lighten(c, 0.35)
}
