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
            // Para audio vacío/silencio, no llamar al LLM (ahorra 17s) y crear nota con aviso
            let summary = crate::llm::SummaryOutput {
                resumen: "Audio vacío o silencio; no hay contenido para resumir.".into(),
                puntos_clave: vec![],
                conceptos: vec![],
                tareas: vec![],
            };
            let audio_link = wav_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("audio.wav");
            let input = crate::notes::NoteInput {
                materia,
                tema,
                date,
                duration_secs: wav_duration_secs(wav_path).unwrap_or(0.0),
                transcript: "",
                summary: &summary,
                audio_link,
                tags,
            };
            let note = NoteBuilder::default().build(&input)?;
            // Vault ops resilientes para no perder nota aunque falle copia/MOC
            let audio_name = match self.vault.copy_audio(materia, wav_path) {
                Ok(n) => n,
                Err(e) => {
                    warn!("copy_audio falló en vacío: {}, usando link original", e);
                    audio_link.to_string()
                }
            };
            let note = note.replace(
                &format!("[[{}]]", audio_link),
                &format!("[[{}]]", audio_name),
            );
            let written = self.vault.write_note(materia, tema, date, &note)?;
            let moc_path = match self.vault.update_moc(materia) {
                Ok(p) => p,
                Err(e) => {
                    warn!("update_moc falló en vacío: {}", e);
                    self.vault.materia_dir(materia)?.join("_MOC.md")
                }
            };
            return Ok(ProcessedClass {
                note_path: written,
                audio_name,
                moc_path,
                summary,
                clean_text: String::new(),
                raw_text: raw,
            });
        }
        // Si la transcripción es solo música/ruido, LLM puede devolver un rechazo; lo detectamos
        // Umbral 100 palabras para considerar "solo música" incluso en audios de 1-2 min con hallucination
        let is_music_only = raw.to_lowercase().contains("música")
            && raw.split_whitespace().count() < 100;

        if let Some(tx) = &progress_tx {
            let _ = tx.send("Limpiando transcripción con LLM...".into());
        }
        tracing::debug!("limpiando transcripción con LLM");
        let clean_raw = match self.llm.clean_text(&raw).await {
            Ok(c) => c,
            Err(e) => {
                warn!("LLM clean_text falló ({}), usando transcripción cruda", e);
                if let Some(tx) = &progress_tx {
                    let _ = tx.send(format!("LLM limpiando falló, usando crudo: {}", e));
                }
                raw.clone()
            }
        };
        // Detectar rechazo del LLM (ej. qwen2.5-coder con música: "Lo siento, pero no puedo...")
        let is_refusal = clean_raw.to_lowercase().contains("lo siento")
            || clean_raw.to_lowercase().contains("no puedo procesar")
            || clean_raw.to_lowercase().contains("proporcionar la transcripción");
        let clean = if is_refusal || clean_raw.trim().is_empty() {
            if is_refusal {
                warn!("LLM devolvió rechazo para música/ruido, usando transcripción cruda");
            }
            if is_music_only {
                // Para audio solo con música, no intentar resumir con LLM (será vacío)
                raw.clone()
            } else if clean_raw.trim().is_empty() {
                raw.clone()
            } else {
                clean_raw
            }
        } else {
            clean_raw
        };
        tracing::debug!("texto limpio: {} chars", clean.len());
        if let Some(tx) = &progress_tx {
            let _ = tx.send(format!("Texto limpio ({} chars) — generando resumen...", clean.len()));
        }

        tracing::debug!("generando resumen estructurado con LLM");
        // Si el texto limpio sigue siendo solo música, no pedir resumen (evita _Sin resumen)
        let summary = if is_music_only && clean.to_lowercase().contains("música") {
            warn!("transcripción solo música, se omite resumen LLM");
            crate::llm::SummaryOutput {
                resumen: "Audio contiene principalmente música o silencio; no hay contenido hablado para resumir.".into(),
                puntos_clave: vec![],
                conceptos: vec![],
                tareas: vec![],
            }
        } else {
            match self.llm.summarize(&clean).await {
                Ok(s) => s,
                Err(e) => {
                    warn!("LLM summarize falló ({}), usando resumen fallback", e);
                    if let Some(tx) = &progress_tx {
                        let _ = tx.send(format!("LLM resumen falló: {}", e));
                    }
                    crate::llm::SummaryOutput {
                        resumen: clean.chars().take(500).collect::<String>(),
                        puntos_clave: vec![],
                        conceptos: vec![],
                        tareas: vec![],
                    }
                }
            }
        };
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

        // Copiar audio a attachments (no fatal si falla, usamos nombre original)
        let audio_name = match self.vault.copy_audio(materia, wav_path) {
            Ok(n) => n,
            Err(e) => {
                warn!("copy_audio falló, usando link original: {}", e);
                if let Some(tx) = &progress_tx {
                    let _ = tx.send(format!("Aviso: no se pudo copiar audio: {}", e));
                }
                audio_link.to_string()
            }
        };

        // Reemplazar el link en la nota por el nombre final.
        let note = note.replace(
            &format!("[[{}]]", audio_link),
            &format!("[[{}]]", audio_name),
        );

        // Escribir nota (siempre intenta, si falla avisa pero no pierde transcripción)
        let written = match self.vault.write_note(materia, tema, date, &note) {
            Ok(p) => p,
            Err(e) => {
                warn!("write_note falló: {}", e);
                if let Some(tx) = &progress_tx {
                    let _ = tx.send(format!("Error escribiendo nota: {}", e));
                }
                return Err(e).context("escribiendo nota");
            }
        };
        tracing::debug!("nota escrita en {}", written.display());
        if let Some(tx) = &progress_tx {
            let _ = tx.send(format!("Nota escrita: {} — actualizando índice...", written.display()));
        }

        // Actualizar MOC de la materia (no fatal si falla)
        let moc_path = match self.vault.update_moc(materia) {
            Ok(p) => p,
            Err(e) => {
                warn!("update_moc falló: {}, ignorando", e);
                if let Some(tx) = &progress_tx {
                    let _ = tx.send(format!("Aviso MOC: {}", e));
                }
                self.vault.materia_dir(materia)?.join("_MOC.md")
            }
        };
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
/// Si el header está corrupto (duration 0 pero archivo tiene datos), estima desde tamaño de archivo.
fn wav_duration_secs(path: &Path) -> Option<f32> {
    if let Ok(reader) = hound::WavReader::open(path) {
        let spec = reader.spec();
        let n = reader.duration();
        if n > 0 && spec.sample_rate > 0 {
            return Some(n as f32 / spec.sample_rate as f32);
        }
    }
    // Fallback para header corrupto: estimar desde tamaño de archivo
    if let Ok(meta) = std::fs::metadata(path) {
        let size = meta.len().saturating_sub(44) as f32;
        if let Ok(r) = hound::WavReader::open(path) {
            let spec = r.spec();
            if spec.sample_rate > 0 && spec.channels > 0 && spec.bits_per_sample > 0 {
                let bytes_per_sec = spec.sample_rate as f32 * spec.channels as f32 * (spec.bits_per_sample as f32 / 8.0);
                if bytes_per_sec > 0.0 {
                    return Some(size / bytes_per_sec);
                }
            }
        }
        // último fallback 16kHz mono 16-bit = 32000 bytes/sec
        return Some(size / 32000.0);
    }
    None
}
