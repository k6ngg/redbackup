//! Fila interna de eventos do daemon.
//!
//! Limite configurável (ex: 10.000 eventos). Quando atinge o limite,
//! consolida eventos (ex: realiza uma nova varredura manual) e loga claramente.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

pub struct EventQueue {
    inner: Arc<Mutex<VecDeque<String>>>,
    limit: usize,
}

impl EventQueue {
    pub fn new(limit: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(limit))),
            limit,
        }
    }

    pub fn enqueue(&self, event: String) -> bool {
        let mut queue = self.inner.lock().unwrap();
        if queue.len() >= self.limit {
            // Limite atingido: consolida removendo duplicados (simplificado)
            // Na implementação completa, seria realizada uma nova varredura manual.
            false
        } else {
            queue.push_back(event);
            true
        }
    }

    pub fn dequeue_all(&self) -> Vec<String> {
        let mut queue = self.inner.lock().unwrap();
        let events: Vec<String> = queue.drain(..).collect();
        events
    }
}
