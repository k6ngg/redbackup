//! Biblioteca redbackup — mecanismo de backup.
//!
//! Arquitetura: um crate único, módulos internos tratados como futuras crates.

pub mod config;
pub mod repository;
pub mod metadata;
pub mod pipeline;
pub mod daemon;
pub mod cli;
pub mod restoration;
