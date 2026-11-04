//! Orquestração do backup de uma origem para o repositório.
//!
//! Cada arquivo é processado em fluxo, com no máximo um chunk em memória. Os
//! chunks físicos são gravados antes de qualquer metadado. A transação SQLite
//! que registra o arquivo, seus chunks e as referências é curta e só é aberta
//! depois que todos os chunks do arquivo estão persistidos.
//!
//! Quando o conteúdo é idêntico ao do último snapshot concluído, o snapshot em
//! andamento é descartado e o resultado recebe o status `unchanged`; nenhum
//! registro redundante é publicado.
//!
//! Um chunk físico só é reutilizado depois de autenticado, descomprimido e ter
//! seu hash BLAKE3 recalculado. Um arquivo existente mas corrompido nunca é
//! incluído silenciosamente em um snapshot.

use crate::config::settings::Settings;
use crate::config::validation::{relative_path_for_metadata, set_directory_permissions};
use crate::metadata::repository::{
    clear_chunk_corruption, connect_or_create, create_snapshot, discard_snapshot, finish_snapshot,
    get_chunk, insert_file, insert_file_chunk, mark_chunk_corrupted, previous_completed_snapshot,
    snapshots_have_same_content, upsert_chunk, verify_snapshot_integrity,
};
use crate::pipeline::{chunking, compress, crypto, hash, verify, write};
use crate::repository::lock::RepositoryLock;
use crate::repository::{chunk_path, index_db_path, initialize_repository, metadata_dir};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Erro isolado de um arquivo; não interrompe o restante do backup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileError {
    pub path: String,
    pub message: String,
}

/// Resultado de um backup.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BackupReport {
    pub snapshot_id: i64,
    pub files_total: usize,
    pub files_completed: usize,
    pub files_failed: usize,
    pub bytes_total: u64,
    pub chunks_new: usize,
    pub chunks_reused: usize,
    pub errors: Vec<FileError>,
    pub status: String,
}

struct WalkResult {
    files: Vec<PathBuf>,
    errors: Vec<FileError>,
}

struct ProcessedFile {
    relative_path: String,
    size: i64,
    permissions: Option<i64>,
    modification_time: Option<i64>,
    chunks: Vec<(String, u64, Option<u64>)>,
    bytes: u64,
    chunks_new: usize,
    chunks_reused: usize,
}

/// Executa um backup completo da origem configurada.
pub fn run_backup(repo_root: &Path, settings: &Settings, key: &[u8; 32]) -> Result<BackupReport> {
    settings.validate()?;
    let root = initialize_repository(
        repo_root
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Caminho do repositório não é UTF-8"))?,
    )?;
    // Exclusivo: dois backups não devem competir pelas mesmas referências nem
    // pela publicação de chunks.
    let _lock = RepositoryLock::exclusive(&root)?;
    let mut progress = crate::status::ProgressWriter::new(&root)?;
    let result = (|| {
        let source = Path::new(&settings.source_path)
            .canonicalize()
            .with_context(|| format!("Origem não encontrada: {}", settings.source_path))?;
        if !source.is_dir() {
            anyhow::bail!("A origem não é um diretório: {}", source.display());
        }
        // A origem nunca pode estar dentro do repositório (nem ser a própria raiz):
        // o walker exclui o repositório para não salvar os próprios chunks. Sem
        // esta checagem, o resultado seria um snapshot vazio marcado como
        // concluído.
        if source.starts_with(&root) {
            anyhow::bail!(
                "A origem não pode ser a raiz do repositório nem estar dentro dele: {}",
                source.display()
            );
        }

        set_directory_permissions(&metadata_dir(&root), 0o700)?;
        let db_path = index_db_path(&root);
        let mut conn = connect_or_create(
            db_path
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Caminho do banco não é UTF-8"))?,
        )?;

        let snapshot_id = create_snapshot(&conn, &source.to_string_lossy())?;
        progress.value.report.snapshot_id = snapshot_id;
        let result = backup_files(
            &mut conn,
            &root,
            &source,
            settings,
            key,
            snapshot_id,
            &mut progress,
        );

        match result {
            Ok(mut report) => {
                // A integridade é verificada depois que todos os arquivos foram
                // registrados; só então o snapshot pode ser publicado.
                progress.value.stage = "verifying".into();
                progress.value.report = report.clone();
                progress.publish(true);
                let problems = verify_snapshot_integrity(&conn, snapshot_id, &root)?;
                for problem in problems {
                    report.errors.push(FileError {
                        path: "<snapshot>".to_string(),
                        message: problem,
                    });
                }
                report.files_failed = report
                    .errors
                    .iter()
                    .filter(|e| e.path != "<snapshot>")
                    .count();
                // Um evento do watcher pode não alterar o conteúdo. Nesse caso,
                // descarta o snapshot em andamento em vez de publicar um registro
                // idêntico ao último concluído.
                if report.errors.is_empty() {
                    if let Some(previous) = previous_completed_snapshot(&conn)? {
                        if snapshots_have_same_content(&conn, snapshot_id, previous)? {
                            discard_snapshot(&mut conn, snapshot_id)?;
                            report.status = "unchanged".to_string();
                            report.snapshot_id = previous;
                            return Ok(report);
                        }
                    }
                }
                let status = if report.errors.is_empty() {
                    "completed"
                } else {
                    "failed"
                };
                finish_snapshot(&conn, snapshot_id, status)?;
                report.status = status.to_string();
                report.snapshot_id = snapshot_id;

                Ok(report)
            }
            Err(error) => {
                if let Err(finish_error) = finish_snapshot(&conn, snapshot_id, "failed") {
                    eprintln!(
                        "ERRO: não foi possível marcar o snapshot {} como falho: {}",
                        snapshot_id, finish_error
                    );
                }
                Err(error)
            }
        }
    })();
    progress.finish(&result);
    result
}

#[allow(clippy::too_many_arguments)]
fn backup_files(
    conn: &mut rusqlite::Connection,
    repo_root: &Path,
    source: &Path,
    settings: &Settings,
    key: &[u8; 32],
    snapshot_id: i64,
    progress: &mut crate::status::ProgressWriter,
) -> Result<BackupReport> {
    let mut report = BackupReport {
        snapshot_id,
        ..BackupReport::default()
    };
    let walk = walk_source(source, repo_root, progress)?;
    report.files_total = walk.files.len() + walk.errors.len();
    report.errors.extend(walk.errors);

    // Um chunk validado uma vez neste backup não precisa ser lido novamente
    // para cada ocorrência. O lock exclusivo impede que outro processo o
    // altere enquanto o backup está ativo.
    let mut validated_chunks = HashSet::new();
    progress.value.report = report.clone();
    progress.value.totals_known = true;
    progress.value.stage = "processing".into();
    progress.publish(true);
    for path in &walk.files {
        report.files_failed = report.errors.len();
        progress.value.stage = "processing".into();
        progress.value.report = report.clone();
        progress.value.current_file = Some(path.display().to_string());
        progress.value.current_file_bytes = 0;
        progress.publish(true);
        match process_file(
            conn,
            repo_root,
            source,
            path,
            settings,
            key,
            progress,
            &mut validated_chunks,
        ) {
            Ok(processed) => {
                progress.value.stage = "registering".into();
                progress.publish(false);
                match register_file(conn, snapshot_id, &processed) {
                    Ok(()) => {
                        report.files_completed += 1;
                        report.bytes_total += processed.bytes;
                        report.chunks_new += processed.chunks_new;
                        report.chunks_reused += processed.chunks_reused;
                    }
                    Err(error) => {
                        report.errors.push(FileError {
                            path: processed.relative_path,
                            message: format!("falha ao registrar metadados: {}", error),
                        });
                    }
                }
            }
            Err(error) => {
                let relative = relative_path_for_metadata(source, path)
                    .unwrap_or_else(|_| path.display().to_string());
                report.errors.push(FileError {
                    path: relative,
                    message: error.to_string(),
                });
            }
        }
    }

    report.files_failed = report.errors.len();
    progress.value.report = report.clone();
    progress.value.current_file = None;
    progress.publish(true);
    Ok(report)
}

fn register_file(
    conn: &mut rusqlite::Connection,
    snapshot_id: i64,
    processed: &ProcessedFile,
) -> Result<()> {
    let tx = conn
        .transaction()
        .context("Falha ao abrir transação do arquivo")?;

    // Cada ocorrência de chunk em `file_chunks` incrementa exatamente uma vez
    // `reference_count`, inclusive quando o mesmo chunk aparece repetidamente.
    for (hash_id, size, compressed_size) in &processed.chunks {
        upsert_chunk(&tx, hash_id, *size, *compressed_size)?;
    }

    let file_id = insert_file(
        &tx,
        snapshot_id,
        &processed.relative_path,
        processed.permissions,
        processed.modification_time,
        processed.size,
    )?;

    for (order, (hash_id, _size, _compressed)) in processed.chunks.iter().enumerate() {
        insert_file_chunk(&tx, file_id, hash_id, order as i64)?;
    }

    tx.commit()
        .context("Falha ao confirmar metadados do arquivo")?;
    Ok(())
}

/// Valida um chunk já armazenado antes de reutilizá-lo.
fn validate_reusable_chunk(
    conn: &rusqlite::Connection,
    physical_path: &Path,
    hash_id: &str,
    key: &[u8; 32],
) -> Result<()> {
    match verify::validate_stored_chunk(physical_path, key, hash_id) {
        Ok(true) => {
            // Um chunk que fora marcado como corrompido e agora foi validado
            // tem o diagnóstico limpo, como no scrubbing.
            clear_chunk_corruption(conn, hash_id)?;
            Ok(())
        }
        Ok(false) => {
            let reason = format!(
                "hash BLAKE3 recalculado não confere para o chunk armazenado {}",
                hash_id
            );
            record_chunk_corruption(conn, hash_id, &reason)?;
            anyhow::bail!(
                "Chunk armazenado {} está corrompido: o hash BLAKE3 recalculado não confere; ele não será reutilizado",
                hash_id
            )
        }
        Err(error) => {
            let reason = format!("chunk armazenado não pôde ser validado: {:#}", error);
            record_chunk_corruption(conn, hash_id, &reason)?;
            Err(error).with_context(|| {
                format!(
                    "Chunk armazenado {} não pôde ser validado e não será reutilizado",
                    hash_id
                )
            })
        }
    }
}

/// Registra a corrupção quando o chunk já possui metadados no banco.
///
/// Um arquivo físico pode existir sem linha em `chunks` após um encerramento
/// inesperado; nesse caso não há diagnóstico a atualizar, mas o chunk ainda
/// precisa ser rejeitado.
fn record_chunk_corruption(conn: &rusqlite::Connection, hash_id: &str, reason: &str) -> Result<()> {
    if get_chunk(conn, hash_id)?.is_some() {
        mark_chunk_corrupted(conn, hash_id, reason).with_context(|| {
            format!(
                "Falha ao registrar corrupção do chunk {} ({})",
                hash_id, reason
            )
        })?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn process_file(
    conn: &rusqlite::Connection,
    repo_root: &Path,
    source: &Path,
    path: &Path,
    settings: &Settings,
    key: &[u8; 32],
    progress: &mut crate::status::ProgressWriter,
    validated_chunks: &mut HashSet<String>,
) -> Result<ProcessedFile> {
    let relative_path = relative_path_for_metadata(source, path)?;
    let file = File::open(path)
        .with_context(|| format!("Falha ao abrir arquivo de origem: {}", path.display()))?;
    let before = file
        .metadata()
        .with_context(|| format!("Falha ao ler metadados: {}", path.display()))?;
    if !before.is_file() {
        anyhow::bail!("Não é um arquivo regular: {}", path.display());
    }

    let size_before = before.len();
    let modified_before = before.modified().ok();
    let permissions = permissions_for_metadata(&before);
    let modification_time = modified_before.and_then(system_time_to_unix);

    let mut chunks = Vec::new();
    let mut bytes = 0u64;
    let mut chunks_new = 0usize;
    let mut chunks_reused = 0usize;

    for chunk_result in chunking::stream_chunks(
        file,
        settings.minimum_chunk_size(),
        settings.maximum_chunk_size(),
    ) {
        let chunk = chunk_result.context("Falha ao dividir arquivo em chunks")?;
        let hash_id = hash::hash_bytes(&chunk);
        let physical_path = chunk_path(repo_root, &hash_id)?;
        let already_stored = physical_path.is_file();

        if already_stored && !validated_chunks.contains(&hash_id) {
            validate_reusable_chunk(conn, &physical_path, &hash_id, key)?;
            validated_chunks.insert(hash_id.clone());
        }

        let compressed_size = if already_stored {
            chunks_reused += 1;
            get_chunk(conn, &hash_id)?.and_then(|info| {
                info.compressed_size
                    .and_then(|value| if value >= 0 { Some(value as u64) } else { None })
            })
        } else {
            let compressed = compress::try_compress(&chunk, 3)?;
            let encrypted = crypto::try_encrypt(&compressed, key)?;
            write::atomic_write(&encrypted, &physical_path)
                .with_context(|| format!("Falha ao gravar chunk {}", hash_id))?;
            // O conteúdo foi lido e cifrado nesta execução; não há necessidade
            // de reler o arquivo recém-gravado se ele reaparecer no backup.
            validated_chunks.insert(hash_id.clone());
            chunks_new += 1;
            Some(encrypted.len() as u64)
        };

        bytes += chunk.len() as u64;
        chunks.push((hash_id, chunk.len() as u64, compressed_size));
        progress.value.current_file_bytes = bytes;
        progress.value.bytes_processed += chunk.len() as u64;
        if already_stored {
            progress.value.chunks_reused += 1;
        } else {
            progress.value.chunks_new += 1;
        }
        progress.publish(false);
    }

    let after = fs::metadata(path)
        .with_context(|| format!("Falha ao reler metadados: {}", path.display()))?;
    let modified_after = after.modified().ok();
    if after.len() != size_before || modified_after != modified_before {
        anyhow::bail!(
            "Arquivo alterado durante o backup (tamanho/mtime mudou): {}",
            path.display()
        );
    }
    if bytes != size_before {
        anyhow::bail!(
            "Tamanho lido ({}) difere do tamanho do arquivo ({}): {}",
            bytes,
            size_before,
            path.display()
        );
    }

    Ok(ProcessedFile {
        relative_path,
        size: size_before as i64,
        permissions,
        modification_time,
        chunks,
        bytes,
        chunks_new,
        chunks_reused,
    })
}

#[cfg(unix)]
fn permissions_for_metadata(metadata: &fs::Metadata) -> Option<i64> {
    use std::os::unix::fs::PermissionsExt;
    Some((metadata.permissions().mode() & 0o777) as i64)
}

#[cfg(not(unix))]
fn permissions_for_metadata(_metadata: &fs::Metadata) -> Option<i64> {
    None
}

fn system_time_to_unix(time: std::time::SystemTime) -> Option<i64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs() as i64)
}

fn walk_source(
    source: &Path,
    repo_root: &Path,
    progress: &mut crate::status::ProgressWriter,
) -> Result<WalkResult> {
    let repo_root = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    let mut result = WalkResult {
        files: Vec::new(),
        errors: Vec::new(),
    };
    walk_dir(source, &repo_root, &mut result, progress)?;
    result.files.sort();
    Ok(result)
}

fn walk_dir(
    dir: &Path,
    repo_root: &Path,
    result: &mut WalkResult,
    progress: &mut crate::status::ProgressWriter,
) -> Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            result.errors.push(FileError {
                path: dir.display().to_string(),
                message: format!("Falha ao ler diretório: {}", error),
            });
            return Ok(());
        }
    };

    let mut paths = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => paths.push(entry.path()),
            Err(error) => result.errors.push(FileError {
                path: dir.display().to_string(),
                message: format!("Falha ao ler entrada do diretório: {}", error),
            }),
        }
    }
    paths.sort();

    for path in paths {
        if path == repo_root || path.starts_with(repo_root) {
            continue;
        }
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                result.errors.push(FileError {
                    path: path.display().to_string(),
                    message: format!("Falha ao ler metadados: {}", error),
                });
                continue;
            }
        };
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            // Não segue symlinks: evita ciclos e backup de dados fora da origem.
            continue;
        }
        if file_type.is_dir() {
            walk_dir(&path, repo_root, result, progress)?;
        } else if file_type.is_file() {
            if path.to_str().is_none() {
                result.errors.push(FileError {
                    path: path.display().to_string(),
                    message: "Caminho não é UTF-8 e não pode ser registrado".to_string(),
                });
                continue;
            }
            result.files.push(path);
            progress.value.discovered_files = result.files.len();
            progress.publish(false);
        }
    }
    Ok(())
}
