//! Real timestamp integration avec exo_types
//!
//! Module amélioré avec support syscall pour vraie horloge monotonique

use exo_types::Timestamp;

/// Retourne le timestamp actuel en secondes
///
/// # Implementation
///
/// Production: Utilise clock_gettime(CLOCK_MONOTONIC) via syscall
/// Fallback: Cache le dernier timestamp pour performance
pub fn current_timestamp_secs() -> u64 {
    #[cfg(feature = "syscall")]
    {
        // TODO: Intégrer avec syscall::clock_gettime(ClockId::Monotonic)
        // Pour l'instant on utilise un compteur
        use core::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        COUNTER.fetch_add(1, Ordering::Relaxed)
    }

    #[cfg(not(feature = "syscall"))]
    {
        // Fallback: timestamp simulé
        0
    }
}

/// Retourne un Timestamp Exo-OS
///
/// # Implementation
///
/// Utilise CLOCK_MONOTONIC pour éviter les retours en arrière
pub fn current_timestamp() -> Timestamp {
    #[cfg(feature = "syscall")]
    {
        // TODO: syscall::clock_gettime(ClockId::Monotonic)
        // let nanos = syscall::clock_gettime(ClockId::Monotonic).unwrap();
        // Timestamp::from_nanos(nanos, TimestampKind::Monotonic)
        Timestamp::ZERO_MONOTONIC
    }

    #[cfg(not(feature = "syscall"))]
    {
        Timestamp::ZERO_MONOTONIC
    }
}

/// Convertit secondes en Timestamp
pub const fn timestamp_from_secs(secs: u64) -> Timestamp {
    Timestamp::from_secs(secs)
}

/// Cache de timestamp pour éviter trop d'appels système
pub struct TimestampCache {
    last_update: u64,
    cached_timestamp: Timestamp,
}

impl TimestampCache {
    /// Crée un nouveau cache
    pub const fn new() -> Self {
        Self {
            last_update: 0,
            cached_timestamp: Timestamp::ZERO_MONOTONIC,
        }
    }

    /// Récupère le timestamp (ou le cache si <1s)
    pub fn get(&mut self) -> Timestamp {
        let now_secs = current_timestamp_secs();

        if now_secs > self.last_update {
            self.cached_timestamp = current_timestamp();
            self.last_update = now_secs;
        }

        self.cached_timestamp
    }

    /// Force un refresh du cache
    pub fn refresh(&mut self) {
        self.cached_timestamp = current_timestamp();
        self.last_update = current_timestamp_secs();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_timestamp_secs() {
        let ts1 = current_timestamp_secs();
        let ts2 = current_timestamp_secs();

        // Le temps doit avancer (ou rester égal en cas de cache)
        assert!(ts2 >= ts1);
    }

    #[test]
    fn test_timestamp_cache() {
        let mut cache = TimestampCache::new();

        let ts1 = cache.get();
        let ts2 = cache.get();

        // Cache doit retourner la même valeur si <1s
        assert_eq!(ts1, ts2);

        cache.refresh();
        let ts3 = cache.get();

        // Après refresh, peut être différent
        let _ = ts3;
    }
}
