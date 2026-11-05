//! Cópias redundantes locais de chunks. Não substitui o backup dos metadados.
//!
//! O estado persistido descreve a última sincronização, não uma garantia de que
//! o destino não foi alterado posteriormente. Nenhuma cópia é removida aqui.
use crate::config::settings::Settings;
use crate::metadata::repository::{connect_or_create, verify_snapshot_integrity};
use crate::pipeline::{verify::validated_chunk_size, write};
use crate::repository::key_binding;
use crate::repository::lock::RepositoryLock;
use crate::repository::{chunk_path, config_dir, index_db_path, metadata_dir};
use anyhow::{Context, Result};
use rand::RngCore;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    BestEffort,
    Required,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    pub destination: PathBuf,
    pub mode: Mode,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub state: String,
    pub detail: Option<String>,
}

fn config_path(root: &Path) -> PathBuf {
    config_dir(root).join("redundancy.json")
}
fn state_path(root: &Path) -> PathBuf {
    metadata_dir(root).join("redundancy-state.json")
}
fn marker_path(root: &Path) -> PathBuf {
    root.join("redbackup-redundancy.json")
}

pub fn load(root: &Path) -> Result<Option<Config>> {
    let bytes = match fs::read(config_path(root)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let config: Config =
        serde_json::from_slice(&bytes).context("Configuração de redundância inválida")?;
    if config.version != 1
        || config.id.len() != 64
        || !config
            .id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        || !config.destination.is_absolute()
    {
        anyhow::bail!("Configuração de redundância inválida ou versão desconhecida");
    }
    Ok(Some(config))
}

pub fn read_state(root: &Path) -> Result<Option<State>> {
    match fs::read(state_path(root)) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Diagnóstico somente de disponibilidade; não substitui nova verificação dos dados.
pub fn observed_state(root: &Path) -> Result<State> {
    let Some(config) = load(root)? else {
        return Ok(State {
            state: "disabled".into(),
            detail: None,
        });
    };
    if let Err(error) = validate_destination(root, &config) {
        return Ok(State {
            state: "unavailable".into(),
            detail: Some(format!("{:#}", error)),
        });
    }
    Ok(read_state(root)?.unwrap_or(State {
        state: "pending".into(),
        detail: None,
    }))
}

pub fn set_state(root: &Path, state: &str, detail: Option<String>) -> Result<()> {
    write::atomic_write(
        &serde_json::to_vec(&State {
            state: state.into(),
            detail,
        })?,
        &state_path(root),
    )?;
    Ok(())
}

/// Configuração inicial exige diretório existente e vazio; nunca adota dados alheios.
pub fn configure(root: &Path, settings: &Settings, destination: &Path, mode: Mode) -> Result<()> {
    let _lock = RepositoryLock::exclusive(root)?;
    if load(root)?.is_some() {
        anyhow::bail!("Redundância já configurada; não é permitido sobrescrever a configuração");
    }
    let root = root.canonicalize()?;
    let source = Path::new(&settings.source_path).canonicalize()?;
    let dest = destination
        .canonicalize()
        .context("Destino redundante deve existir")?;
    if !dest.is_dir()
        || dest.starts_with(&root)
        || root.starts_with(&dest)
        || dest.starts_with(&source)
        || source.starts_with(&dest)
    {
        anyhow::bail!("Destino redundante deve ser diretório independente, sem sobreposição com origem ou repositório");
    }
    if fs::read_dir(&dest)?.next().transpose()?.is_some() {
        anyhow::bail!("Destino redundante deve estar vazio");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if fs::metadata(&root)?.dev() == fs::metadata(&dest)?.dev() {
            eprintln!("AVISO: destino redundante e repositório estão no mesmo dispositivo; falha do disco pode afetar ambos.");
        }
    }
    let mut random = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut random);
    let id = blake3::Hash::from_bytes(random).to_hex().to_string();
    let config = Config {
        version: 1,
        destination: dest.clone(),
        mode,
        id,
    };
    // create_new impede que outra configuração concorrente adote o mesmo destino.
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut marker = options.open(marker_path(&dest))?;
    marker.write_all(config.id.as_bytes())?;
    marker.sync_all()?;
    write::sync_existing_file(&marker_path(&dest))?;
    // Persistir primeiro o estado pendente impede que configuração recém-ativada
    // pareça protegida antes de copiar snapshots existentes.
    set_state(&root, "pending", None)?;
    write::atomic_write(&serde_json::to_vec_pretty(&config)?, &config_path(&root))?;
    Ok(())
}

fn validate_destination(root: &Path, config: &Config) -> Result<()> {
    let dest = config
        .destination
        .canonicalize()
        .context("Destino redundante indisponível")?;
    if dest != config.destination || dest.starts_with(root) || root.starts_with(&dest) {
        anyhow::bail!("Destino redundante mudou ou sobrepõe o repositório");
    }
    if let Ok(settings) = Settings::load(root) {
        if let Ok(source) = Path::new(&settings.source_path).canonicalize() {
            if dest.starts_with(&source) || source.starts_with(&dest) {
                anyhow::bail!("Destino redundante sobrepõe a origem atual");
            }
        }
    }
    if fs::read_to_string(marker_path(&dest))? != config.id {
        anyhow::bail!("Identidade do destino redundante não confere");
    }
    Ok(())
}

/// Sincroniza snapshots concluídos e, opcionalmente, o snapshot em andamento.
/// O chamador mantém o lock exclusivo do repositório principal.
pub fn sync_locked(
    root: &Path,
    conn: &Connection,
    config: &Config,
    key: &[u8; 32],
    current: Option<i64>,
) -> Result<usize> {
    validate_destination(root, config)?;
    let mut snapshots =
        conn.prepare("SELECT id FROM snapshots WHERE status='completed' OR id=?1")?;
    for id in snapshots.query_map([current], |row| row.get::<_, i64>(0))? {
        let id = id?;
        let problems = verify_snapshot_integrity(conn, id, root)?;
        if !problems.is_empty() {
            anyhow::bail!("Snapshot {} inconsistente: {}", id, problems.join("; "));
        }
    }
    let mut stmt = conn.prepare("SELECT DISTINCT c.hash_id,c.size FROM chunks c JOIN file_chunks fc ON fc.chunk_hash=c.hash_id JOIN files f ON f.id=fc.file_id JOIN snapshots s ON s.id=f.snapshot_id WHERE s.status='completed' OR s.id=?1")?;
    let chunks = stmt.query_map([current], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })?;
    let mut count = 0;
    for chunk in chunks {
        let (hash, size) = chunk?;
        let expected = u64::try_from(size).context("Tamanho de chunk inválido")?;
        let source = chunk_path(root, &hash)?;
        let target = chunk_path(&config.destination, &hash)?;
        // Nunca siga diretórios simbólicos criados no destino redundante.
        for dir in [
            config.destination.join("data"),
            target
                .parent()
                .context("Chunk sem diretório")?
                .to_path_buf(),
        ] {
            match fs::symlink_metadata(&dir) {
                Ok(meta) if !meta.file_type().is_dir() => {
                    anyhow::bail!("Diretório redundante inseguro: {}", dir.display())
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        match fs::symlink_metadata(&target) {
            Ok(meta) => {
                if !meta.file_type().is_file()
                    || validated_chunk_size(&target, key, &hash)? != Some(expected)
                {
                    anyhow::bail!(
                        "Cópia redundante inválida; preservada para diagnóstico: {}",
                        hash
                    );
                }
                write::sync_existing_file(&target)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if validated_chunk_size(&source, key, &hash)? != Some(expected) {
                    anyhow::bail!("Chunk principal inválido: {}", hash);
                }
                let bytes = fs::read(&source)?;
                write::atomic_write(&bytes, &target)?;
                if validated_chunk_size(&target, key, &hash)? != Some(expected) {
                    anyhow::bail!("Cópia redundante recém-gravada inválida: {}", hash);
                }
            }
            Err(error) => return Err(error.into()),
        }
        count += 1;
    }
    Ok(count)
}

pub fn sync_repository(root: &Path, key: &[u8; 32]) -> Result<usize> {
    let _lock = RepositoryLock::exclusive(root)?;
    let config = load(root)?.context("Redundância não configurada")?;
    let conn = connect_or_create(
        index_db_path(root)
            .to_str()
            .context("Caminho do banco não é UTF-8")?,
    )?;
    key_binding::require_matching_key_if_bound(&conn, key)?;
    if key_binding::stored_verifier(&conn)?.is_none() {
        anyhow::bail!("Repositório sem chave vinculada");
    }
    set_state(root, "pending", None)?;
    match sync_locked(root, &conn, &config, key, None) {
        Ok(count) => {
            set_state(root, "protected", None)?;
            Ok(count)
        }
        Err(error) => {
            set_state(root, "degraded", Some(format!("{:#}", error)))?;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::run_backup;
    use std::process::Command;
    const KEY: [u8; 32] = [12; 32];

    #[test]
    fn child_sync() {
        let Ok(root) = std::env::var("REDBACKUP_REDUNDANCY_TEST_ROOT") else {
            return;
        };
        sync_repository(&Path::new(&root).join("repo"), &KEY).unwrap();
        panic!("o ponto de interrupção não foi alcançado");
    }

    #[test]
    fn interrupcao_de_copia_pode_ser_repetida() {
        for point in ["chunk_write", "before_rename", "after_rename"] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            let source = root.join("source");
            let repo = root.join("repo");
            let destination = root.join("dest");
            fs::create_dir(&source).unwrap();
            fs::create_dir(&destination).unwrap();
            fs::write(source.join("file"), b"conteudo recuperavel").unwrap();
            let settings = Settings {
                source_path: source.display().to_string(),
                destination_path: repo.display().to_string(),
                chunk_min_size: 64,
                chunk_max_size: 1024,
                ..Settings::default()
            };
            run_backup(&repo, &settings, &KEY).unwrap();
            configure(&repo, &settings, &destination, Mode::Required).unwrap();
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "redundancy::tests::child_sync"])
                .env("REDBACKUP_REDUNDANCY_TEST_ROOT", root)
                .env("REDBACKUP_TEST_CRASH_POINT", point)
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(86),
                "{}: {}",
                point,
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(read_state(&repo).unwrap().unwrap().state, "pending");
            assert_eq!(sync_repository(&repo, &KEY).unwrap(), 1);
            assert_eq!(sync_repository(&repo, &KEY).unwrap(), 1);
            assert_eq!(read_state(&repo).unwrap().unwrap().state, "protected");
        }
    }
}
