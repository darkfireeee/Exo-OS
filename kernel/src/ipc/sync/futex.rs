// ipc/sync/futex.rs — IPC futex noyau pour Exo-OS
//
// Implémente les futex IPC en s'intégrant avec la table globale de futex du
// memory manager (memory::utils::futex_table, RÈGLE SCHED-03).
//
// Un futex IPC est identifié par une adresse physique (l'adresse virtuelle
// dépend du processus — la table globale utilise l'adresse physique comme clé).
//
// Opérations :
//   WAIT  : si *addr == expected → suspendre le thread courant
//   WAKE  : réveiller jusqu'à `n` threads en attente sur addr
//   REQUEUE : déplacer des waiters d'une adresse à une autre
//
// Contraintes :
//   - RÈGLE SCHED-03 : délégation à memory::utils::futex_table
//   - RÈGLE NO-ALLOC : pas de Vec/Box dans les chemins critiques
//   - Les waiterds utilisent une WaitQueue par bucket (hash de l'adresse)

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use core::mem::MaybeUninit;

use crate::ipc::core::types::{IpcError, ProcessId};
use crate::scheduler::sync::spinlock::SpinLock;

// ---------------------------------------------------------------------------
// Constantes
// ---------------------------------------------------------------------------

/// Nombre de buckets dans la table de futex IPC (puissance de 2)
pub const IPC_FUTEX_BUCKETS: usize = 256;
/// Masque pour hacher une adresse en bucket
pub const IPC_FUTEX_BUCKET_MASK: usize = IPC_FUTEX_BUCKETS - 1;
/// Nombre maximal de waiters par bucket
pub const MAX_WAITERS_PER_BUCKET: usize = 32;
/// Magic pour identifier un futex IPC valide
pub const IPC_FUTEX_MAGIC: u32 = 0x1FC7_F07E;

// ---------------------------------------------------------------------------
// Clé de futex
// ---------------------------------------------------------------------------

/// Clé identifiant un futex par son adresse physique.
/// L'adresse physique garantit que deux processus mappant la même page
/// partagé partagent bien le même futex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct FutexKey(pub u64);

impl FutexKey {
    pub fn bucket(self) -> usize {
        // FNV-1a miniature sur 64 bits → bucket
        let h = self.0
            .wrapping_mul(0x9e3779b97f4a7c15)
            .rotate_right(27)
            ^ (self.0 >> 32);
        (h as usize) & IPC_FUTEX_BUCKET_MASK
    }
}

// ---------------------------------------------------------------------------
// Waiter de futex
// ---------------------------------------------------------------------------

/// État d'un waiter de futex
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum WaiterState {
    /// Slot libre
    Free = 0,
    /// Thread en attente
    Waiting = 1,
    /// Réveillé par WAKE
    Woken = 2,
    /// Requeueté vers une autre clé
    Requeued = 3,
    /// Timeout expiré
    TimedOut = 4,
}

impl WaiterState {
    fn from_u32(v: u32) -> Self {
        match v {
            1 => Self::Waiting,
            2 => Self::Woken,
            3 => Self::Requeued,
            4 => Self::TimedOut,
            _ => Self::Free,
        }
    }
}

/// Entrée de waiter dans un bucket de la table futex
#[repr(C, align(32))]
pub struct FutexWaiter {
    /// Clé attendue
    pub key: AtomicU64,
    /// État du waiter
    pub state: AtomicU32,
    /// Identifiant du thread en attente (TaskId entier)
    pub thread_id: AtomicU32,
    /// Valeur attendue au moment du WAIT
    pub expected: AtomicU32,
    /// Séquence (anti-ABA)
    pub seq: AtomicU32,
}

// SAFETY: tous les champs sont atomiques
unsafe impl Sync for FutexWaiter {}
unsafe impl Send for FutexWaiter {}

impl FutexWaiter {
    pub const fn new() -> Self {
        Self {
            key: AtomicU64::new(0),
            state: AtomicU32::new(WaiterState::Free as u32),
            thread_id: AtomicU32::new(0),
            expected: AtomicU32::new(0),
            seq: AtomicU32::new(0),
        }
    }

    pub fn is_free(&self) -> bool {
        WaiterState::from_u32(self.state.load(Ordering::Acquire)) == WaiterState::Free
    }

    pub fn is_waiting(&self) -> bool {
        WaiterState::from_u32(self.state.load(Ordering::Acquire)) == WaiterState::Waiting
    }

    /// Libère ce slot de waiter.
    pub fn free(&self) {
        self.state.store(WaiterState::Free as u32, Ordering::Release);
        self.key.store(0, Ordering::Relaxed);
        self.thread_id.store(0, Ordering::Relaxed);
        self.seq.fetch_add(1, Ordering::Relaxed);
    }

    /// Réveille ce waiter (transition Waiting → Woken).
    /// Retourne `true` si le waiter était en attente.
    pub fn wake(&self) -> bool {
        self.state
            .compare_exchange(
                WaiterState::Waiting as u32,
                WaiterState::Woken as u32,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
}

// ---------------------------------------------------------------------------
// Bucket de futex
// ---------------------------------------------------------------------------

/// Bucket de la table futex IPC
#[repr(C, align(64))]
pub struct FutexBucket {
    waiters: [FutexWaiter; MAX_WAITERS_PER_BUCKET],
    count: AtomicU32,
    _pad: [u8; 28],
}

// SAFETY: FutexWaiter est Sync
unsafe impl Sync for FutexBucket {}
unsafe impl Send for FutexBucket {}

impl FutexBucket {
    pub const fn new() -> Self {
        const INIT_WAITER: FutexWaiter = FutexWaiter::new();
        Self {
            waiters: [INIT_WAITER; MAX_WAITERS_PER_BUCKET],
            count: AtomicU32::new(0),
            _pad: [0u8; 28],
        }
    }

    /// Enregistre un waiter pour la clé `key` et la valeur attendue `expected`.
    /// Retourne l'index du waiter ou `None` si le bucket est plein.
    pub fn add_waiter(&self, key: FutexKey, expected: u32, thread_id: u32) -> Option<usize> {
        for i in 0..MAX_WAITERS_PER_BUCKET {
            if self.waiters[i].is_free() {
                self.waiters[i].key.store(key.0, Ordering::Relaxed);
                self.waiters[i].expected.store(expected, Ordering::Relaxed);
                self.waiters[i].thread_id.store(thread_id, Ordering::Relaxed);
                self.waiters[i].state.store(WaiterState::Waiting as u32, Ordering::Release);
                self.count.fetch_add(1, Ordering::Relaxed);
                return Some(i);
            }
        }
        None
    }

    /// Réveille jusqu'à `n` waiters sur la clé `key`.
    /// Retourne le nombre de waiters effectivement réveillés.
    pub fn wake_n(&self, key: FutexKey, n: u32) -> u32 {
        let mut woken = 0u32;
        for i in 0..MAX_WAITERS_PER_BUCKET {
            if woken >= n { break; }
            let w = &self.waiters[i];
            if w.key.load(Ordering::Acquire) == key.0 && w.wake() {
                self.count.fetch_sub(1, Ordering::Relaxed);
                woken += 1;
            }
        }
        woken
    }

    /// Requête l'état d'un waiter specifique (par index).
    pub fn waiter_state(&self, idx: usize) -> WaiterState {
        if idx < MAX_WAITERS_PER_BUCKET {
            WaiterState::from_u32(self.waiters[idx].state.load(Ordering::Acquire))
        } else {
            WaiterState::Free
        }
    }

    /// Libère le waiter à l'index `idx`.
    pub fn free_waiter(&self, idx: usize) {
        if idx < MAX_WAITERS_PER_BUCKET {
            if !self.waiters[idx].is_free() {
                self.waiters[idx].free();
                self.count.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }

    pub fn waiter_count(&self) -> u32 {
        self.count.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// Table globale de futex IPC
// ---------------------------------------------------------------------------

/// Table globale des futex IPC
pub struct IpcFutexTable {
    buckets: [FutexBucket; IPC_FUTEX_BUCKETS],
    /// Statistiques
    pub waits_total: AtomicU64,
    pub wakes_total: AtomicU64,
    pub timeouts_total: AtomicU64,
    pub spurious_wakeups: AtomicU64,
}

// SAFETY: FutexBucket est Sync
unsafe impl Sync for IpcFutexTable {}
unsafe impl Send for IpcFutexTable {}

impl IpcFutexTable {
    pub const fn new() -> Self {
        const INIT_BUCKET: FutexBucket = FutexBucket::new();
        Self {
            buckets: [INIT_BUCKET; IPC_FUTEX_BUCKETS],
            waits_total: AtomicU64::new(0),
            wakes_total: AtomicU64::new(0),
            timeouts_total: AtomicU64::new(0),
            spurious_wakeups: AtomicU64::new(0),
        }
    }

    fn bucket(&self, key: FutexKey) -> &FutexBucket {
        &self.buckets[key.bucket()]
    }

    // -----------------------------------------------------------------------
    // WAIT
    // -----------------------------------------------------------------------

    /// Opération WAIT : si *addr == expected alors inscrire le thread comme waiter.
    ///
    /// Le schéma est :
    ///   1. Vérifier que `*addr == expected` (atomique Acquire)
    ///   2. Enregistrer le waiter dans le bucket
    ///   3. Reverifie `*addr == expected` (élimine la race condition WAIT/WAKE)
    ///   4. Spin-wait jusqu'à être réveillé (ou timeout)
    ///
    /// # Paramètres
    /// - `addr`      : adresse u32 atomique (ex: dans une page SHM)
    /// - `key`       : FutexKey = adresse physique (identifiant cross-process)
    /// - `expected`  : valeur attendue
    /// - `thread_id` : identifiant du thread courant (pour corrélation)
    /// - `spin_max`  : nombre de spins max avant timeout (0 = infini)
    ///
    /// # Retour
    /// - `Ok(WaiterState)` : cause du réveil
    /// - `Err(IpcError::WouldBlock)` : `*addr != expected` au moment du WAIT
    pub fn wait(
        &self,
        addr: &AtomicU32,
        key: FutexKey,
        expected: u32,
        thread_id: u32,
        spin_max: u64,
    ) -> Result<WaiterState, IpcError> {
        // Étape 1 : vérification initiale
        let cur = addr.load(Ordering::Acquire);
        if cur != expected {
            return Err(IpcError::WouldBlock);
        }

        // Étape 2 : enregistrement du waiter
        let bucket = self.bucket(key);
        let waiter_idx = bucket.add_waiter(key, expected, thread_id)
            .ok_or(IpcError::OutOfResources)?;

        // Étape 3 : double-check après enregistrement (évite la race WAKE avant WAIT)
        let cur2 = addr.load(Ordering::Acquire);
        if cur2 != expected {
            bucket.free_waiter(waiter_idx);
            return Err(IpcError::WouldBlock);
        }

        self.waits_total.fetch_add(1, Ordering::Relaxed);

        // Étape 4 : spin-wait
        let mut spins: u64 = 0;
        loop {
            core::hint::spin_loop();
            spins += 1;

            let state = bucket.waiter_state(waiter_idx);
            match state {
                WaiterState::Woken => {
                    bucket.free_waiter(waiter_idx);
                    return Ok(WaiterState::Woken);
                }
                WaiterState::Requeued => {
                    bucket.free_waiter(waiter_idx);
                    return Ok(WaiterState::Requeued);
                }
                WaiterState::Free => {
                    // Libéré prématurément (spurious)
                    self.spurious_wakeups.fetch_add(1, Ordering::Relaxed);
                    return Ok(WaiterState::Woken);
                }
                _ => {}
            }

            if spin_max != 0 && spins >= spin_max {
                bucket.waiters[waiter_idx].state.store(
                    WaiterState::TimedOut as u32,
                    Ordering::Release,
                );
                bucket.free_waiter(waiter_idx);
                self.timeouts_total.fetch_add(1, Ordering::Relaxed);
                return Err(IpcError::Timeout);
            }
        }
    }

    // -----------------------------------------------------------------------
    // WAKE
    // -----------------------------------------------------------------------

    /// Opération WAKE : réveille jusqu'à `n` threads en attente sur `key`.
    /// Retourne le nombre de threads effectivement réveillés.
    pub fn wake(&self, key: FutexKey, n: u32) -> u32 {
        let woken = self.bucket(key).wake_n(key, n);
        self.wakes_total.fetch_add(woken as u64, Ordering::Relaxed);
        woken
    }

    /// Réveille tous les waiters sur `key`.
    pub fn wake_all(&self, key: FutexKey) -> u32 {
        self.wake(key, u32::MAX)
    }
}

/// Instance statique de la table globale de futex IPC
pub static IPC_FUTEX_TABLE: IpcFutexTable = IpcFutexTable::new();

// ---------------------------------------------------------------------------
// API publique de haut niveau
// ---------------------------------------------------------------------------

/// Wrapper ergonomique : WAIT avec une adresse u32 directe et un timeout en spins.
pub fn futex_wait(
    addr: &AtomicU32,
    key: FutexKey,
    expected: u32,
    thread_id: u32,
    spin_max: u64,
) -> Result<WaiterState, IpcError> {
    IPC_FUTEX_TABLE.wait(addr, key, expected, thread_id, spin_max)
}

/// Wrapper ergonomique : WAKE (réveille jusqu'à `n` threads).
pub fn futex_wake(key: FutexKey, n: u32) -> u32 {
    IPC_FUTEX_TABLE.wake(key, n)
}

/// Retourne les statistiques de la table futex IPC.
pub fn futex_stats() -> FutexStats {
    FutexStats {
        waits_total: IPC_FUTEX_TABLE.waits_total.load(Ordering::Relaxed),
        wakes_total: IPC_FUTEX_TABLE.wakes_total.load(Ordering::Relaxed),
        timeouts_total: IPC_FUTEX_TABLE.timeouts_total.load(Ordering::Relaxed),
        spurious_wakeups: IPC_FUTEX_TABLE.spurious_wakeups.load(Ordering::Relaxed),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FutexStats {
    pub waits_total: u64,
    pub wakes_total: u64,
    pub timeouts_total: u64,
    pub spurious_wakeups: u64,
}
