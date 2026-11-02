use redbackup::{cli, config::settings::Settings, repository::initialize_repository};

#[test]
fn status_sem_chave_e_sem_snapshots() {
    let tmp = tempfile::tempdir().unwrap();
    let root = initialize_repository(tmp.path().to_str().unwrap()).unwrap();
    Settings {
        source_path: tmp.path().join("origem").display().to_string(),
        destination_path: root.display().to_string(),
        ..Settings::default()
    }
    .save(&root)
    .unwrap();
    assert_eq!(
        cli::run(vec![
            "redbackup".into(),
            "status".into(),
            "--repo".into(),
            root.display().to_string()
        ]),
        0
    );
    assert!(!root.join("metadata/backup-progress.json").exists());
}

#[test]
fn status_nao_inicializa_destino_invalido() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(
        cli::run(vec![
            "redbackup".into(),
            "status".into(),
            "--repo".into(),
            tmp.path().display().to_string()
        ]),
        1
    );
    assert!(!tmp.path().join("metadata").exists());
}

#[test]
fn completed_backup_persists_chunk_counters_and_duration() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    std::fs::create_dir(&source).unwrap();
    let payload: Vec<u8> = (0..80000).map(|n| (n % 251) as u8).collect();
    std::fs::write(source.join("large.bin"), &payload).unwrap();
    let root = initialize_repository(tmp.path().join("repo").to_str().unwrap()).unwrap();
    let settings = Settings {
        source_path: source.display().to_string(),
        destination_path: root.display().to_string(),
        chunk_min_size: 64,
        chunk_max_size: 1024,
        ..Settings::default()
    };
    settings.save(&root).unwrap();
    let report = redbackup::backup::run_backup(&root, &settings, &[42; 32]).unwrap();
    let status = redbackup::status::repository_status(&root).unwrap();
    assert_eq!(status["backup_active"], false);
    assert_eq!(status["backup_interrupted"], false);
    assert_eq!(status["backup"]["stage"], "completed");
    assert_eq!(status["backup"]["bytes_processed"], payload.len());
    assert_eq!(status["backup"]["chunks_new"], report.chunks_new);
    assert_eq!(status["backup"]["chunks_reused"], report.chunks_reused);
    assert!(status["backup"]["elapsed_seconds"].as_f64().unwrap() > 0.0);
    assert_eq!(status["last_completed"]["id"], report.snapshot_id);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(root.join("metadata"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}

#[cfg(unix)]
#[test]
fn daemon_answers_status_while_another_connection_waits() {
    use redbackup::daemon::{client::send_command_with_timeout, protocol::Request};
    use std::{
        os::unix::net::UnixStream,
        time::{Duration, Instant},
    };
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            if self.0.try_wait().ok().flatten().is_none() {
                self.0.kill().unwrap();
            }
            self.0.wait().unwrap();
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = initialize_repository(tmp.path().to_str().unwrap()).unwrap();
    let key = tmp.path().join("key");
    redbackup::config::key::generate_key_file(&key).unwrap();
    Settings {
        source_path: tmp.path().display().to_string(),
        destination_path: root.display().to_string(),
        key_file_path: Some(key.display().to_string()),
        ..Settings::default()
    }
    .save(&root)
    .unwrap();
    let mut child = ChildGuard(
        std::process::Command::new(env!("CARGO_BIN_EXE_redbackup"))
            .args(["daemon", "--repo", root.to_str().unwrap()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let socket = root.join("metadata/redbackup.sock");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !socket.exists() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "daemon encerrou antes de criar socket"
        );
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let waiting = UnixStream::connect(&socket).unwrap();
    let response =
        send_command_with_timeout(&socket, &Request::new("status"), Duration::from_secs(2))
            .unwrap();
    assert!(response.is_ok());
    assert_eq!(response.data.unwrap()["snapshots"]["completed"], 0);
    drop(waiting);
    assert!(
        send_command_with_timeout(&socket, &Request::new("shutdown"), Duration::from_secs(2))
            .unwrap()
            .is_ok()
    );
    while child.0.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!socket.exists());
}
