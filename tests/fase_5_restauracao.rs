//! Testes da Fase 5 — Restauração.

use std::fs;

use redbackup::restoration::rebuild::restore_file;

const CHAVE_TESTE: [u8; 32] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
    17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32
];

#[test]
fn restauracao_impede_sobrescrita_sem_forca() {
    let temp_dir = tempfile::tempdir().expect("falha ao criar tempdir");
    let dest = temp_dir.path().join("arquivo_restaurado.txt");
    fs::write(&dest, b"existente").unwrap();

    let result = restore_file(
        &[], // sem chunks
        &dest,
        temp_dir.path(),
        &CHAVE_TESTE,
        false,
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("force"));
}
