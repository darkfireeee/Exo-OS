//! RecoveryLog — journal des opérations de récupération ExoFS (no_std).

use core::sync::atomic::{AtomicU64, Ordering};
use crate::arch::time::read_ticks;
use super::boot_recovery::RecoveryEvent;

const RECOVERY_LOG_SIZE: usize = 1024;

pub static RECOVERY_LOG: RecoveryLog = RecoveryLog::new_const();

/// Entrée du journal de récupération.
#[derive(Clone, Copy, Debug)]
pub struct RecoveryLogEntry {
    pub tick:  u64,
    pub event: RecoveryEventKind,
    pub data:  u64,
}

/// Version sérialisable de RecoveryEvent.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryEventKind {
    BootStart    = 0x01,
    SlotSelected = 0x02,
    ReplayStart  = 0x03,
    ReplayDone   = 0x04,
    BootDone     = 0x05,
    FsckStarted  = 0x06,
    FsckDone     = 0x07,
    RepairApplied= 0x08,
}

pub struct RecoveryLog {
    ring: [core::cell::UnsafeCell<RecoveryLogEntry>; RECOVERY_LOG_SIZE],
    head: AtomicU64,
    count: AtomicU64,
}

// SAFETY: accès géré par atomic fetch_add sur head.
unsafe impl Sync for RecoveryLog {}

impl RecoveryLog {
    pub const fn new_const() -> Self {
        const ZERO: core::cell::UnsafeCell<RecoveryLogEntry> =
            core::cell::UnsafeCell::new(RecoveryLogEntry {
                tick: 0,
                event: RecoveryEventKind::BootStart,
                data: 0,
            });
        Self { ring: [ZERO; RECOVERY_LOG_SIZE], head: AtomicU64::new(0), count: AtomicU64::new(0) }
    }

    pub fn log_event(&self, event: RecoveryEvent) {
        let (kind, data) = match event {
            RecoveryEvent::BootStart                => (RecoveryEventKind::BootStart, 0),
            RecoveryEvent::SlotSelected(s)          => (RecoveryEventKind::SlotSelected, s.0 as u64),
            RecoveryEvent::ReplayStart              => (RecoveryEventKind::ReplayStart, 0),
            RecoveryEvent::ReplayDone               => (RecoveryEventKind::ReplayDone, 0),
            RecoveryEvent::BootDone                 => (RecoveryEventKind::BootDone, 0),
            RecoveryEvent::FsckStarted              => (RecoveryEventKind::FsckStarted, 0),
            RecoveryEvent::FsckDone                 => (RecoveryEventKind::FsckDone, 0),
            RecoveryEvent::RepairApplied(n)         => (RecoveryEventKind::RepairApplied, n as u64),
        };
        let entry = RecoveryLogEntry { tick: read_ticks(), event: kind, data };
        let idx = self.head.fetch_add(1, Ordering::Relaxed) as usize % RECOVERY_LOG_SIZE;
        // SAFETY: Slot tournant géré par fetch_add atomique.
        unsafe { *self.ring[idx].get() = entry; }
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn count(&self) -> u64 { self.count.load(Ordering::Relaxed) }

    pub fn read_recent(&self, n: usize) -> alloc::vec::Vec<RecoveryLogEntry> {
        let total = self.count.load(Ordering::Relaxed) as usize;
        let head  = self.head.load(Ordering::Relaxed) as usize;
        let n     = n.min(RECOVERY_LOG_SIZE).min(total);
        let mut out = alloc::vec::Vec::new();
        let _ = out.try_reserve(n);
        for i in 0..n {
            let idx = (head + RECOVERY_LOG_SIZE - n + i) % RECOVERY_LOG_SIZE;
            // SAFETY: Lecture diagnostique sans lock.
            let e = unsafe { *self.ring[idx].get() };
            out.push(e);
        }
        out
    }
}
