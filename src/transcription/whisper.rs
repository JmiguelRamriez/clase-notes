//! Wrapper sobre `whisper-rs` que transcribe archivos WAV completos.
//!
//! Estrategia: cargar el modelo una vez, leer el WAV con `hound`,
//! convertir las muestras a `f32` en rango [-1, 1], y ejecutar
//! `full()` para obtener todos los segmentos.
//! Para audios largos (> chunk_secs) hace chunked con overlap 1s:
//! evita OOM en VRAM 4GB (RTX 3050) y permite progreso/cancel ación
//! entre chunks. En CPU también evita 1 único buffer de 57M samples.

use anyhow::{Context, Result};
use hound::WavReader;
use std::path::Path;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::config::WhisperConfig;

/// Transcriptor Whisper. El contexto es pesado; se reutiliza entre
/// llamadas dentro de un mismo proceso.
pub struct WhisperTranscriber {
    ctx: WhisperContext,
    language: String,
    use_gpu: bool,
    chunk_secs: u32,
}

impl WhisperTranscriber {
    pub fn new(cfg: &WhisperConfig) -> Result<Self> {
        if !cfg.model_path.exists() {
            anyhow::bail!(
                "modelo Whisper no encontrado en {}; descárgalo con \
                `clase-notes download-model` o desde https://huggingface.co/ggerganov/whisper.cpp",
                cfg.model_path.display()
            );
        }
        let mut params = WhisperContextParameters::default();
        // RTX 3050 4GB: si use_gpu=true y el binario se compilo con --features cuda,
        // whisper.cpp usará CUDA; si no, cae a CPU sin error.
        params.use_gpu(cfg.use_gpu);
        if cfg.use_gpu {
            tracing::info!("Whisper GPU habilitado (use_gpu=true)");
        }
        let ctx = WhisperContext::new_with_params(
            cfg.model_path.to_str().context("path de modelo no es UTF-8")?,
            params,
        )
        .with_context(|| {
            format!(
                "cargando modelo Whisper desde {}",
                cfg.model_path.display()
            )
        })?;
        Ok(Self {
            ctx,
            language: cfg.language.clone(),
            use_gpu: cfg.use_gpu,
            chunk_secs: cfg.chunk_secs,
        })
    }

    /// Transcribe un archivo WAV (16 kHz mono PCM 16-bit) a texto plano.
    /// Si el WAV es multi-canal, hace downmix a mono promediando canales.
    pub fn transcribe_file(&self, wav_path: &Path) -> Result<String> {
        self.transcribe_file_with_progress(wav_path, None)
    }

    /// Variante con progreso por chunk para la TUI: cada chunk envía un String al channel.
    pub fn transcribe_file_with_progress(
        &self,
        wav_path: &Path,
        progress_tx: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<String> {
        let mut reader = WavReader::open(wav_path)
            .with_context(|| format!("abriendo {}", wav_path.display()))?;
        let spec = reader.spec();

        // Convertir samples a f32 normalizado.
        let mut raw_samples: Vec<f32> = match (spec.bits_per_sample, spec.sample_format) {
            (16, hound::SampleFormat::Int) => reader
                .samples::<i16>()
                .map(|s| s.map(|v| v as f32 / i16::MAX as f32))
                .collect::<Result<Vec<_>, _>>()?,
            (32, hound::SampleFormat::Float) => reader
                .samples::<f32>()
                .map(|s| s.map(|v| v as f32))
                .collect::<Result<Vec<_>, _>>()?,
            _ => anyhow::bail!(
                "WAV con formato no soportado: {}-bit {:?}",
                spec.bits_per_sample,
                spec.sample_format
            ),
        };

        // Fallback para WAV con header corrupto (hound devuelve 0 samples pero el archivo tiene datos)
        // Esto pasó con un WAV de 62MB con header max-size sin finalizar.
        if raw_samples.is_empty() {
            if let Ok(meta) = std::fs::metadata(wav_path) {
                if meta.len() > 44 {
                    tracing::warn!(
                        "header WAV corrupto (0 samples) pero archivo tiene {} bytes, intentando lectura raw",
                        meta.len()
                    );
                    if let Ok(bytes) = std::fs::read(wav_path) {
                        if bytes.len() > 44 {
                            let pcm_bytes = &bytes[44..];
                            let mut fallback = Vec::with_capacity(pcm_bytes.len() / 2);
                            for chunk in pcm_bytes.chunks_exact(2) {
                                let s = i16::from_le_bytes([chunk[0], chunk[1]]);
                                fallback.push(s as f32 / i16::MAX as f32);
                            }
                            if !fallback.is_empty() {
                                tracing::info!("fallback raw leyó {} samples", fallback.len());
                                raw_samples = fallback;
                            }
                        }
                    }
                }
            }
        }

        // Downmix multi-canal a mono (promediando canales).
        let channels = spec.channels as usize;
        let samples: Vec<f32> = if channels == 1 {
            raw_samples
        } else {
            tracing::info!("downmix de {} canales a mono", channels);
            raw_samples
                .chunks(channels)
                .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                .collect()
        };

        // Duración real después del downmix.
        let duration_secs = samples.len() as f32 / spec.sample_rate as f32;
        tracing::debug!(
            "transcribiendo {} muestras ({:.1}s, {} Hz, {} ch → mono, gpu={}, chunk={}s)",
            samples.len(),
            duration_secs,
            spec.sample_rate,
            spec.channels,
            self.use_gpu,
            self.chunk_secs
        );

        // Si el WAV no es 16 kHz, hacemos un resample simple (linear) para
        // cumplir con el requisito de Whisper. (Whisper hace resample
        // internamente, pero lo hacemos explícito para mayor fidelidad.)
        let samples = if spec.sample_rate != 16000 {
            resample_linear(&samples, spec.sample_rate, 16000)
        } else {
            samples
        };

        // Decidir chunked vs single
        let chunk_secs = if self.chunk_secs == 0 { 0 } else { self.chunk_secs };
        if chunk_secs == 0 || samples.len() <= (chunk_secs as usize * 16000) {
            // Audio corto: single full (rápido, sin overhead)
            return self.transcribe_samples(&samples);
        }

        // Audio largo: chunked con overlap 1s
        self.transcribe_chunked(&samples, chunk_secs, progress_tx)
    }

    fn transcribe_samples(&self, samples: &[f32]) -> Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        // Skip chunk silencioso (VAD simple RMS)
        if is_silent(samples) {
            tracing::debug!("chunk silencioso, se omite Whisper");
            return Ok(String::new());
        }
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(&self.language));
        params.set_print_realtime(false);
        params.set_print_progress(false);
        params.set_print_timestamps(false);
        params.set_n_threads(num_cpus());
        // Para audios largos con música/ruido, deshabilitar fallback de temperatura/logprob
        // que hace que Whisper reintente con temp 0.2 y tarde mucho (visto en 32min con música)
        params.set_temperature(0.0);
        params.set_temperature_inc(0.0);
        params.set_logprob_thold(-10.0); // no fallar por avg_logprobs < -1.0
        params.set_no_speech_thold(0.6); // skip música/silencio más rápido

        let mut state = self.ctx.create_state().context("creando estado Whisper")?;
        state
            .full(params, samples)
            .context("ejecutando Whisper full()")?;

        let num_segments = state.full_n_segments().context("contando segmentos")?;
        let mut text = String::new();
        for i in 0..num_segments {
            // Usar lossy para no abortar 1h por un byte inválido (ruido/música)
            let segment = match state.full_get_segment_text(i) {
                Ok(s) => s,
                Err(e) if e.to_string().contains("Invalid UTF-8") => {
                    tracing::warn!("segmento {} UTF-8 inválido, usando lossy: {}", i, e);
                    state
                        .full_get_segment_text_lossy(i)
                        .context("leyendo segmento lossy")?
                }
                Err(e) => return Err(e).context("leyendo segmento"),
            };
            let segment = segment.trim();
            if !segment.is_empty() {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(segment);
            }
        }
        Ok(text)
    }

    fn transcribe_chunked(
        &self,
        samples: &[f32],
        chunk_secs: u32,
        progress_tx: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<String> {
        let chunk_len = chunk_secs as usize * 16000;
        let overlap = 16000; // 1s
        let stride = chunk_len.saturating_sub(overlap).max(1);
        let total_chunks = (samples.len() + stride - 1) / stride;
        tracing::debug!(
            "chunked: {} chunks de {}s (overlap 1s), total {:.1}s",
            total_chunks,
            chunk_secs,
            samples.len() as f32 / 16000.0
        );
        // Primer progreso para TUI
        if let Some(tx) = &progress_tx {
            let _ = tx.send(format!(
                "Whisper chunk 0/{} — iniciando ({}s total)",
                total_chunks,
                (samples.len() as f32 / 16000.0).round() as u32
            ));
        }
        let mut full_text = String::new();
        let mut chunk_idx = 0usize;
        let mut start = 0usize;
        while start < samples.len() {
            let end = (start + chunk_len).min(samples.len());
            let chunk = &samples[start..end];
            chunk_idx += 1;
            let from = start as f32 / 16000.0;
            let to = end as f32 / 16000.0;
            tracing::debug!(
                "  chunk {}/{} [{:.1}s - {:.1}s] {} samples",
                chunk_idx,
                total_chunks,
                from,
                to,
                chunk.len()
            );
            // Enviar progreso a TUI si hay channel
            if let Some(tx) = &progress_tx {
                let _ = tx.send(format!(
                    "Whisper {}/{} [{:.0}s-{:.0}s] transcribiendo...",
                    chunk_idx, total_chunks, from, to
                ));
            }
            // Fast path para música: si los primeros 5s son "[Música]", asumir todo el chunk es música
            // Evita transcribir 30s de música que Whisper tarda mucho en intentar decodificar
            let mut txt = if chunk.len() > 80000 {
                let preview = &chunk[..80000.min(chunk.len())];
                // Solo si el preview no es silencio (RMS > 0.003)
                if !is_silent(preview) {
                    if let Ok(preview_txt) = self.transcribe_samples(preview) {
                        if preview_txt.to_lowercase().contains("música") {
                            tracing::debug!("chunk {}/{} detectado como música, se omite resto", chunk_idx, total_chunks);
                            if let Some(tx) = &progress_tx {
                                let _ = tx.send(format!("Whisper {}/{} — música detectada, saltando", chunk_idx, total_chunks));
                            }
                            "[Música]".to_string()
                        } else {
                            self.transcribe_samples(chunk)?
                        }
                    } else {
                        self.transcribe_samples(chunk)?
                    }
                } else {
                    self.transcribe_samples(chunk)?
                }
            } else {
                self.transcribe_samples(chunk)?
            };
            // Dedup overlap: si el chunk anterior termina igual que el prefijo del nuevo,
            // recortar duplicado (heurística simple de 3 palabras)
            if !full_text.is_empty() && !txt.is_empty() {
                txt = dedup_overlap(&full_text, &txt);
            }
            if !txt.is_empty() {
                if !full_text.is_empty() {
                    full_text.push(' ');
                }
                full_text.push_str(&txt);
            }
            if end >= samples.len() {
                break;
            }
            start += stride;
        }
        Ok(full_text)
    }
}

fn num_cpus() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as i32)
        .unwrap_or(4)
}

fn is_silent(samples: &[f32]) -> bool {
    if samples.is_empty() {
        return true;
    }
    // RMS < -50dB ~ 0.003
    let rms = compute_rms(samples);
    rms < 0.003
}

fn compute_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

/// Resample lineal simple. Suficiente para audio de voz donde se busca
/// mantener coherencia temporal.
fn resample_linear(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || input.is_empty() {
        return input.to_vec();
    }
    let ratio = to_rate as f64 / from_rate as f64;
    let new_len = (input.len() as f64 * ratio) as usize;
    let mut out = Vec::with_capacity(new_len);
    for i in 0..new_len {
        let src_idx = i as f64 / ratio;
        let i0 = src_idx.floor() as usize;
        let i1 = (i0 + 1).min(input.len() - 1);
        let frac = src_idx - i0 as f64;
        let v = input[i0] as f64 * (1.0 - frac) + input[i1] as f64 * frac;
        out.push(v as f32);
    }
    out
}

/// Quita prefijo duplicado por overlap: si los últimos N palabras de `prev`
/// coinciden con las primeras N palabras de `next`, recorta.
fn dedup_overlap(prev: &str, next: &str) -> String {
    let prev_words: Vec<&str> = prev.split_whitespace().collect();
    let next_words: Vec<&str> = next.split_whitespace().collect();
    if prev_words.len() < 3 || next_words.len() < 3 {
        return next.to_string();
    }
    // Probar con 3, 4, 5 palabras de overlap
    for n in (3..=5).rev() {
        if n > prev_words.len() || n > next_words.len() {
            continue;
        }
        let tail = &prev_words[prev_words.len() - n..];
        let head = &next_words[..n];
        if tail == head {
            return next_words[n..].join(" ");
        }
    }
    next.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_linear_identity_when_equal() {
        let v = vec![0.1, 0.2, 0.3, 0.4];
        let r = resample_linear(&v, 16000, 16000);
        assert_eq!(r.len(), 4);
    }

    #[test]
    fn resample_linear_doubles_length() {
        let v = vec![0.0; 100];
        let r = resample_linear(&v, 16000, 32000);
        assert!((r.len() as i32 - 200).abs() <= 1);
    }

    #[test]
    fn resample_linear_handles_empty() {
        let r = resample_linear(&[], 16000, 8000);
        assert!(r.is_empty());
    }

    #[test]
    fn dedup_overlap_trims_duplicate() {
        let prev = "hola mundo esta es una prueba";
        let next = "es una prueba que sigue";
        assert_eq!(dedup_overlap(prev, next), "que sigue");
    }

    #[test]
    fn dedup_overlap_no_false_positive() {
        let prev = "hola mundo";
        let next = "otra cosa distinta";
        assert_eq!(dedup_overlap(prev, next), "otra cosa distinta");
    }

    #[test]
    fn is_silent_detects_silence() {
        let silent = vec![0.0; 16000];
        assert!(is_silent(&silent));
        let loud = vec![0.5; 16000];
        assert!(!is_silent(&loud));
    }
}
