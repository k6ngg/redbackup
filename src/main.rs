//! redbackup — mecanismo de backup com chunking, hashing, deduplicação, compressão, criptografia e metadados SQLite.
//!
//! Arquitetura: um crate único, módulos internos tratados como futuras crates.
//! Prioridade máxima: integridade dos dados (pipeline atômico, chunks temporários + rename atômico).

fn main() {
    // Fase 0: estrutura inicial. O daemon e CLI serão inicializados nas fases subsequentes.
    println!("redbackup v0.1.0 — Fase 0: estrutura inicializada.");
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
