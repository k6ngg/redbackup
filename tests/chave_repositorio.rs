//! Vinculação da chave ao repositório.
//!
//! Cobre a recusa de uma chave diferente em backup, restauração, scrubbing e
//! daemon, a migração de repositórios antigos e a vinculação feita por `init`.

use redbackup::config::settings::Settings;
use redbackup::metadata::repository::{connect_or_create, list_chunk_hashes, list_snapshots};
use redbackup::repository::{index_db_path, key_binding, settings_path};
use redbackup::restoration::rebuild::restore_from_snapshot;
use redbackup::{backup, config::key, scrubbing};
use std::fs;
use std::path::Path;
use std::process::Command;

const CHAVE_A: [u8; 32] = [1u8; 32];
const CHAVE_B: [u8; 32] = [2u8; 32];

fn configurar(source: &Path, repo: &Path) -> Settings {
    Settings {
        source_path: source.display().to_string(),
        destination_path: repo.display().to_string(),
        key_file_path: None,
        chunk_min_size: 64,
        chunk_max_size: 1024,
        retention_days: 30,
    }
}

fn escrever(source: &Path, relative: &str, conteudo: &[u8]) {
    let path = source.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, conteudo).unwrap();
}

fn conectar(repo: &Path) -> rusqlite::Connection {
    connect_or_create(index_db_path(repo).to_str().unwrap()).unwrap()
}

#[test]
fn repositorio_rejeita_chave_diferente_em_todas_as_operacoes() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("origem");
    let repo = temp.path().join("repo");
    fs::create_dir_all(&source).unwrap();
    escrever(&source, "arquivo.txt", b"conteudo vinculado a chave A");
    escrever(&source, "vazio.txt", b"");

    let settings = configurar(&source, &repo);
    let primeiro = backup::run_backup(&repo, &settings, &CHAVE_A).unwrap();
    assert_eq!(primeiro.status, "completed");

    let conn = conectar(&repo);
    let snapshots_antes = list_snapshots(&conn).unwrap().len();
    let chunks_antes = list_chunk_hashes(&conn).unwrap().len();

    // Novo conteúdo faria o backup gravar chunks se a chave B fosse aceita.
    escrever(&source, "novo.txt", b"conteudo que so existe na chave B");

    let erro = backup::run_backup(&repo, &settings, &CHAVE_B).unwrap_err();
    assert!(
        erro.to_string().contains("vinculada"),
        "erro inesperado: {:#}",
        erro
    );
    assert_eq!(
        list_snapshots(&conn).unwrap().len(),
        snapshots_antes,
        "backup com chave errada não pode criar snapshot"
    );
    assert_eq!(
        list_chunk_hashes(&conn).unwrap().len(),
        chunks_antes,
        "backup com chave errada não pode gravar chunks"
    );

    let saida = temp.path().join("nao-restaurar.txt");
    let erro = restore_from_snapshot(
        &repo,
        &conn,
        primeiro.snapshot_id,
        "arquivo.txt",
        &saida,
        &CHAVE_B,
        false,
    )
    .unwrap_err();
    assert!(
        erro.to_string().contains("vinculada"),
        "erro inesperado: {:#}",
        erro
    );
    assert!(!saida.exists());

    // Mesmo um arquivo vazio, sem chunks, deve ser recusado com a chave errada.
    let saida_vazia = temp.path().join("vazio-restaurado.txt");
    let erro = restore_from_snapshot(
        &repo,
        &conn,
        primeiro.snapshot_id,
        "vazio.txt",
        &saida_vazia,
        &CHAVE_B,
        false,
    )
    .unwrap_err();
    assert!(
        erro.to_string().contains("vinculada"),
        "erro inesperado: {:#}",
        erro
    );
    assert!(!saida_vazia.exists());

    let erro = scrubbing::scrub_repository(&repo, &CHAVE_B, None).unwrap_err();
    assert!(
        erro.to_string().contains("vinculada"),
        "erro inesperado: {:#}",
        erro
    );
    let marcados: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM chunks WHERE corrupted_at IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        marcados, 0,
        "a chave errada não pode marcar chunks como corrompidos"
    );

    assert!(key_binding::check_bound_key(&repo, &CHAVE_B).is_err());
    assert!(key_binding::check_bound_key(&repo, &CHAVE_A).is_ok());

    // A chave correta continua funcionando e o conteúdo vazio é restaurável.
    let segundo = backup::run_backup(&repo, &settings, &CHAVE_A).unwrap();
    assert_eq!(segundo.status, "completed");
    restore_from_snapshot(
        &repo,
        &conn,
        primeiro.snapshot_id,
        "vazio.txt",
        &saida_vazia,
        &CHAVE_A,
        false,
    )
    .unwrap();
    assert_eq!(fs::read(&saida_vazia).unwrap(), b"");
}

#[test]
fn migracao_de_repositorio_antigo_valida_todos_os_chunks() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("origem");
    let repo = temp.path().join("repo");
    fs::create_dir_all(&source).unwrap();
    escrever(&source, "arquivo.txt", b"conteudo legado");

    let settings = configurar(&source, &repo);
    let primeiro = backup::run_backup(&repo, &settings, &CHAVE_A).unwrap();
    assert_eq!(primeiro.status, "completed");

    // Simula um repositório criado antes da migração v3.
    let conn = conectar(&repo);
    conn.execute("DELETE FROM repository_key", []).unwrap();
    assert!(key_binding::stored_verifier(&conn).unwrap().is_none());

    let erro = backup::run_backup(&repo, &settings, &CHAVE_B).unwrap_err();
    assert!(
        erro.to_string().contains("vincular a chave"),
        "erro inesperado: {:#}",
        erro
    );
    assert!(
        key_binding::stored_verifier(&conn).unwrap().is_none(),
        "a chave errada não pode ser vinculada"
    );

    let segundo = backup::run_backup(&repo, &settings, &CHAVE_A).unwrap();
    assert_eq!(segundo.status, "unchanged");
    assert_eq!(
        key_binding::stored_verifier(&conn).unwrap(),
        Some(key_binding::verifier_for_key(&CHAVE_A))
    );
}

#[test]
fn init_vincula_chave_e_recusa_outra_chave_sem_alterar_configuracao() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("origem");
    let repo = temp.path().join("repo");
    let chave_a = temp.path().join("chave-a");
    let chave_b = temp.path().join("chave-b");
    let config_home = temp.path().join("config-home");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&config_home).unwrap();
    fs::write(source.join("arquivo.txt"), b"conteudo init").unwrap();
    key::generate_key_file(&chave_a).unwrap();
    key::generate_key_file(&chave_b).unwrap();

    let executar = |chave: &Path| {
        Command::new(env!("CARGO_BIN_EXE_redbackup"))
            .env("XDG_CONFIG_HOME", &config_home)
            .env("HOME", temp.path())
            .args([
                "init",
                "--source",
                source.to_str().unwrap(),
                "--dest",
                repo.to_str().unwrap(),
                "--key-file",
                chave.to_str().unwrap(),
                "--min-chunk",
                "64",
                "--max-chunk",
                "1024",
            ])
            .output()
            .unwrap()
    };

    let primeira = executar(&chave_a);
    assert!(
        primeira.status.success(),
        "init falhou: {}",
        String::from_utf8_lossy(&primeira.stderr)
    );

    let settings_antes = fs::read_to_string(settings_path(&repo)).unwrap();
    let chave_a_carregada = key::load_key(&chave_a).unwrap();
    let conn = conectar(&repo);
    assert_eq!(
        key_binding::stored_verifier(&conn).unwrap(),
        Some(key_binding::verifier_for_key(&chave_a_carregada))
    );

    let segunda = executar(&chave_b);
    assert!(!segunda.status.success(), "init deveria recusar a chave B");
    assert!(
        String::from_utf8_lossy(&segunda.stderr).contains("vinculada"),
        "erro inesperado: {}",
        String::from_utf8_lossy(&segunda.stderr)
    );
    assert_eq!(
        fs::read_to_string(settings_path(&repo)).unwrap(),
        settings_antes,
        "settings.json não pode ser alterado por uma chave incompatível"
    );
    assert_eq!(
        key_binding::stored_verifier(&conn).unwrap(),
        Some(key_binding::verifier_for_key(&chave_a_carregada))
    );
}

#[test]
fn repositorio_novo_sem_chunks_adota_a_chave_no_primeiro_backup() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("origem");
    let repo = temp.path().join("repo");
    fs::create_dir_all(&source).unwrap();
    let settings = configurar(&source, &repo);

    let report = backup::run_backup(&repo, &settings, &CHAVE_B).unwrap();
    assert_eq!(report.status, "completed");
    assert_eq!(report.files_total, 0);

    let conn = conectar(&repo);
    assert_eq!(
        key_binding::stored_verifier(&conn).unwrap(),
        Some(key_binding::verifier_for_key(&CHAVE_B))
    );
    assert!(backup::run_backup(&repo, &settings, &CHAVE_A).is_err());
}
