//! Fila interna de eventos do daemon.
//!
//! Limite configurável. Quando o limite é atingido, o evento novo é rejeitado
//! e o chamador deve executar uma nova varredura completa, evitando perda
//! silenciosa de alterações em cenários de alta atividade.

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
        let mut queue = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if queue.len() >= self.limit {
            false
        } else {
            queue.push_back(event);
            true
        }
    }

    pub fn dequeue_all(&self) -> Vec<String> {
        let mut queue = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        queue.drain(..).collect()
    }

    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
