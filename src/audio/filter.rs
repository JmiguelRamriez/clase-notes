//! Filtros de audio aplicados en la cadena de captura.
//!
//! Hay tres presets seleccionables desde la TUI:
//!
//! - `Silencio`:   noise gate agresivo (-30 dB), normalizer fuerte.
//! - `Normal`:     noise gate moderado (-40 dB), normalizer suave.
//! - `SinFiltro`:  pasa audio crudo sin tocar nada.
//!
//! El navegador (en el caso de audio del teléfono) ya aplica
//! `echoCancellation`, `noiseSuppression` y `autoGainControl`. El
//! filtro de servidor es una segunda capa para ruidos que el browser
//! no alcanza a filtrar — voces fuertes de compañeros, golpes de
//! silla, etc.

#![allow(dead_code)]

/// Preset de filtrado de audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioPreset {
    /// Noise gate agresivo, normalizer fuerte. Para salones ruidosos.
    Silencio,
    /// Noise gate moderado, normalizer suave. Uso general.
    Normal,
    /// Sin procesamiento, audio crudo.
    SinFiltro,
}

impl AudioPreset {
    /// Parsea desde el nombre usado en config.toml.
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "silencio" | "quiet" | "ruidoso" | "noisy" => Self::Silencio,
            "normal" | "default" | "" => Self::Normal,
            "sin_filtro" | "sinfiltro" | "off" | "raw" | "none" => Self::SinFiltro,
            _ => Self::Normal,
        }
    }

    /// Nombre canónico para config.toml.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Silencio => "silencio",
            Self::Normal => "normal",
            Self::SinFiltro => "sin_filtro",
        }
    }

    /// Etiqueta corta para mostrar en la TUI.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Silencio => "Silencio",
            Self::Normal => "Normal",
            Self::SinFiltro => "Sin filtro",
        }
    }

    /// Siguiente preset en el ciclo (para rotar con Tab).
    pub fn next(&self) -> Self {
        match self {
            Self::Silencio => Self::Normal,
            Self::Normal => Self::SinFiltro,
            Self::SinFiltro => Self::Silencio,
        }
    }
}

/// Filtro de audio con estado. Aplica noise gate + RMS normalizer
/// a ventanas de samples.
///
/// Thread-safe internamente: usa estado por instancia, pero no es
/// `Send` para procesamiento concurrente — crear uno por stream.
pub struct AudioFilter {
    preset: AudioPreset,
    /// Umbral del noise gate en amplitud lineal (0.0..1.0).
    threshold: f32,
    /// RMS objetivo para el normalizer (0.0..1.0).
    target_rms: f32,
    /// Ganancia actual del normalizer (suavizada para evitar saltos).
    gain: f32,
    /// Tamaño de ventana para calcular RMS.
    window_size: usize,
    /// Buffer de la ventana actual.
    window: Vec<f32>,
}

impl AudioFilter {
    /// Crea un filtro con el preset dado.
    pub fn new(preset: AudioPreset) -> Self {
        let (threshold_db, target_rms) = match preset {
            AudioPreset::Silencio => (-30.0, 0.3),
            AudioPreset::Normal => (-40.0, 0.5),
            AudioPreset::SinFiltro => {
                // Umbral muy bajo = noise gate desactivado, target 1.0 = sin normalizer.
                (-120.0, 1.0)
            }
        };
        Self {
            preset,
            threshold: db_to_linear(threshold_db),
            target_rms,
            gain: 1.0,
            window_size: 1600, // 100ms @ 16kHz
            window: Vec::with_capacity(1600),
        }
    }

    /// Devuelve el preset actual.
    pub fn preset(&self) -> AudioPreset {
        self.preset
    }

    /// Procesa un buffer de samples in-place. Devuelve la cantidad
    /// de samples que quedaron en silencio (para métricas).
    pub fn process(&mut self, samples: &mut [f32]) -> usize {
        if matches!(self.preset, AudioPreset::SinFiltro) {
            return 0;
        }

        let mut silenced = 0;

        // Paso 1: noise gate por muestra.
        for s in samples.iter_mut() {
            if s.abs() < self.threshold {
                if *s != 0.0 {
                    *s = 0.0;
                    silenced += 1;
                }
            }
        }

        // Paso 2: normalizer por ventana (RMS-based con suavizado).
        for chunk in samples.chunks_mut(self.window_size) {
            let rms = compute_rms(chunk);
            if rms > 0.001 {
                let desired_gain = (self.target_rms / rms).clamp(0.1, 10.0);
                // Suavizar la ganancia para evitar saltos.
                self.gain = 0.7 * self.gain + 0.3 * desired_gain;
            }
            for s in chunk.iter_mut() {
                *s = (*s * self.gain).clamp(-1.0, 1.0);
            }
        }

        silenced
    }
}

/// Convierte dB a amplitud lineal.
fn db_to_linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Calcula el RMS de un buffer de samples.
fn compute_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

/// Aplica el filtro a un archivo WAV in-place (lee, filtra, reescribe).
/// Se usa como post-procesamiento después de la captura.
///
/// El WAV se reescribe con el mismo sample rate y canales (1).
pub fn apply_filter_to_wav(
    path: &std::path::Path,
    preset: AudioPreset,
) -> anyhow::Result<()> {
    use anyhow::Context;
    use hound::{SampleFormat, WavReader, WavSpec, WavWriter};

    if matches!(preset, AudioPreset::SinFiltro) {
        // No hace nada.
        return Ok(());
    }

    let mut reader = WavReader::open(path).context("abriendo WAV para filtrar")?;
    let spec = reader.spec();

    // Solo soportamos PCM integer 16-bit (lo que produce el recorder).
    if spec.bits_per_sample != 16 || spec.sample_format != SampleFormat::Int {
        anyhow::bail!(
            "filtro solo soporta WAV PCM 16-bit, got {:?} {} bits",
            spec.sample_format,
            spec.bits_per_sample
        );
    }

    // Leer todos los samples como f32 normalizado.
    let mut samples: Vec<f32> = reader
        .samples::<i16>()
        .map(|s| s.unwrap_or(0) as f32 / i16::MAX as f32)
        .collect();
    drop(reader);

    // Aplicar filtro.
    let mut filter = AudioFilter::new(preset);
    filter.process(&mut samples);

    // Reescribir el WAV.
    let new_spec = WavSpec {
        channels: spec.channels,
        sample_rate: spec.sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(path, new_spec).context("creando WAV filtrado")?;
    for s in &samples {
        let i = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        writer.write_sample(i).context("escribiendo sample")?;
    }
    writer.finalize().context("finalizando WAV filtrado")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_round_trip() {
        for p in [AudioPreset::Silencio, AudioPreset::Normal, AudioPreset::SinFiltro] {
            assert_eq!(AudioPreset::from_str(p.as_str()), p);
        }
    }

    #[test]
    fn preset_from_str_ignores_case() {
        assert_eq!(AudioPreset::from_str("SILENCIO"), AudioPreset::Silencio);
        assert_eq!(AudioPreset::from_str("Normal"), AudioPreset::Normal);
        assert_eq!(AudioPreset::from_str("SIN_FILTRO"), AudioPreset::SinFiltro);
    }

    #[test]
    fn preset_from_str_defaults_to_normal() {
        assert_eq!(AudioPreset::from_str("desconocido"), AudioPreset::Normal);
        assert_eq!(AudioPreset::from_str(""), AudioPreset::Normal);
    }

    #[test]
    fn preset_next_cycles() {
        assert_eq!(AudioPreset::Silencio.next(), AudioPreset::Normal);
        assert_eq!(AudioPreset::Normal.next(), AudioPreset::SinFiltro);
        assert_eq!(AudioPreset::SinFiltro.next(), AudioPreset::Silencio);
    }

    #[test]
    fn db_to_linear_works() {
        assert!((db_to_linear(0.0) - 1.0).abs() < 1e-6);
        assert!((db_to_linear(-20.0) - 0.1).abs() < 1e-3);
        assert!((db_to_linear(-40.0) - 0.01).abs() < 1e-4);
    }

    #[test]
    fn rms_of_silence_is_zero() {
        let samples = vec![0.0; 1000];
        assert_eq!(compute_rms(&samples), 0.0);
    }

    #[test]
    fn rms_of_constant_is_constant() {
        let samples = vec![0.5; 1000];
        assert!((compute_rms(&samples) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn noise_gate_silences_quiet_samples() {
        let mut filter = AudioFilter::new(AudioPreset::Silencio);
        // -30 dB = ~0.0316, así que samples < 0.03 deben silenciarse.
        let mut samples = vec![0.001, 0.002, 0.5, -0.4, 0.005];
        let silenced = filter.process(&mut samples);
        assert!(silenced > 0);
        assert_eq!(samples[0], 0.0);
        assert_eq!(samples[1], 0.0);
        // Los fuertes pasan el gate (pueden ser normalizados).
        assert!(samples[2].abs() > 0.0);
        assert!(samples[3].abs() > 0.0);
    }

    #[test]
    fn normalizer_boosts_quiet_signal() {
        let mut filter = AudioFilter::new(AudioPreset::Silencio);
        // Señal baja pero sobre el umbral: -30 dB ≈ 0.0316, usamos 0.05.
        let mut samples = vec![0.05; 3200];
        let _ = filter.process(&mut samples);
        // Después del normalizer, la señal debería ser más fuerte.
        let max_after = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(
            max_after > 0.05,
            "normalizer debería haber amplificado la señal (max: {})",
            max_after
        );
    }

    #[test]
    fn sin_filtro_passes_through_unchanged() {
        let mut filter = AudioFilter::new(AudioPreset::SinFiltro);
        let original = vec![0.001, 0.002, 0.5, -0.4, 0.005];
        let mut samples = original.clone();
        let silenced = filter.process(&mut samples);
        assert_eq!(silenced, 0);
        assert_eq!(samples, original, "sin_filtro no debe tocar el audio");
    }

    #[test]
    fn silences_are_preserved_through_clamp() {
        let mut filter = AudioFilter::new(AudioPreset::Silencio);
        // Audio muy fuerte: el normalizer debería bajarlo (clamp a 1.0).
        let mut samples = vec![2.0; 3200];
        let _ = filter.process(&mut samples);
        let max_after = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(max_after <= 1.0, "samples deben estar clampeados a [-1, 1]");
    }

    #[test]
    fn apply_filter_to_wav_sin_filtro_is_noop() {
        use hound::{SampleFormat, WavSpec, WavWriter};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.wav");
        let spec = WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut w = WavWriter::create(&path, spec).unwrap();
        for _ in 0..1000 {
            w.write_sample(100i16).unwrap();
        }
        w.finalize().unwrap();
        // sin_filtro no debe modificar el archivo.
        apply_filter_to_wav(&path, AudioPreset::SinFiltro).unwrap();
        // Verificar que el WAV sigue ahí y tiene datos.
        let r = hound::WavReader::open(&path).unwrap();
        assert_eq!(r.duration(), 1000);
    }

    #[test]
    fn apply_filter_to_wav_silences_quiet_samples() {
        use hound::{SampleFormat, WavSpec, WavWriter};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.wav");
        let spec = WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut w = WavWriter::create(&path, spec).unwrap();
        // Escribir 100 samples casi en silencio.
        for _ in 0..100 {
            w.write_sample(10i16).unwrap();
        }
        w.finalize().unwrap();
        // Silencio debería matarlos.
        apply_filter_to_wav(&path, AudioPreset::Silencio).unwrap();
        let r = hound::WavReader::open(&path).unwrap();
        let samples: Vec<i16> = r.into_samples().map(|s| s.unwrap()).collect();
        // Todos deberían ser 0 (10 < 30 dB threshold).
        assert!(
            samples.iter().all(|&s| s == 0),
            "todos los samples deberían estar silenciados"
        );
    }
}
