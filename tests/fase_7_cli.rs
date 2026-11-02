//! Testes da Fase 7 — CLI e Comandos.

use redbackup::cli::commands::{cmd_init, cmd_prune};
use std::fs;

#[test]
fn cli_init_rejeita_argumentos_vazios() {
    // Este teste verifica que o CLI trata configuração corretamente.
    // A função `cmd_init` retorna `i32` (código de saída).
    let code = cmd_init("", "", None);
    assert_eq!(code, 1); // erro de configuração
}

#[test]
fn cli_init_nao_cria_destino_quando_chave_e_invalida() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("origem");
    fs::create_dir_all(&source).unwrap();

    let dest = temp.path().join("repo");
    let key = temp.path().join("chave-invalida");
    fs::write(&key, b"").unwrap();

    let code = cmd_init(
        source.to_str().unwrap(),
        dest.to_str().unwrap(),
        Some(key.to_str().unwrap()),
    );
    assert_eq!(code, 1);
    assert!(
        !dest.exists(),
        "o destino não pode ser criado antes de validar a chave"
    );
}

#[test]
fn cli_prune_exige_forca() {
    let code = cmd_prune(false);
    assert_eq!(code, 1); // erro: comando destrutivo requer `--force`
}
