//! Reconstrução de arquivos a partir de chunks persistidos.
//!
//! Etapas:
//! 1. Consultar metadados (`file_chunks` ordenados).
//! 2. Ler cada chunk físico (`/data/{prefixo_2_chars}/{hash}`).
//! 3. Descriptografar (`AES-256-GCM`) e descomprimir (`zstd`).
//! 4. Concatenar em arquivo temporário.
//! 5. Validar resultado (hash BLAKE3, se necessário).
//! 6. Mover atômico para destino final (com verificação de conflito).

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use anyhow::{Result, Context};
use crate::pipeline::{crypto, compress};

/// Reconstrói um arquivo restaurado a partir do repositório.
/// `key` é a chave AES-256 (32 bytes).
/// Se `force` é false e o arquivo de destino já existe, retorna erro.
pub fn restore_file(
    chunk_hashes: &[(String, i32)], // ordenados por `order_index`
    dest_path: &Path,
    data_root: &Path,
    key: &[u8; 32],
    force: bool,
) -> Result<()> {
    if dest_path.exists() && !force {
        anyhow::bail!(
            "Arquivo de destino já existe e `force` não está ativado: {}",
            dest_path.display()
        );
    }
    let parent = dest_path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;

    let tmp_path = parent.join(format!(
        ".restored_tmp_{}",
        std::process::id()
    ));
    {
        let mut output = File::create(&tmp_path)?;
        for (hash_id, _order) in chunk_hashes {
            let prefix = if hash_id.len() >= 2 {
                &hash_id[..2]
            } else {
                "00"
            };
            let chunk_path = data_root.join("data").join(prefix).join(hash_id);
            if !chunk_path.exists() {
                anyhow::bail!(
                    "Chunk físico ausente no repositório: {} (procurado em {})",
                    hash_id,
                    chunk_path.display()
                );
            }
            let mut chunk_file = File::open(&chunk_path)
                .with_context(|| format!("Falha ao ler chunk: {}", chunk_path.display()))?;
            let mut encrypted_data = Vec::new();
            chunk_file.read_to_end(&mut encrypted_data)?;

            // Descriptografa e descomprime
            let decrypted = crypto::decrypt(&encrypted_data, key);
            let decompressed = compress::decompress(&decrypted);
            output.write_all(&decompressed)?;
        }
        output.sync_all()?;
    }

    // Validação simples: arquivo temporário deve existir e ter conteúdo
    let meta = fs::metadata(&tmp_path)?;
    if meta.len() == 0 {
        anyhow::bail!("Arquivo restaurado está vazio: {}", tmp_path.display());
    }

    fs::rename(&tmp_path, dest_path)?;
    Ok(())
}
