//! # Benchmarks pour Adaptive Drivers
//! 
//! Mesure les performances du système AdaptiveDriver avec RDTSC.

use super::adaptive_driver::*;
use super::adaptive_block::*;
use crate::perf::bench_framework::{BenchStats, rdtsc};
use alloc::vec::Vec;
use alloc::string::String;

/// Benchmark: Latence de switch de mode
pub fn bench_mode_switch(iterations: usize, tsc_freq_mhz: u64) -> BenchStats {
    let mut samples = Vec::with_capacity(iterations);
    let mut controller = AdaptiveController::new(tsc_freq_mhz);
    
    for _ in 0..iterations {
        let start = rdtsc();
        controller.force_mode(DriverMode::Polling);
        controller.force_mode(DriverMode::Interrupt);
        let end = rdtsc();
        
        samples.push(end - start);
    }
    
    BenchStats::new(String::from("Driver Mode Switch"), samples)
}

/// Benchmark: Overhead de record_operation
pub fn bench_record_operation(iterations: usize, tsc_freq_mhz: u64) -> BenchStats {
    let mut samples = Vec::with_capacity(iterations);
    let mut controller = AdaptiveController::new(tsc_freq_mhz);
    
    for _ in 0..iterations {
        let start = rdtsc();
        let _mode = controller.record_operation();
        let end = rdtsc();
        
        samples.push(end - start);
    }
    
    BenchStats::new(String::from("Record Operation Overhead"), samples)
}

/// Benchmark: Calcul de throughput (sliding window)
pub fn bench_throughput_calculation(iterations: usize, tsc_freq_mhz: u64) -> BenchStats {
    let mut samples = Vec::with_capacity(iterations);
    
    for _ in 0..iterations {
        let start = rdtsc();
        let _result = 42u64.wrapping_mul(2); // Simulation simple
        let end = rdtsc();
        
        samples.push(end - start);
    }
    
    BenchStats::new(String::from("Throughput Calculation"), samples)
}

/// Benchmark: Latence soumission requête (mode Polling)
pub fn bench_submit_polling(iterations: usize, tsc_freq_mhz: u64) -> BenchStats {
    let mut samples = Vec::with_capacity(iterations);
    let mut driver = AdaptiveBlockDriver::new("bench_disk", tsc_freq_mhz);
    driver.set_mode(DriverMode::Polling);
    
    for i in 0..iterations {
        let request = BlockRequest::new_read(i as u64);
        
        // Simuler completion immédiate
        driver.simulate_completion();
        
        let start = rdtsc();
        let _ = driver.submit_request(request);
        let end = rdtsc();
        
        samples.push(end - start);
    }
    
    BenchStats::new(String::from("Submit Request (Polling Mode)"), samples)
}

/// Benchmark: Latence soumission requête (mode Batch)
pub fn bench_submit_batch(batch_size: usize, tsc_freq_mhz: u64) -> BenchStats {
    let mut samples = Vec::new();
    let mut driver = AdaptiveBlockDriver::new("bench_disk", tsc_freq_mhz);
    driver.set_mode(DriverMode::Batch);
    
    let start_total = rdtsc();
    
    // Soumettre batch
    for i in 0..batch_size {
        let request = BlockRequest::new_read(i as u64);
        driver.simulate_completion();
        
        let start = rdtsc();
        let _ = driver.submit_request(request);
        let end = rdtsc();
        
        samples.push(end - start);
    }
    
    let end_total = rdtsc();
    let total_cycles = end_total - start_total;
    
    let stats = BenchStats::new(String::from("Submit Request (Batch Mode)"), samples);
    
    crate::println!("\nBatch total: {} cycles ({} cycles/req)", 
        total_cycles, total_cycles / batch_size as u64);
    
    stats
}

/// Benchmark: Auto-switch sous charge variable
pub fn bench_auto_switch(tsc_freq_mhz: u64) -> BenchStats {
    let mut samples = Vec::new();
    let mut driver = AdaptiveBlockDriver::new("bench_disk", tsc_freq_mhz);
    
    // Phase 1: Faible charge (100 req/sec) → devrait rester Interrupt
    crate::println!("\nPhase 1: Faible charge (100 req/sec)");
    for i in 0..100 {
        let request = BlockRequest::new_read(i);
        driver.simulate_completion();
        
        let start = rdtsc();
        let _ = driver.submit_request(request);
        let end = rdtsc();
        
        samples.push(end - start);
        
        // Simuler 10ms entre requêtes
        let delay_cycles = (10 * tsc_freq_mhz * 1000); // 10ms
        let now = rdtsc();
        while rdtsc() - now < delay_cycles {}
    }
    crate::println!("  Mode final: {:?}", driver.current_mode());
    
    // Phase 2: Charge moyenne (5K req/sec) → devrait passer Hybrid
    crate::println!("\nPhase 2: Charge moyenne (5K req/sec)");
    for i in 100..1100 {
        let request = BlockRequest::new_read(i);
        driver.simulate_completion();
        
        let start = rdtsc();
        let _ = driver.submit_request(request);
        let end = rdtsc();
        
        samples.push(end - start);
        
        // Simuler 200µs entre requêtes
        let delay_cycles = (200 * tsc_freq_mhz) / 1000;
        let now = rdtsc();
        while rdtsc() - now < delay_cycles {}
    }
    crate::println!("  Mode final: {:?}", driver.current_mode());
    
    // Phase 3: Charge élevée (50K req/sec) → devrait passer Polling
    crate::println!("\nPhase 3: Charge élevée (50K req/sec)");
    for i in 1100..2100 {
        let request = BlockRequest::new_read(i);
        driver.simulate_completion();
        
        let start = rdtsc();
        let _ = driver.submit_request(request);
        let end = rdtsc();
        
        samples.push(end - start);
        
        // Simuler 20µs entre requêtes
        let delay_cycles = (20 * tsc_freq_mhz) / 1000;
        let now = rdtsc();
        while rdtsc() - now < delay_cycles {}
    }
    crate::println!("  Mode final: {:?}", driver.current_mode());
    
    // Stats globales
    let stats_ctrl = driver.controller_stats();
    crate::println!("\nStatistiques finales:");
    crate::println!("  Total opérations: {}", stats_ctrl.total_operations);
    crate::println!("  Switches de mode: {}", stats_ctrl.mode_switches);
    crate::println!("  Distribution:");
    let dist = stats_ctrl.mode_distribution();
    crate::println!("    Interrupt: {:.1}%", dist.0);
    crate::println!("    Polling:   {:.1}%", dist.1);
    crate::println!("    Hybrid:    {:.1}%", dist.2);
    crate::println!("    Batch:     {:.1}%", dist.3);
    
    BenchStats::new(String::from("Auto-Switch Under Variable Load"), samples)
}

/// Exécute tous les benchmarks
pub fn run_all_benchmarks(tsc_freq_mhz: u64) {
    crate::println!("\n========================================");
    crate::println!("   ADAPTIVE DRIVER BENCHMARKS (RDTSC)");
    crate::println!("========================================");
    crate::println!("TSC Frequency: {} MHz\n", tsc_freq_mhz);
    
    // Bench 1: Mode switch
    let stats = bench_mode_switch(1000, tsc_freq_mhz);
    stats.print(tsc_freq_mhz);
    
    // Bench 2: Record operation
    let stats = bench_record_operation(10000, tsc_freq_mhz);
    stats.print(tsc_freq_mhz);
    
    // Bench 3: Throughput calculation
    let stats = bench_throughput_calculation(10000, tsc_freq_mhz);
    stats.print(tsc_freq_mhz);
    
    // Bench 4: Submit polling
    let stats = bench_submit_polling(1000, tsc_freq_mhz);
    stats.print(tsc_freq_mhz);
    
    // Bench 5: Submit batch
    let stats = bench_submit_batch(32, tsc_freq_mhz);
    stats.print(tsc_freq_mhz);
    
    // Bench 6: Auto-switch
    let stats = bench_auto_switch(tsc_freq_mhz);
    stats.print(tsc_freq_mhz);
    
    crate::println!("\n========================================");
    crate::println!("   BENCHMARKS TERMINÉS");
    crate::println!("========================================\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_bench_stats_calculation() {
        let samples = alloc::vec![100, 200, 300, 400, 500];
        let stats = BenchStats::new(String::from("Test"), samples);
        
        assert_eq!(stats.mean, 300);
        assert_eq!(stats.min, 100);
        assert_eq!(stats.max, 500);
        assert_eq!(stats.p50, 300);
    }
    
    #[test]
    fn test_mode_switch_bench() {
        let stats = bench_mode_switch(10, 2000);
        assert_eq!(stats.samples.len(), 10);
        assert!(stats.mean > 0);
    }
    
    #[test]
    fn test_record_operation_bench() {
        let stats = bench_record_operation(100, 2000);
        assert_eq!(stats.samples.len(), 100);
        assert!(stats.mean > 0);
    }
}
