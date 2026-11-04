#![cfg(target_os = "linux")]

use std::fs;
use std::process::Command;

#[test]
fn falha_fsync_diretorio_impede_conclusao_inclusive_na_repeticao() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let source = root.join("source");
    let repo = root.join("repo");
    let key = root.join("key");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("arquivo"), b"dados para teste de durabilidade").unwrap();
    let c = root.join("fault.c");
    let lib = root.join("fault.so");
    fs::write(
        &c,
        r#"
#define _GNU_SOURCE
#include <dlfcn.h>
#include <unistd.h>
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
int fsync(int fd) {
    struct stat st; char link[64], path[4096];
    if (fstat(fd, &st) == 0 && S_ISDIR(st.st_mode)) {
        snprintf(link, sizeof(link), "/proc/self/fd/%d", fd);
        ssize_t n = readlink(link, path, sizeof(path)-1);
        if (n > 0) { path[n] = 0; if (strstr(path, "/repo/data/")) {
            fprintf(stderr, "FSYNC_INJETADO\n"); errno = EIO; return -1;
        }}
    }
    int (*real_fsync)(int) = dlsym(RTLD_NEXT, "fsync");
    return real_fsync(fd);
}
"#,
    )
    .unwrap();
    assert!(Command::new("cc")
        .args(["-shared", "-fPIC", "-o"])
        .arg(&lib)
        .arg(&c)
        .arg("-ldl")
        .status()
        .unwrap()
        .success());

    let run = |args: &[&str], inject: bool| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_redbackup"));
        cmd.env("HOME", root)
            .env("XDG_CONFIG_HOME", root.join("config"));
        if inject {
            cmd.env("LD_PRELOAD", &lib);
        }
        cmd.args(args).output().unwrap()
    };
    let repo_str = repo.to_str().unwrap();
    assert!(run(
        &[
            "init",
            "--source",
            source.to_str().unwrap(),
            "--dest",
            repo_str,
            "--key-file",
            key.to_str().unwrap()
        ],
        false
    )
    .status
    .success());
    for _ in 0..2 {
        let result = run(&["backup", "--repo", repo_str, "--no-daemon"], true);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("FSYNC_INJETADO"));
    }
    let conn = rusqlite::Connection::open(repo.join("metadata/index.db")).unwrap();
    let statuses: Vec<String> = conn
        .prepare("SELECT status FROM snapshots ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(statuses, ["failed", "failed"]);
    let references: i64 = conn
        .query_row("SELECT COUNT(*) FROM file_chunks", [], |row| row.get(0))
        .unwrap();
    assert_eq!(references, 0);
    assert!(run(&["backup", "--repo", repo_str, "--no-daemon"], false)
        .status
        .success());
    let completed: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM snapshots WHERE status='completed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(completed, 1);
}
