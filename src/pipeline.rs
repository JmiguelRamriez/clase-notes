//! Pipeline de procesamiento: WAV → transcripción → LLM → nota → bóveda.

use anyhow::{Context, Result};
use chrono::NaiveDate;
use std::path::Path;
use std::sync::Arc;
use tracing::warn;

use crate::audio::AudioRecorder;
use crate::config::Config;
use crate::llm::{LlmClient, SummaryOutput};
use crate::notes::NoteBuilder;
use crate::obsidian::ObsidianVault;
use crate::transcription::WhisperTranscriber;

/// Manejadores pesados compartidos. El contexto de Whisper se carga
/// una sola vez y se reusa entre clases.
pub struct Pipeline {
    pub config: Config,
    pub transcriber: Arc<WhisperTranscriber>,
    pub llm: LlmClient,
    pub vault: ObsidianVault,
}

impl Pipeline {
    pub fn new(config: Config) -> Result<Self> {
        let transcriber =
            Arc::new(WhisperTranscriber::new(&config.whisper).context("cargando Whisper")?);
        let llm = LlmClient::new(config.llm.clone()).context("creando cliente LLM")?;
        let vault = ObsidianVault::new(&config.obsidian);
        Ok(Self {
            config,
            transcriber,
            llm,
            vault,
        })
    }

    /// Graba audio del micrófono y devuelve el archivo WAV.
    pub async fn record(
        &self,
        output_path: &Path,
    ) -> Result<crate::audio::RecordingResult> {
        let rec = AudioRecorder::new(self.config.audio.sample_rate);
        let path = output_path.to_path_buf();
        // cpal es bloqueante; lo movemos a un hilo dedicado.
        let result = tokio::task::spawn_blocking(move || rec.record(&path))
            .await
            .context("hilo de grabación se cayó")??;
        Ok(result)
    }

    /// Procesa un WAV existente: transcribe + limpia + resume + guarda.
    pub async fn process_existing(
        &self,
        wav_path: &Path,
        materia: &str,
        tema: &str,
        date: NaiveDate,
        tags: &[String],
    ) -> Result<ProcessedClass> {
        self.process_existing_with_progress(wav_path, materia, tema, date, tags, None)
            .await
    }

    /// Variante con progreso para la TUI (envía Strings que se muestran como LlmProgress).
    pub async fn process_existing_with_progress(
        &self,
        wav_path: &Path,
        materia: &str,
        tema: &str,
        date: NaiveDate,
        tags: &[String],
        progress_tx: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<ProcessedClass> {
        if let Some(tx) = &progress_tx {
            let _ = tx.send(format!("Iniciando transcripción: {}", wav_path.display()));
        }
        tracing::debug!("transcribiendo {}", wav_path.display());
        // Whisper es bloqueante (CPU/GPU), lo movemos a spawn_blocking para no bloquear Tokio
        // y permitir progreso por chunk via channel.
        let transcriber = Arc::clone(&self.transcriber);
        let path = wav_path.to_path_buf();
        let pt = progress_tx.clone();
        let raw = tokio::task::spawn_blocking(move || {
            transcriber.transcribe_file_with_progress(&path, pt)
        })
        .await
        .context("hilo de transcripción se cayó")??;
        tracing::debug!("transcripción cruda: {} chars", raw.len());
        if let Some(tx) = &progress_tx {
            let _ = tx.send(format!("Transcripción lista ({} chars) — limpiando con LLM...", raw.len()));
        }

        if raw.trim().is_empty() {
            warn!("la transcripción quedó vacía; el LLM puede fallar");
        }

        if let Some(tx) = &progress_tx {
            let _ = tx.send("Limpiando transcripción con LLM...".into());
        }
        tracing::debug!("limpiando transcripción con LLM");
        let clean = self.llm.clean_text(&raw).await?;
        tracing::debug!("texto limpio: {} chars", clean.len());
        if let Some(tx) = &progress_tx {
            let _ = tx.send(format!("Texto limpio ({} chars) — generando resumen...", clean.len()));
        }

        tracing::debug!("generando resumen estructurado con LLM");
        let summary = self.llm.summarize(&clean).await?;
        tracing::debug!(
            "resumen: {} puntos, {} conceptos, {} tareas",
            summary.puntos_clave.len(),
            summary.conceptos.len(),
            summary.tareas.len()
        );

        // Construir nota.
        let audio_link = wav_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("audio.wav");
        let input = crate::notes::NoteInput {
            materia,
            tema,
            date,
            duration_secs: wav_duration_secs(wav_path).unwrap_or(0.0),
            transcript: &clean,
            summary: &summary,
            audio_link,
            tags,
        };
        let note = NoteBuilder::default().build(&input)?;

        // Copiar audio a attachments.
        self.vault.ensure()?;
        let audio_name = self.vault.copy_audio(materia, wav_path)?;

        // Reemplazar el link en la nota por el nombre final.
        let note = note.replace(
            &format!("[[{}]]", audio_link),
            &format!("[[{}]]", audio_name),
        );

        // Escribir nota.
        let written = self.vault.write_note(materia, tema, date, &note)?;
        tracing::debug!("nota escrita en {}", written.display());

        // Actualizar MOC de la materia.
        let moc_path = self.vault.update_moc(materia)?;
        tracing::debug!("MOC actualizado en {}", moc_path.display());

        Ok(ProcessedClass {
            note_path: written,
            audio_name,
            moc_path,
            summary,
            clean_text: clean,
            raw_text: raw,
        })
    }
}

#[derive(Debug)]
pub struct ProcessedClass {
    pub note_path: std::path::PathBuf,
    pub audio_name: String,
    pub moc_path: std::path::PathBuf,
    #[allow(dead_code)]
    pub summary: SummaryOutput,
    #[allow(dead_code)]
    pub clean_text: String,
    #[allow(dead_code)]
    pub raw_text: String,
}

/// Lee la duración de un WAV leyendo su spec y los samples.
fn wav_duration_secs(path: &Path) -> Option<f32> {
    let reader = hound::WavReader::open(path).ok()?;
    let spec = reader.spec();
    let n = reader.duration(); // total samples
    Some(n as f32 / spec.sample_rate as f32)
}
