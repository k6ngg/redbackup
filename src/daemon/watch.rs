//! Constant-memory event coalescing and cooperative shutdown.
use crate::{backup, config::settings::Settings};
use anyhow::Result;
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

pub fn observe(
    root: &Path,
    settings: &Settings,
    dirty: Arc<AtomicBool>,
) -> Result<RecommendedWatcher> {
    let excluded = root.canonicalize()?;
    let mut watcher = RecommendedWatcher::new(
        move |event: notify::Result<notify::Event>| match event {
            Ok(event) => {
                if !matches!(event.kind, notify::EventKind::Access(_))
                    && (event.paths.is_empty()
                        || event.paths.iter().any(|p| !p.starts_with(&excluded)))
                {
                    dirty.store(true, Ordering::Release);
                }
            }
            Err(error) => {
                eprintln!("Watcher: {}; nova varredura pendente", error);
                dirty.store(true, Ordering::Release);
            }
        },
        Config::default(),
    )?;
    watcher.watch(Path::new(&settings.source_path), RecursiveMode::Recursive)?;
    Ok(watcher)
}

pub fn worker(
    root: &Path,
    settings: &Settings,
    key: &[u8; 32],
    debounce: u64,
    dirty: &AtomicBool,
    stop: &AtomicBool,
) {
    let debounce = Duration::from_secs(debounce.max(1));
    let mut pending = false;
    let mut last = Instant::now();
    let mut first = Instant::now();
    while !stop.load(Ordering::Acquire) {
        if dirty.swap(false, Ordering::AcqRel) {
            if !pending {
                first = Instant::now();
            }
            pending = true;
            last = Instant::now();
        }
        if pending && (last.elapsed() >= debounce || first.elapsed() >= Duration::from_secs(60)) {
            if stop.load(Ordering::Acquire) {
                break;
            }
            match backup::run_backup(root, settings, key) {
                Ok(report) if report.status == "completed" => {
                    println!(
                        "Backup automático concluído: snapshot {}",
                        report.snapshot_id
                    );
                    pending = false;
                }
                Ok(report) => {
                    eprintln!(
                        "Snapshot {} falhou: {:?}",
                        report.snapshot_id, report.errors
                    );
                    last = Instant::now();
                    first = last;
                }
                Err(error) => {
                    eprintln!("Backup automático adiado: {:#}", error);
                    last = Instant::now();
                    first = last;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub fn run(root: &Path, settings: &Settings, key: &[u8; 32], debounce: u64) -> Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    let _signals = super::lifecycle::Signals::new(stop.clone())?;
    let dirty = Arc::new(AtomicBool::new(true));
    let _watcher = observe(root, settings, dirty.clone())?;
    worker(root, settings, key, debounce, &dirty, &stop);
    Ok(())
}
