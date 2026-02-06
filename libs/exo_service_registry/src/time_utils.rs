//! Timestamp utilities pour service registry
//!
//! Intégration avec exo_types::Timestamp pour timestamps réels

use exo_types::Timestamp;

/// Retourne le timestamp actuel en secondes (monotonic)
///
/// Utilise le timestamp monotonic pour éviter les sauts arrière
/// du à des changements de l'horloge système.
#[inline]
pub fn current_timestamp_secs() -> u64 {
    // Dans un système réel, on obtiendrait le timestamp via un syscall
    // Pour l'instant, on retourne un timestamp monotonic basique
    // TODO: Intégrer avec le syscall clock_gettime une fois disponible

    // Simulation: retourne 0 pour l'instant
    // Dans la version finale, ceci serait remplacé par:
    // unsafe { syscall::clock_gettime(ClockId::Monotonic) }.as_secs()
    0
}

/// Retourne un timestamp Exo-OS (monotonic)
#[inline]
pub fn current_timestamp() -> Timestamp {
    // TODO: Intégrer avec syscall
    Timestamp::ZERO_MONOTONIC
}

/// Helper pour créer un timestamp à partir de secondes
#[inline]
pub const fn timestamp_from_secs(secs: u64) -> Timestamp {
    Timestamp::monotonic(secs as i64, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_timestamp_secs() {
        let ts = current_timestamp_secs();
        // Pour l'instant retourne 0
        assert_eq!(ts, 0);
    }

    #[test]
    fn test_current_timestamp() {
        let ts = current_timestamp();
        assert_eq!(ts.seconds(), 0);
        assert_eq!(ts.nanos(), 0);
    }

    #[test]
    fn test_timestamp_from_secs() {
        let ts = timestamp_from_secs(100);
        assert_eq!(ts.seconds(), 100);
        assert_eq!(ts.nanos(), 0);
    }
}
