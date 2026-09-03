//! Pantalla de menú principal.

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

    // Header.
    let title = Paragraph::new(Line::from(vec![
        Span::styled("🎙  ", theme::title_style()),
        Span::styled("clase-notes", theme::title_style()),
        Span::raw("  ·  "),
        Span::styled(
            "Whisper local + Ollama + Obsidian",
            theme::label_style(),
        ),
    ]))
    .block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(title, chunks[0]);

    // Menú.
    let items = vec![
        ListItem::new("  Grabar clase"),
        ListItem::new("  Procesar WAV existente"),
        ListItem::new("  Ver notas recientes"),
        ListItem::new("  Conectar teléfono"),
        ListItem::new("  Salir"),
    ];
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(Span::styled(" Menú ", theme::label_style())),
        )
        .highlight_style(theme::selected_style())
        .highlight_symbol("▶ ");
    let mut state = ratatui::widgets::ListState::default();
    state.select(Some(app.menu_index));
    f.render_stateful_widget(list, chunks[1], &mut state);

    // Status.
    let status = Paragraph::new(app.status.clone())
        .style(theme::label_style())
        .block(Block::default().borders(Borders::TOP));
    f.render_widget(status, chunks[2]);
}
