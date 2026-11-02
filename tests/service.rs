#![cfg(unix)]
use redbackup::{config::settings::Settings, repository::initialize_repository};
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
        }
        self.0.wait().unwrap();
    }
}
fn wait_for(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !predicate() {
        assert!(Instant::now() < deadline, "timeout");
        std::thread::sleep(Duration::from_millis(50));
    }
}
fn completed(root: &Path) -> i64 {
    let conn = rusqlite::Connection::open(root.join("metadata/index.db")).unwrap();
    conn.query_row(
        "SELECT COUNT(*) FROM snapshots WHERE status='completed'",
        [],
        |r| r.get(0),
    )
    .unwrap()
}
#[test]
fn service_monitors_rejects_duplicate_and_stops_on_sigterm() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("example.txt"), b"first").unwrap();
    let root = initialize_repository(tmp.path().join("repo").to_str().unwrap()).unwrap();
    let key = tmp.path().join("key");
    redbackup::config::key::generate_key_file(&key).unwrap();
    Settings {
        source_path: source.display().to_string(),
        destination_path: root.display().to_string(),
        key_file_path: Some(key.display().to_string()),
        ..Settings::default()
    }
    .save(&root)
    .unwrap();
    let spawn = || {
        Command::new(env!("CARGO_BIN_EXE_redbackupd"))
            .args(["--repo", root.to_str().unwrap()])
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap()
    };
    let mut daemon = Process(spawn());
    wait_for(|| root.join("metadata/redbackup.sock").exists());
    let mut duplicate = Process(spawn());
    wait_for(|| duplicate.0.try_wait().unwrap().is_some());
    assert!(!duplicate.0.wait().unwrap().success());
    wait_for(|| completed(&root) >= 1);
    std::fs::write(source.join("example.txt"), b"second").unwrap();
    wait_for(|| completed(&root) >= 2);
    assert!(Command::new("kill")
        .args(["-TERM", &daemon.0.id().to_string()])
        .status()
        .unwrap()
        .success());
    wait_for(|| daemon.0.try_wait().unwrap().is_some());
    assert!(daemon.0.wait().unwrap().success());
    assert!(!root.join("metadata/redbackup.sock").exists());
}

#[test]
fn supervisor_waits_and_discovers_init_without_arguments() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("config");
    let source = tmp.path().join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("example.txt"), b"registered").unwrap();
    let root = tmp.path().join("repo");
    let key = tmp.path().join("key");
    let mut daemon = Process(
        Command::new(env!("CARGO_BIN_EXE_redbackupd"))
            .env("XDG_CONFIG_HOME", &config)
            .env_remove("REDBACKUP_REPO")
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    wait_for(|| config.join("redbackup/manager.lock").exists());
    assert!(daemon.0.try_wait().unwrap().is_none());
    assert!(!root.exists());
    let cli = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_redbackup"))
            .env("XDG_CONFIG_HOME", &config)
            .args(args)
            .stdout(Stdio::null())
            .status()
            .unwrap()
    };
    assert!(cli(&[
        "init",
        "--source",
        source.to_str().unwrap(),
        "--dest",
        root.to_str().unwrap(),
        "--key-file",
        key.to_str().unwrap()
    ])
    .success());
    wait_for(|| root.join("metadata/redbackup.sock").exists());
    wait_for(|| completed(&root) >= 1);
    assert!(cli(&["repository", "remove", root.to_str().unwrap()]).success());
    wait_for(|| !root.join("metadata/redbackup.sock").exists());
    assert_eq!(completed(&root), 1);
    // Missing key is retried without replacing it or losing the registration.
    std::fs::rename(&key, tmp.path().join("saved-key")).unwrap();
    assert!(cli(&["repository", "add", root.to_str().unwrap()]).success());
    std::thread::sleep(Duration::from_secs(3));
    assert!(!key.exists());
    assert!(!root.join("metadata/redbackup.sock").exists());
    std::fs::rename(tmp.path().join("saved-key"), &key).unwrap();
    wait_for(|| root.join("metadata/redbackup.sock").exists());
    assert!(Command::new("kill")
        .args(["-TERM", &daemon.0.id().to_string()])
        .status()
        .unwrap()
        .success());
    wait_for(|| daemon.0.try_wait().unwrap().is_some());
    assert!(daemon.0.wait().unwrap().success());
    assert!(!root.join("metadata/redbackup.sock").exists());
}
