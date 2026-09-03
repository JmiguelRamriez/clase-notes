//! Configuración global de `clase-notes`.
//!
//! Se carga desde `~/.config/clase-notes/config.toml`. Si no existe, se
//! crea con valores por defecto en el primer arranque.

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub obsidian: ObsidianConfig,
    pub audio: AudioConfig,
    pub whisper: WhisperConfig,
    pub llm: LlmConfig,
    #[serde(default)]
    pub tui: TuiConfig,
    #[serde(default)]
    pub phone: PhoneConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObsidianConfig {
    pub vault_path: PathBuf,
    #[serde(default = "default_notes_subdir")]
    pub notes_subdir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,
    #[serde(default = "default_channels")]
    pub channels: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhisperConfig {
    pub model_path: PathBuf,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_use_gpu")]
    pub use_gpu: bool,
    #[serde(default = "default_chunk_secs")]
    pub chunk_secs: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Proveedor LLM: "ollama" (default, local) o "groq" (nube, rápido)
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_endpoint")]
    pub endpoint: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// Modelo Groq (OpenAI-compatible). Ej: llama-3.3-70b-versatile, mixtral-8x7b-32768, llama-3.1-8b-instant
    #[serde(default = "default_groq_model")]
    pub groq_model: String,
    /// API key Groq. Preferible vía env GROQ_API_KEY (no se escribe en git).
    /// Si se pone aquí, el archivo debe tener chmod 600. Se lee también de env si está vacío.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub groq_api_key: Option<String>,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            endpoint: default_endpoint(),
            model: default_model(),
            temperature: default_temperature(),
            groq_model: default_groq_model(),
            groq_api_key: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TuiConfig {
    #[serde(default = "default_theme")]
    pub color_theme: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhoneConfig {
    #[serde(default = "default_phone_port")]
    pub port: u16,
    #[serde(default = "default_phone_preset")]
    pub preset: String,
}

fn default_notes_subdir() -> String {
    "Clases".to_string()
}
fn default_sample_rate() -> u32 {
    16000
}
fn default_channels() -> u16 {
    1
}
fn default_language() -> String {
    "es".to_string()
}
fn default_use_gpu() -> bool {
    false
}
fn default_chunk_secs() -> u32 {
    30
}
fn default_endpoint() -> String {
    "http://localhost:11434".to_string()
}
fn default_provider() -> String {
    "ollama".to_string()
}
fn default_model() -> String {
    "llama3.1:8b".to_string()
}
fn default_groq_model() -> String {
    "openai/gpt-oss-20b".to_string()
}
fn default_temperature() -> f32 {
    0.3
}
fn default_theme() -> String {
    "dark".to_string()
}
fn default_phone_port() -> u16 {
    8443
}
fn default_phone_preset() -> String {
    "normal".to_string()
}

impl LlmConfig {
    /// API key efectiva: prioriza env GROQ_API_KEY sobre config.toml
    pub fn effective_groq_api_key(&self) -> Option<String> {
        if let Ok(k) = std::env::var("GROQ_API_KEY") {
            if !k.trim().is_empty() {
                return Some(k.trim().to_string());
            }
        }
        self.groq_api_key
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    pub fn is_groq(&self) -> bool {
        self.provider.trim().eq_ignore_ascii_case("groq")
    }
}

impl Default for PhoneConfig {
    fn default() -> Self {
        Self {
            port: default_phone_port(),
            preset: default_phone_preset(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        let home = directories::UserDirs::new()
            .and_then(|u| u.home_dir().to_path_buf().into())
            .unwrap_or_else(|| PathBuf::from("/tmp"));

        Self {
            obsidian: ObsidianConfig {
                vault_path: home.join("Obsidian"),
                notes_subdir: default_notes_subdir(),
            },
            audio: AudioConfig {
                sample_rate: default_sample_rate(),
                channels: default_channels(),
            },
            whisper: WhisperConfig {
                model_path: home.join(".local/share/clase-notes/ggml-tiny.bin"),
                language: default_language(),
                use_gpu: default_use_gpu(),
                chunk_secs: default_chunk_secs(),
            },
            llm: LlmConfig {
                provider: default_provider(),
                endpoint: default_endpoint(),
                model: default_model(),
                temperature: default_temperature(),
                groq_model: default_groq_model(),
                groq_api_key: None,
            },
            tui: TuiConfig {
                color_theme: default_theme(),
            },
            phone: PhoneConfig::default(),
        }
    }
}

impl Config {
    /// Devuelve la ruta del archivo de configuración:
    /// `~/.config/clase-notes/config.toml` (Linux).
    pub fn config_path() -> Result<PathBuf> {
        let dirs = ProjectDirs::from("dev", "josemr21", "clase-notes")
            .context("no se pudo determinar el directorio de configuración")?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Carga la configuración desde disco. Si no existe, la crea con
    /// valores por defecto y la devuelve.
    pub fn load_or_init() -> Result<Self> {
        let path = Self::config_path()?;
        if path.exists() {
            let s = fs::read_to_string(&path)
                .with_context(|| format!("leyendo {}", path.display()))?;
            let cfg: Config = toml::from_str(&s)
                .with_context(|| format!("parseando {}", path.display()))?;
            Ok(cfg)
        } else {
            let cfg = Config::default();
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("creando {}", parent.display())
                })?;
            }
            let s = toml::to_string_pretty(&cfg)?;
            fs::write(&path, s)
                .with_context(|| format!("escribiendo {}", path.display()))?;
            tracing::info!("configuración inicial creada en {}", path.display());
            Ok(cfg)
        }
    }

    /// Devuelve la ruta completa al directorio donde se escriben las notas.
    pub fn notes_dir(&self) -> PathBuf {
        self.obsidian.vault_path.join(&self.obsidian.notes_subdir)
    }
}

/// Devuelve la ruta al directorio de trabajo (donde se guardan WAVs
/// intermedios). Por defecto: `~/.local/share/clase-notes/recordings/`.
pub fn work_dir() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("dev", "josemr21", "clase-notes")
        .context("no se pudo determinar el directorio de datos")?;
    let path = dirs.data_dir().join("recordings");
    fs::create_dir_all(&path)?;
    Ok(path)
}

/// Crea un archivo WAV vacío con los parámetros dados y devuelve el writer.
#[allow(dead_code)]
pub fn create_wav_writer(path: &Path, sample_rate: u32, channels: u16) -> Result<hound::WavWriter<std::io::BufWriter<std::fs::File>>> {
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let writer = hound::WavWriter::create(path, spec)
        .with_context(|| format!("creando {}", path.display()))?;
    Ok(writer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_sensible_values() {
        let cfg = Config::default();
        assert_eq!(cfg.audio.sample_rate, 16000);
        assert_eq!(cfg.audio.channels, 1);
        assert_eq!(cfg.llm.endpoint, "http://localhost:11434");
    }

    #[test]
    fn notes_dir_appends_subdir() {
        let cfg = Config::default();
        let d = cfg.notes_dir();
        assert!(d.ends_with("Clases"));
    }

    #[test]
    fn config_serializes_round_trip() {
        let cfg = Config::default();
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.llm.model, cfg.llm.model);
    }
}
