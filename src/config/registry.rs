//! Per-user opt-in repository registry. Never stores keys or scans disks.
use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Default, Serialize, Deserialize)]
struct Registry {
    repositories: Vec<PathBuf>,
}

pub fn directory() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME").filter(|p| !p.is_empty()) {
        let path = PathBuf::from(path);
        anyhow::ensure!(path.is_absolute(), "XDG_CONFIG_HOME deve ser absoluto");
        return Ok(path.join("redbackup"));
    }
    Ok(
        PathBuf::from(std::env::var_os("HOME").context("HOME não definido")?)
            .join(".config/redbackup"),
    )
}

pub fn list_at(dir: &Path) -> Result<Vec<PathBuf>> {
    match fs::read(dir.join("repositories.json")) {
        Ok(raw) => {
            let registry: Registry =
                serde_json::from_slice(&raw).context("Cadastro de repositórios inválido")?;
            anyhow::ensure!(
                registry.repositories.iter().all(|p| p.is_absolute()),
                "Cadastro contém caminho relativo"
            );
            Ok(registry.repositories)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

pub fn update_at(dir: &Path, root: &Path, add: bool) -> Result<()> {
    let root = if add {
        crate::repository::open_repository(root)?
    } else {
        root.canonicalize().unwrap_or(if root.is_absolute() {
            root.into()
        } else {
            std::env::current_dir()?.join(root)
        })
    };
    if add {
        crate::config::settings::Settings::load(&root)?;
    }
    fs::create_dir_all(dir)?;
    crate::config::validation::set_directory_permissions(dir, 0o700)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options.open(dir.join("registry.lock"))?;
    lock.lock_exclusive()?;
    let mut repositories = list_at(dir)?;
    repositories.retain(|p| p != &root);
    if add {
        repositories.push(root);
    }
    repositories.sort();
    repositories.dedup();
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    temp.write_all(&serde_json::to_vec_pretty(&Registry { repositories })?)?;
    temp.as_file().sync_all()?;
    temp.persist(dir.join("repositories.json"))?;
    #[cfg(unix)]
    {
        std::fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

pub fn register(root: &Path) -> Result<()> {
    update_at(&directory()?, root, true)
}
