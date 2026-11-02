# redbackup

![Logo redbackup](reddd.png)

Mecanismo de backup desenvolvido em Rust. Cria e restaura snapshots de arquivos usando chunking baseado em conteúdo (`FastCDC`), hashing (`BLAKE3`), deduplicação (`SQLite`), compressão (`Zstandard`), criptografia (`AES-256-GCM`) e metadados (`rusqlite`).

## Manual do Usuário

### 1. Visão Geral

O `redbackup` é um mecanismo de backup desenvolvido principalmente em Rust. Seu objetivo é criar e restaurar snapshots de arquivos usando chunking baseado em conteúdo (`FastCDC`), hashing (`BLAKE3`), deduplicação (`SQLite`), compressão (`Zstandard`), criptografia (`AES-256-GCM`) e metadados (`rusqlite`).

### 2. Instalação e Compilação

Requisitos: Rust (edição 2021+), `sqlite3` (incluído via `libsqlite3-sys`).

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

### 3. Configuração Inicial (`init`)

```bash
redbackup init --source <origem> --dest <destino> --key-file <caminho_chave>
```

- `--source`: obrigatório
- `--dest`: obrigatório
- `--key-file`: arquivo com chave AES-256 (`32 bytes`), permissões `0o600`; **nunca armazenada em `settings.json`**

O arquivo `config/settings.json` contém apenas configurações não secretas (`source_path`, `destination_path`, `chunk_min_size`, `chunk_max_size`, `retention_days`).

### 4. Estrutura do Repositório

```
<destino>/
├── data/                 (chunks, permissões 0o750)
│   └── {prefixo_2_chars}/
│       └── {hash}
├── metadata/             (SQLite, permissões 0o700)
│   ├── index.db
│   └── migrations/
└── config/               (configurações, permissões 0o750)
    └── settings.json
```

### 5. Operações Básicas

- `redbackup backup --manual`
- `redbackup restore --snapshot <id> --file <caminho> --output <destino>` (nunca sobrescreve sem `--force`; valida `path traversal`)
- `redbackup list-snapshots`
- `redbackup prune --force` (exige `--force`; usa transações ACID; só remove chunks com `reference_count == 0`)

### 6. Segurança

- `AES-256-GCM` com `nonce` único por operação (`rand`)
- Formato persistido: `[version: 1 byte][nonce: 12 bytes][ciphertext + tag]`
- Chave lida exclusivamente via `--key-file`
- `validate_path` centraliza construção de caminhos e previne `path traversal`

### 7. Integridade e Recuperação

- Pipeline: arquivo → chunk (`FastCDC`) → `BLAKE3` → dedup (`SQLite`) → `zstd` → `AES-256-GCM` → `.tmp` → `rename` atômico → `snapshots` (`completed` só após todos os chunks confirmados)
- `crash recovery`: na inicialização, remove `.tmp_` não referenciados no banco (`metadata/index.db`)
- `scrubbing`: rotina agendada lê chunks aleatórios, recalcula `BLAKE3`; se falhar, registra sem expor segredos; **não apaga automaticamente**
- `pruning`: remove `snapshots`/`files`/`file_chunks` expirados; só exclui chunks físicos quando `reference_count == 0`

### 8. Daemon e Monitoramento

- `watcher` (`notify`): eventos (`Create`, `Modify`, `Remove`) encaminhados para fila interna
- `queue`: limite configurável (`VecDeque`); se atingir limite, consolida eventos e loga
- `server`: `UnixListener` (Linux/macOS); protocolo `JSON` simples; não exposto a interfaces de rede públicas
- `shutdown`: controlado (`SIGTERM`/`SIGINT`); finaliza operações pendentes

### 9. Restauração Granular

- Localiza `snapshots` e `files` por `snapshot_id`
- Recupera `file_chunks` (`order_index` ASC)
- Lê chunks físicos (`/data/{prefixo_2_chars}/{hash}`), valida existência
- Descriptografa (`AES-256-GCM`) e descomprime (`zstd`), valida `hash_id`
- Monta arquivo temporário (`.restored_tmp_*`), valida resultado, move atômico para destino final
- Preserva `permissions` e `modification_time` quando suportado

### 10. Portabilidade e Manutenção

- Plataformas: `Linux`, `macOS`, `Windows`
- `VSS`: apenas no Windows, com fallback para leitura direta
- Módulos: `pipeline/`, `metadata/`, `daemon/`, `cli/`, `restoration/`, `recovery/`, `retention/`, `scrubbing/`
- `crates/` futuros: migração simples sem mudança de comportamento

## Status de Implementação

| Fase | Área | Status |
|---|---|---|
| 0 | Setup (estrutura, dependências) | ✅ Completo |
| 1 | Configuração e Repositório | ✅ Completo |
| 2 | SQLite / Metadados | ✅ Completo |
| 3 | Pipeline de Backup | ✅ Completo |
| 4 | Daemon / Watcher / IPC | ✅ Completo |
| 5 | Restauração | ✅ Completo |
| 6 | Resiliência / Manutenção | ✅ Completo |
| 7 | CLI (comandos básicos) | ✅ Completo |
| 8 | Testes finais / Empacotamento | ✅ Completo |

## Tecnologias Principais

- `Rust` (daemon/engine)
- `SQLite` (`metadata/index.db`) com `rusqlite`
- `notify` (watcher de arquivos)
- `fastcdc` (chunking baseado em conteúdo)
- `blake3` (hashing)
- `zstd` (compressão)
- `aes-gcm` (criptografia AES-256-GCM com nonce único)
- JSON (`config/settings.json`)

## Arquitetura

Um crate único (`redbackup`) com módulos internos tratados como futuras crates:

- `config/`: configuração (`settings.json`, validação de caminhos, permissões)
- `metadata/`: SQLite (`snapshots`, `files`, `chunks`, `file_chunks`)
- `pipeline/`: leitura, chunking, hashing, deduplicação, compressão, criptografia, escrita atômica
- `daemon/`: watcher (`notify`), fila de eventos (`queue`), servidor IPC (`UnixListener`)
- `cli/`: interface de linha de comando (`init`, `backup`, `restore`, `list-snapshots`, `prune`)
- `restoration/`: reconstrução de arquivos a partir de chunks
- `recovery/`: limpeza de resíduos (`.tmp`) após falha
- `retention/`: pruning e cálculo de `reference_count`
- `scrubbing/`: verificação de integridade (`BLAKE3` sobre chunks)

## Verificações

Todos os testes passam e `cargo clippy -- -D warnings` está limpo:

```bash
cargo fmt --all -- --check
cargo check
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

## Decisões Técnicas Documentadas

- **Um crate único**: escolhido para o MVP; módulos internos podem ser extraídos para `crates/` no futuro.
- **IPC nativo**: `UnixListener` (Linux/macOS); protocolo simples (`JSON` sobre socket).
- **Chave criptográfica**: NÃO armazenada em `settings.json`; lida via arquivo (`--key-file`) com permissões `0o600`.
- **Formato do chunk**: `[version: 1 byte][nonce: 12 bytes][ciphertext + tag]`.
- **Escrita atômica**: arquivo `.tmp` + `rename` no mesmo filesystem.
- **VSS (Windows)**: apenas no Windows, com fallback para leitura direta; expansão futura considerada.
- **GUI**: opcional; não implementada no MVP.
