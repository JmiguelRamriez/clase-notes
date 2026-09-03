//! Pantalla de procesamiento de WAV existente.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::tui::app::App;
use crate::tui::theme;

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .split(area);

    let title = Paragraph::new(Line::from(Span::styled(
        "Procesar WAV existente",
        theme::title_style(),
    )))
    .block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(title, chunks[0]);

    let body = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Por ahora usa la línea de comandos:",
            theme::label_style(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "    clase-notes process ruta/al/audio.wav \\",
            theme::value_style(),
        )),
        Line::from(Span::styled(
            "        --materia \"Cálculo II\" \\",
            theme::value_style(),
        )),
        Line::from(Span::styled(
            "        --tema \"Límites\" \\",
            theme::value_style(),
        )),
        Line::from(Span::styled(
            "        --tags calculo-ii,limites",
            theme::value_style(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Próximamente: selector interactivo desde la TUI.",
            theme::warn_style(),
        )),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(" Info ", theme::label_style())),
    );
    f.render_widget(body, chunks[1]);

    let status = Paragraph::new(app.status.clone())
        .style(theme::label_style())
        .block(Block::default().borders(Borders::TOP));
    f.render_widget(status, chunks[2]);
}
