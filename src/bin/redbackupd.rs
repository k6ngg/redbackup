//! Dedicated daemon entry point; shares the engine and argument validation.
fn main() {
    let mut args = vec!["redbackup".to_string(), "daemon".to_string()];
    args.extend(std::env::args().skip(1));
    std::process::exit(redbackup::cli::run(args));
}
