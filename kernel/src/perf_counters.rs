//! Module de mesure de performance pour le noyau Exo-OS
//!
//! Ce module fournit des outils pour mesurer et analyser les performances
//! des différents composants du noyau en utilisant les compteurs CPU.

use alloc::string::{String, ToString};
use core::sync::atomic::{AtomicU64, Ordering};
use crate::{println, print};

/// Compteur de temps basé sur les cycles CPU (RDTSC)
#[inline(always)]
pub fn rdtsc() -> u64 {
    unsafe {
        core::arch::x86_64::_rdtsc()
    }
}

/// Types de composants à mesurer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Component {
    Vga,
    Interrupts,
    Scheduler,
    Memory,
    Syscall,
    Ipc,
    Drivers,
    KernelBoot,
    Unknown,
}

impl Component {
    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => Component::Vga,
            1 => Component::Interrupts,
            2 => Component::Scheduler,
            3 => Component::Memory,
            4 => Component::Syscall,
            5 => Component::Ipc,
            6 => Component::Drivers,
            7 => Component::KernelBoot,
            _ => Component::Unknown,
        }
    }
    
    pub fn as_u8(&self) -> u8 {
        match self {
            Component::Vga => 0,
            Component::Interrupts => 1,
            Component::Scheduler => 2,
            Component::Memory => 3,
            Component::Syscall => 4,
            Component::Ipc => 5,
            Component::Drivers => 6,
            Component::KernelBoot => 7,
            Component::Unknown => 255,
        }
    }
}

/// Statistiques de performance pour un composant
pub struct ComponentStats {
    pub component: Component,
    pub total_calls: AtomicU64,
    pub total_cycles: AtomicU64,
    pub min_cycles: AtomicU64,
    pub max_cycles: AtomicU64,
}

impl ComponentStats {
    pub const fn new(component: Component) -> Self {
        Self {
            component,
            total_calls: AtomicU64::new(0),
            total_cycles: AtomicU64::new(0),
            min_cycles: AtomicU64::new(u64::MAX),
            max_cycles: AtomicU64::new(0),
        }
    }
    
    /// Enregistre une mesure de performance
    pub fn record(&self, cycles: u64) {
        // Incrémenter le compteur d'appels
        self.total_calls.fetch_add(1, Ordering::Relaxed);
        
        // Ajouter au total des cycles
        self.total_cycles.fetch_add(cycles, Ordering::Relaxed);
        
        // Mettre à jour le minimum
        let current_min = self.min_cycles.load(Ordering::Relaxed);
        if cycles < current_min {
            self.min_cycles.store(cycles, Ordering::Relaxed);
        }
        
        // Mettre à jour le maximum
        let current_max = self.max_cycles.load(Ordering::Relaxed);
        if cycles > current_max {
            self.max_cycles.store(cycles, Ordering::Relaxed);
        }
    }
    
    /// Calcule la moyenne des cycles
    pub fn average_cycles(&self) -> u64 {
        let calls = self.total_calls.load(Ordering::Relaxed);
        let total = self.total_cycles.load(Ordering::Relaxed);
        if calls > 0 {
            total / calls
        } else {
            0
        }
    }
    
    /// Génère un rapport de performance
    pub fn generate_report(&self) -> String {
        let calls = self.total_calls.load(Ordering::Relaxed);
        let total = self.total_cycles.load(Ordering::Relaxed);
        let min = self.min_cycles.load(Ordering::Relaxed);
        let max = self.max_cycles.load(Ordering::Relaxed);
        let avg = self.average_cycles();
        
        let component_name = match self.component {
            Component::Vga => "VGA Display",
            Component::Interrupts => "Interrupts",
            Component::Scheduler => "Scheduler",
            Component::Memory => "Memory",
            Component::Syscall => "Syscall",
            Component::Ipc => "IPC",
            Component::Drivers => "Drivers",
            Component::KernelBoot => "Kernel Boot",
            Component::Unknown => "Unknown",
        };
        
        let mut report = String::new();
        report.push_str("=== ");
        report.push_str(component_name);
        report.push_str(" ===\n");
        report.push_str("  Appels: ");
        report.push_str(&calls.to_string());
        report.push_str("\n  Cycles total: ");
        report.push_str(&total.to_string());
        report.push_str("\n  Cycles min: ");
        if min == u64::MAX {
            report.push_str("0");
        } else {
            report.push_str(&min.to_string());
        }
        report.push_str("\n  Cycles max: ");
        report.push_str(&max.to_string());
        report.push_str("\n  Cycles moyen: ");
        report.push_str(&avg.to_string());
        report.push_str("\n  Temps estimé*: ");
        
        // Remplacer format! par une concatenation manuelle
        let time_us = (avg as f64) / 3000.0; // Estimation à 3 GHz
        report.push_str(&time_us.to_string());
        report.push_str(" µs\n");
        
        report
    }
}

/// Gestionnaire global des statistiques de performance
pub struct PerformanceManager {
    stats: [ComponentStats; 8],
}

impl PerformanceManager {
    pub const fn new() -> Self {
        Self {
            stats: [
                ComponentStats::new(Component::Vga),
                ComponentStats::new(Component::Interrupts),
                ComponentStats::new(Component::Scheduler),
                ComponentStats::new(Component::Memory),
                ComponentStats::new(Component::Syscall),
                ComponentStats::new(Component::Ipc),
                ComponentStats::new(Component::Drivers),
                ComponentStats::new(Component::KernelBoot),
            ],
        }
    }
    
    /// Enregistre une mesure pour un composant
    pub fn record(&self, component: Component, cycles: u64) {
        let index = component.as_u8() as usize;
        if index < self.stats.len() {
            self.stats[index].record(cycles);
        }
    }
    
    /// Génère un rapport complet
    pub fn generate_full_report(&self) -> String {
        let mut report = String::new();
        report.push_str("========== RAPPORT DE PERFORMANCE EXO-OS ==========\n");
        report.push_str("Fréquence CPU estimée: 3.0 GHz\n");
        report.push_str("* Les temps sont des estimations basées sur 3 GHz\n");
        report.push_str("====================================================\n\n");
        
        for stats in &self.stats {
            let calls = stats.total_calls.load(Ordering::Relaxed);
            if calls > 0 {
                report.push_str(&stats.generate_report());
                report.push('\n');
            }
        }
        
        report.push_str("====================================================\n");
        report.push_str("Fin du rapport\n");
        
        report
    }
    
    /// Efface toutes les statistiques
    pub fn reset(&self) {
        for stats in &self.stats {
            stats.total_calls.store(0, Ordering::Relaxed);
            stats.total_cycles.store(0, Ordering::Relaxed);
            stats.min_cycles.store(u64::MAX, Ordering::Relaxed);
            stats.max_cycles.store(0, Ordering::Relaxed);
        }
    }
    
    /// Obtient les statistiques d'un composant
    pub fn get_stats(&self, component: Component) -> Option<&ComponentStats> {
        let index = component.as_u8() as usize;
        if index < self.stats.len() {
            Some(&self.stats[index])
        } else {
            None
        }
    }
}

/// Instance globale du gestionnaire de performance
lazy_static::lazy_static! {
    pub static ref PERF_MANAGER: PerformanceManager = PerformanceManager::new();
}

/// Macro pour mesurer les performances d'une fonction
#[macro_export]
macro_rules! measure_performance {
    ($component:expr, $func:block) => {{
        let start = $crate::perf_counters::rdtsc();
        let result = $func;
        let end = $crate::perf_counters::rdtsc();
        let cycles = end - start;
        
        $crate::perf_counters::PERF_MANAGER.record($component, cycles);
        result
    }};
}

/// Macro pour mesurer les performances d'une expression
#[macro_export]
macro_rules! measure_perf_expr {
    ($component:expr, $expr:expr) => {{
        let start = $crate::perf_counters::rdtsc();
        let result = $expr;
        let end = $crate::perf_counters::rdtsc();
        let cycles = end - start;
        
        $crate::perf_counters::PERF_MANAGER.record($component, cycles);
        result
    }};
}

/// Mesure directe des performances
pub fn measure_direct(component: Component, start_cycles: u64, end_cycles: u64) {
    let cycles = end_cycles - start_cycles;
    PERF_MANAGER.record(component, cycles);
}

/// Imprime un rapport de performance via println!
pub fn print_performance_report() {
    let report = PERF_MANAGER.generate_full_report();
    // Utiliser println! directement
    println!("{}", report);
}

/// Imprime un rapport synthétique
pub fn print_summary_report() {
    println!("========== SYNTHESE DE PERFORMANCE ==========");
    
    // VGA
    if let Some(stats) = PERF_MANAGER.get_stats(Component::Vga) {
        let calls = stats.total_calls.load(Ordering::Relaxed);
        if calls > 0 {
            let avg = stats.average_cycles();
            let time_us = (avg as f64) / 3000.0;
            println!("VGA: {} appels, {} cycles moyen ({:.3} µs)", calls, avg, time_us);
        }
    }
    
    // Scheduler
    if let Some(stats) = PERF_MANAGER.get_stats(Component::Scheduler) {
        let calls = stats.total_calls.load(Ordering::Relaxed);
        if calls > 0 {
            let avg = stats.average_cycles();
            let time_us = (avg as f64) / 3000.0;
            println!("Scheduler: {} appels, {} cycles moyen ({:.3} µs)", calls, avg, time_us);
        }
    }
    
    // Syscall
    if let Some(stats) = PERF_MANAGER.get_stats(Component::Syscall) {
        let calls = stats.total_calls.load(Ordering::Relaxed);
        if calls > 0 {
            let avg = stats.average_cycles();
            let time_us = (avg as f64) / 3000.0;
            println!("Syscall: {} appels, {} cycles moyen ({:.3} µs)", calls, avg, time_us);
        }
    }
    
    // Memory
    if let Some(stats) = PERF_MANAGER.get_stats(Component::Memory) {
        let calls = stats.total_calls.load(Ordering::Relaxed);
        if calls > 0 {
            let avg = stats.average_cycles();
            let time_us = (avg as f64) / 3000.0;
            println!("Memory: {} appels, {} cycles moyen ({:.3} µs)", calls, avg, time_us);
        }
    }

    // IPC
    if let Some(stats) = PERF_MANAGER.get_stats(Component::Ipc) {
        let calls = stats.total_calls.load(Ordering::Relaxed);
        if calls > 0 {
            let avg = stats.average_cycles();
            let time_us = (avg as f64) / 3000.0;
            println!("IPC: {} appels, {} cycles moyen ({:.3} µs)", calls, avg, time_us);
        }
    }

    // Drivers
    if let Some(stats) = PERF_MANAGER.get_stats(Component::Drivers) {
        let calls = stats.total_calls.load(Ordering::Relaxed);
        if calls > 0 {
            let avg = stats.average_cycles();
            let time_us = (avg as f64) / 3000.0;
            println!("Drivers: {} appels, {} cycles moyen ({:.3} µs)", calls, avg, time_us);
        }
    }

    // Kernel Boot
    if let Some(stats) = PERF_MANAGER.get_stats(Component::KernelBoot) {
        let calls = stats.total_calls.load(Ordering::Relaxed);
        if calls > 0 {
            let avg = stats.average_cycles();
            let time_us = (avg as f64) / 3000.0;
            println!("KernelBoot: {} mesures, {} cycles moyen ({:.3} µs)", calls, avg, time_us);
        }
    }
    
    println!("==============================================");
}

/// Vérifie si le système de performance est activé
pub fn is_enabled() -> bool {
    // TODO: Ajouter une configuration pour activer/désactiver
    true
}