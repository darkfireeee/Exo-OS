// libs/exo_ipc/src/protocol/flow_control.rs
//! Flow control et backpressure pour éviter la saturation

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::util::cache::CachePadded;

/// Stratégie de flow control
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowStrategy {
    /// Pas de flow control (meilleure performance, risque de saturation)
    None,
    
    /// Token bucket (rate limiting)
    TokenBucket,
    
    /// Sliding window (débit variable)
    SlidingWindow,
    
    /// Credit-based (crédit de messages)
    CreditBased,
}

/// Contrôleur de flow avec token bucket
///
/// Implémente un algorithme de token bucket pour limiter le débit
pub struct TokenBucketFlowController {
    /// Nombre de tokens disponibles
    tokens: CachePadded<AtomicU64>,
    
    /// Capacité maximale du bucket
    capacity: u64,
    
    /// Taux de remplissage (tokens par seconde)
    refill_rate: u64,
    
    /// Dernier timestamp de refill
    last_refill: CachePadded<AtomicU64>,
    
    /// Nombre de messages acceptés
    accepted: CachePadded<AtomicUsize>,
    
    /// Nombre de messages rejetés
    rejected: CachePadded<AtomicUsize>,
}

impl TokenBucketFlowController {
    /// Crée un nouveau contrôleur
    ///
    /// # Arguments
    /// * `capacity` - Capacité maximale (burst size)
    /// * `refill_rate` - Taux de remplissage (tokens/sec)
    pub fn new(capacity: u64, refill_rate: u64) -> Self {
        Self {
            tokens: CachePadded::new(AtomicU64::new(capacity)),
            capacity,
            refill_rate,
            last_refill: CachePadded::new(AtomicU64::new(0)),
            accepted: CachePadded::new(AtomicUsize::new(0)),
            rejected: CachePadded::new(AtomicUsize::new(0)),
        }
    }
    
    /// Tente d'acquérir des tokens
    ///
    /// # Arguments
    /// * `count` - Nombre de tokens à acquérir
    /// * `current_time` - Timestamp actuel (en unités arbitraires)
    ///
    /// # Returns
    /// `true` si les tokens ont été acquis, `false` sinon
    pub fn try_acquire(&self, count: u64, current_time: u64) -> bool {
        // Refill les tokens basé sur le temps écoulé
        self.refill(current_time);
        
        let mut current_tokens = self.tokens.load(Ordering::Relaxed);
        
        loop {
            if current_tokens < count {
                self.rejected.fetch_add(1, Ordering::Relaxed);
                return false;
            }
            
            match self.tokens.compare_exchange_weak(
                current_tokens,
                current_tokens - count,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.accepted.fetch_add(1, Ordering::Relaxed);
                    return true;
                }
                Err(actual) => {
                    current_tokens = actual;
                }
            }
        }
    }
    
    /// Refill les tokens basé sur le temps écoulé
    fn refill(&self, current_time: u64) {
        let last = self.last_refill.load(Ordering::Relaxed);
        
        if current_time <= last {
            return;
        }
        
        // Calculer combien de tokens ajouter
        let elapsed = current_time - last;
        let tokens_to_add = (elapsed * self.refill_rate) / 1000; // Assume time in ms
        
        if tokens_to_add == 0 {
            return;
        }
        
        // Mise à jour atomique
        let mut current = self.tokens.load(Ordering::Relaxed);
        
        loop {
            let new_tokens = (current + tokens_to_add).min(self.capacity);
            
            match self.tokens.compare_exchange_weak(
                current,
                new_tokens,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.last_refill.store(current_time, Ordering::Release);
                    break;
                }
                Err(actual) => {
                    current = actual;
                }
            }
        }
    }
    
    /// Nombre de tokens disponibles
    pub fn available_tokens(&self) -> u64 {
        self.tokens.load(Ordering::Relaxed)
    }
    
    /// Taux d'acceptation (0.0 - 1.0)
    pub fn acceptance_rate(&self) -> f32 {
        let accepted = self.accepted.load(Ordering::Relaxed) as f32;
        let rejected = self.rejected.load(Ordering::Relaxed) as f32;
        let total = accepted + rejected;
        
        if total > 0.0 {
            accepted / total
        } else {
            1.0
        }
    }
    
    /// Reset les statistiques
    pub fn reset_stats(&self) {
        self.accepted.store(0, Ordering::Relaxed);
        self.rejected.store(0, Ordering::Relaxed);
    }
}

/// Contrôleur de flow avec fenêtre glissante
pub struct SlidingWindowFlowController {
    /// Taille de la fenêtre (nombre de messages autorisés dans la période)
    window_size: usize,
    
    /// Période de la fenêtre (en unités de temps)
    window_period: u64,
    
    /// Compteur de messages dans la fenêtre actuelle
    current_count: CachePadded<AtomicUsize>,
    
    /// Timestamp de début de la fenêtre actuelle
    window_start: CachePadded<AtomicU64>,
}

impl SlidingWindowFlowController {
    /// Crée un nouveau contrôleur
    pub fn new(window_size: usize, window_period: u64) -> Self {
        Self {
            window_size,
            window_period,
            current_count: CachePadded::new(AtomicUsize::new(0)),
            window_start: CachePadded::new(AtomicU64::new(0)),
        }
    }
    
    /// Tente d'autoriser un message
    pub fn try_allow(&self, current_time: u64) -> bool {
        let window_start = self.window_start.load(Ordering::Relaxed);
        
        // Vérifier si on est dans une nouvelle fenêtre
        if current_time >= window_start + self.window_period {
            // Nouvelle fenêtre - reset
            self.window_start.store(current_time, Ordering::Release);
            self.current_count.store(1, Ordering::Release);
            return true;
        }
        
        // Fenêtre actuelle - vérifier le quota
        let count = self.current_count.fetch_add(1, Ordering::Relaxed);
        
        if count < self.window_size {
            true
        } else {
            // Quota dépassé - annuler l'incrémentation
            self.current_count.fetch_sub(1, Ordering::Relaxed);
            false
        }
    }
    
    /// Nombre de messages autorisés restants dans la fenêtre
    pub fn remaining(&self) -> usize {
        let count = self.current_count.load(Ordering::Relaxed);
        self.window_size.saturating_sub(count)
    }
}

/// Contrôleur de flow basé sur crédits
pub struct CreditBasedFlowController {
    /// Crédits disponibles
    credits: CachePadded<AtomicUsize>,
    
    /// Crédits maximum
    max_credits: usize,
}

impl CreditBasedFlowController {
    /// Crée un nouveau contrôleur
    pub fn new(initial_credits: usize) -> Self {
        Self {
            credits: CachePadded::new(AtomicUsize::new(initial_credits)),
            max_credits: initial_credits,
        }
    }
    
    /// Tente de consommer des crédits
    pub fn try_consume(&self, count: usize) -> bool {
        let mut current = self.credits.load(Ordering::Relaxed);
        
        loop {
            if current < count {
                return false;
            }
            
            match self.credits.compare_exchange_weak(
                current,
                current - count,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(actual) => current = actual,
            }
        }
    }
    
    /// Ajoute des crédits (appelé lors d'ACK)
    pub fn add_credits(&self, count: usize) {
        let mut current = self.credits.load(Ordering::Relaxed);
        
        loop {
            let new_credits = (current + count).min(self.max_credits);
            
            match self.credits.compare_exchange_weak(
                current,
                new_credits,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }
    
    /// Crédits disponibles
    pub fn available(&self) -> usize {
        self.credits.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_token_bucket() {
        let controller = TokenBucketFlowController::new(10, 5);
        
        // Devrait avoir 10 tokens initialement
        assert!(controller.try_acquire(5, 0));
        assert!(controller.try_acquire(5, 0));
        
        // Devrait être vide maintenant
        assert!(!controller.try_acquire(1, 0));
        
        // Après refill (1000ms = 5 tokens)
        assert!(controller.try_acquire(1, 1000));
    }
    
    #[test]
    fn test_sliding_window() {
        let controller = SlidingWindowFlowController::new(5, 1000);
        
        // Devrait autoriser 5 messages
        for _ in 0..5 {
            assert!(controller.try_allow(0));
        }
        
        // Le 6ème devrait être rejeté
        assert!(!controller.try_allow(0));
        
        // Nouvelle fenêtre
        assert!(controller.try_allow(1000));
    }
    
    #[test]
    fn test_credit_based() {
        let controller = CreditBasedFlowController::new(10);
        
        assert!(controller.try_consume(5));
        assert_eq!(controller.available(), 5);
        
        assert!(controller.try_consume(5));
        assert_eq!(controller.available(), 0);
        
        assert!(!controller.try_consume(1));
        
        controller.add_credits(3);
        assert_eq!(controller.available(), 3);
    }
}
