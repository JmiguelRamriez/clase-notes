//! Estado de la aplicación TUI y bucle principal.

use anyhow::Result;
use chrono::{Local, NaiveDate};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};
use tokio::sync::mpsc;

use crate::config::Config;
use crate::pipeline::Pipeline;
use crate::tui::screens;

/// Pantalla actual de la TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Menu,
    Record,
    Process,
    Recent,
    Phone,
}

/// Mensajes del hilo de grabación/procesamiento al hilo de la TUI.
#[derive(Debug, Clone)]
pub enum UiMessage {
    RecordingFinished {
        path: PathBuf,
        duration_secs: f32,
    },
    RecordingCancelled,
    LlmProgress(String),
    ProcessingFinished {
        note_path: PathBuf,
    },
    PhoneServerReady(Arc<crate::audio::PhoneState>),
    Error(String),
}

/// Estado mutable de la app.
pub struct App {
    pub config: Config,
    pub pipeline: Arc<OnceLock<Arc<Pipeline>>>,
    pub screen: Screen,
    pub menu_index: usize,
    pub record_form: RecordForm,
    pub status: String,
    pub is_busy: bool,
    pub recent_notes: Vec<PathBuf>,
    /// Flag global para detener grabación (compartido con AudioRecorder).
    pub stop_flag: Arc<AtomicBool>,
    pub tx: mpsc::UnboundedSender<UiMessage>,
    pub rx: mpsc::UnboundedReceiver<UiMessage>,
    /// Puerto del servidor del teléfono.
    pub phone_port: u16,
    /// Fuente de audio seleccionada en la pantalla de grabación.
    pub audio_source: AudioSource,
    /// Preset de filtrado de audio.
    pub audio_preset: crate::audio::AudioPreset,
    /// Estado del teléfono (server ya arrancado).
    pub phone_state: Option<Arc<crate::audio::PhoneState>>,
}

#[derive(Debug, Clone, Default)]
pub struct RecordForm {
    pub materia: String,
    pub tema: String,
    pub date: NaiveDate,
    pub tags: String,
    pub editing_field: usize, // 0=materia, 1=tema, 2=tags, 3=source, 4=preset
}

/// Fuente de audio para la grabación.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSource {
    Microphone,
    Phone,
}

impl Default for AudioSource {
    fn default() -> Self {
        Self::Microphone
    }
}

impl AudioSource {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Microphone => "Micrófono",
            Self::Phone => "Teléfono",
        }
    }

    pub fn next(&self) -> Self {
        match self {
            Self::Microphone => Self::Phone,
            Self::Phone => Self::Microphone,
        }
    }
}

impl App {
    pub fn new(config: Config) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let phone_port = config.phone.port;
        let audio_preset = crate::audio::AudioPreset::from_str(&config.phone.preset);
        Self {
            config,
            pipeline: Arc::new(OnceLock::new()),
            screen: Screen::Menu,
            menu_index: 0,
            record_form: RecordForm {
                date: Local::now().date_naive(),
                ..Default::default()
            },
            status: String::from("Listo. Usa ↑/↓ y Enter."),
            is_busy: false,
            recent_notes: Vec::new(),
            stop_flag: Arc::new(AtomicBool::new(false)),
            tx,
            rx,
            phone_port,
            audio_source: AudioSource::default(),
            audio_preset,
            phone_state: None,
        }
    }

    /// URL del servidor del teléfono en la red local.
    pub fn phone_url(&self) -> String {
        let ip = crate::audio::local_ip();
        format!("https://{}:{}/", ip, self.phone_port)
    }

    /// Carga el pipeline (Whisper + LLM). Pesado; se hace bajo demanda y
    /// se cachea. `None` en la primera carga se considera "no listo
    /// todavía"; si falla, devuelve error.
    pub fn pipeline(&self) -> Result<Arc<Pipeline>> {
        if let Some(p) = self.pipeline.get() {
            return Ok(Arc::clone(p));
        }
        // El pipeline se construye una sola vez.
        let p = Arc::new(Pipeline::new(self.config.clone())?);
        let _ = self.pipeline.set(Arc::clone(&p));
        Ok(p)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Result<AppAction> {
        if key.kind != KeyEventKind::Press {
            return Ok(AppAction::None);
        }
        match self.screen {
            Screen::Menu => self.handle_menu_key(key),
            Screen::Record => self.handle_record_key(key),
            Screen::Process => Ok(AppAction::None),
            Screen::Recent => self.handle_recent_key(key),
            Screen::Phone => self.handle_phone_key(key),
        }
    }

    fn handle_menu_key(&mut self, key: KeyEvent) -> Result<AppAction> {
        let items = 5;
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Ok(AppAction::Quit),
            KeyCode::Up | KeyCode::Char('k') => {
                if self.menu_index > 0 {
                    self.menu_index -= 1;
                }
                Ok(AppAction::None)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.menu_index + 1 < items {
                    self.menu_index += 1;
                }
                Ok(AppAction::None)
            }
            KeyCode::Enter => {
                let next = match self.menu_index {
                    0 => Screen::Record,
                    1 => Screen::Process,
                    2 => {
                        self.load_recent()?;
                        Screen::Recent
                    }
                    3 => {
                        // Arrancar el servidor del teléfono si no está corriendo.
                        if self.phone_state.is_none() {
                            self.status = "Iniciando servidor del teléfono...".into();
                            let port = self.phone_port;
                            let tx = self.tx.clone();
                            tokio::spawn(async move {
                                use crate::config::work_dir;
                                let cert_path = match work_dir() {
                                    Ok(w) => w.join("phone-server.pem"),
                                    Err(e) => {
                                        let _ = tx.send(UiMessage::Error(format!(
                                            "work_dir: {}",
                                            e
                                        )));
                                        return;
                                    }
                                };
                                match crate::audio::start_server(port, cert_path).await {
                                    Ok(state) => {
                                        let _ = tx.send(UiMessage::PhoneServerReady(
                                            Arc::new(state),
                                        ));
                                    }
                                    Err(e) => {
                                        let _ = tx.send(UiMessage::Error(format!(
                                            "servidor teléfono: {:#}",
                                            e
                                        )));
                                    }
                                }
                            });
                        }
                        Screen::Phone
                    }
                    _ => {
                        return Ok(AppAction::Quit);
                    }
                };
                self.screen = next;
                Ok(AppAction::None)
            }
            _ => Ok(AppAction::None),
        }
    }

    fn handle_phone_key(&mut self, key: KeyEvent) -> Result<AppAction> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.screen = Screen::Menu;
                Ok(AppAction::None)
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                // Copiar URL al portapapeles.
                let url = self.phone_url();
                let mut copied = false;
                let mut error_msg = String::new();
                match copy_to_clipboard(&url) {
                    Ok(()) => {
                        copied = true;
                    }
                    Err(e) => {
                        error_msg = e.to_string();
                    }
                }

                // Además, imprimir la URL al scrollback del main screen
                // saliendo momentáneamente del alternate screen. Esto
                // permite que la URL quede visible y copiable incluso
                // si el portapapeles no funciona.
                print_to_scrollback(&url);

                if copied {
                    self.status = format!("✓ URL copiada al portapapeles: {}", url);
                } else {
                    self.status = format!(
                        "✗ No se pudo copiar: {} | La URL también se imprimió en el scrollback (abajo).",
                        error_msg
                    );
                }
                Ok(AppAction::None)
            }
            _ => Ok(AppAction::None),
        }
    }

    fn handle_record_key(&mut self, key: KeyEvent) -> Result<AppAction> {
        // Si está grabando, las teclas son diferentes.
        if self.is_busy {
            match key.code {
                KeyCode::Char(' ') => {
                    return Ok(AppAction::TogglePause);
                }
                KeyCode::Enter => {
                    return Ok(AppAction::StopRecording);
                }
                KeyCode::Esc => {
                    return Ok(AppAction::CancelRecording);
                }
                _ => return Ok(AppAction::None),
            }
        }
        // Modo edición de formulario.
        match key.code {
            KeyCode::Esc => {
                self.screen = Screen::Menu;
                Ok(AppAction::None)
            }
            KeyCode::Tab => {
                self.record_form.editing_field =
                    (self.record_form.editing_field + 1) % 5;
                Ok(AppAction::None)
            }
            KeyCode::Backspace => {
                // Backspace solo aplica a campos de texto (0-2).
                if self.record_form.editing_field <= 2 {
                    self.record_form_pop();
                }
                Ok(AppAction::None)
            }
            KeyCode::Left | KeyCode::Right => {
                // En campos 3 (fuente) y 4 (preset), las flechas ciclan.
                match self.record_form.editing_field {
                    3 => {
                        self.audio_source = self.audio_source.next();
                    }
                    4 => {
                        self.audio_preset = self.audio_preset.next();
                    }
                    _ => {}
                }
                Ok(AppAction::None)
            }
            KeyCode::Char(c) => {
                if self.record_form.editing_field <= 2 {
                    self.record_form_push(c);
                }
                Ok(AppAction::None)
            }
            KeyCode::Enter => {
                if self.record_form.materia.is_empty()
                    || self.record_form.tema.is_empty()
                {
                    self.status = "Materia y tema son obligatorios".into();
                    return Ok(AppAction::None);
                }
                return Ok(AppAction::StartRecording);
            }
            _ => Ok(AppAction::None),
        }
    }

    fn handle_recent_key(&mut self, key: KeyEvent) -> Result<AppAction> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.screen = Screen::Menu;
                Ok(AppAction::None)
            }
            _ => Ok(AppAction::None),
        }
    }

    fn record_form_pop(&mut self) {
        match self.record_form.editing_field {
            0 => {
                self.record_form.materia.pop();
            }
            1 => {
                self.record_form.tema.pop();
            }
            2 => {
                self.record_form.tags.pop();
            }
            _ => {}
        }
    }

    fn record_form_push(&mut self, c: char) {
        match self.record_form.editing_field {
            0 => self.record_form.materia.push(c),
            1 => self.record_form.tema.push(c),
            2 => self.record_form.tags.push(c),
            _ => {}
        }
    }

    fn load_recent(&mut self) -> Result<()> {
        let dir = self.config.notes_dir();
        self.recent_notes.clear();
        if dir.exists() {
            let mut entries: Vec<_> = std::fs::read_dir(&dir)?
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path()
                        .extension()
                        .map(|x| x == "md")
                        .unwrap_or(false)
                })
                .map(|e| e.path())
                .collect();
            entries.sort();
            entries.reverse();
            self.recent_notes = entries.into_iter().take(20).collect();
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub enum AppAction {
    None,
    Quit,
    StartRecording,
    StopRecording,
    CancelRecording,
    TogglePause,
}

/// Dibuja un frame de la TUI.
pub fn draw(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    terminal.draw(|f| {
        let area = f.area();
        match app.screen {
            Screen::Menu => screens::menu::draw(f, area, app),
            Screen::Record => screens::record::draw(f, area, app),
            Screen::Process => screens::process::draw(f, area, app),
            Screen::Recent => screens::recent::draw(f, area, app),
            Screen::Phone => screens::phone::draw(f, area, app),
        }
    })?;
    Ok(())
}

/// Procesa mensajes pendientes del canal sin bloquear.
pub fn poll_messages(app: &mut App) {
    while let Ok(msg) = app.rx.try_recv() {
        match msg {
            UiMessage::RecordingFinished {
                path,
                duration_secs,
            } => {
                app.is_busy = false;
                // Mostrar solo el nombre del archivo, no la ruta completa.
                let filename = path
                    .file_name()
                    .and_then(|f| f.to_str())
                    .unwrap_or("audio.wav");
                app.status = format!(
                    "✓ Grabado {} ({:.0}s) — procesando...",
                    filename,
                    duration_secs
                );
                let _ = app.tx.send(UiMessage::LlmProgress(
                    "Inicializando transcripción...".into(),
                ));
                app.process_after_recording(path);
            }
            UiMessage::RecordingCancelled => {
                app.is_busy = false;
                app.status = "✗ Grabación cancelada".into();
            }
            UiMessage::LlmProgress(s) => {
                app.status = s;
            }
            UiMessage::ProcessingFinished { note_path } => {
                app.is_busy = false;
                // Mostrar nombre del archivo, no ruta completa.
                let filename = note_path
                    .file_name()
                    .and_then(|f| f.to_str())
                    .unwrap_or("nota.md");
                app.status = format!("✓ Nota: {}", filename);
            }
            UiMessage::PhoneServerReady(state) => {
                app.phone_state = Some(state);
                app.status = "✓ Servidor del teléfono listo. Escaneá el QR.".into();
            }
            UiMessage::Error(e) => {
                app.is_busy = false;
                app.status = format!("✗ {}", e);
            }
        }
    }
}

impl App {
    /// Lanza el procesamiento como tarea async usando el pipeline cacheado.
    fn process_after_recording(&mut self, wav_path: PathBuf) {
        let pipeline = match self.pipeline() {
            Ok(p) => p,
            Err(e) => {
                self.status = format!("✗ Pipeline: {:#}", e);
                return;
            }
        };
        let materia = self.record_form.materia.clone();
        let tema = self.record_form.tema.clone();
        let date = self.record_form.date;
        let tags: Vec<String> = self
            .record_form
            .tags
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let tx = self.tx.clone();
        let progress_tx = self.tx.clone();

        tokio::spawn(async move {
            // Notificar progreso.
            let _ = progress_tx.send(UiMessage::LlmProgress(
                "Transcribiendo audio con Whisper...".into(),
            ));
            let res = pipeline
                .process_existing(&wav_path, &materia, &tema, date, &tags)
                .await;
            match res {
                Ok(processed) => {
                    let _ = tx.send(UiMessage::ProcessingFinished {
                        note_path: processed.note_path,
                    });
                }
                Err(e) => {
                    let _ = tx.send(UiMessage::Error(format!("{:#}", e)));
                }
            }
        });
    }
}

/// Lee un evento de crossterm de forma no bloqueante.
#[allow(dead_code)]
pub fn read_event() -> Result<Option<Event>> {
    if crossterm::event::poll(std::time::Duration::from_millis(100))? {
        Ok(Some(crossterm::event::read()?))
    } else {
        Ok(None)
    }
}

/// Copia un string al portapapeles del sistema.
///
/// Intenta en orden: arboard (cross-platform) → xclip → xsel → wl-copy.
/// Si ninguno funciona, devuelve un error con instrucciones.
pub fn copy_to_clipboard(text: &str) -> Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    // 1) arboard (funciona en X11 y Wayland sin herramientas externas).
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        if clipboard.set_text(text.to_string()).is_ok() {
            return Ok(());
        }
    }

    // 2) Fallback a herramientas CLI.
    let tools: &[(&str, &[&str])] = &[
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
        ("wl-copy", &[]),
    ];
    for (cmd, args) in tools {
        let result = Command::new(cmd)
            .args(*args)
            .stdin(Stdio::piped())
            .spawn();
        if let Ok(mut child) = result {
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = stdin.write_all(text.as_bytes());
            }
            if child.wait().map(|s| s.success()).unwrap_or(false) {
                return Ok(());
            }
        }
    }

    anyhow::bail!(
        "no se encontró un portapapeles. Instalá xclip, xsel, wl-copy o arboard"
    )
}

/// Intenta dejar la URL visible incluso si el portapapeles falla.
/// En lugar de manipular el alternate screen (que rompe el estado de
/// ratatui y puede dejar el terminal en raw mode), simplemente loguea
/// la URL y la deja en el status de la TUI. El usuario puede copiar
/// desde el status o reintentar con `c`.
fn print_to_scrollback(text: &str) {
    // No tocar EnterAlternateScreen/LeaveAlternateScreen aquí: ratatui
    // gestiona el alternate screen en runner.rs y cualquier
    // manipulación manual aquí deja el terminal en estado inconsistente
    // (raw mode desincronizado, cursor perdido, etc.).
    // En su lugar, logueamos para que quede en `RUST_LOG` y dejamos
    // que el caller actualice `app.status` con la URL completa.
    tracing::info!("URL del teléfono (copiable): {}", text);
    // También intentamos escribir a stderr de forma no destructiva;
    // si el terminal está en alternate screen no será visible hasta
    // salir de la TUI, pero al salir quedará en el scrollback.
    eprintln!("\nclase-notes URL: {}\n", text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_to_clipboard_fails_gracefully() {
        // No podemos testear el éxito real sin un display server,
        // pero verificamos que la función existe y compila.
        let _ = copy_to_clipboard("test");
    }
}
