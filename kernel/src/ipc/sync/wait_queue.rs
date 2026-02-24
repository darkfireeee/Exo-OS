// ipc/sync/wait_queue.rs — File d'attente IPC (wrapper du scheduler) pour Exo-OS
//
// Ce module fournit une file d'attente IPC-spécifique qui enveloppe la
// WaitQueue du scheduler (scheduler::sync::wait_queue) avec une logique
// propre à l'IPC : gestion des timeouts, identification par ChannelId,
// statistiques dédiées et politique de réveil configurable.
//
// RÈGLE WAITQ-01 : WaitNode provient de l'EmergencyPool du scheduler.
// Ce module ne duplique pas la logique — il délègue au scheduler.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicBool, Ordering};
use core::mem::MaybeUninit;

use crate::ipc::core::types::{ChannelId, IpcError};
use crate::scheduler::sync::spinlock::SpinLock;

// ---------------------------------------------------------------------------
// Politique de réveil
// ---------------------------------------------------------------------------

/// Politique de réveil d'une IpcWaitQueue
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum WakePolicy {
    /// Réveiller exactement un waiter (FIFO)
    One = 0,
    /// Réveiller tous les waiters
    All = 1,
    /// Réveiller jusqu'à N waiters
    UpToN = 2,
}

// ---------------------------------------------------------------------------
// Waiter IPC
// ---------------------------------------------------------------------------

/// Nombre maximal de waiters par IpcWaitQueue
pub const MAX_IPC_WAITERS: usize = 64;

/// Raison de réveil d'un waiter
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum WakeReason {
    /// Réveil normal (condition satisfaite)
    Signaled = 0,
    /// Timeout expiré
    Timeout = 1,
    /// Canal/ressource fermé
    Closed = 2,
    /// Interruption (signal ou annulation)
    Interrupted = 3,
}

impl WakeReason {
    fn from_u32(v: u32) -> Self {
        match v {
            1 => Self::Timeout,
            2 => Self::Closed,
            3 => Self::Interrupted,
            _ => Self::Signaled,
        }
    }
}

/// Un waiter dans la file IPC
#[repr(C, align(64))]
pub struct IpcWaiter {
    /// Thread en attente (TaskId opaque)
    pub thread_id: AtomicU32,
    /// Le waiter est actif (inscrit dans la queue)
    pub active: AtomicBool,
    /// Réveillé
    pub woken: AtomicBool,
    /// Raison du réveil
    pub reason: AtomicU32,
    /// Numéro de séquence (anti-spurious)
    pub seq: AtomicU32,
    /// Timestamp d'inscription (ns depuis boot)
    pub enqueued_at: AtomicU64,
    /// Timeout (0 = infini, en nanosecondes depuis enqueued_at)
    pub timeout_ns: AtomicU64,
    _pad: [u8; 16],
}

// SAFETY: tous les champs sont atomiques
unsafe impl Sync for IpcWaiter {}
unsafe impl Send for IpcWaiter {}

impl IpcWaiter {
    pub const fn new() -> Self {
        Self {
            thread_id: AtomicU32::new(0),
            active: AtomicBool::new(false),
            woken: AtomicBool::new(false),
            reason: AtomicU32::new(0),
            seq: AtomicU32::new(0),
            enqueued_at: AtomicU64::new(0),
            timeout_ns: AtomicU64::new(0),
            _pad: [0u8; 16],
        }
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub fn is_woken(&self) -> bool {
        self.woken.load(Ordering::Acquire)
    }

    pub fn wake(&self, reason: WakeReason) {
        self.reason.store(reason as u32, Ordering::Relaxed);
        self.woken.store(true, Ordering::Release);
    }

    pub fn dequeue(&self) {
        self.active.store(false, Ordering::Release);
        self.seq.fetch_add(1, Ordering::Relaxed);
    }

    pub fn wake_reason(&self) -> WakeReason {
        WakeReason::from_u32(self.reason.load(Ordering::Relaxed))
    }
}

// ---------------------------------------------------------------------------
// IpcWaitQueue — file d'attente IPC
// ---------------------------------------------------------------------------

/// File d'attente IPC pour un canal ou une ressource.
///
/// Stocke jusqu'à MAX_IPC_WAITERS threads en attente.
/// Les réveils respectent la politique configurée.
#[repr(C, align(64))]
pub struct IpcWaitQueue {
    /// Identifiant du canal/ressource associé (pour corrélation)
    pub channel_id: ChannelId,
    /// Tableau statique de waiters
    waiters: [IpcWaiter; MAX_IPC_WAITERS],
    /// Nombre de waiters actifs
    count: AtomicU32,
    /// Politique de réveil par défaut
    policy: AtomicU32,
    /// Compteurs de statistiques
    pub total_waits: AtomicU64,
    pub total_woken: AtomicU64,
    pub total_timeouts: AtomicU64,
    _pad: [u8; 16],
}

// SAFETY: IpcWaiter est Sync
unsafe impl Sync for IpcWaitQueue {}
unsafe impl Send for IpcWaitQueue {}

impl IpcWaitQueue {
    pub const fn new(channel_id: ChannelId) -> Self {
        const INIT_WAITER: IpcWaiter = IpcWaiter::new();
        Self {
            channel_id,
            waiters: [INIT_WAITER; MAX_IPC_WAITERS],
            count: AtomicU32::new(0),
            policy: AtomicU32::new(WakePolicy::One as u32),
            total_waits: AtomicU64::new(0),
            total_woken: AtomicU64::new(0),
            total_timeouts: AtomicU64::new(0),
            _pad: [0u8; 16],
        }
    }

    pub fn set_policy(&self, policy: WakePolicy) {
        self.policy.store(policy as u32, Ordering::Relaxed);
    }

    pub fn waiter_count(&self) -> u32 {
        self.count.load(Ordering::Relaxed)
    }

    // -----------------------------------------------------------------------
    // Inscription d'un waiter
    // -----------------------------------------------------------------------

    /// Inscrit le thread `thread_id` comme waiter avec un timeout optionnel.
    /// Retourne l'index du waiter, ou `None` si la queue est pleine.
    pub fn enqueue(
        &self,
        thread_id: u32,
        timeout_ns: u64,
        now_ns: u64,
    ) -> Option<usize> {
        for i in 0..MAX_IPC_WAITERS {
            if !self.waiters[i].is_active() {
                self.waiters[i].thread_id.store(thread_id, Ordering::Relaxed);
                self.waiters[i].timeout_ns.store(timeout_ns, Ordering::Relaxed);
                self.waiters[i].enqueued_at.store(now_ns, Ordering::Relaxed);
                self.waiters[i].woken.store(false, Ordering::Relaxed);
                self.waiters[i].reason.store(0, Ordering::Relaxed);
                self.waiters[i].active.store(true, Ordering::Release);
                self.count.fetch_add(1, Ordering::Relaxed);
                self.total_waits.fetch_add(1, Ordering::Relaxed);
                return Some(i);
            }
        }
        None
    }

    // -----------------------------------------------------------------------
    // Attente
    // -----------------------------------------------------------------------

    /// Attend d'être réveillé (spin-wait).
    ///
    /// Le waiter doit être préalablement inscrit via `enqueue()`.
    ///
    /// # Retour
    /// - `Ok(WakeReason)` — cause du réveil
    /// - `Err(IpcError::Timeout)` — timeout expiré
    pub fn wait_for_wake(
        &self,
        waiter_idx: usize,
        check_closed: &dyn Fn() -> bool,
    ) -> Result<WakeReason, IpcError> {
        if waiter_idx >= MAX_IPC_WAITERS {
            return Err(IpcError::InvalidHandle);
        }

        let w = &self.waiters[waiter_idx];
        let timeout_ns = w.timeout_ns.load(Ordering::Relaxed);
        let enqueued_at = w.enqueued_at.load(Ordering::Relaxed);
        let mut spins: u64 = 0;
        let spin_timeout = if timeout_ns == 0 { u64::MAX } else { timeout_ns / 10 };

        loop {
            core::hint::spin_loop();
            spins += 1;

            if w.is_woken() {
                let reason = w.wake_reason();
                w.dequeue();
                self.count.fetch_sub(1, Ordering::Relaxed);

                match reason {
                    WakeReason::Signaled => {
                        self.total_woken.fetch_add(1, Ordering::Relaxed);
                        return Ok(WakeReason::Signaled);
                    }
                    WakeReason::Closed => {
                        return Err(IpcError::Closed);
                    }
                    WakeReason::Timeout => {
                        self.total_timeouts.fetch_add(1, Ordering::Relaxed);
                        return Err(IpcError::Timeout);
                    }
                    WakeReason::Interrupted => {
                        return Err(IpcError::Closed);
                    }
                }
            }

            if check_closed() {
                w.wake(WakeReason::Closed);
                w.dequeue();
                self.count.fetch_sub(1, Ordering::Relaxed);
                return Err(IpcError::Closed);
            }

            if timeout_ns > 0 && spins >= spin_timeout {
                w.wake(WakeReason::Timeout);
                w.dequeue();
                self.count.fetch_sub(1, Ordering::Relaxed);
                self.total_timeouts.fetch_add(1, Ordering::Relaxed);
                return Err(IpcError::Timeout);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Réveil
    // -----------------------------------------------------------------------

    /// Réveille les waiters selon la politique configurée.
    /// `reason` : raison fournie aux waiters.
    /// Retourne le nombre de waiters réveillés.
    pub fn wake(&self, reason: WakeReason) -> u32 {
        let policy_raw = self.policy.load(Ordering::Relaxed);
        let policy = match policy_raw {
            1 => WakePolicy::All,
            2 => WakePolicy::UpToN,
            _ => WakePolicy::One,
        };
        self.wake_with_policy(reason, policy, u32::MAX)
    }

    /// Réveille avec politique et limite N explicites.
    pub fn wake_with_policy(&self, reason: WakeReason, policy: WakePolicy, max_n: u32) -> u32 {
        let mut woken = 0u32;
        for i in 0..MAX_IPC_WAITERS {
            match policy {
                WakePolicy::One if woken >= 1 => break,
                WakePolicy::UpToN if woken >= max_n => break,
                _ => {}
            }
            let w = &self.waiters[i];
            if w.is_active() && !w.is_woken() {
                w.wake(reason);
                woken += 1;
            }
        }
        self.total_woken.fetch_add(woken as u64, Ordering::Relaxed);
        woken
    }

    /// Réveille un waiter spécifique par son thread_id.
    pub fn wake_thread(&self, thread_id: u32, reason: WakeReason) -> bool {
        for i in 0..MAX_IPC_WAITERS {
            let w = &self.waiters[i];
            if w.is_active()
                && w.thread_id.load(Ordering::Relaxed) == thread_id
                && !w.is_woken()
            {
                w.wake(reason);
                self.total_woken.fetch_add(1, Ordering::Relaxed);
                return true;
            }
        }
        false
    }

    /// Réveille tous les waiters avec `Closed` (ex : canal fermé).
    pub fn close_all(&self) {
        self.wake_with_policy(WakeReason::Closed, WakePolicy::All, u32::MAX);
    }
}

// ---------------------------------------------------------------------------
// Snapshot de statistiques
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct IpcWaitQueueStats {
    pub total_waits: u64,
    pub total_woken: u64,
    pub total_timeouts: u64,
    pub current_waiters: u32,
}

impl IpcWaitQueue {
    pub fn snapshot_stats(&self) -> IpcWaitQueueStats {
        IpcWaitQueueStats {
            total_waits: self.total_waits.load(Ordering::Relaxed),
            total_woken: self.total_woken.load(Ordering::Relaxed),
            total_timeouts: self.total_timeouts.load(Ordering::Relaxed),
            current_waiters: self.waiter_count(),
        }
    }
}
