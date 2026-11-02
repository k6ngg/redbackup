//! Chunking baseado em conteúdo com FastCDC.
//!
//! A versão em memória é usada em testes e em arquivos pequenos; o backup usa
//! `stream_chunks`, que mantém apenas `max_size` bytes em memória por vez.

use std::io::Read;

fn even(value: usize) -> usize {
    if value.is_multiple_of(2) {
        value
    } else {
        value.saturating_add(1)
    }
}

/// Normaliza parâmetros para os limites aceitos pelo FastCDC 2020.
pub fn normalize_params(min_size: usize, max_size: usize) -> (usize, usize, usize) {
    let min = even(min_size.max(64));
    let max = even(max_size.max(1024).max(min));
    let avg = even(std::cmp::max(min, 256)).min(max);
    (min, avg, max)
}

/// Divide um buffer em chunks. Retorna vazio para entrada vazia.
pub fn chunk_file(data: &[u8], min_size: usize, max_size: usize) -> Vec<Vec<u8>> {
    if data.is_empty() {
        return Vec::new();
    }
    let (min, avg, max) = normalize_params(min_size, max_size);
    let cdc = fastcdc::v2020::FastCDC::new(data, min, avg, max);
    cdc.map(|chunk| data[chunk.offset..chunk.offset + chunk.length].to_vec())
        .collect()
}

/// Iterador de chunks que lê o arquivo em fluxo.
pub struct ChunkStream<R: Read> {
    inner: fastcdc::v2020::StreamCDC<R>,
}

impl<R: Read> Iterator for ChunkStream<R> {
    type Item = anyhow::Result<Vec<u8>>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|result| {
            result
                .map(|chunk| chunk.data)
                .map_err(|error| anyhow::anyhow!(error))
        })
    }
}

/// Cria um iterador de chunks sem carregar o arquivo inteiro na memória.
pub fn stream_chunks<R: Read>(reader: R, min_size: usize, max_size: usize) -> ChunkStream<R> {
    let (min, avg, max) = normalize_params(min_size, max_size);
    ChunkStream {
        inner: fastcdc::v2020::StreamCDC::new(reader, min, avg, max),
    }
}
