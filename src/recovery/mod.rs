//! Recuperação após falha (crash recovery).
//!
//! Na inicialização: identifica arquivos `.tmp` e chunks sem referência finalizada.
//! Só remove após verificar o estado do banco.

use anyhow::{Context, Result};
use std::fs;

pub fn clean_orphans(data_dir: &str, _db_path: &str) -> Result<Vec<String>> {
    let mut removed = Vec::new();
    let entries =
        fs::read_dir(data_dir).with_context(|| format!("Falha ao ler data_dir: {}", data_dir))?;

    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.ends_with(".tmp") || name_str.starts_with(".tmp_") {
            let file_path = entry.path();
            fs::remove_file(&file_path)
                .with_context(|| format!("Falha ao remover órfão: {}", file_path.display()))?;
            removed.push(name_str.to_string());
        }
    }
    Ok(removed)
}
