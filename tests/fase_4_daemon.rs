//! Testes da Fase 4 — Daemon, Watcher e Fila.

use redbackup::daemon::queue::EventQueue;

#[test]
fn fila_interna_com_limite() {
    let fila = EventQueue::new(3);
    assert!(fila.enqueue("evento_1".to_string()));
    assert!(fila.enqueue("evento_2".to_string()));
    assert!(fila.enqueue("evento_3".to_string()));
    // Limite atingido (3 eventos)
    assert!(!fila.enqueue("evento_4".to_string()));
    let eventos = fila.dequeue_all();
    assert_eq!(eventos.len(), 3);
}
