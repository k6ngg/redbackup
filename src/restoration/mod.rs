//! Restauração de arquivos e diretórios a partir de snapshots persistidos.
//!
//! Regras:
//! - Nunca sobrescrever silenciosamente um arquivo existente (exige `--force` ou confirmação).
//! - Validar caminhos restaurados para impedir `path traversal`.
//! - Preservar `permissions` e `modification_time` quando disponíveis.
//! - Interromper e informar claramente se algum chunk falhar (não apresentar saída parcial como sucesso).

pub mod rebuild;
