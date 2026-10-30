//! Daemon e watcher: observação de sistema de arquivos e fila de eventos.
//!
//! Usa `notify` para eventos (criação, modificação, exclusão).
//! Fila interna com limite (ex: 10.000 eventos).
//! Eventos duplicados/consolidados tratados.
//! Processamento pesado não ocorre diretamente no callback; encaminhado para worker.
//! Comunicação local via IPC nativo.
//! Shutdown controlado (SIGTERM/SIGINT): finalizar operações pendentes, liberar recursos.
