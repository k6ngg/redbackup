//! Leitura e geração segura do arquivo de chave AES-256.
//!
//! Formatos aceitos:
//! - 32 bytes binários; ou
//! - 64 caracteres hexadecimais (com quebra de linha opcional).
//!
//! A chave nunca é exibida em logs nem gravada em `settings.json`.

use crate::config::validation::set_file_permissions;
use anyhow::{Context, Result};
use rand::RngCore;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

/// Lê e valida uma chave de 32 bytes.
pub fn load_key(path: &Path) -> Result<[u8; 32]> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("Arquivo de chave não encontrado: {}", path.display()))?;
    if !metadata.file_type().is_file() {
        anyhow::bail!(
            "O caminho da chave não é um arquivo regular: {}",
            path.display()
        );
    }

    #[cfg(unix)]
    {
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            // Corrige automaticamente permissões excessivamente abertas. A
            // alternativa seria recusar a operação e deixar a chave exposta.
            set_file_permissions(path, 0o600)
                .with_context(|| format!("Falha ao restringir permissões de {}", path.display()))?;
            eprintln!(
                "AVISO: permissões de {} ajustadas para 0600.",
                path.display()
            );
        }
    }

    let mut file =
        File::open(path).with_context(|| format!("Falha ao abrir chave: {}", path.display()))?;
    let mut raw = Vec::new();
    file.read_to_end(&mut raw)
        .with_context(|| format!("Falha ao ler chave: {}", path.display()))?;

    if raw.len() == 32 {
        let mut key = [0u8; 32];
        key.copy_from_slice(&raw);
        return Ok(key);
    }

    if let Ok(text) = std::str::from_utf8(&raw) {
        let trimmed = text.trim();
        if trimmed.len() == 64 && trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
            let mut key = [0u8; 32];
            for (index, byte) in key.iter_mut().enumerate() {
                let start = index * 2;
                *byte = u8::from_str_radix(&trimmed[start..start + 2], 16)
                    .context("Chave hexadecimal inválida")?;
            }
            return Ok(key);
        }
    }

    anyhow::bail!(
        "Arquivo de chave inválido: esperado 32 bytes binários ou 64 caracteres hexadecimais ({})",
        path.display()
    )
}

/// Cria uma chave aleatória em `path`. Recusa sobrescrever arquivo existente.
pub fn generate_key_file(path: &Path) -> Result<()> {
    if path.exists() {
        anyhow::bail!(
            "Arquivo de chave já existe; não será sobrescrito: {}",
            path.display()
        );
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            anyhow::bail!("Diretório da chave não existe: {}", parent.display());
        }
    }

    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);

    let mut file = options
        .open(path)
        .with_context(|| format!("Falha ao criar arquivo de chave: {}", path.display()))?;
    file.write_all(&key)
        .with_context(|| format!("Falha ao gravar chave: {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("Falha ao sincronizar chave: {}", path.display()))?;
    set_file_permissions(path, 0o600)?;
    Ok(())
}

/// Garante que a chave exista, gerando-a quando o caminho ainda não existe.
pub fn ensure_key_file(path: &Path) -> Result<[u8; 32]> {
    if !path.exists() {
        generate_key_file(path)?;
        eprintln!(
            "AVISO: nova chave gerada em {}. Guarde-a em local seguro; sem ela os backups não podem ser restaurados.",
            path.display()
        );
    }
    load_key(path)
}
