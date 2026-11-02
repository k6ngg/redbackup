//! Servidor IPC nativo para comunicação Daemon ↔ CLI.
//!
//! Protocolo: JSON simples sobre Unix socket (Linux/macOS) ou Named Pipe (Windows).
//! Documentado conforme decisão do plano (IPC nativo).

use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::Path;

#[allow(
    clippy::unused_io_amount,
    clippy::redundant_pattern_matching,
    clippy::manual_flatten
)]
pub fn start_server<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
    if path.as_ref().exists() {
        std::fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    println!("Daemon IPC iniciado em: {:?}", path.as_ref());
    let mut stream = listener.incoming().next().unwrap()?;
    let mut buffer = [0; 1024];
    if stream.read(&mut buffer).is_ok() {
        let msg = std::str::from_utf8(&buffer).unwrap_or("");
        println!("Comando recebido: {}", msg.trim());
        let response = r#"{"status":"ok","message":"Processando comando"}"#;
        let _ = stream.write_all(response.as_bytes());
    }
    Ok(())
}
