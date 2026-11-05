//! Recuperação após falha (crash recovery).
//!
//! Na inicialização/manutenção:
//! - remove arquivos `.tmp_*`, que nunca são nomes finais de chunk;
//! - marca snapshots `in-progress` como `failed`;
//! - remove chunks físicos sem referência, mas somente depois de consultar o
//!   banco e confirmar que não há operação em andamento.

mod repair;

use crate::metadata::repository::{
    chunk_actual_references, clear_chunk_corruption, connect_or_create, count_in_progress,
    fail_stale_in_progress_snapshots, mark_chunk_corrupted, verify_snapshot_integrity,
};
use crate::pipeline::verify::validated_chunk_size;
use crate::repository::key_binding;
use crate::repository::lock::RepositoryLock;
use crate::repository::{chunk_path, data_dir, index_db_path, validate_hash_id};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct RecoveryReport {
    pub removed: Vec<String>,
    pub stale_snapshots_failed: usize,
    pub repaired: Vec<RepairedChunk>,
    pub unrecoverable: Vec<RecoveryProblem>,
}

#[derive(Debug, Clone)]
pub struct RepairedChunk {
    pub hash: String,
    pub preserved_original: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct RecoveryProblem {
    pub hash: String,
    pub reason: String,
}

/// Recupera um repositório inteiro.
pub fn recover_repository(repo_root: &Path, key: &[u8; 32]) -> Result<RecoveryReport> {
    let _lock = RepositoryLock::exclusive(repo_root)?;
    let db_path = index_db_path(repo_root);
    if !db_path.is_file() {
        anyhow::bail!("Banco de metadados ausente: {}", db_path.display());
    }
    let mut conn = connect_or_create(
        db_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Caminho do banco não é UTF-8"))?,
    )?;
    key_binding::require_matching_key_if_bound(&conn, key)?;
    // Sem verificador não é possível distinguir corrupção de chave errada.
    // Não destrua o status de backups antigos com base em uma suposição.
    if key_binding::stored_verifier(&conn)?.is_none() {
        let has_references: bool =
            conn.query_row("SELECT EXISTS(SELECT 1 FROM file_chunks)", [], |row| {
                row.get(0)
            })?;
        if has_references {
            anyhow::bail!("Repositório antigo sem chave vinculada: não é possível distinguir chave incorreta de corrupção; não foi feita nenhuma alteração");
        }
    }
    let completed: Vec<i64> = {
        let mut stmt = conn.prepare("SELECT id FROM snapshots WHERE status = 'completed'")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let source = crate::redundancy::load(repo_root);
    let chunks: Vec<(String, i64)> = {
        let mut stmt = conn.prepare("SELECT DISTINCT c.hash_id,c.size FROM chunks c JOIN file_chunks fc ON fc.chunk_hash=c.hash_id JOIN files f ON f.id=fc.file_id JOIN snapshots s ON s.id=f.snapshot_id WHERE s.status IN ('completed','failed')")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut validation: HashMap<String, Option<String>> = HashMap::new();
    let mut repaired = Vec::new();
    let mut unrecoverable = Vec::new();
    for (hash, size) in chunks {
        let issue = match chunk_path(repo_root, &hash)
            .and_then(|path| validated_chunk_size(&path, key, &hash))
        {
            Ok(Some(actual)) if i64::try_from(actual).ok() == Some(size) => {
                // Uma execução anterior pode ter terminado após o rename e antes
                // do fsync do diretório; confirme a durabilidade antes de aceitar.
                crate::pipeline::write::sync_existing_file(&chunk_path(repo_root, &hash)?)?;
                None
            }
            Ok(Some(_)) => Some("tamanho do chunk não corresponde aos metadados".to_string()),
            Ok(None) => Some("hash BLAKE3 não confere".to_string()),
            Err(error) => Some(format!("falha ao validar chunk: {:#}", error)),
        };
        let problem = if let Some(reason) = issue {
            let result = match &source {
                Ok(Some(config)) => repair::repair(repo_root, config, &hash, size, key),
                Ok(None) => Err(anyhow::anyhow!("redundância não configurada")),
                Err(error) => Err(anyhow::anyhow!(
                    "configuração de redundância inválida: {:#}",
                    error
                )),
            };
            match result {
                Ok(preserved_original) => {
                    repaired.push(RepairedChunk {
                        hash: hash.clone(),
                        preserved_original,
                    });
                    None
                }
                Err(error) => {
                    let reason = format!("{}; reparo indisponível: {:#}", reason, error);
                    unrecoverable.push(RecoveryProblem {
                        hash: hash.clone(),
                        reason: reason.clone(),
                    });
                    Some(reason)
                }
            }
        } else {
            None
        };
        validation.insert(hash, problem);
    }
    let mut invalid_snapshots = Vec::new();
    for id in completed {
        let mut problems = verify_snapshot_integrity(&conn, id, repo_root)?;
        let mut stmt = conn.prepare("SELECT DISTINCT fc.chunk_hash FROM file_chunks fc JOIN files f ON f.id=fc.file_id WHERE f.snapshot_id=?1")?;
        for hash in stmt.query_map([id], |row| row.get::<_, String>(0))? {
            let hash = hash?;
            if let Some(Some(reason)) = validation.get(&hash) {
                problems.push(format!("chunk {}: {}", hash, reason));
            }
        }
        if !problems.is_empty() {
            eprintln!(
                "AVISO: snapshot {} marcado como falho: {}",
                id,
                problems.join("; ")
            );
            invalid_snapshots.push(id);
        }
    }
    let tx = conn.transaction()?;
    let mut stale_snapshots_failed = fail_stale_in_progress_snapshots(&tx)?;
    for id in invalid_snapshots {
        tx.execute("UPDATE snapshots SET status = 'failed' WHERE id = ?1", [id])?;
        stale_snapshots_failed += 1;
    }
    for (hash, problem) in validation {
        if let Some(reason) = problem {
            mark_chunk_corrupted(&tx, &hash, &reason)?;
        } else {
            clear_chunk_corruption(&tx, &hash)?;
        }
    }
    tx.execute("UPDATE chunks SET reference_count = (SELECT COUNT(*) FROM file_chunks WHERE chunk_hash = chunks.hash_id)", [])?;
    tx.commit()?;

    let removed = clean_orphans_locked(
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
        repaired,
        unrecoverable,
    })
}

/// Remove arquivos temporários e chunks sem referência.
///
/// Se o banco não puder ser aberto, apenas arquivos `.tmp_*` são removidos;
/// chunks finais nunca são apagados sem verificação do banco.
pub fn clean_orphans(data_dir: &str, db_path: &str) -> Result<Vec<String>> {
    let root = Path::new(data_dir).parent().unwrap_or(Path::new(data_dir));
    let _lock = RepositoryLock::exclusive(root)?;
    clean_orphans_locked(data_dir, db_path)
}

fn clean_orphans_locked(data_dir: &str, db_path: &str) -> Result<Vec<String>> {
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
                if chunk_actual_references(&conn, name)? == 0 {
                    fs::remove_file(&path).with_context(|| {
                        format!("Falha ao remover chunk órfão: {}", path.display())
                    })?;
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
                {
                    fs::remove_file(&path).with_context(|| {
                        format!("Falha ao remover chunk órfão: {}", path.display())
                    })?;
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

#[cfg(test)]
mod repair_tests {
    use super::*;
    use crate::{
        backup::run_backup,
        config::settings::Settings,
        metadata::repository::list_chunk_hashes,
        redundancy::{self, Mode},
    };
    use std::process::Command;
    const KEY: [u8; 32] = [71; 32];

    #[test]
    fn child_repair() {
        let Ok(root) = std::env::var("REDBACKUP_REPAIR_TEST_ROOT") else {
            return;
        };
        recover_repository(&Path::new(&root).join("repo"), &KEY).unwrap();
        panic!("ponto de interrupção não alcançado");
    }

    #[test]
    fn reparo_interrompido_pode_ser_repetido() {
        for point in [
            "repair_before_preserve",
            "repair_before_publish",
            "repair_after_publish",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            let source = root.join("source");
            let repo = root.join("repo");
            let dest = root.join("redundancy");
            fs::create_dir(&source).unwrap();
            fs::create_dir(&dest).unwrap();
            fs::write(source.join("file"), b"dados recuperaveis").unwrap();
            let settings = Settings {
                source_path: source.display().to_string(),
                destination_path: repo.display().to_string(),
                chunk_min_size: 64,
                chunk_max_size: 1024,
                ..Settings::default()
            };
            run_backup(&repo, &settings, &KEY).unwrap();
            settings.save(&repo).unwrap();
            redundancy::configure(&repo, &settings, &dest, Mode::Required).unwrap();
            redundancy::sync_repository(&repo, &KEY).unwrap();
            let conn = connect_or_create(index_db_path(&repo).to_str().unwrap()).unwrap();
            let hash = list_chunk_hashes(&conn).unwrap()[0].clone();
            let path = chunk_path(&repo, &hash).unwrap();
            let expected = fs::read(&path).unwrap();
            fs::write(&path, b"danificado").unwrap();
            drop(conn);
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "recovery::repair_tests::child_repair"])
                .env("REDBACKUP_REPAIR_TEST_ROOT", root)
                .env("REDBACKUP_TEST_CRASH_POINT", point)
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(86),
                "{}: {}",
                point,
                String::from_utf8_lossy(&output.stderr)
            );
            for _ in 0..2 {
                let report = recover_repository(&repo, &KEY).unwrap();
                assert!(report.unrecoverable.is_empty());
                assert_eq!(fs::read(&path).unwrap(), expected);
                let conn = connect_or_create(index_db_path(&repo).to_str().unwrap()).unwrap();
                let status: String = conn
                    .query_row("SELECT status FROM snapshots WHERE id=1", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(status, "completed");
            }
            let quarantine = repo.join("metadata/quarantine");
            assert!(fs::read_dir(quarantine)
                .unwrap()
                .any(|entry| fs::read(entry.unwrap().path()).unwrap() == b"danificado"));
        }
    }
}
