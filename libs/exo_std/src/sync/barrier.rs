// libs/exo_std/src/sync/barrier.rs
//! Barrière de synchronisation pour coordination de threads
//!
//! Permet à N threads d'attendre mutuellement à un point de rendez-vous.

use core::sync::atomic::{AtomicUsize, Ordering};
use crate::Result;
use crate::error::SyncError;

/// Barrière de synchronisation
///
/// # Exemple
/// ```no_run
/// use exo_std::sync::Barrier;
/// use exo_std::thread;
///
/// let barrier = Barrier::new(3);
///
/// for _ in 0..3 {
///     let b = &barrier;
///     thread::spawn(move || {
///         // Travail parallèle
///         
///         // Attente à la barrière
///         let result = b.wait();
///         
///         if result.is_leader() {
///             // Un seul thread exécute ceci
///         }
///     });
/// }
/// ```
pub struct Barrier {
    /// Nombre de threads qui doivent wait()
    num_threads: usize,
    /// Compteur de threads arrivés
    count: AtomicUsize,
    /// Génération actuelle (pour détecter les cycles)
    generation: AtomicUsize,
}

/// Résultat d'un wait() sur barrière
pub struct BarrierWaitResult {
    is_leader: bool,
}

impl Barrier {
    /// Crée une nouvelle barrière pour `n` threads
    ///
    /// # Panics
    /// Panique si `n` == 0
    pub fn new(n: usize) -> Self {
        assert!(n > 0, "Barrier count must be > 0");
        
        Self {
            num_threads: n,
            count: AtomicUsize::new(0),
            generation: AtomicUsize::new(0),
        }
    }
    
    /// Attend que tous les threads atteignent la barrière
    ///
    /// Retourne un résultat indiquant si ce thread est le "leader"
    /// (le dernier à arriver). Exactement un thread recevra `is_leader() == true`.
    pub fn wait(&self) -> Result<BarrierWaitResult> {
        let gen = self.generation.load(Ordering::Acquire);
        
        // Incrémente le compteur
        let count = self.count.fetch_add(1, Ordering::AcqRel) + 1;
        
        if count < self.num_threads {
            // Pas encore tous arrivés, attendre
            self.wait_for_generation(gen)?;
            Ok(BarrierWaitResult { is_leader: false })
        } else {
            // Dernier thread: réinitialise pour prochain cycle
            self.count.store(0, Ordering::Release);
            self.generation.fetch_add(1, Ordering::Release);
            Ok(BarrierWaitResult { is_leader: true })
        }
    }
    
    /// Attend que la génération change
    fn wait_for_generation(&self, gen: usize) -> Result<()> {
        let mut backoff = super::mutex::Backoff::new();
        
        loop {
            let current_gen = self.generation.load(Ordering::Acquire);
            if current_gen != gen {
                return Ok(());
            }
            
            backoff.spin();
            if backoff.should_yield() {
                crate::syscall::thread::yield_now();
            }
            backoff.next();
        }
    }
}

impl BarrierWaitResult {
    /// Retourne true si ce thread est le leader (dernier arrivé)
    #[inline]
    pub fn is_leader(&self) -> bool {
        self.is_leader
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_barrier() {
        let barrier = Barrier::new(1);
        let result = barrier.wait().unwrap();
        assert!(result.is_leader());
    }
}
