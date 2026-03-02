//! CachePressure — surveillance de la pression mémoire sur le cache ExoFS (no_std).

use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

/// Niveaux de pression mémoire.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PressureLevel {
    Normal   = 0,
    Low      = 1,
    Medium   = 2,
    High     = 3,
    Critical = 4,
}

pub static CACHE_PRESSURE: CachePressure = CachePressure::new_const();

pub struct CachePressure {
    level:           AtomicU8,
    free_bytes:      AtomicU64,
    total_bytes:     AtomicU64,
    pressure_ticks:  AtomicU64,
}

impl CachePressure {
    pub const fn new_const() -> Self {
        Self {
            level:          AtomicU8::new(PressureLevel::Normal as u8),
            free_bytes:     AtomicU64::new(0),
            total_bytes:    AtomicU64::new(0),
            pressure_ticks: AtomicU64::new(0),
        }
    }

    pub fn update(&self, free_bytes: u64, total_bytes: u64) {
        self.free_bytes.store(free_bytes, Ordering::Release);
        self.total_bytes.store(total_bytes, Ordering::Release);

        let level = if total_bytes == 0 {
            PressureLevel::Normal
        } else {
            let used_pct = (total_bytes - free_bytes) * 100 / total_bytes;
            match used_pct {
                0..=60  => PressureLevel::Normal,
                61..=75 => PressureLevel::Low,
                76..=85 => PressureLevel::Medium,
                86..=94 => PressureLevel::High,
                _       => PressureLevel::Critical,
            }
        };

        let old: PressureLevel = match self.level.swap(level as u8, Ordering::SeqCst) {
            1 => PressureLevel::Low,
            2 => PressureLevel::Medium,
            3 => PressureLevel::High,
            4 => PressureLevel::Critical,
            _ => PressureLevel::Normal,
        };

        if level > old {
            self.pressure_ticks.store(crate::arch::time::read_ticks(), Ordering::Relaxed);
        }
    }

    pub fn level(&self) -> PressureLevel {
        match self.level.load(Ordering::Acquire) {
            1 => PressureLevel::Low,
            2 => PressureLevel::Medium,
            3 => PressureLevel::High,
            4 => PressureLevel::Critical,
            _ => PressureLevel::Normal,
        }
    }

    pub fn is_under_pressure(&self) -> bool {
        self.level() >= PressureLevel::Medium
    }

    pub fn free_bytes(&self) -> u64 { self.free_bytes.load(Ordering::Acquire) }
    pub fn total_bytes(&self) -> u64 { self.total_bytes.load(Ordering::Acquire) }
    pub fn last_pressure_tick(&self) -> u64 { self.pressure_ticks.load(Ordering::Relaxed) }
}
