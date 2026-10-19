//! Terminal formatting helpers (ANSI colours, compact mana costs, padding).

use crate::card::{Card, ColorSet, symbols_in};

#[derive(Clone, Copy)]
pub struct Style {
    pub color: bool,
}

impl Style {
    pub fn paint(&self, text: &str, code: &str) -> String {
        if self.color {
            format!("\x1b[{}m{}\x1b[0m", code, text)
        } else {
            text.to_string()
        }
    }

    pub fn bold(&self, text: &str) -> String {
        self.paint(text, "1")
    }

    pub fn dim(&self, text: &str) -> String {
        self.paint(text, "2")
    }

    pub fn good(&self, text: &str) -> String {
        self.paint(text, "32")
    }

    pub fn warn(&self, text: &str) -> String {
        self.paint(text, "33")
    }

    pub fn bad(&self, text: &str) -> String {
        self.paint(text, "31")
    }

    pub fn identity_code(id: ColorSet, land: bool) -> &'static str {
        if land && id.is_empty() {
            return "37";
        }
        match id.count() {
            0 => "36",
            1 => match id.indices().next().unwrap() {
                0 => "97",
                1 => "94",
                2 => "35",
                3 => "91",
                _ => "92",
            },
            _ => "33",
        }
    }

    /// Card name tinted by colour identity (gold for multicolour, cyan for colourless).
    pub fn name(&self, card: &Card) -> String {
        self.paint(
            &card.name,
            Style::identity_code(card.identity(), card.is_land()),
        )
    }

    pub fn name_padded(&self, card: &Card, width: usize) -> String {
        let pad = width.saturating_sub(card.name.chars().count());
        format!("{}{}", self.name(card), " ".repeat(pad))
    }
}

/// "{2}{R/W}" -> "2(R/W)", "{4}{R}{R}" -> "4RR".
pub fn compact_cost(cost: &str) -> String {
    symbols_in(cost)
        .into_iter()
        .map(|s| {
            if s.contains('/') {
                format!("({})", s)
            } else {
                s
            }
        })
        .collect()
}

pub fn pad(s: &str, width: usize) -> String {
    let n = s.chars().count();
    if n >= width {
        s.chars().take(width).collect()
    } else {
        format!("{}{}", s, " ".repeat(width - n))
    }
}

pub fn bar(n: u32, scale: u32) -> String {
    "█".repeat((n * scale) as usize)
}

pub fn pct(x: f32) -> String {
    format!("{:>3.0}%", x * 100.0)
}

/// Word-wraps rules text for the `show` view.
pub fn wrap(text: &str, width: usize, indent: &str) -> String {
    let mut out = String::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            if line.chars().count() + word.chars().count() + 1 > width && !line.is_empty() {
                out.push_str(indent);
                out.push_str(&line);
                out.push('\n');
                line.clear();
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        out.push_str(indent);
        out.push_str(&line);
        out.push('\n');
    }
    out
}
