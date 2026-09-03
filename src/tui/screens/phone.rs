//! Pantalla "Conectar teléfono": muestra un QR code para que el
//! iPhone se conecte al servidor WebSocket.

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

    // Header con indicador de conexión.
    let connected = app.phone_state.as_ref().is_some_and(|s| s.is_connected());
    let header_span = if connected {
        Span::styled(" ● CONECTADO ", theme::success_style())
    } else {
        Span::styled(" ○ Esperando... ", theme::warn_style())
    };
    let title = Paragraph::new(Line::from(vec![
        Span::styled("  ", theme::title_style()),
        Span::styled("Conectar teléfono", theme::title_style()),
        Span::raw("    "),
        header_span,
    ]))
    .block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(title, chunks[0]);

    // Cuerpo: split horizontal entre QR e instrucciones.
    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(chunks[1]);

    // QR code (centrado).
    let qr_text = build_qr_block(app);
    let qr_paragraph = Paragraph::new(qr_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(Span::styled(" Código QR ", theme::label_style())),
        )
        .style(theme::value_style())
        .alignment(ratatui::layout::Alignment::Center);
    f.render_widget(qr_paragraph, body_chunks[0]);

    // Instrucciones.
    let instructions = build_instructions(app);
    let instr_paragraph = Paragraph::new(instructions)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(Span::styled(" Instrucciones ", theme::label_style())),
        )
        .style(theme::value_style());
    f.render_widget(instr_paragraph, body_chunks[1]);

    // Status.
    let status_text = if connected {
        format!(
            "✓ iPhone conectado. Volvé al menú → Grabar clase (Fuente: Teléfono).  Estado: {}",
            app.status
        )
    } else {
        app.status.clone()
    };
    let status = Paragraph::new(status_text)
        .style(theme::value_style())
        .block(Block::default().borders(Borders::ALL).title(" Estado "));
    f.render_widget(status, chunks[2]);
}

fn build_qr_block(app: &App) -> Vec<Line<'static>> {
    let url = app.phone_url();

    match build_ascii_qr(&url) {
        Ok(lines) => lines,
        Err(e) => {
            vec![
                Line::from(Span::styled(
                    "⚠ No se pudo generar QR",
                    theme::warn_style(),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    format!("Copiá esta URL y abrila en el iPhone:"),
                    theme::label_style(),
                )),
                Line::from(""),
                Line::from(Span::styled(url, theme::value_style())),
                Line::from(""),
                Line::from(Span::styled(
                    format!("Error: {}", e),
                    theme::warn_style(),
                )),
            ]
        }
    }
}

/// Genera un QR code como caracteres half-block (`▀▄█`).
///
/// Cada caracter representa 2 filas del QR, manteniendo el aspect
/// ratio cuadrado en la mayoría de terminales. Sin colores ANSI —
/// funciona en cualquier terminal, fondo claro u oscuro, y se
/// puede copiar y pegar.
fn build_ascii_qr(data: &str) -> anyhow::Result<Vec<Line<'static>>> {
    use qr2term::render::Color;
    use qr2term::qr::Qr;

    let matrix = Qr::from(data.as_bytes())?.to_matrix();
    let size = matrix.size();
    let pixels = matrix.pixels();

    let mut lines = Vec::with_capacity(size.div_ceil(2) + 2);
    lines.push(Line::from(""));

    for row in (0..size).step_by(2) {
        let mut line = String::with_capacity(size + 2);
        line.push_str("  "); // margen
        for col in 0..size {
            let top = matches!(pixels[row * size + col], Color::Dark);
            let bottom = if row + 1 < size {
                matches!(pixels[(row + 1) * size + col], Color::Dark)
            } else {
                false
            };
            line.push(match (top, bottom) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        lines.push(Line::from(line));
    }

    lines.push(Line::from(""));
    Ok(lines)
}

fn build_instructions(app: &App) -> Vec<Line<'static>> {
    let url = app.phone_url();
    vec![
        Line::from(""),
        Line::from(Span::styled(
            "1. Conectá el iPhone a la misma WiFi que esta PC",
            theme::value_style(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "2. Escaneá el QR con la cámara del iPhone",
            theme::value_style(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "3. Safari muestra \"Conexión no privada\".",
            theme::warn_style(),
        )),
        Line::from(Span::styled(
            "   Tocá \"Avanzado\" → \"Visitar sitio web\"",
            theme::warn_style(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "4. Tocá el botón \"Iniciar\" y aceptá el micrófono",
            theme::value_style(),
        )),
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled(
            "─── Si no carga ───",
            theme::warn_style(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "• Verificá que la hora del iPhone sea correcta",
            theme::label_style(),
        )),
        Line::from(Span::styled(
            "• Tocá 'c' para copiar la URL y pegarla en Safari",
            theme::label_style(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "─── Atajos ───",
            theme::warn_style(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "c = copiar URL · ESC = volver",
            theme::warn_style(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "URL: ",
            theme::label_style(),
        )),
        Line::from(Span::styled(url, theme::value_style())),
    ]
}
