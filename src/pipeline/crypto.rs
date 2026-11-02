//! Criptografia AES-256-GCM com nonce único por operação.
//!
//! Formato persistido: `[version: 1 byte] [nonce: 12 bytes] [ciphertext + tag]`.

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use anyhow::Result;
use rand::Rng;

const VERSION: u8 = 1;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;

/// Criptografia que propaga falhas.
pub fn try_encrypt(data: &[u8], key: &[u8; 32]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| anyhow::anyhow!("Chave AES-256-GCM inválida"))?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, data)
        .map_err(|_| anyhow::anyhow!("Falha na criptografia AES-GCM"))?;

    let mut result = Vec::with_capacity(1 + NONCE_LEN + ciphertext.len());
    result.push(VERSION);
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

/// Descriptografia autenticada. Nunca retorna dados não autenticados.
pub fn decrypt(data: &[u8], key: &[u8; 32]) -> Result<Vec<u8>> {
    if data.is_empty() {
        anyhow::bail!("Chunk criptografado vazio");
    }
    if data[0] != VERSION {
        anyhow::bail!(
            "Versão de formato de criptografia não suportada: {}",
            data[0]
        );
    }
    if data.len() < 1 + NONCE_LEN + TAG_LEN {
        anyhow::bail!(
            "Chunk criptografado truncado: {} bytes (mínimo {})",
            data.len(),
            1 + NONCE_LEN + TAG_LEN
        );
    }

    let nonce_bytes = &data[1..1 + NONCE_LEN];
    let ciphertext = &data[1 + NONCE_LEN..];
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| anyhow::anyhow!("Chave AES-256-GCM inválida"))?;
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| anyhow::anyhow!("Falha na autenticação/descriptografia AES-GCM"))
}

/// Conveniência para testes e chamadas em memória.
pub fn encrypt(data: &[u8], key: &[u8; 32]) -> Vec<u8> {
    try_encrypt(data, key).expect("Falha inesperada na criptografia AES-GCM em memória")
}
