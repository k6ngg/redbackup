//! Camada de repositório (CRUD) para metadados SQLite.
//!
//! Regras:
//! - Consultas parametrizadas (nunca montar SQL com concatenação).
//! - Transações ACID para operações relacionadas.
//! - Atualização segura de `reference_count` (evitar contagens negativas).

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

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
         ON CONFLICT(hash_id) DO NOTHING",
    )?;
    stmt.execute(params![
        hash_id,
        size as i64,
        compressed_size.as_ref().map(|v| *v as i64)
    ])?;
    drop(stmt);

    // Incrementa `reference_count`
    conn.execute(
        "UPDATE chunks SET reference_count = reference_count + 1 WHERE hash_id = ?1",
        params![hash_id],
    )
    .with_context(|| format!("Falha ao incrementar reference_count para {}", hash_id))?;

    Ok(())
}

/// Decrementa `reference_count` de forma segura (não permite negativo).
pub fn decrement_reference_count(conn: &Connection, hash_id: &str) -> Result<bool> {
    let mut stmt = conn.prepare("SELECT reference_count FROM chunks WHERE hash_id = ?1")?;
    let count: Option<i64> = stmt
        .query_row(params![hash_id], |row| row.get(0))
        .optional()?;
    drop(stmt);

    if let Some(c) = count {
        if c <= 0 {
            anyhow::bail!(
                "Tentativa de decrementar reference_count negativo para {}",
                hash_id
            );
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
pub fn get_chunk(conn: &Connection, hash_id: &str) -> Result<Option<ChunkInfo>> {
    let mut stmt = conn.prepare(
        "SELECT hash_id, size, compressed_size, reference_count FROM chunks WHERE hash_id = ?1",
    )?;
    let result = stmt
        .query_row(params![hash_id], |row| {
            Ok(ChunkInfo {
                hash_id: row.get(0)?,
                size: row.get(1)?,
                compressed_size: row.get(2)?,
                reference_count: row.get(3)?,
            })
        })
        .optional()?;
    Ok(result)
}

/// Recupera a lista ordenada de chunks para um arquivo específico dentro de um snapshot.
pub fn get_file_chunks(
    conn: &Connection,
    snapshot_id: i64,
    file_path: &str,
) -> Result<Vec<(String, i32)>> {
    let mut stmt = conn.prepare(
        "SELECT c.hash_id, fc.order_index
         FROM file_chunks fc
         JOIN files f ON fc.file_id = f.id
         JOIN chunks c ON fc.chunk_hash = c.hash_id
         WHERE f.snapshot_id = ?1 AND f.path = ?2
         ORDER BY fc.order_index ASC",
    )?;
    let rows = stmt.query_map(params![snapshot_id, file_path], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i32>(1)?))
    })?;
    let mut chunks = Vec::new();
    for row in rows {
        chunks.push(row?);
    }
    Ok(chunks)
}
