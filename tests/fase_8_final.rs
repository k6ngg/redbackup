//! Testes finais da Fase 8 — Integração ponta-a-ponta, stress e portabilidade.

use redbackup::pipeline::{chunking, compress, crypto, hash, write};
use std::fs;

const CHAVE_TESTE: [u8; 32] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32,
];

/// Teste de integração ponta-a-ponta: arquivo → chunk → hash → compress → crypto → write.
#[test]
fn integracao_ponta_a_ponta() {
    let temp_dir = tempfile::tempdir().expect("falha ao criar tempdir");
    let dados = b"Arquivo de teste para integracao final do redbackup.";
    let chunks = chunking::chunk_file(dados, 64, 1024);
    assert!(!chunks.is_empty());

    for chunk in &chunks {
        let hash_str = hash::hash_bytes(chunk);
        assert!(!hash_str.is_empty());
        assert_eq!(hash_str.len(), 64); // BLAKE3 hex = 64 chars

        let comprimido = compress::compress(chunk, 3);
        let criptografado = crypto::encrypt(&comprimido, &CHAVE_TESTE);
        let arquivo_chunk = temp_dir
            .path()
            .join(format!("chunk_final_{}", &hash_str[..8]));
        write::atomic_write(&criptografado, &arquivo_chunk).expect("falha na escrita atomica");
        assert!(arquivo_chunk.exists());
    }
}

/// Teste de stress simplificado: simula +100 arquivos pequenos.
#[test]
fn stress_muitos_arquivos() {
    let temp_dir = tempfile::tempdir().expect("falha ao criar tempdir");
    for i in 0..150 {
        let arquivo = temp_dir.path().join(format!("arquivo_{}.txt", i));
        fs::write(&arquivo, format!("conteudo do arquivo {}", i)).expect("falha na escrita");
        assert!(arquivo.exists());
    }
}
