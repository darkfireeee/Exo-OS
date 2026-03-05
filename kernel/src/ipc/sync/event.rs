// ipc/sync/event.rs — Événement IPC binaire et compteur pour Exo-OS
//
// Ce module implémente deux primitives de synchronisation légères :
//
// 1. `IpcEvent` — événement binaire (set/clear/wait) à base d'AtomicU32.
//    Un seul bit d'état suffit. Supporte le mode "auto-reset" (le flag est
//    effacé dès qu'un waiter est réveillé) et le mode "manual-reset"
//    (reste positionné jusqu'à clear() explicite).
//
// 2. `IpcCountingEvent` — compteur d'événements (sémaphore léger) :
//    chaque `signal()` incrémente un compteur ; `wait()` le décrémente.
//    Si le compteur est nul, le thread spin-attend.
//
// RÈGLE EVENT-01 : pas de Vec, pas de Box. Les tables sont statiques.
// RÈGLE EVENT-02 : le spin-wait inclut un compteur de timeout.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicBool, Ordering};
use core::mem::MaybeUninit;

use crate::ipc::core::types::{ChannelId, IpcError};
use crate::scheduler::sync::spinlock::SpinLock;

// ---------------------------------------------------------------------------
// IpcEvent — événement binaire
// ---------------------------------------------------------------------------

/// Mode de réinitialisation de l'événement
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum EventMode {
    /// L'événement est effacé automatiquement lors du premier réveil
    AutoReset = 0,
    /// L'événement reste positionné jusqu'à `clear()` explicite
    ManualReset = 1,
}

/// États internes de l'IpcEvent
mod event_state {
    pub const CLEAR: u32 = 0;
    pub const SET: u32 = 1;
}

/// Événement IPC binaire.  
///
/// Thread-safe, no_std. Le spin-wait est borné par `spin_max` iterations.
#[repr(C, align(64))]
pub struct IpcEvent {
    /// Nom opaque (debug)
    pub id: u32,
    /// État courant
    state: AtomicU32,
    /// Mode auto-reset vs manual-reset
    mode: AtomicU32,
    /// Nombre de waiters courants
    waiter_count: AtomicU32,
    /// Statistiques
    pub total_sets: AtomicU64,
    pub total_clears: AtomicU64,
    pub total_waits: AtomicU64,
    pub total_timeouts: AtomicU64,
    _pad: [u8; 16],
}

// SAFETY: tous les champs internes sont atomiques
unsafe impl Sync for IpcEvent {}
unsafe impl Send for IpcEvent {}

impl IpcEvent {
    pub const fn new(id: u32, mode: EventMode) -> Self {
        Self {
            id,
            state: AtomicU32::new(event_state::CLEAR),
            mode: AtomicU32::new(mode as u32),
            waiter_count: AtomicU32::new(0),
            total_sets: AtomicU64::new(0),
            total_clears: AtomicU64::new(0),
            total_waits: AtomicU64::new(0),
            total_timeouts: AtomicU64::new(0),
            _pad: [0u8; 16],
        }
    }

    /// Positionne l'événement (réveille les waiters si AUTO_RESET ou tous si MANUAL).
    pub fn set(&self) {
        self.state.store(event_state::SET, Ordering::Release);
        self.total_sets.fetch_add(1, Ordering::Relaxed);
    }

    /// Efface l'événement manuellement.
    pub fn clear(&self) {
        self.state.store(event_state::CLEAR, Ordering::Release);
        self.total_clears.fetch_add(1, Ordering::Relaxed);
    }

    /// Vérifie l'état sans attendre.
    pub fn is_set(&self) -> bool {
        self.state.load(Ordering::Acquire) == event_state::SET
    }

    /// Attend que l'événement soit positionné.
    ///
    /// - `spin_max=0` → attente infinie
    /// - En mode AUTO_RESET : efface l'événement atomiquement avant de retourner
    ///
    /// Retourne `Ok(())` si l'événement a été détecté,  
    /// `Err(IpcError::Timeout)` si le timeout a expiré.
    pub fn wait(&self, spin_max: u64) -> Result<(), IpcError> {
        self.waiter_count.fetch_add(1, Ordering::Relaxed);
        self.total_waits.fetch_add(1, Ordering::Relaxed);

        let limit = if spin_max == 0 { u64::MAX } else { spin_max };
        let mut spins = 0u64;

        loop {
            core::hint::spin_loop();
            spins += 1;

            if self.state.load(Ordering::Acquire) == event_state::SET {
                let mode = self.mode.load(Ordering::Relaxed);
                if mode == EventMode::AutoReset as u32 {
                    // CAS pour prendre l'événement en exclusion mutuelle
                    match self.state.compare_exchange(
                        event_state::SET,
                        event_state::CLEAR,
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => {
                            self.waiter_count.fetch_sub(1, Ordering::Relaxed);
                            return Ok(());
                        }
                        Err(_) => {
                            // Un autre thread a pris l'événement, continuer à attendre
                            continue;
                        }
                    }
                } else {
                    // ManualReset : tout le monde est réveillé
                    self.waiter_count.fetch_sub(1, Ordering::Relaxed);
                    return Ok(());
                }
            }

            if spins >= limit {
                self.waiter_count.fetch_sub(1, Ordering::Relaxed);
                self.total_timeouts.fetch_add(1, Ordering::Relaxed);
                return Err(IpcError::Timeout);
            }
        }
    }

    /// Attend et efface en une seule opération (atomique pour AUTO_RESET).
    pub fn wait_and_clear(&self, spin_max: u64) -> Result<(), IpcError> {
        self.wait(spin_max)
    }

    /// Snapshot statistiques
    pub fn snapshot(&self) -> IpcEventStats {
        IpcEventStats {
            id: self.id,
            is_set: self.is_set(),
            waiter_count: self.waiter_count.load(Ordering::Relaxed),
            total_sets: self.total_sets.load(Ordering::Relaxed),
            total_clears: self.total_clears.load(Ordering::Relaxed),
            total_waits: self.total_waits.load(Ordering::Relaxed),
            total_timeouts: self.total_timeouts.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct IpcEventStats {
    pub id: u32,
    pub is_set: bool,
    pub waiter_count: u32,
    pub total_sets: u64,
    pub total_clears: u64,
    pub total_waits: u64,
    pub total_timeouts: u64,
}

// ---------------------------------------------------------------------------
// IpcCountingEvent — sémaphore léger basé sur un compteur atomique
// ---------------------------------------------------------------------------

/// Valeur maximale du compteur (protection overflow)
pub const MAX_EVENT_COUNT: u32 = 65535;

/// Sémaphore léger IPC.
///
/// `signal()` → incrémente le compteur.  
/// `wait()` → décrémente si compteur > 0, sinon spin-wait.
#[repr(C, align(64))]
pub struct IpcCountingEvent {
    pub id: u32,
    count: AtomicU32,
    max_count: AtomicU32,
    pub total_signals: AtomicU64,
    pub total_waits: AtomicU64,
    pub total_timeouts: AtomicU64,
    _pad: [u8; 16],
}

unsafe impl Sync for IpcCountingEvent {}
unsafe impl Send for IpcCountingEvent {}

impl IpcCountingEvent {
    pub const fn new(id: u32, initial: u32, max_count: u32) -> Self {
        let cap = if max_count > MAX_EVENT_COUNT { MAX_EVENT_COUNT } else { max_count };
        let init = if initial > cap { cap } else { initial };
        Self {
            id,
            count: AtomicU32::new(init),
            max_count: AtomicU32::new(cap),
            total_signals: AtomicU64::new(0),
            total_waits: AtomicU64::new(0),
            total_timeouts: AtomicU64::new(0),
            _pad: [0u8; 16],
        }
    }

    /// Incrémente le compteur (signal). Revient immédiatement.
    /// Retourne `Err(IpcError::Overflow)` si déjà au maximum.
    pub fn signal(&self) -> Result<(), IpcError> {
        let max = self.max_count.load(Ordering::Relaxed);
        loop {
            let old = self.count.load(Ordering::Relaxed);
            if old >= max {
                return Err(IpcError::Invalid);
            }
            match self.count.compare_exchange_weak(
                old,
                old + 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.total_signals.fetch_add(1, Ordering::Relaxed);
                    return Ok(());
                }
                Err(_) => core::hint::spin_loop(),
            }
        }
    }

    /// Attend que le compteur soit > 0 puis décrémente.
    pub fn wait(&self, spin_max: u64) -> Result<(), IpcError> {
        self.total_waits.fetch_add(1, Ordering::Relaxed);
        let limit = if spin_max == 0 { u64::MAX } else { spin_max };
        let mut spins = 0u64;

        loop {
            core::hint::spin_loop();
            spins += 1;

            let old = self.count.load(Ordering::Acquire);
            if old > 0 {
                match self.count.compare_exchange_weak(
                    old,
                    old - 1,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return Ok(()),
                    Err(_) => continue,
                }
            }

            if spins >= limit {
                self.total_timeouts.fetch_add(1, Ordering::Relaxed);
                return Err(IpcError::Timeout);
            }
        }
    }

    /// Tente de décrémenter sans attendre.
    pub fn try_wait(&self) -> bool {
        loop {
            let old = self.count.load(Ordering::Acquire);
            if old == 0 {
                return false;
            }
            if self.count.compare_exchange_weak(
                old,
                old - 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ).is_ok() {
                return true;
            }
        }
    }

    /// Valeur courante du compteur.
    pub fn count(&self) -> u32 {
        self.count.load(Ordering::Relaxed)
    }

    /// Reset le compteur à `initial`.
    pub fn reset(&self, initial: u32) {
        let max = self.max_count.load(Ordering::Relaxed);
        let v = if initial > max { max } else { initial };
        self.count.store(v, Ordering::Release);
    }
}

// ---------------------------------------------------------------------------
// Table globale d'événements IPC
// ---------------------------------------------------------------------------

/// Nombre maximal d'IpcEvent dans la table globale
pub const MAX_IPC_EVENTS: usize = 128;

/// Slot dans la table globale
#[repr(C)]
struct EventSlot {
    event: MaybeUninit<IpcEvent>,
    occupied: AtomicBool,
}

impl EventSlot {
    const fn empty() -> Self {
        Self {
            event: MaybeUninit::uninit(),
            occupied: AtomicBool::new(false),
        }
    }
}

/// Table globale des IpcEvent
struct IpcEventTable {
    slots: [EventSlot; MAX_IPC_EVENTS],
    count: AtomicU32,
}

// SAFETY: accès protégé par CAS sur `occupied`
unsafe impl Sync for IpcEventTable {}

impl IpcEventTable {
    const fn new() -> Self {
        const EMPTY: EventSlot = EventSlot::empty();
        Self {
            slots: [EMPTY; MAX_IPC_EVENTS],
            count: AtomicU32::new(0),
        }
    }

    fn alloc(&self, id: u32, mode: EventMode) -> Option<usize> {
        for i in 0..MAX_IPC_EVENTS {
            if !self.slots[i].occupied.load(Ordering::Relaxed) {
                if self.slots[i].occupied.compare_exchange(
                    false,
                    true,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                ).is_ok() {
                    let ptr = self.slots[i].event.as_ptr() as *mut IpcEvent;
                    // SAFETY: CAS AcqRel garantit l'exclusivité; MaybeUninit<IpcEvent> write-once.
                    unsafe {
                        ptr.write(IpcEvent::new(id, mode));
                    }
                    self.count.fetch_add(1, Ordering::Relaxed);
                    return Some(i);
                }
            }
        }
        None
    }

    fn get(&self, idx: usize) -> Option<&IpcEvent> {
        if idx >= MAX_IPC_EVENTS {
            return None;
        }
        if !self.slots[idx].occupied.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: slot initialized (occupied=true)
        Some(unsafe { &*self.slots[idx].event.as_ptr() })
    }

    fn free(&self, idx: usize) -> bool {
        if idx >= MAX_IPC_EVENTS {
            return false;
        }
        if self.slots[idx].occupied.compare_exchange(
            true,
            false,
            Ordering::AcqRel,
            Ordering::Relaxed,
        ).is_ok() {
            self.count.fetch_sub(1, Ordering::Relaxed);
            return true;
        }
        false
    }

    fn count(&self) -> u32 {
        self.count.load(Ordering::Relaxed)
    }
}

// Table statique globale
static IPC_EVENT_TABLE: IpcEventTable = IpcEventTable::new();

// ---------------------------------------------------------------------------
// API publique
// ---------------------------------------------------------------------------

/// Crée un événement IPC dans la table globale.
/// Retourne l'index (handle) de l'événement.
pub fn event_create(id: u32, mode: EventMode) -> Option<usize> {
    IPC_EVENT_TABLE.alloc(id, mode)
}

/// Positionne l'événement `idx`.
pub fn event_set(idx: usize) -> Result<(), IpcError> {
    IPC_EVENT_TABLE.get(idx).ok_or(IpcError::InvalidHandle).map(|e| e.set())
}

/// Efface l'événement `idx`.
pub fn event_clear(idx: usize) -> Result<(), IpcError> {
    IPC_EVENT_TABLE.get(idx).ok_or(IpcError::InvalidHandle).map(|e| e.clear())
}

/// Vérifie l'état de l'événement sans attendre.
pub fn event_is_set(idx: usize) -> Option<bool> {
    IPC_EVENT_TABLE.get(idx).map(|e| e.is_set())
}

/// Attend l'événement `idx` avec spin-max.
pub fn event_wait(idx: usize, spin_max: u64) -> Result<(), IpcError> {
    IPC_EVENT_TABLE.get(idx).ok_or(IpcError::InvalidHandle)?.wait(spin_max)
}

/// Détruit un événement (libère son slot).
pub fn event_destroy(idx: usize) -> bool {
    IPC_EVENT_TABLE.free(idx)
}

/// Compteur d'événements créés.
pub fn event_count() -> u32 {
    IPC_EVENT_TABLE.count()
}

/// Récupère le snapshot de statistiques pour un événement.
pub fn event_stats(idx: usize) -> Option<IpcEventStats> {
    IPC_EVENT_TABLE.get(idx).map(|e| e.snapshot())
}
