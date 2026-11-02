//! Compressão de chunks com Zstandard.

use zstd::stream::encode_all;

pub fn compress(data: &[u8], level: i32) -> Vec<u8> {
    encode_all(data, level).expect("Falha na compressão zstd")
}

pub fn decompress(data: &[u8]) -> Vec<u8> {
    zstd::stream::decode_all(data).expect("Falha na descompressão zstd")
}
