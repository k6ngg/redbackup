//! Reconstrução de arquivos a partir de chunks persistidos.
//!
//! O arquivo é montado em um temporário no diretório de destino, validado
//! chunk a chunk e só então publicado com rename atômico. Em caso de falha o
//! temporário é descartado e nenhum arquivo parcial é apresentado como sucesso.

use crate::metadata::repository::{get_file_chunks, get_file_info, FileInfo};
use crate::pipeline::{compress, crypto, hash};
use crate::repository::chunk_path;
use crate::repository::key_binding;
use crate::repository::lock::RepositoryLock;
use anyhow::{Context, Result};
use filetime::FileTime;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Resultado de uma restauração.
#[derive(Debug, Clone)]
pub struct RestoreReport {
    pub path: PathBuf,
    pub bytes: u64,
    pub chunks: usize,
}

/// Restaura um arquivo usando os metadados persistidos no snapshot.
pub fn restore_from_snapshot(
    repo_root: &Path,
    conn: &rusqlite::Connection,
    snapshot_id: i64,
    stored_path: &str,
    output: &Path,
    key: &[u8; 32],
    force: bool,
) -> Result<RestoreReport> {
    crate::repository::checked_relative_path(stored_path)?;
    key_binding::require_matching_key_if_bound(conn, key)?;
    // Compartilhado: impede que pruning/recuperação removam chunks enquanto o
    // arquivo está sendo lido, sem impedir outras restaurações/scrubbings.
    let _lock = RepositoryLock::shared(repo_root)?;

    let file = get_file_info(conn, snapshot_id, stored_path)?.ok_or_else(|| {
        anyhow::anyhow!(
            "Arquivo não encontrado no snapshot {}: {}",
            snapshot_id,
            stored_path
        )
    })?;
    let chunks = get_file_chunks(conn, snapshot_id, stored_path)?;
    let final_path = resolve_output_path(output, &file);

    rebuild(&chunks, Some(&file), &final_path, repo_root, key, force)
}

/// Reconstrói um arquivo a partir de uma lista de hashes já consultada.
///
/// Mantida para uso direto e testes; a interface principal é
/// [`restore_from_snapshot`], que consulta o banco e preserva metadados.
pub fn restore_file(
    chunk_hashes: &[(String, i32)],
    dest_path: &Path,
    data_root: &Path,
    key: &[u8; 32],
    force: bool,
) -> Result<()> {
    rebuild(chunk_hashes, None, dest_path, data_root, key, force)?;
    Ok(())
}

fn rebuild(
    chunks: &[(String, i32)],
    file: Option<&FileInfo>,
    dest_path: &Path,
    repo_root: &Path,
    key: &[u8; 32],
    force: bool,
) -> Result<RestoreReport> {
    if dest_path.exists() && !force {
        anyhow::bail!(
            "Arquivo de destino já existe e `force` não está ativado: {}",
            dest_path.display()
        );
    }

    let parent = dest_path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .with_context(|| format!("Falha ao criar diretório de destino: {}", parent.display()))?;

    let mut temp = NamedTempFile::new_in(parent)
        .with_context(|| format!("Falha ao criar arquivo temporário em {}", parent.display()))?;

    let mut bytes = 0u64;
    for (hash_id, _order) in chunks {
        let stored = chunk_path(repo_root, hash_id)?;
        let encrypted = fs::read(&stored)
            .with_context(|| format!("Chunk ausente ou ilegível: {}", stored.display()))?;
        let decrypted = crypto::decrypt(&encrypted, key)
            .with_context(|| format!("Falha ao descriptografar chunk {}", hash_id))?;
        let decompressed = compress::try_decompress(&decrypted)
            .with_context(|| format!("Falha ao descomprimir chunk {}", hash_id))?;

        let recalculated = hash::hash_bytes(&decompressed);
        if recalculated != *hash_id {
            anyhow::bail!(
                "Integridade violada no chunk {}: hash recalculado {}",
                hash_id,
                recalculated
            );
        }

        temp.write_all(&decompressed)
            .with_context(|| format!("Falha ao escrever chunk {} no temporário", hash_id))?;
        bytes += decompressed.len() as u64;
    }

    temp.as_file_mut()
        .sync_all()
        .context("Falha ao sincronizar arquivo restaurado")?;

    if let Some(file) = file {
        if file.size >= 0 && bytes != file.size as u64 {
            anyhow::bail!(
                "Tamanho restaurado ({}) difere do metadado ({}): {}",
                bytes,
                file.size,
                file.path
            );
        }
        apply_metadata(temp.as_file(), file)?;
    }

    if force {
        temp.persist(dest_path).map_err(|error| {
            anyhow::anyhow!("Falha ao publicar arquivo restaurado: {}", error.error)
        })?;
    } else {
        temp.persist_noclobber(dest_path).map_err(|error| {
            anyhow::anyhow!(
                "Arquivo de destino apareceu durante a restauração (use `force` para substituir): {}",
                error.error
            )
        })?;
    }

    Ok(RestoreReport {
        path: dest_path.to_path_buf(),
        bytes,
        chunks: chunks.len(),
    })
}

fn resolve_output_path(output: &Path, file: &FileInfo) -> PathBuf {
    if output.is_dir() {
        let name = Path::new(&file.path)
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("restaurado"));
        output.join(name)
    } else {
        output.to_path_buf()
    }
}

fn apply_metadata(file: &fs::File, info: &FileInfo) -> Result<()> {
    #[cfg(unix)]
    if let Some(mode) = info.permissions {
        let permissions = fs::Permissions::from_mode((mode as u32) & 0o777);
        file.set_permissions(permissions)
            .context("Falha ao aplicar permissões do arquivo restaurado")?;
    }

    if let Some(mtime) = info.modification_time {
        let time = FileTime::from_unix_time(mtime, 0);
        filetime::set_file_handle_times(file, None, Some(time))
            .context("Falha ao aplicar modification_time do arquivo restaurado")?;
    }

    Ok(())
}
