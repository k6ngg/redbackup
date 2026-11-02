//! Leitura segura de arquivos para backup.
//!
//! O pipeline de backup usa `File` diretamente com `stream_chunks`, mantendo
//! memória limitada ao tamanho máximo de chunk. Esta função permanece para
//! operações pontuais e testes.

use std::fs::File;
use std::io::{BufReader, Read};

pub fn read_file_stream(path: &str) -> std::io::Result<Vec<u8>> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer)?;
    Ok(buffer)
}
