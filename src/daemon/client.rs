//! Cliente IPC para o daemon local.

use crate::daemon::protocol::{Request, Response};
use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::net::UnixStream;

#[cfg(unix)]
pub fn send_command(socket_path: &Path, request: &Request) -> Result<Response> {
    send_command_with_timeout(socket_path, request, Duration::from_secs(3600))
}

#[cfg(unix)]
pub fn send_command_with_timeout(
    socket_path: &Path,
    request: &Request,
    timeout: Duration,
) -> Result<Response> {
    let mut stream = UnixStream::connect(socket_path).with_context(|| {
        format!(
            "Não foi possível conectar ao daemon em {}",
            socket_path.display()
        )
    })?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout.min(Duration::from_secs(30))))?;

    let payload = serde_json::to_vec(request).context("Falha ao serializar comando IPC")?;
    stream
        .write_all(&payload)
        .context("Falha ao enviar comando ao daemon")?;
    stream
        .write_all(b"\n")
        .context("Falha ao finalizar comando IPC")?;
    stream.flush().context("Falha ao enviar comando IPC")?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .context("Falha ao ler resposta do daemon")?;
    if line.trim().is_empty() {
        anyhow::bail!("Daemon encerrou a conexão sem resposta");
    }
    serde_json::from_str(&line).context("Resposta IPC inválida")
}

#[cfg(not(unix))]
pub fn send_command(_socket_path: &Path, _request: &Request) -> Result<Response> {
    anyhow::bail!("IPC nativo via Unix socket não está disponível nesta plataforma")
}

/// Testa se há um daemon atendendo no socket.
pub fn is_available(socket_path: &Path) -> bool {
    send_command(socket_path, &Request::new("ping")).is_ok()
}
