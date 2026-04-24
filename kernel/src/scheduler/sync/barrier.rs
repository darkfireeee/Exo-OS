// kernel/src/scheduler/sync/barrier.rs
//
// Barrière de synchronisation — attend que N threads arrivent au point commun.

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

    /// Signale l'arrivée du thread courant. Retourne `true` si c'est le dernier
    /// (la barrière est franchie).
    pub fn wait(&self) -> bool {
        let gen = self.generation.load(Ordering::Acquire);
        let prev = self.arrived.fetch_add(1, Ordering::AcqRel);
        if prev + 1 == self.target {
            // Réinitialiser pour la prochaine utilisation.
            self.arrived.store(0, Ordering::Release);
            self.generation.fetch_add(1, Ordering::Release);
            return true;
        }
        // Boucle active jusqu'à la fin de la génération courante.
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
