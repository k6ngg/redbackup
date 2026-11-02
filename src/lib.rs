//! Biblioteca redbackup — mecanismo de backup.
//!
//! Arquitetura: um crate único, módulos internos tratados como futuras crates.

pub mod backup;
pub mod cli;
pub mod config;
pub mod daemon;
pub mod metadata;
pub mod pipeline;
pub mod recovery;
pub mod repository;
pub mod restoration;
pub mod retention;
pub mod scrubbing;
