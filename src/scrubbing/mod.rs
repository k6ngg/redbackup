//! Scrubbing (verificação de integridade) dos chunks armazenados.
//!
//! Regra: descriptografa, descomprime, recalcula BLAKE3 e compara com `hash_id`.
//! Se falhar, registra diagnóstico sem expor segredos, mas NÃO apaga automaticamente.

use crate::pipeline::{compress, crypto, hash};
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

pub fn scrub_chunk(chunk_path: &Path, key: &[u8; 32], expected_hash: &str) -> Result<bool> {
    let encrypted_data = fs::read(chunk_path).with_context(|| {
        format!(
            "Falha ao ler chunk para scrubbing: {}",
            chunk_path.display()
        )
    })?;
    let decrypted = crypto::decrypt(&encrypted_data, key);
    let decompressed = compress::decompress(&decrypted);
    let recalculated = hash::hash_bytes(&decompressed);
    Ok(recalculated == expected_hash)
}
