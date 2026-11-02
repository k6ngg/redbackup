//! Pruning (retenção) e exclusão de chunks não referenciados.
//!
//! Regras de segurança:
//! - Nunca remover chunks enquanto existir backup em andamento.
//! - Recalcular `reference_count` a partir de `file_chunks` após remover
//!   snapshots, em vez de confiar em decrementos cegos.
//! - Excluir o arquivo físico antes de remover a linha do banco. Se a exclusão
//!   falhar, o banco continua descrevendo um chunk sem referências, o que é
//!   recuperável em uma próxima execução.

use crate::metadata::repository::{
    chunk_actual_references, connect_or_create, count_in_progress, recalculate_reference_counts,
};
use crate::repository::lock::RepositoryLock;
use crate::repository::{chunk_path, index_db_path};
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct PruneReport {
    pub snapshots_removed: usize,
    pub chunks_removed: usize,
    pub bytes_freed: u64,
    pub deferred: bool,
    pub errors: Vec<String>,
}

/// Executa retenção e limpeza de chunks órfãos para um repositório.
pub fn prune_repository(repo_root: &Path, retention_days: u64) -> Result<PruneReport> {
    let _lock = RepositoryLock::exclusive(repo_root)?;
    let db_path = index_db_path(repo_root);
    let mut conn = connect_or_create(
        db_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Caminho do banco não é UTF-8"))?,
    )?;
    let snapshots_removed = prune_expired_conn(&mut conn, retention_days)?;
    let (removed_hashes, bytes_freed, deferred, errors) =
        remove_unreferenced_conn(&conn, repo_root)?;
    Ok(PruneReport {
        snapshots_removed,
        chunks_removed: removed_hashes.len(),
        bytes_freed,
        deferred,
        errors,
    })
}

/// Remove snapshots concluídos/falhos mais antigos que `retention_days`.
///
/// Retorna a quantidade de snapshots removidos. Implementação de compatibilidade
/// que abre o banco a partir do caminho informado.
pub fn prune_expired_snapshots(db_path: &str, retention_days: u64) -> Result<()> {
    let repo_root = Path::new(db_path)
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| {
            anyhow::anyhow!("db_path não está em <repositório>/metadata: {}", db_path)
        })?;
    let _lock = RepositoryLock::exclusive(repo_root)?;
    let mut conn = connect_or_create(db_path)?;
    prune_expired_conn(&mut conn, retention_days)?;
    Ok(())
}

fn prune_expired_conn(conn: &mut Connection, retention_days: u64) -> Result<usize> {
    let tx = conn
        .transaction()
        .context("Falha ao abrir transação de pruning")?;
    let cutoff = format!("-{} days", retention_days);

    let snapshot_ids: Vec<i64> = {
        let mut stmt = tx.prepare(
            "SELECT id FROM snapshots
             WHERE status != 'in-progress'
               AND datetime(timestamp) < datetime('now', ?1)
             ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![cutoff], |row| row.get(0))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        ids
    };

    for snapshot_id in &snapshot_ids {
        tx.execute("DELETE FROM snapshots WHERE id = ?1", params![snapshot_id])?;
    }

    // A fonte de verdade é `file_chunks`; recalcular evita contagens negativas
    // ou incrementos duplicados por falhas parciais.
    recalculate_reference_counts(&tx)?;
    tx.commit().context("Falha ao confirmar pruning")?;
    Ok(snapshot_ids.len())
}

/// Remove arquivos físicos de chunks com `reference_count == 0`.
///
/// Retorna os hashes removidos. Implementação de compatibilidade.
pub fn remove_unreferenced_chunks(data_dir: &str, db_path: &str) -> Result<Vec<String>> {
    let root = Path::new(data_dir)
        .parent()
        .ok_or_else(|| anyhow::anyhow!("data_dir não possui diretório pai: {}", data_dir))?;
    let _lock = RepositoryLock::exclusive(root)?;
    let conn = connect_or_create(db_path)?;
    let (removed, _bytes, _deferred, errors) = remove_unreferenced_conn(&conn, root)?;
    if !errors.is_empty() {
        anyhow::bail!("{}", errors.join("; "));
    }
    Ok(removed)
}

fn remove_unreferenced_conn(
    conn: &Connection,
    repo_root: &Path,
) -> Result<(Vec<String>, u64, bool, Vec<String>)> {
    if count_in_progress(conn)? > 0 {
        return Ok((Vec::new(), 0, true, Vec::new()));
    }

    let candidates: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT hash_id FROM chunks
             WHERE reference_count <= 0
             ORDER BY hash_id ASC",
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let mut hashes = Vec::new();
        for row in rows {
            hashes.push(row?);
        }
        hashes
    };

    let mut removed = Vec::new();
    let mut bytes_freed = 0u64;
    let mut errors = Vec::new();

    for hash_id in candidates {
        // Confirma na fonte de verdade antes de qualquer exclusão.
        let actual = chunk_actual_references(conn, &hash_id)?;
        if actual > 0 {
            conn.execute(
                "UPDATE chunks SET reference_count = ?1 WHERE hash_id = ?2",
                params![actual, hash_id],
            )?;
            continue;
        }

        let path = match chunk_path(repo_root, &hash_id) {
            Ok(path) => path,
            Err(error) => {
                errors.push(error.to_string());
                continue;
            }
        };

        if path.exists() {
            match fs::metadata(&path) {
                Ok(metadata) => match fs::remove_file(&path) {
                    Ok(()) => bytes_freed += metadata.len(),
                    Err(error) => {
                        errors.push(format!(
                            "falha ao remover chunk {} ({}): {}",
                            hash_id,
                            path.display(),
                            error
                        ));
                        // Não remova a linha do banco: ela ainda descreve o
                        // arquivo que permanece no disco.
                        continue;
                    }
                },
                Err(error) => {
                    errors.push(format!(
                        "falha ao ler chunk {} ({}): {}",
                        hash_id,
                        path.display(),
                        error
                    ));
                    continue;
                }
            }
        }

        let deleted = conn.execute(
            "DELETE FROM chunks
             WHERE hash_id = ?1
               AND reference_count <= 0
               AND NOT EXISTS (
                   SELECT 1 FROM file_chunks WHERE file_chunks.chunk_hash = chunks.hash_id
               )",
            params![hash_id],
        )?;
        if deleted > 0 {
            removed.push(hash_id);
        }
    }

    Ok((removed, bytes_freed, false, errors))
}
