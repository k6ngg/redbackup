//! redbackup — mecanismo de backup com chunking, hashing, deduplicação, compressão, criptografia e metadados SQLite.
//!
//! Arquitetura: um crate único, módulos internos tratados como futuras crates.
//! Prioridade máxima: integridade dos dados (pipeline atômico, chunks temporários + rename atômico).

use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();
    let code = if args.len() < 2 {
        redbackup::cli::commands::cmd_init("", "", None)
    } else {
        match args[1].as_str() {
            "init" => {
                let mut source = String::new();
                let mut destination = String::new();
                let mut key_file = None;
                let mut i = 2;
                while i < args.len() {
                    match args[i].as_str() {
                        "--source" => {
                            i += 1;
                            if i < args.len() {
                                source = args[i].clone();
                            }
                        }
                        "--dest" => {
                            i += 1;
                            if i < args.len() {
                                destination = args[i].clone();
                            }
                        }
                        "--key-file" => {
                            i += 1;
                            if i < args.len() {
                                key_file = Some(args[i].clone());
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                redbackup::cli::commands::cmd_init(&source, &destination, key_file.as_deref())
            }
            "backup" => {
                if args.get(2).map(|s| s.as_str()) == Some("--manual") {
                    redbackup::cli::commands::cmd_backup_manual()
                } else {
                    eprintln!("Uso: redbackup backup --manual");
                    1
                }
            }
            "restore" => {
                let mut snapshot = String::new();
                let mut file_path = String::new();
                let mut output = String::new();
                let mut force = false;
                let mut i = 2;
                while i < args.len() {
                    match args[i].as_str() {
                        "--snapshot" => {
                            i += 1;
                            if i < args.len() {
                                snapshot = args[i].clone();
                            }
                        }
                        "--file" => {
                            i += 1;
                            if i < args.len() {
                                file_path = args[i].clone();
                            }
                        }
                        "--output" => {
                            i += 1;
                            if i < args.len() {
                                output = args[i].clone();
                            }
                        }
                        "--force" => {
                            force = true;
                        }
                        _ => {}
                    }
                    i += 1;
                }
                redbackup::cli::commands::cmd_restore(&snapshot, &file_path, &output, force)
            }
            "list-snapshots" => redbackup::cli::commands::cmd_list_snapshots(),
            "prune" => {
                let mut force = false;
                let mut i = 2;
                while i < args.len() {
                    if args[i].as_str() == "--force" {
                        force = true;
                    }
                    i += 1;
                }
                redbackup::cli::commands::cmd_prune(force)
            }
            "help" | "--help" | "-h" => {
                println!("redbackup v0.1.0");
                println!("Uso:");
                println!("  init --source <path> --dest <path> --key-file <file>");
                println!("  backup --manual");
                println!("  restore --snapshot <id> --file <path> --output <path> [--force]");
                println!("  list-snapshots");
                println!("  prune --force");
                0
            }
            _ => {
                eprintln!(
                    "Comando desconhecido: {}. Use 'redbackup help' para ajuda.",
                    args[1]
                );
                1
            }
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
