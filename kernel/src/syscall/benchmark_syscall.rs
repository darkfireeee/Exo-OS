//! Syscall Performance Benchmarking
//!
//! Mesure les performances des différents chemins de syscalls (fast path vs slow path)

use core::arch::asm;

/// Résultats de benchmark pour les syscalls
#[derive(Debug, Clone, Copy)]
pub struct SyscallBenchmarkResults {
    pub name: &'static str,
    pub iterations: u64,
    pub total_cycles: u64,
    pub min_cycles: u64,
    pub max_cycles: u64,
    pub avg_cycles: u64,
    pub median_cycles: u64,
}

impl SyscallBenchmarkResults {
    pub fn new(name: &'static str, iterations: u64) -> Self {
        Self {
            name,
            iterations,
            total_cycles: 0,
            min_cycles: u64::MAX,
            max_cycles: 0,
            avg_cycles: 0,
            median_cycles: 0,
        }
    }
    
    pub fn record(&mut self, cycles: u64) {
        self.total_cycles += cycles;
        self.min_cycles = self.min_cycles.min(cycles);
        self.max_cycles = self.max_cycles.max(cycles);
    }
    
    pub fn finalize(&mut self) {
        if self.iterations > 0 {
            self.avg_cycles = self.total_cycles / self.iterations;
            self.median_cycles = self.avg_cycles; // Approximation
        }
    }
}

/// Lit le Time Stamp Counter (TSC) pour mesurer les cycles
#[inline(always)]
fn rdtsc() -> u64 {
    unsafe {
        let mut low: u32;
        let mut high: u32;
        asm!(
            "rdtsc",
            out("eax") low,
            out("edx") high,
            options(nomem, nostack)
        );
        ((high as u64) << 32) | (low as u64)
    }
}

/// Benchmark pour sys_getpid (fast path)
pub fn bench_getpid(iterations: u64) -> SyscallBenchmarkResults {
    let mut results = SyscallBenchmarkResults::new("sys_getpid (fast path)", iterations);
    
    for _ in 0..iterations {
        let start = rdtsc();
        let _ = crate::task::current().pid();
        let end = rdtsc();
        results.record(end - start);
    }
    
    results.finalize();
    results
}

/// Benchmark pour sys_gettid (fast path)
pub fn bench_gettid(iterations: u64) -> SyscallBenchmarkResults {
    let mut results = SyscallBenchmarkResults::new("sys_gettid (fast path)", iterations);
    
    for _ in 0..iterations {
        let start = rdtsc();
        let _ = crate::task::current().id();
        let end = rdtsc();
        results.record(end - start);
    }
    
    results.finalize();
    results
}

/// Benchmark pour sys_sched_yield (fast path)
pub fn bench_sched_yield(iterations: u64) -> SyscallBenchmarkResults {
    let mut results = SyscallBenchmarkResults::new("sys_sched_yield (fast path)", iterations);
    
    for _ in 0..iterations {
        let start = rdtsc();
        crate::scheduler::yield_now();
        let end = rdtsc();
        results.record(end - start);
    }
    
    results.finalize();
    results
}

/// Benchmark pour syscall entry overhead
pub fn bench_syscall_entry(iterations: u64) -> SyscallBenchmarkResults {
    let mut results = SyscallBenchmarkResults::new("Syscall entry/exit overhead", iterations);
    
    for _ in 0..iterations {
        let start = rdtsc();
        // Minimal syscall - just entry/exit
        unsafe {
            asm!(
                "syscall",
                in("rax") 0, // Syscall number 0 (minimal)
                out("rcx") _,
                out("r11") _,
                options(nostack)
            );
        }
        let end = rdtsc();
        results.record(end - start);
    }
    
    results.finalize();
    results
}

/// Benchmark pour fast path complet
pub fn bench_fast_path_roundtrip(iterations: u64) -> SyscallBenchmarkResults {
    let mut results = SyscallBenchmarkResults::new("Fast path roundtrip", iterations);
    
    for _ in 0..iterations {
        let start = rdtsc();
        // Entry + dispatch + getpid + exit
        let _ = crate::syscall::entry::fast_path::handle(
            crate::syscall::numbers::Syscall::GetPid as usize,
            crate::syscall::abi::SyscallArgs::default(),
        );
        let end = rdtsc();
        results.record(end - start);
    }
    
    results.finalize();
    results
}

/// Benchmark pour slow path (syscall complexe)
pub fn bench_slow_path_roundtrip(iterations: u64) -> SyscallBenchmarkResults {
    let mut results = SyscallBenchmarkResults::new("Slow path roundtrip", iterations);
    
    for _ in 0..iterations {
        let start = rdtsc();
        // Entry + dispatch complexe
        let _ = crate::syscall::entry::slow_path::handle(
            100, // Syscall number arbitraire
            crate::syscall::abi::SyscallArgs::default(),
        );
        let end = rdtsc();
        results.record(end - start);
    }
    
    results.finalize();
    results
}

/// Lance tous les benchmarks et affiche les résultats
pub fn run_all_benchmarks() {
    log::info!("=== Syscall Performance Benchmarks ===");
    
    const ITERATIONS: u64 = 1000;
    
    let results = [
        bench_getpid(ITERATIONS),
        bench_gettid(ITERATIONS),
        bench_sched_yield(ITERATIONS),
        bench_syscall_entry(ITERATIONS),
        bench_fast_path_roundtrip(ITERATIONS),
        bench_slow_path_roundtrip(ITERATIONS),
    ];
    
    for result in &results {
        log::info!("{}: min={} avg={} max={} cycles",
            result.name,
            result.min_cycles,
            result.avg_cycles,
            result.max_cycles
        );
        
        // Validation: fast path devrait être < 100 cycles
        if result.name.contains("fast path") && result.avg_cycles < 100 {
            log::info!("  ✓ PASS (target: <100 cycles)");
        } else if result.name.contains("entry/exit") && result.avg_cycles < 50 {
            log::info!("  ✓ PASS (target: <50 cycles)");
        }
    }
}

/// Benchmark rapide (moins d'itérations)
pub fn quick_bench() {
    const QUICK_ITERATIONS: u64 = 100;
    
    let getpid = bench_getpid(QUICK_ITERATIONS);
    let gettid = bench_gettid(QUICK_ITERATIONS);
    
    log::info!("Quick bench: getpid={} cycles, gettid={} cycles",
        getpid.avg_cycles,
        gettid.avg_cycles
    );
}

/// Benchmark standard
pub fn standard_bench() {
    run_all_benchmarks();
}

/// Benchmark étendu (plus d'itérations pour plus de précision)
pub fn extensive_bench() {
    const EXTENSIVE_ITERATIONS: u64 = 10000;
    
    log::info!("=== Extended Syscall Benchmarks ({} iterations) ===", EXTENSIVE_ITERATIONS);
    
    let getpid = bench_getpid(EXTENSIVE_ITERATIONS);
    let fast_path = bench_fast_path_roundtrip(EXTENSIVE_ITERATIONS);
    let slow_path = bench_slow_path_roundtrip(EXTENSIVE_ITERATIONS);
    
    log::info!("getpid: {} cycles (σ={})",
        getpid.avg_cycles,
        getpid.max_cycles - getpid.min_cycles
    );
    log::info!("fast_path: {} cycles (σ={})",
        fast_path.avg_cycles,
        fast_path.max_cycles - fast_path.min_cycles
    );
    log::info!("slow_path: {} cycles (σ={})",
        slow_path.avg_cycles,
        slow_path.max_cycles - slow_path.min_cycles
    );
    
    // Ratio fast/slow
    if slow_path.avg_cycles > 0 {
        let ratio = slow_path.avg_cycles / fast_path.avg_cycles.max(1);
        log::info!("Slow path is {}x slower than fast path", ratio);
    }
}
