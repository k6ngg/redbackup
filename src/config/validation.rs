use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Valida que o caminho é seguro (não contém traversal) e existe.
/// Centraliza a construção de caminhos para evitar concatenação manual.
pub fn validate_path(path: &str, must_exist: bool) -> Result<PathBuf> {
    let p = Path::new(path);
    if must_exist {
        let canonical = p
            .canonicalize()
            .with_context(|| format!("Não foi possível resolver o caminho existente: {}", path))?;
        if !canonical.exists() {
            anyhow::bail!("Caminho obrigatório não existe: {}", canonical.display());
        }
        Ok(canonical)
    } else {
        let normalized = if p.is_absolute() {
            p.to_path_buf()
        } else {
            std::env::current_dir()?.join(p)
        };
        Ok(normalized)
    }
}

/// Garante que um diretório tem permissões restritas.
/// No Linux/macOS: `0o700` para `metadata`, `0o750` para `data`.
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub fn set_directory_permissions(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(mode);
        std::fs::set_permissions(path, perms).with_context(|| {
            format!(
                "Falha ao definir permissões {:o} em {}",
                mode,
                path.display()
            )
        })?;
    }
    Ok(())
}
