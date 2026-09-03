//! Pantalla de procesamiento de WAV existente — formulario + selector de archivos.

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
            Constraint::Length(12),
            Constraint::Min(6),
            Constraint::Length(3),
        ])
        .split(area);

    // Header
    let title = Paragraph::new(Line::from(vec![
        Span::styled("▶  ", theme::title_style()),
        Span::styled("Procesar WAV existente", theme::title_style()),
        if app.is_busy {
            Span::styled("  ● Procesando...", theme::warn_style())
        } else {
            Span::raw("")
        },
    ]))
    .block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(title, chunks[0]);

    // Form
    let form = &app.process_form;
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

    let wav_display = if form.wav_path.is_empty() && active == 0 {
        "_".to_string()
    } else if form.wav_path.is_empty() {
        "(vacío)".to_string()
    } else {
        // Mostrar solo nombre si la ruta es muy larga, pero mantener tooltip con ruta completa en status
        form.wav_path.clone()
    };
    let materia_display = if form.materia.is_empty() && active == 1 {
        "_".to_string()
    } else {
        form.materia.clone()
    };
    let tema_display = if form.tema.is_empty() && active == 2 {
        "_".to_string()
    } else {
        form.tema.clone()
    };
    let tags_display = if form.tags.is_empty() && active == 3 {
        "_".to_string()
    } else {
        form.tags.clone()
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled("  WAV:      ", label_style(0)),
            Span::styled(wav_display, field_style(0)),
        ]),
        Line::from(vec![
            Span::styled("  Materia:  ", label_style(1)),
            Span::styled(materia_display, field_style(1)),
        ]),
        Line::from(vec![
            Span::styled("  Tema:     ", label_style(2)),
            Span::styled(tema_display, field_style(2)),
        ]),
        Line::from(vec![
            Span::styled("  Tags:     ", label_style(3)),
            Span::styled(tags_display, field_style(3)),
        ]),
        Line::from(vec![
            Span::styled("  Fecha:    ", theme::label_style()),
            Span::styled(form.date.to_string(), theme::value_style()),
        ]),
    ];
    if !app.is_busy {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Tab / Shift+Tab para cambiar campo · ↑/↓ navega WAVs · Enter procesa · Esc vuelve",
            theme::label_style(),
        )));
    } else {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Procesando... Esc para volver al menú (el proceso sigue en fondo)",
            theme::warn_style(),
        )));
    }

    let form_block = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(" Datos ", theme::label_style())),
    );
    f.render_widget(form_block, chunks[1]);

    // Picker de WAVs
    let picker_title = format!(" WAVs detectados ({}) ", app.process_wavs.len());
    let items: Vec<ListItem> = if app.process_wavs.is_empty() {
        vec![
            ListItem::new(Span::styled(
                "  No se encontraron WAVs. Escribí la ruta manualmente arriba.",
                theme::label_style(),
            )),
            ListItem::new(Span::styled(
                "  Busca en: ./ , ./grabaciones/, ~/.local/share/clase-notes/recordings/",
                theme::label_style(),
            )),
        ]
    } else {
        app.process_wavs
            .iter()
            .enumerate()
            .map(|(idx, p)| {
                let name = p.display().to_string();
                let style = if idx == app.process_picker_index && active == 0 {
                    theme::selected_style()
                } else {
                    theme::value_style()
                };
                let prefix = if idx == app.process_picker_index && active == 0 {
                    "▶ "
                } else {
                    "  "
                };
                // Verificar si existe
                let exists = p.exists();
                let suffix = if !exists { " (no existe)" } else { "" };
                ListItem::new(Line::from(vec![
                    Span::styled(prefix, style),
                    Span::styled(format!("{}{}", name, suffix), style),
                ]))
            })
            .collect()
    };
    let picker = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(picker_title, theme::label_style())),
    );
    f.render_widget(picker, chunks[2]);

    // Status
    let status = Paragraph::new(app.status.clone())
        .style(theme::label_style())
        .block(Block::default().borders(Borders::TOP));
    f.render_widget(status, chunks[3]);
}
