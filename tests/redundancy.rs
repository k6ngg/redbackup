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
