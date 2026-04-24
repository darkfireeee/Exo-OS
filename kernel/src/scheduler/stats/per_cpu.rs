// kernel/src/scheduler/stats/per_cpu.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Statistiques par CPU — compteurs atomiques hautes performances
// ═══════════════════════════════════════════════════════════════════════════════

use crate::scheduler::smp::topology::MAX_CPUS;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Bloc de statistiques par CPU (cache-aligné sur 64 octets)
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C, align(64))]
pub struct CpuStats {
    pub context_switches: AtomicU64,
    pub involuntary_sw: AtomicU64,
    pub voluntary_sw: AtomicU64,
    pub migrations_sent: AtomicU64,
    pub migrations_rcvd: AtomicU64,
    pub idle_time_ns: AtomicU64,
    pub run_time_ns: AtomicU64,
    pub ticks: AtomicU64,
}

impl CpuStats {
    const fn new() -> Self {
        Self {
            context_switches: AtomicU64::new(0),
            involuntary_sw: AtomicU64::new(0),
            voluntary_sw: AtomicU64::new(0),
            migrations_sent: AtomicU64::new(0),
            migrations_rcvd: AtomicU64::new(0),
            idle_time_ns: AtomicU64::new(0),
            run_time_ns: AtomicU64::new(0),
            ticks: AtomicU64::new(0),
        }
    }
}

const INIT_STATS: CpuStats = CpuStats::new();
static CPU_STATS: [CpuStats; MAX_CPUS] = [INIT_STATS; MAX_CPUS];

// ─────────────────────────────────────────────────────────────────────────────
// Accesseurs
// ─────────────────────────────────────────────────────────────────────────────

pub fn stats(cpu: usize) -> Option<&'static CpuStats> {
    CPU_STATS.get(cpu)
}

pub fn inc_context_switches(cpu: usize, voluntary: bool) {
    if let Some(s) = CPU_STATS.get(cpu) {
        s.context_switches.fetch_add(1, Ordering::Relaxed);
        if voluntary {
            s.voluntary_sw.fetch_add(1, Ordering::Relaxed);
        } else {
            s.involuntary_sw.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub fn add_run_time(cpu: usize, ns: u64) {
    if let Some(s) = CPU_STATS.get(cpu) {
        s.run_time_ns.fetch_add(ns, Ordering::Relaxed);
    }
}

pub fn add_idle_time(cpu: usize, ns: u64) {
    if let Some(s) = CPU_STATS.get(cpu) {
        s.idle_time_ns.fetch_add(ns, Ordering::Relaxed);
    }
}

pub fn inc_ticks(cpu: usize) {
    if let Some(s) = CPU_STATS.get(cpu) {
        s.ticks.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn inc_migrations_sent(cpu: usize) {
    if let Some(s) = CPU_STATS.get(cpu) {
        s.migrations_sent.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn inc_migrations_rcvd(cpu: usize) {
    if let Some(s) = CPU_STATS.get(cpu) {
        s.migrations_rcvd.fetch_add(1, Ordering::Relaxed);
    }
}
