//! Cliente HTTP para Ollama. Implementa dos operaciones:
//!
//! 1. `clean_text`: corrige la transcripción cruda.
//! 2. `summarize`: genera las secciones de notas a partir del texto limpio.

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::config::LlmConfig;
use crate::llm::prompts;

/// Salida de `summarize`: las 4 secciones tal como las emite el LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryOutput {
    pub resumen: String,
    pub puntos_clave: Vec<String>,
    pub conceptos: Vec<(String, String)>, // (nombre, definición)
    pub tareas: Vec<String>,
}

pub struct LlmClient {
    cfg: LlmConfig,
    http: Client,
}

impl LlmClient {
    pub fn new(cfg: LlmConfig) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(300)) // Ollama puede tardar mucho en CPU.
            .build()
            .context("construyendo cliente HTTP")?;
        Ok(Self { cfg, http })
    }

    /// Verifica que el LLM esté corriendo y el modelo esté disponible.
    /// - Ollama: valida `GET /api/tags` y que `cfg.model` esté pulleado.
    /// - Groq: valida `GROQ_API_KEY` y `GET /openai/v1/models` con Bearer.
    pub async fn health_check(&self) -> Result<()> {
        if self.cfg.is_groq() {
            return self.health_check_groq().await;
        }
        #[derive(Deserialize)]
        struct TagsResp {
            models: Option<Vec<ModelEntry>>,
        }
        #[derive(Deserialize)]
        struct ModelEntry {
            name: String,
        }

        let url = format!("{}/api/tags", self.cfg.endpoint);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {}", url))?;
        if !resp.status().is_success() {
            anyhow::bail!("Ollama no responde OK: {}", resp.status());
        }
        let body: TagsResp = resp
            .json()
            .await
            .context("parseando /api/tags de Ollama")?;

        let models = body.models.unwrap_or_default();
        if models.is_empty() {
            anyhow::bail!(
                "Ollama responde pero no tiene ningún modelo instalado. \
                 Instalá el configurado ({}): ollama pull {}",
                self.cfg.model,
                self.cfg.model
            );
        }
        let wanted = self.cfg.model.trim();
        let found = models.iter().any(|m| {
            let n = m.name.trim();
            n == wanted
                || n.split(':').next().unwrap_or(n) == wanted.split(':').next().unwrap_or(wanted)
        });
        if !found {
            let available: Vec<String> = models.iter().map(|m| m.name.clone()).collect();
            anyhow::bail!(
                "Ollama responde pero el modelo '{}' no está instalado. \
                 Disponibles: [{}]. Instalalo con: ollama pull {}",
                wanted,
                available.join(", "),
                wanted
            );
        }
        Ok(())
    }

    async fn health_check_groq(&self) -> Result<()> {
        let key = self.cfg.effective_groq_api_key().ok_or_else(|| {
            anyhow::anyhow!(
                "Groq seleccionado (provider=groq) pero no hay API key. \
                 Poné GROQ_API_KEY en env o groq_api_key en config.toml (chmod 600) y revocá la key expuesta."
            )
        })?;
        let url = "https://api.groq.com/openai/v1/models";
        let resp = self
            .http
            .get(url)
            .header("Authorization", format!("Bearer {}", key))
            .send()
            .await
            .with_context(|| format!("GET {}", url))?;
        let status = resp.status();
        if !status.is_success() {
            let txt = resp.text().await.unwrap_or_default();
            anyhow::bail!("Groq no responde OK: {} — {}", status, txt);
        }
        // Opcional: verificar que groq_model esté en la lista, si no solo avisar
        let body: serde_json::Value = resp.json().await.context("parseando /v1/models de Groq")?;
        let wanted = self.cfg.groq_model.trim();
        if let Some(arr) = body.get("data").and_then(|d| d.as_array()) {
            let found = arr.iter().any(|m| m.get("id").and_then(|v| v.as_str()).map(|s| s == wanted).unwrap_or(false));
            if !found {
                tracing::warn!("Groq modelo '{}' no aparece en /v1/models, igual se intentará usar", wanted);
            }
        }
        tracing::info!("Groq OK (modelo {})", wanted);
        Ok(())
    }

    /// Llamada genérica (no streaming). Usa Ollama o Groq según provider.
    async fn generate(&self, system: &str, user: &str) -> Result<String> {
        if self.cfg.is_groq() {
            return self.generate_groq(system, user).await;
        }
        #[derive(Serialize)]
        struct Req<'a> {
            model: &'a str,
            prompt: &'a str,
            system: &'a str,
            stream: bool,
            options: Options<'a>,
        }
        #[derive(Serialize)]
        struct Options<'a> {
            temperature: f32,
            num_ctx: usize,
            #[serde(skip_serializing_if = "Option::is_none")]
            stop: Option<&'a [&'a str]>,
        }
        #[derive(Deserialize)]
        struct Resp {
            response: String,
            #[allow(dead_code)]
            done: bool,
        }

        let body = Req {
            model: &self.cfg.model,
            prompt: user,
            system,
            stream: false,
            options: Options {
                temperature: self.cfg.temperature,
                num_ctx: 8192,
                stop: None,
            },
        };

        let url = format!("{}/api/generate", self.cfg.endpoint);
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {}", url))?;
        let status = resp.status();
        if !status.is_success() {
            let txt = resp.text().await.unwrap_or_default();
            anyhow::bail!("Ollama devolvió {}: {}", status, txt);
        }
        let parsed: Resp = resp.json().await.context("parseando respuesta Ollama")?;
        Ok(parsed.response)
    }

    async fn generate_groq(&self, system: &str, user: &str) -> Result<String> {
        #[derive(Serialize)]
        struct ChatReq<'a> {
            model: &'a str,
            messages: Vec<Msg<'a>>,
            temperature: f32,
            stream: bool,
        }
        #[derive(Serialize)]
        struct Msg<'a> {
            role: &'a str,
            content: &'a str,
        }
        #[derive(Deserialize)]
        struct ChatResp {
            choices: Vec<Choice>,
        }
        #[derive(Deserialize)]
        struct Choice {
            message: MsgOwned,
        }
        #[derive(Deserialize)]
        struct MsgOwned {
            content: String,
        }

        let key = self.cfg.effective_groq_api_key().ok_or_else(|| {
            anyhow::anyhow!("GROQ_API_KEY no configurada para provider=groq")
        })?;
        let body = ChatReq {
            model: &self.cfg.groq_model,
            messages: vec![
                Msg { role: "system", content: system },
                Msg { role: "user", content: user },
            ],
            temperature: self.cfg.temperature,
            stream: false,
        };
        let url = "https://api.groq.com/openai/v1/chat/completions";
        let resp = self
            .http
            .post(url)
            .header("Authorization", format!("Bearer {}", key))
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {}", url))?;
        let status = resp.status();
        if !status.is_success() {
            let txt = resp.text().await.unwrap_or_default();
            anyhow::bail!("Groq devolvió {}: {}", status, txt);
        }
        let parsed: ChatResp = resp.json().await.context("parseando respuesta Groq")?;
        let content = parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default();
        if content.trim().is_empty() {
            anyhow::bail!("Groq devolvió respuesta vacía");
        }
        Ok(content)
    }

    /// Limpia la transcripción cruda.
    pub async fn clean_text(&self, raw: &str) -> Result<String> {
        self.generate(prompts::CLEANUP_SYSTEM, raw).await
    }

    /// Genera el resumen estructurado.
    pub async fn summarize(&self, clean: &str) -> Result<SummaryOutput> {
        let md = self.generate(prompts::SUMMARY_SYSTEM, clean).await?;
        Ok(parse_summary(&md))
    }
}

/// Parsea la salida Markdown del LLM en `SummaryOutput`. Tolera
/// variaciones menores (mayúsculas, espacios, prefijos `##`).
fn parse_summary(md: &str) -> SummaryOutput {
    let sections = split_sections(md);

    let resumen = sections
        .get("resumen")
        .cloned()
        .unwrap_or_default()
        .trim()
        .to_string();

    let puntos_clave = sections
        .get("puntos clave")
        .map(|s| extract_bullets(s))
        .unwrap_or_default();

    let conceptos = sections
        .get("conceptos")
        .map(|s| extract_concepts(s))
        .unwrap_or_default();

    let tareas_raw = sections
        .get("tareas / pendientes")
        .or_else(|| sections.get("tareas/pendientes"))
        .or_else(|| sections.get("tareas"))
        .cloned()
        .unwrap_or_default();
    let tareas = extract_bullets(&tareas_raw);

    SummaryOutput {
        resumen,
        puntos_clave,
        conceptos,
        tareas,
    }
}

/// Divide el Markdown en secciones por encabezados `##`.
fn split_sections(md: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let mut current: Option<String> = None;
    let mut buf = String::new();
    for line in md.lines() {
        let trimmed = line.trim();
        if let Some(stripped) = trimmed.strip_prefix("##") {
            // Guardar sección anterior.
            if let Some(key) = current.take() {
                map.insert(key, buf.trim().to_string());
            }
            current = Some(stripped.trim().to_lowercase());
            buf.clear();
        } else if current.is_some() {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    if let Some(key) = current {
        map.insert(key, buf.trim().to_string());
    }
    map
}

/// Extrae bullets (líneas que empiezan con `- `, `* ` o `- [ ]`).
fn extract_bullets(s: &str) -> Vec<String> {
    s.lines()
        .map(|l| l.trim())
        .filter(|l| l.starts_with("- ") || l.starts_with("* ") || l.starts_with("- [ ]"))
        .map(|l| {
            l.trim_start_matches("- [ ]")
                .trim_start_matches("- ")
                .trim_start_matches("* ")
                .trim()
                .to_string()
        })
        .filter(|l| !l.is_empty() && l.to_lowercase() != "ninguna explícita")
        .collect()
}

/// Extrae `[[Concepto]] — definición` o `[[Concepto]] - definición`.
fn extract_concepts(s: &str) -> Vec<(String, String)> {
    s.lines()
        .map(|l| l.trim())
        .filter(|l| l.starts_with("- ") || l.starts_with("* "))
        .filter_map(|l| {
            let body = l
                .trim_start_matches("- ")
                .trim_start_matches("* ")
                .trim();
            // Formato esperado: [[Nombre]] — def
            let start = body.find("[[")?;
            let end = body[start + 2..].find("]]")?;
            let name = body[start + 2..start + 2 + end].trim().to_string();
            let after = body[start + 2 + end + 2..].trim();
            // Separador: —, --, -
            let def = after
                .trim_start_matches(['—', '–', '-'])
                .trim()
                .to_string();
            Some((name, def))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_summary_basic() {
        let md = r#"## Resumen
Clase de cálculo sobre límites.

## Puntos clave
- Definición épsilon-delta
- Límites laterales
- Indeterminación 0/0

## Conceptos
- [[Límite]] — valor al que tiende una función
- [[Continuidad]] — no hay saltos

## Tareas / pendientes
- [ ] Ejercicios 1 al 10
- [ ] Leer capítulo 3
"#;
        let s = parse_summary(md);
        assert!(s.resumen.contains("cálculo"));
        assert_eq!(s.puntos_clave.len(), 3);
        assert_eq!(s.conceptos.len(), 2);
        assert_eq!(s.conceptos[0].0, "Límite");
        assert_eq!(s.tareas.len(), 2);
    }

    #[test]
    fn extract_bullets_ignores_none_marker() {
        let s = "- [ ] Una tarea\n- Ninguna explícita\n- [ ] Otra";
        let b = extract_bullets(s);
        assert_eq!(b.len(), 2);
    }

    #[test]
    fn extract_concepts_with_dash_separator() {
        let s = "- [[Topología]] - estudio de la continuidad";
        let c = extract_concepts(s);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].0, "Topología");
    }

    #[tokio::test]
    async fn health_check_ok_when_model_present() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/api/tags")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"models":[{"name":"llama3.1:8b"},{"name":"mistral:latest"}]}"#)
            .create_async()
            .await;
        let cfg = crate::config::LlmConfig {
            endpoint: server.url(),
            model: "llama3.1:8b".into(),
            temperature: 0.3,
            ..Default::default()
        };
        let client = LlmClient::new(cfg).unwrap();
        assert!(client.health_check().await.is_ok());
    }

    #[tokio::test]
    async fn health_check_fails_when_model_missing() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/api/tags")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"models":[{"name":"mistral:latest"}]}"#)
            .create_async()
            .await;
        let cfg = crate::config::LlmConfig {
            endpoint: server.url(),
            model: "llama3.1:8b".into(),
            temperature: 0.3,
            ..Default::default()
        };
        let client = LlmClient::new(cfg).unwrap();
        let err = client.health_check().await.unwrap_err().to_string();
        assert!(err.contains("llama3.1:8b"), "err: {}", err);
        assert!(err.contains("ollama pull"), "err: {}", err);
    }

    #[tokio::test]
    async fn health_check_fails_when_no_models() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/api/tags")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"models":[]}"#)
            .create_async()
            .await;
        let cfg = crate::config::LlmConfig {
            endpoint: server.url(),
            model: "llama3.1:8b".into(),
            temperature: 0.3,
            ..Default::default()
        };
        let client = LlmClient::new(cfg).unwrap();
        assert!(client.health_check().await.is_err());
    }

    #[tokio::test]
    async fn groq_health_check_fails_without_key() {
        let cfg = crate::config::LlmConfig {
            provider: "groq".into(),
            groq_model: "llama-3.3-70b-versatile".into(),
            groq_api_key: None,
            endpoint: "http://localhost:11434".into(),
            model: "dummy".into(),
            temperature: 0.3,
        };
        std::env::remove_var("GROQ_API_KEY");
        let client = LlmClient::new(cfg).unwrap();
        let err = client.health_check().await.unwrap_err().to_string();
        assert!(err.contains("GROQ_API_KEY"), "err: {}", err);
    }
}
