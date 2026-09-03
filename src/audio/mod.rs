//! Módulo de audio: captura del micrófono y escritura a WAV.
//!
//! Hay dos backends:
//!
//! - `real` (cuando se compila con `--features audio-capture`): usa `cpal`.
//! - `null` (por defecto): produce silencio. Útil para entornos sin
//!   sistema de audio (CI, contenedores) y para que los tests
//!   compilen sin libasound2-dev.

pub mod filter;
#[cfg(feature = "audio-capture")]
pub mod phone;
#[cfg(not(feature = "audio-capture"))]
pub mod phone_stub;
pub mod recorder;

pub use filter::{apply_filter_to_wav, AudioPreset};
#[cfg(feature = "audio-capture")]
pub use phone::{local_ip, start_server, PhoneState, PHONE_SAMPLE_RATE};
#[cfg(not(feature = "audio-capture"))]
pub use phone_stub::{local_ip, start_server, PhoneState, PHONE_SAMPLE_RATE};
pub use recorder::{AudioRecorder, RecordingResult};
