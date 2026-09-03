//! Pantalla de notas recientes.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
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
        "Notas recientes",
        theme::title_style(),
    )))
    .block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(title, chunks[0]);

    let items: Vec<ListItem> = if app.recent_notes.is_empty() {
        vec![ListItem::new(Span::styled(
            "  No hay notas aún.",
            theme::label_style(),
        ))]
    } else {
        app.recent_notes
            .iter()
            .map(|p| {
                let name = p
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("?")
                    .to_string();
                ListItem::new(Line::from(Span::styled(format!("  {}", name), theme::value_style())))
            })
            .collect()
    };
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(" Últimas 20 ", theme::label_style())),
    );
    f.render_widget(list, chunks[1]);

    let status = Paragraph::new("Esc para volver al menú")
        .style(theme::label_style())
        .block(Block::default().borders(Borders::TOP));
    f.render_widget(status, chunks[2]);
}
