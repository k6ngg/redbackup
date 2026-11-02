//! Servidor IPC nativo do daemon.
//!
//! Em Unix usa `UnixListener`; cada conexão transporta uma linha JSON. O
//! servidor atende conexões com concorrência limitada e compartilha a mesma engine usada
//! pela CLI local (`backup`, `prune`, etc.), evitando duplicação de lógica.

use crate::backup;
use crate::config::settings::Settings;
use crate::daemon::protocol::{Request, Response};
use crate::metadata::repository::{connect_or_create, list_snapshots};
use crate::repository::{index_db_path, socket_path};
use crate::retention;
use crate::scrubbing;
use anyhow::{Context, Result};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[cfg(unix)]
pub fn serve(repo_root: &Path, key: &[u8; 32]) -> Result<()> {
    let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _signals = super::lifecycle::Signals::new(shutdown.clone())?;
    serve_until(repo_root, key, shutdown)
}

#[cfg(unix)]
pub fn serve_until(
    repo_root: &Path,
    key: &[u8; 32],
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    use std::os::unix::net::{UnixListener, UnixStream};

    let settings = Settings::load(repo_root)?;
    use fs2::FileExt;
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::Arc;
    let instance = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(crate::repository::metadata_dir(repo_root).join("daemon.lock"))?;
    instance
        .try_lock_exclusive()
        .context("Já existe um daemon para este repositório")?;
    let path = socket_path(repo_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() {
        if UnixStream::connect(&path).is_ok() {
            anyhow::bail!(
                "Já existe um daemon em execução no socket {}",
                path.display()
            );
        }
        fs::remove_file(&path)
            .with_context(|| format!("Falha ao remover socket antigo: {}", path.display()))?;
    }

    let listener = UnixListener::bind(&path)
        .with_context(|| format!("Falha ao criar socket IPC: {}", path.display()))?;
    let _socket_cleanup = SocketCleanup(path.clone());
    let mut perms = fs::metadata(&path)?.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(&path, perms)?;

    println!("redbackup daemon ouvindo em {}", path.display());
    println!("Origem: {}", settings.source_path);
    println!("Use Ctrl+C para encerrar (ou envie o comando `shutdown`).");

    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let dirty = Arc::new(AtomicBool::new(true));
    let _watcher = super::watch::observe(repo_root, &settings, dirty.clone())?;
    let active = AtomicUsize::new(0);
    listener.set_nonblocking(true)?;
    std::thread::scope(|scope| {
        let worker_stop = &shutdown;
        let worker_settings = &settings;
        let worker_dirty = &dirty;
        scope.spawn(move || {
            super::watch::worker(
                repo_root,
                worker_settings,
                key,
                5,
                worker_dirty,
                worker_stop,
            )
        });
        while !shutdown.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _)) => {
                    if active.load(Ordering::Acquire) >= 16 {
                        drop(stream);
                        continue;
                    }
                    active.fetch_add(1, Ordering::AcqRel);
                    let active = &active;
                    let shutdown = &shutdown;
                    let settings = &settings;
                    scope.spawn(move || {
                        match handle_connection(stream, repo_root, settings, key, shutdown) {
                            Ok(true) => shutdown.store(true, Ordering::Release),
                            Ok(false) => {}
                            Err(error) => eprintln!("ERRO IPC: {:#}", error),
                        }
                        active.fetch_sub(1, Ordering::AcqRel);
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(error) => {
                    eprintln!("ERRO IPC: {}", error);
                    shutdown.store(true, Ordering::Release);
                    break;
                }
            }
        }
    });

    if shutdown.load(Ordering::Acquire) {
        println!("Daemon encerrado.");
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn serve(_repo_root: &Path, _key: &[u8; 32]) -> Result<()> {
    anyhow::bail!("O daemon IPC nativo requer Unix socket (Linux/macOS)")
}

#[cfg(unix)]
fn handle_connection(
    stream: std::os::unix::net::UnixStream,
    repo_root: &Path,
    settings: &Settings,
    key: &[u8; 32],
    stopping: &std::sync::atomic::AtomicBool,
) -> Result<bool> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?).take(64 * 1024 + 1);
    let mut line = String::new();
    let read = reader.read_line(&mut line)?;
    if read == 0 {
        return Ok(false);
    }
    if line.len() > 64 * 1024 {
        write_response(stream, &Response::error("Comando IPC excede 64 KiB"))?;
        return Ok(false);
    }

    let request: Request = match serde_json::from_str(&line) {
        Ok(request) => request,
        Err(error) => {
            write_response(
                stream,
                &Response::error(format!("JSON inválido: {}", error)),
            )?;
            return Ok(false);
        }
    };

    if stopping.load(std::sync::atomic::Ordering::Acquire)
        && !matches!(request.command.as_str(), "ping" | "status" | "shutdown")
    {
        write_response(
            stream,
            &Response::error("Daemon encerrando; nova tarefa recusada"),
        )?;
        return Ok(false);
    }
    let (response, shutdown) = match request.command.as_str() {
        "ping" => (Response::ok("pong"), false),
        "status" => (
            match crate::status::repository_status(repo_root) {
                Ok(data) => Response::ok("Status do repositório").with_data(data),
                Err(error) => Response::error(format!("Status indisponível: {:#}", error)),
            },
            false,
        ),
        "backup" => match backup::run_backup(repo_root, settings, key) {
            Ok(report) => {
                let status = if report.status == "completed" {
                    "ok"
                } else {
                    "error"
                };
                let message = format!(
                    "snapshot {}: {} arquivo(s), {} concluído(s), {} falha(s), {} byte(s), {} chunk(s) novo(s), {} reutilizado(s)",
                    report.snapshot_id,
                    report.files_total,
                    report.files_completed,
                    report.files_failed,
                    report.bytes_total,
                    report.chunks_new,
                    report.chunks_reused
                );
                let data = serde_json::json!({
                    "snapshot_id": report.snapshot_id,
                    "status": report.status,
                    "files_total": report.files_total,
                    "files_completed": report.files_completed,
                    "files_failed": report.files_failed,
                    "bytes_total": report.bytes_total,
                    "chunks_new": report.chunks_new,
                    "chunks_reused": report.chunks_reused,
                    "errors": report.errors,
                });
                (
                    Response {
                        status: status.to_string(),
                        message,
                        data: Some(data),
                    },
                    false,
                )
            }
            Err(error) => (Response::error(format!("Backup falhou: {}", error)), false),
        },
        "list-snapshots" => match snapshot_listing(repo_root) {
            Ok(message) => (Response::ok(message), false),
            Err(error) => (Response::error(error.to_string()), false),
        },
        "prune" => {
            let days = request
                .args
                .get("days")
                .and_then(|value| value.as_u64())
                .unwrap_or(settings.retention_days);
            match retention::prune_repository(repo_root, days) {
                Ok(report) => (
                    Response::ok(format!(
                        "{} snapshot(s) removido(s), {} chunk(s) removido(s), {} byte(s) liberados",
                        report.snapshots_removed, report.chunks_removed, report.bytes_freed
                    )),
                    false,
                ),
                Err(error) => (Response::error(format!("Pruning falhou: {}", error)), false),
            }
        }
        "scrub" => match scrubbing::scrub_repository(repo_root, key, None) {
            Ok(report) => {
                let status = if report.corrupted.is_empty() {
                    "ok"
                } else {
                    "error"
                };
                let message = format!(
                    "{} chunk(s) verificado(s), {} corrompido(s)",
                    report.checked,
                    report.corrupted.len()
                );
                (
                    Response {
                        status: status.to_string(),
                        message,
                        data: None,
                    },
                    false,
                )
            }
            Err(error) => (
                Response::error(format!("Scrubbing falhou: {}", error)),
                false,
            ),
        },
        "shutdown" => (Response::ok("Encerrando daemon"), true),
        other => (
            Response::error(format!("Comando desconhecido: {}", other)),
            false,
        ),
    };

    write_response(stream, &response)?;
    Ok(shutdown)
}

#[cfg(unix)]
fn write_response(mut stream: std::os::unix::net::UnixStream, response: &Response) -> Result<()> {
    let payload = serde_json::to_vec(response).context("Falha ao serializar resposta IPC")?;
    stream.write_all(&payload)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

fn snapshot_listing(repo_root: &Path) -> Result<String> {
    let db_path = index_db_path(repo_root);
    let conn = connect_or_create(
        db_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Caminho do banco não é UTF-8"))?,
    )?;
    let snapshots = list_snapshots(&conn)?;
    if snapshots.is_empty() {
        return Ok("Nenhum snapshot encontrado.".to_string());
    }
    let mut lines = Vec::new();
    for snapshot in snapshots {
        lines.push(format!(
            "{}  {}  {}  {} arquivo(s)  {} byte(s)  {}",
            snapshot.id,
            snapshot.timestamp,
            snapshot.status,
            snapshot.file_count,
            snapshot.total_bytes,
            snapshot.source_path
        ));
    }
    Ok(lines.join("\n"))
}

#[cfg(unix)]
struct SocketCleanup(std::path::PathBuf);
#[cfg(unix)]
impl Drop for SocketCleanup {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.0) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!("Falha ao remover socket: {}", error);
            }
        }
    }
}
