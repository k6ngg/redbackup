//! Testes da Fase 3 — Pipeline de Backup (chunking, hashing, compressão, criptografia, escrita atômica).

use redbackup::pipeline::{chunking, compress, crypto, hash, verify, write};
use std::fs;

const CHAVE_TESTE: [u8; 32] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32,
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
    assert!(
        tmp_files.is_empty(),
        "Nao deve haver arquivos .tmp residuais"
    );
}

#[test]
fn validacao_de_chunk_armazenado_detecta_corrupcao() {
    let dados = b"conteudo de referencia para validacao de chunk armazenado";
    let temp_dir = tempfile::tempdir().expect("falha ao criar tempdir");
    let caminho = temp_dir.path().join("data/ab/chunk_validado");

    let esperado = hash::hash_bytes(dados);
    let comprimido = compress::try_compress(dados, 3).unwrap();
    let criptografado = crypto::try_encrypt(&comprimido, &CHAVE_TESTE).unwrap();
    write::atomic_write(&criptografado, &caminho).unwrap();

    assert!(
        verify::validate_stored_chunk(&caminho, &CHAVE_TESTE, &esperado).unwrap(),
        "chunk íntegro deveria ser aceito"
    );
    assert!(
        !verify::validate_stored_chunk(&caminho, &CHAVE_TESTE, &"0".repeat(64)).unwrap(),
        "hash divergente deveria ser rejeitado"
    );

    let mut adulterado = criptografado.clone();
    let ultimo = adulterado.len() - 1;
    adulterado[ultimo] ^= 0x01;
    fs::write(&caminho, &adulterado).unwrap();
    assert!(
        verify::validate_stored_chunk(&caminho, &CHAVE_TESTE, &esperado).is_err(),
        "adulteração deveria falhar na autenticação"
    );

    fs::write(&caminho, &criptografado[..criptografado.len() / 2]).unwrap();
    assert!(
        verify::validate_stored_chunk(&caminho, &CHAVE_TESTE, &esperado).is_err(),
        "chunk truncado deveria ser rejeitado"
    );

    // Outra chave não pode autenticar o chunk e também deve ser rejeitada.
    fs::write(&caminho, &criptografado).unwrap();
    let mut outra_chave = CHAVE_TESTE;
    outra_chave[0] ^= 0xff;
    assert!(
        verify::validate_stored_chunk(&caminho, &outra_chave, &esperado).is_err(),
        "chave incorreta deveria falhar na autenticação"
    );
}
