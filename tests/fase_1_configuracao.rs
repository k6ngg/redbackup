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

#[test]
fn inicializacao_rejeita_diretorio_nao_vazio_sem_repositorio() {
    let temp_dir = tempfile::tempdir().expect("falha ao criar tempdir");
    let dest = temp_dir.path().join("pasta_de_imagens");
    fs::create_dir_all(&dest).unwrap();
    let imagem = dest.join("captura.png");
    fs::write(&imagem, b"conteudo").unwrap();

    let result = initialize_repository(dest.to_str().unwrap());
    assert!(
        result.is_err(),
        "não deveria transformar uma pasta arbitrária em repositório"
    );
    assert!(imagem.exists(), "arquivo do usuário não pode ser alterado");
    assert!(
        !dest.join("metadata").exists(),
        "nenhuma estrutura de repositório deveria ser criada"
    );
}
