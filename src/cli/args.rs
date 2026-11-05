//! Definições e validação sintática da interface de linha de comando.
use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "redbackup",
    version,
    about = "Mecanismo de backup com deduplicação, compressão e criptografia.",
    after_help = "O repositório é resolvido por --repo, REDBACKUP_REPO ou diretório atual.\nA configuração guarda apenas o caminho da chave, nunca seu conteúdo."
)]
pub(super) struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Args)]
pub(super) struct Repository {
    /// Raiz do repositório
    #[arg(long, value_name = "CAMINHO")]
    pub repo: Option<String>,
}

#[derive(Args)]
pub(super) struct KeyRepository {
    #[command(flatten)]
    pub repository: Repository,
    /// Arquivo da chave (padrão: o configurado)
    #[arg(long, value_name = "ARQUIVO")]
    pub key_file: Option<String>,
}

#[derive(Clone, Copy, ValueEnum)]
pub(super) enum RedundancyMode {
    /// Permite concluir backup principal íntegro se a cópia falhar
    BestEffort,
    /// Exige cópia redundante válida para concluir o snapshot
    Required,
}

#[derive(Subcommand)]
pub(super) enum RepositoryCommand {
    /// Listar repositórios cadastrados
    List,
    /// Cadastrar repositório para monitoramento
    Add { path: String },
    /// Remover do cadastro sem apagar backups
    Remove { path: String },
}

#[derive(Subcommand)]
pub(super) enum Command {
    /// Inicializar repositório, origem e chave
    Init {
        /// Diretório existente a ser salvo
        #[arg(long, value_name = "ORIGEM")]
        source: String,
        /// Raiz do repositório (criada se não existir)
        #[arg(long, value_name = "DESTINO")]
        dest: String,
        /// Arquivo da chave (gerado se não existir)
        #[arg(long, value_name = "ARQUIVO")]
        key_file: String,
        /// Dias mínimos de retenção (padrão: 30)
        #[arg(long)]
        retention_days: Option<u64>,
        /// Tamanho mínimo FastCDC, até 1MiB (padrão: 1MiB)
        #[arg(long, value_parser = parse_size)]
        min_chunk: Option<u64>,
        /// Tamanho máximo FastCDC, até 16MiB (padrão: 4MiB)
        #[arg(long, value_parser = parse_size)]
        max_chunk: Option<u64>,
    },
    /// Gerar chave AES-256 sem sobrescrever arquivo existente
    Keygen {
        #[arg(long)]
        path: String,
    },
    /// Consultar status do repositório e progresso do backup
    Status {
        #[command(flatten)]
        repository: Repository,
        #[arg(long)]
        watch: bool,
    },
    /// Executar backup da origem configurada
    Backup {
        #[command(flatten)]
        repository: KeyRepository,
        #[arg(long, hide = true)]
        manual: bool,
        /// Forçar execução local
        #[arg(long)]
        no_daemon: bool,
    },
    /// Restaurar um arquivo de um snapshot
    Restore {
        #[command(flatten)]
        repository: KeyRepository,
        #[arg(long)]
        snapshot: i64,
        /// Caminho relativo mostrado por list-files
        #[arg(long)]
        file: String,
        #[arg(long)]
        output: String,
        /// Substituir explicitamente destino existente
        #[arg(long)]
        force: bool,
    },
    /// Listar snapshots
    ListSnapshots(Repository),
    /// Listar arquivos de um snapshot
    ListFiles {
        #[command(flatten)]
        repository: Repository,
        #[arg(long)]
        snapshot: i64,
    },
    /// Remover snapshots expirados; exige --force
    Prune {
        #[command(flatten)]
        repository: Repository,
        #[arg(long)]
        days: Option<u64>,
        #[arg(long)]
        force: bool,
    },
    /// Verificar autenticação e hash dos chunks
    Scrub {
        #[command(flatten)]
        repository: KeyRepository,
        #[arg(long)]
        sample: Option<usize>,
    },
    /// Recuperar repositório após interrupção
    Recover(KeyRepository),
    /// Configurar cópia redundante em diretório independente e vazio
    RedundancyConfigure {
        #[command(flatten)]
        repository: Repository,
        #[arg(long)]
        dest: String,
        #[arg(long, value_enum)]
        mode: RedundancyMode,
    },
    /// Sincronizar e verificar cópias redundantes
    RedundancySync(KeyRepository),
    /// Iniciar daemon IPC ou supervisor multi-repositório
    Daemon(KeyRepository),
    /// Monitorar origem e executar backups após alterações
    Watch {
        #[command(flatten)]
        repository: KeyRepository,
        #[arg(long, default_value_t = 5)]
        debounce: u64,
    },
    /// Gerenciar cadastro do supervisor
    Repository {
        #[command(subcommand)]
        command: RepositoryCommand,
    },
    /// Exibir versão
    Version,
}

fn parse_size(value: &str) -> Result<u64, String> {
    let lowercase = value.trim().to_ascii_lowercase();
    let suffixes = [
        ("kib", 1024u64),
        ("mib", 1024 * 1024),
        ("gib", 1024 * 1024 * 1024),
        ("kb", 1000),
        ("mb", 1000 * 1000),
        ("gb", 1000 * 1000 * 1000),
    ];
    let (number, multiplier) = suffixes
        .iter()
        .find_map(|(suffix, multiplier)| lowercase.strip_suffix(suffix).map(|n| (n, *multiplier)))
        .unwrap_or((&lowercase, 1));
    number
        .trim()
        .parse::<u64>()
        .map_err(|_| format!("número inválido: {value}"))?
        .checked_mul(multiplier)
        .ok_or_else(|| format!("valor muito grande: {value}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn definicoes_consistentes() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
    #[test]
    fn tamanhos_e_limites_numericos() {
        assert_eq!(parse_size("1MiB").unwrap(), 1048576);
        assert_eq!(parse_size("2KB").unwrap(), 2000);
        assert!(parse_size("18446744073709551615GiB").is_err());
        assert!(parse_size("inválido").is_err());
    }

    #[test]
    fn formas_validas_e_compatibilidade() {
        for option in [vec!["--repo", "destino"], vec!["--repo=destino"]] {
            let mut args = vec!["redbackup", "backup", "--manual", "--no-daemon"];
            args.extend(option);
            let cli = Cli::try_parse_from(args).unwrap();
            match cli.command.unwrap() {
                Command::Backup {
                    repository,
                    manual,
                    no_daemon,
                } => {
                    assert_eq!(repository.repository.repo.as_deref(), Some("destino"));
                    assert!(manual && no_daemon);
                }
                _ => panic!("comando incorreto"),
            }
        }
        let cli = Cli::try_parse_from([
            "redbackup",
            "init",
            "--source",
            "origem",
            "--dest",
            "destino",
            "--key-file",
            "chave",
            "--min-chunk=1MiB",
            "--max-chunk",
            "4MiB",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Init {
                min_chunk: Some(1048576),
                max_chunk: Some(4194304),
                ..
            })
        ));
        for mode in ["best-effort", "required"] {
            assert!(Cli::try_parse_from([
                "redbackup",
                "redundancy-configure",
                "--dest",
                "destino",
                "--mode",
                mode
            ])
            .is_ok());
        }
    }
}
