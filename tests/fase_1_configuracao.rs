//! Testes da Fase 1 — Configuração e inicialização segura.

use redbackup::repository::initialize_repository;
use std::fs;

#[test]
fn inicializacao_cria_estrutura() {
    let temp_dir = tempfile::tempdir().expect("falha ao criar tempdir");
    let dest = temp_dir.path().join("repositorio_teste");
    let result = initialize_repository(dest.to_str().unwrap());
    assert!(result.is_ok(), "Inicialização falhou: {:?}", result);
    assert!(dest.join("data").exists());
    assert!(dest.join("metadata").exists());
    assert!(dest.join("config").exists());
}

#[test]
fn inicializacao_detecta_inconsistencia() {
    let temp_dir = tempfile::tempdir().expect("falha ao criar tempdir");
    let dest = temp_dir.path().join("repositorio_inconsistente");
    fs::create_dir_all(&dest).unwrap();
    fs::create_dir_all(dest.join("metadata")).unwrap();
    fs::File::create(dest.join("metadata/index.db")).unwrap();
    // Não cria `data/`
    let result = initialize_repository(dest.to_str().unwrap());
    assert!(
        result.is_err(),
        "Deveria detectar inconsistência (db existe, data ausente)"
    );
}
