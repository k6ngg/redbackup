//! Comandos da CLI do redbackup.
//!
//! Códigos de saída:
//! - 0: sucesso
//! - 1: erro de configuração/uso
//! - 2: erro de I/O
//! - 3: erro de criptografia
//! - 4: erro de integridade/engine

use crate::backup;
use crate::config::key;
use crate::config::settings::{
    Settings, CHUNK_MAX_BYTES, CHUNK_MAX_MIN_BYTES, CHUNK_MIN_BYTES, CHUNK_MIN_MAX_BYTES,
};
use crate::config::validation::{set_file_permissions, validate_path};
use crate::daemon::client;
use crate::daemon::protocol::Request;
use crate::metadata::migrations::run_migrations;
use crate::metadata::repository::{
    connect_or_create, list_files, list_snapshots, FileInfo, SnapshotInfo,
};
use crate::recovery;
use crate::repository::{
    index_db_path, initialize_repository, key_binding, open_repository, settings_path, socket_path,
};
use crate::restoration::rebuild::restore_from_snapshot;
use crate::retention;
use crate::scrubbing;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub fn exit_with_code(code: i32, msg: &str) -> ! {
    eprintln!("ERRO [{}]: {}", code, msg);
    std::process::exit(code);
}

/// Comando `init` — configura diretórios de origem/destino e chave.
pub fn cmd_init(source: &str, destination: &str, key_file: Option<&str>) -> i32 {
    cmd_init_full(source, destination, key_file, None, None, None, None)
}

pub fn cmd_init_full(
    source: &str,
    destination: &str,
    key_file: Option<&str>,
    retention_days: Option<u64>,
    chunk_min: Option<u64>,
    chunk_max: Option<u64>,
    metadata_backup: Option<&str>,
) -> i32 {
    if source.trim().is_empty() || destination.trim().is_empty() {
        eprintln!("ERRO [1]: Diretórios de origem e destino são obrigatórios.");
        return 1;
    }
    let Some(key_file) = key_file.filter(|path| !path.trim().is_empty()) else {
        eprintln!(
            "ERRO [1]: `--key-file` é obrigatório. A chave não é armazenada em settings.json."
        );
        return 1;
    };

    let source_path = match validate_path(source, true) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("ERRO [1]: Origem inválida: {}", error);
            return 1;
        }
    };
    if !source_path.is_dir() {
        eprintln!(
            "ERRO [1]: A origem deve ser um diretório: {}",
            source_path.display()
        );
        return 1;
    }
    if let Ok(destination_existing) = Path::new(destination).canonicalize() {
        if destination_existing == source_path {
            eprintln!("ERRO [1]: Origem e destino não podem ser o mesmo diretório.");
            return 1;
        }
    }

    let key_path = PathBuf::from(key_file);
    // Valide a chave antes de criar qualquer estrutura no destino. Se o
    // arquivo já existe, ele precisa ser válido; se não existe, o diretório
    // pai precisa estar pronto para receber a chave gerada.
    if key_path.exists() {
        if let Err(error) = key::load_key(&key_path) {
            eprintln!("ERRO [1]: Chave inválida: {}", error);
            return 1;
        }
    } else if let Some(parent) = key_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.is_dir() {
            eprintln!(
                "ERRO [1]: Diretório da chave não existe ou não é um diretório: {}",
                parent.display()
            );
            return 1;
        }
    }

    let destination_path = match initialize_repository(destination) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("ERRO [2]: Falha ao inicializar repositório: {}", error);
            return 2;
        }
    };
    if source_path.starts_with(&destination_path) {
        eprintln!(
            "ERRO [1]: A origem não pode ser a raiz do repositório nem estar dentro dele: {}",
            source_path.display()
        );
        return 1;
    }
    let (key, key_path) = match key::ensure_key_file(&key_path) {
        Ok(key) => match key_path.canonicalize() {
            Ok(path) => (key, path),
            Err(error) => {
                eprintln!("ERRO [2]: Falha ao resolver caminho da chave: {}", error);
                return 2;
            }
        },
        Err(error) => {
            eprintln!("ERRO [1]: Chave inválida: {}", error);
            return 1;
        }
    };

    let mut settings = Settings {
        source_path: source_path.to_string_lossy().to_string(),
        destination_path: destination_path.to_string_lossy().to_string(),
        key_file_path: Some(key_path.to_string_lossy().to_string()),
        ..Settings::default()
    };
    if let Some(days) = retention_days {
        settings.retention_days = days;
    }
    if let Some(min) = chunk_min {
        settings.chunk_min_size = min;
    }
    if let Some(max) = chunk_max {
        settings.chunk_max_size = max;
    }

    if let Err(error) = settings.validate() {
        eprintln!("ERRO [1]: Configuração inválida: {}", error);
        return 1;
    }

    // A chave é vinculada antes de alterar settings.json. Em um repositório
    // existente, uma chave diferente é recusada sem sobrescrever a configuração.
    let conn = match open_database(&destination_path) {
        Ok(conn) => conn,
        Err(error) => {
            eprintln!("ERRO [4]: {}", error);
            return 4;
        }
    };
    if let Err(error) = key_binding::bind_repository_key(&conn, &destination_path, &key) {
        eprintln!("ERRO [1]: Chave incompatível: {:#}", error);
        return 1;
    }
    drop(conn);

    if let Err(error) = settings.save(&destination_path) {
        eprintln!("ERRO [2]: Falha ao salvar configuração: {}", error);
        return 2;
    }
    let settings_path = settings_path(&destination_path);
    if let Err(error) = set_file_permissions(&settings_path, 0o600) {
        eprintln!(
            "ERRO [2]: Falha ao restringir permissões da configuração: {}",
            error
        );
        return 2;
    }

    if let Some(dest_dir) = metadata_backup.filter(|v| !v.trim().is_empty()) {
        if let Err(error) =
            crate::metadata_backup::configure(&destination_path, Path::new(dest_dir))
        {
            eprintln!(
                "ERRO [1]: Falha ao configurar a cópia de metadados: {:#}",
                error
            );
            return 1;
        }
        println!(
            "Cópia de metadados configurada em {} (atualizada após cada backup).",
            dest_dir
        );
    }

    if let Err(error) = crate::config::registry::register(&destination_path) {
        eprintln!(
            "Repositório criado, mas cadastro automático falhou: {:#}. Use repository add.",
            error
        );
        return 2;
    }
    println!("Repositório cadastrado para monitoramento automático.");
    println!(
        "Repositório inicializado em: {}",
        destination_path.display()
    );
    println!("Configuração salva em: {}", settings_path.display());
    println!(
        "Chave lida de: {} (não armazenada no JSON)",
        key_path.display()
    );
    println!(
        "Chunking: min={} max={} bytes; retenção: {} dia(s).",
        settings.chunk_min_size, settings.chunk_max_size, settings.retention_days
    );
    0
}

/// Comando `keygen` — cria uma chave AES-256 com permissões restritas.
pub fn cmd_keygen(path: &str) -> i32 {
    if path.trim().is_empty() {
        eprintln!("ERRO [1]: Informe o caminho do arquivo de chave.");
        return 1;
    }
    match key::generate_key_file(Path::new(path)) {
        Ok(()) => {
            println!("Chave AES-256 gerada em {}", path);
            println!("Guarde este arquivo em local seguro e faça backup dele.");
            0
        }
        Err(error) => {
            eprintln!("ERRO [1]: {}", error);
            1
        }
    }
}

/// Comando `backup --manual` — dispara backup manual.
pub fn cmd_backup_manual() -> i32 {
    cmd_backup(None, None, true)
}

pub fn cmd_backup(repo: Option<&str>, key_file: Option<&str>, use_daemon: bool) -> i32 {
    let root = match resolve_repository(repo) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("ERRO [1]: {}", error);
            return 1;
        }
    };

    if use_daemon {
        let socket = socket_path(&root);
        if socket.exists() {
            match client::send_command(&socket, &Request::new("backup")) {
                Ok(response) => {
                    println!("{}", response.message);
                    if let Some(data) = &response.data {
                        if let Some(errors) = data.get("errors").and_then(|value| value.as_array())
                        {
                            for error in errors {
                                let path = error
                                    .get("path")
                                    .and_then(|value| value.as_str())
                                    .unwrap_or("<desconhecido>");
                                let message = error
                                    .get("message")
                                    .and_then(|value| value.as_str())
                                    .unwrap_or("erro sem detalhes");
                                eprintln!("ARQUIVO FALHOU [{}]: {}", path, message);
                            }
                        }
                        if let Some(status) = data.get("status").and_then(|value| value.as_str()) {
                            if !matches!(status, "completed" | "unchanged") {
                                return 4;
                            }
                        }
                        if let Some(redundancy) = data.get("redundancy").filter(|v| !v.is_null()) {
                            println!(
                                "Redundância: {}",
                                redundancy["state"].as_str().unwrap_or("desconhecida")
                            );
                            if let Some(detail) = redundancy["detail"].as_str() {
                                eprintln!("AVISO: redundância: {}", detail);
                            }
                        }
                        if let Some(metadata_backup) =
                            data.get("metadata_backup").filter(|v| !v.is_null())
                        {
                            println!(
                                "Cópia de metadados: {} ({} snapshot(s) coberto(s))",
                                metadata_backup["state"].as_str().unwrap_or("desconhecida"),
                                metadata_backup["covered_snapshots"].as_u64().unwrap_or(0)
                            );
                            if let Some(detail) = metadata_backup["detail"].as_str() {
                                eprintln!("AVISO: cópia de metadados: {}", detail);
                            }
                        }
                    }
                    return if response.is_ok() { 0 } else { 4 };
                }
                Err(error) => {
                    println!(
                        "Daemon indisponível ({}). Executando backup localmente.",
                        error
                    );
                }
            }
        }
    }

    let settings = match Settings::load(&root) {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("ERRO [1]: {}", error);
            return 1;
        }
    };
    let key = match load_key_for(&settings, key_file) {
        Ok(key) => key,
        Err(error) => {
            eprintln!("ERRO [1]: {}", error);
            return 1;
        }
    };

    match backup::run_backup(&root, &settings, &key) {
        Ok(report) => {
            if report.status == "unchanged" {
                println!(
                    "Nenhuma alteração detectada; snapshot {} mantido ({} arquivo(s), {} byte(s)).",
                    report.snapshot_id, report.files_completed, report.bytes_total
                );
            } else {
                println!(
                    "Snapshot {} finalizado com status `{}`: {} arquivo(s) de {} processado(s), {} byte(s).",
                    report.snapshot_id,
                    report.status,
                    report.files_completed,
                    report.files_total,
                    report.bytes_total
                );
                println!(
                    "Chunks: {} novo(s), {} reutilizado(s).",
                    report.chunks_new, report.chunks_reused
                );
            }
            for error in &report.errors {
                eprintln!("ARQUIVO FALHOU [{}]: {}", error.path, error.message);
            }
            if let Some(redundancy) = &report.redundancy {
                println!("Redundância: {}", redundancy.state);
                if let Some(detail) = &redundancy.detail {
                    eprintln!("AVISO: redundância: {}", detail);
                }
            }
            if let Some(metadata_backup) = &report.metadata_backup {
                println!(
                    "Cópia de metadados: {} ({} snapshot(s) coberto(s))",
                    metadata_backup.state, metadata_backup.covered_snapshots
                );
                if let Some(detail) = &metadata_backup.detail {
                    eprintln!("AVISO: cópia de metadados: {}", detail);
                }
            }
            if matches!(report.status.as_str(), "completed" | "unchanged") {
                0
            } else {
                4
            }
        }
        Err(error) => {
            eprintln!("ERRO [{}]: Backup falhou: {}", error_code(&error), error);
            error_code(&error)
        }
    }
}

pub fn cmd_redundancy_configure(repo: Option<&str>, dest: &str, mode: Option<&str>) -> i32 {
    let result = (|| -> Result<()> {
        let mode = match mode {
            Some("best-effort") => crate::redundancy::Mode::BestEffort,
            Some("required") => crate::redundancy::Mode::Required,
            _ => anyhow::bail!("Escolha explicitamente --mode best-effort ou --mode required"),
        };
        let root = resolve_repository(repo)?;
        let settings = Settings::load(&root)?;
        crate::redundancy::configure(&root, &settings, Path::new(dest), mode)?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            println!("Redundância configurada; execute redundancy-sync para copiar snapshots existentes.");
            0
        }
        Err(error) => {
            eprintln!("ERRO: falha ao configurar redundância: {:#}", error);
            1
        }
    }
}

pub fn cmd_redundancy_sync(repo: Option<&str>, key_file: Option<&str>) -> i32 {
    let result = (|| -> Result<usize> {
        let root = resolve_repository(repo)?;
        let settings = Settings::load(&root)?;
        let key = load_key_for(&settings, key_file)?;
        crate::redundancy::sync_repository(&root, &key)
    })();
    match result {
        Ok(count) => {
            println!("Redundância verificada: {} chunk(s).", count);
            0
        }
        Err(error) => {
            eprintln!(
                "ERRO: redundância degradada ou sincronização não iniciada: {:#}",
                error
            );
            4
        }
    }
}

pub fn cmd_metadata_backup_configure(repo: Option<&str>, dest: &str) -> i32 {
    let result = (|| -> Result<()> {
        let root = resolve_repository(repo)?;
        crate::metadata_backup::configure(&root, Path::new(dest))
    })();
    match result {
        Ok(()) => {
            println!("Cópia de metadados configurada; os próximos backups a atualizarão automaticamente. Use metadata-backup para criar ou renovar a cópia agora.");
            0
        }
        Err(error) => {
            eprintln!("ERRO: falha ao configurar cópia de metadados: {:#}", error);
            1
        }
    }
}

pub fn cmd_metadata_backup(repo: Option<&str>, dest: Option<&str>) -> i32 {
    let result = (|| -> Result<crate::metadata_backup::Manifest> {
        let root = resolve_repository(repo)?;
        let settings = Settings::load(&root)?;
        let destination = if let Some(path) = dest {
            // Um único comando configura (se necessário) e cria a cópia.
            let dest_path = PathBuf::from(path);
            crate::metadata_backup::configure(&root, &dest_path)?;
            dest_path
        } else if let Some(path) = &settings.metadata_backup_path {
            PathBuf::from(path)
        } else {
            anyhow::bail!("Cópia de metadados não configurada: use `init --metadata-backup <diretório-vazio>`, `metadata-backup-configure --dest <diretório-vazio>` ou `metadata-backup --dest <diretório-vazio>`");
        };
        crate::metadata_backup::create_backup(&root, &destination)
    })();
    match result {
        Ok(manifest) => {
            println!(
                "Cópia de metadados criada: {} snapshot(s) protegido(s) (último: {}), schema {}, {} chunk(s).",
                manifest.completed_snapshot_ids.len(),
                manifest
                    .last_completed_snapshot_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "nenhum".into()),
                manifest.schema_version,
                manifest.chunk_count
            );
            println!("Guarde a chave original separadamente; ela não faz parte da cópia.");
            0
        }
        Err(error) => {
            eprintln!(
                "ERRO: cópia de metadados falhou (última cópia válida preservada): {:#}",
                error
            );
            4
        }
    }
}

pub fn cmd_metadata_verify(from: &str, repo: Option<&str>) -> i32 {
    let result = (|| -> Result<()> {
        let report = crate::metadata_backup::verify_backup(Path::new(from))?;
        println!(
            "Cópia válida: formato {}, schema {}, {} snapshot(s) coberto(s) (último: {}), {} chunk(s), {} arquivo(s).",
            report.manifest.format_version,
            report.manifest.schema_version,
            report.manifest.completed_snapshot_ids.len(),
            report
                .manifest
                .last_completed_snapshot_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "nenhum".into()),
            report.summary.chunk_count,
            report.summary.file_count
        );
        if let Some(repo) = repo {
            let root = resolve_repository(Some(repo))?;
            let conn = open_database(&root)?;
            let snapshots = list_snapshots(&conn)?;
            let covered: std::collections::HashSet<i64> = report
                .manifest
                .completed_snapshot_ids
                .iter()
                .copied()
                .collect();
            let newer: Vec<i64> = snapshots
                .iter()
                .filter(|s| s.status == "completed" && !covered.contains(&s.id))
                .map(|s| s.id)
                .collect();
            if newer.is_empty() {
                println!("Nenhum snapshot concluído posterior à cópia.");
            } else {
                let ids: Vec<String> = newer.iter().map(|id| id.to_string()).collect();
                println!(
                    "Cópia desatualizada: snapshot(s) {} não está(ão) coberto(s). Execute metadata-backup.",
                    ids.join(", ")
                );
            }
        }
        Ok(())
    })();
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!(
                "ERRO: verificação da cópia de metadados falhou: {:#}",
                error
            );
            4
        }
    }
}

pub fn cmd_metadata_restore(
    from: &str,
    repo: Option<&str>,
    key_file: Option<&str>,
    force: bool,
) -> i32 {
    let result = (|| -> Result<crate::metadata_backup::RestoreReport> {
        // O destino pode ser um diretório novo, portanto resolve_repository
        // (que exige settings.json e index.db) não serve aqui.
        let candidate = if let Some(repo) = repo {
            PathBuf::from(repo)
        } else if let Ok(env_repo) = std::env::var("REDBACKUP_REPO") {
            if env_repo.trim().is_empty() {
                std::env::current_dir()?
            } else {
                PathBuf::from(env_repo)
            }
        } else {
            std::env::current_dir()?
        };
        let target = validate_path(
            candidate
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Caminho do repositório não é UTF-8"))?,
            false,
        )?;
        let key = match key_file.filter(|p| !p.trim().is_empty()) {
            Some(path) => Some(key::load_key(&PathBuf::from(path))?),
            None => {
                if let Ok(settings) = Settings::load(&target) {
                    load_key_for(&settings, None).ok()
                } else {
                    None
                }
            }
        };
        crate::metadata_backup::restore_backup(Path::new(from), &target, force, key.as_ref())
    })();
    match result {
        Ok(report) => {
            println!(
                "Metadados restaurados: {} snapshot(s) coberto(s), {} completo(s).",
                report.covered_snapshots, report.complete_snapshots
            );
            if let Some(path) = &report.preserved_existing {
                println!("Banco anterior preservado em: {}", path.display());
            }
            if report.new_repository {
                println!(
                    "Repositório novo: copie os chunks para data/, reconfigure caminhos com `init` (chave original) e verifique com `recover`/`scrub`."
                );
            }
            if report.needs_init {
                println!("Falta configuração: execute `init` com origem, destino e chave originais antes de operar o repositório.");
            }
            for problem in &report.incomplete_snapshots {
                eprintln!(
                    "SNAPSHOT INCOMPLETO [{}]: {}",
                    problem.snapshot_id,
                    problem.problems.join("; ")
                );
            }
            if report.is_complete() {
                0
            } else {
                eprintln!("Restauração parcial: dados recuperáveis foram preservados, mas nem todos os snapshots cobertos estão completos.");
                4
            }
        }
        Err(error) => {
            eprintln!("ERRO: restauração de metadados falhou: {:#}", error);
            error_code(&error)
        }
    }
}

/// Comando `restore` — restaura arquivo a partir de snapshot.
pub fn cmd_restore(snapshot: &str, file_path: &str, output: &str, force: bool) -> i32 {
    cmd_restore_full(None, None, snapshot, file_path, output, force)
}

pub fn cmd_restore_full(
    repo: Option<&str>,
    key_file: Option<&str>,
    snapshot: &str,
    file_path: &str,
    output: &str,
    force: bool,
) -> i32 {
    if file_path.trim().is_empty() || output.trim().is_empty() {
        eprintln!("ERRO [1]: Caminho de arquivo e saída são obrigatórios para restauração.");
        return 1;
    }
    let snapshot_id: i64 = match snapshot.parse() {
        Ok(id) => id,
        Err(_) => {
            eprintln!("ERRO [1]: Snapshot inválido: {}", snapshot);
            return 1;
        }
    };

    let root = match resolve_repository(repo) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("ERRO [1]: {}", error);
            return 1;
        }
    };
    let settings = match Settings::load(&root) {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("ERRO [1]: {}", error);
            return 1;
        }
    };
    let key = match load_key_for(&settings, key_file) {
        Ok(key) => key,
        Err(error) => {
            eprintln!("ERRO [1]: {}", error);
            return 1;
        }
    };
    let conn = match open_database(&root) {
        Ok(conn) => conn,
        Err(error) => {
            eprintln!("ERRO [4]: {}", error);
            return 4;
        }
    };

    match restore_from_snapshot(
        &root,
        &conn,
        snapshot_id,
        file_path,
        Path::new(output),
        &key,
        force,
    ) {
        Ok(report) => {
            println!(
                "Arquivo restaurado em {} ({} byte(s), {} chunk(s)).",
                report.path.display(),
                report.bytes,
                report.chunks
            );
            0
        }
        Err(error) => {
            eprintln!(
                "ERRO [{}]: Restauração falhou: {}",
                error_code(&error),
                error
            );
            error_code(&error)
        }
    }
}

/// Comando `list-snapshots` — lista snapshots disponíveis.
pub fn cmd_list_snapshots() -> i32 {
    cmd_list_snapshots_full(None)
}

pub fn cmd_list_snapshots_full(repo: Option<&str>) -> i32 {
    let root = match resolve_repository(repo) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("ERRO [1]: {}", error);
            return 1;
        }
    };
    let conn = match open_database(&root) {
        Ok(conn) => conn,
        Err(error) => {
            eprintln!("ERRO [4]: {}", error);
            return 4;
        }
    };
    match list_snapshots(&conn) {
        Ok(snapshots) => {
            print_snapshots(&snapshots);
            0
        }
        Err(error) => {
            eprintln!("ERRO [4]: {}", error);
            4
        }
    }
}

/// Comando `list-files` — lista arquivos de um snapshot.
pub fn cmd_list_files(repo: Option<&str>, snapshot_id: i64) -> i32 {
    let root = match resolve_repository(repo) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("ERRO [1]: {}", error);
            return 1;
        }
    };
    let conn = match open_database(&root) {
        Ok(conn) => conn,
        Err(error) => {
            eprintln!("ERRO [4]: {}", error);
            return 4;
        }
    };
    match list_files(&conn, snapshot_id) {
        Ok(files) => {
            print_files(&files);
            0
        }
        Err(error) => {
            eprintln!("ERRO [4]: {}", error);
            4
        }
    }
}

/// Comando `prune` — realiza pruning de snapshots expirados.
pub fn cmd_prune(force: bool) -> i32 {
    cmd_prune_full(None, force, None, false)
}

pub fn cmd_prune_full(
    repo: Option<&str>,
    force: bool,
    days: Option<u64>,
    allow_protected: bool,
) -> i32 {
    if !force {
        eprintln!("ERRO [1]: Comando destrutivo `prune` requer `--force`.");
        return 1;
    }
    let root = match resolve_repository(repo) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("ERRO [1]: {}", error);
            return 1;
        }
    };
    let settings = match Settings::load(&root) {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("ERRO [1]: {}", error);
            return 1;
        }
    };
    let retention_days = days.unwrap_or(settings.retention_days);
    match retention::prune_repository_with_options(&root, retention_days, allow_protected) {
        Ok(report) => {
            println!(
                "Pruning concluído: {} snapshot(s), {} chunk(s), {} byte(s) liberados.",
                report.snapshots_removed, report.chunks_removed, report.bytes_freed
            );
            if report.deferred {
                println!(
                    "Chunks não removidos porque há snapshot em andamento; execute novamente depois."
                );
            }
            for error in &report.errors {
                eprintln!("AVISO: {}", error);
            }
            for id in &report.protected_removed {
                eprintln!(
                    "AVISO: snapshot {} protegido pela cópia de metadados foi removido com --allow-protected; execute metadata-backup para atualizar a cobertura.",
                    id
                );
            }
            if !report.protected_skipped.is_empty() {
                let ids: Vec<String> = report
                    .protected_skipped
                    .iter()
                    .map(|id| id.to_string())
                    .collect();
                eprintln!(
                    "AVISO: {} snapshot(s) expirado(s) protegido(s) pela cópia de metadados foram mantidos: {}. Atualize a cópia de metadados ou use --allow-protected para removê-los explicitamente.",
                    report.protected_skipped.len(),
                    ids.join(", ")
                );
                return 4;
            }
            if report.errors.is_empty() {
                0
            } else {
                4
            }
        }
        Err(error) => {
            eprintln!("ERRO [{}]: Pruning falhou: {}", error_code(&error), error);
            error_code(&error)
        }
    }
}

/// Comando `scrub` — verifica integridade dos chunks.
pub fn cmd_scrub(repo: Option<&str>, key_file: Option<&str>, sample: Option<usize>) -> i32 {
    let root = match resolve_repository(repo) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("ERRO [1]: {}", error);
            return 1;
        }
    };
    let settings = match Settings::load(&root) {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("ERRO [1]: {}", error);
            return 1;
        }
    };
    let key = match load_key_for(&settings, key_file) {
        Ok(key) => key,
        Err(error) => {
            eprintln!("ERRO [1]: {}", error);
            return 1;
        }
    };
    match scrubbing::scrub_repository(&root, &key, sample) {
        Ok(report) => {
            println!(
                "Scrubbing: {} chunk(s) verificado(s), {} corrompido(s).",
                report.checked,
                report.corrupted.len()
            );
            for problem in &report.corrupted {
                eprintln!("CORRUPTO [{}]: {}", problem.hash_id, problem.reason);
            }
            if report.corrupted.is_empty() {
                0
            } else {
                4
            }
        }
        Err(error) => {
            eprintln!("ERRO [{}]: Scrubbing falhou: {}", error_code(&error), error);
            error_code(&error)
        }
    }
}

/// Comando `recover` — limpa resíduos e snapshots interrompidos.
pub fn cmd_recover(repo: Option<&str>, key_file: Option<&str>) -> i32 {
    let root = match resolve_repository(repo) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("ERRO [1]: {}", error);
            return 1;
        }
    };
    let settings = match Settings::load(&root) {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("ERRO [1]: {}", error);
            return 1;
        }
    };
    let key = match load_key_for(&settings, key_file) {
        Ok(key) => key,
        Err(error) => {
            eprintln!("ERRO [1]: {}", error);
            return 1;
        }
    };
    match recovery::recover_repository(&root, &key) {
        Ok(report) => {
            println!(
                "Recuperação executada: {} arquivo(s) removido(s), {} snapshot(s) marcado(s) como falho(s).",
                report.removed.len(),
                report.stale_snapshots_failed
            );
            for removed in &report.removed {
                println!("  removido: {}", removed);
            }
            for chunk in &report.repaired {
                println!("  chunk reparado: {}", chunk.hash);
                if let Some(path) = &chunk.preserved_original {
                    println!("    original preservado: {}", path.display());
                }
            }
            for problem in &report.unrecoverable {
                eprintln!(
                    "CHUNK NÃO RECUPERÁVEL [{}]: {}",
                    problem.hash, problem.reason
                );
            }
            if report.unrecoverable.is_empty() {
                0
            } else {
                4
            }
        }
        Err(error) => {
            eprintln!(
                "ERRO [{}]: Recuperação falhou: {}",
                error_code(&error),
                error
            );
            error_code(&error)
        }
    }
}

/// Comando `daemon` — inicia o servidor IPC em primeiro plano.
pub fn cmd_daemon(repo: Option<&str>, key_file: Option<&str>) -> i32 {
    if repo.is_none() && std::env::var_os("REDBACKUP_REPO").is_none() {
        if key_file.is_some() {
            eprintln!("--key-file requer --repo no modo explícito");
            return 1;
        }
        return match crate::daemon::manager::run() {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("Daemon: {:#}", error);
                1
            }
        };
    }
    let root = match resolve_repository(repo) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("ERRO [1]: {}", error);
            return 1;
        }
    };
    let settings = match Settings::load(&root) {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("ERRO [1]: {}", error);
            return 1;
        }
    };
    let key = match load_key_for(&settings, key_file) {
        Ok(key) => key,
        Err(error) => {
            eprintln!("ERRO [1]: {}", error);
            return 1;
        }
    };
    match crate::daemon::server::serve(&root, &key) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("ERRO [{}]: Daemon falhou: {}", error_code(&error), error);
            error_code(&error)
        }
    }
}

/// Comando `watch` — monitora a origem e executa backups após debounce.
pub fn cmd_watch(repo: Option<&str>, key_file: Option<&str>, debounce_seconds: u64) -> i32 {
    let root = match resolve_repository(repo) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("ERRO [1]: {}", error);
            return 1;
        }
    };
    let settings = match Settings::load(&root) {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("ERRO [1]: {}", error);
            return 1;
        }
    };
    let key = match load_key_for(&settings, key_file) {
        Ok(key) => key,
        Err(error) => {
            eprintln!("ERRO [1]: {}", error);
            return 1;
        }
    };
    match crate::daemon::watch::run(&root, &settings, &key, debounce_seconds) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("ERRO [{}]: Watcher falhou: {}", error_code(&error), error);
            error_code(&error)
        }
    }
}

fn resolve_repository(repo: Option<&str>) -> Result<PathBuf> {
    let candidate = if let Some(repo) = repo {
        PathBuf::from(repo)
    } else if let Ok(env_repo) = std::env::var("REDBACKUP_REPO") {
        if env_repo.trim().is_empty() {
            std::env::current_dir()?
        } else {
            PathBuf::from(env_repo)
        }
    } else {
        std::env::current_dir()?
    };
    let root = validate_path(
        candidate
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Caminho do repositório não é UTF-8"))?,
        true,
    )?;
    open_repository(&root)
}

fn load_key_for(settings: &Settings, key_file: Option<&str>) -> Result<[u8; 32]> {
    let path = if let Some(path) = key_file.filter(|path| !path.trim().is_empty()) {
        PathBuf::from(path)
    } else if let Some(path) = &settings.key_file_path {
        PathBuf::from(path)
    } else {
        anyhow::bail!(
            "Nenhuma chave configurada. Use `--key-file <caminho>` ou execute `init` novamente."
        );
    };
    key::load_key(&path)
}

fn open_database(root: &Path) -> Result<rusqlite::Connection> {
    let db_path = index_db_path(root);
    let conn = connect_or_create(
        db_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Caminho do banco não é UTF-8"))?,
    )?;
    run_migrations(&conn)?;
    Ok(conn)
}

fn print_snapshots(snapshots: &[SnapshotInfo]) {
    if snapshots.is_empty() {
        println!("Nenhum snapshot encontrado.");
        return;
    }
    println!(
        "{:<6} {:<20} {:<12} {:>8} {:>14}  ORIGEM",
        "ID", "TIMESTAMP", "STATUS", "ARQUIVOS", "BYTES"
    );
    for snapshot in snapshots {
        println!(
            "{:<6} {:<20} {:<12} {:>8} {:>14}  {}",
            snapshot.id,
            snapshot.timestamp,
            snapshot.status,
            snapshot.file_count,
            snapshot.total_bytes,
            snapshot.source_path
        );
    }
}

fn print_files(files: &[FileInfo]) {
    if files.is_empty() {
        println!("Nenhum arquivo registrado neste snapshot.");
        return;
    }
    println!("{:<8} {:>14} {:<12}  CAMINHO", "ID", "BYTES", "PERMISSÕES");
    for file in files {
        let permissions = file
            .permissions
            .map(|value| format!("{:o}", value))
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:<8} {:>14} {:<12}  {}",
            file.id, file.size, permissions, file.path
        );
    }
}

fn error_code(error: &anyhow::Error) -> i32 {
    if error
        .chain()
        .any(|cause| cause.downcast_ref::<std::io::Error>().is_some())
    {
        return 2;
    }
    let text = error.to_string().to_lowercase();
    if text.contains("criptograf")
        || text.contains("autentica")
        || text.contains("aes")
        || text.contains("nonce")
        || text.contains("chave")
    {
        return 3;
    }
    4
}

/// Valida limites de chunking expostos pela CLI antes de chamar o init.
pub fn validate_chunk_options(min: Option<u64>, max: Option<u64>) -> Result<()> {
    if let Some(min) = min {
        if min < CHUNK_MIN_BYTES {
            anyhow::bail!("--min-chunk deve ser pelo menos {}", CHUNK_MIN_BYTES);
        }
        if min > CHUNK_MIN_MAX_BYTES {
            anyhow::bail!("--min-chunk não pode exceder {} bytes", CHUNK_MIN_MAX_BYTES);
        }
    }
    if let Some(max) = max {
        if !(CHUNK_MAX_MIN_BYTES..=CHUNK_MAX_BYTES).contains(&max) {
            anyhow::bail!(
                "--max-chunk deve estar entre {} e {} bytes",
                CHUNK_MAX_MIN_BYTES,
                CHUNK_MAX_BYTES
            );
        }
    }
    if let (Some(min), Some(max)) = (min, max) {
        if min > max {
            anyhow::bail!("--min-chunk não pode ser maior que --max-chunk");
        }
    }
    Ok(())
}

/// Utilitário para comandos que recebem um caminho de configuração.
pub fn configured_repository(path: &Path) -> Result<PathBuf> {
    open_repository(path).context("Repositório inválido")
}

/// Consulta metadados sem exigir chave ou executar migrações.
pub fn cmd_status(repo: Option<&str>, watch: bool) -> i32 {
    loop {
        let result = (|| -> Result<()> {
            let root = resolve_repository(repo)?;
            let data = crate::status::repository_status(&root)?;
            let daemon = daemon_state(&root);
            println!(
                "Repositório: {}\nOrigem: {}\nDaemon: {}",
                root.display(),
                data["source"].as_str().unwrap_or("?"),
                daemon
            );
            println!(
                "Snapshots: {} concluídos; {} falhos; {} em andamento",
                data["snapshots"]["completed"],
                data["snapshots"]["failed"],
                data["snapshots"]["in-progress"]
            );
            if data["last_completed"].is_null() {
                println!("Último backup concluído: nenhum");
            } else {
                println!(
                    "Último backup concluído: snapshot {} em {}",
                    data["last_completed"]["id"],
                    data["last_completed"]["timestamp"].as_str().unwrap_or("?")
                );
            }
            println!(
                "Chunks: {}; corrupção registrada: {} (use scrub para verificar)",
                data["chunks"], data["corruption_recorded"]
            );
            println!(
                "Redundância (última sincronização): {}",
                data["redundancy"]["state"].as_str().unwrap_or("disabled")
            );
            println!(
                "Cópia de metadados: {} ({} snapshot(s) coberto(s))",
                data["metadata_backup"]["state"]
                    .as_str()
                    .unwrap_or("disabled"),
                data["metadata_backup"]["covered_snapshots"]
                    .as_u64()
                    .unwrap_or(0)
            );
            if let Some(detail) = data["metadata_backup"]["detail"].as_str() {
                println!("  detalhe: {}", detail);
            }
            let active = data["backup_active"].as_bool().unwrap_or(false);
            println!(
                "Backup: {}",
                if active {
                    "em execução"
                } else if data["backup_interrupted"].as_bool().unwrap_or(false) {
                    "interrompido; execute recover antes de continuar"
                } else {
                    "inativo"
                }
            );
            if !data["backup"].is_null() {
                let p: crate::status::BackupProgress =
                    serde_json::from_value(data["backup"].clone())?;
                let stage = match p.stage.as_str() {
                    "discovering" => "descobrindo arquivos",
                    "processing" => "processando arquivo",
                    "registering" => "registrando metadados",
                    "verifying" => "verificando referências",
                    "completed" => "concluído",
                    "unchanged" => "sem alterações",
                    "failed" => "falhou",
                    _ => "desconhecida",
                };
                println!(
                    "Etapa: {}; PID: {}; snapshot: {}",
                    stage, p.pid, p.report.snapshot_id
                );
                println!(
                    "Início (Unix): {}; atualização (Unix): {}; duração: {:.1}s",
                    p.started_at, p.updated_at, p.elapsed_seconds
                );
                println!(
                    "Arquivos descobertos: {}; total: {}; concluídos: {}; falhas: {}",
                    p.discovered_files,
                    if p.totals_known {
                        p.report.files_total.to_string()
                    } else {
                        "calculando".into()
                    },
                    p.report.files_completed,
                    p.report.files_failed
                );
                println!(
                    "Arquivo atual: {}; bytes no arquivo: {}",
                    p.current_file.as_deref().unwrap_or("nenhum"),
                    p.current_file_bytes
                );
                println!("Bytes processados: {}; bytes confirmados: {}; chunks novos: {}; reutilizados: {}", p.bytes_processed, p.report.bytes_total, p.chunks_new, p.chunks_reused);
                if let Some(error) = p.error {
                    println!("Erro: {}", error);
                }
                for error in p.report.errors {
                    println!("Falha [{}]: {}", error.path, error.message);
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("Status indisponível: {:#}", error);
            return 1;
        }
        if !watch {
            return 0;
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

fn daemon_state(root: &Path) -> &'static str {
    #[cfg(unix)]
    {
        match client::send_command_with_timeout(
            &socket_path(root),
            &Request::new("ping"),
            std::time::Duration::from_secs(1),
        ) {
            Ok(response) if response.is_ok() => "ativo",
            _ if socket_path(root).exists() => "indisponível (socket presente)",
            _ => "inativo",
        }
    }
    #[cfg(not(unix))]
    {
        let _ = root;
        "não suportado nesta plataforma"
    }
}

pub fn cmd_repository(args: &[String]) -> i32 {
    let result = (|| -> Result<()> {
        let dir = crate::config::registry::directory()?;
        match args {
            [action] if action == "list" => {
                for path in crate::config::registry::list_at(&dir)? {
                    println!("{}", path.display());
                }
            }
            [action, path] if action == "add" || action == "remove" => {
                crate::config::registry::update_at(&dir, Path::new(path), action == "add")?;
                println!("Cadastro atualizado; remoção não apaga backups.");
            }
            _ => anyhow::bail!("Uso: redbackup repository list | add <caminho> | remove <caminho>"),
        }
        Ok(())
    })();
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("Cadastro: {:#}", error);
            1
        }
    }
}
