//! Vinculação entre a chave AES-256 e o repositório.
//!
//! O repositório guarda apenas um verificador derivado da chave com BLAKE3 em
//! modo keyed. O verificador permite confirmar que a chave fornecida é a mesma
//! usada pelo repositório sem armazenar a chave, um hash simples da chave ou
//! qualquer material que permita recuperá-la.
//!
//! Repositórios criados antes da existência do verificador são vinculados na
//! primeira operação que precisa gravar dados. Nessa migração, todos os chunks
//! fisicamente presentes são autenticados com a chave fornecida; se qualquer um
//! falhar, a vinculação é recusada para não misturar chaves.

use crate::metadata::migrations::run_migrations;
use crate::metadata::repository::{connect_or_create, list_chunk_hashes};
use crate::pipeline::verify::validate_stored_chunk;
use crate::repository::{chunk_path, index_db_path};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

const VERIFIER_CONTEXT: &[u8] = b"redbackup.repository-key.v1";
const VERIFIER_PREFIX: &str = "blake3-keyed-v1:";

/// Deriva o verificador público da chave para o repositório.
pub fn verifier_for_key(key: &[u8; 32]) -> String {
    let digest = blake3::keyed_hash(key, VERIFIER_CONTEXT);
    format!("{}{}", VERIFIER_PREFIX, digest.to_hex())
}

/// Lê o verificador persistido. Retorna `None` em repositórios antigos que
/// ainda não possuem a tabela ou a linha de vinculação.
pub fn stored_verifier(conn: &Connection) -> Result<Option<String>> {
    let table_exists: bool = conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM sqlite_master
             WHERE type = 'table' AND name = 'repository_key'
         )",
        [],
        |row| row.get(0),
    )?;
    if !table_exists {
        return Ok(None);
    }
    conn.query_row(
        "SELECT verifier FROM repository_key WHERE id = 1",
        [],
        |row| row.get(0),
    )
    .optional()
    .context("Falha ao consultar a chave vinculada ao repositório")
}

/// Exige que a chave corresponda à vinculação existente.
///
/// Repositórios antigos sem vinculação permanecem legíveis com a chave
/// fornecida; apenas operações de gravação fazem a vinculação definitiva por
/// meio de [`bind_repository_key`].
pub fn require_matching_key_if_bound(conn: &Connection, key: &[u8; 32]) -> Result<()> {
    if let Some(stored) = stored_verifier(conn)? {
        ensure_verifier_matches(&stored, &verifier_for_key(key))?;
    }
    Ok(())
}

/// Vincula a chave ao repositório ou confirma a vinculação existente.
///
/// Se o repositório ainda não tiver vinculação, todos os chunks existentes são
/// validados com a chave antes de gravar o verificador. A operação é
/// idempotente para a mesma chave.
pub fn bind_repository_key(conn: &Connection, repo_root: &Path, key: &[u8; 32]) -> Result<()> {
    let expected = verifier_for_key(key);
    if let Some(stored) = stored_verifier(conn)? {
        return ensure_verifier_matches(&stored, &expected);
    }

    verify_existing_chunks(conn, repo_root, key)?;

    // `ON CONFLICT DO NOTHING` evita sobrescrever uma vinculação criada
    // concorrentemente; a leitura seguinte confirma qual valor prevaleceu.
    conn.execute(
        "INSERT INTO repository_key (id, verifier)
         VALUES (1, ?1)
         ON CONFLICT(id) DO NOTHING",
        params![expected],
    )
    .context("Falha ao vincular a chave ao repositório")?;

    let stored = stored_verifier(conn)?.ok_or_else(|| {
        anyhow::anyhow!("Falha ao persistir a vinculação da chave ao repositório")
    })?;
    ensure_verifier_matches(&stored, &expected)
}

/// Valida a chave contra a vinculação, executando migrações pendentes.
///
/// Usado por processos de longa duração para falhar antes de iniciar um
/// serviço com a chave errada.
pub fn check_bound_key(repo_root: &Path, key: &[u8; 32]) -> Result<()> {
    let db_path = index_db_path(repo_root);
    let conn = connect_or_create(
        db_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Caminho do banco não é UTF-8"))?,
    )?;
    run_migrations(&conn).context("Falha ao preparar schema para validar a chave")?;
    require_matching_key_if_bound(&conn, key)
}

fn ensure_verifier_matches(stored: &str, expected: &str) -> Result<()> {
    if stored != expected {
        anyhow::bail!(
            "A chave fornecida não corresponde à chave vinculada a este repositório. \
             Use a chave original; nenhum dado foi alterado."
        );
    }
    Ok(())
}

fn verify_existing_chunks(conn: &Connection, repo_root: &Path, key: &[u8; 32]) -> Result<()> {
    for hash_id in list_chunk_hashes(conn)? {
        let path = chunk_path(repo_root, &hash_id)?;
        if !path.is_file() {
            // Um chunk ausente não revela qual chave foi usada; a ausência em si
            // é tratada pela verificação de integridade e pela recuperação.
            continue;
        }
        let valid = validate_stored_chunk(&path, key, &hash_id).with_context(|| {
            format!(
                "Não foi possível vincular a chave ao repositório: o chunk {} não pôde ser validado",
                hash_id
            )
        })?;
        if !valid {
            anyhow::bail!(
                "Não foi possível vincular a chave ao repositório: o chunk {} foi autenticado, \
                 mas seu hash não confere. O repositório pode conter dados cifrados com outra \
                 chave; use a chave original.",
                hash_id
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verificador_depende_da_chave_e_e_deterministico() {
        let chave_a = [1u8; 32];
        let chave_b = [2u8; 32];
        assert_eq!(verifier_for_key(&chave_a), verifier_for_key(&chave_a));
        assert_ne!(verifier_for_key(&chave_a), verifier_for_key(&chave_b));
        assert!(verifier_for_key(&chave_a).starts_with(VERIFIER_PREFIX));
    }
}
