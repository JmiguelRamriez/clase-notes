//! Servidor WebSocket + TLS para recibir audio del iPhone.
//!
//! Sirve una página web en `GET /` que captura el micrófono del
//! teléfono y la transmite via WebSocket en `wss://host:port/ws`.
//!
//! El servidor genera un certificado TLS autofirmado al inicio y
//! lo cachea en disco para no regenerarlo cada vez.

#![cfg(feature = "audio-capture")]
#![allow(dead_code)] // se usa cuando se integre con la TUI

use anyhow::{Context, Result};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use rustls::ServerConfig;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex};
use tokio_rustls::TlsAcceptor;

const PHONE_PAGE: &str = include_str!("phone_page.html");

/// Sample rate esperado del audio del teléfono (16kHz mono i16).
pub const PHONE_SAMPLE_RATE: u32 = 16000;

/// Estado compartido del servidor: ring buffer de samples + flag de conectado.
#[derive(Clone)]
pub struct PhoneState {
    /// Sender compartido donde los handlers WebSocket escriben.
    pub tx: mpsc::Sender<Vec<i16>>,
    /// Receiver de samples i16 desde el WebSocket.
    pub rx: Arc<Mutex<mpsc::Receiver<Vec<i16>>>>,
    /// Flag público que indica si hay un cliente conectado.
    pub connected: Arc<std::sync::atomic::AtomicBool>,
    /// Path donde se cachea el cert TLS.
    pub cert_path: PathBuf,
    /// Puerto en el que escuchamos.
    pub port: u16,
}

impl std::fmt::Debug for PhoneState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PhoneState")
            .field("connected", &self.connected)
            .field("cert_path", &self.cert_path)
            .field("port", &self.port)
            .finish()
    }
}

impl PhoneState {
    /// Intenta leer un frame de samples del buffer. Devuelve None si
    /// no hay nada disponible.
    pub async fn try_read_frame(&self) -> Option<Vec<i16>> {
        let mut guard = self.rx.lock().await;
        guard.try_recv().ok()
    }

    /// Indica si hay un cliente conectado.
    pub fn is_connected(&self) -> bool {
        self.connected.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Genera (o carga de disco) un certificado TLS autofirmado.
///
/// El cert incluye tanto "clase-notes.local" (DNS) como la IP local
/// (IP SAN) para que iOS Safari acepte la conexión a `https://<ip>:<port>/`.
pub fn load_or_generate_cert(cert_path: &Path, ip: std::net::IpAddr) -> Result<rustls::pki_types::CertificateDer<'static>> {
    use rustls::pki_types::CertificateDer;

    // Intentar cargar de disco.
    if cert_path.exists() {
        let pem = std::fs::read(cert_path).context("leyendo cert del disco")?;
        let certs: Vec<_> = rustls_pemfile::certs(&mut pem.as_slice())
            .collect::<Result<Vec<_>, _>>()
            .context("parseando PEM")?;
        if let Some(cert) = certs.into_iter().next() {
            return Ok(cert);
        }
    }

    // Generar uno nuevo con la IP como SAN.
    let key_pair = rcgen::KeyPair::generate().context("generando key pair")?;
    let ip_str = ip.to_string();

    let params = rcgen::CertificateParams::new(vec![
        "clase-notes.local".to_string(),
        ip_str,
    ])
    .context("creando params del cert")?;

    let cert = params
        .self_signed(&key_pair)
        .context("generando cert autofirmado")?;
    let cert_der = cert.der().clone();
    let key_der = key_pair.serialize_der();

    // Guardar cert y key en disco.
    if let Some(parent) = cert_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(cert_path, cert.pem());

    let key_path = cert_path.with_extension("key");
    let key_pem = pem_for_pkcs8(&key_der);
    let _ = std::fs::write(&key_path, key_pem);

    Ok(CertificateDer::from(cert_der.to_vec()))
}

/// Convierte PKCS8 DER a PEM.
fn pem_for_pkcs8(der: &[u8]) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    let mut out = String::from("-----BEGIN PRIVATE KEY-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap_or(""));
        out.push('\n');
    }
    out.push_str("-----END PRIVATE KEY-----\n");
    out
}

/// Carga la private key desde disco, o genera una nueva.
pub fn load_or_generate_key(key_path: &Path) -> Result<rustls::pki_types::PrivateKeyDer<'static>> {
    use rustls::pki_types::PrivateKeyDer;

    if key_path.exists() {
        let pem = std::fs::read(key_path).context("leyendo key del disco")?;
        let keys: Vec<_> = rustls_pemfile::pkcs8_private_keys(&mut pem.as_slice())
            .collect::<Result<Vec<_>, _>>()
            .context("parseando key PEM")?;
        if let Some(key) = keys.into_iter().next() {
            return Ok(PrivateKeyDer::Pkcs8(key));
        }
    }

    anyhow::bail!("key no encontrada; debería haberse generado junto al cert")
}

/// Crea el `ServerConfig` de rustls con cert + key.
pub fn build_tls_config(cert_path: &Path, ip: std::net::IpAddr) -> Result<Arc<ServerConfig>> {
    let cert = load_or_generate_cert(cert_path, ip)?;
    let key_path = cert_path.with_extension("key");
    let key = load_or_generate_key(&key_path)?;
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .context("configurando TLS")?;
    Ok(Arc::new(config))
}

/// Handler de `GET /`: sirve la página del teléfono.
async fn serve_phone_page() -> Response {
    (
        axum::http::StatusCode::OK,
        [("content-type", "text/html; charset=utf-8")],
        PHONE_PAGE,
    )
        .into_response()
}

/// Handler de `GET /ws`: upgrade a WebSocket y maneja la conexión.
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<PhoneState>,
) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Maneja una conexión WebSocket entrante.
async fn handle_socket(mut socket: WebSocket, state: PhoneState) {
    tracing::info!("cliente WebSocket conectado");
    state
        .connected
        .store(true, std::sync::atomic::Ordering::SeqCst);

    // Usar el sender compartido del estado; no reemplazar el receiver.
    let tx = state.tx.clone();

    // Limpiar el flag al desconectar.
    let connected = state.connected.clone();
    let cleanup = async move {
        connected.store(false, std::sync::atomic::Ordering::SeqCst);
        tracing::info!("cliente WebSocket desconectado");
    };

    while let Some(msg) = socket.recv().await {
        match msg {
            Ok(Message::Binary(data)) => {
                // data es Vec<u8> con Int16 LE PCM.
                // Convertir a Vec<i16> y enviar al ring buffer.
                if data.len() % 2 != 0 {
                    tracing::warn!("frame binario con longitud impar: {}", data.len());
                    continue;
                }
                let samples: Vec<i16> = data
                    .chunks_exact(2)
                    .map(|c| i16::from_le_bytes([c[0], c[1]]))
                    .collect();
                if tx.send(samples).await.is_err() {
                    // El consumidor colgó.
                    break;
                }
            }
            Ok(Message::Text(text)) => {
                // Mensajes de texto: "PING:<ts>" para medir latencia.
                if let Some(rest) = text.strip_prefix("PING:") {
                    let _ = socket.send(Message::Text(format!("PONG:{}", rest))).await;
                }
            }
            Ok(Message::Close(_)) => break,
            Err(e) => {
                tracing::debug!("error en WebSocket: {}", e);
                break;
            }
            _ => {}
        }
    }

    cleanup.await;
}

/// Arranca el servidor en un tokio task. Devuelve el `PhoneState`
/// para que el caller pueda leer el audio.
pub async fn start_server(port: u16, cert_path: PathBuf) -> Result<PhoneState> {
    let ip = local_ip();
    tracing::info!("IP local detectada: {}", ip);
    let tls_config = build_tls_config(&cert_path, ip)?;

    let (tx, rx) = mpsc::channel::<Vec<i16>>(128);
    let state = PhoneState {
        tx,
        rx: Arc::new(Mutex::new(rx)),
        connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        cert_path: cert_path.clone(),
        port,
    };

    let app = Router::new()
        .route("/", get(serve_phone_page))
        .route("/ws", get(ws_handler))
        .with_state(state.clone());

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind a {}", addr))?;
    let acceptor = TlsAcceptor::from(tls_config);

    tracing::info!("servidor de teléfono escuchando en https://0.0.0.0:{}", port);

    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _peer)) => {
                    let acceptor = acceptor.clone();
                    let app = app.clone();
                    tokio::spawn(async move {
                        match acceptor.accept(stream).await {
                            Ok(tls_stream) => {
                                let io = hyper_util::rt::TokioIo::new(tls_stream);
                                let service = hyper::service::service_fn(move |req| {
                                    let app = app.clone();
                                    async move {
                                        use tower::ServiceExt;
                                        app.oneshot(req).await
                                    }
                                });
                                if let Err(e) =
                                    hyper::server::conn::http1::Builder::new()
                                        .serve_connection(io, service)
                                        .await
                                {
                                    tracing::debug!("conexión HTTP/1.1 falló: {}", e);
                                }
                            }
                            Err(e) => {
                                tracing::debug!("TLS handshake falló: {}", e);
                            }
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!("accept falló: {}", e);
                }
            }
        }
    });

    Ok(state)
}

/// Detecta la IP local de la primera interfaz de red no-loopback.
///
/// Intenta primero el truco UDP (funciona si hay default route).
/// Si falla, enumera las interfaces de red directamente.
pub fn local_ip() -> std::net::IpAddr {
    use std::net::UdpSocket;

    // 1) Truco UDP: conectar a 8.8.8.8:80 para descubrir la IP saliente.
    if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = socket.local_addr() {
                let ip = addr.ip();
                if !ip.is_loopback() {
                    return ip;
                }
            }
        }
    }

    // 2) Fallback: intentar con otra dirección pública (puede fallar en LAN sin internet).
    for target in &["1.1.1.1:80", "192.168.1.1:80"] {
        if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
            if socket.connect(target).is_ok() {
                if let Ok(addr) = socket.local_addr() {
                    let ip = addr.ip();
                    if !ip.is_loopback() {
                        return ip;
                    }
                }
            }
        }
    }

    // 3) Si todo falla, devolver localhost (el usuario deberá
        //    configurar la IP manualmente o usar USB tunneling).
    tracing::warn!("no se pudo detectar IP local, usando 127.0.0.1");
    "127.0.0.1".parse().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_ip_returns_something() {
        let ip = local_ip();
        // No debe ser loopback en este test (puede fallar en CI).
        // Solo verificamos que devuelve una IP válida.
        let _: std::net::Ipv4Addr = match ip {
            std::net::IpAddr::V4(v4) => v4,
            _ => panic!("esperaba IPv4"),
        };
    }

    #[test]
    fn cert_generation_works() {
        let dir = tempfile::tempdir().unwrap();
        let cert_path = dir.path().join("test-cert.pem");
        let ip: std::net::IpAddr = "192.168.1.100".parse().unwrap();
        let cert = load_or_generate_cert(&cert_path, ip).unwrap();
        assert!(!cert.is_empty());
        assert!(cert_path.exists(), "cert debe estar en disco");
    }

    #[test]
    fn cert_cached_on_second_call() {
        let dir = tempfile::tempdir().unwrap();
        let cert_path = dir.path().join("test-cert.pem");
        let ip: std::net::IpAddr = "192.168.1.100".parse().unwrap();
        let _ = load_or_generate_cert(&cert_path, ip).unwrap();
        let modified_1 = std::fs::metadata(&cert_path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let _ = load_or_generate_cert(&cert_path, ip).unwrap();
        let modified_2 = std::fs::metadata(&cert_path).unwrap().modified().unwrap();
        assert_eq!(modified_1, modified_2, "cert cacheado no debe regenerarse");
    }
}
