//! Tests unitaires pour PerCpuQueue
//! Ces tests s'exécutent sans nécessiter de hardware SMP réel

#![cfg(test)]

use super::*;
use alloc::sync::Arc;
use crate::scheduler::thread::Thread;

/// Test: Création et initialisation
#[test]
fn test_percpu_queue_new() {
    let queue = PerCpuQueue::new(0);
    assert_eq!(queue.cpu_id(), 0);
    assert_eq!(queue.len(), 0);
    assert!(queue.is_empty());
}

/// Test: Enqueue/Dequeue basique
#[test]
fn test_enqueue_dequeue_single() {
    let queue = PerCpuQueue::new(0);
    
    // Créer un thread test (stub)
    let thread = Arc::new(Thread::new_kernel(
        1,
        "test_thread",
        test_entry_point,
        4096
    ));
    
    // Enqueue
    queue.enqueue(thread.clone());
    assert_eq!(queue.len(), 1);
    assert!(!queue.is_empty());
    
    // Dequeue
    let dequeued = queue.dequeue().unwrap();
    assert_eq!(dequeued.id(), thread.id());
    assert_eq!(queue.len(), 0);
    assert!(queue.is_empty());
}

/// Test: FIFO ordering
#[test]
fn test_fifo_ordering() {
    let queue = PerCpuQueue::new(0);
    
    // Enqueue 5 threads
    for i in 1..=5 {
        let thread = Arc::new(Thread::new_kernel(
            i,
            "test",
            test_entry_point,
            4096
        ));
        queue.enqueue(thread);
    }
    
    assert_eq!(queue.len(), 5);
    
    // Dequeue doit respecter l'ordre FIFO
    for i in 1..=5 {
        let thread = queue.dequeue().unwrap();
        assert_eq!(thread.id(), i);
    }
    
    assert!(queue.is_empty());
}

/// Test: Dequeue sur queue vide
#[test]
fn test_dequeue_empty() {
    let queue = PerCpuQueue::new(0);
    assert!(queue.dequeue().is_none());
}

/// Test: Work stealing - steal_half basic
#[test]
fn test_steal_half_even() {
    let queue = PerCpuQueue::new(0);
    
    // Enqueue 10 threads
    for i in 1..=10 {
        let thread = Arc::new(Thread::new_kernel(
            i,
            "test",
            test_entry_point,
            4096
        ));
        queue.enqueue(thread);
    }
    
    assert_eq!(queue.len(), 10);
    
    // Steal half
    let stolen = queue.steal_half();
    assert_eq!(stolen.len(), 5);
    assert_eq!(queue.len(), 5);
    
    // Vérifier que les bons threads sont volés (les plus anciens)
    for (i, thread) in stolen.iter().enumerate() {
        assert_eq!(thread.id(), (i + 1) as u64);
    }
}

/// Test: Work stealing - steal_half impair
#[test]
fn test_steal_half_odd() {
    let queue = PerCpuQueue::new(0);
    
    // Enqueue 9 threads
    for i in 1..=9 {
        let thread = Arc::new(Thread::new_kernel(
            i,
            "test",
            test_entry_point,
            4096
        ));
        queue.enqueue(thread);
    }
    
    assert_eq!(queue.len(), 9);
    
    // Steal half (9/2 = 4)
    let stolen = queue.steal_half();
    assert_eq!(stolen.len(), 4);
    assert_eq!(queue.len(), 5);
}

/// Test: Work stealing sur queue vide
#[test]
fn test_steal_half_empty() {
    let queue = PerCpuQueue::new(0);
    let stolen = queue.steal_half();
    assert!(stolen.is_empty());
}

/// Test: Work stealing sur queue avec 1 thread
#[test]
fn test_steal_half_single() {
    let queue = PerCpuQueue::new(0);
    
    let thread = Arc::new(Thread::new_kernel(
        1,
        "test",
        test_entry_point,
        4096
    ));
    queue.enqueue(thread);
    
    // Ne devrait rien voler (laisse au moins 1)
    let stolen = queue.steal_half();
    assert!(stolen.is_empty());
    assert_eq!(queue.len(), 1);
}

/// Test: Statistics - enqueue count
#[test]
fn test_stats_enqueue_count() {
    let queue = PerCpuQueue::new(0);
    
    let initial = queue.stats().enqueue_count;
    
    // Enqueue 3 threads
    for i in 1..=3 {
        let thread = Arc::new(Thread::new_kernel(
            i,
            "test",
            test_entry_point,
            4096
        ));
        queue.enqueue(thread);
    }
    
    assert_eq!(queue.stats().enqueue_count, initial + 3);
}

/// Test: Statistics - dequeue count
#[test]
fn test_stats_dequeue_count() {
    let queue = PerCpuQueue::new(0);
    
    // Enqueue puis dequeue
    for i in 1..=3 {
        let thread = Arc::new(Thread::new_kernel(
            i,
            "test",
            test_entry_point,
            4096
        ));
        queue.enqueue(thread);
    }
    
    let initial_dequeue = queue.stats().dequeue_count;
    
    for _ in 0..3 {
        queue.dequeue();
    }
    
    assert_eq!(queue.stats().dequeue_count, initial_dequeue + 3);
}

/// Test: Statistics - steal count
#[test]
fn test_stats_steal_count() {
    let queue = PerCpuQueue::new(0);
    
    // Enqueue 10 threads
    for i in 1..=10 {
        let thread = Arc::new(Thread::new_kernel(
            i,
            "test",
            test_entry_point,
            4096
        ));
        queue.enqueue(thread);
    }
    
    let initial_steal = queue.stats().steal_count;
    
    // Steal
    queue.steal_half();
    
    assert_eq!(queue.stats().steal_count, initial_steal + 1);
}

/// Test: Multiple CPUs - indépendance
#[test]
fn test_multiple_queues_independence() {
    let queue0 = PerCpuQueue::new(0);
    let queue1 = PerCpuQueue::new(1);
    
    // Enqueue sur CPU 0
    for i in 1..=5 {
        let thread = Arc::new(Thread::new_kernel(
            i,
            "test",
            test_entry_point,
            4096
        ));
        queue0.enqueue(thread);
    }
    
    // Enqueue sur CPU 1
    for i in 6..=10 {
        let thread = Arc::new(Thread::new_kernel(
            i,
            "test",
            test_entry_point,
            4096
        ));
        queue1.enqueue(thread);
    }
    
    assert_eq!(queue0.len(), 5);
    assert_eq!(queue1.len(), 5);
    
    // Dequeue sur CPU 0 ne doit pas affecter CPU 1
    queue0.dequeue();
    assert_eq!(queue0.len(), 4);
    assert_eq!(queue1.len(), 5);
}

/// Test: Fairness - work stealing distribution
#[test]
fn test_work_stealing_fairness() {
    let queue = PerCpuQueue::new(0);
    
    // Enqueue 100 threads
    for i in 1..=100 {
        let thread = Arc::new(Thread::new_kernel(
            i,
            "test",
            test_entry_point,
            4096
        ));
        queue.enqueue(thread);
    }
    
    // Steal plusieurs fois
    let mut total_stolen = 0;
    for _ in 0..5 {
        let stolen = queue.steal_half();
        total_stolen += stolen.len();
    }
    
    // Doit avoir volé environ 50% du total
    // Steal 1: 50, reste 50
    // Steal 2: 25, reste 25
    // Steal 3: 12, reste 13
    // Steal 4: 6, reste 7
    // Steal 5: 3, reste 4
    // Total stolen: 50+25+12+6+3 = 96
    assert!(total_stolen >= 90);
    assert!(queue.len() <= 10);
}

/// Test: Concurrent simulation - pas de race conditions
#[test]
fn test_concurrent_simulation() {
    let queue = PerCpuQueue::new(0);
    
    // Simuler operations concurrentes (single-threaded test)
    // Enqueue/dequeue/steal interleaved
    for round in 0..10 {
        // Enqueue batch
        for i in 0..10 {
            let thread = Arc::new(Thread::new_kernel(
                (round * 10 + i) as u64,
                "test",
                test_entry_point,
                4096
            ));
            queue.enqueue(thread);
        }
        
        // Dequeue some
        for _ in 0..3 {
            queue.dequeue();
        }
        
        // Steal some
        if round % 2 == 0 {
            queue.steal_half();
        }
    }
    
    // Vérifier cohérence
    let stats = queue.stats();
    assert!(stats.enqueue_count >= 100);
    assert!(stats.dequeue_count >= 30);
    assert!(stats.steal_count >= 5);
}

/// Dummy thread entry point pour tests
fn test_entry_point() -> ! {
    loop {
        unsafe { core::arch::asm!("hlt") };
    }
}
