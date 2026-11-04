//! Validação de chunks já armazenados no repositório.
//!
//! Antes de reutilizar um chunk, o backup precisa confirmar que o arquivo
//! existente ainda corresponde ao hash do conteúdo. A presença física do
//! arquivo não é suficiente: ele pode ter sido truncado, alterado ou
//! substituído depois de gravado.
//!
//! A validação autentica o conteúdo com AES-256-GCM, descomprime com Zstandard
//! e recalcula BLAKE3. Nenhum dado é alterado ou apagado por esta rotina.

use crate::pipeline::{compress, crypto, hash};
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

/// Valida um chunk armazenado contra o hash BLAKE3 esperado.
///
/// Retorna `Ok(true)` quando o conteúdo é autêntico e corresponde ao hash,
/// `Ok(false)` quando o hash recalculado diverge e `Err` quando o arquivo não
/// pode ser lido, autenticado ou descomprimido. Em todos os casos de falha o
/// chamador deve tratar o chunk como não reutilizável.
pub fn validate_stored_chunk(
    chunk_path: &Path,
    key: &[u8; 32],
    expected_hash: &str,
) -> Result<bool> {
    let encrypted_data = fs::read(chunk_path).with_context(|| {
        format!(
            "Falha ao ler chunk armazenado para validação: {}",
            chunk_path.display()
        )
    })?;
    let decrypted = crypto::decrypt(&encrypted_data, key).with_context(|| {
        format!(
            "Falha ao autenticar/descriptografar chunk armazenado: {}",
            chunk_path.display()
        )
    })?;
    let decompressed = compress::try_decompress(&decrypted).with_context(|| {
        format!(
            "Falha ao descomprimir chunk armazenado: {}",
            chunk_path.display()
        )
    })?;
    let recalculated = hash::hash_bytes(&decompressed);
    Ok(recalculated == expected_hash)
}
