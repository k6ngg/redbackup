//! Chunking baseado em conteúdo com FastCDC.
//!
//! Usa parâmetros configuráveis (min_size, max_size) conforme `config/settings.rs`.

pub fn chunk_file(data: &[u8], min_size: usize, max_size: usize) -> Vec<Vec<u8>> {
    let avg_size = std::cmp::max(min_size, 256);
    let mut chunks = Vec::new();
    let cdc = fastcdc::FastCDC::new(data, min_size, avg_size, max_size);
    for chunk in cdc {
        let start = chunk.offset;
        let end = start + chunk.length;
        chunks.push(data[start..end].to_vec());
    }
    chunks
}
