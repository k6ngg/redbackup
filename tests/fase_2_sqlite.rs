//! Testes da Fase 2 — Schema SQLite e Metadados.

use redbackup::metadata::migrations::run_migrations;
use redbackup::metadata::repository::{
    connect_or_create, decrement_reference_count, get_chunk, upsert_chunk,
};

#[test]
fn aplicacao_de_migracao_cria_tabelas() {
    let temp_dir = tempfile::tempdir().expect("falha ao criar tempdir");
    let db_path = temp_dir.path().join("test_index.db");
    let conn = connect_or_create(db_path.to_str().unwrap()).expect("falha ao criar conn");
    run_migrations(&conn).expect("falha ao rodar migrações");
    // Se chegar até aqui sem erro, as tabelas foram criadas.
    assert!(db_path.exists());

    let versions: Vec<i64> = {
        let mut stmt = conn
            .prepare("SELECT version FROM schema_version ORDER BY version")
            .unwrap();
        let rows = stmt.query_map([], |row| row.get(0)).unwrap();
        rows.map(|row| row.unwrap()).collect()
    };
    assert_eq!(versions, vec![1, 2]);

    let columns: Vec<String> = {
        let mut stmt = conn.prepare("PRAGMA table_info(chunks)").unwrap();
        let rows = stmt.query_map([], |row| row.get(1)).unwrap();
        rows.map(|row| row.unwrap()).collect()
    };
    assert!(columns.iter().any(|name| name == "corrupted_at"));
    assert!(columns.iter().any(|name| name == "corruption_reason"));
}

#[test]
fn operacao_chunk_referencia_e_consulta() {
    let temp_dir = tempfile::tempdir().expect("falha ao criar tempdir");
    let db_path = temp_dir.path().join("test_ref.db");
    let conn = connect_or_create(db_path.to_str().unwrap()).expect("falha ao criar conn");
    run_migrations(&conn).expect("falha ao rodar migrações");

    // Inserir chunk
    upsert_chunk(&conn, "hash_teste_01", 1024, Some(512)).expect("falha no upsert");
    let chunk = get_chunk(&conn, "hash_teste_01").expect("falha na consulta");
    assert!(chunk.is_some());
    assert_eq!(chunk.unwrap().reference_count, 1); // reference_count = 1

    // Decrementar referência
    let dec = decrement_reference_count(&conn, "hash_teste_01").expect("falha no decremento");
    assert!(dec);
    let chunk_after = get_chunk(&conn, "hash_teste_01").expect("falha na consulta pós-decremento");
    assert_eq!(chunk_after.unwrap().reference_count, 0);
}
