//! exo-observability — Primitives d'observabilité, traçage et télémétrie Exo-OS
//!
//! Spécification: docs/Vision v0.2.0/SPEC-EXO-OBSERVABILITY.md
//! Fournit un traçage lock-free, des histogrammes de latence (log2) et
//! la sortie de diagnostic immédiate sur le port QEMU 0xE9.

#![no_std]

pub mod e9_log;

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
pub use e9_log::{write_byte_e9, write_bytes_e9, write_str_e9};
use log::Level;

/// Enregistrement d'événement de trace léger (24 octets)
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TraceEvent {
    pub timestamp_ns: u64,
    pub event_id: u32,
    pub thread_id: u32,
    pub payload: u64,
}

/// Tampon circulaire de traces sans verrou (lock-free) à capacité fixe N (puissance de 2)
pub struct TraceRingBuffer<const N: usize> {
    head: AtomicUsize,
    buffer: [TraceEvent; N],
}

impl<const N: usize> TraceRingBuffer<N> {
    pub const fn new() -> Self {
        assert!(N > 0 && (N & (N - 1)) == 0, "N doit être une puissance de deux");
        Self {
            head: AtomicUsize::new(0),
            buffer: [TraceEvent {
                timestamp_ns: 0,
                event_id: 0,
                thread_id: 0,
                payload: 0,
            }; N],
        }
    }

    /// Enregistre un événement de manière concurrente sans blocage
    #[inline]
    pub fn record(&mut self, event: TraceEvent) {
        let idx = self.head.fetch_add(1, Ordering::Relaxed) & (N - 1);
        self.buffer[idx] = event;
    }

    /// Récupère le dernier événement enregistré
    #[inline]
    pub fn last_event(&self) -> Option<TraceEvent> {
        let cur = self.head.load(Ordering::Relaxed);
        if cur == 0 {
            None
        } else {
            let idx = (cur - 1) & (N - 1);
            Some(self.buffer[idx])
        }
    }

    /// Nombre total d'événements enregistrés depuis le démarrage
    #[inline(always)]
    pub fn total_recorded(&self) -> usize {
        self.head.load(Ordering::Relaxed)
    }
}

/// Histogramme de latence sans allocation avec granularité logarithmique (log2)
pub struct LatencyHistogram {
    /// 64 seaux (buckets) correspondant à log2(nanosecondes)
    buckets: [AtomicU64; 64],
}

impl LatencyHistogram {
    pub const fn new() -> Self {
        // Initialisation statique sans macro const incompatible
        const ZERO: AtomicU64 = AtomicU64::new(0);
        Self {
            buckets: [ZERO; 64],
        }
    }

    /// Enregistre une mesure de latence en nanosecondes
    #[inline]
    pub fn observe_ns(&self, duration_ns: u64) {
        let bucket = if duration_ns == 0 {
            0
        } else {
            (64 - duration_ns.leading_zeros() as usize).min(63)
        };
        self.buckets[bucket].fetch_add(1, Ordering::Relaxed);
    }

    /// Compte d'occurrences pour un seau log2 spécifique
    #[inline]
    pub fn count_at_bucket(&self, bucket: usize) -> u64 {
        if bucket < 64 {
            self.buckets[bucket].load(Ordering::Relaxed)
        } else {
            0
        }
    }
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self::new()
    }
}

/// Compteurs de performance atomiques de base
#[derive(Default)]
pub struct PerfCounters {
    pub syscalls: AtomicU64,
    pub page_faults: AtomicU64,
    pub context_switches: AtomicU64,
    pub irq_count: AtomicU64,
    pub net_packets_rx: AtomicU64,
    pub net_packets_tx: AtomicU64,
}

impl PerfCounters {
    pub const fn new() -> Self {
        Self {
            syscalls: AtomicU64::new(0),
            page_faults: AtomicU64::new(0),
            context_switches: AtomicU64::new(0),
            irq_count: AtomicU64::new(0),
            net_packets_rx: AtomicU64::new(0),
            net_packets_tx: AtomicU64::new(0),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Compatibilité API existante
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExoLogRecord {
    pub level: Level,
    pub target: &'static str,
    pub code: u32,
}

pub const OBSERVABILITY_PORTS: &[(&str, &str, &str)] = &[
    ("log", "log-upstream", "kernel/Ring services facade"),
    ("tracing", "tracing-upstream", "Ring3 structured telemetry"),
];

pub const fn severity_score(level: Level) -> u8 {
    match level {
        Level::Error => 5,
        Level::Warn => 4,
        Level::Info => 3,
        Level::Debug => 2,
        Level::Trace => 1,
    }
}

pub fn observability_stress_signature(iterations: u32) -> u64 {
    let levels = [
        Level::Error,
        Level::Warn,
        Level::Info,
        Level::Debug,
        Level::Trace,
    ];
    let mut acc = 0x4558_4f4c_4f47_u64;
    for i in 0..iterations.max(1) {
        let level = levels[i as usize % levels.len()];
        let rec = ExoLogRecord {
            level,
            target: "exo",
            code: i,
        };
        acc = acc.rotate_left(7) ^ rec.code as u64 ^ severity_score(rec.level) as u64;
    }
    acc
}

