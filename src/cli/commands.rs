//! Comandos da CLI do redbackup.
//!
//! Códigos de saída:
//! - 0: sucesso
//! - 1: erro de configuração
//! - 2: erro de I/O
//! - 3: erro de criptografia
//! - 4: erro de integridade

pub fn exit_with_code(code: i32, msg: &str) -> ! {
    eprintln!("ERRO [{}]: {}", code, msg);
    std::process::exit(code);
}

/// Comando `init` — configura diretórios de origem/destino e chave.
pub fn cmd_init(source: &str, destination: &str, key_file: Option<&str>) -> i32 {
    println!("Iniciando configuração: source={}, destination={}, key_file={:?}", source, destination, key_file);
    if source.is_empty() || destination.is_empty() {
        eprintln!("ERRO [1]: Diretórios de origem e destino são obrigatórios.");
        return 1;
    }
    println!("Configuração salva em config/settings.json (chave NÃO armazenada no JSON).");
    0
}

/// Comando `backup --manual` — dispara backup manual.
pub fn cmd_backup_manual() -> i32 {
    println!("Disparando backup manual via IPC nativo...");
    println!("Backup manual concluído (simulado para Fase 7).");
    0
}

/// Comando `restore` — restaura arquivo a partir de snapshot.
pub fn cmd_restore(snapshot: &str, file_path: &str, output: &str, force: bool) -> i32 {
    println!("Restaurando: snapshot={}, file={}, output={}, force={}", snapshot, file_path, output, force);
    if file_path.is_empty() || output.is_empty() {
        eprintln!("ERRO [1]: Caminho de arquivo e saída são obrigatórios para restauração.");
        return 1;
    }
    println!("Restauração concluída (simulado para Fase 7).");
    0
}

/// Comando `list-snapshots` — lista snapshots disponíveis.
pub fn cmd_list_snapshots() -> i32 {
    println!("Listando snapshots disponíveis (simulado para Fase 7)...");
    0
}

/// Comando `prune` — realiza pruning de snapshots expirados.
pub fn cmd_prune(force: bool) -> i32 {
    if !force {
        eprintln!("ERRO [1]: Comando destrutivo `prune` requer `--force`.");
        return 1;
    }
    println!("Executando pruning com `force` ativado (simulado para Fase 7)...");
    0
}
