//! # Module de Benchmark pour Exo-OS
//!
//! Fournit des outils pour mesurer les performances en cycles CPU.
//! Objectif: Écraser Linux avec des métriques vérifiables.

use core::sync::atomic::{AtomicU64, Ordering};

/// Résultats de benchmark
#[derive(Debug, Clone, Copy)]
pub struct BenchResult {
    /// Nombre de cycles minimum
    pub min_cycles: u64,
    /// Nombre de cycles maximum
    pub max_cycles: u64,
    /// Nombre de cycles moyen
    pub avg_cycles: u64,
    /// Nombre d'itérations
    pub iterations: u64,
}

impl BenchResult {
    pub fn new() -> Self {
        Self {
            min_cycles: u64::MAX,
            max_cycles: 0,
            avg_cycles: 0,
            iterations: 0,
        }
    }
}

/// Lit le compteur de cycles CPU (TSC - Time Stamp Counter)
#[inline(always)]
pub fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, preserves_flags)
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Lit le compteur de cycles avec sérialisation (plus précis)
#[inline(always)]
pub fn rdtscp() -> u64 {
    let lo: u32;
    let hi: u32;
    let _aux: u32;
    unsafe {
        core::arch::asm!(
            "rdtscp",
            out("eax") lo,
            out("edx") hi,
            out("ecx") _aux,
            options(nomem, nostack, preserves_flags)
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Sérialise les instructions avant la mesure
#[inline(always)]
pub fn serialize() {
    unsafe {
        // CPUID sérialise le pipeline mais on ne peut pas utiliser rbx directement
        // car LLVM le réserve. On utilise une approche alternative.
        core::arch::asm!(
            "xor eax, eax",
            "push rbx",
            "cpuid",
            "pop rbx",
            out("eax") _,
            out("ecx") _,
            out("edx") _,
            options(nomem, preserves_flags)
        );
    }
}

/// Mesure le nombre de cycles pour exécuter une closure
#[inline(always)]
pub fn measure<F: FnOnce()>(f: F) -> u64 {
    serialize();
    let start = rdtsc();
    f();
    serialize();
    let end = rdtsc();
    end.saturating_sub(start)
}

/// Mesure le nombre de cycles moyen sur plusieurs itérations
pub fn benchmark<F: Fn()>(iterations: usize, f: F) -> BenchResult {
    let mut result = BenchResult::new();
    let mut total: u64 = 0;
    
    // Warmup (3 itérations)
    for _ in 0..3 {
        f();
    }
    
    // Mesures réelles
    for _ in 0..iterations {
        let cycles = measure(|| f());
        
        if cycles < result.min_cycles {
            result.min_cycles = cycles;
        }
        if cycles > result.max_cycles {
            result.max_cycles = cycles;
        }
        total += cycles;
    }
    
    result.iterations = iterations as u64;
    result.avg_cycles = total / (iterations as u64);
    
    result
}

// ============================================================================
// Benchmarks spécifiques Exo-OS
// ============================================================================

/// Statistiques globales de benchmark
pub struct GlobalBenchStats {
    pub context_switch_cycles: AtomicU64,
    pub ipc_send_cycles: AtomicU64,
    pub ipc_recv_cycles: AtomicU64,
    pub alloc_cycles: AtomicU64,
    pub free_cycles: AtomicU64,
    pub syscall_cycles: AtomicU64,
    pub scheduler_pick_cycles: AtomicU64,
}

impl GlobalBenchStats {
    pub const fn new() -> Self {
        Self {
            context_switch_cycles: AtomicU64::new(0),
            ipc_send_cycles: AtomicU64::new(0),
            ipc_recv_cycles: AtomicU64::new(0),
            alloc_cycles: AtomicU64::new(0),
            free_cycles: AtomicU64::new(0),
            syscall_cycles: AtomicU64::new(0),
            scheduler_pick_cycles: AtomicU64::new(0),
        }
    }
    
    pub fn record_context_switch(&self, cycles: u64) {
        // Moyenne mobile exponentielle
        let current = self.context_switch_cycles.load(Ordering::Relaxed);
        if current == 0 {
            self.context_switch_cycles.store(cycles, Ordering::Relaxed);
        } else {
            let new_avg = (current * 7 + cycles) / 8;
            self.context_switch_cycles.store(new_avg, Ordering::Relaxed);
        }
    }
    
    pub fn record_ipc_send(&self, cycles: u64) {
        let current = self.ipc_send_cycles.load(Ordering::Relaxed);
        if current == 0 {
            self.ipc_send_cycles.store(cycles, Ordering::Relaxed);
        } else {
            let new_avg = (current * 7 + cycles) / 8;
            self.ipc_send_cycles.store(new_avg, Ordering::Relaxed);
        }
    }
    
    pub fn record_scheduler_pick(&self, cycles: u64) {
        let current = self.scheduler_pick_cycles.load(Ordering::Relaxed);
        if current == 0 {
            self.scheduler_pick_cycles.store(cycles, Ordering::Relaxed);
        } else {
            let new_avg = (current * 7 + cycles) / 8;
            self.scheduler_pick_cycles.store(new_avg, Ordering::Relaxed);
        }
    }
    
    /// Affiche les statistiques
    pub fn print_stats(&self) {
        log::info!("═══════════════════════════════════════════════════════");
        log::info!("  📊 BENCHMARK STATS - Exo-OS vs Linux");
        log::info!("═══════════════════════════════════════════════════════");
        
        let ctx_sw = self.context_switch_cycles.load(Ordering::Relaxed);
        let ipc = self.ipc_send_cycles.load(Ordering::Relaxed);
        let sched = self.scheduler_pick_cycles.load(Ordering::Relaxed);
        
        log::info!("  Context Switch: {} cycles (Linux: ~2134, Target: 304)", ctx_sw);
        log::info!("  IPC Send:       {} cycles (Linux: ~1247, Target: 347)", ipc);
        log::info!("  Scheduler Pick: {} cycles (Linux: ~200,  Target: 87)", sched);
        log::info!("═══════════════════════════════════════════════════════");
    }
}

/// Instance globale des statistiques
pub static BENCH_STATS: GlobalBenchStats = GlobalBenchStats::new();

/// Initialise le module de benchmark
pub fn init() {
    log::info!("[BENCH] Benchmark module initialized");
    log::info!("[BENCH] TSC frequency will be calibrated...");
}

/// Exécute les benchmarks de base et affiche les résultats
pub fn run_basic_benchmarks() {
    log::info!("[BENCH] Running basic benchmarks...");
    
    // Benchmark rdtsc overhead
    let tsc_result = benchmark(1000, || {
        let _ = rdtsc();
    });
    log::info!("[BENCH] rdtsc overhead: {} cycles (min: {}, max: {})", 
              tsc_result.avg_cycles, tsc_result.min_cycles, tsc_result.max_cycles);
    
    // Benchmark empty function call
    let call_result = benchmark(1000, || {
        core::hint::black_box(());
    });
    log::info!("[BENCH] Empty call: {} cycles", call_result.avg_cycles);
    
    log::info!("[BENCH] Basic benchmarks complete");
}
