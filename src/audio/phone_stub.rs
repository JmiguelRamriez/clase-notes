//! Stub del servidor del teléfono cuando se compila sin `audio-capture`.
//! Permite que `cargo check` y `cargo test` pasen sin ALSA/cpal.
//! Todas las operaciones que requieren audio real devuelven un error explicativo.

use anyhow::Result;
use std::path::PathBuf;

pub const PHONE_SAMPLE_RATE: u32 = 16000;

#[derive(Clone, Debug)]
pub struct PhoneState {
    #[allow(dead_code)]
    pub port: u16,
    #[allow(dead_code)]
    pub cert_path: PathBuf,
    #[allow(dead_code)]
    pub connected: std::sync::Arc<std::sync::atomic::AtomicBool>,
    // rx dummy para que el tipo exista; nunca produce datos.
    _priv: (),
}

impl PhoneState {
    pub async fn try_read_frame(&self) -> Option<Vec<i16>> {
        None
    }

    pub fn is_connected(&self) -> bool {
        false
    }
}

pub fn local_ip() -> std::net::IpAddr {
    // Sin red real, devolver localhost para que phone_url() no paniquee.
    "127.0.0.1".parse().unwrap()
}

pub async fn start_server(_port: u16, _cert_path: PathBuf) -> Result<PhoneState> {
    anyhow::bail!(
        "este binario se compiló sin la feature 'audio-capture'.\n\
         Recompila con: cargo build --release --features audio-capture"
    )
}
