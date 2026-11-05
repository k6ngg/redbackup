//! Configuração persistida em `<repositório>/config/settings.json`.
//!
//! A chave de criptografia nunca é armazenada aqui. O arquivo guarda apenas
//! caminhos, parâmetros de chunking e política de retenção.

use crate::pipeline::write::atomic_write_with_mode;
use crate::repository::config_dir;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Limites aceitos pelo FastCDC (versão 2020).
pub const CHUNK_MIN_BYTES: u64 = 64;
pub const CHUNK_MIN_MAX_BYTES: u64 = 1_048_576; // MINIMUM_MAX
pub const CHUNK_MAX_MIN_BYTES: u64 = 1024; // MAXIMUM_MIN
pub const CHUNK_MAX_BYTES: u64 = 16 * 1024 * 1024; // MAXIMUM_MAX

/// Configurações do redbackup.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Settings {
    /// Diretório de origem do backup.
    pub source_path: String,
    /// Diretório de destino (raiz do repositório de backup).
    pub destination_path: String,
    /// Caminho para o arquivo que contém a chave. O conteúdo da chave nunca
    /// entra neste JSON.
    pub key_file_path: Option<String>,
    /// Parâmetros de chunking (FastCDC) em bytes.
    pub chunk_min_size: u64,
    pub chunk_max_size: u64,
    /// Dias mínimos de retenção antes de permitir o pruning de um snapshot.
    pub retention_days: u64,
    /// Diretório independente com a cópia de metadados (sem a chave).
    #[serde(default)]
    pub metadata_backup_path: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            source_path: String::new(),
            destination_path: String::new(),
            key_file_path: None,
            chunk_min_size: 1_048_576, // 1 MiB
            chunk_max_size: 4_194_304, // 4 MiB
            retention_days: 30,
            metadata_backup_path: None,
        }
    }
}

impl Settings {
    /// Caminho padrão do arquivo de configuração dentro de um repositório.
    pub fn path_in(repo_root: &Path) -> PathBuf {
        config_dir(repo_root).join("settings.json")
    }

    pub fn load(repo_root: &Path) -> Result<Self> {
        let path = Self::path_in(repo_root);
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("Falha ao ler configuração: {}", path.display()))?;
        let settings: Self = serde_json::from_str(&raw)
            .with_context(|| format!("Configuração inválida em {}", path.display()))?;
        settings.validate()?;
        Ok(settings)
    }

    pub fn save(&self, repo_root: &Path) -> Result<()> {
        self.validate()?;
        let path = Self::path_in(repo_root);
        let json = serde_json::to_vec_pretty(self).context("Falha ao serializar configuração")?;
        atomic_write_with_mode(&json, &path, 0o600)
            .with_context(|| format!("Falha ao gravar configuração: {}", path.display()))?;
        Ok(())
    }

    /// Valida os campos não secretos e os parâmetros de chunking.
    pub fn validate(&self) -> Result<()> {
        if self.source_path.trim().is_empty() {
            anyhow::bail!("`source_path` é obrigatório na configuração");
        }
        if self.destination_path.trim().is_empty() {
            anyhow::bail!("`destination_path` é obrigatório na configuração");
        }
        if self.chunk_min_size < CHUNK_MIN_BYTES {
            anyhow::bail!(
                "`chunk_min_size` deve ser pelo menos {} bytes",
                CHUNK_MIN_BYTES
            );
        }
        if self.chunk_min_size > CHUNK_MIN_MAX_BYTES {
            anyhow::bail!(
                "`chunk_min_size` não pode exceder {} bytes (limite do FastCDC)",
                CHUNK_MIN_MAX_BYTES
            );
        }
        if self.chunk_max_size < CHUNK_MAX_MIN_BYTES {
            anyhow::bail!(
                "`chunk_max_size` deve ser pelo menos {} bytes",
                CHUNK_MAX_MIN_BYTES
            );
        }
        if self.chunk_min_size > self.chunk_max_size {
            anyhow::bail!("`chunk_min_size` não pode ser maior que `chunk_max_size`");
        }
        if self.chunk_max_size > CHUNK_MAX_BYTES {
            anyhow::bail!(
                "`chunk_max_size` não pode exceder {} bytes",
                CHUNK_MAX_BYTES
            );
        }
        let avg = std::cmp::max(self.chunk_min_size, 256);
        if avg > self.chunk_max_size {
            anyhow::bail!(
                "parâmetros de chunking inconsistentes: média calculada ({}) maior que o máximo ({})",
                avg,
                self.chunk_max_size
            );
        }
        Ok(())
    }

    /// Tamanho médio usado pelo FastCDC (derivado dos parâmetros persistidos).
    pub fn average_chunk_size(&self) -> usize {
        std::cmp::max(self.chunk_min_size, 256) as usize
    }

    pub fn minimum_chunk_size(&self) -> usize {
        self.chunk_min_size as usize
    }

    pub fn maximum_chunk_size(&self) -> usize {
        self.chunk_max_size as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_roundtrip() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(config_dir(root)).unwrap();
        let settings = Settings {
            source_path: "/tmp/origem".to_string(),
            destination_path: root.display().to_string(),
            ..Settings::default()
        };
        settings.save(root).unwrap();
        let loaded = Settings::load(root).unwrap();
        assert_eq!(settings, loaded);
    }

    #[test]
    fn rejeita_chunking_invalido() {
        let settings = Settings {
            source_path: "/tmp/a".to_string(),
            destination_path: "/tmp/b".to_string(),
            chunk_min_size: 10,
            ..Settings::default()
        };
        assert!(settings.validate().is_err());
    }

    #[test]
    fn rejeita_min_acima_do_limite_do_fastcdc() {
        let settings = Settings {
            source_path: "/tmp/a".to_string(),
            destination_path: "/tmp/b".to_string(),
            chunk_min_size: 2 * 1024 * 1024,
            chunk_max_size: 4 * 1024 * 1024,
            ..Settings::default()
        };
        assert!(settings.validate().is_err());
    }
}
