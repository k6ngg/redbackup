//! Hashing de chunks com BLAKE3.

use blake3::Hasher;

pub fn hash_bytes(data: &[u8]) -> String {
    let mut hasher = Hasher::new();
    hasher.update(data);
    hasher.finalize().to_hex().to_string()
}
