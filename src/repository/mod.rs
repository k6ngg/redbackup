//! Inicialização segura e construção centralizada de caminhos do repositório.
//!
//! Estrutura:
//! ```text
//! <destino>/
//! ├── data/
//! │   └── {prefixo_2_chars}/
//! │       └── {hash}
//! ├── metadata/
//! │   ├── index.db
//! │   ├── redbackup.lock   (lock entre processos)
//! │   └── redbackup.sock   (daemon)
//! └── config/
//!     └── settings.json
//! ```

use crate::config::validation::{
    set_directory_permissions, set_file_permissions, validate_path, validate_relative_path,
};
use crate::metadata::migrations::run_migrations;
use crate::metadata::repository::connect_or_create;
use anyhow::{Context, Result};
use std::fs::{self, create_dir_all};
use std::path::{Path, PathBuf};

pub mod init;
pub mod lock;

pub fn data_dir(repo_root: &Path) -> PathBuf {
    repo_root.join("data")
}

pub fn metadata_dir(repo_root: &Path) -> PathBuf {
    repo_root.join("metadata")
}

pub fn config_dir(repo_root: &Path) -> PathBuf {
    repo_root.join("config")
}

pub fn index_db_path(repo_root: &Path) -> PathBuf {
    metadata_dir(repo_root).join("index.db")
}

pub fn settings_path(repo_root: &Path) -> PathBuf {
    config_dir(repo_root).join("settings.json")
}

pub fn socket_path(repo_root: &Path) -> PathBuf {
    metadata_dir(repo_root).join("redbackup.sock")
}

/// Valida um hash BLAKE3 (64 caracteres hexadecimais minúsculos).
pub fn validate_hash_id(hash: &str) -> Result<()> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        anyhow::bail!("Hash de chunk inválido: {}", hash);
    }
    Ok(())
}

/// Caminho físico de um chunk dentro do repositório.
///
/// O hash é validado antes de ser usado, impedindo que metadados corrompidos
/// escapem de `data/`.
pub fn chunk_path(repo_root: &Path, hash: &str) -> Result<PathBuf> {
    validate_hash_id(hash)?;
    Ok(data_dir(repo_root).join(&hash[..2]).join(hash))
}

/// Inicializa (ou valida) a estrutura física do repositório e o banco SQLite.
///
/// A operação é idempotente e não apaga dados existentes.
pub fn initialize_repository(destination_path: &str) -> Result<PathBuf> {
    let dest = validate_path(destination_path, false)?;
    if dest.exists() && !dest.is_dir() {
        anyhow::bail!("O destino não é um diretório: {}", dest.display());
    }
    create_dir_all(&dest)
        .with_context(|| format!("Falha ao criar diretório de destino: {}", dest.display()))?;
    let dest = dest
        .canonicalize()
        .with_context(|| format!("Falha ao resolver destino: {}", dest.display()))?;

    // Nunca transforme silenciosamente uma pasta arbitrária em repositório.
    // Um destino não vazio só é aceito se já parecer um repositório
    // redbackup (marcadores) ou contiver apenas os diretórios parciais que a
    // inicialização cria. Isso evita misturar o repositório com dados do
    // usuário, como uma pasta de imagens.
    let mut names = Vec::new();
    let entries = fs::read_dir(&dest)
        .with_context(|| format!("Falha ao ler diretório de destino: {}", dest.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("Falha ao ler entrada de {}", dest.display()))?;
        names.push(entry.file_name());
    }
    if !names.is_empty() {
        let has_marker = settings_path(&dest).exists() || index_db_path(&dest).exists();
        let only_repo_dirs = names.iter().all(|name| {
            matches!(
                name.to_str(),
                Some("data") | Some("metadata") | Some("config")
            )
        });
        if !has_marker && !only_repo_dirs {
            anyhow::bail!(
                "O diretório de destino não está vazio e não parece ser um repositório \
                 redbackup: {}. Use um diretório vazio ou dedicado para o repositório.",
                dest.display()
            );
        }
    }

    let data = data_dir(&dest);
    let metadata = metadata_dir(&dest);
    let config = config_dir(&dest);

    // Consistência parcial: banco sem dados ou dados sem banco indicam que o
    // repositório foi movido/corrompido. Não tente "consertar" automaticamente
    // quando já existem chunks no disco.
    let db_exists = index_db_path(&dest).exists();
    let data_exists = data.exists();
    if db_exists && !data_exists {
        anyhow::bail!(
            "Inconsistência detectada: `metadata/index.db` existe, mas `data/` está ausente. \
             Não é seguro inicializar automaticamente. Verifique o repositório em: {}",
            dest.display()
        );
    }
    if data_exists && !db_exists {
        let has_chunks = std::fs::read_dir(&data)
            .with_context(|| format!("Falha ao ler data/: {}", data.display()))?
            .filter_map(|entry| entry.ok())
            .any(|entry| entry.path().is_file() || entry.path().is_dir());
        if has_chunks {
            anyhow::bail!(
                "Inconsistência detectada: `data/` contém dados, mas `metadata/index.db` está ausente. \
                 Verifique o repositório em: {}",
                dest.display()
            );
        }
    }

    for (dir, mode) in [
        (&data, 0o750u32),
        (&metadata, 0o700u32),
        (&config, 0o750u32),
    ] {
        create_dir_all(dir)
            .with_context(|| format!("Falha ao criar diretório: {}", dir.display()))?;
        set_directory_permissions(dir, mode).with_context(|| {
            format!(
                "Falha ao definir permissões no diretório: {}",
                dir.display()
            )
        })?;
    }

    let db_path = index_db_path(&dest);
    let conn = connect_or_create(
        db_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Caminho do banco não é UTF-8"))?,
    )?;
    run_migrations(&conn)?;
    drop(conn);
    set_file_permissions(&db_path, 0o600)?;

    Ok(dest)
}

/// Resolve a raiz de um repositório já inicializado.
pub fn open_repository(repo_root: &Path) -> Result<PathBuf> {
    let root = validate_path(
        repo_root
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Caminho do repositório não é UTF-8"))?,
        true,
    )?;
    if !root.is_dir() {
        anyhow::bail!("Repositório não é um diretório: {}", root.display());
    }
    if !settings_path(&root).exists() {
        anyhow::bail!(
            "Configuração não encontrada em {}. Execute `redbackup init` ou use `--repo <destino>`.",
            settings_path(&root).display()
        );
    }
    if !index_db_path(&root).exists() {
        anyhow::bail!(
            "Banco de metadados não encontrado em {}. O repositório está incompleto.",
            index_db_path(&root).display()
        );
    }
    Ok(root)
}

/// Monta um caminho de chunk relativo ao repositório sem validar o hash.
/// Usado apenas para diagnóstico de arquivos potencialmente órfãos.
pub fn loose_chunk_path(repo_root: &Path, hash: &str) -> PathBuf {
    let prefix = if hash.len() >= 2 { &hash[..2] } else { "00" };
    data_dir(repo_root).join(prefix).join(hash)
}

/// Verifica se um caminho persistido é seguro e retorna sua versão textual.
pub fn checked_relative_path(path: &str) -> Result<&str> {
    validate_relative_path(path)?;
    Ok(path)
}
