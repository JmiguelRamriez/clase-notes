//! Paleta de colores de la TUI. Oscura y monocromática con un acento.

use ratatui::style::{Color, Modifier, Style};

pub const BG: Color = Color::Rgb(15, 18, 24);
pub const FG: Color = Color::Rgb(220, 225, 235);
pub const MUTED: Color = Color::Rgb(120, 130, 150);
pub const ACCENT: Color = Color::Rgb(180, 140, 255);
pub const SUCCESS: Color = Color::Rgb(120, 220, 160);
#[allow(dead_code)]
const _SUCCESS: Color = SUCCESS;
pub const WARN: Color = Color::Rgb(240, 180, 80);
#[allow(dead_code)]
const ERROR: Color = Color::Rgb(240, 90, 90);

pub fn title_style() -> Style {
    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
}

pub fn label_style() -> Style {
    Style::default().fg(MUTED)
}

pub fn value_style() -> Style {
    Style::default().fg(FG)
}

pub fn selected_style() -> Style {
    Style::default()
        .fg(BG)
        .bg(ACCENT)
        .add_modifier(Modifier::BOLD)
}

#[allow(dead_code)]
pub fn success_style() -> Style {
    Style::default().fg(SUCCESS)
}

pub fn warn_style() -> Style {
    Style::default().fg(WARN)
}

#[allow(dead_code)]
pub fn error_style() -> Style {
    Style::default().fg(ERROR)
}
