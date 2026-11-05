# redbackup

![Logo redbackup](reddd.png)

Mecanismo de backup desenvolvido em Rust. Cria e restaura snapshots de arquivos usando chunking baseado em conteúdo (`FastCDC`), hashing (`BLAKE3`), deduplicação (`SQLite`), compressão (`Zstandard`), criptografia (`AES-256-GCM`) e metadados (`rusqlite`).

## Manual do Usuário

### 1. Visão Geral

O `redbackup` grava snapshots de um diretório de origem em um repositório de destino. Cada arquivo é dividido em chunks definidos pelo conteúdo; chunks já existentes são reutilizados por hash, sem recomprimir nem recriptografar, mas somente depois de autenticados, descomprimidos e terem o BLAKE3 recalculado. Os metadados ficam em SQLite e permitem listar snapshots, listar arquivos, restaurar um arquivo específico, verificar integridade e aplicar retenção.

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

`init` é idempotente para a estrutura do repositório: não apaga chunks nem metadados existentes. Ele recusa repositórios fisicamente inconsistentes, por exemplo `metadata/index.db` sem `data/` ou `data/` com arquivos sem banco. Um destino não vazio que não pareça um repositório redbackup também é recusado, para não misturar o repositório com dados do usuário (como uma pasta de imagens); use um diretório vazio ou dedicado. A chave é validada antes da criação da estrutura do repositório, evitando efeitos colaterais quando o arquivo de chave é inválido. `init` também vincula a chave ao repositório; repetir o comando com uma chave diferente é recusado sem alterar `settings.json` nem os dados existentes.

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

Percorre a origem recursivamente, processa cada arquivo em fluxo, grava chunks novos de forma atômica e registra o snapshot. Antes de qualquer gravação, a chave é comparada com a vinculação do repositório; uma chave diferente é recusada sem criar snapshot ou chunk. Antes de reutilizar um chunk existente, o backup o autentica, descomprime e recalcula seu BLAKE3; se a validação falhar, o chunk não é reutilizado, a corrupção é registrada em `chunks` e o arquivo falha. Arquivos com erro são reportados individualmente; o snapshot só recebe status `completed` se todos os arquivos e a verificação final de integridade passarem. Caso contrário, recebe `failed`, mas os dados já gravados permanecem recuperáveis.

Se o conteúdo da origem for idêntico ao do último snapshot concluído, o snapshot em andamento é descartado e o resultado é `unchanged`: nenhum registro redundante é publicado. A comparação usa caminhos, tamanhos, permissões e a sequência de hashes de chunks; alterações apenas de `mtime` não geram um novo snapshot.

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
redbackup recover [--repo <repositório>] [--key-file <chave>]
```

Sob bloqueio exclusivo, verifica referências, tamanhos e ordem dos chunks dos snapshots concluídos, autentica e descomprime os chunks referenciados e confere seu BLAKE3 e tamanho. Quando existe redundância configurada, tenta reparar chunks ausentes ou inválidos de snapshots concluídos **ou falhos** usando as cópias redundantes. Cada cópia é autenticada com a chave do repositório, descomprimida e validada quanto ao tamanho e hash antes de ser gravada e novamente validada em arquivo temporário; apenas então é publicada atomicamente. Antes de substituir um arquivo corrompido, preserva uma cópia em `metadata/quarantine/`, sem exclusão automática. O comando informa os hashes reparados e o caminho dos originais preservados.

Após o reparo, revalida os snapshots concluídos; marca como `failed` os que ainda apresentarem inconsistências e os snapshots interrompidos, e recalcula as contagens de referências em uma transação SQLite. **Snapshots já marcados como `failed` não são promovidos automaticamente**, mesmo se seus chunks forem reparados, pois podem ter falhado por arquivos que nunca foram incluídos. Depois remove temporários `.tmp_*` e chunks físicos sem referências. A operação pode ser repetida; um banco ausente impede a recuperação. Se uma cópia válida não estiver disponível, os dados originais são preservados, o problema é reportado por hash e o comando retorna código `4`.

A chave original é obrigatória: usa-se o arquivo configurado ou `--key-file`. Uma chave diferente da vinculada é recusada antes de qualquer alteração. Repositórios antigos sem verificador de chave e com chunks referenciados são recusados, pois não é possível distinguir com segurança uma chave incorreta de dados corrompidos; nesse caso, preserve os dados e valide a chave antes de vincular o repositório. A redundância precisa estar configurada, disponível e conter uma cópia compatível; sem ela não é possível reconstruir os dados. No Windows, a durabilidade do rename após queda de energia não é garantida pela implementação atual.

#### Redundância de chunks

A redundância é opcional e desativada por padrão. Prepare um diretório existente e vazio, preferencialmente em outro disco, e configure-o:

```bash
redbackup redundancy-configure --repo <repositório> --dest <diretório-vazio> --mode best-effort
redbackup redundancy-sync --repo <repositório> [--key-file <chave>]
```

O primeiro comando associa o destino exclusivamente ao repositório; o segundo valida e copia os chunks dos snapshots já concluídos. Os backups seguintes também sincronizam automaticamente antes da finalização. A opção `--mode` é obrigatória: escolha `best-effort` para permitir que um snapshot principal íntegro seja concluído mesmo se a cópia falhar (nesse caso a redundância fica `degraded`), ou `required` para marcar o snapshot como falho quando a sincronização não puder ser concluída. Corrija o problema e execute `redundancy-sync` ou tente o backup novamente. A exigência de escolha explícita aplica-se a novas configurações; configurações existentes mantêm seu modo.

O `status` apresenta `disabled`, `pending`, `protected`, `degraded` ou `unavailable`. `protected` significa que a **última sincronização** verificou as cópias dos chunks dos snapshots concluídos; não garante que o destino permaneceu íntegro desde então. Execute `redundancy-sync` novamente para verificar. O destino usa um identificador próprio, recusando o uso acidental de outro diretório. Diretórios sobrepostos à origem ou ao repositório, bem como destinos inicialmente não vazios, são recusados. Em Unix há um aviso quando ambos aparentam estar no mesmo dispositivo; isso não detecta todos os domínios de falha compartilhados.

As cópias são arquivos cifrados com a mesma chave e são autenticadas, descomprimidas e verificadas por tamanho e BLAKE3. Uma cópia já existente mas inválida não é sobrescrita; ela é preservada para diagnóstico e a sincronização falha. Nenhum `prune` remove cópias redundantes nesta versão, portanto o espaço ocupado pode crescer. A configuração não pode ser substituída automaticamente. Uma interrupção durante a configuração inicial pode deixar o marcador de identificação no destino antes da configuração ser salva; nesse caso, examine o diretório antes de qualquer limpeza manual.

**Limites:** a redundância guarda chunks, não uma cópia independente de `metadata/index.db` ou da configuração. Guarde separadamente um backup consistente dos metadados e da chave. O comando `recover` pode usar a redundância configurada para reparar chunks, mas não recria dados que não estejam disponíveis. No Windows, a durabilidade do rename após queda de energia não é garantida. A sincronização percorre e verifica todos os chunks referenciados pelos snapshots concluídos, podendo ser demorada.

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

O atendimento permite até 16 conexões simultâneas: consultas podem ser atendidas durante um backup. `shutdown` encerra o daemon de forma controlada e remove o socket. Em plataformas sem Unix socket, o daemon não está disponível e a CLI deve ser usada localmente.

#### `watch`

```bash
redbackup watch [--repo <repositório>] [--key-file <chave>] [--debounce N]
```

Monitora a origem com `notify`. Eventos de criação, modificação e remoção são enfileirados; após `N` segundos sem novos eventos (padrão `5`), um backup completo é executado. O callback do watcher não faz I/O pesado. As notificações são consolidadas em memória constante. Há uma varredura inicial; depois, alterações disparam backups com debounce e espera máxima de 60 segundos sob atividade contínua.

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
- O repositório guarda em `metadata/index.db` apenas um verificador derivado da chave com BLAKE3 em modo keyed. A chave não é persistida e o verificador não permite recuperá-la.
- Backup, restauração, scrubbing e daemon exigem que a chave corresponda ao verificador. `init` e o primeiro backup de um repositório antigo criam a vinculação; na migração, todos os chunks existentes são autenticados com a chave fornecida e qualquer divergência impede a vinculação.
- Em Unix, chaves e configurações usam `0600`; `metadata/` usa `0700`; `data/` e `config/` usam `0750`.
- Caminhos de chunks são derivados de hash BLAKE3 validado e não podem escapar de `data/`.
- Caminhos restaurados são relativos e validados contra `..`, caminhos absolutos e separadores inconsistentes.
- O daemon usa socket local com permissão `0600`; não há listener de rede.
- Um lock cooperativo (`flock`/`LockFileEx` via `fs2`) impede backup, pruning e recuperação simultâneos; scrubbing e restauração usam lock compartilhado.

### 7. Integridade e Recuperação

- O pipeline é: arquivo → chunk FastCDC → BLAKE3 → consulta de deduplicação → validação do chunk existente → `zstd` → AES-256-GCM → temporário + rename atômico → metadados em transação → verificação final → snapshot `completed`.
- Um chunk só é considerado reutilizável depois de lido, autenticado com AES-GCM, descomprimido e ter o BLAKE3 recalculado. A presença do arquivo no disco não basta; corrupção detectada no backup é registrada em `chunks` e torna o snapshot `failed`.
- Chunks são gravados antes de qualquer metadado que os referencie. Um chunk só passa a ser referenciado dentro de uma transação SQLite curta.
- Em Unix, a publicação sincroniza o arquivo e os diretórios ancestrais; falhas de abertura ou sincronização impedem a conclusão do backup. Um chunk que permaneça no disco após uma falha é sincronizado novamente antes de ser reutilizado. Em plataformas não Unix, a biblioteca padrão não confirma a durabilidade do rename de diretório após queda de energia.
- `reference_count` é o número de linhas em `file_chunks`; o `prune` recalcula esse valor a partir da fonte de verdade em vez de confiar em decrementos cegos.
- Arquivos alterados durante a leitura (tamanho ou `mtime`) são rejeitados para evitar metadados inconsistentes.
- `recover` remove apenas `.tmp_*` e chunks sem referência; nunca apaga um chunk referenciado.
- `scrub` registra corrupção sem apagar dados. A recuperação de um chunk corrompido depende de outra cópia/snapshot válido ou de backup externo.
- Uma chave incompatível não altera diagnósticos: backup, restauração, scrubbing e daemon falham antes de gravar chunks, publicar snapshots ou marcar corrupção.

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
- Rotação, derivação e recuperação de chaves não estão implementadas. Perder a chave significa perder a capacidade de restaurar; a chave vinculada ao repositório não pode ser trocada automaticamente.
- GUI e empacotamento multiplataforma não fazem parte desta versão.

### 10. Arquitetura

Um crate único (`redbackup`) com módulos internos tratados como futuras crates:

- `config/`: `settings.json`, validação de caminhos, permissões e chave.
- `repository/`: construção centralizada de caminhos, inicialização e vinculação da chave ao repositório.
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

Os testes incluem integração ponta a ponta com backup, deduplicação, restauração byte a byte, arquivos vazios, nomes Unicode, permissões/`mtime`, pruning, scrubbing com corrupção, rejeição de chunks corrompidos na deduplicação, vinculação e recusa de chaves incompatíveis e recuperação de snapshots interrompidos.

## Decisões Técnicas Documentadas

- **Um crate único**: escolhido para o MVP; módulos internos podem ser extraídos para `crates/` no futuro.
- **IPC nativo**: `UnixListener` (Linux/macOS); protocolo de uma linha JSON por conexão.
- **Fallback local**: se o daemon não estiver disponível, a CLI executa a mesma engine localmente, sem duplicar lógica de domínio.
- **Chave criptográfica**: não armazenada em `settings.json`; apenas o caminho do arquivo é persistido. O banco guarda um verificador BLAKE3 keyed que vincula a chave ao repositório.
- **Formato do chunk**: `[versão: 1 byte][nonce: 12 bytes][ciphertext + tag]`.
- **Escrita atômica**: temporário no mesmo filesystem + `persist`/rename; no Windows, `tempfile` usa substituição atômica.
- **Migrações**: schema versionado em `schema_version`; a v2 adiciona o diagnóstico de corrupção e a v3 cria a vinculação da chave.
- **VSS (Windows)**: previsto, não implementado nesta versão.
- **GUI**: opcional; não implementada.

### Consultar o status do repositório

```bash
redbackup status --repo /home/usuario/backups/redbackup
redbackup status --repo /home/usuario/backups/redbackup --watch
```

A consulta não exige chave: mostra origem, snapshots por estado, último backup
concluído, chunks e corrupção já registrada. `--watch` repete a consulta a cada
2 segundos; use Ctrl+C para sair. Não executa migrações nem substitui `scrub`.

O daemon é consultado por `ping`, com timeout de leitura/escrita de 1 segundo.
Um socket antigo é mostrado como indisponível, sem afirmar que o serviço está ativo.
O daemon atende até 16 conexões simultâneas e aguarda os atendimentos terminarem
ao receber `shutdown`. Os locks continuam impedindo escritas simultâneas.

O backup publica progresso tanto em execução local quanto pelo daemon:

- `stage`: `discovering`, `processing`, `registering`, `verifying`, `completed` ou `failed`.
- `started_at` e `updated_at`: segundos Unix; `elapsed_seconds`: duração.
- `current_file` e `current_file_bytes`: arquivo atual e bytes processados nele.
- `bytes_processed`, `chunks_new` e `chunks_reused`: trabalho realizado, incluindo
  arquivos que eventualmente falhem; `report` contém os resultados confirmados.
- `discovered_files` e `totals_known`: descoberta de arquivos; o total só fica
  disponível depois da varredura. Não há estimativa de tempo restante.

O progresso é atualizado entre chunks, com publicação periódica limitada a uma
vez a cada 250 ms, além das mudanças de arquivo e etapas principais. Uma operação
de I/O demorada pode atrasar atualizações. Os registros ficam em
`metadata/backup-progress.json`; um lock próprio em `backup-activity.lock`
confirma atividade sem depender da validade do PID. Se o lock foi liberado e o
registro não tem estado final, o status indica interrupção e sugere `recover`.
O status não repara nem remove registros. Erros ao publicar telemetria são
reportados como avisos e não alteram o resultado do backup.

O comando IPC `status` retorna o mesmo resumo em `data`, incluindo
`backup_active`, `backup_interrupted` e `backup`. Não requer a chave do cliente.
O progresso detalhado refere-se ao backup; outras operações preservam os locks
existentes, mas não publicam os mesmos contadores.

### Executar como serviço Linux (`redbackupd`)

O executável `redbackupd` atende IPC e monitora a origem configurada. Ele realiza
uma varredura inicial após 5 segundos e cria novos snapshots após alterações,
com debounce de 5 segundos. Atividade contínua dispara uma varredura após até
60 segundos. Falhas deixam o trabalho pendente para nova tentativa. Eventos do
próprio repositório são excluídos e notificações são consolidadas em memória
constante. Uma operação já em execução pode prolongar esses intervalos. Quando o
evento não altera o conteúdo, a varredura é executada mas o snapshot em
andamento é descartado (`unchanged`), mantendo o último snapshot concluído.

Instale os dois executáveis e a unidade do usuário:

```bash
cargo install --path . --force --locked
mkdir -p "$HOME/.config/systemd/user"
install -m644 packaging/systemd/redbackupd.service "$HOME/.config/systemd/user/redbackupd.service"
systemctl --user daemon-reload
systemctl --user enable --now redbackupd.service
```

A unidade usa `~/.cargo/bin/redbackupd`. Se você usa outro diretório de instalação,
ajuste `ExecStart`. Para atualizar uma unidade antiga, copie novamente o arquivo,
execute `daemon-reload` e `systemctl --user restart redbackupd.service`.
`service.env` deixou de ser necessário.

Sem argumentos, `redbackupd` supervisiona os repositórios cadastrados em
`$XDG_CONFIG_HOME/redbackup/repositories.json` ou, por padrão,
`~/.config/redbackup/repositories.json`. Sem cadastro, permanece ativo aguardando
configuração. Não procura repositórios pelo disco e não gera chaves.

Após um `redbackup init` bem-sucedido, o destino é cadastrado automaticamente para
monitoramento. O supervisor consulta o cadastro a cada 2 segundos e inicia os
repositórios cadastrados, sem precisar reiniciar o serviço. Cada repositório usa
sua própria configuração e chave. Para cadastrar um repositório existente:

```bash
redbackup repository add /home/usuario/backups/redbackup
redbackup repository list
```

Todos os destinos cadastrados são selecionados para monitoramento. Para parar de
monitorar um deles, preservando seus arquivos e snapshots:

```bash
redbackup repository remove /home/usuario/backups/redbackup
```

A remoção aguarda operações já aceitas terminarem. Destinos desconectados,
configurações inválidas e chaves ausentes são reportados nos logs e tentados
novamente a cada 10 segundos. Um cadastro JSON inválido não substitui o arquivo
nem encerra os serviços já ativos. O cadastro usa escrita atômica e lock para
preservar registros em inicializações concorrentes.

`--repo` e a variável `REDBACKUP_REPO` mantêm o modo explícito de um único
repositório. Para o modo automático, deixe essa variável ausente no ambiente do
serviço. O comando IPC `shutdown` encerra um worker; enquanto estiver cadastrado,
o supervisor poderá iniciá-lo novamente. Use `repository remove` para desativá-lo
ou `systemctl --user stop redbackupd.service` para parar todos.

```bash
systemctl --user status redbackupd.service
journalctl --user -u redbackupd.service -f
```

Para consultar os backups e controlar o serviço:

```bash
redbackup status --repo /home/usuario/backups/redbackup --watch
systemctl --user stop redbackupd.service
systemctl --user restart redbackupd.service
```

O serviço é ativado pelo gerenciador do usuário. Para mantê-lo disponível sem
sessão aberta, configure `loginctl enable-linger usuario` conforme as permissões
da máquina. A origem, o destino e a chave precisam estar acessíveis nesse contexto.

`SIGTERM`, `SIGINT` e o comando IPC `shutdown` solicitam parada controlada:
o daemon recusa novas tarefas e aguarda as operações aceitas terminarem. A unidade
usa `TimeoutStopSec=infinity` para não interromper um backup por timeout; uma
operação de I/O travada pode, portanto, prolongar a parada indefinidamente.
Reinícios após falha têm intervalo de 10 segundos. Encerramentos abruptos podem
exigir `recover`; a inicialização não apaga resíduos automaticamente.

Uma instância por repositório é garantida por `metadata/daemon.lock`. O comando
`redbackup daemon --repo ...` permanece como alternativa compatível e usa o mesmo
serviço, incluindo monitoramento. `redbackup watch` continua disponível para
monitoramento separado; evite executá-lo junto do daemon para a mesma origem.
Para execução manual use `redbackupd --repo /caminho/do/repositorio`.

## Pacotes Linux para validação local

Gere cada pacote em uma VM limpa da distribuição alvo com acesso ao registro
do Cargo. Os binários são compilados a partir do `Cargo.lock` e os pacotes
instalam uma unit de usuário em conjunto com os dois executáveis em
`/usr/bin`. Os comandos abaixo partem da raiz do checkout. Os diretórios
passados a `prepare.sh` devem ser novos.

**Debian 13 (amd64)**

```bash
sudo apt install build-essential dpkg-dev debhelper cargo rustc pkgconf libsqlite3-dev
work=$(mktemp -d)
source_dir=$(./packaging/prepare.sh deb "$work/deb")
(cd "$source_dir" && dpkg-buildpackage -b -us -uc)
dpkg-deb --info "$work"/deb/redbackup_*_amd64.deb
dpkg-deb --contents "$work"/deb/redbackup_*_amd64.deb
sudo apt install "$work"/deb/redbackup_*_amd64.deb
```

**openSUSE Tumbleweed (x86_64)**

```bash
sudo zypper install rpm-build cargo rust gcc pkg-config sqlite3-devel systemd-devel systemd-rpm-macros
work=$(mktemp -d)
version=$(./packaging/prepare.sh rpm "$work/rpm")
rpmbuild -bb --define "_topdir $work/rpm" \
    --define "redbackup_version $version" packaging/rpm/redbackup.spec
rpm -qpl "$work"/rpm/RPMS/x86_64/redbackup-"$version"-*.rpm
rpm -qpR "$work"/rpm/RPMS/x86_64/redbackup-"$version"-*.rpm
sudo zypper install "$work"/rpm/RPMS/x86_64/redbackup-"$version"-*.rpm
```

**Ativação por conta**

Se estiver migrando de uma instalação via Cargo, pare a instância antiga
com `systemctl --user disable --now redbackupd.service`. Mova a unit manual
`~/.config/systemd/user/redbackupd.service` para um caminho de backup ainda
livre fora desse diretório. Uma unit local ou um override de `ExecStart`
pode ter precedência sobre a versão empacotada.

Como o usuário que executará os backups, rode:

```bash
systemctl --user daemon-reload
systemctl --user enable --now redbackupd.service
systemctl --user show -p FragmentPath -p ExecStart -p MainPID redbackupd.service
```

Após atualizar o pacote, execute `systemctl --user daemon-reload` e
`systemctl --user restart redbackupd.service` em cada conta que já usa o
serviço. Antes de remover o pacote, execute
`systemctl --user disable --now redbackupd.service` nessas contas. A
configuração, as chaves e os repositórios continuam sob controle do usuário.

Estas receitas são para builds locais. Defina a licença do projeto antes
de publicar os pacotes.
