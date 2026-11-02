//! Compressão de chunks com Zstandard.

use anyhow::{Context, Result};
use zstd::stream::encode_all;

/// Compressão que propaga falhas (usada pelo pipeline de backup).
pub fn try_compress(data: &[u8], level: i32) -> Result<Vec<u8>> {
    encode_all(data, level).context("Falha na compressão zstd")
}

/// Descompressão que propaga falhas (usada na restauração e scrubbing).
pub fn try_decompress(data: &[u8]) -> Result<Vec<u8>> {
    zstd::stream::decode_all(data).context("Falha na descompressão zstd")
}

/// Conveniência para testes e chamadas em memória.
pub fn compress(data: &[u8], level: i32) -> Vec<u8> {
    try_compress(data, level).expect("Falha inesperada na compressão zstd em memória")
}

/// Conveniência para testes e chamadas em memória.
pub fn decompress(data: &[u8]) -> Vec<u8> {
    try_decompress(data).expect("Falha inesperada na descompressão zstd em memória")
}
