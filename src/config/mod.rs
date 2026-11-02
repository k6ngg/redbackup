//! Configuração e validação de `config/settings.json`.
//!
//! Regra: chave de criptografia NÃO armazenada em `settings.json`.
//! Usar arquivo separado (`--key-file`) com permissões `0o600`.

pub mod key;
pub mod registry;
pub mod settings;
pub mod validation;
