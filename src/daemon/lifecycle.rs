//! Process-lifetime resources for services.
use std::sync::{atomic::AtomicBool, Arc};

pub struct Signals {
    #[cfg(unix)]
    ids: Vec<signal_hook::SigId>,
}
impl Signals {
    pub fn new(stop: Arc<AtomicBool>) -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            let mut guard = Self { ids: Vec::new() };
            for signal in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
                guard
                    .ids
                    .push(signal_hook::flag::register(signal, stop.clone())?);
            }
            Ok(guard)
        }
        #[cfg(not(unix))]
        {
            let _ = stop;
            Ok(Self {})
        }
    }
}
impl Drop for Signals {
    fn drop(&mut self) {
        #[cfg(unix)]
        for id in self.ids.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}
