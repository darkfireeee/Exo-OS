//! alert.rs — Système d'alertes ExoFS (no_std).

use core::sync::atomic::{AtomicU64, Ordering};
use crate::arch::time::read_ticks;

const ALERT_RING_SIZE: usize = 256;

pub static ALERT_LOG: AlertLog = AlertLog::new_const();

/// Niveau d'alerte.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AlertLevel {
    Info     = 0,
    Warning  = 1,
    Error    = 2,
    Critical = 3,
}

/// Entrée d'alerte.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Alert {
    pub tick:    u64,
    pub level:   AlertLevel,
    pub code:    u16,
    pub _pad:    [u8; 5],
    pub msg:     [u8; 48],   // Message ASCII paddé de zéros.
}

const _: () = assert!(core::mem::size_of::<Alert>() == 64);

pub struct AlertLog {
    ring:    [core::cell::UnsafeCell<Alert>; ALERT_RING_SIZE],
    head:    AtomicU64,
    n_crit:  AtomicU64,
}

// SAFETY: Accès concurrent géré par fetch_add atomique.
unsafe impl Sync for AlertLog {}

impl AlertLog {
    pub const fn new_const() -> Self {
        const ZERO: core::cell::UnsafeCell<Alert> = core::cell::UnsafeCell::new(Alert {
            tick: 0, level: AlertLevel::Info, code: 0, _pad: [0;5], msg: [0;48],
        });
        Self { ring: [ZERO; ALERT_RING_SIZE], head: AtomicU64::new(0), n_crit: AtomicU64::new(0) }
    }

    pub fn push(&self, level: AlertLevel, code: u16, msg: &[u8]) {
        let idx = self.head.fetch_add(1, Ordering::Relaxed) as usize % ALERT_RING_SIZE;
        let mut msg_arr = [0u8; 48];
        let n = msg.len().min(48);
        msg_arr[..n].copy_from_slice(&msg[..n]);
        let entry = Alert { tick: read_ticks(), level, code, _pad: [0;5], msg: msg_arr };
        // SAFETY: accès exclusif garanti par index tournant atomique.
        unsafe { *self.ring[idx].get() = entry; }
        if level >= AlertLevel::Critical {
            self.n_crit.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn critical_count(&self) -> u64 { self.n_crit.load(Ordering::Relaxed) }

    pub fn tail(&self) -> Alert {
        let head = self.head.load(Ordering::Relaxed) as usize;
        let idx  = (head + ALERT_RING_SIZE - 1) % ALERT_RING_SIZE;
        // SAFETY: lecture sans lock acceptable pour diagnostic dernier événement.
        unsafe { *self.ring[idx].get() }
    }
}
