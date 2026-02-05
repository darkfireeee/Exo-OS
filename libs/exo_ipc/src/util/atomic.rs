// libs/exo_ipc/src/util/atomic.rs
//! Helpers pour opérations atomiques optimisées

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use core::hint;

/// Séquence de backoff exponentiel pour spin-wait
#[derive(Debug)]
pub struct Backoff {
    step: u32,
}

impl Backoff {
    /// Crée un nouveau backoff
    pub const fn new() -> Self {
        Self { step: 0 }
    }
    
    /// Effectue un spin avec backoff exponentiel
    pub fn spin(&mut self) {
        // Limiter le backoff à 2^6 = 64 spins max
        let spins = 1u32 << self.step.min(6);
        
        for _ in 0..spins {
            hint::spin_loop();
        }
        
        self.step += 1;
    }
    
    /// Effectue un snooze (yield au scheduler)
    #[cfg(target_os = "linux")]
    pub fn snooze(&mut self) {
        if self.step <= 10 {
            self.spin();
        } else {
            // Yield au scheduler après plusieurs tentatives
            core::hint::spin_loop();
        }
    }
    
    #[cfg(not(target_os = "linux"))]
    pub fn snooze(&mut self) {
        self.spin();
    }
    
    /// Reset le backoff
    pub fn reset(&mut self) {
        self.step = 0;
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

/// Atomique pour compteur de séquence
pub struct SequenceCounter {
    counter: AtomicU64,
}

impl SequenceCounter {
    /// Crée un nouveau compteur
    pub const fn new() -> Self {
        Self {
            counter: AtomicU64::new(1), // Commence à 1 (0 = invalide)
        }
    }
    
    /// Génère un nouveau numéro de séquence
    pub fn next(&self) -> u64 {
        self.counter.fetch_add(1, Ordering::Relaxed)
    }
    
    /// Récupère la valeur actuelle
    pub fn current(&self) -> u64 {
        self.counter.load(Ordering::Relaxed)
    }
    
    /// Reset le compteur
    pub fn reset(&self) {
        self.counter.store(1, Ordering::Relaxed);
    }
}

impl Default for SequenceCounter {
    fn default() -> Self {
        Self::new()
    }
}

/// Wrapper pour AtomicBool avec meilleure API
pub struct AtomicFlag {
    flag: AtomicBool,
}

impl AtomicFlag {
    /// Crée un nouveau flag
    pub const fn new(initial: bool) -> Self {
        Self {
            flag: AtomicBool::new(initial),
        }
    }
    
    /// Définit le flag
    pub fn set(&self) {
        self.flag.store(true, Ordering::Release);
    }
    
    /// Efface le flag
    pub fn clear(&self) {
        self.flag.store(false, Ordering::Release);
    }
    
    /// Test le flag
    pub fn is_set(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }
    
    /// Test-and-set atomique
    pub fn test_and_set(&self) -> bool {
        self.flag.swap(true, Ordering::AcqRel)
    }
    
    /// Test-and-clear atomique
    pub fn test_and_clear(&self) -> bool {
        self.flag.swap(false, Ordering::AcqRel)
    }
    
    /// Compare-and-swap
    pub fn compare_exchange(&self, current: bool, new: bool) -> Result<bool, bool> {
        self.flag
            .compare_exchange(current, new, Ordering::AcqRel, Ordering::Acquire)
    }
}

impl Default for AtomicFlag {
    fn default() -> Self {
        Self::new(false)
    }
}

/// Compteur de références atomique
pub struct AtomicRefCount {
    count: AtomicUsize,
}

impl AtomicRefCount {
    /// Crée un nouveau compteur
    pub const fn new(initial: usize) -> Self {
        Self {
            count: AtomicUsize::new(initial),
        }
    }
    
    /// Incrémente le compteur
    pub fn inc(&self) -> usize {
        self.count.fetch_add(1, Ordering::Relaxed)
    }
    
    /// Décrémente le compteur et retourne la nouvelle valeur
    pub fn dec(&self) -> usize {
        self.count.fetch_sub(1, Ordering::AcqRel)
    }
    
    /// Récupère la valeur actuelle
    pub fn get(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }
    
    /// Vérifie si c'est la dernière référence
    pub fn is_last(&self) -> bool {
        self.get() == 1
    }
}

/// Statistiques atomiques pour monitoring
#[derive(Debug)]
pub struct AtomicStats {
    pub messages_sent: AtomicU64,
    pub messages_received: AtomicU64,
    pub bytes_sent: AtomicU64,
    pub bytes_received: AtomicU64,
    pub errors: AtomicU64,
}

impl AtomicStats {
    /// Crée de nouvelles statistiques
    pub const fn new() -> Self {
        Self {
            messages_sent: AtomicU64::new(0),
            messages_received: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
            errors: AtomicU64::new(0),
        }
    }
    
    /// Enregistre un message envoyé
    pub fn record_send(&self, bytes: u64) {
        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        self.bytes_sent.fetch_add(bytes, Ordering::Relaxed);
    }
    
    /// Enregistre un message reçu
    pub fn record_recv(&self, bytes: u64) {
        self.messages_received.fetch_add(1, Ordering::Relaxed);
        self.bytes_received.fetch_add(bytes, Ordering::Relaxed);
    }
    
    /// Enregistre une erreur
    pub fn record_error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Récupère un snapshot des statistiques
    pub fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            messages_sent: self.messages_sent.load(Ordering::Relaxed),
            messages_received: self.messages_received.load(Ordering::Relaxed),
            bytes_sent: self.bytes_sent.load(Ordering::Relaxed),
            bytes_received: self.bytes_received.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
        }
    }
    
    /// Reset les statistiques
    pub fn reset(&self) {
        self.messages_sent.store(0, Ordering::Relaxed);
        self.messages_received.store(0, Ordering::Relaxed);
        self.bytes_sent.store(0, Ordering::Relaxed);
        self.bytes_received.store(0, Ordering::Relaxed);
        self.errors.store(0, Ordering::Relaxed);
    }
}

impl Default for AtomicStats {
    fn default() -> Self {
        Self::new()
    }
}

/// Snapshot non-atomique des statistiques
#[derive(Debug, Clone, Copy)]
pub struct StatsSnapshot {
    pub messages_sent: u64,
    pub messages_received: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub errors: u64,
}

/// Atomique optimisé pour index de ring buffer
pub struct RingIndex {
    value: AtomicUsize,
}

impl RingIndex {
    /// Crée un nouvel index
    pub const fn new(initial: usize) -> Self {
        Self {
            value: AtomicUsize::new(initial),
        }
    }
    
    /// Charge la valeur (relaxed ordering)
    #[inline]
    pub fn load_relaxed(&self) -> usize {
        self.value.load(Ordering::Relaxed)
    }
    
    /// Charge la valeur (acquire ordering)
    #[inline]
    pub fn load_acquire(&self) -> usize {
        self.value.load(Ordering::Acquire)
    }
    
    /// Store la valeur (release ordering)
    #[inline]
    pub fn store_release(&self, val: usize) {
        self.value.store(val, Ordering::Release);
    }
    
    /// Incrémente et retourne l'ancienne valeur (wrapping)
    #[inline]
    pub fn fetch_add_wrapping(&self, delta: usize) -> usize {
        self.value.fetch_add(delta, Ordering::Release)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_backoff_progression() {
        let mut backoff = Backoff::new();
        assert_eq!(backoff.step, 0);
        
        backoff.spin();
        assert!(backoff.step > 0);
    }
    
    #[test]
    fn test_sequence_counter() {
        let counter = SequenceCounter::new();
        assert_eq!(counter.next(), 1);
        assert_eq!(counter.next(), 2);
        assert_eq!(counter.next(), 3);
        
        counter.reset();
        assert_eq!(counter.next(), 1);
    }
    
    #[test]
    fn test_atomic_flag() {
        let flag = AtomicFlag::new(false);
        assert!(!flag.is_set());
        
        flag.set();
        assert!(flag.is_set());
        
        assert!(flag.test_and_clear());
        assert!(!flag.is_set());
    }
    
    #[test]
    fn test_atomic_stats() {
        let stats = AtomicStats::new();
        
        stats.record_send(100);
        stats.record_send(200);
        stats.record_recv(150);
        
        let snapshot = stats.snapshot();
        assert_eq!(snapshot.messages_sent, 2);
        assert_eq!(snapshot.messages_received, 1);
        assert_eq!(snapshot.bytes_sent, 300);
        assert_eq!(snapshot.bytes_received, 150);
    }
}
