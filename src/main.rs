//! `clase-notes` — entrypoint.

use anyhow::{Context, Result};
use clap::Parser;
use chrono::NaiveDate;
use tracing_subscriber::{fmt, EnvFilter};

mod audio;
mod cli;
mod config;
mod llm;
mod notes;
mod obsidian;
mod pipeline;
mod transcription;
mod tui;

use cli::{Cli, Command};
use config::Config;

#[tokio::main]
async fn main() -> Result<()> {
    // Instalar crypto provider para rustls (ring).
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Logging.
    let _ = fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new("info,clase_notes=debug")
        }))
        .with_target(false)
        .try_init();

    let cli = Cli::parse();
    let cfg = Config::load_or_init().context("cargando configuración")?;

    match cli.command.unwrap_or(Command::Tui) {
        Command::Tui => {
            let mut app = tui::App::new(cfg);
            tui::runner::run(&mut app).await?;
        }
        Command::Process {
            wav,
            materia,
            tema,
            date,
            tags,
        } => {
            let date = parse_date(date.as_deref())?;
            let wav_path = std::path::PathBuf::from(wav);
            let pipeline = pipeline::Pipeline::new(cfg)?;
            let result = pipeline
                .process_existing(&wav_path, &materia, &tema, date, &tags)
                .await?;
            println!("✓ Nota escrita en: {}", result.note_path.display());
            println!("  Audio: {}", result.audio_name);
            println!("  MOC actualizado: {}", result.moc_path.display());
            println!(
                "  Puntos clave: {}, Conceptos: {}, Tareas: {}",
                result.summary.puntos_clave.len(),
                result.summary.conceptos.len(),
                result.summary.tareas.len()
            );
        }
        Command::Record {
            materia,
            tema,
            date,
            tags,
            non_interactive: _,
        } => {
            // Por simplicidad, redirigir a la TUI si no se especifica
            // non_interactive; en modo no interactivo, grabar bloqueante.
            let date = parse_date(date.as_deref())?;
            let pipeline = pipeline::Pipeline::new(cfg)?;
            let stamp = date.format("%Y-%m-%d").to_string();
            let path = config::work_dir()?.join(format!(
                "{}-{}-{}.wav",
                stamp,
                slug(&materia),
                slug(&tema)
            ));
            println!("Grabando a: {}", path.display());
            println!("Pulsa Ctrl+C para detener.");
            let result = pipeline.record(&path).await?;
            println!("✓ Grabado ({:.0}s). Procesando...", result.duration_secs);
            let processed = pipeline
                .process_existing(&result.wav_path, &materia, &tema, date, &tags)
                .await?;
            println!("✓ Nota escrita en: {}", processed.note_path.display());
            println!("✓ MOC actualizado: {}", processed.moc_path.display());
        }
        Command::Config { path } => {
            if path {
                println!("{}", Config::config_path()?.display());
            } else {
                println!("{:#?}", cfg);
            }
        }
        Command::Doctor => {
            run_doctor(&cfg).await;
        }
        Command::PhoneServer { port } => {
            run_phone_server(port).await?;
        }
    }

    Ok(())
}

fn parse_date(s: Option<&str>) -> Result<NaiveDate> {
    match s {
        Some(d) => NaiveDate::parse_from_str(d, "%Y-%m-%d")
            .with_context(|| format!("fecha inválida: {}", d)),
        None => Ok(chrono::Local::now().date_naive()),
    }
}

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

async fn run_doctor(cfg: &Config) {
    println!("=== clase-notes doctor ===\n");

    // 1. Whisper model.
    if cfg.whisper.model_path.exists() {
        println!("✓ Whisper model: {}", cfg.whisper.model_path.display());
    } else {
        println!(
            "✗ Whisper model NO encontrado: {}",
            cfg.whisper.model_path.display()
        );
        println!(
            "  Descárgalo con:\n    ~/clase-notes/scripts/download-whisper-model.sh medium"
        );
    }

    // 2. Ollama.
    let client = llm::LlmClient::new(cfg.llm.clone()).unwrap();
    match client.health_check().await {
        Ok(_) => println!(
            "✓ Ollama responde en {} (modelo '{}' disponible)",
            cfg.llm.endpoint, cfg.llm.model
        ),
        Err(e) => println!("✗ Ollama: {}", e),
    }

    // 3. Bóveda.
    if cfg.obsidian.vault_path.exists() {
        println!("✓ Bóveda Obsidian: {}", cfg.obsidian.vault_path.display());
    } else {
        println!(
            "⚠ Bóveda Obsidian NO existe: {}",
            cfg.obsidian.vault_path.display()
        );
    }
}

async fn run_phone_server(port: u16) -> Result<()> {
    let data_dir = config::work_dir()?;
    let cert_path = data_dir.join("server-cert.pem");
    let ip = audio::local_ip();
    let url = format!("https://{}:{}/", ip, port);

    println!("=== clase-notes phone-server ===\n");
    println!("IP detectada:  {}", ip);
    println!("Puerto:        {}", port);
    println!("URL:           {}", url);
    println!();
    println!("Escaneá el QR con la cámara del iPhone o ingresá");
    println!("la URL manualmente en Safari.");
    println!();
    println!("Esperando conexión del teléfono... (Ctrl+C para salir)\n");

    let state = audio::start_server(port, cert_path).await?;

    // Esperar a que se conecte un cliente.
    loop {
        if state.is_connected() {
            println!("● ¡Teléfono conectado! Esperando audio...");
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}
