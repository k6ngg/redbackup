//! Camada de repositório (CRUD) para metadados SQLite.
//!
//! Regras:
//! - Consultas parametrizadas (nunca montar SQL com concatenação).
//! - Transações ACID para operações relacionadas.
//! - `reference_count` reflete o número de linhas em `file_chunks`.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

pub fn connect_or_create(db_path: &str) -> Result<Connection> {
    let conn = Connection::open(db_path)
        .with_context(|| format!("Falha ao abrir/criar SQLite: {}", db_path))?;
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;
         PRAGMA synchronous = FULL;
         PRAGMA journal_mode = WAL;",
    )
    .context("Falha ao configurar SQLite")?;
    Ok(conn)
}

/// Informações de um chunk armazenado.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkInfo {
    pub hash_id: String,
    pub size: i64,
    pub compressed_size: Option<i64>,
    pub reference_count: i64,
}

/// Informações de um snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotInfo {
    pub id: i64,
    pub timestamp: String,
    pub status: String,
    pub source_path: String,
    pub file_count: i64,
    pub total_bytes: i64,
}

/// Informações de um arquivo registrado em um snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct FileInfo {
    pub id: i64,
    pub snapshot_id: i64,
    pub path: String,
    pub permissions: Option<i64>,
    pub modification_time: Option<i64>,
    pub size: i64,
}

/// Insere ou atualiza um chunk e incrementa `reference_count`.
///
/// Cada chamada representa uma referência de `file_chunks`. O chamador deve
/// garantir que a linha correspondente em `file_chunks` será criada na mesma
/// transação.
pub fn upsert_chunk(
    conn: &Connection,
    hash_id: &str,
    size: u64,
    compressed_size: Option<u64>,
) -> Result<()> {
    if let Some(existing) = get_chunk(conn, hash_id)? {
        if existing.size >= 0 && existing.size as u64 != size {
            anyhow::bail!(
                "Inconsistência de tamanho para o chunk {}: banco={} novo={}",
                hash_id,
                existing.size,
                size
            );
        }
    }

    conn.execute(
        "INSERT INTO chunks (hash_id, size, compressed_size, reference_count)
         VALUES (?1, ?2, ?3, 0)
         ON CONFLICT(hash_id) DO NOTHING",
        params![
            hash_id,
            size as i64,
            compressed_size.map(|value| value as i64)
        ],
    )
    .with_context(|| format!("Falha ao inserir chunk {}", hash_id))?;

    // Preenche o tamanho comprimido quando ele passou a ser conhecido.
    conn.execute(
        "UPDATE chunks
         SET compressed_size = COALESCE(compressed_size, ?2)
         WHERE hash_id = ?1",
        params![hash_id, compressed_size.map(|value| value as i64)],
    )?;

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

/// Registra que o scrubbing detectou corrupção em um chunk.
///
/// O registro é apenas diagnóstico: os dados não são apagados nem alterados.
pub fn mark_chunk_corrupted(conn: &Connection, hash_id: &str, reason: &str) -> Result<()> {
    let changed = conn
        .execute(
            "UPDATE chunks
             SET corrupted_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now'),
                 corruption_reason = ?2
             WHERE hash_id = ?1",
            params![hash_id, reason],
        )
        .with_context(|| format!("Falha ao registrar corrupção do chunk {}", hash_id))?;
    if changed == 0 {
        anyhow::bail!(
            "Chunk {} não existe no banco; corrupção não pôde ser registrada",
            hash_id
        );
    }
    Ok(())
}

/// Limpa o diagnóstico de corrupção depois que o chunk foi validado.
pub fn clear_chunk_corruption(conn: &Connection, hash_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE chunks
         SET corrupted_at = NULL, corruption_reason = NULL
         WHERE hash_id = ?1",
        params![hash_id],
    )
    .with_context(|| format!("Falha ao limpar corrupção do chunk {}", hash_id))?;
    Ok(())
}

/// Lista chunks marcados com corrupção, com o motivo registrado.
pub fn list_corrupted_chunks(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT hash_id, COALESCE(corruption_reason, 'motivo não registrado')
         FROM chunks
         WHERE corrupted_at IS NOT NULL
         ORDER BY corrupted_at DESC, hash_id ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut chunks = Vec::new();
    for row in rows {
        chunks.push(row?);
    }
    Ok(chunks)
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

/// Cria um snapshot em andamento e retorna seu id.
pub fn create_snapshot(conn: &Connection, source_path: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO snapshots (timestamp, status, source_path)
         VALUES (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), 'in-progress', ?1)",
        params![source_path],
    )
    .context("Falha ao criar snapshot")?;
    Ok(conn.last_insert_rowid())
}

/// Finaliza um snapshot com o status indicado.
pub fn finish_snapshot(conn: &Connection, snapshot_id: i64, status: &str) -> Result<()> {
    if !matches!(status, "completed" | "failed") {
        anyhow::bail!("Status de snapshot inválido: {}", status);
    }
    conn.execute(
        "UPDATE snapshots SET status = ?1 WHERE id = ?2",
        params![status, snapshot_id],
    )
    .with_context(|| format!("Falha ao finalizar snapshot {}", snapshot_id))?;
    Ok(())
}

/// Retorna o snapshot concluído mais recente, se houver.
pub fn previous_completed_snapshot(conn: &Connection) -> Result<Option<i64>> {
    conn.query_row(
        "SELECT id FROM snapshots WHERE status = 'completed' ORDER BY id DESC LIMIT 1",
        [],
        |row| row.get(0),
    )
    .optional()
    .context("Falha ao consultar o último snapshot concluído")
}

/// Compara o conteúdo registrado em dois snapshots.
///
/// O critério é o conjunto de caminhos, tamanhos, permissões e a sequência de
/// hashes de chunks. `modification_time` não entra na comparação: uma alteração
/// apenas de mtime não representa novo conteúdo e não deve gerar um snapshot
/// redundante. Permissões continuam sendo comparadas porque afetam a restauração.
pub fn snapshots_have_same_content(
    conn: &Connection,
    left_snapshot: i64,
    right_snapshot: i64,
) -> Result<bool> {
    if !same_file_rows(conn, left_snapshot, right_snapshot)? {
        return Ok(false);
    }
    same_chunk_rows(conn, left_snapshot, right_snapshot)
}

fn same_file_rows(conn: &Connection, left_snapshot: i64, right_snapshot: i64) -> Result<bool> {
    let mut left_stmt = conn.prepare(
        "SELECT path, size, permissions FROM files WHERE snapshot_id = ?1 ORDER BY path",
    )?;
    let mut right_stmt = conn.prepare(
        "SELECT path, size, permissions FROM files WHERE snapshot_id = ?1 ORDER BY path",
    )?;
    let mut left_rows = left_stmt.query(params![left_snapshot])?;
    let mut right_rows = right_stmt.query(params![right_snapshot])?;
    loop {
        match (left_rows.next()?, right_rows.next()?) {
            (None, None) => return Ok(true),
            (Some(left), Some(right)) => {
                let left_path: String = left.get(0)?;
                let right_path: String = right.get(0)?;
                let left_size: i64 = left.get(1)?;
                let right_size: i64 = right.get(1)?;
                let left_permissions: Option<i64> = left.get(2)?;
                let right_permissions: Option<i64> = right.get(2)?;
                if left_path != right_path
                    || left_size != right_size
                    || left_permissions != right_permissions
                {
                    return Ok(false);
                }
            }
            _ => return Ok(false),
        }
    }
}

fn same_chunk_rows(conn: &Connection, left_snapshot: i64, right_snapshot: i64) -> Result<bool> {
    let mut left_stmt = conn.prepare(
        "SELECT f.path, fc.order_index, fc.chunk_hash
         FROM file_chunks fc
         JOIN files f ON f.id = fc.file_id
         WHERE f.snapshot_id = ?1
         ORDER BY f.path, fc.order_index",
    )?;
    let mut right_stmt = conn.prepare(
        "SELECT f.path, fc.order_index, fc.chunk_hash
         FROM file_chunks fc
         JOIN files f ON f.id = fc.file_id
         WHERE f.snapshot_id = ?1
         ORDER BY f.path, fc.order_index",
    )?;
    let mut left_rows = left_stmt.query(params![left_snapshot])?;
    let mut right_rows = right_stmt.query(params![right_snapshot])?;
    loop {
        match (left_rows.next()?, right_rows.next()?) {
            (None, None) => return Ok(true),
            (Some(left), Some(right)) => {
                let left_path: String = left.get(0)?;
                let right_path: String = right.get(0)?;
                let left_order: i64 = left.get(1)?;
                let right_order: i64 = right.get(1)?;
                let left_hash: String = left.get(2)?;
                let right_hash: String = right.get(2)?;
                if left_path != right_path || left_order != right_order || left_hash != right_hash {
                    return Ok(false);
                }
            }
            _ => return Ok(false),
        }
    }
}

/// Descarta um snapshot em andamento e desfaz suas referências.
///
/// Usado quando o conteúdo é idêntico ao do snapshot concluído anterior. Os
/// chunks físicos não são removidos: qualquer chunk referenciado continua em uso
/// pelo snapshot anterior. A operação é transacional para nunca deixar
/// `reference_count` inconsistente com `file_chunks`.
pub fn discard_snapshot(conn: &mut Connection, snapshot_id: i64) -> Result<()> {
    let tx = conn
        .transaction()
        .context("Falha ao abrir transação para descartar snapshot")?;
    tx.execute(
        "UPDATE chunks
         SET reference_count = reference_count - (
             SELECT COUNT(*)
             FROM file_chunks fc
             JOIN files f ON f.id = fc.file_id
             WHERE f.snapshot_id = ?1 AND fc.chunk_hash = chunks.hash_id
         )
         WHERE hash_id IN (
             SELECT fc.chunk_hash
             FROM file_chunks fc
             JOIN files f ON f.id = fc.file_id
             WHERE f.snapshot_id = ?1
         )",
        params![snapshot_id],
    )
    .context("Falha ao desfazer referências do snapshot descartado")?;
    let removed = tx
        .execute(
            "DELETE FROM snapshots WHERE id = ?1 AND status = 'in-progress'",
            params![snapshot_id],
        )
        .context("Falha ao remover snapshot descartado")?;
    if removed != 1 {
        anyhow::bail!(
            "Snapshot {} não está em andamento e não pode ser descartado",
            snapshot_id
        );
    }
    tx.commit()
        .context("Falha ao confirmar descarte do snapshot")
}

/// Marca snapshots deixados em andamento por uma execução anterior como falhos.
pub fn fail_stale_in_progress_snapshots(conn: &Connection) -> Result<usize> {
    let changed = conn
        .execute(
            "UPDATE snapshots SET status = 'failed' WHERE status = 'in-progress'",
            [],
        )
        .context("Falha ao marcar snapshots interrompidos como falhos")?;
    Ok(changed)
}

/// Registra um arquivo pertencente a um snapshot.
pub fn insert_file(
    conn: &Connection,
    snapshot_id: i64,
    path: &str,
    permissions: Option<i64>,
    modification_time: Option<i64>,
    size: i64,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO files (snapshot_id, path, permissions, modification_time, size)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![snapshot_id, path, permissions, modification_time, size],
    )
    .with_context(|| format!("Falha ao registrar arquivo {}", path))?;
    Ok(conn.last_insert_rowid())
}

/// Registra a associação ordenada entre arquivo e chunk.
pub fn insert_file_chunk(
    conn: &Connection,
    file_id: i64,
    chunk_hash: &str,
    order_index: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO file_chunks (file_id, chunk_hash, order_index)
         VALUES (?1, ?2, ?3)",
        params![file_id, chunk_hash, order_index],
    )
    .with_context(|| {
        format!(
            "Falha ao registrar chunk {} do arquivo {}",
            chunk_hash, file_id
        )
    })?;
    Ok(())
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

/// Consulta os metadados de um arquivo dentro de um snapshot.
pub fn get_file_info(
    conn: &Connection,
    snapshot_id: i64,
    file_path: &str,
) -> Result<Option<FileInfo>> {
    let mut stmt = conn.prepare(
        "SELECT id, snapshot_id, path, permissions, modification_time, size
         FROM files
         WHERE snapshot_id = ?1 AND path = ?2
         ORDER BY id ASC
         LIMIT 1",
    )?;
    let result = stmt
        .query_row(params![snapshot_id, file_path], |row| {
            Ok(FileInfo {
                id: row.get(0)?,
                snapshot_id: row.get(1)?,
                path: row.get(2)?,
                permissions: row.get(3)?,
                modification_time: row.get(4)?,
                size: row.get(5)?,
            })
        })
        .optional()?;
    Ok(result)
}

/// Lista snapshots com contagem de arquivos e tamanho total.
pub fn list_snapshots(conn: &Connection) -> Result<Vec<SnapshotInfo>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.timestamp, s.status, s.source_path,
                COUNT(f.id) AS file_count,
                COALESCE(SUM(f.size), 0) AS total_bytes
         FROM snapshots s
         LEFT JOIN files f ON f.snapshot_id = s.id
         GROUP BY s.id, s.timestamp, s.status, s.source_path
         ORDER BY s.id DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(SnapshotInfo {
            id: row.get(0)?,
            timestamp: row.get(1)?,
            status: row.get(2)?,
            source_path: row.get(3)?,
            file_count: row.get(4)?,
            total_bytes: row.get(5)?,
        })
    })?;
    let mut snapshots = Vec::new();
    for row in rows {
        snapshots.push(row?);
    }
    Ok(snapshots)
}

/// Consulta um snapshot pelo id.
pub fn get_snapshot(conn: &Connection, snapshot_id: i64) -> Result<Option<SnapshotInfo>> {
    Ok(list_snapshots(conn)?
        .into_iter()
        .find(|snapshot| snapshot.id == snapshot_id))
}

/// Lista os arquivos de um snapshot.
pub fn list_files(conn: &Connection, snapshot_id: i64) -> Result<Vec<FileInfo>> {
    let mut stmt = conn.prepare(
        "SELECT id, snapshot_id, path, permissions, modification_time, size
         FROM files
         WHERE snapshot_id = ?1
         ORDER BY path ASC",
    )?;
    let rows = stmt.query_map(params![snapshot_id], |row| {
        Ok(FileInfo {
            id: row.get(0)?,
            snapshot_id: row.get(1)?,
            path: row.get(2)?,
            permissions: row.get(3)?,
            modification_time: row.get(4)?,
            size: row.get(5)?,
        })
    })?;
    let mut files = Vec::new();
    for row in rows {
        files.push(row?);
    }
    Ok(files)
}

/// Conta snapshots que ainda estão em andamento.
pub fn count_in_progress(conn: &Connection) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM snapshots WHERE status = 'in-progress'",
        [],
        |row| row.get(0),
    )
    .context("Falha ao consultar snapshots em andamento")
}

/// Recalcula `reference_count` a partir das referências reais em `file_chunks`.
///
/// Deve ser executado dentro da mesma transação que altera snapshots/arquivos.
pub fn recalculate_reference_counts(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE chunks
         SET reference_count = (
             SELECT COUNT(*) FROM file_chunks WHERE file_chunks.chunk_hash = chunks.hash_id
         )",
        [],
    )
    .context("Falha ao recalcular reference_count")?;
    Ok(())
}

/// Verifica a integridade de um snapshot: todo chunk referenciado deve existir
/// no banco e no disco.
pub fn verify_snapshot_integrity(
    conn: &Connection,
    snapshot_id: i64,
    repo_root: &std::path::Path,
) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT fc.chunk_hash
         FROM file_chunks fc
         JOIN files f ON f.id = fc.file_id
         WHERE f.snapshot_id = ?1",
    )?;
    let rows = stmt.query_map(params![snapshot_id], |row| row.get::<_, String>(0))?;
    let mut problems = Vec::new();
    for row in rows {
        let hash = row?;
        match get_chunk(conn, &hash)? {
            Some(chunk) => {
                if chunk.size < 0 {
                    problems.push(format!("chunk {} tem tamanho inválido no banco", hash));
                    continue;
                }
                match crate::repository::chunk_path(repo_root, &hash) {
                    Ok(path) => {
                        if !path.is_file() {
                            problems.push(format!(
                                "chunk {} ausente no disco ({})",
                                hash,
                                path.display()
                            ));
                        }
                    }
                    Err(error) => problems.push(error.to_string()),
                }
            }
            None => problems.push(format!("chunk {} referenciado mas ausente no banco", hash)),
        }
    }
    Ok(problems)
}

/// Conta quantas referências reais um chunk possui em `file_chunks`.
pub fn chunk_actual_references(conn: &Connection, hash_id: &str) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM file_chunks WHERE chunk_hash = ?1",
        params![hash_id],
        |row| row.get(0),
    )
    .context("Falha ao contar referências do chunk")
}

/// Lista todos os hashes de chunks conhecidos.
pub fn list_chunk_hashes(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT hash_id FROM chunks ORDER BY hash_id")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut hashes = Vec::new();
    for row in rows {
        hashes.push(row?);
    }
    Ok(hashes)
}
