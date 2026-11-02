//! Sistema de migrações versionadas para o banco SQLite.
//!
//! Usa uma tabela `schema_version` para rastrear migrações aplicadas.

use anyhow::{Context, Result};
use rusqlite::OptionalExtension;
use rusqlite::{params, Connection};

pub fn run_migrations(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY,
            applied_at TEXT DEFAULT CURRENT_TIMESTAMP
        );",
    )
    .context("Falha ao criar tabela de migrações")?;

    // Verifica se a migração inicial (v1) já foi aplicada
    let version: Option<i32> = conn
        .query_row(
            "SELECT version FROM schema_version WHERE version = 1",
            params![],
            |row| row.get(0),
        )
        .optional()?;

    if version.is_none() {
        // Aplica schema inicial (definido em schema.rs)
        // Aqui chamamos a aplicação do schema
        conn.execute_batch(super::schema::INITIAL_SCHEMA)
            .context("Falha ao aplicar schema v1")?;

        conn.execute(
            "INSERT INTO schema_version (version) VALUES (?1)",
            params![1],
        )
        .context("Falha ao registrar versão 1")?;
    }

    Ok(())
}
