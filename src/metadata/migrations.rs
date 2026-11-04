//! Sistema de migrações versionadas para o banco SQLite.
//!
//! Usa uma tabela `schema_version` para rastrear migrações aplicadas. Cada
//! migração é incremental e idempotente para bancos já existentes.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

pub fn run_migrations(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY,
            applied_at TEXT DEFAULT CURRENT_TIMESTAMP
        );",
    )
    .context("Falha ao criar tabela de migrações")?;

    apply_v1(conn)?;
    apply_v2(conn)?;
    apply_v3(conn)?;
    Ok(())
}

fn already_applied(conn: &Connection, version: i32) -> Result<bool> {
    let applied: Option<i32> = conn
        .query_row(
            "SELECT version FROM schema_version WHERE version = ?1",
            params![version],
            |row| row.get(0),
        )
        .optional()?;
    Ok(applied.is_some())
}

fn record(conn: &Connection, version: i32) -> Result<()> {
    conn.execute(
        "INSERT INTO schema_version (version) VALUES (?1)",
        params![version],
    )
    .with_context(|| format!("Falha ao registrar versão {}", version))?;
    Ok(())
}

fn apply_v1(conn: &Connection) -> Result<()> {
    if already_applied(conn, 1)? {
        return Ok(());
    }
    conn.execute_batch(super::schema::INITIAL_SCHEMA)
        .context("Falha ao aplicar schema v1")?;
    record(conn, 1)
}

/// v2: rastreia corrupção detectada pelo scrubbing, sem apagar dados
/// automaticamente.
fn apply_v2(conn: &Connection) -> Result<()> {
    if already_applied(conn, 2)? {
        return Ok(());
    }

    let mut columns = Vec::new();
    {
        let mut stmt = conn.prepare("PRAGMA table_info(chunks)")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        for row in rows {
            columns.push(row?);
        }
    }
    if !columns.iter().any(|name| name == "corrupted_at") {
        conn.execute("ALTER TABLE chunks ADD COLUMN corrupted_at TEXT", [])
            .context("Falha ao adicionar coluna corrupted_at")?;
    }
    if !columns.iter().any(|name| name == "corruption_reason") {
        conn.execute("ALTER TABLE chunks ADD COLUMN corruption_reason TEXT", [])
            .context("Falha ao adicionar coluna corruption_reason")?;
    }

    record(conn, 2)
}

/// v3: vincula a chave ao repositório por meio de um verificador derivado com
/// BLAKE3 keyed. A chave em si nunca é persistida.
fn apply_v3(conn: &Connection) -> Result<()> {
    if already_applied(conn, 3)? {
        return Ok(());
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS repository_key (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            verifier TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        );",
    )
    .context("Falha ao criar tabela repository_key")?;
    record(conn, 3)
}
