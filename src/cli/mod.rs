//! Parsing declarativo e despacho para os comandos compartilhados com a engine.
mod args;
pub mod commands;

use args::{Cli, Command, RedundancyMode, RepositoryCommand};
use clap::{CommandFactory, FromArgMatches};
use commands::*;

fn help_in_portuguese(command: clap::Command) -> clap::Command {
    let command = if command.get_version().is_some() {
        command.disable_version_flag(true).arg(
            clap::Arg::new("version")
                .long("version")
                .short('V')
                .action(clap::ArgAction::Version)
                .help("Imprimir versão"),
        )
    } else {
        command
    };
    command
        .disable_help_flag(true)
        .arg(
            clap::Arg::new("help")
                .long("help")
                .short('h')
                .action(clap::ArgAction::Help)
                .help("Imprimir ajuda"),
        )
        .help_template("{about-with-newline}\nUso: {usage}\n\n{all-args}{after-help}")
        .subcommand_help_heading("Comandos")
        .mut_args(|arg| {
            if arg.is_positional() {
                arg.help_heading("Argumentos")
            } else {
                arg.help_heading("Opções")
            }
        })
        .mut_subcommands(help_in_portuguese)
}

pub fn run(args: Vec<String>) -> i32 {
    let parsed = help_in_portuguese(Cli::command())
        .try_get_matches_from(args)
        .and_then(|matches| Cli::from_arg_matches(&matches));
    let cli = match parsed {
        Ok(cli) => cli,
        Err(error) => {
            // clap usa 2 para erros de uso; o contrato público do redbackup usa 1.
            let code = i32::from(error.use_stderr());
            if let Err(io_error) = error.print() {
                eprintln!("Falha ao imprimir mensagem da CLI: {io_error}");
                return 2;
            }
            return code;
        }
    };
    let Some(command) = cli.command else {
        if let Err(error) = help_in_portuguese(Cli::command()).print_help() {
            eprintln!("Falha ao imprimir ajuda: {error}");
            return 2;
        }
        println!();
        return 0;
    };
    match command {
        Command::Init {
            source,
            dest,
            key_file,
            retention_days,
            min_chunk,
            max_chunk,
            metadata_backup,
        } => {
            if let Err(error) = validate_chunk_options(min_chunk, max_chunk) {
                eprintln!("ERRO [1]: {error}");
                return 1;
            }
            cmd_init_full(
                &source,
                &dest,
                Some(&key_file),
                retention_days,
                min_chunk,
                max_chunk,
                metadata_backup.as_deref(),
            )
        }
        Command::Keygen { path } => cmd_keygen(&path),
        Command::Status { repository, watch } => cmd_status(repository.repo.as_deref(), watch),
        Command::Backup {
            repository,
            no_daemon,
            ..
        } => cmd_backup(
            repository.repository.repo.as_deref(),
            repository.key_file.as_deref(),
            !no_daemon,
        ),
        Command::Restore {
            repository,
            snapshot,
            file,
            output,
            force,
        } => cmd_restore_full(
            repository.repository.repo.as_deref(),
            repository.key_file.as_deref(),
            &snapshot.to_string(),
            &file,
            &output,
            force,
        ),
        Command::ListSnapshots(repository) => cmd_list_snapshots_full(repository.repo.as_deref()),
        Command::ListFiles {
            repository,
            snapshot,
        } => cmd_list_files(repository.repo.as_deref(), snapshot),
        Command::Prune {
            repository,
            days,
            force,
            allow_protected,
        } => cmd_prune_full(repository.repo.as_deref(), force, days, allow_protected),
        Command::Scrub { repository, sample } => cmd_scrub(
            repository.repository.repo.as_deref(),
            repository.key_file.as_deref(),
            sample,
        ),
        Command::Recover(repository) => cmd_recover(
            repository.repository.repo.as_deref(),
            repository.key_file.as_deref(),
        ),
        Command::RedundancyConfigure {
            repository,
            dest,
            mode,
        } => cmd_redundancy_configure(
            repository.repo.as_deref(),
            &dest,
            Some(match mode {
                RedundancyMode::BestEffort => "best-effort",
                RedundancyMode::Required => "required",
            }),
        ),
        Command::RedundancySync(repository) => cmd_redundancy_sync(
            repository.repository.repo.as_deref(),
            repository.key_file.as_deref(),
        ),
        Command::MetadataBackupConfigure { repository, dest } => {
            cmd_metadata_backup_configure(repository.repo.as_deref(), &dest)
        }
        Command::MetadataBackup { repository, dest } => {
            cmd_metadata_backup(repository.repo.as_deref(), dest.as_deref())
        }
        Command::MetadataVerify { from, repository } => {
            cmd_metadata_verify(&from, repository.repo.as_deref())
        }
        Command::MetadataRestore {
            from,
            repository,
            force,
        } => cmd_metadata_restore(
            &from,
            repository.repository.repo.as_deref(),
            repository.key_file.as_deref(),
            force,
        ),
        Command::Daemon(repository) => cmd_daemon(
            repository.repository.repo.as_deref(),
            repository.key_file.as_deref(),
        ),
        Command::Watch {
            repository,
            debounce,
        } => cmd_watch(
            repository.repository.repo.as_deref(),
            repository.key_file.as_deref(),
            debounce,
        ),
        Command::Repository { command } => cmd_repository(&match command {
            RepositoryCommand::List => vec!["list".into()],
            RepositoryCommand::Add { path } => vec!["add".into(), path],
            RepositoryCommand::Remove { path } => vec!["remove".into(), path],
        }),
        Command::Version => {
            println!("redbackup {}", env!("CARGO_PKG_VERSION"));
            0
        }
    }
}
