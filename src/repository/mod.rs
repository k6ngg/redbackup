//! Inicialização segura do repositório de destino.
//!
//! Estrutura prevista:
//! <destino>/
//! ├── data/                 (chunks criptografados/comprimidos)
//! │   └── {prefixo_2_chars}/
//! │       └── {hash}
//! ├── metadata/
//! │   ├── index.db         (SQLite)
//! │   └── migrations/
//! └── config/
//!     └── settings.json
//!
//! Regras:
//! - Não apaga nem sobrescreve dados existentes sem verificação explícita.
//! - Cria diretórios com permissões restritas se não existirem.
//! - Se `metadata/index.db` existe mas `data/` está vazio (ou vice-versa), detecta inconsistência.

use std::fs::create_dir_all;

pub mod init;
use crate::config::validation::{set_directory_permissions, validate_path};
use anyhow::{Context, Result};
use std::path::PathBuf;

pub fn initialize_repository(destination_path: &str) -> Result<PathBuf> {
    let dest = validate_path(destination_path, false)?;

    // Subdiretórios
    let data_dir = dest.join("data");
    let metadata_dir = dest.join("metadata");
    let config_dir = dest.join("config");

    // Verifica consistência parcial: se `metadata/index.db` existe mas `data` não, é inconsistente
    let db_exists = metadata_dir.join("index.db").exists();
    let data_exists = data_dir.exists();

    if db_exists && !data_exists {
        anyhow::bail!(
            "Inconsistência detectada: `metadata/index.db` existe, mas `data/` está ausente. \
            Não é seguro inicializar automaticamente. Verifique o repositório em: {}",
            dest.display()
        );
    }

    // Cria diretórios se não existirem
    for (dir, mode) in [
        (&data_dir, 0o750u32),
        (&metadata_dir, 0o700u32),
        (&config_dir, 0o750u32),
    ] {
        if !dir.exists() {
            create_dir_all(dir)
                .with_context(|| format!("Falha ao criar diretório: {}", dir.display()))?;
            set_directory_permissions(dir, mode).with_context(|| {
                format!(
                    "Falha ao definir permissões no diretório: {}",
                    dir.display()
                )
            })?;
        }
    }

    Ok(dest)
}
