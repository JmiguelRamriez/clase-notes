//! Integración con un LLM local (Ollama por defecto).

pub mod client;
pub mod prompts;

pub use client::{LlmClient, SummaryOutput};
