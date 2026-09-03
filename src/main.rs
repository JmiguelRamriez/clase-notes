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

    // Logging: en CLI mostramos info por defecto, en TUI evitamos debug ruidoso
    // que corrompe el alternate screen. El usuario puede activar debug con RUST_LOG=debug.
    let cli = Cli::parse();
    let default_filter = if matches!(cli.command, None | Some(crate::cli::Command::Tui)) {
        "info"
    } else {
        "info,clase_notes=debug"
    };
    let _ = fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new(default_filter)
        }))
        .with_target(false)
        .try_init();

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

    // 1. Whisper model + GPU.
    if cfg.whisper.model_path.exists() {
        let meta = std::fs::metadata(&cfg.whisper.model_path).ok();
        let size = meta
            .map(|m| format!("{:.0} MB", m.len() as f64 / 1024.0 / 1024.0))
            .unwrap_or_else(|| "?".into());
        println!(
            "✓ Whisper model: {} ({}), lang={}, gpu={}, chunk={}s",
            cfg.whisper.model_path.display(),
            size,
            cfg.whisper.language,
            cfg.whisper.use_gpu,
            cfg.whisper.chunk_secs
        );
        if cfg.whisper.use_gpu {
            // Intentar detectar GPU sin fallar si nvidia-smi no está
            let gpu_info = std::process::Command::new("nvidia-smi")
                .arg("--query-gpu=name,memory.total")
                .arg("--format=csv,noheader")
                .output();
            match gpu_info {
                Ok(o) if o.status.success() => {
                    let txt = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if !txt.is_empty() {
                        println!("  GPU detectada: {}", txt);
                    } else {
                        println!("  GPU: use_gpu=true pero nvidia-smi no reporta GPU");
                    }
                }
                _ => println!("  ⚠ use_gpu=true pero nvidia-smi no encontrado — ¿driver/CUDA instalados?"),
            }
            #[cfg(not(feature = "cuda"))]
            println!("  ⚠ Binario compilado SIN --features cuda: use_gpu no acelerará (recompila con --features cuda)");
        } else {
            println!("  (usa CPU; para RTX 3050 pon use_gpu=true en config.toml y recompila con --features cuda)");
        }
    } else {
        println!(
            "✗ Whisper model NO encontrado: {}",
            cfg.whisper.model_path.display()
        );
        println!(
            "  Descárgalo con:\n    ./scripts/download-whisper-model.sh small  # 4GB VRAM recomendado\n    ./scripts/download-whisper-model.sh medium # si tenés 8GB+"
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

    // 4. Tip para 1h en 4GB
    if cfg.whisper.chunk_secs == 0 {
        println!("  Tip: 1h de audio sin chunked puede OOM en 4GB. Pon chunk_secs=30 en config.toml");
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
