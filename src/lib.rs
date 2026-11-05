//! Biblioteca redbackup — mecanismo de backup.
//!
//! Arquitetura: um crate único, módulos internos tratados como futuras crates.

pub mod backup;
pub mod cli;
pub mod config;
pub mod daemon;
pub mod metadata;
pub mod metadata_backup;
pub mod pipeline;
pub mod recovery;
pub mod redundancy;
pub mod repository;
pub mod restoration;
pub mod retention;
pub mod scrubbing;

pub mod status;

// Presente somente no executável de testes, nunca na biblioteca distribuída.
#[cfg(test)]
pub(crate) fn crash_point(point: &str) {
    if std::env::var("REDBACKUP_TEST_CRASH_POINT").as_deref() == Ok(point) {
        std::process::exit(86);
    }
}
