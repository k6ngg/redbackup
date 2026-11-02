# redbackup

![Logo redbackup](reddd.png)

Mecanismo de backup desenvolvido em Rust. Cria e restaura snapshots de arquivos usando chunking baseado em conteúdo (`FastCDC`), hashing (`BLAKE3`), deduplicação (`SQLite`), compressão (`Zstandard`), criptografia (`AES-256-GCM`) e metadados (`rusqlite`).

## Manual do Usuário

### 1. Visão Geral

O `redbackup` grava snapshots de um diretório de origem em um repositório de destino. Cada arquivo é dividido em chunks definidos pelo conteúdo; chunks já existentes são reutilizados por hash, sem recomprimir nem recriptografar. Os metadados ficam em SQLite e permitem listar snapshots, listar arquivos, restaurar um arquivo específico, verificar integridade e aplicar retenção.

O motor funciona localmente pela CLI e também pode ser exposto por um daemon local via Unix socket. A CLI usa o daemon quando ele está disponível para `backup` e executa a mesma engine localmente quando não está.

### 2. Instalação e Compilação

Requisitos: Rust (edição 2021+) e as dependências nativas do SQLite/Zstandard resolvidas pelo Cargo.

```bash
cargo build
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

O binário de desenvolvimento fica em `target/debug/redbackup`. Os exemplos abaixo usam `redbackup`; substitua por `./target/debug/redbackup` ou `cargo run --` se preferir.

### 3. Tutorial: fazer backup e restaurar um arquivo

Este exemplo salva os arquivos de `/home/usuario/documentos` no repositório `/home/usuario/backups/redbackup` e depois restaura um documento. Os caminhos seguem o formato Linux: substitua `usuario` pelo seu nome de usuário e adapte as pastas ao seu ambiente. A origem deve existir e o destino deve ser um diretório vazio ou um repositório redbackup existente.

#### Passo 1 — Criar uma chave nova

Crie o diretório que guardará a chave, caso ainda não exista:

```bash
mkdir -p /home/usuario/chaves
```

Gere a chave com o próprio redbackup:

```bash
redbackup keygen --path /home/usuario/chaves/redbackup.key
```

O comando cria um arquivo de 32 bytes aleatórios e aplica permissão `0600` em Unix. **Não crie um arquivo vazio para servir de chave.** O caminho informado deve estar livre: `keygen` recusa sobrescrever qualquer arquivo existente, inclusive um arquivo vazio. Se já houver um arquivo vazio nesse caminho, escolha outro nome para a nova chave e use esse mesmo caminho nas etapas seguintes.

Gere a chave apenas na configuração inicial. Nos próximos backups deste repositório, preserve e use a mesma chave.

#### Passo 2 — Inicializar o repositório

```bash
redbackup init \
  --source /home/usuario/documentos \
  --dest /home/usuario/backups/redbackup \
  --key-file /home/usuario/chaves/redbackup.key
```

`--source` indica a pasta que será salva; `--dest`, onde os chunks e metadados serão armazenados; e `--key-file`, a chave usada na criptografia. O comando grava essas configurações em `/home/usuario/backups/redbackup/config/settings.json`, incluindo o caminho da chave, sem incluir seu conteúdo.

A inicialização prepara o repositório. O snapshot é criado no próximo passo.

#### Passo 3 — Executar o backup

```bash
redbackup backup --repo /home/usuario/backups/redbackup
```

O programa lê a origem configurada e cria um snapshot. Confira o ID informado e o status: `completed` indica conclusão sem erros reportados; `failed` indica que houve falhas e que as mensagens devem ser examinadas.

Para salvar novas versões dos arquivos posteriormente, execute novamente esse comando. Cada execução cria outro snapshot e pode reutilizar chunks já armazenados. Não é necessário repetir `keygen` ou `init`.

#### Passo 4 — Consultar snapshots e arquivos

Liste os snapshots disponíveis:

```bash
redbackup list-snapshots --repo /home/usuario/backups/redbackup
```

Use o ID retornado para consultar os arquivos. Neste exemplo, o snapshot escolhido tem ID `1`; substitua esse número se o ID desejado for outro:

```bash
redbackup list-files --repo /home/usuario/backups/redbackup --snapshot 1
```

Copie o caminho relativo do arquivo exatamente como aparece nessa listagem. Nomes com espaços devem ficar entre aspas no comando de restauração.

#### Passo 5 — Restaurar um arquivo

Supondo que a listagem contenha `relatorio.pdf`, execute:

```bash
redbackup restore \
  --repo /home/usuario/backups/redbackup \
  --snapshot 1 \
  --file "relatorio.pdf" \
  --output /tmp/relatorio-restaurado.pdf
```

O comando reconstrói o arquivo e valida os chunks antes de publicar a saída em `/tmp/relatorio-restaurado.pdf`. Abra o arquivo restaurado para conferir o resultado. Se o destino já existir, escolha outro caminho ou use `--force` quando quiser substituí-lo explicitamente.

O caminho da chave é recuperado da configuração, por isso não precisa ser repetido nos comandos `backup` e `restore` deste exemplo.

#### Passo 6 — Verificar os chunks armazenados

```bash
redbackup scrub --repo /home/usuario/backups/redbackup
```

Esse comando verifica autenticação, descompressão e hash dos chunks e reporta problemas encontrados. Ele não repara automaticamente dados corrompidos.

**Guarde `/home/usuario/chaves/redbackup.key` separadamente do repositório e mantenha uma cópia segura dela. Sem a chave original, os backups não podem ser restaurados.**

O repositório pode ser indicado de três formas, nesta ordem de precedência:

1. `--repo <diretório>` na linha de comando;
2. variável de ambiente `REDBACKUP_REPO`;
3. diretório atual, que deve ser a raiz do repositório (conter `config/settings.json`).

### 4. Comandos

#### `init`

```bash
redbackup init --source <origem> --dest <repositório> --key-file <arquivo> \
  [--retention-days N] [--min-chunk N] [--max-chunk N]
```

- `--source`: diretório existente a ser salvo (obrigatório).
- `--dest`: raiz do repositório (obrigatório). É criada se não existir.
- `--key-file`: arquivo da chave (obrigatório). Se não existir, uma chave aleatória de 32 bytes é gerada com permissão `0600`. Diretórios, arquivos com formato inválido e caminhos sem permissão são recusados.
- `--retention-days`: dias mínimos antes de um snapshot poder ser removido pelo `prune` (padrão `30`).
- `--min-chunk` / `--max-chunk`: limites do FastCDC. Aceitam bytes ou sufixos `KiB`, `MiB`, `KB`, `MB`, `GB`. O mínimo deve ficar entre 64 bytes e 1 MiB; o máximo, entre 1 KiB e 16 MiB.

`init` é idempotente para a estrutura do repositório: não apaga chunks nem metadados existentes. Ele recusa repositórios fisicamente inconsistentes, por exemplo `metadata/index.db` sem `data/` ou `data/` com arquivos sem banco. Um destino não vazio que não pareça um repositório redbackup também é recusado, para não misturar o repositório com dados do usuário (como uma pasta de imagens); use um diretório vazio ou dedicado. A chave é validada antes da criação da estrutura do repositório, evitando efeitos colaterais quando o arquivo de chave é inválido.

O arquivo `config/settings.json` guarda apenas caminhos e parâmetros; **a chave nunca é gravada nele**.

#### `keygen`

```bash
redbackup keygen --path <arquivo>
```

Gera 32 bytes aleatórios, recusa sobrescrever um arquivo existente e aplica permissão `0600` em Unix. Sem a chave, os chunks não podem ser restaurados; guarde-a separadamente do repositório e faça backup dela.

#### `backup`

```bash
redbackup backup [--repo <repositório>] [--key-file <chave>] [--manual] [--no-daemon]
```

Percorre a origem recursivamente, processa cada arquivo em fluxo, grava chunks novos de forma atômica e registra o snapshot. Arquivos com erro são reportados individualmente; o snapshot só recebe status `completed` se todos os arquivos e a verificação final de integridade passarem. Caso contrário, recebe `failed`, mas os dados já gravados permanecem recuperáveis.

`--manual` é aceito por compatibilidade e não altera o comportamento. `--no-daemon` força a execução local mesmo que exista um daemon no repositório.

#### `restore`

```bash
redbackup restore --snapshot <id> --file <caminho_relativo> --output <arquivo|diretório> \
  [--repo <repositório>] [--key-file <chave>] [--force]
```

- `--file` é o caminho relativo mostrado por `list-files`.
- Se `--output` for um diretório existente, o nome do arquivo original é acrescentado.
- Sem `--force`, um destino existente é recusado. Com `--force`, ele é substituído.
- O arquivo é montado em um temporário no diretório de destino, validado chunk a chunk (autenticação, descompressão, tamanho e BLAKE3) e só então publicado com rename atômico.
- Permissões Unix e `modification_time` (precisão de segundos) são restauradas quando disponíveis.

#### `list-snapshots` e `list-files`

```bash
redbackup list-snapshots [--repo <repositório>]
redbackup list-files --snapshot <id> [--repo <repositório>]
```

Listam, respectivamente, snapshots (ID, data, status, arquivos, bytes e origem) e arquivos de um snapshot (ID, bytes, permissões e caminho relativo).

#### `prune`

```bash
redbackup prune --force [--days N] [--repo <repositório>]
```

Comando destrutivo: exige `--force`. Remove snapshots `completed`/`failed` mais antigos que `--days` (ou `retention_days` da configuração), em uma transação, e recalcula `reference_count` a partir de `file_chunks`. Só depois remove arquivos físicos de chunks sem nenhuma referência. Se houver snapshot `in-progress`, a remoção de chunks é adiada para não competir com um backup ativo.

#### `scrub`

```bash
redbackup scrub [--repo <repositório>] [--key-file <chave>] [--sample N]
```

Lê cada chunk (ou uma amostra aleatória de `N`), descriptografa, descomprime e recalcula BLAKE3. Corrupção é registrada nas colunas `corrupted_at`/`corruption_reason` da tabela `chunks`, reportada no terminal e **não apaga nem altera os dados**. O comando retorna código `4` se encontrar corrupção.

#### `recover`

```bash
redbackup recover [--repo <repositório>]
```

Remove arquivos temporários `.tmp_*`, marca snapshots deixados em `in-progress` como `failed` e remove chunks físicos sem referência quando não há backup em andamento. Executar quando nenhum backup estiver ativo. É seguro repetir a operação.

#### `daemon`

```bash
redbackup daemon [--repo <repositório>] [--key-file <chave>]
```

Inicia o servidor IPC em `<repositório>/metadata/redbackup.sock` (Unix socket com permissão `0600`, sem exposição de rede). O protocolo é uma linha JSON por conexão:

```json
{"command":"ping"}
{"command":"status"}
{"command":"backup"}
{"command":"list-snapshots"}
{"command":"prune","args":{"days":30}}
{"command":"scrub"}
{"command":"shutdown"}
```

O atendimento é sequencial: um backup em execução bloqueia novas conexões até terminar. `shutdown` encerra o daemon de forma controlada e remove o socket. Em plataformas sem Unix socket, o daemon não está disponível e a CLI deve ser usada localmente.

#### `watch`

```bash
redbackup watch [--repo <repositório>] [--key-file <chave>] [--debounce N]
```

Monitora a origem com `notify`. Eventos de criação, modificação e remoção são enfileirados; após `N` segundos sem novos eventos (padrão `5`), um backup completo é executado. O callback do watcher não faz I/O pesado. A fila tem limite de 10.000 eventos; ao atingi-lo, o backup é disparado imediatamente e a fila é consolidada. Sem eventos, não há backup periódico.

### 5. Estrutura do Repositório

```text
<destino>/
├── data/                       # chunks criptografados (0o750)
│   └── {2 primeiros chars do hash}/
│       └── {hash BLAKE3}
├── metadata/                   # metadados restritos (0o700)
│   ├── index.db                # SQLite (WAL)
│   ├── redbackup.lock          # lock cooperativo entre processos
│   └── redbackup.sock          # daemon, quando ativo
└── config/                     # configuração (0o750)
    └── settings.json           # sem a chave
```

O banco usa `foreign_keys = ON`, `journal_mode = WAL`, `synchronous = FULL` e `busy_timeout`. Além de `index.db`, o SQLite pode manter `index.db-wal` e `index.db-shm` durante operações normais.

### 6. Segurança

- AES-256-GCM com nonce aleatório de 12 bytes para cada operação de criptografia.
- Formato persistido do chunk: `[versão: 1 byte][nonce: 12 bytes][ciphertext + tag]`.
- A descriptografia falha de forma segura se a autenticação, o formato ou a versão estiverem inválidos.
- A chave é lida de arquivo separado, nunca de `settings.json`, e o conteúdo não é exibido em logs.
- Em Unix, chaves e configurações usam `0600`; `metadata/` usa `0700`; `data/` e `config/` usam `0750`.
- Caminhos de chunks são derivados de hash BLAKE3 validado e não podem escapar de `data/`.
- Caminhos restaurados são relativos e validados contra `..`, caminhos absolutos e separadores inconsistentes.
- O daemon usa socket local com permissão `0600`; não há listener de rede.
- Um lock cooperativo (`flock`/`LockFileEx` via `fs2`) impede backup, pruning e recuperação simultâneos; scrubbing e restauração usam lock compartilhado.

### 7. Integridade e Recuperação

- O pipeline é: arquivo → chunk FastCDC → BLAKE3 → consulta de deduplicação → `zstd` → AES-256-GCM → temporário + rename atômico → metadados em transação → verificação final → snapshot `completed`.
- Chunks são gravados antes de qualquer metadado que os referencie. Um chunk só passa a ser referenciado dentro de uma transação SQLite curta.
- `reference_count` é o número de linhas em `file_chunks`; o `prune` recalcula esse valor a partir da fonte de verdade em vez de confiar em decrementos cegos.
- Arquivos alterados durante a leitura (tamanho ou `mtime`) são rejeitados para evitar metadados inconsistentes.
- `recover` remove apenas `.tmp_*` e chunks sem referência; nunca apaga um chunk referenciado.
- `scrub` registra corrupção sem apagar dados. A recuperação de um chunk corrompido depende de outra cópia/snapshot válido ou de backup externo.

### 8. Códigos de Saída

| Código | Significado |
|---|---|
| `0` | Sucesso |
| `1` | Erro de uso/configuração (argumentos, chave, repositório inválido) |
| `2` | Erro de I/O |
| `3` | Erro de criptografia/autenticação |
| `4` | Erro de integridade/engine, snapshot com falhas, corrupção detectada |

### 9. Portabilidade e Limitações Atuais

- Linux e macOS: CLI, backup, restauração, daemon e watch funcionam com Unix socket.
- Windows: o motor de backup/restauração usa caminhos normalizados para `/` e publicação atômica via `tempfile`; o daemon IPC por Unix socket não está implementado. A integração VSS é prevista no backlog, mas **não está implementada**; arquivos em uso podem falhar e são reportados individualmente.
- Symlinks não são seguidos nem incluídos no backup. Arquivos com caminho não UTF-8 são reportados e ignorados. Nomes contendo `\` literal em Unix são rejeitados para manter compatibilidade de restauração entre plataformas.
- A origem não pode ser a raiz do repositório nem estar dentro dele; essa configuração é recusada por `init` e pelo motor de backup.
- `mtime` é persistido com precisão de segundos. Permissões são restauradas em Unix; no Windows, o campo pode ser omitido.
- Backup, pruning e recuperação são serializados por um lock exclusivo de repositório; scrubbing e restauração usam lock compartilhado. As gravações de chunks continuam atômicas e as transações SQLite são independentes do lock de arquivo.
- Rotação, derivação e recuperação de chaves não estão implementadas. Perder a chave significa perder a capacidade de restaurar.
- GUI e empacotamento multiplataforma não fazem parte desta versão.

### 10. Arquitetura

Um crate único (`redbackup`) com módulos internos tratados como futuras crates:

- `config/`: `settings.json`, validação de caminhos, permissões e chave.
- `repository/`: construção centralizada de caminhos e inicialização do repositório.
- `metadata/`: schema, migrações versionadas e CRUD SQLite.
- `pipeline/`: chunking, hashing, compressão, criptografia e escrita atômica.
- `backup/`: orquestração do backup, deduplicação e registro de metadados.
- `restoration/`: reconstrução e validação de arquivos.
- `retention/`: pruning e remoção de chunks órfãos.
- `scrubbing/`: verificação de integridade e diagnóstico persistido.
- `recovery/`: recuperação após interrupção.
- `daemon/`: protocolo, cliente, servidor IPC, watcher e fila de eventos.
- `cli/`: parsing de argumentos e comandos.

### 11. Status de Implementação

| Área | Status |
|---|---|
| Configuração, chave e inicialização | ✅ Implementado |
| SQLite, migrações e metadados | ✅ Implementado |
| Pipeline de backup com deduplicação | ✅ Implementado |
| Restauração com validação e metadados | ✅ Implementado |
| Pruning e recuperação | ✅ Implementado |
| Scrubbing com diagnóstico persistido | ✅ Implementado |
| CLI completa | ✅ Implementado |
| Daemon IPC e watcher (Unix) | ✅ Implementado |
| Daemon IPC no Windows | ❌ Não implementado |
| VSS no Windows | ❌ Não implementado |
| GUI | ❌ Não implementada |
| Empacotamento multiplataforma | ❌ Não implementado |

## Tecnologias Principais

- `Rust` (daemon/engine)
- `SQLite` (`metadata/index.db`) com `rusqlite`
- `notify` (watcher de arquivos)
- `fastcdc` 5 (chunking baseado em conteúdo, com streaming)
- `blake3` (hashing)
- `zstd` (compressão)
- `aes-gcm` (AES-256-GCM com nonce único)
- `tempfile` (arquivos temporários e publicação atômica)
- `filetime` (restauração de `mtime`)
- `fs2` (lock cooperativo de repositório entre processos)
- JSON (`config/settings.json`)

## Verificações

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Os testes incluem integração ponta a ponta com backup, deduplicação, restauração byte a byte, arquivos vazios, nomes Unicode, permissões/`mtime`, pruning, scrubbing com corrupção e recuperação de snapshots interrompidos.

## Decisões Técnicas Documentadas

- **Um crate único**: escolhido para o MVP; módulos internos podem ser extraídos para `crates/` no futuro.
- **IPC nativo**: `UnixListener` (Linux/macOS); protocolo de uma linha JSON por conexão.
- **Fallback local**: se o daemon não estiver disponível, a CLI executa a mesma engine localmente, sem duplicar lógica de domínio.
- **Chave criptográfica**: não armazenada em `settings.json`; apenas o caminho do arquivo é persistido.
- **Formato do chunk**: `[versão: 1 byte][nonce: 12 bytes][ciphertext + tag]`.
- **Escrita atômica**: temporário no mesmo filesystem + `persist`/rename; no Windows, `tempfile` usa substituição atômica.
- **Migrações**: schema versionado em `schema_version`; a v2 adiciona o diagnóstico de corrupção.
- **VSS (Windows)**: previsto, não implementado nesta versão.
- **GUI**: opcional; não implementada.
