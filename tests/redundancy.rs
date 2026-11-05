use redbackup::{
    backup,
    config::settings::Settings,
    metadata::repository::{connect_or_create, list_chunk_hashes},
    redundancy::{self, Mode},
    repository::{chunk_path, index_db_path},
};
use std::{fs, path::Path};

const KEY: [u8; 32] = [37; 32];

fn setup(
    root: &Path,
) -> (
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
    Settings,
) {
    let source = root.join("source");
    let repo = root.join("repo");
    let dest = root.join("redundant");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&dest).unwrap();
    fs::write(source.join("file"), b"conteudo inicial para backup").unwrap();
    let settings = Settings {
        source_path: source.display().to_string(),
        destination_path: repo.display().to_string(),
        chunk_min_size: 64,
        chunk_max_size: 1024,
        ..Settings::default()
    };
    assert_eq!(
        backup::run_backup(&repo, &settings, &KEY).unwrap().status,
        "completed"
    );
    settings.save(&repo).unwrap();
    (source, repo, dest, settings)
}

#[test]
fn sincroniza_snapshots_existentes_e_repeticao_verifica_copias() {
    let tmp = tempfile::tempdir().unwrap();
    let (_, repo, dest, settings) = setup(tmp.path());
    redundancy::configure(&repo, &settings, &dest, Mode::BestEffort).unwrap();
    assert_eq!(
        redundancy::read_state(&repo).unwrap().unwrap().state,
        "pending"
    );
    assert_eq!(redundancy::sync_repository(&repo, &KEY).unwrap(), 1);
    let conn = connect_or_create(index_db_path(&repo).to_str().unwrap()).unwrap();
    let hash = &list_chunk_hashes(&conn).unwrap()[0];
    assert_eq!(
        fs::read(chunk_path(&repo, hash).unwrap()).unwrap(),
        fs::read(chunk_path(&dest, hash).unwrap()).unwrap()
    );
    assert_eq!(redundancy::sync_repository(&repo, &KEY).unwrap(), 1);
    assert_eq!(
        redundancy::read_state(&repo).unwrap().unwrap().state,
        "protected"
    );
    assert_eq!(
        redbackup::status::repository_status(&repo).unwrap()["redundancy"]["state"],
        "protected"
    );
    assert!(redundancy::sync_repository(&repo, &[38; 32]).is_err());
    assert_eq!(
        redundancy::read_state(&repo).unwrap().unwrap().state,
        "protected"
    );
}

#[test]
fn falha_de_destino_distingue_modos_e_pode_ser_repetida() {
    for mode in [Mode::BestEffort, Mode::Required] {
        let tmp = tempfile::tempdir().unwrap();
        let (source, repo, dest, settings) = setup(tmp.path());
        redundancy::configure(&repo, &settings, &dest, mode).unwrap();
        redundancy::sync_repository(&repo, &KEY).unwrap();
        let offline = tmp.path().join("offline");
        fs::rename(&dest, &offline).unwrap();
        fs::write(source.join("novo"), b"novos dados nao compartilhados").unwrap();
        let report = backup::run_backup(&repo, &settings, &KEY).unwrap();
        assert_eq!(
            report.status,
            if mode == Mode::Required {
                "failed"
            } else {
                "completed"
            }
        );
        assert_eq!(report.redundancy.unwrap().state, "degraded");
        assert_eq!(
            redundancy::read_state(&repo).unwrap().unwrap().state,
            "degraded"
        );
        fs::rename(&offline, &dest).unwrap();
        redundancy::sync_repository(&repo, &KEY).unwrap();
        assert_eq!(
            redundancy::read_state(&repo).unwrap().unwrap().state,
            "protected"
        );
        if mode == Mode::Required {
            assert_eq!(
                backup::run_backup(&repo, &settings, &KEY).unwrap().status,
                "completed"
            );
        }
    }
}

#[test]
fn copia_corrompida_nao_e_sobrescrita_e_repositorio_principal_permanece_intacto() {
    let tmp = tempfile::tempdir().unwrap();
    let (_, repo, dest, settings) = setup(tmp.path());
    redundancy::configure(&repo, &settings, &dest, Mode::BestEffort).unwrap();
    redundancy::sync_repository(&repo, &KEY).unwrap();
    let conn = connect_or_create(index_db_path(&repo).to_str().unwrap()).unwrap();
    let hash = &list_chunk_hashes(&conn).unwrap()[0];
    let original = fs::read(chunk_path(&repo, hash).unwrap()).unwrap();
    let target = chunk_path(&dest, hash).unwrap();
    fs::write(&target, b"copia corrompida").unwrap();
    assert!(redundancy::sync_repository(&repo, &KEY).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"copia corrompida");
    assert_eq!(
        fs::read(chunk_path(&repo, hash).unwrap()).unwrap(),
        original
    );
    assert_eq!(
        redundancy::read_state(&repo).unwrap().unwrap().state,
        "degraded"
    );
}

#[test]
fn recusa_destinos_sobrepostos_e_nao_vazios() {
    let tmp = tempfile::tempdir().unwrap();
    let (source, repo, dest, settings) = setup(tmp.path());
    assert!(redundancy::configure(&repo, &settings, &source, Mode::Required).is_err());
    assert!(redundancy::configure(&repo, &settings, &repo, Mode::Required).is_err());
    fs::write(dest.join("outros-dados"), b"preservar").unwrap();
    assert!(redundancy::configure(&repo, &settings, &dest, Mode::Required).is_err());
    assert_eq!(fs::read(dest.join("outros-dados")).unwrap(), b"preservar");
}

#[test]
fn backup_novo_e_copiado_automaticamente() {
    let tmp = tempfile::tempdir().unwrap();
    let (source, repo, dest, settings) = setup(tmp.path());
    redundancy::configure(&repo, &settings, &dest, Mode::Required).unwrap();
    fs::write(source.join("outro"), b"segundo conteudo exclusivo").unwrap();
    let report = backup::run_backup(&repo, &settings, &KEY).unwrap();
    assert_eq!(report.status, "completed");
    assert_eq!(report.redundancy.unwrap().state, "protected");
    let conn = connect_or_create(index_db_path(&repo).to_str().unwrap()).unwrap();
    for hash in list_chunk_hashes(&conn).unwrap() {
        assert_eq!(
            fs::read(chunk_path(&repo, &hash).unwrap()).unwrap(),
            fs::read(chunk_path(&dest, &hash).unwrap()).unwrap()
        );
    }
}

#[test]
fn comandos_cli_configuram_e_sincronizam() {
    use std::process::Command;
    let tmp = tempfile::tempdir().unwrap();
    let (_, repo, dest, mut settings) = setup(tmp.path());
    let key = tmp.path().join("key");
    fs::write(&key, KEY).unwrap();
    settings.key_file_path = Some(key.display().to_string());
    settings.save(&repo).unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_redbackup"))
            .env("HOME", tmp.path())
            .env("XDG_CONFIG_HOME", tmp.path().join("config"))
            .args(args)
            .output()
            .unwrap()
    };
    let repo = repo.to_str().unwrap();
    let dest = dest.to_str().unwrap();
    let configured = run(&[
        "redundancy-configure",
        "--repo",
        repo,
        "--dest",
        dest,
        "--mode",
        "required",
    ]);
    assert!(
        configured.status.success(),
        "{}",
        String::from_utf8_lossy(&configured.stderr)
    );
    let synced = run(&["redundancy-sync", "--repo", repo]);
    assert!(
        synced.status.success(),
        "{}",
        String::from_utf8_lossy(&synced.stderr)
    );
    let status = run(&["status", "--repo", repo]);
    assert!(status.status.success());
    assert!(String::from_utf8_lossy(&status.stdout).contains("protected"));
}

#[test]
fn configuracao_exige_modo_explicito_sem_criar_arquivos_em_falhas() {
    use std::process::Command;
    for mode in ["best-effort", "required"] {
        let tmp = tempfile::tempdir().unwrap();
        let (_, repo, dest, _) = setup(tmp.path());
        let run = |extra: &[&str]| {
            Command::new(env!("CARGO_BIN_EXE_redbackup"))
                .env("HOME", tmp.path())
                .env("XDG_CONFIG_HOME", tmp.path().join("config"))
                .args([
                    "redundancy-configure",
                    "--repo",
                    repo.to_str().unwrap(),
                    "--dest",
                    dest.to_str().unwrap(),
                ])
                .args(extra)
                .output()
                .unwrap()
        };
        let missing = run(&[]);
        assert!(!missing.status.success());
        assert!(String::from_utf8_lossy(&missing.stderr).contains("--mode"));
        assert!(!run(&["--mode", "invalid"]).status.success());
        assert!(!run(&["--mode", ""]).status.success());
        assert_eq!(
            redbackup::cli::commands::cmd_redundancy_configure(
                Some(repo.to_str().unwrap()),
                dest.to_str().unwrap(),
                None
            ),
            1
        );
        assert!(fs::read_dir(&dest).unwrap().next().is_none());
        assert!(!repo.join("config/redundancy.json").exists());
        assert!(!repo.join("metadata/redundancy-state.json").exists());
        let result = run(&["--mode", mode]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let config = redundancy::load(&repo).unwrap().unwrap();
        assert_eq!(
            config.mode,
            if mode == "required" {
                Mode::Required
            } else {
                Mode::BestEffort
            }
        );
    }
}

#[test]
fn recover_repara_chunk_corrompido_ou_ausente_e_preserva_original() {
    for missing in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let (_, repo, dest, settings) = setup(tmp.path());
        redundancy::configure(&repo, &settings, &dest, Mode::BestEffort).unwrap();
        redundancy::sync_repository(&repo, &KEY).unwrap();
        let conn = connect_or_create(index_db_path(&repo).to_str().unwrap()).unwrap();
        let hash = list_chunk_hashes(&conn).unwrap()[0].clone();
        let original = chunk_path(&repo, &hash).unwrap();
        let expected = fs::read(&original).unwrap();
        if missing {
            fs::remove_file(&original).unwrap();
        } else {
            fs::write(&original, b"dados corrompidos").unwrap();
        }
        let result = redbackup::recovery::recover_repository(&repo, &KEY).unwrap();
        assert_eq!(result.repaired.len(), 1);
        assert!(result.unrecoverable.is_empty());
        assert_eq!(fs::read(&original).unwrap(), expected);
        match &result.repaired[0].preserved_original {
            Some(path) => {
                assert!(!missing);
                assert_eq!(fs::read(path).unwrap(), b"dados corrompidos");
            }
            None => assert!(missing),
        }
        let status: String = conn
            .query_row("SELECT status FROM snapshots WHERE id=1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "completed");
        let output = tmp.path().join("restore");
        redbackup::restoration::rebuild::restore_from_snapshot(
            &repo, &conn, 1, "file", &output, &KEY, false,
        )
        .unwrap();
        assert_eq!(fs::read(output).unwrap(), b"conteudo inicial para backup");
        assert!(redbackup::recovery::recover_repository(&repo, &KEY)
            .unwrap()
            .repaired
            .is_empty());
    }
}

#[test]
fn reparo_recusa_copia_invalida_e_tentativa_posterior_nao_promove_snapshot_falho() {
    let tmp = tempfile::tempdir().unwrap();
    let (_, repo, dest, settings) = setup(tmp.path());
    redundancy::configure(&repo, &settings, &dest, Mode::BestEffort).unwrap();
    redundancy::sync_repository(&repo, &KEY).unwrap();
    let conn = connect_or_create(index_db_path(&repo).to_str().unwrap()).unwrap();
    let hash = list_chunk_hashes(&conn).unwrap()[0].clone();
    let original = chunk_path(&repo, &hash).unwrap();
    let redundant = chunk_path(&dest, &hash).unwrap();
    let valid = fs::read(&redundant).unwrap();
    fs::write(&original, b"corrompido").unwrap();
    fs::write(&redundant, b"invalido").unwrap();
    let first = redbackup::recovery::recover_repository(&repo, &KEY).unwrap();
    assert_eq!(first.unrecoverable.len(), 1);
    assert!(first.repaired.is_empty());
    assert_eq!(fs::read(&original).unwrap(), b"corrompido");
    let status: String = conn
        .query_row("SELECT status FROM snapshots WHERE id=1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(status, "failed");
    fs::write(&redundant, valid).unwrap();
    let second = redbackup::recovery::recover_repository(&repo, &KEY).unwrap();
    assert_eq!(second.repaired.len(), 1);
    assert!(second.unrecoverable.is_empty());
    let status: String = conn
        .query_row("SELECT status FROM snapshots WHERE id=1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(status, "failed");
}

#[test]
fn reparo_recusa_copia_cifrada_com_outra_chave_e_destino_indisponivel() {
    for offline in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let (_, repo, dest, settings) = setup(tmp.path());
        redundancy::configure(&repo, &settings, &dest, Mode::BestEffort).unwrap();
        redundancy::sync_repository(&repo, &KEY).unwrap();
        let conn = connect_or_create(index_db_path(&repo).to_str().unwrap()).unwrap();
        let hash = list_chunk_hashes(&conn).unwrap()[0].clone();
        let original = chunk_path(&repo, &hash).unwrap();
        fs::write(&original, b"corrompido").unwrap();
        if offline {
            fs::rename(&dest, tmp.path().join("offline")).unwrap();
        } else {
            let compressed =
                redbackup::pipeline::compress::try_compress(b"conteudo inicial para backup", 3)
                    .unwrap();
            let ciphertext =
                redbackup::pipeline::crypto::try_encrypt(&compressed, &[99; 32]).unwrap();
            fs::write(chunk_path(&dest, &hash).unwrap(), ciphertext).unwrap();
        }
        let result = redbackup::recovery::recover_repository(&repo, &KEY).unwrap();
        assert_eq!(result.unrecoverable.len(), 1);
        assert!(result.repaired.is_empty());
        assert_eq!(fs::read(&original).unwrap(), b"corrompido");
        assert!(!repo.join("metadata/quarantine").exists());
        let status: String = conn
            .query_row("SELECT status FROM snapshots WHERE id=1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "failed");
    }
}

#[test]
fn recover_cli_indica_corrupto_sem_copia_com_codigo_de_erro() {
    let tmp = tempfile::tempdir().unwrap();
    let (_, repo, _, mut settings) = setup(tmp.path());
    let key = tmp.path().join("key");
    fs::write(&key, KEY).unwrap();
    settings.key_file_path = Some(key.display().to_string());
    settings.save(&repo).unwrap();
    let conn = connect_or_create(index_db_path(&repo).to_str().unwrap()).unwrap();
    let hash = list_chunk_hashes(&conn).unwrap()[0].clone();
    fs::write(chunk_path(&repo, &hash).unwrap(), b"corrompido").unwrap();
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_redbackup"))
        .args(["recover", "--repo", repo.to_str().unwrap()])
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&result.stderr).contains("CHUNK NÃO RECUPERÁVEL"));
}

#[test]
fn reparo_de_chunk_compartilhado_preserva_snapshots_e_chunks_nao_relacionados() {
    let tmp = tempfile::tempdir().unwrap();
    let (source, repo, dest, settings) = setup(tmp.path());
    redundancy::configure(&repo, &settings, &dest, Mode::Required).unwrap();
    redundancy::sync_repository(&repo, &KEY).unwrap();
    fs::write(source.join("other"), b"conteudo totalmente independente").unwrap();
    let second = backup::run_backup(&repo, &settings, &KEY).unwrap();
    assert_eq!(second.status, "completed");
    let conn = connect_or_create(index_db_path(&repo).to_str().unwrap()).unwrap();
    let shared: String = conn.query_row("SELECT fc.chunk_hash FROM file_chunks fc JOIN files f ON f.id=fc.file_id WHERE f.snapshot_id=1 AND f.path='file'", [], |r| r.get(0)).unwrap();
    let other: String = conn.query_row("SELECT fc.chunk_hash FROM file_chunks fc JOIN files f ON f.id=fc.file_id WHERE f.snapshot_id=?1 AND f.path='other'", [second.snapshot_id], |r| r.get(0)).unwrap();
    let other_path = chunk_path(&repo, &other).unwrap();
    let other_before = fs::read(&other_path).unwrap();
    fs::write(chunk_path(&repo, &shared).unwrap(), b"corrompido").unwrap();
    let report = redbackup::recovery::recover_repository(&repo, &KEY).unwrap();
    assert_eq!(report.repaired.len(), 1);
    assert_eq!(fs::read(other_path).unwrap(), other_before);
    let statuses: Vec<String> = conn
        .prepare("SELECT status FROM snapshots ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(statuses, ["completed", "completed"]);
    for id in [1, second.snapshot_id] {
        let output = tmp.path().join(format!("restored-{id}"));
        redbackup::restoration::rebuild::restore_from_snapshot(
            &repo, &conn, id, "file", &output, &KEY, false,
        )
        .unwrap();
        assert_eq!(fs::read(output).unwrap(), b"conteudo inicial para backup");
    }
}
