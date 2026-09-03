//! Definición de subcomandos CLI con `clap`.

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "clase-notes", version, about = "Notas de clase con Whisper + LLM + Obsidian", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Lanza la TUI (interfaz interactiva en terminal).
    Tui,

    /// Procesa un archivo WAV ya grabado.
    Process {
        /// Ruta al archivo WAV de entrada.
        wav: String,
        /// Materia (ej. "Cálculo II").
        #[arg(long)]
        materia: String,
        /// Tema de la clase (ej. "Límites").
        #[arg(long)]
        tema: String,
        /// Fecha de la clase (YYYY-MM-DD). Por defecto: hoy.
        #[arg(long)]
        date: Option<String>,
        /// Tags adicionales separados por coma.
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
    },

    /// Graba audio y lo procesa en un solo paso.
    Record {
        #[arg(long)]
        materia: String,
        #[arg(long)]
        tema: String,
        #[arg(long)]
        date: Option<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        /// Salir automáticamente al terminar (útil para scripts).
        #[arg(long)]
        non_interactive: bool,
    },

    /// Muestra o edita la configuración.
    Config {
        /// Mostrar la ruta del archivo de configuración.
        #[arg(long)]
        path: bool,
    },

    /// Verifica el entorno (Whisper, Ollama, bóveda).
    Doctor,

    /// Arranca el servidor HTTPS para conexión del teléfono.
    /// Imprime la URL y queda escuchando hasta Ctrl+C.
    PhoneServer {
        /// Puerto del servidor (default: 8443).
        #[arg(long, default_value_t = 8443)]
        port: u16,
    },
}
