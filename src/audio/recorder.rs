//! Grabador de audio con `cpal`. Captura del micrófono por defecto,
//! convierte a i16 PCM mono y escribe a un WAV en disco.
//!
//! Para habilitar la captura de audio, compila con
//! `--features audio-capture`. Sin esa feature, el módulo de audio
//! solo está disponible vía la pipeline (que recibe WAVs ya grabados
//! por otra herramienta).

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct RecordingResult {
    pub wav_path: PathBuf,
    #[allow(dead_code)]
    pub duration_secs: f32,
    #[allow(dead_code)]
    pub sample_count: usize,
}

/// Estado del grabador. Usa `AtomicBool` para señalizar pausa/detener
/// de forma segura entre el hilo de UI y el callback de audio.
pub struct AudioRecorder {
    #[allow(dead_code)]
    sample_rate: u32,
    is_paused: Arc<AtomicBool>,
    is_stopped: Arc<AtomicBool>,
    #[cfg(feature = "audio-capture")]
    inner: RealRecorder,
}

impl AudioRecorder {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            is_paused: Arc::new(AtomicBool::new(false)),
            is_stopped: Arc::new(AtomicBool::new(false)),
            #[cfg(feature = "audio-capture")]
            inner: RealRecorder::new(),
        }
    }

    #[allow(dead_code)]
    pub fn pause(&self) {
        self.is_paused.store(true, Ordering::SeqCst);
    }

    #[allow(dead_code)]
    pub fn resume(&self) {
        self.is_paused.store(false, Ordering::SeqCst);
    }

    #[allow(dead_code)]
    pub fn stop(&self) {
        self.is_stopped.store(true, Ordering::SeqCst);
    }

    /// Inicia la grabación. Bloquea hasta que `is_stopped` sea `true`.
    #[cfg(feature = "audio-capture")]
    #[allow(dead_code)]
    pub fn record(&self, output_path: &Path) -> Result<RecordingResult> {
        self.inner.record(
            output_path,
            self.sample_rate,
            &self.is_paused,
            &self.is_stopped,
        )
    }

    /// Variante para TUI: usa un `Arc<AtomicBool>` externo como flag de stop.
    /// **Clona el Arc** (no el valor) para que la señal de stop se propague
    /// correctamente al callback de cpal.
    #[cfg(feature = "audio-capture")]
    pub fn record_with_stop(
        &self,
        output_path: &Path,
        stop: Arc<AtomicBool>,
    ) -> Result<RecordingResult> {
        self.inner
            .record(output_path, self.sample_rate, &self.is_paused, &stop)
    }

    /// Sin la feature audio-capture, estos métodos devuelven error.
    #[cfg(not(feature = "audio-capture"))]
    #[allow(dead_code)]
    pub fn record(&self, _output_path: &Path) -> Result<RecordingResult> {
        anyhow::bail!(
            "este binario se compiló sin la feature 'audio-capture'.\n\
             Recompila con: cargo build --release --features audio-capture"
        )
    }

    #[cfg(not(feature = "audio-capture"))]
    pub fn record_with_stop(
        &self,
        _output_path: &Path,
        _stop: Arc<AtomicBool>,
    ) -> Result<RecordingResult> {
        anyhow::bail!(
            "este binario se compiló sin la feature 'audio-capture'.\n\
             Recompila con: cargo build --release --features audio-capture"
        )
    }
}

// ============================================================================
// Backend real (cpal)
// ============================================================================

#[cfg(feature = "audio-capture")]
struct RealRecorder;

#[cfg(feature = "audio-capture")]
impl RealRecorder {
    fn new() -> Self {
        Self
    }

    /// Graba audio del micrófono. Bloquea hasta que `is_stopped` sea `true`.
    ///
    /// **Importante:** `is_stopped` debe ser un `Arc<AtomicBool>` compartido
    /// (no una copia del valor) para que la señal de stop se propague desde
    /// el hilo de la TUI hasta el callback de cpal.
    fn record(
        &self,
        output_path: &std::path::Path,
        sample_rate: u32,
        is_paused: &Arc<AtomicBool>,
        is_stopped: &Arc<AtomicBool>,
    ) -> Result<RecordingResult> {
        use anyhow::Context;
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        use cpal::{SampleFormat, StreamConfig};
        use hound::WavWriter;
        use std::io::BufWriter;
        use std::sync::Mutex;
        use std::time::Instant;

        use crate::config::create_wav_writer;

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .context("no hay dispositivo de entrada de audio disponible")?;
        let device_name = device.name().unwrap_or_else(|_| "desconocido".into());
        tracing::info!("dispositivo de audio: {}", device_name);

        let writer: WavWriter<BufWriter<std::fs::File>> =
            create_wav_writer(output_path, sample_rate, 1)?;
        let writer = Arc::new(Mutex::new(Some(writer)));

        let supported = device
            .supported_input_configs()
            .context("no se pudieron obtener configs soportadas")?;

        let mut chosen = None;
        for cfg in supported {
            if cfg.channels() == 1
                && cfg.min_sample_rate().0 <= sample_rate
                && cfg.max_sample_rate().0 >= sample_rate
            {
                chosen = Some(cfg.with_sample_rate(cpal::SampleRate(sample_rate)));
                break;
            }
        }
        let supported_cfg = if let Some(c) = chosen {
            c
        } else {
            let mut any = None;
            for cfg in device.supported_input_configs()? {
                if cfg.min_sample_rate().0 <= sample_rate
                    && cfg.max_sample_rate().0 >= sample_rate
                {
                    any = Some(cfg.with_sample_rate(cpal::SampleRate(sample_rate)));
                    break;
                }
            }
            any.context(format!(
                "ningún config de entrada soporta sample_rate={}",
                sample_rate
            ))?
        };

        let cfg_channels = supported_cfg.channels();
        let stream_config = StreamConfig {
            channels: cfg_channels,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        // Flag para evitar que el callback de error de cpal se queje
        // durante el drop del stream (comportamiento normal de ALSA).
        let is_shutting_down = Arc::new(AtomicBool::new(false));
        let err_flag = Arc::clone(&is_shutting_down);
        let err_fn = move |err| {
            // Solo loguear errores que NO sean durante el shutdown normal.
            if !err_flag.load(Ordering::SeqCst) {
                tracing::warn!("error en stream de audio: {}", err);
            }
        };

        let is_paused_cb = Arc::clone(is_paused);
        let is_stopped_cb = Arc::clone(is_stopped);
        let mono_mode = cfg_channels == 1;

        tracing::debug!(
            "iniciando stream: {} canales, {} Hz, formato {:?}, {}",
            cfg_channels,
            sample_rate,
            supported_cfg.sample_format(),
            if mono_mode { "mono" } else { "multi-canal (downmix a mono)" }
        );

        let p = Arc::clone(&is_paused_cb);
        let s = Arc::clone(&is_stopped_cb);
        let w = Arc::clone(&writer);
        let ch = cfg_channels as usize;

        let stream = match supported_cfg.sample_format() {
            // --- F32: el formato más común en Linux moderno ---
            SampleFormat::F32 => device
                .build_input_stream(
                    &stream_config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        if p.load(Ordering::SeqCst) || s.load(Ordering::SeqCst) {
                            return;
                        }
                        if let Ok(mut g) = w.lock() {
                            if let Some(writer) = g.as_mut() {
                                if mono_mode {
                                    for &sample in data {
                                        let s16 =
                                            (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                                        let _ = writer.write_sample(s16);
                                    }
                                } else {
                                    for frame in data.chunks(ch) {
                                        let avg: f32 = frame.iter().sum::<f32>() / ch as f32;
                                        let s16 =
                                            (avg.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                                        let _ = writer.write_sample(s16);
                                    }
                                }
                            }
                        }
                    },
                    err_fn,
                    None,
                )
                .context("construyendo stream f32")?,

            // --- I16: formato nativo de hound, sin conversión ---
            SampleFormat::I16 => device
                .build_input_stream(
                    &stream_config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        if p.load(Ordering::SeqCst) || s.load(Ordering::SeqCst) {
                            return;
                        }
                        if let Ok(mut g) = w.lock() {
                            if let Some(writer) = g.as_mut() {
                                if mono_mode {
                                    for &sample in data {
                                        let _ = writer.write_sample(sample);
                                    }
                                } else {
                                    for frame in data.chunks(ch) {
                                        let sum: i32 =
                                            frame.iter().map(|&s| s as i32).sum();
                                        let avg = (sum / ch as i32) as i16;
                                        let _ = writer.write_sample(avg);
                                    }
                                }
                            }
                        }
                    },
                    err_fn,
                    None,
                )
                .context("construyendo stream i16")?,

            // --- I32: convierto de 32-bit a 16-bit ---
            SampleFormat::I32 => device
                .build_input_stream(
                    &stream_config,
                    move |data: &[i32], _: &cpal::InputCallbackInfo| {
                        if p.load(Ordering::SeqCst) || s.load(Ordering::SeqCst) {
                            return;
                        }
                        if let Ok(mut g) = w.lock() {
                            if let Some(writer) = g.as_mut() {
                                if mono_mode {
                                    for &sample in data {
                                        let s16 = (sample >> 16) as i16;
                                        let _ = writer.write_sample(s16);
                                    }
                                } else {
                                    for frame in data.chunks(ch) {
                                        let sum: i64 =
                                            frame.iter().map(|&s| s as i64).sum();
                                        let avg = (sum / ch as i64) as i16;
                                        let _ = writer.write_sample(avg);
                                    }
                                }
                            }
                        }
                    },
                    err_fn,
                    None,
                )
                .context("construyendo stream i32")?,

            // --- U8: formato de micrófonos básicos/USB (rango 0-255) ---
            SampleFormat::U8 => device
                .build_input_stream(
                    &stream_config,
                    move |data: &[u8], _: &cpal::InputCallbackInfo| {
                        if p.load(Ordering::SeqCst) || s.load(Ordering::SeqCst) {
                            return;
                        }
                        if let Ok(mut g) = w.lock() {
                            if let Some(writer) = g.as_mut() {
                                if mono_mode {
                                    for &sample in data {
                                        // U8: 0-255, centro en 128.
                                        let f = (sample as f32 - 128.0) / 128.0;
                                        let s16 = (f.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                                        let _ = writer.write_sample(s16);
                                    }
                                } else {
                                    for frame in data.chunks(ch) {
                                        let avg: f32 = frame
                                            .iter()
                                            .map(|&s| (s as f32 - 128.0) / 128.0)
                                            .sum::<f32>()
                                            / ch as f32;
                                        let s16 = (avg.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                                        let _ = writer.write_sample(s16);
                                    }
                                }
                            }
                        }
                    },
                    err_fn,
                    None,
                )
                .context("construyendo stream u8")?,

            // --- I8: similar a U8 pero con signo ---
            SampleFormat::I8 => device
                .build_input_stream(
                    &stream_config,
                    move |data: &[i8], _: &cpal::InputCallbackInfo| {
                        if p.load(Ordering::SeqCst) || s.load(Ordering::SeqCst) {
                            return;
                        }
                        if let Ok(mut g) = w.lock() {
                            if let Some(writer) = g.as_mut() {
                                if mono_mode {
                                    for &sample in data {
                                        let f = sample as f32 / i8::MAX as f32;
                                        let s16 = (f.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                                        let _ = writer.write_sample(s16);
                                    }
                                } else {
                                    for frame in data.chunks(ch) {
                                        let avg: f32 = frame
                                            .iter()
                                            .map(|&s| s as f32)
                                            .sum::<f32>()
                                            / (ch as f32 * i8::MAX as f32);
                                        let s16 = (avg.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                                        let _ = writer.write_sample(s16);
                                    }
                                }
                            }
                        }
                    },
                    err_fn,
                    None,
                )
                .context("construyendo stream i8")?,

            // --- U16, U32, I64, U64, F64: conversión genérica ---
            SampleFormat::U16 | SampleFormat::U32 | SampleFormat::I64 |
            SampleFormat::U64 | SampleFormat::F64 => {
                // Estos formatos son poco comunes en entrada de audio.
                // Los intentamos convertir vía sus representaciones más cercanas.
                match supported_cfg.sample_format() {
                    SampleFormat::U16 => device
                        .build_input_stream(
                            &stream_config,
                            move |data: &[u16], _: &cpal::InputCallbackInfo| {
                                if p.load(Ordering::SeqCst) || s.load(Ordering::SeqCst) {
                                    return;
                                }
                                if let Ok(mut g) = w.lock() {
                                    if let Some(writer) = g.as_mut() {
                                        for &sample in data {
                                            // U16: 0-65535, centro en 32768.
                                            let f = (sample as f32 - 32768.0) / 32768.0;
                                            let s16 =
                                                (f.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                                            let _ = writer.write_sample(s16);
                                        }
                                    }
                                }
                            },
                            err_fn,
                            None,
                        )
                        .context("construyendo stream u16")?,
                    SampleFormat::F64 => device
                        .build_input_stream(
                            &stream_config,
                            move |data: &[f64], _: &cpal::InputCallbackInfo| {
                                if p.load(Ordering::SeqCst) || s.load(Ordering::SeqCst) {
                                    return;
                                }
                                if let Ok(mut g) = w.lock() {
                                    if let Some(writer) = g.as_mut() {
                                        for &sample in data {
                                            let f = sample as f32;
                                            let s16 =
                                                (f.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                                            let _ = writer.write_sample(s16);
                                        }
                                    }
                                }
                            },
                            err_fn,
                            None,
                        )
                        .context("construyendo stream f64")?,
                    _ => anyhow::bail!(
                        "formato de muestra no soportado: {:?}",
                        supported_cfg.sample_format()
                    ),
                }
            }

            fmt => anyhow::bail!("formato de muestra no soportado: {:?}", fmt),
        };

        stream.play().context("iniciando stream de audio")?;
        tracing::debug!("grabando a {}", output_path.display());

        // Bloquear hasta que is_stopped sea true. El callback de cpal
        // escribe en el WAV en un thread separado del runtime de cpal.
        let start = Instant::now();
        while !is_stopped.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        // Detener el stream y finalizar el WAV.
        // Marcar como shutting_down para que el callback de error no se queje.
        is_shutting_down.store(true, Ordering::SeqCst);
        drop(stream);
        if let Ok(mut g) = writer.lock() {
            if let Some(w) = g.take() {
                w.finalize().context("finalizando WAV")?;
            }
        }
        let elapsed = start.elapsed().as_secs_f32();
        let sample_count = (elapsed * sample_rate as f32) as usize;
        tracing::debug!(
            "grabación finalizada: {:.1}s, {} muestras",
            elapsed,
            sample_count
        );
        Ok(RecordingResult {
            wav_path: output_path.to_path_buf(),
            duration_secs: elapsed,
            sample_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorder_constructs_with_sample_rate() {
        let r = AudioRecorder::new(16000);
        assert!(!r.is_paused.load(Ordering::SeqCst));
        assert!(!r.is_stopped.load(Ordering::SeqCst));
    }

    #[test]
    fn pause_and_resume_toggle_flag() {
        let r = AudioRecorder::new(16000);
        r.pause();
        assert!(r.is_paused.load(Ordering::SeqCst));
        r.resume();
        assert!(!r.is_paused.load(Ordering::SeqCst));
    }

    #[test]
    fn stop_sets_flag() {
        let r = AudioRecorder::new(16000);
        r.stop();
        assert!(r.is_stopped.load(Ordering::SeqCst));
    }
}
