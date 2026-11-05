//! Contrato público do parser, sem executar operações sobre repositórios reais.
use std::process::{Command, Output};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_redbackup"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn ajuda_e_versao_preservam_codigos_e_canais() {
    for args in [
        vec![],
        vec!["help"],
        vec!["--help"],
        vec!["-h"],
        vec!["--version"],
        vec!["-V"],
        vec!["version"],
    ] {
        let result = cli(&args);
        assert_eq!(result.status.code(), Some(0), "{args:?}");
        assert!(!result.stdout.is_empty());
        assert!(result.stderr.is_empty());
    }
    let result = cli(&["restore", "--help"]);
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(text.contains("--snapshot"));
    assert!(!text.contains("redundancy-sync"));
    let result = cli(&["repository", "add", "--help"]);
    assert_eq!(result.status.code(), Some(0));
    assert!(String::from_utf8(result.stdout).unwrap().contains("<PATH>"));
    let result = Command::new(env!("CARGO_BIN_EXE_redbackupd"))
        .arg("--help")
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(0));
    assert!(String::from_utf8(result.stdout)
        .unwrap()
        .contains("--key-file"));
}

#[test]
fn erros_de_parsing_retornam_um_antes_de_executar_comandos() {
    for args in [
        vec!["inexistente", "--help"],
        vec!["backup", "--repo"],
        vec!["backup", "--repo", "--no-daemon"],
        vec!["backup", "--repos", "x"],
        vec!["backup", "--repo", "a", "--repo", "b"],
        vec!["prune", "--force=true"],
        vec!["prune", "--force", "--days", "1KiB"],
        vec!["watch", "--debounce", "5MB"],
        vec!["init", "--source", "a", "--dest", "b"],
        vec![
            "restore",
            "--snapshot",
            "abc",
            "--file",
            "x",
            "--output",
            "y",
        ],
        vec!["redundancy-configure", "--dest", "x", "--mode", "rapido"],
        vec!["repository", "add"],
    ] {
        let result = cli(&args);
        assert_eq!(result.status.code(), Some(1), "{args:?}: {result:?}");
        assert!(!result.stderr.is_empty());
        assert!(result.stdout.is_empty());
    }
    assert_eq!(cli(&["prune"]).status.code(), Some(1));
}

#[test]
fn fluxo_cli_restaura_bytes_e_preserva_conflitos() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir(root.join("source")).unwrap();
    let payload = b"conteudo de teste\x00\xff";
    std::fs::write(root.join("source/documento"), payload).unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_redbackup"))
            .args(args)
            .current_dir(root)
            .env("XDG_CONFIG_HOME", root.join("config-usuario"))
            .env("REDBACKUP_REPO", root.join("repo"))
            .output()
            .unwrap()
    };
    for args in [
        vec!["keygen", "--path", "key"],
        vec![
            "init",
            "--source=source",
            "--dest=repo",
            "--key-file=key",
            "--min-chunk=64",
            "--max-chunk=1KiB",
        ],
        vec!["backup", "--manual", "--no-daemon"],
        vec!["list-snapshots"],
        vec!["list-files", "--snapshot", "1"],
        vec![
            "restore",
            "--snapshot=1",
            "--file=documento",
            "--output=restaurado",
        ],
    ] {
        let result = run(&args);
        assert_eq!(result.status.code(), Some(0), "{args:?}: {result:?}");
    }
    assert_eq!(std::fs::read(root.join("restaurado")).unwrap(), payload);
    let restore = [
        "restore",
        "--snapshot=1",
        "--file=documento",
        "--output=restaurado",
    ];
    std::fs::write(root.join("restaurado"), b"existente").unwrap();
    assert!(!run(&restore).status.success());
    assert_eq!(
        std::fs::read(root.join("restaurado")).unwrap(),
        b"existente"
    );
    let mut forced = restore.to_vec();
    forced.push("--force");
    assert!(run(&forced).status.success());
    assert_eq!(std::fs::read(root.join("restaurado")).unwrap(), payload);
}
