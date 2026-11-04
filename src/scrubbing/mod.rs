//! Scrubbing (verificação de integridade) dos chunks armazenados.
//!
//! Regra: descriptografa, descomprime, recalcula BLAKE3 e compara com `hash_id`.
//! Falhas são diagnosticadas sem expor segredos, registradas no banco e nunca
//! apagam ou alteram os dados automaticamente.

use crate::metadata::migrations::run_migrations;
use crate::metadata::repository::{
    clear_chunk_corruption, connect_or_create, list_chunk_hashes, mark_chunk_corrupted,
};
use crate::pipeline::verify::validate_stored_chunk;
use crate::repository::key_binding;
use crate::repository::lock::RepositoryLock;
use crate::repository::{chunk_path, index_db_path};
use anyhow::{Context, Result};
use rand::seq::SliceRandom;
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct ScrubReport {
    pub checked: usize,
    pub corrupted: Vec<ScrubProblem>,
}

#[derive(Debug, Clone)]
pub struct ScrubProblem {
    pub hash_id: String,
    pub reason: String,
}

/// Verifica um chunk isolado.
///
/// Delega ao pipeline para que backup, scrubbing e demais fluxos compartilhem a
/// mesma validação de autenticação, descompressão e hash.
pub fn scrub_chunk(chunk_path: &Path, key: &[u8; 32], expected_hash: &str) -> Result<bool> {
    validate_stored_chunk(chunk_path, key, expected_hash)
}

/// Verifica todos os chunks (ou uma amostra aleatória) do repositório.
pub fn scrub_repository(
    repo_root: &Path,
    key: &[u8; 32],
    sample: Option<usize>,
) -> Result<ScrubReport> {
    let _lock = RepositoryLock::shared(repo_root)?;
    let db_path = index_db_path(repo_root);
    let conn = connect_or_create(
        db_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Caminho do banco não é UTF-8"))?,
    )?;
    run_migrations(&conn).context("Falha ao preparar schema para scrubbing")?;
    // Uma chave diferente não deve gerar falsos diagnósticos de corrupção.
    key_binding::require_matching_key_if_bound(&conn, key)?;

    let mut hashes = list_chunk_hashes(&conn)?;
    if let Some(limit) = sample {
        if limit < hashes.len() {
            hashes.shuffle(&mut rand::thread_rng());
            hashes.truncate(limit);
        }
    }

    let mut report = ScrubReport::default();
    for hash_id in hashes {
        report.checked += 1;
        let problem = match chunk_path(repo_root, &hash_id) {
            Err(error) => Some(error.to_string()),
            Ok(path) if !path.is_file() => {
                Some(format!("arquivo físico ausente: {}", path.display()))
            }
            Ok(path) => match scrub_chunk(&path, key, &hash_id) {
                Ok(true) => None,
                Ok(false) => Some("hash BLAKE3 recalculado não confere".to_string()),
                Err(error) => Some(format!("falha ao validar chunk: {}", error)),
            },
        };

        match problem {
            Some(reason) => {
                mark_chunk_corrupted(&conn, &hash_id, &reason)?;
                report.corrupted.push(ScrubProblem { hash_id, reason });
            }
            None => clear_chunk_corruption(&conn, &hash_id)?,
        }
    }
    Ok(report)
}
