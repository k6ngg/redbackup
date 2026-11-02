//! Testes da Fase 6 — Resiliência, Manutenção e Limpeza.

use redbackup::recovery::clean_orphans;
use std::fs;

#[test]
fn crash_recovery_remove_arquivos_tmp() {
    let temp_dir = tempfile::tempdir().expect("falha ao criar tempdir");
    let data_dir = temp_dir.path().join("data");
    fs::create_dir_all(&data_dir).unwrap();
    fs::File::create(data_dir.join(".tmp_orfao")).unwrap();
    // Como a verificação no `clean_orphans` usa apenas o nome, o arquivo .tmp será considerado.
    let removed =
        clean_orphans(data_dir.to_str().unwrap(), "/tmp/test_invalid.db").expect("falha no clean");
    assert!(!removed.is_empty());
}
