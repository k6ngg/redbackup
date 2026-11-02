//! Pipeline de backup: leitura segura, chunking, hashing, deduplicação, compressão, criptografia, persistência.
//!
//! Etapas verificáveis:
//! 1. Descobrir e validar arquivos.
//! 2. Ler com tratamento de arquivos inacessíveis/alterados/em uso.
//! 3. Dividir com FastCDC (configurável).
//! 4. Calcular BLAKE3.
//! 5. Consultar índice SQLite (deduplicação).
//! 6. Comprimir com Zstandard (novos chunks).
//! 7. Criptografar com AES-256-GCM (nonce único por operação).
//! 8. Gravar com estratégia segura (arquivo temporário + rename atômico).
//! 9. Finalizar metadados e snapshot após confirmar persistência física.

pub mod chunking;
pub mod compress;
pub mod crypto;
pub mod hash;
pub mod reader;
pub mod write;
