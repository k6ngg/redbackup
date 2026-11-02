//! Leitura segura de arquivos para backup.
//!
//! Regra: não carregar o arquivo inteiro na memória quando possível; usar streaming.

use std::fs::File;
use std::io::{Read, BufReader};

pub fn read_file_stream(path: &str) -> std::io::Result<Vec<u8>> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer)?;
    Ok(buffer)
}
