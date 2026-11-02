//! Recuperação após falha (crash recovery).
//!
//! Na inicialização/manutenção:
//! - remove arquivos `.tmp_*`, que nunca são nomes finais de chunk;
//! - marca snapshots `in-progress` como `failed`;
//! - remove chunks físicos sem referência, mas somente depois de consultar o
//!   banco e confirmar que não há operação em andamento.

use crate::metadata::repository::{
    chunk_actual_references, connect_or_create, count_in_progress, fail_stale_in_progress_snapshots,
};
use crate::repository::lock::RepositoryLock;
use crate::repository::{data_dir, index_db_path, validate_hash_id};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct RecoveryReport {
    pub removed: Vec<String>,
    pub stale_snapshots_failed: usize,
}

/// Recupera um repositório inteiro.
pub fn recover_repository(repo_root: &Path) -> Result<RecoveryReport> {
    let _lock = RepositoryLock::exclusive(repo_root)?;
    let db_path = index_db_path(repo_root);
    let mut stale_snapshots_failed = 0usize;
    if db_path.exists() {
        let conn = connect_or_create(
            db_path
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Caminho do banco não é UTF-8"))?,
        )?;
        stale_snapshots_failed = fail_stale_in_progress_snapshots(&conn)?;
    }

    let removed = clean_orphans(
        data_dir(repo_root)
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Caminho de data não é UTF-8"))?,
        db_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Caminho do banco não é UTF-8"))?,
    )?;

    Ok(RecoveryReport {
        removed,
        stale_snapshots_failed,
    })
}

/// Remove arquivos temporários e chunks sem referência.
///
/// Se o banco não puder ser aberto, apenas arquivos `.tmp_*` são removidos;
/// chunks finais nunca são apagados sem verificação do banco.
pub fn clean_orphans(data_dir: &str, db_path: &str) -> Result<Vec<String>> {
    let data_root = Path::new(data_dir);
    let mut removed = Vec::new();
    let mut final_files = Vec::new();

    if !data_root.exists() {
        return Ok(removed);
    }

    collect_files(data_root, &mut removed, &mut final_files)?;

    // Chunks finais só são removidos com o banco disponível. Não crie um banco
    // vazio durante a recuperação: isso mascararia um repositório incompleto.
    if !Path::new(db_path).exists() {
        eprintln!(
            "AVISO: recuperação não verificou chunks finais (banco ausente: {})",
            db_path
        );
        return Ok(removed);
    }
    let conn = match connect_or_create(db_path) {
        Ok(conn) => Some(conn),
        Err(error) => {
            eprintln!(
                "AVISO: recuperação não verificou chunks finais (banco indisponível): {}",
                error
            );
            None
        }
    };

    let Some(conn) = conn else {
        return Ok(removed);
    };

    match count_in_progress(&conn) {
        Ok(0) => {}
        Ok(_) => {
            eprintln!("AVISO: há backup em andamento; chunks finais não serão removidos.");
            return Ok(removed);
        }
        Err(error) => {
            eprintln!(
                "AVISO: recuperação não verificou chunks finais (banco sem schema válido): {}",
                error
            );
            return Ok(removed);
        }
    }

    let repo_root = data_root.parent().unwrap_or(data_root);
    let mut known: HashMap<String, i64> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT hash_id, reference_count FROM chunks")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (hash, count) = row?;
            known.insert(hash, count);
        }
    }

    for path in final_files {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if validate_hash_id(name).is_err() {
            continue;
        }

        match known.get(name).copied() {
            None => {
                // Arquivo com nome de chunk, mas sem linha no banco: resíduo de
                // uma gravação interrompida antes do registro.
                if fs::remove_file(&path).is_ok() {
                    removed.push(display_relative(repo_root, &path));
                }
            }
            Some(reference_count) if reference_count <= 0 => {
                let actual = chunk_actual_references(&conn, name)?;
                if actual > 0 {
                    conn.execute(
                        "UPDATE chunks SET reference_count = ?1 WHERE hash_id = ?2",
                        rusqlite::params![actual, name],
                    )?;
                    continue;
                }
                if fs::remove_file(&path).is_ok() {
                    conn.execute(
                        "DELETE FROM chunks WHERE hash_id = ?1 AND reference_count <= 0",
                        rusqlite::params![name],
                    )?;
                    removed.push(display_relative(repo_root, &path));
                }
            }
            Some(_) => {}
        }
    }

    // Se uma linha do banco existe mas o arquivo físico sumiu, mantém o
    // registro para diagnóstico/scrubbing; não mascare a perda de dados.
    Ok(removed)
}

fn collect_files(
    dir: &Path,
    removed_tmp: &mut Vec<String>,
    final_files: &mut Vec<PathBuf>,
) -> Result<()> {
    let entries =
        fs::read_dir(dir).with_context(|| format!("Falha ao ler diretório: {}", dir.display()))?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            // Não siga symlinks: a limpeza nunca deve alcançar caminhos fora
            // de `data/`.
            continue;
        }
        if file_type.is_dir() {
            collect_files(&path, removed_tmp, final_files)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(".tmp_") || name.ends_with(".tmp") {
            fs::remove_file(&path)
                .with_context(|| format!("Falha ao remover órfão: {}", path.display()))?;
            removed_tmp.push(display_relative(dir, &path));
        } else {
            final_files.push(path);
        }
    }
    Ok(())
}

fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}
