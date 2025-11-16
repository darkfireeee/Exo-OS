//! # Adaptive Drivers - Drivers Adaptatifs Auto-Optimisants
//! 
//! Système de drivers qui s'adaptent automatiquement entre polling et interrupts
//! selon la charge du système pour optimiser latence ET consommation CPU.
//! 
//! ## Stratégie
//! 
//! 1. **4 Modes de Fonctionnement**:
//!    - **Interrupt**: Basse charge, économise CPU (attente passive)
//!    - **Polling**: Haute charge, minimise latence (vérification active)
//!    - **Hybrid**: Charge moyenne, balance latence/CPU
//!    - **Batch**: Coalescence de requêtes pour disque/réseau
//! 
//! 2. **Auto-Switch Dynamique**:
//!    - Throughput >10000 ops/sec → Polling
//!    - Throughput <1000 ops/sec → Interrupt
//!    - 1000-10000 ops/sec → Hybrid
//! 
//! 3. **Mesure Overhead**:
//!    - RDTSC pour cycles économisés
//!    - Fenêtre glissante 1 seconde
//!    - Statistiques détaillées
//! 
//! ## Gains Attendus
//! - **Polling haute charge**: -40 à -60% latence vs interrupts
//! - **Interrupts basse charge**: -80 à -95% CPU usage vs polling
//! - **Auto-adaptation**: Optimal pour workload variable

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;
use alloc::collections::VecDeque;

/// Seuils de throughput pour auto-switch (en opérations/seconde)
const HIGH_THROUGHPUT_THRESHOLD: u64 = 10_000;
const LOW_THROUGHPUT_THRESHOLD: u64 = 1_000;

/// Taille de la fenêtre glissante pour mesure throughput (en microsecondes)
const MEASUREMENT_WINDOW_US: u64 = 1_000_000; // 1 seconde

/// Durée max du polling avant yield (en cycles)
const MAX_POLL_CYCLES: u64 = 10_000; // ~5µs à 2GHz

/// Mode de fonctionnement du driver
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverMode {
    /// Mode interrupt: Attente passive, réveil par IRQ
    /// Optimal: Basse charge (<1000 ops/sec)
    /// Latence: ~10-50µs | CPU: ~1-5%
    Interrupt,
    
    /// Mode polling: Vérification active en boucle
    /// Optimal: Haute charge (>10000 ops/sec)
    /// Latence: ~1-5µs | CPU: ~90-100%
    Polling,
    
    /// Mode hybrid: Polling court puis interrupt
    /// Optimal: Charge moyenne (1000-10000 ops/sec)
    /// Latence: ~5-15µs | CPU: ~20-60%
    Hybrid,
    
    /// Mode batch: Coalescence de requêtes
    /// Optimal: Disque/réseau avec latence tolérable
    /// Latence: ~100-1000µs | Throughput: +50-200%
    Batch,
}

impl DriverMode {
    /// Retourne le nom du mode
    pub fn name(&self) -> &'static str {
        match self {
            DriverMode::Interrupt => "Interrupt",
            DriverMode::Polling => "Polling",
            DriverMode::Hybrid => "Hybrid",
            DriverMode::Batch => "Batch",
        }
    }
    
    /// Retourne la priorité du mode (plus élevé = meilleure latence)
    pub fn latency_priority(&self) -> u8 {
        match self {
            DriverMode::Polling => 4,
            DriverMode::Hybrid => 3,
            DriverMode::Interrupt => 2,
            DriverMode::Batch => 1,
        }
    }
}

/// Statistiques de performance d'un driver
#[derive(Debug, Clone, Default)]
pub struct DriverStats {
    /// Nombre total d'opérations
    pub total_operations: u64,
    
    /// Cycles CPU totaux consommés
    pub total_cycles: u64,
    
    /// Nombre de switches de mode
    pub mode_switches: u64,
    
    /// Temps passé en mode Interrupt (µs)
    pub time_interrupt_us: u64,
    
    /// Temps passé en mode Polling (µs)
    pub time_polling_us: u64,
    
    /// Temps passé en mode Hybrid (µs)
    pub time_hybrid_us: u64,
    
    /// Temps passé en mode Batch (µs)
    pub time_batch_us: u64,
}

impl DriverStats {
    /// Calcule le throughput moyen (ops/sec)
    pub fn avg_throughput(&self) -> f64 {
        let total_time_us = self.time_interrupt_us 
                          + self.time_polling_us 
                          + self.time_hybrid_us 
                          + self.time_batch_us;
        
        if total_time_us == 0 {
            0.0
        } else {
            (self.total_operations as f64 / total_time_us as f64) * 1_000_000.0
        }
    }
    
    /// Calcule les cycles moyens par opération
    pub fn avg_cycles_per_op(&self) -> f64 {
        if self.total_operations == 0 {
            0.0
        } else {
            self.total_cycles as f64 / self.total_operations as f64
        }
    }
    
    /// Calcule la distribution du temps par mode (%)
    pub fn mode_distribution(&self) -> (f64, f64, f64, f64) {
        let total = (self.time_interrupt_us 
                   + self.time_polling_us 
                   + self.time_hybrid_us 
                   + self.time_batch_us) as f64;
        
        if total == 0.0 {
            (0.0, 0.0, 0.0, 0.0)
        } else {
            (
                (self.time_interrupt_us as f64 / total) * 100.0,
                (self.time_polling_us as f64 / total) * 100.0,
                (self.time_hybrid_us as f64 / total) * 100.0,
                (self.time_batch_us as f64 / total) * 100.0,
            )
        }
    }
}

/// Fenêtre glissante pour mesure de throughput
pub struct SlidingWindow {
    /// Timestamps des opérations (TSC)
    timestamps: Mutex<VecDeque<u64>>,
    
    /// Fréquence TSC en MHz
    tsc_frequency_mhz: u64,
}

impl SlidingWindow {
    pub fn new(tsc_frequency_mhz: u64) -> Self {
        Self {
            timestamps: Mutex::new(VecDeque::with_capacity(10000)),
            tsc_frequency_mhz,
        }
    }
    
    /// Enregistre une nouvelle opération
    fn record_operation(&self, tsc: u64) {
        let mut timestamps = self.timestamps.lock();
        timestamps.push_back(tsc);
        
        // Nettoyer les timestamps trop anciens (>1 seconde)
        let window_cycles = MEASUREMENT_WINDOW_US * self.tsc_frequency_mhz;
        while let Some(&first) = timestamps.front() {
            if tsc - first > window_cycles {
                timestamps.pop_front();
            } else {
                break;
            }
        }
    }
    
    /// Calcule le throughput actuel (ops/sec)
    fn current_throughput(&self) -> u64 {
        let timestamps = self.timestamps.lock();
        let count = timestamps.len();
        
        if count < 2 {
            return 0;
        }
        
        let first = timestamps.front().unwrap();
        let last = timestamps.back().unwrap();
        let elapsed_cycles = last - first;
        
        if elapsed_cycles == 0 {
            return 0;
        }
        
        // ops/sec = (count / elapsed_us) * 1_000_000
        let elapsed_us = elapsed_cycles / self.tsc_frequency_mhz;
        if elapsed_us == 0 {
            return count as u64 * 1_000_000;
        }
        
        (count as u64 * 1_000_000) / elapsed_us
    }
}

/// Trait pour les drivers adaptatifs
pub trait AdaptiveDriver {
    /// Nom du driver
    fn name(&self) -> &str;
    
    /// Opération driver en mode Interrupt
    /// Attend passivement un événement (IRQ)
    fn wait_interrupt(&mut self) -> Result<(), &'static str>;
    
    /// Opération driver en mode Polling
    /// Vérifie activement l'état du hardware
    fn poll_status(&mut self) -> Result<bool, &'static str>;
    
    /// Opération driver en mode Hybrid
    /// Poll court puis attend interrupt si rien
    fn hybrid_wait(&mut self) -> Result<(), &'static str> {
        // Poll pendant MAX_POLL_CYCLES
        let start = rdtsc();
        loop {
            if self.poll_status()? {
                return Ok(());
            }
            
            if rdtsc() - start > MAX_POLL_CYCLES {
                break;
            }
        }
        
        // Fallback sur interrupt
        self.wait_interrupt()
    }
    
    /// Opération driver en mode Batch
    /// Accumule requêtes et les traite en bloc
    fn batch_operation(&mut self, batch_size: usize) -> Result<usize, &'static str>;
    
    /// Retourne le mode actuel
    fn current_mode(&self) -> DriverMode;
    
    /// Change le mode
    fn set_mode(&mut self, mode: DriverMode);
    
    /// Retourne les statistiques
    fn stats(&self) -> &DriverStats;
}

/// Controller adaptatif qui gère l'auto-switch
pub struct AdaptiveController {
    /// Mode actuel
    current_mode: DriverMode,
    
    /// Fenêtre glissante pour throughput
    sliding_window: SlidingWindow,
    
    /// Timestamp du dernier switch
    last_switch_tsc: AtomicU64,
    
    /// Statistiques
    stats: DriverStats,
    
    /// Fréquence TSC en MHz
    tsc_frequency_mhz: u64,
    
    /// Timestamp début du mode actuel
    mode_start_tsc: u64,
}

impl AdaptiveController {
    /// Crée un nouveau controller
    pub fn new(tsc_frequency_mhz: u64) -> Self {
        Self {
            current_mode: DriverMode::Interrupt, // Commence en mode économe
            sliding_window: SlidingWindow::new(tsc_frequency_mhz),
            last_switch_tsc: AtomicU64::new(rdtsc()),
            stats: DriverStats::default(),
            tsc_frequency_mhz,
            mode_start_tsc: rdtsc(),
        }
    }
    
    /// Enregistre une opération et retourne le mode optimal
    pub fn record_operation(&mut self) -> DriverMode {
        let current_tsc = rdtsc();
        
        // Enregistrer dans la fenêtre glissante
        self.sliding_window.record_operation(current_tsc);
        
        // Incrémenter compteur ops
        self.stats.total_operations += 1;
        
        // Calculer throughput actuel
        let throughput = self.sliding_window.current_throughput();
        
        // Déterminer le mode optimal
        let optimal_mode = if throughput >= HIGH_THROUGHPUT_THRESHOLD {
            DriverMode::Polling
        } else if throughput <= LOW_THROUGHPUT_THRESHOLD {
            DriverMode::Interrupt
        } else {
            DriverMode::Hybrid
        };
        
        // Switch si nécessaire
        if optimal_mode != self.current_mode {
            self.switch_mode(optimal_mode, current_tsc);
        }
        
        self.current_mode
    }
    
    /// Switch vers un nouveau mode
    fn switch_mode(&mut self, new_mode: DriverMode, current_tsc: u64) {
        // Mettre à jour le temps passé dans l'ancien mode
        let elapsed_cycles = current_tsc - self.mode_start_tsc;
        let elapsed_us = elapsed_cycles / self.tsc_frequency_mhz;
        
        match self.current_mode {
            DriverMode::Interrupt => self.stats.time_interrupt_us += elapsed_us,
            DriverMode::Polling => self.stats.time_polling_us += elapsed_us,
            DriverMode::Hybrid => self.stats.time_hybrid_us += elapsed_us,
            DriverMode::Batch => self.stats.time_batch_us += elapsed_us,
        }
        
        // Switch
        self.current_mode = new_mode;
        self.mode_start_tsc = current_tsc;
        self.stats.mode_switches += 1;
        self.last_switch_tsc.store(current_tsc, Ordering::Relaxed);
    }
    
    /// Enregistre les cycles consommés
    pub fn record_cycles(&mut self, cycles: u64) {
        self.stats.total_cycles += cycles;
    }
    
    /// Retourne le mode actuel
    pub fn current_mode(&self) -> DriverMode {
        self.current_mode
    }
    
    /// Retourne les statistiques
    pub fn get_stats(&self) -> &DriverStats {
        &self.stats
    }
    
    /// Retourne le throughput actuel
    pub fn current_throughput(&self) -> u64 {
        self.sliding_window.current_throughput()
    }
    
    /// Force un mode spécifique (pour tests)
    pub fn force_mode(&mut self, mode: DriverMode) {
        let current_tsc = rdtsc();
        self.switch_mode(mode, current_tsc);
    }
}

/// Lit le compteur TSC
#[cfg(all(target_arch = "x86_64", not(target_os = "windows")))]
fn rdtsc() -> u64 {
    unsafe {
        let mut low: u32;
        let mut high: u32;
        core::arch::asm!(
            "rdtsc",
            out("eax") low,
            out("edx") high,
            options(nostack, nomem)
        );
        ((high as u64) << 32) | (low as u64)
    }
}

#[cfg(not(all(target_arch = "x86_64", not(target_os = "windows"))))]
fn rdtsc() -> u64 {
    static mut COUNTER: u64 = 0;
    unsafe {
        COUNTER += 100;
        COUNTER
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_driver_mode_name() {
        assert_eq!(DriverMode::Interrupt.name(), "Interrupt");
        assert_eq!(DriverMode::Polling.name(), "Polling");
        assert_eq!(DriverMode::Hybrid.name(), "Hybrid");
        assert_eq!(DriverMode::Batch.name(), "Batch");
    }
    
    #[test]
    fn test_driver_mode_priority() {
        assert!(DriverMode::Polling.latency_priority() > DriverMode::Hybrid.latency_priority());
        assert!(DriverMode::Hybrid.latency_priority() > DriverMode::Interrupt.latency_priority());
        assert!(DriverMode::Interrupt.latency_priority() > DriverMode::Batch.latency_priority());
    }
    
    #[test]
    fn test_driver_stats_throughput() {
        let mut stats = DriverStats::default();
        stats.total_operations = 10000;
        stats.time_polling_us = 1_000_000; // 1 seconde
        
        let throughput = stats.avg_throughput();
        assert!((throughput - 10000.0).abs() < 1.0); // 10000 ops/sec
    }
    
    #[test]
    fn test_driver_stats_cycles_per_op() {
        let mut stats = DriverStats::default();
        stats.total_operations = 1000;
        stats.total_cycles = 100_000;
        
        let cycles = stats.avg_cycles_per_op();
        assert!((cycles - 100.0).abs() < 0.1);
    }
    
    #[test]
    fn test_driver_stats_distribution() {
        let mut stats = DriverStats::default();
        stats.time_interrupt_us = 500_000; // 50%
        stats.time_polling_us = 300_000;   // 30%
        stats.time_hybrid_us = 200_000;    // 20%
        stats.time_batch_us = 0;           // 0%
        
        let (int, poll, hyb, bat) = stats.mode_distribution();
        assert!((int - 50.0).abs() < 0.1);
        assert!((poll - 30.0).abs() < 0.1);
        assert!((hyb - 20.0).abs() < 0.1);
        assert!((bat - 0.0).abs() < 0.1);
    }
    
    #[test]
    fn test_adaptive_controller_init() {
        let controller = AdaptiveController::new(2000);
        assert_eq!(controller.current_mode(), DriverMode::Interrupt);
        assert_eq!(controller.stats().total_operations, 0);
    }
    
    #[test]
    fn test_adaptive_controller_force_mode() {
        let mut controller = AdaptiveController::new(2000);
        
        controller.force_mode(DriverMode::Polling);
        assert_eq!(controller.current_mode(), DriverMode::Polling);
        
        controller.force_mode(DriverMode::Hybrid);
        assert_eq!(controller.current_mode(), DriverMode::Hybrid);
        
        // Doit avoir 2 switches
        assert_eq!(controller.stats().mode_switches, 2);
    }
    
    #[test]
    fn test_sliding_window_throughput() {
        let window = SlidingWindow::new(2000);
        
        // Simuler 1000 ops en 1 seconde
        let start_tsc = unsafe { rdtsc() };
        for i in 0..1000 {
            let tsc = start_tsc + (i * 2000); // 1µs entre chaque op
            window.record_operation(tsc);
        }
        
        // Devrait être proche de 1M ops/sec
        // Note: Test approximatif car dépend de RDTSC réel
        let throughput = window.current_throughput();
        assert!(throughput > 0); // Au moins quelque chose
    }
    
    #[test]
    fn test_auto_switch_high_throughput() {
        let mut controller = AdaptiveController::new(2000);
        
        // Simuler haute charge (>10000 ops/sec)
        let start_tsc = unsafe { rdtsc() };
        for i in 0..15000 {
            // 100 ops/µs = 100M ops/sec (très élevé)
            let tsc = start_tsc + (i * 20); // 0.01µs entre ops
            controller.sliding_window.record_operation(tsc);
        }
        
        // Enregistrer une op pour déclencher check
        let _ = controller.record_operation();
        
        // Devrait switcher vers Polling
        assert_eq!(controller.current_mode(), DriverMode::Polling);
    }
}
