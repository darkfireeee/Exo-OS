//! Production Context Switch Benchmark
//!
//! Mesure continue des performances de context switch en production
//! avec threads dédiés qui persistent pendant toute l'exécution.
//!
//! Objectif: Validation continue de la performance <304 cycles
//! Cette approche permet de mesurer la performance RÉELLE en production,
//! pas juste pendant un warmup.

use crate::bench::{rdtsc, serialize};
use crate::scheduler::{self, yield_now};
use alloc::boxed::Box;
use alloc::string::ToString;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Contrôle de l'exécution du benchmark
static BENCHMARK_RUNNING: AtomicBool = AtomicBool::new(false);

/// Résultats du benchmark (atomics pour lecture sans lock)
static MEASUREMENT_COUNT: AtomicU64 = AtomicU64::new(0);
static TOTAL_CYCLES: AtomicU64 = AtomicU64::new(0);
static MIN_CYCLES: AtomicU64 = AtomicU64::new(u64::MAX);
static MAX_CYCLES: AtomicU64 = AtomicU64::new(0);

/// Nombre d'itérations avant d'afficher les résultats
const REPORT_INTERVAL: u64 = 100;

/// Thread dédié au benchmark (persiste pendant toute l'exécution)
fn benchmark_worker_thread() -> ! {
    crate::logger::info("Context switch benchmark worker started");

    // Warmup (10 iterations pour stabiliser les caches)
    for _ in 0..10 {
        yield_now();
    }

    let mut iteration = 0u64;

    while BENCHMARK_RUNNING.load(Ordering::Relaxed) {
        // Mesure précise du context switch (2 switches par yield_now)
        serialize();
        let start = rdtsc();
        yield_now();
        let end = rdtsc();
        serialize();

        let total_cycles = end.saturating_sub(start);
        let cycles_per_switch = total_cycles / 2; // 2 switches par yield

        // Mise à jour des statistiques atomiques
        MEASUREMENT_COUNT.fetch_add(1, Ordering::Relaxed);
        TOTAL_CYCLES.fetch_add(cycles_per_switch, Ordering::Relaxed);

        // Update min (atomic CAS loop)
        let mut current_min = MIN_CYCLES.load(Ordering::Relaxed);
        while cycles_per_switch < current_min {
            match MIN_CYCLES.compare_exchange_weak(
                current_min,
                cycles_per_switch,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(x) => current_min = x,
            }
        }

        // Update max (atomic CAS loop)
        let mut current_max = MAX_CYCLES.load(Ordering::Relaxed);
        while cycles_per_switch > current_max {
            match MAX_CYCLES.compare_exchange_weak(
                current_max,
                cycles_per_switch,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(x) => current_max = x,
            }
        }

        iteration += 1;

        // Rapport périodique tous les 100 samples
        if iteration % REPORT_INTERVAL == 0 {
            print_current_stats();
        }

        // Petite pause pour éviter de monopoliser le CPU
        // (yield implicite via context switch mesuré)
    }

    crate::logger::info("Benchmark worker exiting");
    crate::syscall::handlers::process::sys_exit(0);
}

/// Affiche les statistiques actuelles
fn print_current_stats() {
    let count = MEASUREMENT_COUNT.load(Ordering::Relaxed);
    if count == 0 {
        return;
    }

    let total = TOTAL_CYCLES.load(Ordering::Relaxed);
    let min = MIN_CYCLES.load(Ordering::Relaxed);
    let max = MAX_CYCLES.load(Ordering::Relaxed);
    let avg = total / count;

    crate::logger::info("╔══════════════════════════════════════════════════════════╗");
    crate::logger::info("║      CONTEXT SWITCH BENCHMARK - Production Stats        ║");
    crate::logger::info("╠══════════════════════════════════════════════════════════╣");
    crate::logger::info(&alloc::format!(
        "║  Samples:          {:>10}                          ║",
        count
    ));
    crate::logger::info(&alloc::format!(
        "║  Average:          {:>10} cycles                    ║",
        avg
    ));
    crate::logger::info(&alloc::format!(
        "║  Min:              {:>10} cycles                    ║",
        min
    ));
    crate::logger::info(&alloc::format!(
        "║  Max:              {:>10} cycles                    ║",
        max
    ));
    crate::logger::info("╠══════════════════════════════════════════════════════════╣");
    crate::logger::info(&alloc::format!(
        "║  Target (Exo-OS):  {:>10} cycles                    ║",
        304
    ));
    crate::logger::info(&alloc::format!(
        "║  Limit (Phase 0):  {:>10} cycles                    ║",
        500
    ));
    crate::logger::info(&alloc::format!(
        "║  Linux baseline:   {:>10} cycles                    ║",
        2134
    ));
    crate::logger::info("╠══════════════════════════════════════════════════════════╣");

    // Status avec emojis
    if avg < 304 {
        crate::logger::info("║  Status: ✅ EXCELLENT - Target achieved!               ║");
        let speedup = (2134.0 / avg as f32 * 10.0) as u64;
        crate::logger::info(&alloc::format!(
            "║  vs Linux: {}.{}x FASTER 🚀                            ║",
            speedup / 10,
            speedup % 10
        ));
    } else if avg < 500 {
        crate::logger::info("║  Status: ✅ PASS - Under Phase 0 limit!               ║");
        let speedup = (2134.0 / avg as f32 * 10.0) as u64;
        crate::logger::info(&alloc::format!(
            "║  vs Linux: {}.{}x FASTER 🔥                            ║",
            speedup / 10,
            speedup % 10
        ));
    } else if avg < 2134 {
        crate::logger::info("║  Status: ⚠️  ACCEPTABLE - Faster than Linux           ║");
    } else {
        crate::logger::info("║  Status: ❌ NEEDS OPTIMIZATION                         ║");
    }

    crate::logger::info("╚══════════════════════════════════════════════════════════╝");

    // Enregistrer dans stats globales
    crate::bench::BENCH_STATS.record_context_switch(avg);
}

/// Démarre le benchmark en arrière-plan (thread persistant)
pub fn start_production_benchmark() -> Result<(), &'static str> {
    if BENCHMARK_RUNNING.load(Ordering::Relaxed) {
        return Err("Benchmark already running");
    }

    crate::logger::info("[BENCH] Starting production context switch benchmark...");

    // Reset statistiques
    MEASUREMENT_COUNT.store(0, Ordering::Relaxed);
    TOTAL_CYCLES.store(0, Ordering::Relaxed);
    MIN_CYCLES.store(u64::MAX, Ordering::Relaxed);
    MAX_CYCLES.store(0, Ordering::Relaxed);

    // Activer le benchmark
    BENCHMARK_RUNNING.store(true, Ordering::Release);

    // Créer thread de benchmark (persiste pendant toute l'exécution)
    let thread = Box::new(crate::scheduler::thread::Thread::new_kernel(
        crate::scheduler::thread::alloc_thread_id(),
        "ctx_switch_bench",
        benchmark_worker_thread,
        16384, // 16KB stack
    ));

    // Ajouter au scheduler
    match crate::scheduler::SCHEDULER.add_thread(*thread) {
        Ok(_) => {
            crate::logger::info("[BENCH] ✅ Production benchmark thread started");
            crate::logger::info("[BENCH] Measuring context switches continuously...");
            crate::logger::info("[BENCH] Reports every 100 samples");
            Ok(())
        }
        Err(e) => {
            BENCHMARK_RUNNING.store(false, Ordering::Relaxed);
            crate::logger::error(&alloc::format!(
                "[BENCH] ❌ Failed to start benchmark: {:?}",
                e
            ));
            Err("Failed to add benchmark thread")
        }
    }
}

/// Arrête le benchmark
pub fn stop_production_benchmark() {
    crate::logger::info("[BENCH] Stopping production benchmark...");
    BENCHMARK_RUNNING.store(false, Ordering::Release);

    // Afficher rapport final
    print_final_report();
}

/// Affiche le rapport final avec toutes les statistiques
fn print_final_report() {
    let count = MEASUREMENT_COUNT.load(Ordering::Relaxed);
    if count == 0 {
        crate::logger::warn("[BENCH] No measurements collected");
        return;
    }

    crate::logger::info("");
    crate::logger::info("╔══════════════════════════════════════════════════════════╗");
    crate::logger::info("║         CONTEXT SWITCH BENCHMARK - FINAL REPORT         ║");
    crate::logger::info("╠══════════════════════════════════════════════════════════╣");

    let total = TOTAL_CYCLES.load(Ordering::Relaxed);
    let min = MIN_CYCLES.load(Ordering::Relaxed);
    let max = MAX_CYCLES.load(Ordering::Relaxed);
    let avg = total / count;

    crate::logger::info(&alloc::format!(
        "║  Total measurements:  {:>10}                       ║",
        count
    ));
    crate::logger::info(&alloc::format!(
        "║  Average:             {:>10} cycles                 ║",
        avg
    ));
    crate::logger::info(&alloc::format!(
        "║  Best (min):          {:>10} cycles                 ║",
        min
    ));
    crate::logger::info(&alloc::format!(
        "║  Worst (max):         {:>10} cycles                 ║",
        max
    ));
    crate::logger::info(&alloc::format!(
        "║  Stddev estimate:     {:>10} cycles                 ║",
        (max - min) / 4
    )); // Approximation simple

    crate::logger::info("╠══════════════════════════════════════════════════════════╣");

    // Comparaison avec targets
    if avg < 304 {
        crate::logger::info("║                                                          ║");
        crate::logger::info("║  🎯 TARGET ACHIEVED! Context switch < 304 cycles         ║");
        crate::logger::info("║                                                          ║");
        let speedup = (2134.0 / avg as f32 * 100.0) as u64;
        crate::logger::info(&alloc::format!(
            "║  🚀 {}x FASTER than Linux (2134 cycles)               ║",
            speedup / 100
        ));
    } else if avg < 500 {
        crate::logger::info("║  ✅ PHASE 0 PASSED - Under 500 cycles limit             ║");
    }

    crate::logger::info("╚══════════════════════════════════════════════════════════╝");
}

/// Obtenir les statistiques actuelles (pour API externe)
#[allow(dead_code)]
pub fn get_stats() -> (u64, u64, u64, u64) {
    let count = MEASUREMENT_COUNT.load(Ordering::Relaxed);
    let avg = if count > 0 {
        TOTAL_CYCLES.load(Ordering::Relaxed) / count
    } else {
        0
    };
    let min = MIN_CYCLES.load(Ordering::Relaxed);
    let max = MAX_CYCLES.load(Ordering::Relaxed);

    (avg, min, max, count)
}
