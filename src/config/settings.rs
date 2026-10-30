use serde::{Deserialize, Serialize};

/// Configurações do redbackup.
/// Regra: chave de criptografia NÃO armazenada aqui.
/// A chave é lida via arquivo separado (`--key-file`) com permissões `0o600`.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Settings {
    /// Diretório de origem a ser monitorado/backup.
    pub source_path: String,
    /// Diretório de destino (repositório de backup).
    pub destination_path: String,
    /// Caminho para arquivo de chave de criptografia (não a chave em si).
    /// Se vazio, a chave deve ser fornecida via CLI (`--key-file`).
    pub key_file_path: Option<String>,
    /// Parâmetros de chunking (FastCDC) em bytes.
    pub chunk_min_size: u64,
    pub chunk_max_size: u64,
    /// Política de retenção simplificada (dias mínimos para manter).
    pub retention_days: u64,
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
        }
    }
}
