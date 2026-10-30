//! Interface CLI: comunicação com daemon via IPC nativo.
//!
//! Comandos mínimos:
//! - init --source --dest --key-file
//! - backup --manual
//! - restore --snapshot --file --output (comportamento de conflito explícito)
//! - list-snapshots
//! - prune (exige --force ou confirmação)
//!
//! Mensagens claras, códigos de saída úteis.
//! Chave tratada como segredo: não exposta em argumentos visíveis ou logs.
