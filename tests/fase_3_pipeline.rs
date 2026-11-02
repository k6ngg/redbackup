//! Testes da Fase 3 — Pipeline de Backup (chunking, hashing, compressão, criptografia, escrita atômica).

use std::fs;
use redbackup::pipeline::{chunking, hash, compress, crypto, write};

const CHAVE_TESTE: [u8; 32] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
    17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32
];

#[test]
fn fluxo_pipeline_basico() {
    let dados = b"Conteudo de exemplo para chunking, hashing, compressao e criptografia.";
    let temp_dir = tempfile::tempdir().expect("falha ao criar tempdir");
    let _arquivo_final = temp_dir.path().join("data/ab/chunk_teste");

    // 1. Chunking (min_size >= 64, avg_size >= 256, max_size >= 1024 conforme fastcdc)
    let chunks = chunking::chunk_file(dados, 64, 1024);
    assert!(!chunks.is_empty(), "Deveria gerar pelo menos um chunk");

    // 2. Hashing
    let hashes: Vec<String> = chunks.iter().map(|c| hash::hash_bytes(c)).collect();
    assert!(!hashes.is_empty());

    // 3. Compressao e criptografia para cada chunk
    for (i, chunk) in chunks.iter().enumerate() {
        let comprimido = compress::compress(chunk, 3);
        let criptografado = crypto::encrypt(&comprimido, &CHAVE_TESTE);
        let caminho = temp_dir.path().join(format!("chunk_{}", i));
        write::atomic_write(&criptografado, &caminho).expect("falha na escrita atomica");
        assert!(caminho.exists());
    }

    // 4. Verifica que os arquivos temporarios foram renomeados (nao existem .tmp)
    let tmp_files: Vec<_> = fs::read_dir(temp_dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp_"))
        .collect();
    assert!(tmp_files.is_empty(), "Nao deve haver arquivos .tmp residuais");
}
