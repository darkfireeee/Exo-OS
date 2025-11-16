//! # Loopback Driver - Driver Virtuel pour Démo Adaptive Framework
//! 
//! Driver minimal simulant un périphérique avec charge variable pour tester
//! l'auto-adaptation polling/interrupt de l'AdaptiveController.

use crate::drivers::adaptive_driver::{AdaptiveController, DriverMode};
use crate::perf_counters::rdtsc;
use crate::println;
use core::sync::atomic::{AtomicU64, Ordering};

/// Driver loopback avec simulation de charge
pub struct LoopbackDriver {
    /// Nombre de paquets simulés reçus
    packets_received: AtomicU64,
    
    /// Mode actuel (stub)
    mode: DriverMode,
}

impl LoopbackDriver {
    /// Crée un nouveau driver loopback
    pub fn new() -> Self {
        Self {
            packets_received: AtomicU64::new(0),
            mode: DriverMode::Interrupt,
        }
    }
    
    /// Simule la réception d'un paquet (appel répété pour générer charge)
    pub fn receive_packet(&mut self) -> Option<u64> {
        let packet_id = self.packets_received.fetch_add(1, Ordering::SeqCst);
        Some(packet_id)
    }
    
    /// Force un changement de mode (pour test)
    pub fn set_mode(&mut self, mode: DriverMode) {
        self.mode = mode;
    }
    
    /// Retourne le mode actuel
    pub fn current_mode(&self) -> DriverMode {
        self.mode
    }
    
    /// Retourne les statistiques
    pub fn stats(&self) -> (u64, u64, u64) {
        let packets = self.packets_received.load(Ordering::Relaxed);
        (packets, packets, 0) // Stub
    }
    
    /// Affiche un rapport de performance
    pub fn print_report(&self) {
        let (packets, ops, switches) = self.stats();
        let mode = self.current_mode();
        
        println!("[LoopbackDriver] Rapport:");
        println!("  Paquets reçus: {}", packets);
        println!("  Opérations totales: {}", ops);
        println!("  Switches de mode: {}", switches);
        println!("  Mode actuel: {}", mode.name());
    }
}

/// Test du driver loopback avec montée en charge
pub fn demo_load_test() {
    println!("\n[DEMO] Loopback Driver - Test de charge adaptative");
    
    let mut driver = LoopbackDriver::new();
    
    // Phase 1: Basse charge (devrait rester en Interrupt)
    println!("\n[Phase 1] Basse charge (10 paquets)...");
    driver.set_mode(DriverMode::Interrupt);
    for _ in 0..10 {
        driver.receive_packet();
    }
    println!("  Mode après phase 1: {}", driver.current_mode().name());
    
    // Phase 2: Charge moyenne (devrait passer à Hybrid)
    println!("\n[Phase 2] Charge moyenne (500 paquets)...");
    for _ in 0..500 {
        driver.receive_packet();
    }
    println!("  Mode après phase 2: {}", driver.current_mode().name());
    
    // Phase 3: Haute charge (devrait passer à Polling)
    println!("\n[Phase 3] Haute charge (5000 paquets)...");
    for _ in 0..5000 {
        driver.receive_packet();
    }
    println!("  Mode après phase 3: {}", driver.current_mode().name());
    
    // Phase 4: Retour à basse charge (devrait revenir à Interrupt)
    println!("\n[Phase 4] Retour à basse charge (10 paquets)...");
    for _ in 0..10 {
        driver.receive_packet();
    }
    println!("  Mode après phase 4: {}", driver.current_mode().name());
    
    // Rapport final
    println!("\n");
    driver.print_report();
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_loopback_driver_create() {
        let driver = LoopbackDriver::new();
        assert_eq!(driver.current_mode(), DriverMode::Interrupt); // Mode par défaut
    }
    
    #[test]
    fn test_loopback_driver_receive() {
        let mut driver = LoopbackDriver::new();
        
        // Première réception
        let packet1 = driver.receive_packet();
        assert_eq!(packet1, Some(0));
        
        // Deuxième réception
        let packet2 = driver.receive_packet();
        assert_eq!(packet2, Some(1));
        
        let (packets, _, _) = driver.stats();
        assert_eq!(packets, 2);
    }
    
    #[test]
    fn test_loopback_driver_mode_switch() {
        let mut driver = LoopbackDriver::new();
        
        // Force mode polling
        driver.set_mode(DriverMode::Polling);
        assert_eq!(driver.current_mode(), DriverMode::Polling);
        
        // Force mode batch
        driver.set_mode(DriverMode::Batch);
        assert_eq!(driver.current_mode(), DriverMode::Batch);
    }
}
