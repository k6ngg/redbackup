//! Interface CLI: parsing de argumentos e despacho de comandos.
//!
//! A CLI compartilha a mesma engine do daemon. Quando um daemon está ativo no
//! socket do repositório, `backup` pode ser encaminhado por IPC; caso
//! contrário, a operação roda localmente com o mesmo código de domínio.

pub mod commands;

use commands::*;
use std::collections::{HashMap, HashSet};

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn run(args: Vec<String>) -> i32 {
    if args
        .iter()
        .skip(1)
        .any(|argument| argument == "--help" || argument == "-h")
    {
        print_help();
        return 0;
    }
    let command = args.get(1).map(String::as_str).unwrap_or("help");
    match command {
        "repository" => cmd_repository(&args[2..]),
        "init" => match parse_options(&args[2..]) {
            Ok(parsed) => {
                if let Err(error) = parsed.only(&[
                    "source",
                    "dest",
                    "key-file",
                    "retention-days",
                    "min-chunk",
                    "max-chunk",
                ]) {
                    return usage_error(&error);
                }
                let source = match parsed.required("source") {
                    Ok(value) => value,
                    Err(error) => return usage_error(&error),
                };
                let dest = match parsed.required("dest") {
                    Ok(value) => value,
                    Err(error) => return usage_error(&error),
                };
                let key_file = parsed.value("key-file");
                let retention = match parsed.u64("retention-days") {
                    Ok(value) => value,
                    Err(error) => return usage_error(&error),
                };
                let min_chunk = match parsed.u64("min-chunk") {
                    Ok(value) => value,
                    Err(error) => return usage_error(&error),
                };
                let max_chunk = match parsed.u64("max-chunk") {
                    Ok(value) => value,
                    Err(error) => return usage_error(&error),
                };
                if let Err(error) = validate_chunk_options(min_chunk, max_chunk) {
                    return usage_error(&error.to_string());
                }
                cmd_init_full(source, dest, key_file, retention, min_chunk, max_chunk)
            }
            Err(error) => usage_error(&error),
        },
        "keygen" => match parse_options(&args[2..]) {
            Ok(parsed) => {
                if let Err(error) = parsed.only(&["path"]) {
                    return usage_error(&error);
                }
                match parsed.required("path") {
                    Ok(path) => cmd_keygen(path),
                    Err(error) => usage_error(&error),
                }
            }
            Err(error) => usage_error(&error),
        },
        "status" => match parse_options(&args[2..]) {
            Ok(parsed) => {
                if let Err(error) = parsed.only(&["repo", "watch"]) {
                    return usage_error(&error);
                }
                cmd_status(parsed.value("repo"), parsed.flag("watch"))
            }
            Err(error) => usage_error(&error),
        },
        "backup" => match parse_options(&args[2..]) {
            Ok(parsed) => {
                if let Err(error) = parsed.only(&["repo", "key-file", "manual", "no-daemon"]) {
                    return usage_error(&error);
                }
                cmd_backup(
                    parsed.value("repo"),
                    parsed.value("key-file"),
                    !parsed.flag("no-daemon"),
                )
            }
            Err(error) => usage_error(&error),
        },
        "restore" => match parse_options(&args[2..]) {
            Ok(parsed) => {
                if let Err(error) =
                    parsed.only(&["repo", "key-file", "snapshot", "file", "output", "force"])
                {
                    return usage_error(&error);
                }
                let snapshot = match parsed.required("snapshot") {
                    Ok(value) => value,
                    Err(error) => return usage_error(&error),
                };
                let file = match parsed.required("file") {
                    Ok(value) => value,
                    Err(error) => return usage_error(&error),
                };
                let output = match parsed.required("output") {
                    Ok(value) => value,
                    Err(error) => return usage_error(&error),
                };
                cmd_restore_full(
                    parsed.value("repo"),
                    parsed.value("key-file"),
                    snapshot,
                    file,
                    output,
                    parsed.flag("force"),
                )
            }
            Err(error) => usage_error(&error),
        },
        "list-snapshots" => match parse_options(&args[2..]) {
            Ok(parsed) => {
                if let Err(error) = parsed.only(&["repo"]) {
                    return usage_error(&error);
                }
                cmd_list_snapshots_full(parsed.value("repo"))
            }
            Err(error) => usage_error(&error),
        },
        "list-files" => match parse_options(&args[2..]) {
            Ok(parsed) => {
                if let Err(error) = parsed.only(&["repo", "snapshot"]) {
                    return usage_error(&error);
                }
                let snapshot = match parsed.i64_required("snapshot") {
                    Ok(value) => value,
                    Err(error) => return usage_error(&error),
                };
                cmd_list_files(parsed.value("repo"), snapshot)
            }
            Err(error) => usage_error(&error),
        },
        "prune" => match parse_options(&args[2..]) {
            Ok(parsed) => {
                if let Err(error) = parsed.only(&["repo", "days", "force"]) {
                    return usage_error(&error);
                }
                let days = match parsed.u64("days") {
                    Ok(value) => value,
                    Err(error) => return usage_error(&error),
                };
                cmd_prune_full(parsed.value("repo"), parsed.flag("force"), days)
            }
            Err(error) => usage_error(&error),
        },
        "scrub" => match parse_options(&args[2..]) {
            Ok(parsed) => {
                if let Err(error) = parsed.only(&["repo", "key-file", "sample"]) {
                    return usage_error(&error);
                }
                let sample = match parsed.usize("sample") {
                    Ok(value) => value,
                    Err(error) => return usage_error(&error),
                };
                cmd_scrub(parsed.value("repo"), parsed.value("key-file"), sample)
            }
            Err(error) => usage_error(&error),
        },
        "recover" => match parse_options(&args[2..]) {
            Ok(parsed) => {
                if let Err(error) = parsed.only(&["repo", "key-file"]) {
                    return usage_error(&error);
                }
                cmd_recover(parsed.value("repo"), parsed.value("key-file"))
            }
            Err(error) => usage_error(&error),
        },
        "daemon" => match parse_options(&args[2..]) {
            Ok(parsed) => {
                if let Err(error) = parsed.only(&["repo", "key-file"]) {
                    return usage_error(&error);
                }
                cmd_daemon(parsed.value("repo"), parsed.value("key-file"))
            }
            Err(error) => usage_error(&error),
        },
        "watch" => match parse_options(&args[2..]) {
            Ok(parsed) => {
                if let Err(error) = parsed.only(&["repo", "key-file", "debounce"]) {
                    return usage_error(&error);
                }
                let debounce = match parsed.u64("debounce") {
                    Ok(value) => value.unwrap_or(5),
                    Err(error) => return usage_error(&error),
                };
                cmd_watch(parsed.value("repo"), parsed.value("key-file"), debounce)
            }
            Err(error) => usage_error(&error),
        },
        "help" | "--help" | "-h" => {
            print_help();
            0
        }
        "version" | "--version" | "-V" => {
            println!("redbackup {}", VERSION);
            0
        }
        other => {
            eprintln!("Comando desconhecido: {}. Use `redbackup help`.", other);
            1
        }
    }
}

fn usage_error(message: &str) -> i32 {
    eprintln!("ERRO [1]: {}", message);
    eprintln!("Use `redbackup help` para ver o uso.");
    1
}

fn print_help() {
    println!("redbackup {}", VERSION);
    println!("Mecanismo de backup com deduplicação, compressão e criptografia.");
    println!();
    println!("Uso:");
    println!("  redbackup repository list | add <caminho> | remove <caminho>");
    println!("  redbackup init --source <origem> --dest <repositório> --key-file <chave>");
    println!("      [--retention-days N] [--min-chunk N] [--max-chunk N]");
    println!("  redbackup status [--repo <repositório>] [--watch]");
    println!("  redbackup keygen --path <arquivo>");
    println!(
        "  redbackup backup [--repo <repositório>] [--key-file <chave>] [--manual] [--no-daemon]"
    );
    println!("  redbackup restore --snapshot <id> --file <caminho> --output <arquivo>");
    println!("      [--repo <repositório>] [--key-file <chave>] [--force]");
    println!("  redbackup list-snapshots [--repo <repositório>]");
    println!("  redbackup list-files --snapshot <id> [--repo <repositório>]");
    println!("  redbackup prune --force [--days N] [--repo <repositório>]");
    println!("  redbackup scrub [--repo <repositório>] [--key-file <chave>] [--sample N]");
    println!("  redbackup recover [--repo <repositório>] [--key-file <chave>]");
    println!("  redbackup daemon [--repo <repositório>] [--key-file <chave>]");
    println!("  redbackup watch [--repo <repositório>] [--key-file <chave>] [--debounce N]");
    println!();
    println!("O repositório pode ser indicado por --repo, pela variável REDBACKUP_REPO");
    println!("ou pelo diretório atual (que deve conter config/settings.json).");
    println!("A chave nunca é gravada em settings.json; use um arquivo com permissão 0600.");
}

struct Parsed {
    values: HashMap<String, String>,
    flags: HashSet<String>,
    positionals: Vec<String>,
}

impl Parsed {
    fn only(&self, allowed: &[&str]) -> Result<(), String> {
        if !self.positionals.is_empty() {
            return Err(format!(
                "argumento(s) inesperado(s): {}",
                self.positionals.join(" ")
            ));
        }
        for key in self.values.keys() {
            if !allowed.contains(&key.as_str()) {
                return Err(format!("opção desconhecida: --{}", key));
            }
        }
        for key in &self.flags {
            if !allowed.contains(&key.as_str()) {
                return Err(format!("opção desconhecida: --{}", key));
            }
        }
        Ok(())
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    fn required(&self, name: &str) -> Result<&str, String> {
        self.value(name)
            .ok_or_else(|| format!("opção obrigatória ausente: --{}", name))
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.contains(name)
    }

    fn u64(&self, name: &str) -> Result<Option<u64>, String> {
        match self.value(name) {
            Some(value) => parse_size(value)
                .map(Some)
                .map_err(|error| format!("--{}: {}", name, error)),
            None => Ok(None),
        }
    }

    fn usize(&self, name: &str) -> Result<Option<usize>, String> {
        match self.value(name) {
            Some(value) => value
                .parse::<usize>()
                .map(Some)
                .map_err(|_| format!("--{} inválido: {}", name, value)),
            None => Ok(None),
        }
    }

    fn i64_required(&self, name: &str) -> Result<i64, String> {
        let value = self.required(name)?;
        value
            .parse::<i64>()
            .map_err(|_| format!("--{} inválido: {}", name, value))
    }
}

fn parse_options(args: &[String]) -> Result<Parsed, String> {
    const FLAGS: &[&str] = &["force", "manual", "no-daemon", "help", "watch"];
    let mut parsed = Parsed {
        values: HashMap::new(),
        flags: HashSet::new(),
        positionals: Vec::new(),
    };

    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        if let Some(option) = argument.strip_prefix("--") {
            if option.is_empty() {
                return Err("opção vazia".to_string());
            }
            if let Some((name, value)) = option.split_once('=') {
                if name.is_empty() {
                    return Err("opção vazia".to_string());
                }
                parsed.values.insert(name.to_string(), value.to_string());
                index += 1;
                continue;
            }
            if FLAGS.contains(&option) {
                parsed.flags.insert(option.to_string());
                index += 1;
                continue;
            }
            let value = args
                .get(index + 1)
                .ok_or_else(|| format!("--{} requer um valor", option))?;
            parsed.values.insert(option.to_string(), value.clone());
            index += 2;
            continue;
        }
        parsed.positionals.push(argument.clone());
        index += 1;
    }
    Ok(parsed)
}

fn parse_size(value: &str) -> Result<u64, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("valor vazio".to_string());
    }
    let lowercase = trimmed.to_ascii_lowercase();
    let (number, multiplier) = if let Some(number) = lowercase.strip_suffix("kib") {
        (number, 1024u64)
    } else if let Some(number) = lowercase.strip_suffix("mib") {
        (number, 1024u64 * 1024)
    } else if let Some(number) = lowercase.strip_suffix("gib") {
        (number, 1024u64 * 1024 * 1024)
    } else if let Some(number) = lowercase.strip_suffix("kb") {
        (number, 1000u64)
    } else if let Some(number) = lowercase.strip_suffix("mb") {
        (number, 1000u64 * 1000)
    } else if let Some(number) = lowercase.strip_suffix("gb") {
        (number, 1000u64 * 1000 * 1000)
    } else {
        (lowercase.as_str(), 1u64)
    };
    let number: u64 = number
        .trim()
        .parse()
        .map_err(|_| format!("número inválido: {}", value))?;
    number
        .checked_mul(multiplier)
        .ok_or_else(|| format!("valor muito grande: {}", value))
}

#[cfg(test)]
mod tests {
    use super::parse_size;

    #[test]
    fn parse_tamanhos() {
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert_eq!(parse_size("1KiB").unwrap(), 1024);
        assert_eq!(parse_size("2MiB").unwrap(), 2 * 1024 * 1024);
    }
}
