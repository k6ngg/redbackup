//! Supervises explicitly registered repositories; idle is a healthy state.
#[cfg(unix)]
pub fn run() -> anyhow::Result<()> {
    use crate::{
        config::{key, registry, settings::Settings},
        daemon::{lifecycle::Signals, server},
    };
    use fs2::FileExt;
    use std::os::unix::fs::OpenOptionsExt;
    use std::{
        collections::HashMap,
        fs::OpenOptions,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::{Duration, Instant},
    };
    let dir = registry::directory()?;
    std::fs::create_dir_all(&dir)?;
    crate::config::validation::set_directory_permissions(&dir, 0o700)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(dir.join("manager.lock"))?;
    lock.try_lock_exclusive()
        .map_err(|e| anyhow::anyhow!("Supervisor já ativo: {}", e))?;
    let stop = Arc::new(AtomicBool::new(false));
    let _signals = Signals::new(stop.clone())?;
    println!(
        "Daemon aguardando cadastro em {}",
        dir.join("repositories.json").display()
    );
    let mut running: HashMap<std::path::PathBuf, (Arc<AtomicBool>, std::thread::JoinHandle<()>)> =
        HashMap::new();
    let mut retry = HashMap::new();
    while !stop.load(Ordering::Acquire) {
        match registry::list_at(&dir) {
            Ok(paths) => {
                retry.retain(|path, _| paths.contains(path));
                for (path, (flag, _)) in &running {
                    if !paths.contains(path) || !path.join("config/settings.json").is_file() {
                        flag.store(true, Ordering::Release);
                    }
                }
                let finished: Vec<_> = running
                    .iter()
                    .filter(|(_, (_, handle))| handle.is_finished())
                    .map(|(path, _)| path.clone())
                    .collect();
                for path in finished {
                    if let Some((_, handle)) = running.remove(&path) {
                        if handle.join().is_err() {
                            eprintln!("Worker terminou inesperadamente: {}", path.display());
                        }
                    }
                    retry.insert(path, Instant::now());
                }
                for path in paths {
                    if running.contains_key(&path)
                        || retry
                            .get(&path)
                            .is_some_and(|t| t.elapsed() < Duration::from_secs(10))
                    {
                        continue;
                    }
                    retry.insert(path.clone(), Instant::now());
                    let loaded = (|| -> anyhow::Result<_> {
                        crate::repository::open_repository(&path)?;
                        let settings = Settings::load(&path)?;
                        let key_path = settings
                            .key_file_path
                            .as_deref()
                            .ok_or_else(|| anyhow::anyhow!("Chave não configurada"))?;
                        key::load_key(std::path::Path::new(key_path))
                    })();
                    match loaded {
                        Ok(key) => {
                            let flag = Arc::new(AtomicBool::new(false));
                            let worker_flag = flag.clone();
                            let root = path.clone();
                            let handle = std::thread::spawn(move || {
                                if let Err(error) = server::serve_until(&root, &key, worker_flag) {
                                    eprintln!(
                                        "Repositório {} indisponível: {:#}",
                                        root.display(),
                                        error
                                    );
                                }
                            });
                            running.insert(path, (flag, handle));
                        }
                        Err(error) => eprintln!(
                            "Repositório {} indisponível; nova tentativa em 10s: {:#}",
                            path.display(),
                            error
                        ),
                    }
                }
            }
            Err(error) => eprintln!(
                "Cadastro indisponível; mantendo serviços existentes: {:#}",
                error
            ),
        }
        for _ in 0..20 {
            if stop.load(Ordering::Acquire) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    for (flag, _) in running.values() {
        flag.store(true, Ordering::Release);
    }
    for (_, (_, handle)) in running {
        if handle.join().is_err() {
            eprintln!("Worker terminou inesperadamente durante parada");
        }
    }
    Ok(())
}
#[cfg(not(unix))]
pub fn run() -> anyhow::Result<()> {
    anyhow::bail!("Supervisor requer Unix")
}
