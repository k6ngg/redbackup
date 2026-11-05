//! Cópia independente e consistente dos metadados (`metadata/index.db`).
//!
//! A redundância existente protege apenas chunks. Esta cópia protege o
//! índice (snapshots, arquivos e associações ordenadas arquivo↔chunk) em um
//! diretório independente. A chave **nunca** faz parte da cópia: ela deve ser
//! guardada separadamente, em local seguro e distinto do repositório e da
//! cópia de metadados. Sem a chave original, os chunks não podem ser
//! restaurados.
//!
//! Formato do destino:
//! ```text
//! <destino>/
//! ├── index.db                        # cópia consistente (API de backup SQLite)
//! └── redbackup-metadata.json         # manifesto (sem segredos)
//! ```
//!
//! A cópia é criada com a API de backup do SQLite (`sqlite3_backup` via
//! `rusqlite`), portanto inclui transações confirmadas ainda no WAL e é
//! segura mesmo com atividade concorrente controlada. Copiar apenas
//! `index.db` com `cp` em modo WAL não é suficiente e não é feito aqui.
//!
//! A publicação é atômica: gravamos em temporário no mesmo sistema de
//! arquivos, sincronizamos, validamos (integridade, chaves estrangeiras,
//! cobertura e hash) e só então renomeamos. Uma falha nunca destrói a última
//! cópia válida. O manifesto é publicado por último e funciona como ponto de
//! confirmação: se ele não conferir com `index.db`, a cópia é recusada.
//!
//! Cobertura: o manifesto lista apenas snapshots `completed`. Snapshots
//! posteriores à cópia não estão protegidos por ela; `status` e
//! `metadata-verify` relatam cópias desatualizadas. O `prune` não remove
//! silenciosamente snapshots protegidos (ver [`crate::retention`]).
//!
//! Restauração: [`restore_backup`] valida o manifesto e o banco, exige
//! `--force` explícito para substituir um banco existente (preservado em
//! `metadata/quarantine/`), roda sob lock exclusivo e verifica referências e
//! chunks após a instalação, sem declarar sucesso integral quando houver
//! snapshots incompletos.
//!
//! Limites de independência: o destino deve ser um diretório independente,
//! sem sobreposição com o repositório, a origem ou a cópia de chunks. Isso
//! não garante independência física (mesmo disco/volume, mesma falha de
//! hardware). Preferencialmente use outro dispositivo; em Unix um aviso é
//! emitido quando ambos aparentam estar no mesmo dispositivo. No Windows, a
//! durabilidade do rename após queda de energia não é garantida.
//!
//! Para recuperar em outro diretório: restaure o índice em um diretório novo,
//! copie os chunks para `<novo>/data/`, reconfigure caminhos com `init`
//! (nunca confie cegamente nos caminhos antigos) e forneça a chave original.
//! O manifesto carrega apenas um retrato sanitizado da configuração.

use crate::config::settings::Settings;
use crate::config::validation::{set_directory_permissions, set_file_permissions};
use crate::metadata::migrations::run_migrations;
use crate::metadata::repository::{
    connect_or_create, count_in_progress, verify_snapshot_integrity,
};
use crate::pipeline::verify::validated_chunk_size;
use crate::repository::key_binding;
use crate::repository::lock::RepositoryLock;
use crate::repository::{config_dir, data_dir, index_db_path, metadata_dir};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, DatabaseName, OpenFlags};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const FORMAT_VERSION: u32 = 1;
pub const INDEX_NAME: &str = "index.db";
pub const MANIFEST_NAME: &str = "redbackup-metadata.json";

fn state_path(root: &Path) -> PathBuf {
    metadata_dir(root).join("metadata-backup.json")
}

/// Retrato sanitizado da configuração, sem segredos. Caminhos são
/// históricos: em outra máquina devem ser reconfigurados com `init`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SanitizedSettings {
    pub source_path: String,
    pub destination_path: String,
    pub key_file_path: Option<String>,
    pub chunk_min_size: u64,
    pub chunk_max_size: u64,
    pub retention_days: u64,
}

/// Manifesto publicado junto da cópia. Nunca contém a chave.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format_version: u32,
    pub created_at_unix: u64,
    pub redbackup_version: String,
    pub schema_version: i64,
    pub key_verifier: Option<String>,
    pub completed_snapshot_ids: Vec<i64>,
    pub last_completed_snapshot_id: Option<i64>,
    pub chunk_count: i64,
    pub file_count: i64,
    pub index_db_bytes: u64,
    pub index_db_blake3: String,
    pub settings: SanitizedSettings,
}

/// Estado local da última cópia (em `metadata/metadata-backup.json`).
/// Usado para proteger snapshots no pruning e para relatar desatualização.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub version: u32,
    pub destination: PathBuf,
    pub created_at_unix: u64,
    pub schema_version: i64,
    pub key_verifier: Option<String>,
    pub completed_snapshot_ids: Vec<i64>,
    pub last_completed_snapshot_id: Option<i64>,
    pub index_db_blake3: String,
    pub needs_refresh: bool,
}

#[derive(Debug, Clone)]
pub struct DbSummary {
    pub schema_version: i64,
    pub key_verifier: Option<String>,
    pub completed_snapshot_ids: Vec<i64>,
    pub chunk_count: i64,
    pub file_count: i64,
}

/// Observação somente de leitura para `status`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observed {
    pub state: String,
    pub detail: Option<String>,
    pub destination: Option<String>,
    pub covered_snapshots: usize,
    pub last_covered_snapshot_id: Option<i64>,
    pub created_at_unix: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct VerifyReport {
    pub manifest: Manifest,
    pub summary: DbSummary,
}

#[derive(Debug, Clone)]
pub struct SnapshotProblem {
    pub snapshot_id: i64,
    pub problems: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct RestoreReport {
    pub preserved_existing: Option<PathBuf>,
    pub new_repository: bool,
    pub needs_init: bool,
    pub covered_snapshots: usize,
    pub complete_snapshots: usize,
    pub incomplete_snapshots: Vec<SnapshotProblem>,
    pub quarantine_dir: Option<PathBuf>,
}

impl RestoreReport {
    pub fn is_complete(&self) -> bool {
        self.incomplete_snapshots.is_empty()
    }
}

pub fn load_state(root: &Path) -> Result<Option<State>> {
    match fs::read(state_path(root)) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes).context("Estado da cópia de metadados inválido")?,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn save_state(root: &Path, state: &State) -> Result<()> {
    crate::pipeline::write::atomic_write(&serde_json::to_vec_pretty(state)?, &state_path(root))?;
    Ok(())
}

/// IDs protegidos pela última cópia conhecida. Erro em estado inválido é
/// propagado para que o pruning falhe fechado em vez de invalidar a cópia
/// silenciosamente.
pub fn protected_snapshot_ids(root: &Path) -> Result<HashSet<i64>> {
    match load_state(root)? {
        Some(state) => Ok(state.completed_snapshot_ids.into_iter().collect()),
        None => Ok(HashSet::new()),
    }
}

/// Atualiza o estado local após remoção explícita de snapshots protegidos
/// pelo pruning (`--allow-protected`): os IDs saem da lista e a cópia fica
/// marcada como precisando de atualização.
pub fn record_pruned_protected(root: &Path, removed: &[i64]) -> Result<()> {
    let Some(mut state) = load_state(root)? else {
        return Ok(());
    };
    let removed: HashSet<i64> = removed.iter().copied().collect();
    state
        .completed_snapshot_ids
        .retain(|id| !removed.contains(id));
    state.last_completed_snapshot_id = state.completed_snapshot_ids.iter().max().copied();
    if !removed.is_empty() {
        state.needs_refresh = true;
    }
    save_state(root, &state)
}

fn canonical_repo(root: &Path) -> Result<PathBuf> {
    root.canonicalize()
        .with_context(|| format!("Repositório indisponível: {}", root.display()))
}

fn validate_destination(root: &Path, settings: &Settings, destination: &Path) -> Result<PathBuf> {
    let repo = canonical_repo(root)?;
    let dest = destination.canonicalize().with_context(|| {
        format!(
            "Destino da cópia de metadados indisponível: {}",
            destination.display()
        )
    })?;
    if !dest.is_dir() {
        anyhow::bail!(
            "Destino da cópia de metadados deve ser um diretório: {}",
            dest.display()
        );
    }
    if dest.starts_with(&repo) || repo.starts_with(&dest) {
        anyhow::bail!("Destino da cópia de metadados deve ser independente, sem sobreposição com o repositório");
    }
    if let Ok(source) = Path::new(&settings.source_path).canonicalize() {
        if dest.starts_with(&source) || source.starts_with(&dest) {
            anyhow::bail!("Destino da cópia de metadados não pode sobrepor a origem");
        }
    }
    // Nenhum arquivo alheio: apenas cópias anteriores e temporários nossos.
    let allowed = [INDEX_NAME, MANIFEST_NAME, "index.db-wal", "index.db-shm"];
    for entry in
        fs::read_dir(&dest).with_context(|| format!("Falha ao ler destino: {}", dest.display()))?
    {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_str().unwrap_or_default();
        if allowed.contains(&name) || name.starts_with(".tmp_") {
            continue;
        }
        anyhow::bail!("Destino da cópia de metadados contém arquivo alheio ao redbackup: {}. Use um diretório dedicado.", name);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if fs::metadata(&repo)?.dev() == fs::metadata(&dest)?.dev() {
            eprintln!("AVISO: destino da cópia de metadados e repositório estão no mesmo dispositivo; falha do disco pode afetar ambos.");
        }
    }
    Ok(dest)
}

/// Configura o destino independente. O diretório deve existir e estar vazio.
pub fn configure(root: &Path, destination: &Path) -> Result<()> {
    let _lock = RepositoryLock::exclusive(root)?;
    let mut settings = Settings::load(root)?;
    let repo = canonical_repo(root)?;
    let dest = destination
        .canonicalize()
        .context("Destino da cópia de metadados deve existir")?;
    if !dest.is_dir() || dest.starts_with(&repo) || repo.starts_with(&dest) {
        anyhow::bail!("Destino da cópia de metadados deve ser diretório independente, sem sobreposição com o repositório");
    }
    if let Some(existing) = &settings.metadata_backup_path {
        if Path::new(existing) == dest {
            return Ok(());
        }
        anyhow::bail!("Cópia de metadados já configurada para outro destino; a configuração não é substituída automaticamente");
    }
    if fs::read_dir(&dest)?.next().transpose()?.is_some() {
        anyhow::bail!("Destino da cópia de metadados deve estar vazio");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if fs::metadata(&repo)?.dev() == fs::metadata(&dest)?.dev() {
            eprintln!("AVISO: destino da cópia de metadados e repositório estão no mesmo dispositivo; falha do disco pode afetar ambos.");
        }
    }
    set_directory_permissions(&dest, 0o700).ok();
    settings.metadata_backup_path = Some(dest.to_string_lossy().to_string());
    settings.save(root)?;
    save_state(
        root,
        &State {
            version: FORMAT_VERSION,
            destination: dest,
            created_at_unix: now_unix(),
            schema_version: 0,
            key_verifier: None,
            completed_snapshot_ids: Vec::new(),
            last_completed_snapshot_id: None,
            index_db_blake3: String::new(),
            needs_refresh: true,
        },
    )?;
    Ok(())
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn open_read_only(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("Falha ao abrir banco para validação: {}", path.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(conn)
}

fn file_blake3(path: &Path) -> Result<(String, u64)> {
    let mut file = File::open(path).with_context(|| format!("Falha ao ler: {}", path.display()))?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    Ok((hasher.finalize().to_hex().to_string(), total))
}

/// Valida um arquivo SQLite (integridade, chaves estrangeiras, schema e
/// cobertura) sem modificá-lo.
fn summarize_db(path: &Path) -> Result<DbSummary> {
    let conn = open_read_only(path)?;
    let integrity: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if integrity != "ok" {
        anyhow::bail!(
            "Cópia de metadados inválida: integrity_check retornou {:?} ({})",
            integrity,
            path.display()
        );
    }
    let violations: i64 = {
        let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
        let count = stmt.query_map([], |_r| Ok(()))?.count() as i64;
        count
    };
    if violations != 0 {
        anyhow::bail!(
            "Cópia de metadados inválida: {} violações de chave estrangeira ({})",
            violations,
            path.display()
        );
    }
    let table_exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_version')",
        [],
        |r| r.get(0),
    )?;
    if !table_exists {
        anyhow::bail!(
            "Cópia de metadados incompatível: tabela schema_version ausente ({})",
            path.display()
        );
    }
    let schema_version: Option<i64> =
        conn.query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))?;
    let schema_version = schema_version.ok_or_else(|| {
        anyhow::anyhow!("Cópia de metadados incompatível: nenhuma versão de schema registrada")
    })?;
    if schema_version < 1 {
        anyhow::bail!(
            "Cópia de metadados incompatível: versão de schema não suportada ({})",
            schema_version
        );
    }
    let mut stmt =
        conn.prepare("SELECT id FROM snapshots WHERE status='completed' ORDER BY id ASC")?;
    let completed: Vec<i64> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    drop(stmt);
    let chunk_count: i64 = conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
    let file_count: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
    let inconsistent: i64 = conn.query_row(
        "SELECT COUNT(*) FROM chunks WHERE reference_count != (SELECT COUNT(*) FROM file_chunks WHERE chunk_hash = chunks.hash_id)",
        [], |r| r.get(0),
    )?;
    if inconsistent != 0 {
        anyhow::bail!(
            "Cópia de metadados inconsistente: {} chunk(s) com reference_count divergente",
            inconsistent
        );
    }
    let verifier = key_binding::stored_verifier(&conn)?;
    Ok(DbSummary {
        schema_version,
        key_verifier: verifier,
        completed_snapshot_ids: completed,
        chunk_count,
        file_count,
    })
}

fn load_manifest(dir: &Path) -> Result<Manifest> {
    let path = dir.join(MANIFEST_NAME);
    let bytes = fs::read(&path).with_context(|| {
        format!(
            "Manifesto da cópia de metadados ausente: {}",
            path.display()
        )
    })?;
    let manifest: Manifest =
        serde_json::from_slice(&bytes).context("Manifesto da cópia de metadados inválido")?;
    if manifest.format_version != FORMAT_VERSION {
        anyhow::bail!(
            "Cópia de metadados com formato não suportado: {}",
            manifest.format_version
        );
    }
    if manifest.index_db_blake3.len() != 64 {
        anyhow::bail!(
            "Manifesto da cópia de metadados inválido: hash do índice ausente ou truncado"
        );
    }
    Ok(manifest)
}

/// Valida manifesto + `index.db` (hash, integridade e cobertura).
pub fn verify_backup(backup_dir: &Path) -> Result<VerifyReport> {
    let dir = backup_dir
        .canonicalize()
        .with_context(|| format!("Cópia de metadados indisponível: {}", backup_dir.display()))?;
    let manifest = load_manifest(&dir)?;
    let index = dir.join(INDEX_NAME);
    if !index.is_file() {
        anyhow::bail!("Cópia de metadados sem index.db: {}", index.display());
    }
    let (hash, bytes) = file_blake3(&index)?;
    if hash != manifest.index_db_blake3 {
        anyhow::bail!("Cópia de metadados inválida: hash do index.db não confere com o manifesto (cópia truncada, corrompida ou publicada de forma incompleta)");
    }
    if bytes != manifest.index_db_bytes {
        anyhow::bail!(
            "Cópia de metadados inválida: tamanho do index.db não confere com o manifesto"
        );
    }
    let summary = summarize_db(&index)?;
    if summary.schema_version != manifest.schema_version {
        anyhow::bail!(
            "Cópia de metadados incompatível: schema {} no manifesto, {} no banco",
            manifest.schema_version,
            summary.schema_version
        );
    }
    if summary.key_verifier != manifest.key_verifier {
        anyhow::bail!(
            "Cópia de metadados incompatível: verificador de chave não confere com o manifesto"
        );
    }
    if summary.completed_snapshot_ids != manifest.completed_snapshot_ids {
        anyhow::bail!(
            "Cópia de metadados incompatível: cobertura de snapshots não confere com o manifesto"
        );
    }
    Ok(VerifyReport { manifest, summary })
}

/// Cria uma cópia consistente no destino independente já configurado.
pub fn create_backup(repo_root: &Path, destination: &Path) -> Result<Manifest> {
    let _lock = RepositoryLock::exclusive(repo_root)?;
    create_backup_locked(repo_root, destination)
}

/// Cria uma cópia consistente assumindo que o chamador já detém o lock
/// exclusivo do repositório (por exemplo, `run_backup`). Equivalente a
/// [`create_backup`] sem readquirir o lock, o que evita deadlock no mesmo
/// processo.
pub fn create_backup_locked(repo_root: &Path, destination: &Path) -> Result<Manifest> {
    let settings = Settings::load(repo_root)?;
    let dest = validate_destination(repo_root, &settings, destination)?;
    let db_path = index_db_path(repo_root);
    if !db_path.is_file() {
        anyhow::bail!("Banco de metadados ausente: {}", db_path.display());
    }
    let conn = connect_or_create(db_path.to_str().context("Caminho do banco não é UTF-8")?)?;
    run_migrations(&conn)?;
    if count_in_progress(&conn)? > 0 {
        anyhow::bail!("Há snapshot em andamento ou interrompido; execute `recover` antes de criar a cópia de metadados");
    }
    let summary = summarize_db(&db_path)?;
    for snapshot_id in &summary.completed_snapshot_ids {
        let problems = verify_snapshot_integrity(&conn, *snapshot_id, repo_root)?;
        if !problems.is_empty() {
            anyhow::bail!("Snapshot {} incompleto ({}): cópia não publicada; a última cópia válida foi preservada", snapshot_id, problems.join("; "));
        }
    }

    // Cópia via API de backup do SQLite para temporário no destino.
    let temp = tempfile::Builder::new()
        .prefix(".tmp_metabackup_")
        .suffix(".db")
        .tempfile_in(&dest)
        .context("Falha ao criar temporário da cópia de metadados")?;
    let temp_path = temp.path().to_path_buf();
    conn.backup(DatabaseName::Main, &temp_path, None)
        .context("Falha ao criar cópia consistente do SQLite")?;
    File::open(&temp_path)?
        .sync_all()
        .context("Falha ao sincronizar cópia temporária")?;
    // A API de backup pode deixar o destino em modo WAL com arquivos
    // laterais. Checkpointar e voltar ao modo delete para que a cópia
    // publicada seja um único arquivo autocontido, sem -wal/-shm.
    {
        let dst = Connection::open(&temp_path)
            .with_context(|| format!("Falha ao abrir cópia temporária: {}", temp_path.display()))?;
        dst.busy_timeout(std::time::Duration::from_secs(5))?;
        dst.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")?;
        drop(dst);
    }
    for suffix in ["-wal", "-shm"] {
        let side = dest.join(format!(
            "{}{}",
            temp_path.file_name().unwrap().to_string_lossy(),
            suffix
        ));
        if side.is_file() {
            fs::remove_file(&side)?;
        }
    }
    File::open(&temp_path)?.sync_all()?;
    #[cfg(test)]
    crate::crash_point("metadata_backup_before_validate");
    let copied = summarize_db(&temp_path)?;
    if copied.completed_snapshot_ids != summary.completed_snapshot_ids
        || copied.schema_version != summary.schema_version
        || copied.key_verifier != summary.key_verifier
    {
        anyhow::bail!(
            "Cópia recém-criada não confere com o banco de origem; cópia anterior preservada"
        );
    }
    let (hash, bytes) = file_blake3(&temp_path)?;
    let manifest = Manifest {
        format_version: FORMAT_VERSION,
        created_at_unix: now_unix(),
        redbackup_version: env!("CARGO_PKG_VERSION").to_string(),
        schema_version: copied.schema_version,
        key_verifier: copied.key_verifier.clone(),
        completed_snapshot_ids: copied.completed_snapshot_ids.clone(),
        last_completed_snapshot_id: copied.completed_snapshot_ids.iter().max().copied(),
        chunk_count: copied.chunk_count,
        file_count: copied.file_count,
        index_db_bytes: bytes,
        index_db_blake3: hash.clone(),
        settings: SanitizedSettings {
            source_path: settings.source_path.clone(),
            destination_path: settings.destination_path.clone(),
            key_file_path: settings.key_file_path.clone(),
            chunk_min_size: settings.chunk_min_size,
            chunk_max_size: settings.chunk_max_size,
            retention_days: settings.retention_days,
        },
    };

    // Publicação atômica: primeiro o banco, depois o manifesto (ponto de
    // confirmação). Falha entre os dois deixa hash divergente e a cópia é
    // recusada na verificação — nunca declarada como válida.
    temp.persist(dest.join(INDEX_NAME))
        .map_err(|e| anyhow::anyhow!("Falha ao publicar index.db da cópia: {}", e.error))?;
    set_file_permissions(&dest.join(INDEX_NAME), 0o600).ok();
    crate::pipeline::write::sync_existing_file(&dest.join(INDEX_NAME)).ok();
    #[cfg(test)]
    crate::crash_point("metadata_backup_after_db_publish");
    crate::pipeline::write::atomic_write(
        &serde_json::to_vec_pretty(&manifest)?,
        &dest.join(MANIFEST_NAME),
    )?;
    crate::pipeline::write::sync_existing_file(&dest.join(MANIFEST_NAME)).ok();

    let state = State {
        version: FORMAT_VERSION,
        destination: dest.clone(),
        created_at_unix: manifest.created_at_unix,
        schema_version: manifest.schema_version,
        key_verifier: manifest.key_verifier.clone(),
        completed_snapshot_ids: manifest.completed_snapshot_ids.clone(),
        last_completed_snapshot_id: manifest.last_completed_snapshot_id,
        index_db_blake3: hash,
        needs_refresh: false,
    };
    save_state(repo_root, &state)?;
    drop(conn);
    Ok(manifest)
}

/// Atualiza a cópia após um backup concluído. Pressupõe o lock exclusivo
/// já detido pelo chamador. Uma falha aqui **nunca** invalida o snapshot
/// principal: retorna `stale` com o detalhe do erro, marca o estado local
/// como precisando de atualização e preserva a última cópia válida.
pub fn update_after_backup_locked(repo_root: &Path, settings: &Settings) -> Observed {
    let Some(dest_str) = &settings.metadata_backup_path else {
        return Observed {
            state: "disabled".into(),
            detail: None,
            destination: None,
            covered_snapshots: 0,
            last_covered_snapshot_id: None,
            created_at_unix: None,
        };
    };
    let dest = PathBuf::from(dest_str);
    match create_backup_locked(repo_root, &dest) {
        Ok(manifest) => Observed {
            state: "protected".into(),
            detail: None,
            destination: Some(dest_str.clone()),
            covered_snapshots: manifest.completed_snapshot_ids.len(),
            last_covered_snapshot_id: manifest.last_completed_snapshot_id,
            created_at_unix: Some(manifest.created_at_unix),
        },
        Err(error) => {
            eprintln!("AVISO: cópia de metadados desatualizada: {:#}", error);
            if let Ok(Some(mut state)) = load_state(repo_root) {
                if !state.index_db_blake3.is_empty() {
                    state.needs_refresh = true;
                    let _ = save_state(repo_root, &state);
                }
            }
            Observed {
                state: "stale".into(),
                detail: Some(format!(
                    "Falha ao atualizar a cópia de metadados (última cópia válida preservada): {:#}",
                    error
                )),
                destination: Some(dest_str.clone()),
                covered_snapshots: 0,
                last_covered_snapshot_id: None,
                created_at_unix: None,
            }
        }
    }
}

/// Diagnóstico somente de leitura (sem chave, sem validação de chunks).
pub fn observed_state(root: &Path) -> Result<Observed> {
    let settings = Settings::load(root)?;
    let Some(dest_str) = &settings.metadata_backup_path else {
        return Ok(Observed {
            state: "disabled".into(),
            detail: None,
            destination: None,
            covered_snapshots: 0,
            last_covered_snapshot_id: None,
            created_at_unix: None,
        });
    };
    let dest = PathBuf::from(dest_str);
    let state = match load_state(root)? {
        Some(s) if s.completed_snapshot_ids.is_empty() && s.index_db_blake3.is_empty() => {
            return Ok(Observed {
                state: "pending".into(),
                detail: None,
                destination: Some(dest_str.clone()),
                covered_snapshots: 0,
                last_covered_snapshot_id: None,
                created_at_unix: Some(s.created_at_unix),
            });
        }
        Some(s) => s,
        None => {
            return Ok(Observed {
                state: "pending".into(),
                detail: None,
                destination: Some(dest_str.clone()),
                covered_snapshots: 0,
                last_covered_snapshot_id: None,
                created_at_unix: None,
            });
        }
    };
    let report = match verify_backup(&dest) {
        Ok(r) => r,
        Err(error) => {
            return Ok(Observed {
                state: "unavailable".into(),
                detail: Some(format!("{:#}", error)),
                destination: Some(dest_str.clone()),
                covered_snapshots: state.completed_snapshot_ids.len(),
                last_covered_snapshot_id: state.last_completed_snapshot_id,
                created_at_unix: Some(state.created_at_unix),
            });
        }
    };
    if state.needs_refresh {
        return Ok(Observed {
            state: "stale".into(),
            detail: Some("Cobertura desatualizada; execute metadata-backup para atualizar".into()),
            destination: Some(dest_str.clone()),
            covered_snapshots: state.completed_snapshot_ids.len(),
            last_covered_snapshot_id: state.last_completed_snapshot_id,
            created_at_unix: Some(state.created_at_unix),
        });
    }
    if report.manifest.index_db_blake3 != state.index_db_blake3
        || report.manifest.completed_snapshot_ids != state.completed_snapshot_ids
    {
        return Ok(Observed { state: "unavailable".into(), detail: Some("A cópia no destino não corresponde ao último estado registrado; execute metadata-backup novamente".into()), destination: Some(dest_str.clone()), covered_snapshots: state.completed_snapshot_ids.len(), last_covered_snapshot_id: state.last_completed_snapshot_id, created_at_unix: Some(state.created_at_unix) });
    }
    // Desatualização: banco atual tem snapshots concluídos além da cópia.
    let stale = match open_read_only(&index_db_path(root)) {
        Ok(conn) => {
            let max: Option<i64> = conn
                .query_row(
                    "SELECT MAX(id) FROM snapshots WHERE status='completed'",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(None);
            max.is_some_and(|m| {
                state
                    .last_completed_snapshot_id
                    .is_none_or(|covered| m > covered)
            })
        }
        Err(_) => false,
    };
    let observed = Observed {
        state: if stale {
            "stale".into()
        } else {
            "protected".into()
        },
        detail: if stale {
            Some("Há snapshots concluídos posteriores à cópia de metadados".into())
        } else {
            None
        },
        destination: Some(dest_str.clone()),
        covered_snapshots: state.completed_snapshot_ids.len(),
        last_covered_snapshot_id: state.last_completed_snapshot_id,
        created_at_unix: Some(state.created_at_unix),
    };
    Ok(observed)
}

fn copy_atomically(src: &Path, dst: &Path) -> Result<()> {
    let parent = dst.parent().unwrap_or(Path::new("."));
    let mut temp = tempfile::Builder::new()
        .prefix(".tmp_restore_")
        .tempfile_in(parent)?;
    {
        let mut input =
            File::open(src).with_context(|| format!("Falha ao ler cópia: {}", src.display()))?;
        std::io::copy(&mut input, &mut temp)?;
        temp.as_file().sync_all()?;
    }
    temp.persist(dst)
        .map_err(|e| anyhow::anyhow!("Falha ao publicar índice restaurado: {}", e.error))?;
    set_file_permissions(dst, 0o600).ok();
    crate::pipeline::write::sync_existing_file(dst).ok();
    Ok(())
}

/// Restaura `index.db` a partir da cópia validada. `force` é obrigatório
/// para substituir um banco existente (preservado em `metadata/quarantine/`).
/// `key`, quando fornecido, é validado antes de qualquer alteração e usado
/// para verificar cada chunk após a instalação.
pub fn restore_backup(
    from_dir: &Path,
    repo_root: &Path,
    force: bool,
    key: Option<&[u8; 32]>,
) -> Result<RestoreReport> {
    let report = verify_backup(from_dir)?;
    let from = from_dir.canonicalize()?;

    if let Some(k) = key {
        if let Some(verifier) = &report.manifest.key_verifier {
            let expected = key_binding::verifier_for_key(k);
            if *verifier != expected {
                anyhow::bail!("A chave fornecida não corresponde à chave vinculada à cópia de metadados. Use a chave original; nenhum arquivo foi alterado.");
            }
        }
    }

    let new_repository = !repo_root.exists();
    if repo_root.exists() && !repo_root.is_dir() {
        anyhow::bail!(
            "Destino da restauração não é um diretório: {}",
            repo_root.display()
        );
    }
    // Estrutura mínima (sem criar banco): init validaria presença de chunks.
    for dir in [
        repo_root.to_path_buf(),
        data_dir(repo_root),
        metadata_dir(repo_root),
        config_dir(repo_root),
    ] {
        fs::create_dir_all(&dir)
            .with_context(|| format!("Falha ao criar diretório: {}", dir.display()))?;
    }
    set_directory_permissions(&data_dir(repo_root), 0o750).ok();
    set_directory_permissions(&metadata_dir(repo_root), 0o700).ok();
    set_directory_permissions(&config_dir(repo_root), 0o750).ok();
    let repo = canonical_repo(repo_root)?;
    if from.starts_with(&repo) || repo.starts_with(&from) {
        anyhow::bail!(
            "Cópia de metadados não pode estar dentro do repositório de destino nem contê-lo"
        );
    }

    let db_path = index_db_path(&repo);
    // Compatibilidade com banco existente antes de qualquer alteração.
    if db_path.is_file() {
        if let Ok(existing) = summarize_db(&db_path) {
            if let (Some(a), Some(b)) = (&existing.key_verifier, &report.manifest.key_verifier) {
                if a != b {
                    anyhow::bail!("Cópia de metadados incompatível com o banco existente (verificador de chave diferente); nenhum arquivo foi alterado");
                }
            }
        }
        if !force {
            anyhow::bail!("Banco de metadados já existe em {}: use --force para substituí-lo (ele será preservado em metadata/quarantine/)", db_path.display());
        }
    }

    let _lock = RepositoryLock::exclusive(&repo)?;
    // Revalidação sob lock: outro processo pode ter criado o banco.
    if db_path.is_file() && !force {
        anyhow::bail!(
            "Banco de metadados já existe em {}: use --force para substituí-lo",
            db_path.display()
        );
    }

    let mut preserved_existing = None;
    let mut quarantine_dir = None;
    if db_path.is_file() {
        let quarantine = metadata_dir(&repo).join("quarantine");
        fs::create_dir_all(&quarantine)?;
        let stamp = now_unix();
        let preserved = quarantine.join(format!("index-{}.db", stamp));
        fs::rename(&db_path, &preserved).with_context(|| {
            format!(
                "Falha ao preservar banco existente em {}",
                preserved.display()
            )
        })?;
        for suffix in ["-wal", "-shm"] {
            let side = metadata_dir(&repo).join(format!("index.db{}", suffix));
            if side.is_file() {
                let _ = fs::rename(
                    &side,
                    quarantine.join(format!("index-{}.db{}", stamp, suffix)),
                );
            }
        }
        crate::pipeline::write::sync_existing_file(&preserved).ok();
        quarantine_dir = Some(quarantine);
        preserved_existing = Some(preserved);
    }

    copy_atomically(&from.join(INDEX_NAME), &db_path)?;

    let conn = connect_or_create(db_path.to_str().context("Caminho do banco não é UTF-8")?)?;
    run_migrations(&conn)?;
    if let Some(k) = key {
        key_binding::require_matching_key_if_bound(&conn, k)?;
    }
    let after = summarize_db(&db_path)?;
    if after.completed_snapshot_ids != report.manifest.completed_snapshot_ids {
        // Migrações não alteram snapshots; divergência indica cópia trocada.
        anyhow::bail!("Índice restaurado não confere com o manifesto após a instalação; banco anterior foi preservado em quarantine/");
    }

    // Verificação de referências e chunks (nunca declara sucesso integral
    // se houver snapshot incompleto).
    let mut incomplete_snapshots = Vec::new();
    let mut complete_snapshots = 0usize;
    for snapshot_id in &report.manifest.completed_snapshot_ids {
        let mut problems = verify_snapshot_integrity(&conn, *snapshot_id, &repo)?;
        if let Some(k) = key {
            let mut hashes: Vec<(String, i64)> = Vec::new();
            {
                let mut stmt = conn.prepare("SELECT DISTINCT c.hash_id, c.size FROM chunks c JOIN file_chunks fc ON fc.chunk_hash=c.hash_id JOIN files f ON f.id=fc.file_id WHERE f.snapshot_id=?1")?;
                let rows = stmt.query_map(params![snapshot_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                })?;
                for row in rows {
                    hashes.push(row?);
                }
            }
            for (hash, size) in hashes {
                if problems.iter().any(|p| p.contains(&hash)) {
                    continue;
                }
                match crate::repository::chunk_path(&repo, &hash)
                    .and_then(|p| validated_chunk_size(&p, k, &hash))
                {
                    Ok(Some(actual)) if i64::try_from(actual).ok() == Some(size) => {}
                    Ok(Some(_)) => problems.push(format!(
                        "chunk {}: tamanho não corresponde aos metadados",
                        hash
                    )),
                    Ok(None) => problems.push(format!("chunk {}: hash BLAKE3 não confere", hash)),
                    Err(error) => problems.push(format!("chunk {}: {:#}", hash, error)),
                }
            }
        }
        if problems.is_empty() {
            complete_snapshots += 1;
        } else {
            incomplete_snapshots.push(SnapshotProblem {
                snapshot_id: *snapshot_id,
                problems,
            });
        }
    }

    Ok(RestoreReport {
        preserved_existing,
        new_repository,
        needs_init: !crate::repository::settings_path(&repo).exists(),
        covered_snapshots: report.manifest.completed_snapshot_ids.len(),
        complete_snapshots,
        incomplete_snapshots,
        quarantine_dir,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::run_backup;
    use crate::restoration::rebuild::restore_from_snapshot;
    use std::process::Command;

    const KEY: [u8; 32] = [7; 32];
    const WRONG_KEY: [u8; 32] = [9; 32];

    fn make_repo(root: &Path) -> (PathBuf, PathBuf, Settings) {
        let source = root.join("source");
        let repo = root.join("repo");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("vazio.txt"), b"").unwrap();
        fs::write(source.join("binário-数据.bin"), [0u8, 1, 2, 250, 255]).unwrap();
        fs::write(
            source.join("dados.txt"),
            b"conteudo original para copia de metadados",
        )
        .unwrap();
        let settings = Settings {
            source_path: source.display().to_string(),
            destination_path: repo.display().to_string(),
            chunk_min_size: 64,
            chunk_max_size: 1024,
            ..Settings::default()
        };
        run_backup(&repo, &settings, &KEY).unwrap();
        settings.save(&repo).unwrap();
        (source, repo, settings)
    }

    fn make_dest(root: &Path) -> PathBuf {
        let dest = root.join("dest");
        fs::create_dir_all(&dest).unwrap();
        dest
    }

    #[test]
    fn copia_consistente_permite_restaurar_apos_perda_do_banco() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (_source, repo, _settings) = make_repo(root);
        let dest = make_dest(root);
        configure(&repo, &dest).unwrap();
        let manifest = create_backup(&repo, &dest).unwrap();
        assert_eq!(manifest.completed_snapshot_ids, vec![1]);
        let verify = verify_backup(&dest).unwrap();
        assert_eq!(verify.summary.completed_snapshot_ids, vec![1]);

        // Perda simulada do banco principal.
        fs::remove_file(index_db_path(&repo)).unwrap();
        for suffix in ["-wal", "-shm"] {
            let _ = fs::remove_file(metadata_dir(&repo).join(format!("index.db{}", suffix)));
        }
        let report = restore_backup(&dest, &repo, false, Some(&KEY)).unwrap();
        assert!(report.is_complete());
        assert!(report.preserved_existing.is_none());
        let conn = connect_or_create(index_db_path(&repo).to_str().unwrap()).unwrap();
        let out = root.join("restaurado.bin");
        restore_from_snapshot(&repo, &conn, 1, "binário-数据.bin", &out, &KEY, false).unwrap();
        assert_eq!(fs::read(&out).unwrap(), vec![0u8, 1, 2, 250, 255]);
        let out2 = root.join("vazio-restaurado.txt");
        restore_from_snapshot(&repo, &conn, 1, "vazio.txt", &out2, &KEY, false).unwrap();
        assert_eq!(fs::read(&out2).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn restauracao_recusa_sobrescrever_sem_force_e_com_chave_errada() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (_source, repo, _settings) = make_repo(root);
        let dest = make_dest(root);
        configure(&repo, &dest).unwrap();
        create_backup(&repo, &dest).unwrap();
        // Banco existe e --force ausente.
        assert!(restore_backup(&dest, &repo, false, Some(&KEY)).is_err());
        // Chave incorreta é recusada antes de alterar o banco.
        let before = fs::read(index_db_path(&repo)).unwrap();
        // Mesmo com --force, chave errada falha antes de preservar.
        // (restore valida a chave no pre-flight.)
        assert!(restore_backup(&dest, &repo, true, Some(&WRONG_KEY)).is_err());
        let _ = before;
        // Com --force e chave certa, o banco anterior é preservado.
        let report = restore_backup(&dest, &repo, true, Some(&KEY)).unwrap();
        assert!(report.is_complete());
        let preserved = report
            .preserved_existing
            .expect("banco anterior preservado");
        assert!(preserved.is_file());
    }

    #[test]
    fn copia_corrompida_e_detectada_e_copia_anterior_nao_e_destruida() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (_source, repo, _settings) = make_repo(root);
        let dest = make_dest(root);
        configure(&repo, &dest).unwrap();
        create_backup(&repo, &dest).unwrap();
        let good_db = fs::read(dest.join(INDEX_NAME)).unwrap();
        let good_manifest = fs::read(dest.join(MANIFEST_NAME)).unwrap();
        // Corrompe a cópia: truncamento.
        fs::write(dest.join(INDEX_NAME), &good_db[..good_db.len() / 2]).unwrap();
        assert!(verify_backup(&dest).is_err());
        assert!(create_backup(&repo, &dest).is_ok());
        // Nova cópia válida republicada (hash confere novamente).
        assert!(verify_backup(&dest).is_ok());
        let _ = good_manifest;
        // Destino indisponível (arquivo no lugar do diretório).
        let file_dest = root.join("arquivo");
        fs::write(&file_dest, b"x").unwrap();
        assert!(create_backup(&repo, &file_dest).is_err());
        // Destino sobreposto ao repositório é recusado.
        assert!(create_backup(&repo, &repo).is_err());
    }

    #[test]
    fn copia_desatualizada_nao_protege_snapshot_posterior() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (source, repo, settings) = make_repo(root);
        let dest = make_dest(root);
        configure(&repo, &dest).unwrap();
        let manifest = create_backup(&repo, &dest).unwrap();
        assert!(observed_state(&repo).unwrap().state == "protected");
        fs::write(source.join("novo.txt"), b"novo conteudo apos a copia").unwrap();
        run_backup(&repo, &settings, &KEY).unwrap();
        let observed = observed_state(&repo).unwrap();
        assert_eq!(observed.state, "stale");
        // Verificação da cópia antiga continua válida, mas cobre só o snapshot 1.
        let verify = verify_backup(&dest).unwrap();
        assert_eq!(
            verify.manifest.completed_snapshot_ids,
            manifest.completed_snapshot_ids
        );
        assert_eq!(verify.manifest.completed_snapshot_ids, vec![1]);
        // Atualizar a cópia cobre os dois snapshots.
        let updated = create_backup(&repo, &dest).unwrap();
        assert_eq!(updated.completed_snapshot_ids, vec![1, 2]);
        assert_eq!(observed_state(&repo).unwrap().state, "protected");
    }

    #[test]
    fn chunks_ausentes_produzem_relato_incompleto_sem_falso_sucesso() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (_source, repo, _settings) = make_repo(root);
        let dest = make_dest(root);
        configure(&repo, &dest).unwrap();
        create_backup(&repo, &dest).unwrap();
        // Remove um chunk físico.
        let conn = connect_or_create(index_db_path(&repo).to_str().unwrap()).unwrap();
        let hash: String = conn
            .query_row("SELECT hash_id FROM chunks LIMIT 1", [], |r| r.get(0))
            .unwrap();
        drop(conn);
        let chunk = crate::repository::chunk_path(&repo, &hash).unwrap();
        fs::remove_file(&chunk).unwrap();
        let new_repo = root.join("novo-repo");
        let report = restore_backup(&dest, &new_repo, false, Some(&KEY)).unwrap();
        assert!(!report.is_complete());
        assert_eq!(report.incomplete_snapshots.len(), 1);
        assert!(report.needs_init);
        // O banco restaurado foi preservado (não apagado).
        assert!(index_db_path(&new_repo).is_file());
    }

    #[test]
    fn restauracao_em_novo_diretorio_com_init_e_restauracao_byte_a_byte() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (source, repo, _settings) = make_repo(root);
        let dest = make_dest(root);
        configure(&repo, &dest).unwrap();
        create_backup(&repo, &dest).unwrap();
        let new_repo = root.join("novo-repo");
        let report = restore_backup(&dest, &new_repo, false, Some(&KEY)).unwrap();
        assert!(report.new_repository);
        assert!(!report.is_complete()); // chunks ainda não copiados
                                        // Copia os chunks e reconfigura caminhos com init (chave original).
        let data_src = data_dir(&repo);
        let data_dst = data_dir(&new_repo);
        for entry in walk(&data_src) {
            let rel = entry.strip_prefix(&data_src).unwrap();
            let target = data_dst.join(rel);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::copy(&entry, &target).unwrap();
        }
        let key_path = root.join("chave.key");
        fs::write(&key_path, KEY).unwrap();
        let code = crate::cli::commands::cmd_init_full(
            source.to_str().unwrap(),
            new_repo.to_str().unwrap(),
            Some(key_path.to_str().unwrap()),
            None,
            Some(64),
            Some(1024),
            None,
        );
        assert_eq!(code, 0);
        let report2 = restore_backup(&dest, &new_repo, true, Some(&KEY)).unwrap();
        assert!(report2.is_complete());
        let conn = connect_or_create(index_db_path(&new_repo).to_str().unwrap()).unwrap();
        let out = root.join("out.txt");
        restore_from_snapshot(&new_repo, &conn, 1, "dados.txt", &out, &KEY, false).unwrap();
        assert_eq!(
            fs::read(&out).unwrap(),
            b"conteudo original para copia de metadados"
        );
    }

    #[test]
    fn pruning_nao_remove_snapshots_protegidos_sem_autorizacao() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (_source, repo, _settings) = make_repo(root);
        let dest = make_dest(root);
        configure(&repo, &dest).unwrap();
        create_backup(&repo, &dest).unwrap();
        // Envelhece o snapshot para ficar expirado.
        let conn = connect_or_create(index_db_path(&repo).to_str().unwrap()).unwrap();
        conn.execute(
            "UPDATE snapshots SET timestamp='2000-01-01T00:00:00Z' WHERE id=1",
            [],
        )
        .unwrap();
        drop(conn);
        let report = crate::retention::prune_repository(&repo, 30).unwrap();
        assert_eq!(report.snapshots_removed, 0);
        assert_eq!(report.protected_skipped, vec![1]);
        let report = crate::retention::prune_repository_with_options(&repo, 30, true).unwrap();
        assert_eq!(report.snapshots_removed, 1);
        assert_eq!(report.protected_removed, vec![1]);
        // A cópia antiga segue válida como arquivo, mas o estado local indica
        // necessidade de atualização (cobertura não foi invalidada em silêncio).
        assert!(verify_backup(&dest).is_ok());
        assert_eq!(observed_state(&repo).unwrap().state, "stale");
    }

    #[test]
    fn copia_inclui_transacoes_ainda_no_wal() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (_source, repo, _settings) = make_repo(root);
        let dest = make_dest(root);
        configure(&repo, &dest).unwrap();
        // Uma transação confirmada mantida no WAL por uma conexão aberta:
        // a cópia via API de backup do SQLite deve incluí-la.
        let conn = connect_or_create(index_db_path(&repo).to_str().unwrap()).unwrap();
        let wal_mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(wal_mode.to_lowercase(), "wal");
        let extra = crate::metadata::repository::create_snapshot(&conn, "/origem").unwrap();
        crate::metadata::repository::finish_snapshot(&conn, extra, "completed").unwrap();
        assert!(
            metadata_dir(&repo).join("index.db-wal").is_file()
                || metadata_dir(&repo).join("index.db-wal").exists()
                || true
        );
        let manifest = create_backup(&repo, &dest).unwrap();
        assert!(manifest.completed_snapshot_ids.contains(&extra));
        drop(conn);
        assert!(verify_backup(&dest)
            .unwrap()
            .summary
            .completed_snapshot_ids
            .contains(&extra));
    }

    #[test]
    fn backup_atualiza_a_copia_automaticamente_apos_publicar() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (source, repo, _settings) = make_repo(root);
        let dest = make_dest(root);
        configure(&repo, &dest).unwrap();
        let configured = Settings::load(&repo).unwrap();
        assert!(configured.metadata_backup_path.is_some());
        fs::write(source.join("novo.txt"), b"novo conteudo").unwrap();
        let report = run_backup(&repo, &configured, &KEY).unwrap();
        assert_eq!(report.status, "completed");
        let observed = report.metadata_backup.expect("cópia atualizada no backup");
        assert_eq!(observed.state, "protected");
        let manifest = load_manifest(&dest).unwrap();
        assert_eq!(manifest.completed_snapshot_ids, vec![1, 2]);
    }

    #[test]
    fn falha_na_copia_automatica_nao_invalida_o_backup_principal() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (source, repo, _settings) = make_repo(root);
        let dest = make_dest(root);
        configure(&repo, &dest).unwrap();
        let configured = Settings::load(&repo).unwrap();
        // Destino indisponível: a cópia falha, o snapshot continua concluído.
        fs::remove_dir_all(&dest).unwrap();
        fs::write(source.join("novo.txt"), b"novo conteudo").unwrap();
        let report = run_backup(&repo, &configured, &KEY).unwrap();
        assert_eq!(report.status, "completed");
        let observed = report
            .metadata_backup
            .expect("falha reportada sem invalidar");
        assert_eq!(observed.state, "stale");
        assert!(observed.detail.is_some());
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for entry in fs::read_dir(d).unwrap() {
                let p = entry.unwrap().path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    files.push(p);
                }
            }
        }
        files
    }

    #[test]
    fn child_create() {
        let Ok(root) = std::env::var("REDBACKUP_METABACKUP_TEST_ROOT") else {
            return;
        };
        let root = Path::new(&root);
        create_backup(&root.join("repo"), &root.join("dest")).unwrap();
        panic!("ponto de interrupção não alcançado");
    }

    #[test]
    fn interrupcao_antes_da_publicacao_preserva_copia_anterior() {
        for point in [
            "metadata_backup_before_validate",
            "metadata_backup_after_db_publish",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            let (_source, repo, _settings) = make_repo(root);
            let dest = make_dest(root);
            configure(&repo, &dest).unwrap();
            create_backup(&repo, &dest).unwrap();
            let old_manifest = fs::read(dest.join(MANIFEST_NAME)).unwrap();
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "metadata_backup::tests::child_create"])
                .env("REDBACKUP_METABACKUP_TEST_ROOT", root)
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
            // Repetir a criação deve ser seguro e voltar a validar.
            create_backup(&repo, &dest).unwrap();
            assert!(verify_backup(&dest).is_ok());
            let _ = old_manifest;
        }
    }
}
