//! Park/unpark pour synchronisation de threads

use crate::sync::Mutex;
use crate::thread::ThreadId;

/// Parking state pour un thread
struct ParkState {
    unparked: bool,
}

/// Parker pour le thread courant
pub struct Parker {
    state: Mutex<ParkState>,
}

impl Parker {
    /// Crée un nouveau Parker
    pub fn new() -> Self {
        Self {
            state: Mutex::new(ParkState { unparked: false }),
        }
    }

    /// Park le thread courant
    pub fn park(&self) {
        let mut state = self.state.lock();
        
        while !state.unparked {
            // En pratique, on devrait bloquer ici
            // Pour l'instant, spin
            drop(state);
            crate::thread::yield_now();
            state = self.state.lock();
        }
        
        state.unparked = false;
    }

    /// Park avec timeout
    /// 
    /// Note: L'implémentation actuelle utilise un spin-wait avec yield.
    /// Une vraie implémentation nécessiterait un syscall de blocage avec timeout.
    pub fn park_timeout(&self, timeout: core::time::Duration) {
        use crate::time::Instant;
        
        let start = Instant::now();
        let mut state = self.state.lock();
        
        while !state.unparked {
            if start.elapsed() >= timeout {
                // Timeout atteint
                return;
            }
            
            drop(state);
            crate::thread::yield_now();
            state = self.state.lock();
        }
        
        state.unparked = false;
    }

    /// Unpark le thread
    pub fn unpark(&self) {
        let mut state = self.state.lock();
        state.unparked = true;
    }
}

impl Default for Parker {
    fn default() -> Self {
        Self::new()
    }
}

/// Park le thread courant
///
/// Note: Cette implémentation simplifiée utilise yield_now().
/// Une vraie implémentation nécessiterait:
/// - Un Parker par thread (via TLS)
/// - Un syscall de blocage/réveil
/// - Une table globale de threads parkés
pub fn park() {
    crate::thread::yield_now();
}

/// Park avec timeout
pub fn park_timeout(timeout: core::time::Duration) {
    let _timeout = timeout;
    park();
}

/// Unpark un thread
///
/// Note: Cette implémentation simplifiée est un no-op.
/// Une vraie implémentation nécessiterait:
/// - Une table globale associant ThreadId -> Parker
/// - Un syscall pour réveiller un thread bloqué
/// - Synchronisation pour éviter les races
pub fn unpark(_thread: ThreadId) {
    // No-op dans l'implémentation actuelle
    // Le thread cible doit être réveillé par le scheduler
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parker() {
        let parker = Parker::new();
        
        // Unpark avant park = pas de blocage
        parker.unpark();
        parker.park();
    }

    #[test]
    fn test_park_functions() {
        // Ne devrait pas bloquer en mode test
        park();
    }
}
