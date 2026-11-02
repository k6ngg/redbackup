//! Watcher entry point. Events are coalesced into a constant-memory dirty flag.
pub use super::watch::observe as start_watcher;
