//! Testes da Fase 7 — CLI e Comandos.

use redbackup::cli::commands::{cmd_init, cmd_prune};
use std::fs;
use std::process::Command;

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

#[test]
fn recover_aceita_chave_explicita_e_recusa_chave_incorreta() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let repo = temp.path().join("repo");
    let key = temp.path().join("key");
    let wrong = temp.path().join("wrong");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("arquivo"), b"dados").unwrap();
    fs::write(&key, [7; 32]).unwrap();
    fs::write(&wrong, [8; 32]).unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_redbackup"))
            .env("HOME", temp.path())
            .env("XDG_CONFIG_HOME", temp.path().join("config"))
            .args(args)
            .output()
            .unwrap()
    };
    let repo = repo.to_str().unwrap();
    assert!(run(&[
        "init",
        "--source",
        source.to_str().unwrap(),
        "--dest",
        repo,
        "--key-file",
        key.to_str().unwrap()
    ])
    .status
    .success());
    assert!(run(&["backup", "--repo", repo, "--no-daemon"])
        .status
        .success());
    assert!(!run(&[
        "recover",
        "--repo",
        repo,
        "--key-file",
        wrong.to_str().unwrap()
    ])
    .status
    .success());
    assert!(run(&[
        "recover",
        "--repo",
        repo,
        "--key-file",
        key.to_str().unwrap()
    ])
    .status
    .success());
}
