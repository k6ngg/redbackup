//! Read-only repository summaries and crash-safe backup activity observations.
use crate::{
    backup::BackupReport,
    config::settings::Settings,
    repository::{index_db_path, metadata_dir},
};
use anyhow::Result;
use fs2::FileExt;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BackupProgress {
    pub pid: u32,
    pub started_at: u64,
    pub updated_at: u64,
    pub elapsed_seconds: f64,
    pub stage: String,
    pub current_file: Option<String>,
    pub current_file_bytes: u64,
    pub bytes_processed: u64,
    pub chunks_new: usize,
    pub chunks_reused: usize,
    pub discovered_files: usize,
    pub totals_known: bool,
    pub report: BackupReport,
    pub error: Option<String>,
}

pub(crate) struct ProgressWriter {
    pub value: BackupProgress,
    root: PathBuf,
    _lock: File,
    start: Instant,
    last: Instant,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl ProgressWriter {
    pub fn new(root: &Path) -> Result<Self> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options.open(metadata_dir(root).join("backup-activity.lock"))?;
        FileExt::try_lock_exclusive(&lock)?;
        let mut writer = Self {
            root: root.into(),
            _lock: lock,
            start: Instant::now(),
            last: Instant::now(),
            value: BackupProgress {
                pid: std::process::id(),
                started_at: now(),
                updated_at: now(),
                elapsed_seconds: 0.0,
                stage: "discovering".into(),
                current_file: None,
                current_file_bytes: 0,
                bytes_processed: 0,
                chunks_new: 0,
                chunks_reused: 0,
                discovered_files: 0,
                totals_known: false,
                report: BackupReport::default(),
                error: None,
            },
        };
        writer.publish(true);
        Ok(writer)
    }
    pub fn publish(&mut self, force: bool) {
        if !force && self.last.elapsed() < Duration::from_millis(250) {
            return;
        }
        self.value.updated_at = now();
        self.value.elapsed_seconds = self.start.elapsed().as_secs_f64();
        // Telemetry must never change snapshot success or loosen metadata permissions.
        let result = (|| -> Result<()> {
            let mut tmp = tempfile::Builder::new()
                .prefix(".tmp_progress_")
                .tempfile_in(metadata_dir(&self.root))?;
            tmp.write_all(&serde_json::to_vec(&self.value)?)?;
            tmp.persist(metadata_dir(&self.root).join("backup-progress.json"))?;
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("AVISO: progresso indisponível: {:#}", error);
        }
        self.last = Instant::now();
    }
    pub fn finish(&mut self, result: &Result<BackupReport>) {
        self.value.current_file = None;
        match result {
            Ok(report) => {
                self.value.report = report.clone();
                self.value.stage = report.status.clone();
            }
            Err(error) => {
                self.value.stage = "failed".into();
                self.value.error = Some(format!("{:#}", error));
            }
        }
        self.publish(true);
    }
}

/// A held OS lock, rather than a PID or heartbeat age, proves activity.
pub fn backup_activity(root: &Path) -> Result<(bool, Option<BackupProgress>)> {
    let path = metadata_dir(root).join("backup-activity.lock");
    let lock = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => Some(file),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let active = if let Some(file) = &lock {
        match FileExt::try_lock_shared(file) {
            Ok(()) => false,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => true,
            Err(e) => return Err(e.into()),
        }
    } else {
        false
    };
    let mut progress: Option<BackupProgress> =
        match std::fs::read(metadata_dir(root).join("backup-progress.json")) {
            Ok(raw) => Some(serde_json::from_slice(&raw)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
    if let Some(p) = &mut progress {
        // Compatibility with the first, unversioned report-only progress file.
        if p.stage.is_empty() {
            p.stage = if p.report.status.is_empty() {
                "unknown".into()
            } else {
                p.report.status.clone()
            };
        }
        if active && p.started_at != 0 {
            p.elapsed_seconds = now().saturating_sub(p.started_at) as f64;
        }
    }
    Ok((active, progress))
}

/// Consistent SQLite summary; no migration, key access or chunk validation.
pub fn repository_status(root: &Path) -> Result<serde_json::Value> {
    let settings = Settings::load(root)?;
    let conn = Connection::open_with_flags(index_db_path(root), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.busy_timeout(Duration::from_secs(1))?;
    let tx = conn.unchecked_transaction()?;
    let mut counts = serde_json::Map::new();
    for state in ["completed", "failed", "in-progress"] {
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM snapshots WHERE status=?1",
            [state],
            |r| r.get(0),
        )?;
        counts.insert(state.into(), count.into());
    }
    let last: Option<(i64, String)> = tx
        .query_row(
            "SELECT id,timestamp FROM snapshots WHERE status='completed' ORDER BY id DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (chunks, corrupted): (i64, i64) = tx.query_row(
        "SELECT COUNT(*),COALESCE(SUM(corrupted_at IS NOT NULL),0) FROM chunks",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    tx.commit()?;
    let (active, progress) = backup_activity(root)?;
    let interrupted = !active
        && progress
            .as_ref()
            .is_some_and(|p| !matches!(p.stage.as_str(), "completed" | "unchanged" | "failed"));
    let redundancy = serde_json::to_value(crate::redundancy::observed_state(root)?)?;
    Ok(
        serde_json::json!({"repository":root,"source":settings.source_path,"snapshots":counts,"last_completed":last.map(|(id,timestamp)| serde_json::json!({"id":id,"timestamp":timestamp})),"chunks":chunks,"corruption_recorded":corrupted,"backup_active":active,"backup_interrupted":interrupted,"backup":progress,"redundancy":redundancy}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn activity_lock_distinguishes_running_interrupted_and_finished() {
        let tmp = tempfile::tempdir().unwrap();
        let root = crate::repository::initialize_repository(tmp.path().to_str().unwrap()).unwrap();
        let mut writer = ProgressWriter::new(&root).unwrap();
        writer.value.current_file = Some("large.bin".into());
        writer.value.bytes_processed = 1024;
        writer.publish(true);
        let (active, progress) = backup_activity(&root).unwrap();
        assert!(active);
        assert_eq!(progress.unwrap().bytes_processed, 1024);
        drop(writer); // Models process exit: OS lock released, persisted stage unfinished.
        assert!(!backup_activity(&root).unwrap().0);
        let interrupted = repository_status_with_settings(&root);
        assert_eq!(interrupted["backup_interrupted"], true);
        let mut writer = ProgressWriter::new(&root).unwrap();
        writer.finish(&Ok(BackupReport {
            status: "completed".into(),
            ..BackupReport::default()
        }));
        drop(writer);
        let finished = repository_status_with_settings(&root);
        assert_eq!(finished["backup_interrupted"], false);
        assert_eq!(finished["backup"]["stage"], "completed");
    }
    fn repository_status_with_settings(root: &Path) -> serde_json::Value {
        Settings {
            source_path: "/example".into(),
            destination_path: root.display().to_string(),
            ..Settings::default()
        }
        .save(root)
        .unwrap();
        repository_status(root).unwrap()
    }
    #[test]
    fn telemetry_failure_does_not_fail_backup_result() {
        let tmp = tempfile::tempdir().unwrap();
        let root = crate::repository::initialize_repository(tmp.path().to_str().unwrap()).unwrap();
        let mut writer = ProgressWriter::new(&root).unwrap();
        std::fs::remove_file(metadata_dir(&root).join("backup-progress.json")).unwrap();
        std::fs::create_dir(metadata_dir(&root).join("backup-progress.json")).unwrap();
        writer.finish(&Ok(BackupReport {
            status: "completed".into(),
            ..BackupReport::default()
        }));
        assert_eq!(writer.value.stage, "completed");
    }
}
