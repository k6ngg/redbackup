//! redbackup — mecanismo de backup com chunking, hashing, deduplicação, compressão, criptografia e metadados SQLite.
//!
//! Arquitetura: um crate único, módulos internos tratados como futuras crates.
//! Prioridade máxima: integridade dos dados (pipeline atômico, chunks temporários + rename atômico).

fn main() {
    println!("redbackup v0.1.0 — Fase 1: configuração e inicialização do repositório.");
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
