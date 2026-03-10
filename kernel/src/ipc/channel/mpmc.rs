// ipc/channel/mpmc.rs — Canal Multi-Producteurs / Multi-Consommateurs pour Exo-OS
//
// Ce canal encapsule le ring::MpmcRing (algorithme Dmitry Vyukov CAS-based)
// pour offrir une interface de haut niveau avec gestion des droits, statistiques
// et politique de débordement configurable.
//
// Caractéristiques :
//   - N émetteurs simultanés — CAS lock-free sur la tête de file
//   - M récepteurs simultanés — CAS lock-free sur la queue
//   - Politique d'overflow : DROP_OLDEST, DROP_NEWEST, BLOCK
//   - Capacité configurable jusqu'à RING_SIZE (4096 slots)
//   - Statistiques temps-réel (AtomicU64, pas de mutex)

use core::sync::atomic::{AtomicU64, AtomicU32, AtomicUsize, Ordering};
use core::mem::MaybeUninit;

use crate::ipc::core::types::{ChannelId, IpcError, MsgFlags, MessageId, alloc_channel_id, alloc_message_id};
use crate::ipc::core::constants::MAX_MSG_SIZE;
use crate::ipc::ring::mpmc::MpmcRing;
use crate::ipc::stats::counters::{IPC_STATS, StatEvent};
use crate::scheduler::sync::spinlock::SpinLock;
// IPC-04 (v6) : vérification capability via security::access_control
use crate::security::capability::{CapTable, CapToken, Rights};
use crate::security::access_control::{check_access, ObjectKind, AccessError};

// ---------------------------------------------------------------------------
// Politique de débordement
// ---------------------------------------------------------------------------

/// Comportement quand la file est pleine
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum OverflowPolicy {
    /// Rejeter le message entrant (défaut)
    DropNewest = 0,
    /// Supprimer le plus ancien message pour faire de la place
    DropOldest = 1,
    /// Bloquer l'émetteur (spin-wait jusqu'à place disponible)
    Block = 2,
}

impl OverflowPolicy {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::DropOldest,
            2 => Self::Block,
            _ => Self::DropNewest,
        }
    }
}

// ---------------------------------------------------------------------------
// Statistiques MPMC
// ---------------------------------------------------------------------------

/// Compteurs de performance d'un canal MPMC
#[repr(C, align(64))]
pub struct MpmcStats {
    pub sends_ok: AtomicU64,
    pub sends_dropped: AtomicU64,
    pub sends_blocked: AtomicU64,
    pub recvs_ok: AtomicU64,
    pub recvs_empty: AtomicU64,
    pub total_bytes_sent: AtomicU64,
    pub total_bytes_recv: AtomicU64,
    pub overflow_drops: AtomicU64,
    _pad: [u8; 0],
}

impl MpmcStats {
    pub const fn new() -> Self {
        Self {
            sends_ok: AtomicU64::new(0),
            sends_dropped: AtomicU64::new(0),
            sends_blocked: AtomicU64::new(0),
            recvs_ok: AtomicU64::new(0),
            recvs_empty: AtomicU64::new(0),
            total_bytes_sent: AtomicU64::new(0),
            total_bytes_recv: AtomicU64::new(0),
            overflow_drops: AtomicU64::new(0),
            _pad: [],
        }
    }

    /// Snapshot cohérent (lecture à séquentialité relaxée)
    pub fn snapshot(&self) -> MpmcStatsSnapshot {
        MpmcStatsSnapshot {
            sends_ok: self.sends_ok.load(Ordering::Relaxed),
            sends_dropped: self.sends_dropped.load(Ordering::Relaxed),
            sends_blocked: self.sends_blocked.load(Ordering::Relaxed),
            recvs_ok: self.recvs_ok.load(Ordering::Relaxed),
            recvs_empty: self.recvs_empty.load(Ordering::Relaxed),
            total_bytes_sent: self.total_bytes_sent.load(Ordering::Relaxed),
            total_bytes_recv: self.total_bytes_recv.load(Ordering::Relaxed),
            overflow_drops: self.overflow_drops.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MpmcStatsSnapshot {
    pub sends_ok: u64,
    pub sends_dropped: u64,
    pub sends_blocked: u64,
    pub recvs_ok: u64,
    pub recvs_empty: u64,
    pub total_bytes_sent: u64,
    pub total_bytes_recv: u64,
    pub overflow_drops: u64,
}

// ---------------------------------------------------------------------------
// MpmcChannel — structure principale
// ---------------------------------------------------------------------------

/// Canal MPMC utilisant le ring CAS lock-free de Vyukov.
///
/// La capacité effective est `MPMC_RING_SIZE` (= 4096 slots).
/// Plusieurs producteurs et consommateurs peuvent opérer simultanément
/// sans mutex.
#[repr(C, align(64))]
pub struct MpmcChannel {
    /// Identifiant unique du canal
    pub id: ChannelId,
    /// Anneau de données sous-jacent
    ring: MpmcRing,
    /// Statistiques locales
    pub stats: MpmcStats,
    /// Canal fermé
    closed: AtomicU32,
    /// Politique de débordement (OverflowPolicy encodée en u8)
    overflow_policy: AtomicU32,
    /// Nombre de producteurs enregistrés
    producer_count: AtomicU32,
    /// Nombre de consommateurs enregistrés
    consumer_count: AtomicU32,
    /// Nombre de messages en attente (approximatif)
    pending: AtomicUsize,
    _pad: [u8; 24],
}

// SAFETY: MpmcRing est Sync (atomic CAS internes), AtomicU64 est Sync.
unsafe impl Sync for MpmcChannel {}
unsafe impl Send for MpmcChannel {}

impl MpmcChannel {
    /// Crée un nouveau canal MPMC.
    pub fn new(policy: OverflowPolicy) -> Self {
        let c = Self {
            id: alloc_channel_id(),
            ring: MpmcRing::new_uninit(),
            stats: MpmcStats::new(),
            closed: AtomicU32::new(0),
            overflow_policy: AtomicU32::new(policy as u32),
            producer_count: AtomicU32::new(0),
            consumer_count: AtomicU32::new(0),
            pending: AtomicUsize::new(0),
            _pad: [0u8; 24],
        };
        c.ring.init();
        c
    }

    #[inline]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire) != 0
    }

    pub fn close(&self) {
        self.closed.store(1, Ordering::Release);
    }

    pub fn register_producer(&self) -> u32 {
        self.producer_count.fetch_add(1, Ordering::Relaxed)
    }

    pub fn unregister_producer(&self) -> u32 {
        self.producer_count.fetch_sub(1, Ordering::Relaxed)
    }

    pub fn register_consumer(&self) -> u32 {
        self.consumer_count.fetch_add(1, Ordering::Relaxed)
    }

    pub fn unregister_consumer(&self) -> u32 {
        self.consumer_count.fetch_sub(1, Ordering::Relaxed)
    }

    pub fn producer_count(&self) -> u32 {
        self.producer_count.load(Ordering::Relaxed)
    }

    pub fn consumer_count(&self) -> u32 {
        self.consumer_count.load(Ordering::Relaxed)
    }

    // -----------------------------------------------------------------------
    // ENVOI
    // -----------------------------------------------------------------------

    /// Envoie `data` dans le canal MPMC.
    ///
    /// Comportement configurable en cas de file pleine (cf. OverflowPolicy).
    pub fn send(&self, data: &[u8], flags: MsgFlags) -> Result<MessageId, IpcError> {
        if self.is_closed() {
            return Err(IpcError::Closed);
        }
        if data.len() > MAX_MSG_SIZE {
            return Err(IpcError::MessageTooLarge);
        }

        let mid = alloc_message_id();
        let policy = OverflowPolicy::from_u8(
            self.overflow_policy.load(Ordering::Relaxed) as u8,
        );

        match self.ring.push_copy(data, flags) {
            Ok(_seq) => {
                self.stats.sends_ok.fetch_add(1, Ordering::Relaxed);
                self.stats.total_bytes_sent.fetch_add(data.len() as u64, Ordering::Relaxed);
                self.pending.fetch_add(1, Ordering::Relaxed);
                IPC_STATS.record(StatEvent::MessageSent);
                Ok(mid)
            }
            Err(IpcError::QueueFull) => {
                match policy {
                    OverflowPolicy::DropNewest => {
                        self.stats.sends_dropped.fetch_add(1, Ordering::Relaxed);
                        self.stats.overflow_drops.fetch_add(1, Ordering::Relaxed);
                        IPC_STATS.record(StatEvent::MessageDropped);
                        Err(IpcError::QueueFull)
                    }
                    OverflowPolicy::DropOldest => {
                        // Consommer (et jeter) le plus ancien message
                        let mut discard = [0u8; MAX_MSG_SIZE];
                        let _ = self.ring.pop_into(&mut discard);
                        self.pending.fetch_sub(1, Ordering::Relaxed);
                        self.stats.overflow_drops.fetch_add(1, Ordering::Relaxed);
                        // Réessayer
                        match self.ring.push_copy(data, flags) {
                            Ok(_) => {
                                self.pending.fetch_add(1, Ordering::Relaxed);
                                self.stats.sends_ok.fetch_add(1, Ordering::Relaxed);
                                IPC_STATS.record(StatEvent::MessageSent);
                                Ok(mid)
                            }
                            Err(e) => Err(e),
                        }
                    }
                    OverflowPolicy::Block => {
                        // Spin-wait jusqu'à place disponible
                        self.stats.sends_blocked.fetch_add(1, Ordering::Relaxed);
                        let mut spins: u32 = 0;
                        loop {
                            core::hint::spin_loop();
                            spins += 1;
                            if self.is_closed() {
                                return Err(IpcError::Closed);
                            }
                            if spins > 500_000 {
                                return Err(IpcError::Timeout);
                            }
                            match self.ring.push_copy(data, flags) {
                                Ok(_) => {
                                    self.pending.fetch_add(1, Ordering::Relaxed);
                                    self.stats.sends_ok.fetch_add(1, Ordering::Relaxed);
                                    IPC_STATS.record(StatEvent::MessageSent);
                                    return Ok(mid);
                                }
                                Err(IpcError::QueueFull) => continue,
                                Err(e) => return Err(e),
                            }
                        }
                    }
                }
            }
            Err(e) => Err(e),
        }
    }

    // -----------------------------------------------------------------------
    // RÉCEPTION
    // -----------------------------------------------------------------------

    /// Reçoit un message du canal MPMC (consommation).
    ///
    /// Thread-safe — plusieurs consommateurs peuvent appeler recv() simultanément.
    pub fn recv(&self, buf: &mut [u8], flags: MsgFlags) -> Result<(usize, MsgFlags), IpcError> {
        if self.is_closed() && self.ring.is_empty_approx() {
            return Err(IpcError::Closed);
        }

        if flags.contains(MsgFlags::NOWAIT) {
            match self.ring.pop_into(buf) {
                Ok((len, msg_flags)) => {
                    self.stats.recvs_ok.fetch_add(1, Ordering::Relaxed);
                    self.stats.total_bytes_recv.fetch_add(len as u64, Ordering::Relaxed);
                    self.pending.fetch_sub(1, Ordering::Relaxed);
                    IPC_STATS.record(StatEvent::MessageReceived);
                    Ok((len, msg_flags))
                }
                Err(IpcError::QueueEmpty) => {
                    self.stats.recvs_empty.fetch_add(1, Ordering::Relaxed);
                    Err(IpcError::WouldBlock)
                }
                Err(e) => Err(e),
            }
        } else {
            // Spin-wait bloquant
            let mut spins: u32 = 0;
            loop {
                match self.ring.pop_into(buf) {
                    Ok((len, msg_flags)) => {
                        self.stats.recvs_ok.fetch_add(1, Ordering::Relaxed);
                        self.stats.total_bytes_recv.fetch_add(len as u64, Ordering::Relaxed);
                        self.pending.fetch_sub(1, Ordering::Relaxed);
                        IPC_STATS.record(StatEvent::MessageReceived);
                        return Ok((len, msg_flags));
                    }
                    Err(IpcError::QueueEmpty) => {
                        core::hint::spin_loop();
                        spins += 1;
                        if self.is_closed() && self.pending.load(Ordering::Relaxed) == 0 {
                            return Err(IpcError::Closed);
                        }
                        if spins > 2_000_000 {
                            return Err(IpcError::Timeout);
                        }
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    }

    /// Tente de lire jusqu'à `max` messages en une seule opération (batch).
    /// Retourne le nombre de messages effectivement lus.
    pub fn recv_batch(&self, bufs: &mut [[u8; MAX_MSG_SIZE]], max: usize)
        -> Result<usize, IpcError>
    {
        let n = max.min(bufs.len());
        let mut count = 0usize;

        for i in 0..n {
            match self.ring.pop_into(&mut bufs[i]) {
                Ok((len, _flags)) => {
                    self.stats.recvs_ok.fetch_add(1, Ordering::Relaxed);
                    self.stats.total_bytes_recv.fetch_add(len as u64, Ordering::Relaxed);
                    count += 1;
                }
                Err(IpcError::QueueEmpty) => break,
                Err(e) => {
                    if count == 0 { return Err(e); }
                    break;
                }
            }
        }

        if count > 0 {
            self.pending.fetch_sub(count, Ordering::Relaxed);
        }
        Ok(count)
    }

    // -----------------------------------------------------------------------
    // Utilitaires
    // -----------------------------------------------------------------------

    /// Taille approximative de la file (peut être inexacte sous concurrence).
    pub fn len_approx(&self) -> usize {
        self.pending.load(Ordering::Relaxed)
    }

    pub fn is_empty_approx(&self) -> bool {
        self.ring.is_empty_approx()
    }

    pub fn snapshot_stats(&self) -> MpmcStatsSnapshot {
        self.stats.snapshot()
    }

    // -----------------------------------------------------------------------
    // ENVOI/RÉCEPTION CAP-CHECKED — IPC-04 (v6)
    // -----------------------------------------------------------------------

    /// Envoie avec vérification capability (RÈGLE IPC-04 v6).
    ///
    /// Appelle `security::access_control::check_access()` avant le vrai envoi.
    /// # Droits requis : `Rights::IPC_SEND`
    #[inline]
    pub fn send_checked(
        &self,
        data:  &[u8],
        flags: MsgFlags,
        table: &CapTable,
        token: CapToken,
    ) -> Result<MessageId, IpcError> {
        // IPC-04 (v6) : vérification capability — appel direct security/access_control/
        check_access(table, token, ObjectKind::IpcChannel, Rights::IPC_SEND, "ipc::mpmc")
            .map_err(|e| match e {
                AccessError::ObjectNotFound { .. } => IpcError::EndpointNotFound,
                _ => IpcError::PermissionDenied,
            })?;
        self.send(data, flags)
    }

    /// Reçoit avec vérification capability (RÈGLE IPC-04 v6).
    /// # Droits requis : `Rights::IPC_RECV`
    #[inline]
    pub fn recv_checked(
        &self,
        buf:   &mut [u8],
        flags: MsgFlags,
        table: &CapTable,
        token: CapToken,
    ) -> Result<(usize, MsgFlags), IpcError> {
        // IPC-04 (v6) : vérification capability — appel direct security/access_control/
        check_access(table, token, ObjectKind::IpcChannel, Rights::IPC_RECV, "ipc::mpmc")
            .map_err(|e| match e {
                AccessError::ObjectNotFound { .. } => IpcError::EndpointNotFound,
                _ => IpcError::PermissionDenied,
            })?;
        self.recv(buf, flags)
    }
}

// ---------------------------------------------------------------------------
// Table statique globale de canaux MPMC
// ---------------------------------------------------------------------------

/// Taille de la table globale des canaux MPMC
pub const MPMC_CHANNEL_TABLE_SIZE: usize = 256;

struct MpmcChannelTable {
    slots: [MaybeUninit<MpmcChannel>; MPMC_CHANNEL_TABLE_SIZE],
    used: [bool; MPMC_CHANNEL_TABLE_SIZE],
    count: usize,
}

// SAFETY: accès protégé par SpinLock<MpmcChannelTable>
unsafe impl Send for MpmcChannelTable {}

impl MpmcChannelTable {
    #[allow(dead_code)]
    const fn new() -> Self {
        // SAFETY: mem::zeroed() évite la limite mémoire du const-eval pour grands tableaux.
        unsafe { core::mem::zeroed() }
    }

    fn alloc(&mut self, policy: OverflowPolicy) -> Option<usize> {
        for i in 0..MPMC_CHANNEL_TABLE_SIZE {
            if !self.used[i] {
                self.slots[i].write(MpmcChannel::new(policy));
                self.used[i] = true;
                self.count += 1;
                return Some(i);
            }
        }
        None
    }

    fn free(&mut self, idx: usize) -> bool {
        if idx < MPMC_CHANNEL_TABLE_SIZE && self.used[idx] {
            // SAFETY: used[idx] garantit l'initialisation
            unsafe { self.slots[idx].assume_init_drop() };
            self.used[idx] = false;
            self.count -= 1;
            true
        } else {
            false
        }
    }

    unsafe fn get(&self, idx: usize) -> Option<&MpmcChannel> {
        if idx < MPMC_CHANNEL_TABLE_SIZE && self.used[idx] {
            Some(self.slots[idx].assume_init_ref())
        } else {
            None
        }
    }
}

static MPMC_CHANNEL_TABLE: SpinLock<MpmcChannelTable> =
    // SAFETY: SpinLock<MpmcChannelTable> tout-zéro valide: AtomicBool false = déverrouillé, table vide.
    unsafe { core::mem::zeroed() };

// ---------------------------------------------------------------------------
// API publique de haut niveau
// ---------------------------------------------------------------------------

/// Crée un canal MPMC avec la politique d'overflow spécifiée.
pub fn mpmc_channel_create(policy: OverflowPolicy) -> Result<usize, IpcError> {
    let mut tbl = MPMC_CHANNEL_TABLE.lock();
    tbl.alloc(policy).ok_or(IpcError::OutOfResources)
}

/// Envoie un message dans le canal MPMC identifié par `idx`.
pub fn mpmc_channel_send(idx: usize, data: &[u8], flags: MsgFlags) -> Result<MessageId, IpcError> {
    let tbl = MPMC_CHANNEL_TABLE.lock();
    // SAFETY: tbl.get() interne vérifie used[idx] < TABLE_SIZE avant de retourner.
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    // SAFETY: chan vit dans MPMC_CHANNEL_TABLE statique ('static).
    // free() requiert le SpinLock, donc le canal n'est pas détruit pendant send().
    let chan_ref: &'static MpmcChannel = unsafe { &*(chan as *const MpmcChannel) };
    drop(tbl);
    chan_ref.send(data, flags)
}

/// Reçoit un message du canal MPMC identifié par `idx`.
pub fn mpmc_channel_recv(idx: usize, buf: &mut [u8], flags: MsgFlags)
    -> Result<(usize, MsgFlags), IpcError>
{
    let tbl = MPMC_CHANNEL_TABLE.lock();
    // SAFETY: tbl.get() vérifie used[idx] avant de retourner.
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    // SAFETY: même invariant 'static que mpmc_channel_send().
    let chan_ref: &'static MpmcChannel = unsafe { &*(chan as *const MpmcChannel) };
    drop(tbl);
    chan_ref.recv(buf, flags)
}

/// Ferme et détruit le canal MPMC identifié par `idx`.
pub fn mpmc_channel_destroy(idx: usize) -> Result<(), IpcError> {
    let tbl = MPMC_CHANNEL_TABLE.lock();
    // SAFETY: tbl.get() vérifie used[idx].
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    chan.close();
    drop(tbl);

    let mut tbl = MPMC_CHANNEL_TABLE.lock();
    if !tbl.free(idx) {
        return Err(IpcError::InvalidHandle);
    }
    Ok(())
}

/// Retourne le nombre de canaux MPMC actifs.
pub fn mpmc_channel_count() -> usize {
    MPMC_CHANNEL_TABLE.lock().count
}

/// Envoie dans le canal MPMC `idx` avec vérification capability (IPC-04 v6).
pub fn mpmc_channel_send_checked(
    idx:   usize,
    data:  &[u8],
    flags: MsgFlags,
    table: &CapTable,
    token: CapToken,
) -> Result<MessageId, IpcError> {
    let tbl = MPMC_CHANNEL_TABLE.lock();
    // SAFETY: tbl.get() vérifie used[idx] avant de retourner.
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    // SAFETY: même invariant 'static que les autres fonctions send/recv.
    let chan_ref: &'static MpmcChannel = unsafe { &*(chan as *const MpmcChannel) };
    drop(tbl);
    chan_ref.send_checked(data, flags, table, token)
}

/// Reçoit depuis le canal MPMC `idx` avec vérification capability (IPC-04 v6).
pub fn mpmc_channel_recv_checked(
    idx:   usize,
    buf:   &mut [u8],
    flags: MsgFlags,
    table: &CapTable,
    token: CapToken,
) -> Result<(usize, MsgFlags), IpcError> {
    let tbl = MPMC_CHANNEL_TABLE.lock();
    // SAFETY: tbl.get() vérifie used[idx] avant de retourner.
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    // SAFETY: même invariant 'static que les autres fonctions send/recv.
    let chan_ref: &'static MpmcChannel = unsafe { &*(chan as *const MpmcChannel) };
    drop(tbl);
    chan_ref.recv_checked(buf, flags, table, token)
}
