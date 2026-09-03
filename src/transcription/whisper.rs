//! Wrapper sobre `whisper-rs` que transcribe archivos WAV completos.
//!
//! Estrategia: cargar el modelo una vez, leer el WAV con `hound`,
//! convertir las muestras a `f32` en rango [-1, 1], y ejecutar
//! `full()` para obtener todos los segmentos.

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
        params.use_gpu(false); // CPU por defecto; más portable.
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
        })
    }

    /// Transcribe un archivo WAV (16 kHz mono PCM 16-bit) a texto plano.
    /// Si el WAV es multi-canal, hace downmix a mono promediando canales.
    pub fn transcribe_file(&self, wav_path: &Path) -> Result<String> {
        let mut reader = WavReader::open(wav_path)
            .with_context(|| format!("abriendo {}", wav_path.display()))?;
        let spec = reader.spec();

        // Convertir samples a f32 normalizado.
        let raw_samples: Vec<f32> = match (spec.bits_per_sample, spec.sample_format) {
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
        tracing::info!(
            "transcribiendo {} muestras ({:.1}s, {} Hz, {} ch → mono)",
            samples.len(),
            duration_secs,
            spec.sample_rate,
            spec.channels
        );

        // Si el WAV no es 16 kHz, hacemos un resample simple (linear) para
        // cumplir con el requisito de Whisper. (Whisper hace resample
        // internamente, pero lo hacemos explícito para mayor fidelidad.)
        let samples = if spec.sample_rate != 16000 {
            resample_linear(&samples, spec.sample_rate, 16000)
        } else {
            samples
        };

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(&self.language));
        params.set_print_realtime(false);
        params.set_print_progress(false);
        params.set_print_timestamps(false);
        params.set_n_threads(num_cpus());

        let mut state = self.ctx.create_state().context("creando estado Whisper")?;
        state
            .full(params, &samples)
            .context("ejecutando Whisper full()")?;

        // Concatenar todos los segmentos.
        let num_segments = state.full_n_segments().context("contando segmentos")?;
        let mut text = String::new();
        for i in 0..num_segments {
            let segment = state
                .full_get_segment_text(i)
                .context("leyendo segmento")?;
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
}

fn num_cpus() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as i32)
        .unwrap_or(4)
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
}
