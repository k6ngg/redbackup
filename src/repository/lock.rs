//! Bloqueio cooperativo do repositório entre processos.
//!
//! Usa `flock`/`LockFileEx` (via `fs2`) em `<repositório>/metadata/redbackup.lock`.
//! O bloqueio é liberado automaticamente quando o processo termina, inclusive
//! em caso de crash, evitando arquivos de lock obsoletos.
//!
//! Regras:
//! - backup, pruning e recuperação exigem bloqueio exclusivo;
//! - scrubbing e restauração usam bloqueio compartilhado, pois só leem chunks;
//! - operações de leitura de metadados (listagens) não bloqueiam.

use crate::config::validation::set_file_permissions;
use crate::repository::metadata_dir;
use anyhow::{Context, Result};
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

/// Bloqueio mantido enquanto o valor estiver vivo.
pub struct RepositoryLock {
    file: File,
    #[allow(dead_code)]
    path: PathBuf,
}

impl RepositoryLock {
    /// Bloqueio exclusivo: apenas um processo pode manter por vez.
    pub fn exclusive(repo_root: &Path) -> Result<Self> {
        Self::acquire(repo_root, false)
    }

    /// Bloqueio compartilhado: permite leitores concorrentes, mas exclui
    /// escritores.
    pub fn shared(repo_root: &Path) -> Result<Self> {
        Self::acquire(repo_root, true)
    }

    fn acquire(repo_root: &Path, shared: bool) -> Result<Self> {
        let dir = metadata_dir(repo_root);
        fs::create_dir_all(&dir)
            .with_context(|| format!("Falha ao criar diretório de metadados: {}", dir.display()))?;
        let path = dir.join("redbackup.lock");

        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options
            .open(&path)
            .with_context(|| format!("Falha ao abrir lock do repositório: {}", path.display()))?;
        set_file_permissions(&path, 0o600)?;

        let result = if shared {
            FileExt::try_lock_shared(&file)
        } else {
            FileExt::try_lock_exclusive(&file)
        };
        result.with_context(|| {
            format!(
                "Repositório ocupado por outra operação ({}). Aguarde o backup/pruning terminar.",
                path.display()
            )
        })?;

        Ok(Self { file, path })
    }
}

impl Drop for RepositoryLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}
