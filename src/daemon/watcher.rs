//! Watcher de sistema de arquivos com `notify`.
//!
//! Observa criação, modificação e exclusão de arquivos no diretório configurado.
//! Encaminha eventos para uma fila interna; não processa diretamente no callback.

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::mpsc::Sender;

pub enum FileEvent {
    Created(String),
    Modified(String),
    Deleted(String),
}

pub fn start_watcher(path: &str, sender: Sender<FileEvent>) -> notify::Result<RecommendedWatcher> {
    let mut watcher: RecommendedWatcher = Watcher::new(
        move |res: notify::Result<Event>| {
            if let Ok(event) = res {
                for path_result in event.paths {
                    let path_str = path_result.to_string_lossy().to_string();
                    let evt = match event.kind {
                        notify::EventKind::Create(_) => Some(FileEvent::Created(path_str)),
                        notify::EventKind::Modify(_) => Some(FileEvent::Modified(path_str)),
                        notify::EventKind::Remove(_) => Some(FileEvent::Deleted(path_str)),
                        _ => None,
                    };
                    if let Some(e) = evt {
                        let _ = sender.send(e);
                    }
                }
            }
        },
        Config::default(),
    )?;

    watcher.watch(Path::new(path), RecursiveMode::Recursive)?;
    Ok(watcher)
}
