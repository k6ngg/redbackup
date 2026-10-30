//! Metadados SQLite: `metadata/index.db`.
//!
//! Tabelas principais: snapshots, files, chunks, file_chunks.
//! Migrações versionadas. Chaves primárias, estrangeiras, índices adequados.
//! `PRAGMA foreign_keys = ON`. Consultas parametrizadas. Transações ACID.
