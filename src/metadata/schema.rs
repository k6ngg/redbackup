//! Schema SQLite para metadados do redbackup.
//!
//! Tabelas:
//! - `snapshots`
//! - `files`
//! - `chunks`
//! - `file_chunks`
//!
//! Requisitos: `PRAGMA foreign_keys = ON`; índices adequados.

pub const INITIAL_SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('in-progress', 'completed', 'failed')),
    source_path TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS files (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    snapshot_id INTEGER NOT NULL REFERENCES snapshots(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    permissions INTEGER,
    modification_time INTEGER,
    size INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_files_snapshot ON files(snapshot_id);
CREATE INDEX IF NOT EXISTS idx_files_path ON files(path);

CREATE TABLE IF NOT EXISTS chunks (
    hash_id TEXT PRIMARY KEY,
    size INTEGER NOT NULL,
    compressed_size INTEGER,
    reference_count INTEGER DEFAULT 0 NOT NULL CHECK(reference_count >= 0)
);

CREATE INDEX IF NOT EXISTS idx_chunks_ref ON chunks(reference_count);

CREATE TABLE IF NOT EXISTS file_chunks (
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    chunk_hash TEXT NOT NULL REFERENCES chunks(hash_id) ON DELETE RESTRICT,
    order_index INTEGER NOT NULL,
    PRIMARY KEY (file_id, order_index)
);

CREATE INDEX IF NOT EXISTS idx_file_chunks_hash ON file_chunks(chunk_hash);
"#;
