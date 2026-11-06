// kernel/src/scheduler/sync/barrier.rs
//
// Synchronization barrier — waits for N threads to arrive at a rendezvous point.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub struct KBarrier {
    target: u32,
    arrived: AtomicU32,
    generation: AtomicU64,
}

impl KBarrier {
    pub const fn new(n: u32) -> Self {
        Self {
            target: n,
            arrived: AtomicU32::new(0),
            generation: AtomicU64::new(0),
        }
    }

    /// Signals arrival of current thread. Returns `true` if this is the last thread
    /// (barrier is tripped).
    pub fn wait(&self) -> bool {
        let gen = self.generation.load(Ordering::Acquire);
        let prev = self.arrived.fetch_add(1, Ordering::AcqRel);
        if prev + 1 == self.target {
            // Reset for next generation.
            self.arrived.store(0, Ordering::Release);
            self.generation.fetch_add(1, Ordering::Release);
            return true;
        }
        // Spin loop until current generation advances.
        loop {
            let cur_gen = self.generation.load(Ordering::Acquire);
            if cur_gen != gen {
                break;
            }
            core::hint::spin_loop();
        }
        false
    }

    pub fn reset(&self) {
        self.arrived.store(0, Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
}
