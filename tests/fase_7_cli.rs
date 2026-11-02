//! Testes da Fase 7 — CLI e Comandos.

use redbackup::cli::commands::{cmd_init, cmd_prune};

#[test]
fn cli_init_rejeita_argumentos_vazios() {
    // Este teste verifica que o CLI trata configuração corretamente.
    // A função `cmd_init` retorna `i32` (código de saída).
    let code = cmd_init("", "", None);
    assert_eq!(code, 1); // erro de configuração
}

#[test]
fn cli_prune_exige_forca() {
    let code = cmd_prune(false);
    assert_eq!(code, 1); // erro: comando destrutivo requer `--force`
}
