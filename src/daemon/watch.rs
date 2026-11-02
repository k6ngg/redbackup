//! Loop de monitoramento com debounce.
//!
//! O callback do `notify` apenas envia eventos para um canal. Este loop
//! consolida eventos e só executa o backup após um período de calmaria (ou
//! quando a fila atinge o limite), evitando processamento pesado no callback.

use crate::backup;
use crate::config::settings::Settings;
use crate::daemon::queue::EventQueue;
use crate::daemon::watcher::start_watcher;
use anyhow::{Context, Result};
use std::sync::mpsc;
use std::time::Duration;

pub fn run(
    repo_root: &std::path::Path,
    settings: &Settings,
    key: &[u8; 32],
    debounce_seconds: u64,
) -> Result<()> {
    let (sender, receiver) = mpsc::channel();
    let _watcher = start_watcher(&settings.source_path, sender)
        .context("Falha ao iniciar watcher do sistema de arquivos")?;
    let queue = EventQueue::new(10_000);
    let debounce = Duration::from_secs(debounce_seconds.max(1));

    println!(
        "Monitorando {} (debounce: {}s). Use Ctrl+C para encerrar.",
        settings.source_path,
        debounce.as_secs()
    );

    loop {
        match receiver.recv_timeout(debounce) {
            Ok(event) => {
                if !queue.enqueue(format!("{:?}", event)) {
                    eprintln!(
                        "Fila de eventos cheia; executando backup e consolidando uma nova varredura."
                    );
                    run_once(repo_root, settings, key);
                    let _ = queue.dequeue_all();
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if !queue.is_empty() {
                    let events = queue.dequeue_all();
                    println!("{} evento(s) recebido(s); iniciando backup.", events.len());
                    run_once(repo_root, settings, key);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                println!("Watcher encerrado.");
                return Ok(());
            }
        }
    }
}

fn run_once(repo_root: &std::path::Path, settings: &Settings, key: &[u8; 32]) {
    match backup::run_backup(repo_root, settings, key) {
        Ok(report) => {
            println!(
                "Backup automático: snapshot {} status `{}` ({} arquivo(s), {} byte(s)).",
                report.snapshot_id, report.status, report.files_completed, report.bytes_total
            );
            for error in report.errors {
                eprintln!("ARQUIVO FALHOU [{}]: {}", error.path, error.message);
            }
        }
        Err(error) => eprintln!("Backup automático falhou: {}", error),
    }
}
