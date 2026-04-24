// ipc/channel/async.rs — Canal asynchrone (notifications sans runtime async) pour Exo-OS
//
// Ce module implémente un canal asynchrone sans runtime async/await complet.
// Les threads peuvent enregistrer des callbacks de réveil (Waker-like) qui seront
// appelés quand un message est disponible.
//
// Architecture :
//   - File d'attente de messages (SpscRing ou MpmcRing selon le mode)
//   - Table de wakers statique (MAX_ASYNC_WAKERS = 32)
//   - Notification via AtomicBool "ready" par waker enregistré
//   - Hook de réveil vers le scheduler (appel optionnel si configuré)
//   - Pas de Future/Poll/Waker std — noyau no_std
//
// Patterns d'utilisation :
//   1. Polling actif : `try_recv()` + spin courte durée
//   2. Event-driven : enregistrer un waker → être notifié → appeler `try_recv()`
//   3. Completion callback : enregistrer un `fn(*mut ()) -> ()` appelé à réception

use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::ipc::core::constants::MAX_MSG_SIZE;
use crate::ipc::core::types::{
    alloc_channel_id, alloc_message_id, ChannelId, IpcError, MessageId, MsgFlags,
};
use crate::ipc::ring::spsc::SpscRing;
use crate::ipc::stats::counters::{StatEvent, IPC_STATS};
use crate::scheduler::sync::spinlock::SpinLock;

// ---------------------------------------------------------------------------
// Waker noyau simplifié
// ---------------------------------------------------------------------------

/// Nombre maximal de wakers enregistrés par canal asynchrone
pub const MAX_ASYNC_WAKERS: usize = 32;

/// Type de pointeur de fonction de réveil.
/// `data` = pointeur opaque passé à la registration.
pub type WakeFn = unsafe fn(data: *mut ());

/// Waker enregistré sur un canal asynchrone
#[repr(C, align(32))]
pub struct AsyncWaker {
    /// Fonction de réveil (None = waker libre)
    wake_fn: Option<WakeFn>,
    /// Donnée opaque transmise au wake_fn
    data: *mut (),
    /// Ce waker a été déclenché mais pas encore traité
    fired: AtomicBool,
    /// Waker actif (enregistré)
    active: AtomicBool,
    _pad: [u8; 6],
}

// SAFETY: wake_fn et data sont définis par le registrant qui garantit leur validité
// pendant toute la durée de registration. AtomicBool est Sync.
unsafe impl Sync for AsyncWaker {}
unsafe impl Send for AsyncWaker {}

impl AsyncWaker {
    pub const fn new() -> Self {
        Self {
            wake_fn: None,
            data: core::ptr::null_mut(),
            fired: AtomicBool::new(false),
            active: AtomicBool::new(false),
            _pad: [0u8; 6],
        }
    }

    pub fn register(&mut self, f: WakeFn, data: *mut ()) {
        self.wake_fn = Some(f);
        self.data = data;
        self.fired.store(false, Ordering::Relaxed);
        self.active.store(true, Ordering::Release);
    }

    pub fn deactivate(&mut self) {
        self.active.store(false, Ordering::Release);
        self.wake_fn = None;
        self.data = core::ptr::null_mut();
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub fn has_fired(&self) -> bool {
        self.fired.load(Ordering::Acquire)
    }

    /// Déclenche le waker : appelle wake_fn(data) si actif.
    ///
    /// # SAFETY
    /// Le registrant garantit que `data` reste valide jusqu'à déregistration.
    pub unsafe fn fire(&self) {
        if self.active.load(Ordering::Acquire) {
            if let Some(f) = self.wake_fn {
                self.fired.store(true, Ordering::Release);
                // SAFETY: garantie par le contrat du registrant
                f(self.data);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Table de wakers
// ---------------------------------------------------------------------------

pub struct WakerTable {
    entries: [AsyncWaker; MAX_ASYNC_WAKERS],
    count: usize,
}

// SAFETY: accès protégé par SpinLock
unsafe impl Send for WakerTable {}

impl WakerTable {
    pub const fn new() -> Self {
        const INIT_WAKER: AsyncWaker = AsyncWaker::new();
        Self {
            entries: [INIT_WAKER; MAX_ASYNC_WAKERS],
            count: 0,
        }
    }

    /// Enregistre un waker. Retourne l'index ou `None` si plein.
    pub fn register(&mut self, f: WakeFn, data: *mut ()) -> Option<usize> {
        for i in 0..MAX_ASYNC_WAKERS {
            if !self.entries[i].is_active() {
                self.entries[i].register(f, data);
                self.count += 1;
                return Some(i);
            }
        }
        None
    }

    /// Déregistre le waker à l'index `idx`.
    pub fn unregister(&mut self, idx: usize) -> bool {
        if idx < MAX_ASYNC_WAKERS && self.entries[idx].is_active() {
            self.entries[idx].deactivate();
            self.count -= 1;
            true
        } else {
            false
        }
    }

    /// Déclenche tous les wakers actifs.
    pub fn fire_all(&self) {
        for i in 0..MAX_ASYNC_WAKERS {
            if self.entries[i].is_active() {
                // SAFETY: contrat du registrant (data valide jusqu'à déregistration)
                unsafe { self.entries[i].fire() };
            }
        }
    }

    pub fn count(&self) -> usize {
        self.count
    }
}

// ---------------------------------------------------------------------------
// Statistiques du canal asynchrone
// ---------------------------------------------------------------------------

#[repr(C, align(64))]
pub struct AsyncChannelStats {
    pub sends_ok: AtomicU64,
    pub sends_full: AtomicU64,
    pub recvs_ok: AtomicU64,
    pub recvs_empty: AtomicU64,
    pub waker_fires: AtomicU64,
    pub bytes_transferred: AtomicU64,
    _pad: [u8; 16],
}

impl AsyncChannelStats {
    pub const fn new() -> Self {
        Self {
            sends_ok: AtomicU64::new(0),
            sends_full: AtomicU64::new(0),
            recvs_ok: AtomicU64::new(0),
            recvs_empty: AtomicU64::new(0),
            waker_fires: AtomicU64::new(0),
            bytes_transferred: AtomicU64::new(0),
            _pad: [0u8; 16],
        }
    }

    pub fn snapshot(&self) -> AsyncChannelStatsSnapshot {
        AsyncChannelStatsSnapshot {
            sends_ok: self.sends_ok.load(Ordering::Relaxed),
            sends_full: self.sends_full.load(Ordering::Relaxed),
            recvs_ok: self.recvs_ok.load(Ordering::Relaxed),
            recvs_empty: self.recvs_empty.load(Ordering::Relaxed),
            waker_fires: self.waker_fires.load(Ordering::Relaxed),
            bytes_transferred: self.bytes_transferred.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AsyncChannelStatsSnapshot {
    pub sends_ok: u64,
    pub sends_full: u64,
    pub recvs_ok: u64,
    pub recvs_empty: u64,
    pub waker_fires: u64,
    pub bytes_transferred: u64,
}

// ---------------------------------------------------------------------------
// AsyncChannel — structure principale
// ---------------------------------------------------------------------------

/// Canal asynchrone avec notification par waker.
///
/// Mécanisme :
///   - `send()` → push dans SpscRing → notifie les wakers enregistrés
///   - `try_recv()` → pop depuis SpscRing (non-bloquant)
///   - `register_waker()` → sera appelé à chaque send()
///   - `pending_count()` → nombre approximatif de messages en attente
#[repr(C, align(64))]
pub struct AsyncChannel {
    pub id: ChannelId,
    /// Ring de messages
    ring: SpscRing,
    /// Table de wakers (protégée par SpinLock)
    wakers: SpinLock<WakerTable>,
    /// Statistiques locales
    pub stats: AsyncChannelStats,
    /// Nombre de messages en attente (approximatif)
    pending: AtomicUsize,
    /// Canal fermé
    closed: AtomicU32,
    _pad: [u8; 20],
}

// SAFETY: SpscRing est Sync, SpinLock<WakerTable> est Sync
unsafe impl Sync for AsyncChannel {}
unsafe impl Send for AsyncChannel {}

impl AsyncChannel {
    pub fn new() -> Self {
        let s = Self {
            id: alloc_channel_id(),
            ring: SpscRing::new(),
            wakers: SpinLock::new(WakerTable::new()),
            stats: AsyncChannelStats::new(),
            pending: AtomicUsize::new(0),
            closed: AtomicU32::new(0),
            _pad: [0u8; 20],
        };
        s.ring.init();
        s
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire) != 0
    }

    pub fn close(&self) {
        self.closed.store(1, Ordering::Release);
        // Notifier les wakers que le canal est fermé
        let tbl = self.wakers.lock();
        tbl.fire_all();
    }

    // -----------------------------------------------------------------------
    // Gestion des wakers
    // -----------------------------------------------------------------------

    /// Enregistre un waker qui sera appelé à chaque message disponible.
    /// Retourne l'index du waker pour pouvoir le déregistrer.
    pub fn register_waker(&self, f: WakeFn, data: *mut ()) -> Option<usize> {
        let mut tbl = self.wakers.lock();
        tbl.register(f, data)
    }

    /// Désenregistre le waker à l'index `waker_idx`.
    pub fn unregister_waker(&self, waker_idx: usize) -> bool {
        let mut tbl = self.wakers.lock();
        tbl.unregister(waker_idx)
    }

    /// Retourne le nombre de wakers actifs.
    pub fn waker_count(&self) -> usize {
        self.wakers.lock().count()
    }

    // -----------------------------------------------------------------------
    // Envoi
    // -----------------------------------------------------------------------

    /// Envoie `data` dans le canal asynchrone et notifie les wakers.
    pub fn send(&self, data: &[u8], flags: MsgFlags) -> Result<MessageId, IpcError> {
        if self.is_closed() {
            return Err(IpcError::Closed);
        }
        if data.len() > MAX_MSG_SIZE {
            return Err(IpcError::MessageTooLarge);
        }

        let mid = alloc_message_id();

        match self.ring.push_copy(data, flags) {
            Ok(_) => {
                self.stats.sends_ok.fetch_add(1, Ordering::Relaxed);
                self.stats
                    .bytes_transferred
                    .fetch_add(data.len() as u64, Ordering::Relaxed);
                self.pending.fetch_add(1, Ordering::Relaxed);
                IPC_STATS.record(StatEvent::MessageSent);

                // Notifier les wakers enregistrés
                let tbl = self.wakers.lock();
                if tbl.count() > 0 {
                    tbl.fire_all();
                    self.stats
                        .waker_fires
                        .fetch_add(tbl.count() as u64, Ordering::Relaxed);
                }

                Ok(mid)
            }
            Err(IpcError::QueueFull) => {
                self.stats.sends_full.fetch_add(1, Ordering::Relaxed);
                Err(IpcError::QueueFull)
            }
            Err(e) => Err(e),
        }
    }

    // -----------------------------------------------------------------------
    // Réception
    // -----------------------------------------------------------------------

    /// Tente de recevoir un message sans blocage.
    /// Retourne `IpcError::WouldBlock` si aucun message disponible.
    pub fn try_recv(&self, buf: &mut [u8]) -> Result<(usize, MsgFlags), IpcError> {
        if self.is_closed() && self.ring.is_empty() {
            return Err(IpcError::Closed);
        }

        match self.ring.pop_into(buf) {
            Ok((len, flags)) => {
                self.stats.recvs_ok.fetch_add(1, Ordering::Relaxed);
                self.pending.fetch_sub(1, Ordering::Relaxed);
                IPC_STATS.record(StatEvent::MessageReceived);
                Ok((len, flags))
            }
            Err(IpcError::QueueEmpty) => {
                self.stats.recvs_empty.fetch_add(1, Ordering::Relaxed);
                Err(IpcError::WouldBlock)
            }
            Err(e) => Err(e),
        }
    }

    /// Réception bloquante (spin-wait fini).
    pub fn recv(&self, buf: &mut [u8], timeout_spins: u32) -> Result<(usize, MsgFlags), IpcError> {
        let mut spins = 0u32;
        loop {
            match self.try_recv(buf) {
                Ok(r) => return Ok(r),
                Err(IpcError::WouldBlock) => {
                    core::hint::spin_loop();
                    spins += 1;
                    if self.is_closed() {
                        return Err(IpcError::Closed);
                    }
                    if timeout_spins > 0 && spins >= timeout_spins {
                        return Err(IpcError::Timeout);
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    // -----------------------------------------------------------------------
    // Utilitaires
    // -----------------------------------------------------------------------

    pub fn pending_count(&self) -> usize {
        self.pending.load(Ordering::Relaxed)
    }

    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }

    pub fn snapshot_stats(&self) -> AsyncChannelStatsSnapshot {
        self.stats.snapshot()
    }
}

// ---------------------------------------------------------------------------
// Table statique globale de canaux asynchrones
// ---------------------------------------------------------------------------

pub const ASYNC_CHANNEL_TABLE_SIZE: usize = 256;

struct AsyncChannelTable {
    slots: [MaybeUninit<AsyncChannel>; ASYNC_CHANNEL_TABLE_SIZE],
    used: [bool; ASYNC_CHANNEL_TABLE_SIZE],
    count: usize,
}

// SAFETY: accès protégé par SpinLock
unsafe impl Send for AsyncChannelTable {}

impl AsyncChannelTable {
    #[allow(dead_code)]
    const fn new() -> Self {
        // SAFETY: MaybeUninit zeros valides + AtomicBool false = table vide; jamais lu avant init.
        unsafe { core::mem::zeroed() }
    }

    fn alloc(&mut self) -> Option<usize> {
        for i in 0..ASYNC_CHANNEL_TABLE_SIZE {
            if !self.used[i] {
                self.slots[i].write(AsyncChannel::new());
                self.used[i] = true;
                self.count += 1;
                return Some(i);
            }
        }
        None
    }

    fn free(&mut self, idx: usize) -> bool {
        if idx < ASYNC_CHANNEL_TABLE_SIZE && self.used[idx] {
            // SAFETY: used[idx] garantit que slots[idx] est initialisé; used → false empêche double-drop.
            unsafe { self.slots[idx].assume_init_drop() };
            self.used[idx] = false;
            self.count -= 1;
            true
        } else {
            false
        }
    }

    unsafe fn get(&self, idx: usize) -> Option<&AsyncChannel> {
        if idx < ASYNC_CHANNEL_TABLE_SIZE && self.used[idx] {
            Some(self.slots[idx].assume_init_ref())
        } else {
            None
        }
    }
}

static ASYNC_CHANNEL_TABLE: SpinLock<AsyncChannelTable> =
    // SAFETY: SpinLock<AsyncChannelTable> tout-zéro valide: AtomicBool false = déverrouillé, table vide.
    unsafe { core::mem::zeroed() };

// ---------------------------------------------------------------------------
// API publique de haut niveau
// ---------------------------------------------------------------------------

/// Crée un canal asynchrone. Retourne son index.
pub fn async_channel_create() -> Result<usize, IpcError> {
    let mut tbl = ASYNC_CHANNEL_TABLE.lock();
    tbl.alloc().ok_or(IpcError::OutOfResources)
}

/// Enregistre un waker sur le canal `idx`.
pub fn async_channel_register_waker(
    idx: usize,
    f: WakeFn,
    data: *mut (),
) -> Result<usize, IpcError> {
    let tbl = ASYNC_CHANNEL_TABLE.lock();
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static AsyncChannel = unsafe { &*(chan as *const AsyncChannel) };
    drop(tbl);
    chan_ref
        .register_waker(f, data)
        .ok_or(IpcError::OutOfResources)
}

/// Désenregistre un waker.
pub fn async_channel_unregister_waker(chan_idx: usize, waker_idx: usize) -> Result<(), IpcError> {
    let tbl = ASYNC_CHANNEL_TABLE.lock();
    let chan = unsafe { tbl.get(chan_idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static AsyncChannel = unsafe { &*(chan as *const AsyncChannel) };
    drop(tbl);
    if chan_ref.unregister_waker(waker_idx) {
        Ok(())
    } else {
        Err(IpcError::InvalidHandle)
    }
}

/// Envoie `data` sur le canal asynchrone `idx`.
pub fn async_channel_send(idx: usize, data: &[u8], flags: MsgFlags) -> Result<MessageId, IpcError> {
    let tbl = ASYNC_CHANNEL_TABLE.lock();
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static AsyncChannel = unsafe { &*(chan as *const AsyncChannel) };
    drop(tbl);
    chan_ref.send(data, flags)
}

/// Reçoit un message sans blocage du canal `idx`.
pub fn async_channel_try_recv(idx: usize, buf: &mut [u8]) -> Result<(usize, MsgFlags), IpcError> {
    let tbl = ASYNC_CHANNEL_TABLE.lock();
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static AsyncChannel = unsafe { &*(chan as *const AsyncChannel) };
    drop(tbl);
    chan_ref.try_recv(buf)
}

/// Ferme et détruit le canal asynchrone `idx`.
pub fn async_channel_destroy(idx: usize) -> Result<(), IpcError> {
    let tbl = ASYNC_CHANNEL_TABLE.lock();
    if let Some(chan) = unsafe { tbl.get(idx) } {
        chan.close();
    }
    drop(tbl);
    let mut tbl = ASYNC_CHANNEL_TABLE.lock();
    if !tbl.free(idx) {
        return Err(IpcError::InvalidHandle);
    }
    Ok(())
}

/// Nombre de canaux asynchrones actifs.
pub fn async_channel_count() -> usize {
    ASYNC_CHANNEL_TABLE.lock().count
}
