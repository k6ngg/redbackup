//! Validação de caminhos e permissões.
//!
//! Toda construção de caminho usada em operações de leitura/escrita deve passar
//! por estas funções para evitar travessia de diretório e concatenação manual.

use anyhow::{Context, Result};
use std::path::{Component, Path, PathBuf};

/// Valida que o caminho é seguro (não contém traversal) e, opcionalmente, existe.
pub fn validate_path(path: &str, must_exist: bool) -> Result<PathBuf> {
    if path.trim().is_empty() {
        anyhow::bail!("Caminho vazio não é permitido");
    }

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

/// Garante que um caminho relativo persistido não escape do diretório de destino.
pub fn validate_relative_path(path: &str) -> Result<()> {
    if path.trim().is_empty() {
        anyhow::bail!("Caminho relativo vazio não é permitido");
    }
    if path.contains('\0') {
        anyhow::bail!("Caminho contém byte NUL");
    }
    if path.contains('\\') {
        // Evita que metadados gerados em Unix sejam interpretados como
        // separadores ao restaurar em Windows (e vice-versa).
        anyhow::bail!("Caminho persistido não pode conter '\\': {}", path);
    }

    let candidate = Path::new(path);
    if candidate.is_absolute() {
        anyhow::bail!("Caminho persistido deve ser relativo: {}", path);
    }
    for component in candidate.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                anyhow::bail!("Caminho persistido contém travessia: {}", path);
            }
        }
    }
    Ok(())
}

/// Converte um caminho absoluto em caminho relativo seguro, com separador `/`.
///
/// A normalização é feita por componentes para que caminhos do Windows
/// (`sub\\arquivo`) sejam persistidos como `sub/arquivo`, sem depender do
/// separador nativo. Nomes Unix que contenham `\\` literal continuam rejeitados
/// por [`validate_relative_path`], pois não seriam restauráveis em Windows.
pub fn relative_path_for_metadata(root: &Path, path: &Path) -> Result<String> {
    let relative = path
        .strip_prefix(root)
        .with_context(|| format!("Caminho fora da origem: {}", path.display()))?;

    let mut components = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(value) => {
                let text = value.to_str().ok_or_else(|| {
                    anyhow::anyhow!("Caminho não é UTF-8: {}", relative.display())
                })?;
                components.push(text);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                anyhow::bail!("Caminho inválido para metadados: {}", relative.display())
            }
        }
    }

    if components.is_empty() {
        anyhow::bail!("Caminho relativo vazio não é permitido");
    }
    let text = components.join("/");
    validate_relative_path(&text)?;
    Ok(text)
}

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Garante que um diretório tem permissões restritas.
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

/// Garante que um arquivo sensível (configuração, chave) tenha modo restrito.
pub fn set_file_permissions(path: &Path, mode: u32) -> Result<()> {
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
