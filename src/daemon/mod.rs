//! Daemon e watcher: observação de sistema de arquivos e fila de eventos.
//!
//! O daemon expõe comandos de backup/manutenção via IPC local (Unix socket).
//! O watcher encaminha eventos para uma fila com limite; processamento pesado
//! nunca ocorre diretamente no callback.

pub mod client;
pub mod protocol;
pub mod queue;
pub mod server;
pub mod watch;
pub mod watcher;
