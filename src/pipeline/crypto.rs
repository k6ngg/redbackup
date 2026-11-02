//! Criptografia AES-256-GCM com nonce único por operação.
//!
//! Formato persistido: [version: 1 byte] [nonce: 12 bytes] [ciphertext + tag].

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use rand::Rng;

pub fn encrypt(data: &[u8], key: &[u8; 32]) -> Vec<u8> {
    let cipher = Aes256Gcm::new_from_slice(key).expect("Chave AES-256-GCM inválida");
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, data)
        .expect("Falha na criptografia AES-GCM");
    let mut result = Vec::with_capacity(1 + 12 + ciphertext.len());
    result.push(1); // version
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);
    result
}

pub fn decrypt(data: &[u8], key: &[u8; 32]) -> Vec<u8> {
    if data.is_empty() || data[0] != 1 {
        panic!("Versão de formato de criptografia não suportada");
    }
    let nonce_bytes = &data[1..13];
    let ciphertext = &data[13..];
    let cipher = Aes256Gcm::new_from_slice(key).expect("Chave AES-256-GCM inválida");
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .expect("Falha na descriptografia AES-GCM")
}
