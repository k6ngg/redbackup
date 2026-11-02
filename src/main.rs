//! redbackup — mecanismo de backup com chunking, hashing, deduplicação,
//! compressão, criptografia e metadados SQLite.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    std::process::exit(redbackup::cli::run(args));
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
