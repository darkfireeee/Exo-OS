// kernel/src/process/resource/usage.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Comptabilité des ressources (getrusage) — Exo-OS Couche 1.5
// ═══════════════════════════════════════════════════════════════════════════════


use core::sync::atomic::{AtomicU64, Ordering};

/// Cible de getrusage(2).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum RUsageWho {
    /// Processus courant.
    RSelf      =  0,
    /// Processus enfants terminés.
    RChildren  = -1,
    /// Thread courant.
    RThread    =  1,
}

/// Statistiques de ressource (struct rusage POSIX).
#[repr(C)]
pub struct RUsage {
    /// Temps CPU utilisateur (µs).
    pub utime_us:         AtomicU64,
    /// Temps CPU système (µs).
    pub stime_us:         AtomicU64,
    /// Utilisation mémoire maximale (RSS en pages).
    pub maxrss_pages:     AtomicU64,
    /// Fautes de page mineures.
    pub minor_faults:     AtomicU64,
    /// Fautes de page majeures.
    pub major_faults:     AtomicU64,
    /// Commutations de contexte volontaires.
    pub vol_ctx_switches: AtomicU64,
    /// Commutations de contexte non volontaires.
    pub invol_ctx_switches: AtomicU64,
    /// Nombre de syscalls.
    pub syscalls:         AtomicU64,
    /// Nombre d'appels read().
    pub inblock:          AtomicU64,
    /// Nombre d'appels write().
    pub outblock:         AtomicU64,
    /// Signaux reçus.
    pub signals_received: AtomicU64,
    /// Nombre de messages IPC envoyés.
    pub msgsnd:           AtomicU64,
    /// Nombre de messages IPC reçus.
    pub msgrcv:           AtomicU64,
}

impl RUsage {
    pub const fn new() -> Self {
        Self {
            utime_us:           AtomicU64::new(0),
            stime_us:           AtomicU64::new(0),
            maxrss_pages:       AtomicU64::new(0),
            minor_faults:       AtomicU64::new(0),
            major_faults:       AtomicU64::new(0),
            vol_ctx_switches:   AtomicU64::new(0),
            invol_ctx_switches: AtomicU64::new(0),
            syscalls:           AtomicU64::new(0),
            inblock:            AtomicU64::new(0),
            outblock:           AtomicU64::new(0),
            signals_received:   AtomicU64::new(0),
            msgsnd:             AtomicU64::new(0),
            msgrcv:             AtomicU64::new(0),
        }
    }

    // Les méthodes de mise à jour sont des wrappers atomiques :

    pub fn add_utime(&self, us: u64) {
        self.utime_us.fetch_add(us, Ordering::Relaxed);
    }

    pub fn add_stime(&self, us: u64) {
        self.stime_us.fetch_add(us, Ordering::Relaxed);
    }

    pub fn update_maxrss(&self, pages: u64) {
        let mut current = self.maxrss_pages.load(Ordering::Relaxed);
        while pages > current {
            match self.maxrss_pages.compare_exchange_weak(
                current, pages, Ordering::Relaxed, Ordering::Relaxed,
            ) {
                Ok(_)  => break,
                Err(v) => current = v,
            }
        }
    }

    pub fn add_minor_fault(&self)  { self.minor_faults.fetch_add(1, Ordering::Relaxed); }
    pub fn add_major_fault(&self)  { self.major_faults.fetch_add(1, Ordering::Relaxed); }
    pub fn add_vol_ctx(&self)      { self.vol_ctx_switches.fetch_add(1, Ordering::Relaxed); }
    pub fn add_invol_ctx(&self)    { self.invol_ctx_switches.fetch_add(1, Ordering::Relaxed); }
    pub fn add_syscall(&self)      { self.syscalls.fetch_add(1, Ordering::Relaxed); }
    pub fn add_signal(&self)       { self.signals_received.fetch_add(1, Ordering::Relaxed); }

    /// Accumule les statistiques d'un enfant terminé dans les statistiques enfant.
    pub fn accumulate_child(&self, child: &RUsage) {
        self.utime_us.fetch_add(child.utime_us.load(Ordering::Relaxed), Ordering::Relaxed);
        self.stime_us.fetch_add(child.stime_us.load(Ordering::Relaxed), Ordering::Relaxed);
        self.minor_faults.fetch_add(child.minor_faults.load(Ordering::Relaxed), Ordering::Relaxed);
        self.major_faults.fetch_add(child.major_faults.load(Ordering::Relaxed), Ordering::Relaxed);
    }

    /// Copie snapshot vers un buffer C-style (pour syscall getrusage).
    pub fn snapshot(&self) -> RUsageSnapshot {
        RUsageSnapshot {
            utime_us:           self.utime_us.load(Ordering::Relaxed),
            stime_us:           self.stime_us.load(Ordering::Relaxed),
            maxrss_pages:       self.maxrss_pages.load(Ordering::Relaxed),
            minor_faults:       self.minor_faults.load(Ordering::Relaxed),
            major_faults:       self.major_faults.load(Ordering::Relaxed),
            vol_ctx_switches:   self.vol_ctx_switches.load(Ordering::Relaxed),
            invol_ctx_switches: self.invol_ctx_switches.load(Ordering::Relaxed),
            syscalls:           self.syscalls.load(Ordering::Relaxed),
            inblock:            self.inblock.load(Ordering::Relaxed),
            outblock:           self.outblock.load(Ordering::Relaxed),
            signals_received:   self.signals_received.load(Ordering::Relaxed),
            msgsnd:             self.msgsnd.load(Ordering::Relaxed),
            msgrcv:             self.msgrcv.load(Ordering::Relaxed),
        }
    }
}

/// Snapshot non-atomique de RUsage (pour copie vers userland).
#[derive(Copy, Clone, Debug, Default)]
#[repr(C)]
pub struct RUsageSnapshot {
    pub utime_us:           u64,
    pub stime_us:           u64,
    pub maxrss_pages:       u64,
    pub minor_faults:       u64,
    pub major_faults:       u64,
    pub vol_ctx_switches:   u64,
    pub invol_ctx_switches: u64,
    pub syscalls:           u64,
    pub inblock:            u64,
    pub outblock:           u64,
    pub signals_received:   u64,
    pub msgsnd:             u64,
    pub msgrcv:             u64,
}
