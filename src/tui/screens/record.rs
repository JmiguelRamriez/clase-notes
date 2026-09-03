//! Pantalla de grabación: formulario + estados (idle/recording/processing).

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
            Constraint::Min(8),
            Constraint::Length(5),
        ])
        .split(area);

    // Header.
    let title = Paragraph::new(Line::from(vec![
        Span::styled("●  ", theme::title_style()),
        Span::styled("Grabar clase", theme::title_style()),
    ]))
    .block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(title, chunks[0]);

    // Form.
    let form = &app.record_form;
    let active = form.editing_field;
    let field_style = |i: usize| {
        if i == active {
            theme::selected_style()
        } else {
            theme::value_style()
        }
    };
    let label_style = |i: usize| {
        if i == active {
            theme::title_style()
        } else {
            theme::label_style()
        }
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled("  Materia:  ", label_style(0)),
            Span::styled(
                if form.materia.is_empty() && active == 0 {
                    "_".to_string()
                } else {
                    form.materia.clone()
                },
                field_style(0),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Tema:     ", label_style(1)),
            Span::styled(
                if form.tema.is_empty() && active == 1 {
                    "_".to_string()
                } else {
                    form.tema.clone()
                },
                field_style(1),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Fecha:    ", theme::label_style()),
            Span::styled(form.date.to_string(), theme::value_style()),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Tags:     ", label_style(2)),
            Span::styled(
                if form.tags.is_empty() && active == 2 {
                    "_".to_string()
                } else {
                    form.tags.clone()
                },
                field_style(2),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Fuente:   ", label_style(3)),
            Span::styled(app.audio_source.label(), field_style(3)),
            Span::styled("  (Tab para cambiar)", theme::label_style()),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Filtro:   ", label_style(4)),
            Span::styled(app.audio_preset.label(), field_style(4)),
            Span::styled("  (Tab para ciclar)", theme::label_style()),
        ]),
    ];
    if !app.is_busy {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Tab para cambiar campo · Enter para empezar a grabar",
            theme::label_style(),
        )));
    } else {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Espacio = pausa · Enter = detener · Esc = cancelar",
            theme::warn_style(),
        )));
    }
    let form_paragraph = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(" Datos de la clase ", theme::label_style())),
    );
    f.render_widget(form_paragraph, chunks[1]);

    // Status.
    let status = Paragraph::new(app.status.clone())
        .style(theme::value_style())
        .block(Block::default().borders(Borders::ALL).title(" Estado "));
    f.render_widget(status, chunks[2]);
}
