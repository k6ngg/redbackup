# AGENTS.md — redbackup

## 1. Missão do projeto

O redbackup é um mecanismo de backup desenvolvido principalmente em Rust. Seu objetivo é criar e restaurar snapshots de arquivos usando chunking baseado em conteúdo, hashing, deduplicação, compressão, criptografia e metadados armazenados em SQLite.

Este arquivo orienta agentes de programação que implementam funcionalidades, corrigem defeitos, escrevem testes ou refatoram o projeto. Use-o junto ao backlog do projeto; não presuma que uma tarefa foi concluída apenas porque está descrita aqui.

## 2. Fonte de verdade e limites

- Use o código existente, os testes, as decisões documentadas e o backlog como fontes de verdade.
- Antes de alterar código, inspecione a estrutura do repositório, `Cargo.toml`, documentação e testes existentes.
- O backlog define estas áreas de trabalho: configuração/infraestrutura, SQLite/metadados, pipeline de backup, daemon/monitoramento, restauração, resiliência/manutenção, CLI/GUI e testes/empacotamento.
- Não invente requisitos de produto, comandos de CLI, esquemas de banco ou APIs públicas sem necessidade. Se uma decisão relevante não estiver especificada, escolha a alternativa mais simples e segura, documente a decisão e destaque a suposição na entrega.
- Não marque itens do backlog como concluídos sem verificar a implementação e os testes correspondentes.
- Preserve compatibilidade com o comportamento existente, salvo quando a tarefa pedir explicitamente uma mudança incompatível.

## 3. Tecnologias e componentes previstos

Tecnologias indicadas no backlog:

- **Rust** para o daemon/engine.
- **SQLite**, com o banco em `metadata/index.db`, para metadados.
- **`rusqlite`** para acesso ao SQLite.
- **`notify`** para observar alterações no sistema de arquivos.
- **FastCDC** (`fastcdc`) para dividir arquivos em chunks definidos pelo conteúdo, com parâmetros configuráveis; o backlog cita uma faixa de exemplo de 1 MiB a 4 MiB.
- **BLAKE3** (`blake3`) para calcular hashes dos chunks.
- **Zstandard** (`zstd`) para comprimir chunks únicos.
- **AES-256-GCM**, usando uma implementação adequada como `aes-gcm`, para criptografar os dados comprimidos. ChaCha20-Poly1305 é uma alternativa mencionada no backlog, mas não troque o algoritmo sem uma decisão explícita.
- **JSON** em `config/settings.json` para configurações.
- **VSS no Windows** como integração prevista para lidar com arquivos em uso; implemente com isolamento por plataforma e fallback seguro.
- Comunicação local entre daemon e cliente por IPC, gRPC ou REST local. O protocolo ainda precisa ser escolhido/documentado se não houver decisão no código.
- CLI como interface inicial prevista. GUI com Tauri ou Iced é opcional.

Não adicione dependências apenas por conveniência. Verifique manutenção, compatibilidade, impacto na compilação e necessidade real antes de introduzir uma crate.

## 4. Organização do armazenamento

A estrutura de repositório prevista é:

```text
<destino>/
├── data/
│   └── <prefixo_do_hash>/<hash>
├── metadata/
│   └── index.db
└── config/
    └── settings.json
```

- Trate o diretório de destino como um repositório de backup, não como uma pasta temporária.
- Centralize a construção dos caminhos; evite concatenar caminhos manualmente.
- Valide caminhos e entradas externas antes de usá-los em operações de leitura, escrita ou exclusão.
- Não apague nem sobrescreva dados existentes durante a inicialização sem uma verificação explícita.
- Não presuma que um diretório, arquivo ou registro exista: trate erros e estados parciais.
- Se o layout for alterado, documente a migração e preserve a capacidade de localizar os dados já gravados.

## 5. Regras de arquitetura e qualidade

- Mantenha responsabilidades separadas: configuração, armazenamento, metadados, pipeline de backup, restauração, daemon/watcher e interface do cliente.
- Prefira módulos pequenos com interfaces claras a um único módulo que concentre toda a lógica.
- Mantenha a lógica de domínio separada de detalhes de CLI, sistema operacional e transporte de comunicação.
- Use tipos e erros explícitos. Evite `unwrap()` e `expect()` em caminhos de execução que possam receber dados, arquivos, configurações ou estados externos; reserve-os para invariantes realmente garantidas e justificadas.
- Não silencie erros com `let _ = ...` quando isso puder esconder perda de dados, corrupção ou falha de sincronização.
- Não introduza `unsafe` sem necessidade demonstrável, justificativa documentada e revisão cuidadosa.
- Evite duplicação de lógica de hashing, codificação, validação de caminhos e manipulação de metadados.
- Prefira operações idempotentes quando forem apropriadas para inicialização, recuperação e repetição de tarefas.
- Não faça refatorações amplas sem relação com a tarefa solicitada.
- Documente invariantes não óbvias, formatos persistidos e decisões de compatibilidade.

## 6. Integridade e consistência dos backups

A integridade dos dados é prioridade máxima.

- Um snapshot só pode ser marcado como concluído depois que todos os chunks necessários estiverem gravados com sucesso e seus metadados estiverem consistentes.
- Não publique metadados que apontem para chunks inexistentes ou incompletos.
- Grave novos chunks por meio de uma estratégia segura para evitar que leitores encontrem arquivos parcialmente escritos. Sempre que possível, escreva em arquivo temporário no mesmo sistema de arquivos, finalize e valide o conteúdo e então faça a renomeação atômica.
- Coordene gravações em disco e transações SQLite com um fluxo de recuperação explícito. Uma transação SQL, por si só, não torna atômica uma operação que também grava arquivos no disco.
- Use transações SQLite para alterações relacionadas de metadados e trate falhas de commit.
- Defina claramente quando um chunk passa a ser referenciado e quando seu `reference_count` é alterado. Evite contagens negativas, incrementos duplicados e exclusões prematuras.
- Em operações concorrentes, impeça que dois workers gravem ou removam o mesmo chunk de forma inconsistente.
- Em caso de falha parcial, mantenha dados recuperáveis e deixe que a rotina de recuperação identifique os resíduos com segurança.
- Não remova um chunk enquanto houver referências válidas ou uma operação em andamento que dependa dele.
- Restaurações devem reconstruir os chunks na ordem registrada em `file_chunks`, validar os dados e só substituir o destino final depois que o arquivo temporário estiver completo.
- Preserve, quando suportado e registrado, os metadados de arquivo relevantes, incluindo permissões e horário de modificação.

## 7. Pipeline de backup

Implemente e mantenha o pipeline com etapas verificáveis:

1. Descobrir e validar os arquivos de origem.
2. Ler os dados com tratamento explícito de arquivos inacessíveis, alterados durante a leitura ou em uso.
3. Dividir o conteúdo em chunks com FastCDC e parâmetros configurados.
4. Calcular BLAKE3 para cada chunk.
5. Consultar o índice para determinar se o chunk já existe e pode ser reutilizado.
6. Para chunks novos, comprimir com Zstandard.
7. Criptografar os dados comprimidos antes de gravá-los no repositório.
8. Gravar os chunks com segurança e registrar as associações entre arquivos e chunks.
9. Finalizar os metadados e o snapshot somente após confirmar que todos os dados necessários estão persistidos.

Regras adicionais:

- A deduplicação deve ocorrer antes de repetir compressão e criptografia de um chunk já armazenado.
- Não confie apenas no nome do arquivo para determinar se seu conteúdo é igual.
- Mantenha separadas as responsabilidades de chunking, hashing, compressão, criptografia e persistência para permitir testes isolados.
- Se o arquivo de origem mudar durante o backup, detecte ou trate essa condição para evitar associar metadados inconsistentes a um snapshot.
- Não carregue arquivos inteiros na memória quando for possível processá-los em fluxo ou em blocos limitados.
- Registre erros por arquivo quando isso permitir que o restante do trabalho continue sem declarar um snapshot completo incorretamente.

## 8. Criptografia e segredos

- Use uma biblioteca criptográfica mantida e APIs seguras; não implemente algoritmos criptográficos manualmente.
- Para AES-GCM, use um nonce único para cada operação de criptografia sob a mesma chave. Nunca reutilize nonces com a mesma chave.
- Armazene junto ao chunk os metadados necessários para a descriptografia, como versão/formato, nonce e tag, conforme a representação adotada.
- A descriptografia deve falhar de forma segura quando a autenticação não for válida. Nunca entregue dados não autenticados como se fossem válidos.
- Não grave chaves em logs, mensagens de erro, snapshots de teste versionados ou arquivos de configuração sem proteção.
- Não invente um esquema de derivação, armazenamento, rotação ou recuperação de chaves sem documentar as decisões e seus riscos.
- A CLI pode permitir configurar chaves, mas os valores devem ser tratados como segredos: evite expô-los em argumentos visíveis do processo ou no histórico do shell quando houver alternativa segura.
- Teste entradas corrompidas, chaves incorretas, tags inválidas e truncamento de dados.
- Mudanças no formato criptografado precisam de versionamento e estratégia de compatibilidade.

## 9. SQLite e metadados

O banco previsto é `metadata/index.db`, com as tabelas principais:

- `snapshots`
- `files`
- `chunks`
- `file_chunks`

- Defina chaves primárias, chaves estrangeiras, restrições e índices adequados ao esquema real.
- Documente o significado e as regras de atualização de `reference_count`.
- Use consultas parametrizadas; nunca monte SQL incorporando diretamente valores externos.
- Use transações para operações de metadados que precisam ser consistentes em conjunto.
- Ative e verifique as opções de integridade apropriadas do SQLite, incluindo chaves estrangeiras quando usadas pelo esquema.
- Planeje migrações versionadas em vez de depender apenas de criação de tabelas na inicialização.
- Considere como o banco se comporta após encerramento inesperado e como validar ou recuperar metadados.
- Não altere nomes de colunas, significado de campos ou formato persistido sem migração e testes.
- Evite manter locks ou transações abertas durante operações demoradas de I/O que não precisem ocorrer dentro da transação.

## 10. Daemon, watcher e concorrência

- Use `notify` para observar criação, modificação e exclusão conforme a plataforma suportar.
- Trate eventos duplicados, eventos agrupados, caminhos inválidos, renomeações e falhas do watcher.
- Não execute processamento pesado diretamente no callback do watcher. Encaminhe eventos para uma fila e workers controlados.
- Defina limites para filas, concorrência e memória; não permita crescimento ilimitado em situações de alta atividade.
- Quando a fila atingir seus limites, adote comportamento explícito e recuperável — por exemplo, consolidar eventos ou realizar uma nova varredura — sem perder silenciosamente alterações necessárias.
- Considere que eventos do sistema de arquivos são notificações, não necessariamente um registro perfeito de todas as mudanças.
- Evite processar simultaneamente várias vezes o mesmo caminho quando isso puder causar gravações conflitantes.
- O daemon deve encerrar de maneira controlada, finalizar ou interromper operações com segurança e liberar recursos.
- Restrinja a comunicação local a clientes autorizados conforme os mecanismos disponíveis na plataforma. Não exponha um serviço de controle em interfaces de rede públicas por padrão.
- Se o protocolo de comunicação ainda não estiver definido, compare IPC, gRPC e REST local e registre a escolha antes de construir dependências extensas sobre ela.

## 11. Restauração

- Permita localizar snapshots e arquivos por meio dos metadados persistidos.
- Leia as associações em `file_chunks` na ordem correta.
- Para cada chunk, valide leitura, autenticação criptográfica, descompressão e hash conforme o formato definido.
- Interrompa a restauração e informe claramente qual arquivo ou chunk falhou; não apresente saída parcial como restauração bem-sucedida.
- Monte o arquivo em um temporário no destino ou em local apropriado, valide o resultado e só então mova-o para o destino final.
- Nunca sobrescreva silenciosamente um arquivo existente. Defina o comportamento de conflito explicitamente na interface.
- Valide caminhos restaurados para impedir path traversal e gravação fora do diretório de destino solicitado.
- Preserve permissões e `modification_time` quando os dados estiverem disponíveis e a plataforma permitir.
- Teste restauração de arquivos vazios, arquivos grandes, nomes Unicode, caminhos profundos, chunks ausentes e conteúdo corrompido.

## 12. Recuperação, retenção e integridade

### Crash recovery
- Na inicialização, identifique arquivos temporários e chunks sem referência finalizada.
- Antes de apagar um suposto órfão, verifique o estado do banco e assegure-se de que nenhuma operação válida ainda o utiliza.
- Faça a rotina ser segura para execução repetida e registre as ações tomadas.

### Pruning e retenção
- Remova primeiro as referências de snapshots expirados em uma operação de metadados consistente.
- Recalcule ou atualize contagens de referência de forma verificável.
- Só exclua chunks quando não houver referências válidas nem operações concorrentes que dependam deles.
- Trate falhas de exclusão de arquivos sem deixar o banco afirmar um estado que não corresponde ao disco.
- Documente a política de retenção antes de automatizar exclusões irreversíveis.

### Scrubbing
- Verifique chunks por amostragem ou conforme a agenda configurada.
- Descriptografe e descomprima com validação; recalcule BLAKE3 sobre o conteúdo do chunk segundo o formato definido.
- Marque corrupção de forma explícita nos metadados e produza um diagnóstico útil sem expor segredos.
- Não altere nem descarte automaticamente dados corrompidos sem uma política de recuperação definida.

## 13. Configuração e interface CLI

- Leia e valide `config/settings.json`; apresente erros de configuração com contexto e sem revelar segredos.
- Valide diretórios de origem e destino e os parâmetros de chunking, concorrência, retenção e monitoramento.
- Defina valores padrão seguros e documente-os.
- A CLI deve oferecer operações para configurar diretórios e chaves, iniciar backups manuais, listar snapshots e restaurar arquivos, conforme o backlog.
- A CLI deve se comunicar com o daemon pelo protocolo local escolhido; não duplique a lógica central de backup na interface.
- Comandos destrutivos devem exigir confirmação ou uma opção explícita de força, quando apropriado.
- Forneça mensagens claras, códigos de saída úteis e ajuda de uso.
- GUI é opcional; não a priorize em detrimento da integridade do engine e dos fluxos de backup/restauração.

## 14. Testes obrigatórios por mudança

Escolha testes proporcionais à mudança e execute-os antes de concluir a tarefa.

### Testes unitários
- Hashing BLAKE3 e tratamento de entradas vazias.
- Chunking: conteúdo vazio, pequeno, grande e limites de chunk.
- Compressão/descompressão.
- Criptografia/descriptografia, chave incorreta, nonce/formato inválido e dados adulterados.
- Parser e validação de configurações.
- Cálculo e atualização de referências.

### Testes de integração
- Backup e restauração de ponta a ponta com verificação byte a byte.
- Deduplicação de arquivos iguais e de conteúdo compartilhado entre arquivos.
- Snapshot com arquivos vazios, binários, grandes e nomes Unicode.
- Falhas de leitura, gravação, banco, compressão, criptografia e restauração.
- Reinicialização após falhas simuladas e limpeza segura de órfãos.
- Exclusão de snapshots e pruning sem remover chunks ainda referenciados.

### Testes de carga
- O backlog define um teste do daemon monitorando mais de 100.000 arquivos para avaliar consumo de RAM.
- Meça o consumo em vez de presumir que está dentro do limite.
- Teste filas de eventos sob alta atividade e verifique que a memória permanece limitada.
- Registre ambiente, tamanho do conjunto, duração e resultados para que o teste seja reproduzível.

Não use apenas testes que verificam se a função retorna sucesso; valide também o conteúdo restaurado, os metadados e o estado do repositório.

## 15. Comandos de verificação

Use os comandos compatíveis com o projeto e o ambiente. Como ponto de partida:

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Se o workspace ou algum recurso ainda não existir, adapte o comando e informe a limitação. Não afirme que um comando passou sem executá-lo e observar seu resultado. Adicione testes específicos para a mudança realizada.

## 16. Portabilidade

- O backlog prevê empacotamento para Windows, Linux e macOS.
- Isole código específico de sistema operacional por módulos e condicionais de compilação quando necessário.
- Não presuma que caminhos, permissões, renomeações, bloqueios de arquivos ou APIs de monitoramento funcionam de maneira idêntica em todas as plataformas.
- Implemente VSS apenas no Windows e documente o comportamento alternativo nas demais plataformas.
- Verifique as metas de compilação disponíveis antes de afirmar que há suporte multiplataforma.
- Não prometa binários estáticos sem validar as limitações de cada alvo e suas dependências nativas.

## 17. Fluxo de trabalho do agente

Para cada tarefa:

1. Leia a solicitação e identifique os requisitos e critérios de aceitação.
2. Inspecione os módulos, dependências, testes e documentação relevantes.
3. Apresente um plano curto para mudanças que envolvam várias partes ou afetem dados persistidos.
4. Implemente a menor mudança coerente que resolva o problema.
5. Acrescente ou atualize testes, incluindo casos de erro relevantes.
6. Execute formatação, compilação, testes e lint aplicáveis.
7. Revise o diff para detectar mudanças não relacionadas, segredos acidentais, tratamento de erro insuficiente e riscos à integridade dos backups.
8. Atualize a documentação quando houver mudanças de arquitetura, configuração, formato persistido ou comportamento de CLI.
9. Ao concluir, informe o que mudou, os testes executados e seus resultados, além de limitações ou decisões pendentes.

Se algo falhar, investigue a causa. Não remova testes, reduza garantias de segurança ou enfraqueça verificações apenas para obter uma compilação verde.

## 18. Critérios de conclusão

Uma tarefa só está pronta quando:

- O comportamento solicitado está implementado e consistente com a arquitetura existente.
- Os erros previsíveis são tratados sem perda silenciosa de dados.
- Os testes relevantes foram adicionados ou atualizados e executados.
- A formatação e as verificações aplicáveis passaram, ou as falhas estão explicitamente relatadas.
- Nenhum segredo, dado de backup real ou artefato temporário foi adicionado ao controle de versão por engano.
- A documentação foi atualizada quando necessário.
- As limitações e as suposições restantes estão claramente indicadas.

## 19. Ordem de prioridade

Ao tomar decisões técnicas, priorize nesta ordem:

1. **Integridade e recuperabilidade dos dados.**
2. **Segurança criptográfica e proteção de segredos.**
3. **Consistência entre o armazenamento em disco e o SQLite.**
4. **Correção funcional e tratamento de falhas.**
5. **Uso previsível de memória e recursos.**
6. **Portabilidade e facilidade de manutenção.**
7. **Desempenho, somente sem comprometer os pontos anteriores.**

Em caso de conflito entre conveniência e segurança dos dados, escolha o comportamento mais seguro e explique o impacto.
