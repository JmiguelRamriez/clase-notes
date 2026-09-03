//! Bucle principal de la TUI.

use anyhow::Result;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use crate::audio::AudioRecorder;
use crate::config::work_dir;
use crate::tui::app::{poll_messages, App, AppAction, AudioSource, UiMessage};

pub async fn run(app: &mut App) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = run_loop(&mut terminal, app).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    res
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    let tick_rate = Duration::from_millis(100);

    loop {
        crate::tui::app::draw(terminal, app)?;
        poll_messages(app);

        if crossterm::event::poll(tick_rate)? {
            let ev = crossterm::event::read()?;
            if let crossterm::event::Event::Key(key) = ev {
                let action = app.handle_key(key)?;
                if matches!(action, AppAction::Quit) {
                    app.stop_flag
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    return Ok(());
                }
                if !app.is_busy {
                    match action {
                        AppAction::StartRecording => start_recording(app),
                        AppAction::StartProcessing => app.start_processing(),
                        _ => {}
                    }
                } else {
                    handle_busy_action(app, action);
                }
            }
        }
    }
}

/// Inicia la grabación en un hilo dedicado.
fn start_recording(app: &mut App) {
    let materia = app.record_form.materia.clone();
    let tema = app.record_form.tema.clone();
    let date = app.record_form.date;
    let audio_preset = app.audio_preset;
    let audio_source = app.audio_source;
    let work = match work_dir() {
        Ok(w) => w,
        Err(e) => {
            app.status = format!("✗ No se pudo crear work_dir: {}", e);
            return;
        }
    };
    let stamp = date.format("%Y-%m-%d").to_string();
    let path = work.join(format!("{}-{}-{}.wav", stamp, slug(&materia), slug(&tema)));
    let tx = app.tx.clone();
    let phone_state = app.phone_state.clone();

    // Resetear el flag de stop y configurar la grabación.
    app.stop_flag
        .store(false, std::sync::atomic::Ordering::SeqCst);
    app.is_busy = true;
    app.status = match audio_source {
        AudioSource::Microphone => "Iniciando grabación desde el micrófono...".into(),
        AudioSource::Phone => "Esperando audio del teléfono...".into(),
    };

    // Clonar el Arc del stop_flag para el hilo de grabación.
    let stop_flag = Arc::clone(&app.stop_flag);

    std::thread::spawn(move || {
        let res = match audio_source {
            AudioSource::Microphone => {
                let rec = AudioRecorder::new(16000);
                rec.record_with_stop(&path, stop_flag)
            }
            AudioSource::Phone => {
                // Grabar desde el WebSocket del teléfono.
                if let Some(state) = phone_state {
                    record_from_phone(&path, state, stop_flag)
                } else {
                    Err(anyhow::anyhow!(
                        "el servidor del teléfono no está activo. Andá al menú y elegí 'Conectar teléfono' primero."
                    ))
                }
            }
        };

        match res {
            Ok(r) => {
                // Aplicar filtro de audio al WAV grabado (post-procesamiento).
                if let Err(e) = crate::audio::apply_filter_to_wav(&r.wav_path, audio_preset) {
                    let _ = tx.send(UiMessage::Error(format!("filtro: {:#}", e)));
                    return;
                }
                let _ = tx.send(UiMessage::RecordingFinished {
                    path: r.wav_path,
                    duration_secs: r.duration_secs,
                });
            }
            Err(e) => {
                let _ = tx.send(UiMessage::Error(format!("grabación: {:#}", e)));
            }
        }
    });
}

/// Graba audio desde el WebSocket del teléfono a un archivo WAV.
fn record_from_phone(
    output_path: &std::path::Path,
    state: Arc<crate::audio::PhoneState>,
    stop_flag: Arc<AtomicBool>,
) -> anyhow::Result<crate::audio::RecordingResult> {
    use anyhow::Context;
    use hound::WavWriter;
    use std::time::Instant;

    use crate::config::create_wav_writer;

    let writer: WavWriter<std::io::BufWriter<std::fs::File>> =
        create_wav_writer(output_path, crate::audio::PHONE_SAMPLE_RATE, 1)?;
    let writer = Arc::new(std::sync::Mutex::new(Some(writer)));

    let start = Instant::now();

    // Loop bloqueante que lee frames del WebSocket y los escribe al WAV.
    // Se detiene cuando stop_flag sea true.
    let runtime = tokio::runtime::Handle::current();
    while !stop_flag.load(std::sync::atomic::Ordering::SeqCst) {
        // Intentar leer un frame del WebSocket (no bloqueante).
        let frame = runtime.block_on(async { state.try_read_frame().await });
        match frame {
            Some(samples) => {
                if let Ok(mut g) = writer.lock() {
                    if let Some(w) = g.as_mut() {
                        for s in samples {
                            let _ = w.write_sample(s);
                        }
                    }
                }
            }
            None => {
                // No hay datos todavía (cliente no conectado o esperando).
                // Si no hay cliente conectado, fallar con mensaje útil.
                if !state.is_connected() && start.elapsed().as_secs() > 5 {
                    anyhow::bail!(
                        "el teléfono no se conectó. Volvé al menú, conectá el iPhone, y reintentá."
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    }

    // Finalizar WAV.
    if let Ok(mut g) = writer.lock() {
        if let Some(w) = g.take() {
            w.finalize().context("finalizando WAV")?;
        }
    }

    let elapsed = start.elapsed().as_secs_f32();
    let sample_count = (elapsed * crate::audio::PHONE_SAMPLE_RATE as f32) as usize;
    Ok(crate::audio::RecordingResult {
        wav_path: output_path.to_path_buf(),
        duration_secs: elapsed,
        sample_count,
    })
}

fn handle_busy_action(app: &mut App, action: AppAction) {
    match action {
        AppAction::StopRecording | AppAction::CancelRecording => {
            // Esto dispara la señal de stop que el callback de cpal
            // está escuchando a través del Arc<AtomicBool> compartido.
            app.stop_flag
                .store(true, std::sync::atomic::Ordering::SeqCst);
            app.status = "Deteniendo grabación...".into();
            if let AppAction::CancelRecording = action {
                let _ = app.tx.send(UiMessage::RecordingCancelled);
            }
        }
        _ => {}
    }
}

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}
