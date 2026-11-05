//! Publicação segura de cópias redundantes, sob o lock do repositório.
use crate::pipeline::{
    verify::{validated_chunk_bytes, validated_chunk_size},
    write::sync_existing_file,
};
use crate::redundancy::{self, Config};
use crate::repository::{chunk_path, metadata_dir};
use anyhow::{Context, Result};
use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
};

/// Nunca segue symlinks nos diretórios controlados pelo repositório.
fn check_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("Diretório ausente: {}", path.display()))?;
    if !metadata.file_type().is_dir() {
        anyhow::bail!("Diretório inseguro: {}", path.display());
    }
    Ok(())
}

fn preserve_original(root: &Path, original: &Path, hash: &str) -> Result<Option<PathBuf>> {
    match fs::symlink_metadata(original) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(metadata) if !metadata.file_type().is_file() => anyhow::bail!(
            "Chunk original não é arquivo regular: {}",
            original.display()
        ),
        Ok(_) => {}
    }
    let metadata = metadata_dir(root);
    check_directory(&metadata)?;
    let quarantine = metadata.join("quarantine");
    match fs::create_dir(&quarantine) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    check_directory(&quarantine)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&quarantine, fs::Permissions::from_mode(0o700))?;
    }
    let mut saved = tempfile::Builder::new()
        .prefix(&format!("corrupt-{}-", hash))
        .tempfile_in(&quarantine)?;
    io::copy(&mut File::open(original)?, &mut saved)?;
    saved.as_file().sync_all()?;
    let (_, path) = saved.keep().map_err(|error| error.error)?;
    sync_existing_file(&path)?;
    Ok(Some(path))
}

pub(super) fn repair(
    root: &Path,
    config: &Config,
    hash: &str,
    size: i64,
    key: &[u8; 32],
) -> Result<Option<PathBuf>> {
    redundancy::validate_destination(root, config)?;
    let expected = u64::try_from(size).context("Tamanho de chunk inválido")?;
    let candidate = chunk_path(&config.destination, hash)?;
    check_directory(&config.destination.join("data"))?;
    check_directory(
        candidate
            .parent()
            .context("Chunk redundante sem diretório")?,
    )?;
    if !fs::symlink_metadata(&candidate)?.file_type().is_file() {
        anyhow::bail!("Cópia redundante não é arquivo regular");
    }
    let bytes = fs::read(&candidate).context("Não foi possível ler a cópia redundante")?;
    if validated_chunk_bytes(&bytes, key, hash)? != Some(expected) {
        anyhow::bail!("Cópia redundante não corresponde ao hash ou tamanho esperado");
    }

    let target = chunk_path(root, hash)?;
    let data = root.join("data");
    check_directory(&data)?;
    let parent = target.parent().context("Chunk principal sem diretório")?;
    match fs::create_dir(parent) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    check_directory(parent)?;
    let mut temp = tempfile::Builder::new()
        .prefix(".tmp_repair_")
        .tempfile_in(parent)?;
    temp.write_all(&bytes)?;
    temp.as_file().sync_all()?;
    if validated_chunk_size(temp.path(), key, hash)? != Some(expected) {
        anyhow::bail!("Temporário de reparo inválido");
    }
    #[cfg(test)]
    crate::crash_point("repair_before_preserve");
    let saved = preserve_original(root, &target, hash)?;
    #[cfg(test)]
    crate::crash_point("repair_before_publish");
    temp.persist(&target)
        .map_err(|error| error.error)
        .context("Falha ao publicar chunk reparado")?;
    #[cfg(test)]
    crate::crash_point("repair_after_publish");
    sync_existing_file(&target).context("Falha ao sincronizar chunk reparado")?;
    if validated_chunk_size(&target, key, hash)? != Some(expected) {
        anyhow::bail!("Chunk reparado não passou na verificação final");
    }
    Ok(saved)
}
