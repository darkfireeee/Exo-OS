//! # Framework de Benchmarking Unifié
//! 
//! Orchestration de tous les benchmarks Zero-Copy Fusion avec mesures RDTSC précises.

use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;

/// RDTSC helper unifié
#[cfg(all(target_arch = "x86_64", not(target_os = "windows")))]
pub fn rdtsc() -> u64 {
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
pub fn rdtsc() -> u64 {
    // Fallback pour tests Windows ou autres architectures
    // Simule un compteur incrémental pour les tests
    static mut COUNTER: u64 = 0;
    unsafe {
        COUNTER += 100;
        COUNTER
    }
}

/// Calibration de la fréquence TSC
pub fn calibrate_tsc_frequency() -> u64 {
    // En production : mesurer avec timer PIT/HPET
    // Pour simulation : 2000 MHz (2 GHz typique)
    2000
}

/// Statistiques d'un benchmark
#[derive(Debug, Clone)]
pub struct BenchStats {
    pub name: String,
    pub samples: Vec<u64>,
    pub mean: u64,
    pub min: u64,
    pub max: u64,
    pub std_dev: u64,
    pub p50: u64,
    pub p95: u64,
    pub p99: u64,
}

impl BenchStats {
    pub fn new(name: String, mut samples: Vec<u64>) -> Self {
        if samples.is_empty() {
            return Self {
                name,
                samples: Vec::new(),
                mean: 0,
                min: 0,
                max: 0,
                std_dev: 0,
                p50: 0,
                p95: 0,
                p99: 0,
            };
        }
        
        samples.sort_unstable();
        
        let mean = samples.iter().sum::<u64>() / samples.len() as u64;
        let min = samples[0];
        let max = samples[samples.len() - 1];
        
        // Écart-type (approximation simple pour no_std)
        let variance = samples
            .iter()
            .map(|&x| {
                let diff = if x > mean { x - mean } else { mean - x };
                diff * diff
            })
            .sum::<u64>() / samples.len() as u64;
        
        // Approximation de sqrt pour no_std (méthode de Newton)
        let std_dev = if variance == 0 {
            0
        } else {
            let mut x = variance / 2;
            for _ in 0..10 {
                let x_next = (x + variance / x) / 2;
                if x_next == x {
                    break;
                }
                x = x_next;
            }
            x
        };
        
        // Percentiles
        let p50 = samples[samples.len() * 50 / 100];
        let p95 = samples[samples.len() * 95 / 100];
        let p99 = samples[samples.len() * 99 / 100];
        
        Self {
            name,
            samples,
            mean,
            min,
            max,
            std_dev,
            p50,
            p95,
            p99,
        }
    }
    
    /// Conversion cycles → nanosecondes
    pub fn cycles_to_ns(&self, cycles: u64, tsc_freq_mhz: u64) -> u64 {
        (cycles * 1000) / tsc_freq_mhz
    }
    
    /// Conversion cycles → microsecondes
    pub fn cycles_to_us(&self, cycles: u64, tsc_freq_mhz: u64) -> u64 {
        cycles / tsc_freq_mhz
    }
    
    /// Affichage formaté
    pub fn print(&self, tsc_freq_mhz: u64) {
        crate::println!("\n=== {} ===", self.name);
        crate::println!("  Samples:  {}", self.samples.len());
        crate::println!("  Mean:     {} cycles ({} ns)", 
            self.mean, self.cycles_to_ns(self.mean, tsc_freq_mhz));
        crate::println!("  Min:      {} cycles ({} ns)", 
            self.min, self.cycles_to_ns(self.min, tsc_freq_mhz));
        crate::println!("  Max:      {} cycles ({} ns)", 
            self.max, self.cycles_to_ns(self.max, tsc_freq_mhz));
        crate::println!("  StdDev:   {} cycles ({} ns)", 
            self.std_dev, self.cycles_to_ns(self.std_dev, tsc_freq_mhz));
        crate::println!("  P50:      {} cycles ({} ns)", 
            self.p50, self.cycles_to_ns(self.p50, tsc_freq_mhz));
        crate::println!("  P95:      {} cycles ({} ns)", 
            self.p95, self.cycles_to_ns(self.p95, tsc_freq_mhz));
        crate::println!("  P99:      {} cycles ({} ns)", 
            self.p99, self.cycles_to_ns(self.p99, tsc_freq_mhz));
    }
    
    /// Export CSV (une ligne)
    pub fn to_csv_line(&self, tsc_freq_mhz: u64) -> String {
        format!("{},{},{},{},{},{},{},{},{}\n",
            self.name,
            self.samples.len(),
            self.mean,
            self.cycles_to_ns(self.mean, tsc_freq_mhz),
            self.min,
            self.max,
            self.p50,
            self.p95,
            self.p99
        )
    }
}

/// Comparaison baseline vs optimisé
#[derive(Debug, Clone)]
pub struct BenchComparison {
    pub name: String,
    pub baseline_cycles: u64,
    pub optimized_cycles: u64,
    pub speedup: f64,
    pub improvement_percent: f64,
}

impl BenchComparison {
    pub fn new(name: String, baseline: &BenchStats, optimized: &BenchStats) -> Self {
        let baseline_cycles = baseline.mean;
        let optimized_cycles = optimized.mean;
        
        let speedup = if optimized_cycles > 0 {
            baseline_cycles as f64 / optimized_cycles as f64
        } else {
            0.0
        };
        
        let improvement_percent = if baseline_cycles > 0 {
            ((baseline_cycles as f64 - optimized_cycles as f64) / baseline_cycles as f64) * 100.0
        } else {
            0.0
        };
        
        Self {
            name,
            baseline_cycles,
            optimized_cycles,
            speedup,
            improvement_percent,
        }
    }
    
    pub fn print(&self, tsc_freq_mhz: u64) {
        let to_ns = |cycles: u64| (cycles * 1000) / tsc_freq_mhz;
        
        crate::println!("\n=== Comparison: {} ===", self.name);
        crate::println!("  Baseline:    {} cycles ({} ns)", 
            self.baseline_cycles, to_ns(self.baseline_cycles));
        crate::println!("  Optimized:   {} cycles ({} ns)", 
            self.optimized_cycles, to_ns(self.optimized_cycles));
        crate::println!("  Speedup:     {:.2}×", self.speedup);
        crate::println!("  Improvement: {:.1}%", self.improvement_percent);
    }
}

/// Suite de benchmarks
pub struct BenchmarkSuite {
    pub name: String,
    pub tsc_freq_mhz: u64,
    pub results: Vec<BenchStats>,
    pub comparisons: Vec<BenchComparison>,
}

impl BenchmarkSuite {
    pub fn new(name: String) -> Self {
        let tsc_freq_mhz = calibrate_tsc_frequency();
        
        Self {
            name,
            tsc_freq_mhz,
            results: Vec::new(),
            comparisons: Vec::new(),
        }
    }
    
    /// Ajoute un résultat de benchmark
    pub fn add_result(&mut self, stats: BenchStats) {
        self.results.push(stats);
    }
    
    /// Ajoute une comparaison
    pub fn add_comparison(&mut self, comparison: BenchComparison) {
        self.comparisons.push(comparison);
    }
    
    /// Compare deux benchmarks
    pub fn compare(&mut self, name: String, baseline_idx: usize, optimized_idx: usize) {
        if baseline_idx < self.results.len() && optimized_idx < self.results.len() {
            let comparison = BenchComparison::new(
                name,
                &self.results[baseline_idx],
                &self.results[optimized_idx]
            );
            self.add_comparison(comparison);
        }
    }
    
    /// Affiche tous les résultats
    pub fn print_results(&self) {
        crate::println!("\n╔═══════════════════════════════════════════════════════════╗");
        crate::println!("║  BENCHMARK SUITE: {}                              ║", self.name);
        crate::println!("╚═══════════════════════════════════════════════════════════╝");
        crate::println!("TSC Frequency: {} MHz\n", self.tsc_freq_mhz);
        
        for stats in &self.results {
            stats.print(self.tsc_freq_mhz);
        }
        
        if !self.comparisons.is_empty() {
            crate::println!("\n╔═══════════════════════════════════════════════════════════╗");
            crate::println!("║  COMPARISONS                                              ║");
            crate::println!("╚═══════════════════════════════════════════════════════════╝");
            
            for comp in &self.comparisons {
                comp.print(self.tsc_freq_mhz);
            }
        }
    }
    
    /// Export CSV complet
    pub fn to_csv(&self) -> String {
        let mut csv = String::from("Benchmark,Samples,Mean_Cycles,Mean_ns,Min,Max,P50,P95,P99\n");
        
        for stats in &self.results {
            csv.push_str(&stats.to_csv_line(self.tsc_freq_mhz));
        }
        
        csv.push_str("\nComparison,Baseline_Cycles,Optimized_Cycles,Speedup,Improvement_%\n");
        
        for comp in &self.comparisons {
            csv.push_str(&format!("{},{},{},{:.2},{:.1}\n",
                comp.name,
                comp.baseline_cycles,
                comp.optimized_cycles,
                comp.speedup,
                comp.improvement_percent
            ));
        }
        
        csv
    }
    
    /// Génère rapport Markdown
    pub fn to_markdown(&self) -> String {
        let mut md = format!("# Benchmark Report: {}\n\n", self.name);
        md.push_str(&format!("**TSC Frequency**: {} MHz\n\n", self.tsc_freq_mhz));
        
        md.push_str("## Benchmark Results\n\n");
        md.push_str("| Benchmark | Samples | Mean (cycles) | Mean (ns) | P50 | P95 | P99 |\n");
        md.push_str("|-----------|---------|---------------|-----------|-----|-----|-----|\n");
        
        for stats in &self.results {
            let mean_ns = stats.cycles_to_ns(stats.mean, self.tsc_freq_mhz);
            let p50_ns = stats.cycles_to_ns(stats.p50, self.tsc_freq_mhz);
            let p95_ns = stats.cycles_to_ns(stats.p95, self.tsc_freq_mhz);
            let p99_ns = stats.cycles_to_ns(stats.p99, self.tsc_freq_mhz);
            
            md.push_str(&format!("| {} | {} | {} | {} | {} | {} | {} |\n",
                stats.name,
                stats.samples.len(),
                stats.mean,
                mean_ns,
                p50_ns,
                p95_ns,
                p99_ns
            ));
        }
        
        if !self.comparisons.is_empty() {
            md.push_str("\n## Comparisons\n\n");
            md.push_str("| Optimization | Baseline (cycles) | Optimized (cycles) | Speedup | Improvement |\n");
            md.push_str("|--------------|-------------------|--------------------|---------|--------------|\n");
            
            for comp in &self.comparisons {
                md.push_str(&format!("| {} | {} | {} | {:.2}× | {:.1}% |\n",
                    comp.name,
                    comp.baseline_cycles,
                    comp.optimized_cycles,
                    comp.speedup,
                    comp.improvement_percent
                ));
            }
        }
        
        md
    }
}

/// Exécute une suite de benchmarks avec retry
pub fn run_benchmark_with_retry<F>(
    name: &str,
    iterations: usize,
    retries: usize,
    bench_fn: F
) -> BenchStats 
where
    F: Fn() -> u64
{
    let mut best_mean = u64::MAX;
    let mut best_samples = Vec::new();
    
    for retry in 0..retries {
        let mut samples = Vec::with_capacity(iterations);
        
        for _ in 0..iterations {
            let cycles = bench_fn();
            samples.push(cycles);
        }
        
        let mean = samples.iter().sum::<u64>() / samples.len() as u64;
        
        if mean < best_mean {
            best_mean = mean;
            best_samples = samples;
        }
        
        if retry < retries - 1 {
            // Courte pause entre retries
            let pause_start = rdtsc();
            while rdtsc() - pause_start < 10_000 {} // ~5µs pause
        }
    }
    
    BenchStats::new(String::from(name), best_samples)
}

/// Macro pour créer un benchmark simple
#[macro_export]
macro_rules! benchmark {
    ($name:expr, $iterations:expr, $code:block) => {{
        let mut samples = alloc::vec::Vec::with_capacity($iterations);
        
        for _ in 0..$iterations {
            let start = $crate::perf::bench_framework::rdtsc();
            $code
            let end = $crate::perf::bench_framework::rdtsc();
            samples.push(end - start);
        }
        
        $crate::perf::bench_framework::BenchStats::new(
            alloc::string::String::from($name),
            samples
        )
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_bench_stats_creation() {
        let samples = alloc::vec![100, 200, 300, 400, 500];
        let stats = BenchStats::new(String::from("test"), samples);
        
        assert_eq!(stats.mean, 300);
        assert_eq!(stats.min, 100);
        assert_eq!(stats.max, 500);
        assert_eq!(stats.p50, 300);
    }
    
    #[test]
    fn test_cycles_conversion() {
        let samples = alloc::vec![2000];
        let stats = BenchStats::new(String::from("test"), samples);
        
        // @ 2000 MHz: 2000 cycles = 1000 ns = 1 µs
        assert_eq!(stats.cycles_to_ns(2000, 2000), 1000);
        assert_eq!(stats.cycles_to_us(2000, 2000), 1);
    }
    
    #[test]
    fn test_bench_comparison() {
        let baseline = BenchStats::new(
            String::from("baseline"),
            alloc::vec![1000, 1000, 1000]
        );
        let optimized = BenchStats::new(
            String::from("optimized"),
            alloc::vec![100, 100, 100]
        );
        
        let comp = BenchComparison::new(
            String::from("test"),
            &baseline,
            &optimized
        );
        
        assert_eq!(comp.speedup, 10.0);
        assert_eq!(comp.improvement_percent, 90.0);
    }
    
    #[test]
    fn test_benchmark_suite() {
        let mut suite = BenchmarkSuite::new(String::from("Test Suite"));
        
        let stats1 = BenchStats::new(String::from("bench1"), alloc::vec![100, 200, 300]);
        let stats2 = BenchStats::new(String::from("bench2"), alloc::vec![50, 100, 150]);
        
        suite.add_result(stats1);
        suite.add_result(stats2);
        
        assert_eq!(suite.results.len(), 2);
        
        suite.compare(String::from("bench2 vs bench1"), 0, 1);
        assert_eq!(suite.comparisons.len(), 1);
    }
    
    #[test]
    fn test_csv_export() {
        let stats = BenchStats::new(String::from("test"), alloc::vec![100, 200, 300]);
        let csv = stats.to_csv_line(2000);
        
        assert!(csv.contains("test"));
        assert!(csv.contains("200")); // mean
    }
    
    #[test]
    fn test_rdtsc() {
        let tsc1 = rdtsc();
        let tsc2 = rdtsc();
        
        // TSC devrait être monotone
        assert!(tsc2 >= tsc1);
    }
    
    #[test]
    fn test_run_benchmark_with_retry() {
        let stats = run_benchmark_with_retry(
            "retry_test",
            10,
            3,
            || {
                let start = rdtsc();
                // Simulation travail
                let mut sum = 0u64;
                for i in 0..100 {
                    sum = sum.wrapping_add(i);
                }
                rdtsc() - start
            }
        );
        
        assert_eq!(stats.samples.len(), 10);
        assert!(stats.mean > 0);
    }
}
