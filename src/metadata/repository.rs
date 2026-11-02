//! Camada de repositório (CRUD) para metadados SQLite.
//!
//! Regras:
//! - Consultas parametrizadas (nunca montar SQL com concatenação).
//! - Transações ACID para operações relacionadas.
//! - Atualização segura de `reference_count` (evitar contagens negativas).

use anyhow::{Result, Context};
use rusqlite::{Connection, params, OptionalExtension};

pub fn connect_or_create(db_path: &str) -> Result<Connection> {
    let conn = Connection::open(db_path)
        .with_context(|| format!("Falha ao abrir/crear SQLite: {}", db_path))?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .context("Falha ao ativar foreign keys")?;
    Ok(conn)
}

/// Insere ou atualiza um chunk no banco, garantindo `reference_count >= 0`.
pub fn upsert_chunk(
    conn: &Connection,
    hash_id: &str,
    size: u64,
    compressed_size: Option<u64>,
) -> Result<()> {
    let mut stmt = conn.prepare(
        "INSERT INTO chunks (hash_id, size, compressed_size, reference_count)
         VALUES (?1, ?2, ?3, 0)
         ON CONFLICT(hash_id) DO NOTHING"
    )?;
    stmt.execute(params![hash_id, size as i64, compressed_size.as_ref().map(|v| *v as i64)])?;
    drop(stmt);

    // Incrementa `reference_count`
    conn.execute(
        "UPDATE chunks SET reference_count = reference_count + 1 WHERE hash_id = ?1",
        params![hash_id]
    ).with_context(|| format!("Falha ao incrementar reference_count para {}", hash_id))?;

    Ok(())
}

/// Decrementa `reference_count` de forma segura (não permite negativo).
pub fn decrement_reference_count(
    conn: &Connection,
    hash_id: &str,
) -> Result<bool> {
    let mut stmt = conn.prepare(
        "SELECT reference_count FROM chunks WHERE hash_id = ?1"
    )?;
    let count: Option<i64> = stmt.query_row(params![hash_id], |row| row.get(0)).optional()?;
    drop(stmt);

    if let Some(c) = count {
        if c <= 0 {
            anyhow::bail!("Tentativa de decrementar reference_count negativo para {}", hash_id);
        }
        conn.execute(
            "UPDATE chunks SET reference_count = reference_count - 1 WHERE hash_id = ?1 AND reference_count > 0",
            params![hash_id]
        )?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Informações de um chunk armazenado.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkInfo {
    pub hash_id: String,
    pub size: i64,
    pub compressed_size: Option<i64>,
    pub reference_count: i64,
}

/// Consulta um chunk pelo hash.
pub fn get_chunk(
    conn: &Connection,
    hash_id: &str,
) -> Result<Option<ChunkInfo>> {
    let mut stmt = conn.prepare(
        "SELECT hash_id, size, compressed_size, reference_count FROM chunks WHERE hash_id = ?1"
    )?;
    let result = stmt.query_row(params![hash_id], |row| {
        Ok(ChunkInfo {
            hash_id: row.get(0)?,
            size: row.get(1)?,
            compressed_size: row.get(2)?,
            reference_count: row.get(3)?,
        })
    }).optional()?;
    Ok(result)
}
