//! Testes de integração reais (pós-Fase 8).
//!
//! Diferente dos testes antigos, estes exercitam o motor completo: backup a
//! partir de um diretório real, metadados SQLite, deduplicação, restauração
//! byte a byte, pruning, scrubbing e recuperação.

use redbackup::backup;
use redbackup::config::settings::Settings;
use redbackup::metadata::repository::{
    connect_or_create, create_snapshot, list_chunk_hashes, list_files, list_snapshots,
};
use redbackup::recovery;
use redbackup::repository::lock::RepositoryLock;
use redbackup::repository::{chunk_path, data_dir, index_db_path, initialize_repository};
use redbackup::restoration::rebuild::restore_from_snapshot;
use redbackup::retention;
use redbackup::scrubbing;
use std::fs;
use std::path::{Path, PathBuf};

const CHAVE: [u8; 32] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32,
];

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

fn escrever(source: &Path, relative: &str, conteudo: &[u8]) -> PathBuf {
    let path = source.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, conteudo).unwrap();
    path
}

fn conectar(repo: &Path) -> rusqlite::Connection {
    connect_or_create(index_db_path(repo).to_str().unwrap()).unwrap()
}

fn arquivos_de_chunks(repo: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    fn visit(dir: &Path, result: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, result);
            } else {
                result.push(path);
            }
        }
    }
    visit(&data_dir(repo), &mut result);
    result
}

#[test]
fn bloqueio_impede_escritores_concorrentes() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");

    let primeiro = RepositoryLock::exclusive(&repo).unwrap();
    assert!(
        RepositoryLock::exclusive(&repo).is_err(),
        "segundo bloqueio exclusivo deveria falhar"
    );
    assert!(
        RepositoryLock::shared(&repo).is_err(),
        "bloqueio compartilhado não pode coexistir com exclusivo"
    );
    drop(primeiro);

    let leitor_a = RepositoryLock::shared(&repo).unwrap();
    let leitor_b = RepositoryLock::shared(&repo).unwrap();
    assert!(RepositoryLock::exclusive(&repo).is_err());
    drop(leitor_a);
    drop(leitor_b);
    assert!(RepositoryLock::exclusive(&repo).is_ok());
}

#[test]
fn backup_rejeita_origem_dentro_do_repositorio() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    initialize_repository(repo.to_str().unwrap()).unwrap();
    let source = repo.join("origem");
    fs::create_dir_all(&source).unwrap();
    escrever(&source, "arquivo.txt", b"dados");

    let settings = configurar(&source, &repo);
    let erro = backup::run_backup(&repo, &settings, &CHAVE).unwrap_err();
    assert!(
        erro.to_string().contains("dentro dele"),
        "erro inesperado: {}",
        erro
    );

    let conn = conectar(&repo);
    assert!(
        list_snapshots(&conn).unwrap().is_empty(),
        "nenhum snapshot deveria ser criado"
    );
}

#[test]
fn backup_e_restauracao_ponta_a_ponta() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("origem");
    let repo = temp.path().join("repo");
    fs::create_dir_all(&source).unwrap();

    let texto = "linha de teste com acentuação é ção\n".repeat(400);
    escrever(&source, "notas/ola.txt", texto.as_bytes());
    escrever(&source, "vazio.bin", b"");
    escrever(
        &source,
        "unicode/ãéîõü.txt",
        "conteúdo unicode: 日本語".as_bytes(),
    );
    let binario: Vec<u8> = (0..=255u8).cycle().take(9_000).collect();
    escrever(&source, "binario/dados.bin", &binario);

    let settings = configurar(&source, &repo);
    let report = backup::run_backup(&repo, &settings, &CHAVE).unwrap();
    assert_eq!(report.status, "completed", "erros: {:?}", report.errors);
    assert_eq!(report.files_total, 4);
    assert_eq!(report.files_completed, 4);
    assert_eq!(report.files_failed, 0);
    assert!(report.chunks_new > 0);

    let conn = conectar(&repo);
    let snapshots = list_snapshots(&conn).unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].file_count, 4);
    assert_eq!(
        snapshots[0].total_bytes as u64,
        (texto.len() + "conteúdo unicode: 日本語".len() + binario.len()) as u64
    );

    let arquivos = list_files(&conn, report.snapshot_id).unwrap();
    assert_eq!(arquivos.len(), 4);
    let caminhos: Vec<&str> = arquivos.iter().map(|file| file.path.as_str()).collect();
    assert!(caminhos.contains(&"notas/ola.txt"));
    assert!(caminhos.contains(&"vazio.bin"));
    assert!(caminhos.contains(&"unicode/ãéîõü.txt"));
    assert!(caminhos.contains(&"binario/dados.bin"));

    let destino = temp.path().join("restaurado");
    fs::create_dir_all(&destino).unwrap();
    for arquivo in &arquivos {
        let saida = destino.join(arquivo.path.replace('/', "_"));
        let report = restore_from_snapshot(
            &repo,
            &conn,
            report.snapshot_id,
            &arquivo.path,
            &saida,
            &CHAVE,
            false,
        )
        .unwrap();
        assert_eq!(report.bytes as i64, arquivo.size);
        let original = source.join(&arquivo.path);
        assert_eq!(
            fs::read(&saida).unwrap(),
            fs::read(&original).unwrap(),
            "conteúdo restaurado difere para {}",
            arquivo.path
        );
    }
}

#[cfg(unix)]
#[test]
fn restauracao_preserva_permissoes_e_mtime() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("origem");
    let repo = temp.path().join("repo");
    fs::create_dir_all(&source).unwrap();
    let arquivo = escrever(&source, "metadados.txt", b"conteudo com metadados");
    fs::set_permissions(&arquivo, fs::Permissions::from_mode(0o640)).unwrap();
    let mtime = filetime::FileTime::from_unix_time(1_600_000_000, 0);
    filetime::set_file_mtime(&arquivo, mtime).unwrap();

    let settings = configurar(&source, &repo);
    let report = backup::run_backup(&repo, &settings, &CHAVE).unwrap();
    assert_eq!(report.status, "completed");

    let conn = conectar(&repo);
    let saida = temp.path().join("saida.txt");
    restore_from_snapshot(
        &repo,
        &conn,
        report.snapshot_id,
        "metadados.txt",
        &saida,
        &CHAVE,
        false,
    )
    .unwrap();

    assert_eq!(
        fs::metadata(&saida).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_eq!(
        filetime::FileTime::from_last_modification_time(&fs::metadata(&saida).unwrap())
            .unix_seconds(),
        1_600_000_000
    );
}

#[test]
fn deduplicacao_reutiliza_chunks() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("origem");
    let repo = temp.path().join("repo");
    fs::create_dir_all(&source).unwrap();

    let conteudo = "conteudo repetido para deduplicacao\n".repeat(700);
    escrever(&source, "a.dat", conteudo.as_bytes());
    escrever(&source, "b.dat", conteudo.as_bytes());

    let settings = configurar(&source, &repo);
    let primeiro = backup::run_backup(&repo, &settings, &CHAVE).unwrap();
    assert_eq!(primeiro.status, "completed");
    assert!(
        primeiro.chunks_reused > 0,
        "o segundo arquivo deveria reutilizar chunks"
    );

    let conn = conectar(&repo);
    let fisicos = list_chunk_hashes(&conn).unwrap().len();
    assert_eq!(
        primeiro.chunks_new, fisicos,
        "nenhum chunk físico deveria ser duplicado"
    );

    let segundo = backup::run_backup(&repo, &settings, &CHAVE).unwrap();
    assert_eq!(
        segundo.status, "unchanged",
        "sem alterações, nenhum snapshot novo deve ser publicado"
    );
    assert_eq!(segundo.snapshot_id, primeiro.snapshot_id);
    assert_eq!(
        segundo.chunks_new, 0,
        "segundo backup não deveria gravar chunks"
    );
    assert!(segundo.chunks_reused > 0);
    assert_eq!(list_snapshots(&conn).unwrap().len(), 1);

    // Referências são contadas por ocorrência em `file_chunks`.
    let refs: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(reference_count), 0) FROM chunks",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let ocorrencias: i64 = conn
        .query_row("SELECT COUNT(*) FROM file_chunks", [], |row| row.get(0))
        .unwrap();
    assert_eq!(refs, ocorrencias);
}

#[test]
fn backup_nao_reutiliza_chunk_corrompido() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("origem");
    let repo = temp.path().join("repo");
    fs::create_dir_all(&source).unwrap();

    let conteudo = b"conteudo compartilhado para validacao";
    escrever(&source, "original.txt", conteudo);

    let settings = configurar(&source, &repo);
    let primeiro = backup::run_backup(&repo, &settings, &CHAVE).unwrap();
    assert_eq!(primeiro.status, "completed");

    let conn = conectar(&repo);
    let hash = list_chunk_hashes(&conn).unwrap().remove(0);
    let caminho = chunk_path(&repo, &hash).unwrap();
    let original_criptografado = fs::read(&caminho).unwrap();

    // Corrompe o chunk já armazenado e adiciona outro arquivo que geraria o
    // mesmo chunk. O backup não pode reutilizá-lo nem publicar `completed`.
    let mut corrompido = original_criptografado.clone();
    let ultimo = corrompido.len() - 1;
    corrompido[ultimo] ^= 0x01;
    fs::write(&caminho, &corrompido).unwrap();
    escrever(&source, "segundo.txt", conteudo);

    let segundo = backup::run_backup(&repo, &settings, &CHAVE).unwrap();
    assert_eq!(
        segundo.status, "failed",
        "snapshot com chunk corrompido não pode ser concluído"
    );
    assert_ne!(segundo.snapshot_id, primeiro.snapshot_id);
    assert_eq!(segundo.files_completed, 0);
    assert_eq!(segundo.files_failed, 2);
    assert_eq!(segundo.chunks_new, 0);
    assert_eq!(segundo.chunks_reused, 0);
    assert!(
        segundo.errors.iter().all(|erro| {
            erro.message.contains("não será reutilizado")
                || erro.message.contains("está corrompido")
        }),
        "erros inesperados: {:?}",
        segundo.errors
    );

    let status: String = conn
        .query_row(
            "SELECT status FROM snapshots WHERE id = ?1",
            rusqlite::params![segundo.snapshot_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "failed");

    // Nenhum arquivo ou referência de chunk foi publicado no snapshot falho.
    let arquivos: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE snapshot_id = ?1",
            rusqlite::params![segundo.snapshot_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(arquivos, 0);
    let referencias: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM file_chunks fc
             JOIN files f ON f.id = fc.file_id
             WHERE f.snapshot_id = ?1",
            rusqlite::params![segundo.snapshot_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(referencias, 0);

    // A corrupção fica registrada para diagnóstico sem apagar o chunk.
    let marcado: Option<String> = conn
        .query_row(
            "SELECT corrupted_at FROM chunks WHERE hash_id = ?1",
            rusqlite::params![&hash],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        marcado.is_some(),
        "a corrupção detectada no backup deveria ser registrada"
    );
    assert!(caminho.is_file(), "o chunk corrompido não pode ser apagado");

    // Depois de restaurar o chunk correto, o backup volta a reutilizá-lo,
    // limpa o diagnóstico e permite restaurar o segundo arquivo.
    fs::write(&caminho, &original_criptografado).unwrap();
    let terceiro = backup::run_backup(&repo, &settings, &CHAVE).unwrap();
    assert_eq!(terceiro.status, "completed");
    assert!(terceiro.chunks_reused >= 1);

    let marcado: Option<String> = conn
        .query_row(
            "SELECT corrupted_at FROM chunks WHERE hash_id = ?1",
            rusqlite::params![&hash],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        marcado.is_none(),
        "chunk validado deveria ter o diagnóstico limpo"
    );

    let saida = temp.path().join("segundo-restaurado.txt");
    restore_from_snapshot(
        &repo,
        &conn,
        terceiro.snapshot_id,
        "segundo.txt",
        &saida,
        &CHAVE,
        false,
    )
    .unwrap();
    assert_eq!(fs::read(&saida).unwrap(), conteudo);
}

#[test]
fn alteracao_apenas_de_mtime_nao_cria_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("origem");
    let repo = temp.path().join("repo");
    fs::create_dir_all(&source).unwrap();
    let arquivo = escrever(&source, "arquivo.txt", b"conteudo inalterado");

    let settings = configurar(&source, &repo);
    let primeiro = backup::run_backup(&repo, &settings, &CHAVE).unwrap();
    assert_eq!(primeiro.status, "completed");

    let novo_mtime = filetime::FileTime::from_unix_time(1_700_000_000, 0);
    filetime::set_file_mtime(&arquivo, novo_mtime).unwrap();

    let segundo = backup::run_backup(&repo, &settings, &CHAVE).unwrap();
    assert_eq!(
        segundo.status, "unchanged",
        "mtime sem mudança de conteúdo não deve gerar snapshot"
    );
    assert_eq!(segundo.snapshot_id, primeiro.snapshot_id);

    let conn = conectar(&repo);
    assert_eq!(list_snapshots(&conn).unwrap().len(), 1);
    let refs: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(reference_count), 0) FROM chunks",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let ocorrencias: i64 = conn
        .query_row("SELECT COUNT(*) FROM file_chunks", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        refs, ocorrencias,
        "descartar o snapshot deve desfazer as referências incrementadas"
    );
}

#[cfg(unix)]
#[test]
fn alteracao_de_permissao_cria_novo_snapshot() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("origem");
    let repo = temp.path().join("repo");
    fs::create_dir_all(&source).unwrap();
    let arquivo = escrever(&source, "arquivo.txt", b"conteudo inalterado");
    fs::set_permissions(&arquivo, fs::Permissions::from_mode(0o600)).unwrap();

    let settings = configurar(&source, &repo);
    let primeiro = backup::run_backup(&repo, &settings, &CHAVE).unwrap();
    assert_eq!(primeiro.status, "completed");

    fs::set_permissions(&arquivo, fs::Permissions::from_mode(0o640)).unwrap();
    let segundo = backup::run_backup(&repo, &settings, &CHAVE).unwrap();
    assert_eq!(
        segundo.status, "completed",
        "mudança de permissão afeta a restauração e deve gerar snapshot"
    );
    assert_ne!(segundo.snapshot_id, primeiro.snapshot_id);
}

#[test]
fn alteracao_de_conteudo_cria_novo_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("origem");
    let repo = temp.path().join("repo");
    fs::create_dir_all(&source).unwrap();
    escrever(&source, "arquivo.txt", b"conteudo original");

    let settings = configurar(&source, &repo);
    let primeiro = backup::run_backup(&repo, &settings, &CHAVE).unwrap();
    assert_eq!(primeiro.status, "completed");

    escrever(&source, "arquivo.txt", b"conteudo modificado");
    let segundo = backup::run_backup(&repo, &settings, &CHAVE).unwrap();
    assert_eq!(segundo.status, "completed");
    assert_ne!(segundo.snapshot_id, primeiro.snapshot_id);

    let conn = conectar(&repo);
    let snapshots = list_snapshots(&conn).unwrap();
    assert_eq!(snapshots.len(), 2);
    assert!(snapshots
        .iter()
        .all(|snapshot| snapshot.status == "completed"));
}

#[test]
fn pruning_remove_snapshot_e_chunks_orfaos() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("origem");
    let repo = temp.path().join("repo");
    fs::create_dir_all(&source).unwrap();
    escrever(&source, "arquivo.txt", &b"dados para pruning".repeat(200));

    let settings = configurar(&source, &repo);
    let report = backup::run_backup(&repo, &settings, &CHAVE).unwrap();
    assert_eq!(report.status, "completed");
    assert!(!arquivos_de_chunks(&repo).is_empty());

    let conn = conectar(&repo);
    conn.execute(
        "UPDATE snapshots SET timestamp = '2000-01-01T00:00:00Z' WHERE id = ?1",
        rusqlite::params![report.snapshot_id],
    )
    .unwrap();
    drop(conn);

    let prune = retention::prune_repository(&repo, 30).unwrap();
    assert_eq!(prune.snapshots_removed, 1);
    assert!(prune.chunks_removed > 0);
    assert!(prune.errors.is_empty(), "erros: {:?}", prune.errors);
    assert!(
        arquivos_de_chunks(&repo).is_empty(),
        "chunks órfãos deveriam ter sido removidos do disco"
    );

    let conn = conectar(&repo);
    assert!(list_snapshots(&conn).unwrap().is_empty());
    let chunks: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))
        .unwrap();
    assert_eq!(chunks, 0);
}

#[test]
fn pruning_adia_chunks_quando_ha_snapshot_em_andamento() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("origem");
    let repo = temp.path().join("repo");
    fs::create_dir_all(&source).unwrap();
    escrever(
        &source,
        "arquivo.txt",
        &b"dados para pruning concorrente".repeat(200),
    );

    let settings = configurar(&source, &repo);
    let report = backup::run_backup(&repo, &settings, &CHAVE).unwrap();
    assert_eq!(report.status, "completed");

    let conn = conectar(&repo);
    conn.execute(
        "UPDATE snapshots SET timestamp = '2000-01-01T00:00:00Z' WHERE id = ?1",
        rusqlite::params![report.snapshot_id],
    )
    .unwrap();
    create_snapshot(&conn, "/origem/em-andamento").unwrap();
    drop(conn);

    let prune = retention::prune_repository(&repo, 30).unwrap();
    assert_eq!(prune.snapshots_removed, 1);
    assert!(prune.deferred);
    assert_eq!(prune.chunks_removed, 0);
    assert!(!arquivos_de_chunks(&repo).is_empty());

    // Recover encerra o snapshot em andamento e já remove os chunks órfãos
    // (o banco não tem mais operação em andamento).
    let recovery = recovery::recover_repository(&repo).unwrap();
    assert!(recovery.stale_snapshots_failed >= 1);
    assert!(
        !recovery.removed.is_empty(),
        "recuperação deveria remover os chunks órfãos"
    );
    assert!(arquivos_de_chunks(&repo).is_empty());

    // Um pruning posterior é seguro e idempotente: não há mais o que remover.
    let prune = retention::prune_repository(&repo, 30).unwrap();
    assert!(!prune.deferred);
    assert_eq!(prune.chunks_removed, 0);
    assert!(prune.errors.is_empty(), "erros: {:?}", prune.errors);
    assert!(arquivos_de_chunks(&repo).is_empty());
}

#[test]
fn scrubbing_detecta_chunk_corrompido() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("origem");
    let repo = temp.path().join("repo");
    fs::create_dir_all(&source).unwrap();
    escrever(&source, "arquivo.txt", &b"dados para scrubbing".repeat(300));

    let settings = configurar(&source, &repo);
    let report = backup::run_backup(&repo, &settings, &CHAVE).unwrap();
    assert_eq!(report.status, "completed");

    let limpo = scrubbing::scrub_repository(&repo, &CHAVE, None).unwrap();
    assert!(limpo.corrupted.is_empty());
    assert!(limpo.checked > 0);

    let conn = conectar(&repo);
    let hash = list_chunk_hashes(&conn).unwrap().remove(0);
    let caminho = chunk_path(&repo, &hash).unwrap();
    let original = fs::read(&caminho).unwrap();
    let mut corrompido_bytes = original.clone();
    corrompido_bytes.push(0xff);
    fs::write(&caminho, corrompido_bytes).unwrap();

    let corrompido = scrubbing::scrub_repository(&repo, &CHAVE, None).unwrap();
    assert!(
        corrompido
            .corrupted
            .iter()
            .any(|problem| problem.hash_id == hash),
        "chunk corrompido não foi detectado: {:?}",
        corrompido.corrupted
    );
    let marcado: Option<String> = conn
        .query_row(
            "SELECT corrupted_at FROM chunks WHERE hash_id = ?1",
            rusqlite::params![hash],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        marcado.is_some(),
        "corrupção deveria ficar registrada no banco"
    );

    // Depois de reparar o arquivo, um novo scrubbing limpa o diagnóstico.
    fs::write(&caminho, original).unwrap();
    let limpo = scrubbing::scrub_repository(&repo, &CHAVE, None).unwrap();
    assert!(limpo.corrupted.is_empty());
    let marcado: Option<String> = conn
        .query_row(
            "SELECT corrupted_at FROM chunks WHERE hash_id = ?1",
            rusqlite::params![hash],
            |row| row.get(0),
        )
        .unwrap();
    assert!(marcado.is_none(), "diagnóstico deveria ser limpo");
}

#[test]
fn restauracao_falha_com_chunk_corrompido_sem_criar_saida() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("origem");
    let repo = temp.path().join("repo");
    fs::create_dir_all(&source).unwrap();
    escrever(
        &source,
        "arquivo.txt",
        &b"restauracao com corrupcao".repeat(100),
    );

    let settings = configurar(&source, &repo);
    let report = backup::run_backup(&repo, &settings, &CHAVE).unwrap();
    assert_eq!(report.status, "completed");

    let conn = conectar(&repo);
    let hash = list_chunk_hashes(&conn).unwrap().remove(0);
    let caminho = chunk_path(&repo, &hash).unwrap();
    fs::write(&caminho, b"conteudo invalido").unwrap();

    let saida = temp.path().join("nao_deve_existir.txt");
    let erro = restore_from_snapshot(
        &repo,
        &conn,
        report.snapshot_id,
        "arquivo.txt",
        &saida,
        &CHAVE,
        false,
    )
    .unwrap_err();
    assert!(
        erro.to_string().contains("autenticação")
            || erro.to_string().contains("descriptografar")
            || erro.to_string().contains("criptografado"),
        "erro inesperado: {}",
        erro
    );
    assert!(!saida.exists(), "arquivo parcial não deveria ser publicado");
}

#[test]
fn recuperacao_marca_snapshot_interrompido_e_remove_temporarios() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("origem");
    let repo = temp.path().join("repo");
    fs::create_dir_all(&source).unwrap();
    escrever(&source, "arquivo.txt", b"dados");

    let settings = configurar(&source, &repo);
    let report = backup::run_backup(&repo, &settings, &CHAVE).unwrap();
    assert_eq!(report.status, "completed");

    let conn = conectar(&repo);
    create_snapshot(&conn, "/origem/interrompida").unwrap();
    drop(conn);

    let tmp = data_dir(&repo).join("aa").join(".tmp_residuo");
    fs::create_dir_all(tmp.parent().unwrap()).unwrap();
    fs::write(&tmp, b"parcial").unwrap();

    let recovery = recovery::recover_repository(&repo).unwrap();
    assert!(recovery.stale_snapshots_failed >= 1);
    assert!(
        recovery
            .removed
            .iter()
            .any(|path| path.contains(".tmp_residuo")),
        "temporário não foi removido: {:?}",
        recovery.removed
    );
    assert!(!tmp.exists());

    let conn = conectar(&repo);
    let interrompido = conn
        .query_row(
            "SELECT status FROM snapshots ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap();
    assert_eq!(interrompido, "failed");
}
