// ipc/channel/broadcast.rs — Canal de diffusion (one-to-many) pour Exo-OS
//
// Implémente le pattern publish/subscribe : un publish envoie UNE fois et TOUS
// les abonnés reçoivent une copie independante du message.
//
// Architecture :
//   - Liste statique d'abonnés (MAX_BROADCAST_SUBSCRIBERS = 64)
//   - Chaque abonné possède un SpscRing privé
//   - Publication = copie du message dans chaque ring abonné
//   - Pas d'allocation dynamique, pas de Vec/Box
//   - Statistiques per-abonné + globales AtomicU64
//
// Règles respectées :
//   - RÈGLE NO-ALLOC : pas de Vec/Box/Arc/Rc
//   - Isolation des abonnés : un ring plein n'affecte pas les autres (DROP_SUBSCRIBER)
//   - Aucune importation de process/ ou fs/

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicBool, Ordering};
use core::mem::MaybeUninit;

use crate::ipc::core::types::{ChannelId, IpcError, MsgFlags, MessageId, alloc_channel_id, alloc_message_id};
use crate::ipc::core::constants::MAX_MSG_SIZE;
use crate::ipc::ring::spsc::SpscRing;
use crate::ipc::stats::counters::{IPC_STATS, StatEvent};
use crate::scheduler::sync::spinlock::SpinLock;
// IPC-04 (v6) : vérification capability via security::access_control
use crate::security::capability::{CapTable, CapToken, Rights};
use crate::security::access_control::{check_access, ObjectKind, AccessError};

// ---------------------------------------------------------------------------
// Constantes du broadcast
// ---------------------------------------------------------------------------

/// Nombre maximal d'abonnés par canal de broadcast
/// (limité pour contenir le budget mémoire statique — SpscRing par abonné).
pub const MAX_BROADCAST_SUBSCRIBERS: usize = 16;

/// Identifiant d'abonné : index dans le tableau des abonnés
pub type SubscriberId = u32;

/// Valeur sentinelle — abonné invalide / non enregistré
pub const SUBSCRIBER_INVALID: SubscriberId = u32::MAX;

// ---------------------------------------------------------------------------
// Slot abonné
// ---------------------------------------------------------------------------

/// Ring privé d'un abonné avec ses compteurs de performance
#[repr(C, align(64))]
pub struct SubscriberSlot {
    /// Ring SPSC dédié à cet abonné
    ring: SpscRing,
    /// Abonné actif
    active: AtomicBool,
    /// Identifiant opaque (ex: ThreadId du consommateur)
    owner_id: AtomicU64,
    /// Compteur de messages reçus
    msgs_received: AtomicU64,
    /// Compteur de messages droppés (ring plein)
    msgs_dropped: AtomicU64,
    _pad: [u8; 14],
}

// SAFETY: SpscRing est Sync (barrières AtomicU64 internes), tous les autres
// champs sont atomiques.
unsafe impl Sync for SubscriberSlot {}
unsafe impl Send for SubscriberSlot {}

impl SubscriberSlot {
    pub const fn new() -> Self {
        Self {
            ring: SpscRing::new(),
            active: AtomicBool::new(false),
            owner_id: AtomicU64::new(0),
            msgs_received: AtomicU64::new(0),
            msgs_dropped: AtomicU64::new(0),
            _pad: [0u8; 14],
        }
    }

    /// Initialise et active cet abonné.
    pub fn activate(&self, owner_id: u64) {
        self.owner_id.store(owner_id, Ordering::Relaxed);
        // SAFETY: on initialise le ring avant de marquer active
        self.ring.init();
        self.active.store(true, Ordering::Release);
    }

    /// Désactive l'abonné (ne consomme plus les futurs messages).
    pub fn deactivate(&self) {
        self.active.store(false, Ordering::Release);
        self.owner_id.store(0, Ordering::Relaxed);
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// Écrit un message dans le ring de cet abonné.
    /// Retourne `true` si réussi, `false` si ring plein (message droppé).
    pub fn deliver(&self, data: &[u8], flags: MsgFlags) -> bool {
        if !self.is_active() {
            return false;
        }
        match self.ring.push_copy(data, flags) {
            Ok(_) => {
                self.msgs_received.fetch_add(1, Ordering::Relaxed);
                true
            }
            Err(_) => {
                self.msgs_dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Lit un message depuis le ring de cet abonné.
    pub fn consume(&self, buf: &mut [u8]) -> Result<(usize, MsgFlags), IpcError> {
        if !self.is_active() {
            return Err(IpcError::Closed);
        }
        self.ring.pop_into(buf)
    }

    pub fn msgs_received(&self) -> u64 {
        self.msgs_received.load(Ordering::Relaxed)
    }

    pub fn msgs_dropped(&self) -> u64 {
        self.msgs_dropped.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// Statistiques globales du canal broadcast
// ---------------------------------------------------------------------------

#[repr(C, align(64))]
pub struct BroadcastStats {
    pub publishes: AtomicU64,
    pub total_deliveries: AtomicU64,
    pub total_drops: AtomicU64,
    pub subscriber_joins: AtomicU64,
    pub subscriber_leaves: AtomicU64,
    pub bytes_published: AtomicU64,
    _pad: [u8; 16],
}

impl BroadcastStats {
    pub const fn new() -> Self {
        Self {
            publishes: AtomicU64::new(0),
            total_deliveries: AtomicU64::new(0),
            total_drops: AtomicU64::new(0),
            subscriber_joins: AtomicU64::new(0),
            subscriber_leaves: AtomicU64::new(0),
            bytes_published: AtomicU64::new(0),
            _pad: [0u8; 16],
        }
    }

    pub fn snapshot(&self) -> BroadcastStatsSnapshot {
        BroadcastStatsSnapshot {
            publishes: self.publishes.load(Ordering::Relaxed),
            total_deliveries: self.total_deliveries.load(Ordering::Relaxed),
            total_drops: self.total_drops.load(Ordering::Relaxed),
            subscriber_joins: self.subscriber_joins.load(Ordering::Relaxed),
            subscriber_leaves: self.subscriber_leaves.load(Ordering::Relaxed),
            bytes_published: self.bytes_published.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BroadcastStatsSnapshot {
    pub publishes: u64,
    pub total_deliveries: u64,
    pub total_drops: u64,
    pub subscriber_joins: u64,
    pub subscriber_leaves: u64,
    pub bytes_published: u64,
}

// ---------------------------------------------------------------------------
// BroadcastChannel — structure principale
// ---------------------------------------------------------------------------

/// Canal de diffusion : un producteur, N consommateurs indépendants.
///
/// Chaque abonné dispose d'un SpscRing privé. La méthode `publish()` itère
/// sur tous les abonnés actifs et copie le message dans chacun.
/// La livraison est best-effort : si le ring d'un abonné est plein, le message
/// est droppé pour cet abonné uniquement.
#[repr(C, align(64))]
pub struct BroadcastChannel {
    /// Identifiant unique du canal
    pub id: ChannelId,
    /// Tableau statique des slots abonnés
    subscribers: [SubscriberSlot; MAX_BROADCAST_SUBSCRIBERS],
    /// Bitmap de présence des abonnés (protection de la table sous SpinLock)
    subscriber_bitmap: AtomicU64,
    /// Nombre d'abonnés actifs
    subscriber_count: AtomicU32,
    /// Statistiques globales
    pub stats: BroadcastStats,
    /// Canal fermé
    closed: AtomicU32,
    _pad: [u8; 24],
}

// SAFETY: SubscriberSlot est Sync, BroadcastStats est Sync (AtomicU64)
unsafe impl Sync for BroadcastChannel {}
unsafe impl Send for BroadcastChannel {}

impl BroadcastChannel {
    pub const fn new_uninit() -> Self {
        // SAFETY: SubscriberSlot::new() est const, donc valide
        const INIT_SLOT: SubscriberSlot = SubscriberSlot::new();
        Self {
            id: ChannelId::DANGLING,
            subscribers: [INIT_SLOT; MAX_BROADCAST_SUBSCRIBERS],
            subscriber_bitmap: AtomicU64::new(0),
            subscriber_count: AtomicU32::new(0),
            stats: BroadcastStats::new(),
            closed: AtomicU32::new(0),
            _pad: [0u8; 24],
        }
    }

    pub fn init(&mut self) {
        self.id = alloc_channel_id();
    }

    #[inline]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire) != 0
    }

    pub fn close(&self) {
        self.closed.store(1, Ordering::Release);
        // Désactiver tous les abonnés
        for i in 0..MAX_BROADCAST_SUBSCRIBERS {
            if self.subscribers[i].is_active() {
                self.subscribers[i].deactivate();
            }
        }
    }

    // -----------------------------------------------------------------------
    // Gestion des abonnés
    // -----------------------------------------------------------------------

    /// S'abonner au canal.
    /// Retourne l'identifiant d'abonné (index) ou `None` si saturé.
    pub fn subscribe(&self, owner_id: u64) -> Option<SubscriberId> {
        if self.is_closed() {
            return None;
        }
        // Chercher un slot libre
        for i in 0..MAX_BROADCAST_SUBSCRIBERS {
            if !self.subscribers[i].is_active() {
                // Tentative CAS sur le bitmap (bit i = 0 → 1)
                let mask = 1u64 << (i % 64);
                // On utilise le spinlock implicitement via la sémantique atomique
                // de subscriber.activate() — le bitmap est purement informatif ici.
                self.subscribers[i].activate(owner_id);
                self.subscriber_bitmap.fetch_or(mask, Ordering::AcqRel);
                self.subscriber_count.fetch_add(1, Ordering::Relaxed);
                self.stats.subscriber_joins.fetch_add(1, Ordering::Relaxed);
                return Some(i as SubscriberId);
            }
        }
        None
    }

    /// Se désabonner du canal.
    pub fn unsubscribe(&self, sub_id: SubscriberId) -> bool {
        let idx = sub_id as usize;
        if idx >= MAX_BROADCAST_SUBSCRIBERS {
            return false;
        }
        if self.subscribers[idx].is_active() {
            self.subscribers[idx].deactivate();
            let mask = 1u64 << (idx % 64);
            self.subscriber_bitmap.fetch_and(!mask, Ordering::AcqRel);
            self.subscriber_count.fetch_sub(1, Ordering::Relaxed);
            self.stats.subscriber_leaves.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    pub fn subscriber_count(&self) -> u32 {
        self.subscriber_count.load(Ordering::Relaxed)
    }

    // -----------------------------------------------------------------------
    // Publication
    // -----------------------------------------------------------------------

    /// Publie `data` vers TOUS les abonnés actifs.
    ///
    /// Retourne `(MessageId, delivered, dropped)` :
    ///   - `delivered` : nombre d'abonnés ayant reçu le message
    ///   - `dropped`   : nombre d'abonnés dont le ring était plein
    pub fn publish(&self, data: &[u8], flags: MsgFlags)
        -> Result<(MessageId, u32, u32), IpcError>
    {
        if self.is_closed() {
            return Err(IpcError::Closed);
        }
        if data.len() > MAX_MSG_SIZE {
            return Err(IpcError::MessageTooLarge);
        }

        let mid = alloc_message_id();
        let mut delivered: u32 = 0;
        let mut dropped: u32 = 0;

        // Itérer sur les abonnés actifs via le bitmap
        let mut bitmap = self.subscriber_bitmap.load(Ordering::Acquire);
        while bitmap != 0 {
            let bit = bitmap.trailing_zeros() as usize;
            bitmap &= bitmap - 1; // clear lowest set bit

            if bit < MAX_BROADCAST_SUBSCRIBERS && self.subscribers[bit].is_active() {
                if self.subscribers[bit].deliver(data, flags) {
                    delivered += 1;
                } else {
                    dropped += 1;
                }
            }
        }

        self.stats.publishes.fetch_add(1, Ordering::Relaxed);
        self.stats.total_deliveries.fetch_add(delivered as u64, Ordering::Relaxed);
        self.stats.total_drops.fetch_add(dropped as u64, Ordering::Relaxed);
        self.stats.bytes_published.fetch_add(data.len() as u64, Ordering::Relaxed);

        if delivered > 0 {
            IPC_STATS.record(StatEvent::MessageSent);
        }

        Ok((mid, delivered, dropped))
    }

    // -----------------------------------------------------------------------
    // Consommation (par abonné)
    // -----------------------------------------------------------------------

    /// L'abonné `sub_id` lit son prochain message.
    pub fn recv(&self, sub_id: SubscriberId, buf: &mut [u8]) -> Result<(usize, MsgFlags), IpcError> {
        let idx = sub_id as usize;
        if idx >= MAX_BROADCAST_SUBSCRIBERS {
            return Err(IpcError::InvalidHandle);
        }
        let result = self.subscribers[idx].consume(buf)?;
        IPC_STATS.record(StatEvent::MessageReceived);
        Ok(result)
    }

    /// Snapshot des statistiques globales.
    pub fn snapshot_stats(&self) -> BroadcastStatsSnapshot {
        self.stats.snapshot()
    }

    /// Per-abonné : nombre de messages reçus / droppés.
    pub fn subscriber_stats(&self, sub_id: SubscriberId) -> Option<(u64, u64)> {
        let idx = sub_id as usize;
        if idx >= MAX_BROADCAST_SUBSCRIBERS || !self.subscribers[idx].is_active() {
            return None;
        }
        Some((
            self.subscribers[idx].msgs_received(),
            self.subscribers[idx].msgs_dropped(),
        ))
    }

    // -----------------------------------------------------------------------
    // PUBLICATION/ABONNEMENT CAP-CHECKED — IPC-04 (v6)
    // -----------------------------------------------------------------------

    /// Publie vers tous les abonnés avec vérification capability (RÈGLE IPC-04 v6).
    ///
    /// # Droits requis : `Rights::IPC_SEND`
    #[inline]
    pub fn publish_checked(
        &self,
        data:  &[u8],
        flags: MsgFlags,
        table: &CapTable,
        token: CapToken,
    ) -> Result<(MessageId, u32, u32), IpcError> {
        // IPC-04 (v6) : vérification capability — appel direct security/access_control/
        check_access(table, token, ObjectKind::IpcChannel, Rights::IPC_SEND, "ipc::broadcast")
            .map_err(|e| match e {
                AccessError::ObjectNotFound { .. } => IpcError::EndpointNotFound,
                _ => IpcError::PermissionDenied,
            })?;
        self.publish(data, flags)
    }

    /// S'abonne au canal avec vérification capability (RÈGLE IPC-04 v6).
    ///
    /// # Droits requis : `Rights::IPC_RECV`
    #[inline]
    pub fn subscribe_checked(
        &self,
        owner_id: u64,
        table:    &CapTable,
        token:    CapToken,
    ) -> Result<Option<SubscriberId>, IpcError> {
        // IPC-04 (v6) : vérification capability — appel direct security/access_control/
        check_access(table, token, ObjectKind::IpcChannel, Rights::IPC_RECV, "ipc::broadcast")
            .map_err(|e| match e {
                AccessError::ObjectNotFound { .. } => IpcError::EndpointNotFound,
                _ => IpcError::PermissionDenied,
            })?;
        Ok(self.subscribe(owner_id))
    }

    /// L'abonné `sub_id` lit son prochain message, avec vérification capability.
    ///
    /// # Droits requis : `Rights::IPC_RECV`
    #[inline]
    pub fn recv_checked(
        &self,
        sub_id: SubscriberId,
        buf:    &mut [u8],
        table:  &CapTable,
        token:  CapToken,
    ) -> Result<(usize, MsgFlags), IpcError> {
        // IPC-04 (v6) : vérification capability — appel direct security/access_control/
        check_access(table, token, ObjectKind::IpcChannel, Rights::IPC_RECV, "ipc::broadcast")
            .map_err(|e| match e {
                AccessError::ObjectNotFound { .. } => IpcError::EndpointNotFound,
                _ => IpcError::PermissionDenied,
            })?;
        self.recv(sub_id, buf)
    }
}

// ---------------------------------------------------------------------------
// Table statique globale de canaux broadcast
// ---------------------------------------------------------------------------

pub const BROADCAST_CHANNEL_TABLE_SIZE: usize = 16;

struct BroadcastChannelTable {
    slots: [MaybeUninit<BroadcastChannel>; BROADCAST_CHANNEL_TABLE_SIZE],
    used: [bool; BROADCAST_CHANNEL_TABLE_SIZE],
    count: usize,
}

// SAFETY: accès protégé par SpinLock
unsafe impl Send for BroadcastChannelTable {}

impl BroadcastChannelTable {
    const fn new() -> Self {
        // SAFETY: mem::zeroed() évite la limite mémoire du const-eval pour grands tableaux.
        unsafe { core::mem::zeroed() }
    }

    fn alloc(&mut self) -> Option<usize> {
        for i in 0..BROADCAST_CHANNEL_TABLE_SIZE {
            if !self.used[i] {
                let mut chan = BroadcastChannel::new_uninit();
                chan.init();
                self.slots[i].write(chan);
                self.used[i] = true;
                self.count += 1;
                return Some(i);
            }
        }
        None
    }

    fn free(&mut self, idx: usize) -> bool {
        if idx < BROADCAST_CHANNEL_TABLE_SIZE && self.used[idx] {
            // SAFETY: used[idx] garantit l'init
            unsafe { self.slots[idx].assume_init_drop() };
            self.used[idx] = false;
            self.count -= 1;
            true
        } else {
            false
        }
    }

    unsafe fn get(&self, idx: usize) -> Option<&BroadcastChannel> {
        if idx < BROADCAST_CHANNEL_TABLE_SIZE && self.used[idx] {
            Some(self.slots[idx].assume_init_ref())
        } else {
            None
        }
    }
}

// SAFETY: La section .bss est zéro-initialisée par le bootloader.
// SpinLock<BroadcastChannelTable> tout-zéro = valide (lock free, table vide).
// MaybeUninit::uninit() = 0 byte de const-eval, la mémoire vient du .bss.
static BROADCAST_TABLE: core::mem::MaybeUninit<SpinLock<BroadcastChannelTable>> =
    core::mem::MaybeUninit::uninit();

#[inline(always)]
fn broadcast_table() -> &'static SpinLock<BroadcastChannelTable> {
    // SAFETY: .bss est zéro-initialisé au boot. SpinLock all-zeros = déverrouillé.
    unsafe { BROADCAST_TABLE.assume_init_ref() }
}

// ---------------------------------------------------------------------------
// API publique de haut niveau
// ---------------------------------------------------------------------------

/// Crée un canal de broadcast. Retourne son index.
pub fn broadcast_create() -> Result<usize, IpcError> {
    let mut tbl = broadcast_table().lock();
    tbl.alloc().ok_or(IpcError::OutOfResources)
}

/// S'abonner au canal broadcast `chan_idx`. Retourne le SubscriberId.
pub fn broadcast_subscribe(chan_idx: usize, owner_id: u64) -> Result<SubscriberId, IpcError> {
    let tbl = broadcast_table().lock();
    // SAFETY: tbl.get() vérifie used[chan_idx] avant de retourner.
    let chan = unsafe { tbl.get(chan_idx) }.ok_or(IpcError::InvalidHandle)?;
    // SAFETY: chan vit dans BROADCAST_TABLE statique ('static).
    // free() requiert le SpinLock, donc la durée de vie est garantie.
    let chan_ref: &'static BroadcastChannel = unsafe { &*(chan as *const BroadcastChannel) };
    drop(tbl);
    chan_ref.subscribe(owner_id).ok_or(IpcError::OutOfResources)
}

/// Se désabonner du canal `chan_idx`.
pub fn broadcast_unsubscribe(chan_idx: usize, sub_id: SubscriberId) -> Result<(), IpcError> {
    let tbl = broadcast_table().lock();
    // SAFETY: tbl.get() vérifie used[chan_idx] avant de retourner.
    let chan = unsafe { tbl.get(chan_idx) }.ok_or(IpcError::InvalidHandle)?;
    // SAFETY: même invariant 'static que broadcast_subscribe().
    let chan_ref: &'static BroadcastChannel = unsafe { &*(chan as *const BroadcastChannel) };
    drop(tbl);
    if chan_ref.unsubscribe(sub_id) { Ok(()) } else { Err(IpcError::InvalidHandle) }
}

/// Publier un message vers tous les abonnés du canal `chan_idx`.
pub fn broadcast_publish(chan_idx: usize, data: &[u8], flags: MsgFlags)
    -> Result<(MessageId, u32, u32), IpcError>
{
    let tbl = broadcast_table().lock();
    // SAFETY: tbl.get() vérifie used[chan_idx] avant de retourner.
    let chan = unsafe { tbl.get(chan_idx) }.ok_or(IpcError::InvalidHandle)?;
    // SAFETY: même invariant 'static que broadcast_subscribe().
    let chan_ref: &'static BroadcastChannel = unsafe { &*(chan as *const BroadcastChannel) };
    drop(tbl);
    chan_ref.publish(data, flags)
}

/// Recevoir un message pour l'abonné `sub_id` sur le canal `chan_idx`.
pub fn broadcast_recv(chan_idx: usize, sub_id: SubscriberId, buf: &mut [u8])
    -> Result<(usize, MsgFlags), IpcError>
{
    let tbl = broadcast_table().lock();
    // SAFETY: tbl.get() vérifie used[chan_idx] avant de retourner.
    let chan = unsafe { tbl.get(chan_idx) }.ok_or(IpcError::InvalidHandle)?;
    // SAFETY: même invariant 'static que broadcast_subscribe().
    let chan_ref: &'static BroadcastChannel = unsafe { &*(chan as *const BroadcastChannel) };
    drop(tbl);
    chan_ref.recv(sub_id, buf)
}

/// Ferme et détruit le canal broadcast.
pub fn broadcast_destroy(chan_idx: usize) -> Result<(), IpcError> {
    let tbl = broadcast_table().lock();
    // SAFETY: tbl.get() vérifie used[chan_idx] avant de retourner.
    let chan = unsafe { tbl.get(chan_idx) }.ok_or(IpcError::InvalidHandle)?;
    chan.close();
    drop(tbl);
    let mut tbl = broadcast_table().lock();
    tbl.free(chan_idx);
    Ok(())
}

// ---------------------------------------------------------------------------
// API cap-checked — IPC-04 (v6)
// ---------------------------------------------------------------------------

/// Publier avec vérification capability (IPC-04 v6).
pub fn broadcast_publish_checked(
    chan_idx: usize,
    data:     &[u8],
    flags:    MsgFlags,
    table:    &CapTable,
    token:    CapToken,
) -> Result<(MessageId, u32, u32), IpcError> {
    let tbl = broadcast_table().lock();
    // SAFETY: tbl.get() vérifie used[chan_idx] avant de retourner.
    let chan = unsafe { tbl.get(chan_idx) }.ok_or(IpcError::InvalidHandle)?;
    // SAFETY: même invariant 'static que broadcast_subscribe().
    let chan_ref: &'static BroadcastChannel = unsafe { &*(chan as *const BroadcastChannel) };
    drop(tbl);
    chan_ref.publish_checked(data, flags, table, token)
}

/// S'abonner avec vérification capability (IPC-04 v6).
pub fn broadcast_subscribe_checked(
    chan_idx:  usize,
    owner_id:  u64,
    table:     &CapTable,
    token:     CapToken,
) -> Result<SubscriberId, IpcError> {
    let tbl = broadcast_table().lock();
    // SAFETY: tbl.get() vérifie used[chan_idx] avant de retourner.
    let chan = unsafe { tbl.get(chan_idx) }.ok_or(IpcError::InvalidHandle)?;
    // SAFETY: même invariant 'static que broadcast_subscribe().
    let chan_ref: &'static BroadcastChannel = unsafe { &*(chan as *const BroadcastChannel) };
    drop(tbl);
    chan_ref.subscribe_checked(owner_id, table, token)?
        .ok_or(IpcError::OutOfResources)
}

/// Recevoir avec vérification capability (IPC-04 v6).
pub fn broadcast_recv_checked(
    chan_idx: usize,
    sub_id:   SubscriberId,
    buf:      &mut [u8],
    table:    &CapTable,
    token:    CapToken,
) -> Result<(usize, MsgFlags), IpcError> {
    let tbl = broadcast_table().lock();
    // SAFETY: tbl.get() vérifie used[chan_idx] avant de retourner.
    let chan = unsafe { tbl.get(chan_idx) }.ok_or(IpcError::InvalidHandle)?;
    // SAFETY: même invariant 'static que broadcast_subscribe().
    let chan_ref: &'static BroadcastChannel = unsafe { &*(chan as *const BroadcastChannel) };
    drop(tbl);
    chan_ref.recv_checked(sub_id, buf, table, token)
}
